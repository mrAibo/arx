use super::{AppState, SyncLaunchId, SyncPlanId};
use arx::effect_dispatcher::{EffectDispatcher, EffectResponse};
use arx::jobs::{JobEvent, JobManager};
use arx::services::{
    PaneLoadResponse, PaneLoader, PaneNextPageResponse, WorkspaceScanResponse, WorkspaceScanner,
    WorkspaceSyncController,
};
use arx::workspace_sync_verification::SyncVerificationEvent;
use tokio::sync::mpsc;
use tokio::time::Interval;

#[derive(Clone)]
pub(super) struct SyncUiRuntime {
    pub(super) controller: WorkspaceSyncController,
    pub(super) jobs: JobManager,
    pub(super) job_events: mpsc::UnboundedSender<JobEvent>,
    pub(super) verification_events: mpsc::UnboundedSender<SyncVerificationEvent>,
    pub(super) launch_events: mpsc::UnboundedSender<SyncLaunchResponse>,
    /// Single persistent transfer queue runtime. Copy/Move route here; it owns
    /// the only transfer executor and scheduler keyed by the same JobId.
    pub(super) transfers: arx::transfer_queue_runtime::TransferQueueRuntime,
}

pub(super) struct SyncLaunchResponse {
    pub(super) launch_id: SyncLaunchId,
    pub(super) plan_id: SyncPlanId,
    pub(super) result: Result<String, String>,
}

pub(super) enum RuntimeEvent {
    WorkspaceScan(WorkspaceScanResponse),
    PaneLoad(PaneLoadResponse),
    PaneNextPage(PaneNextPageResponse),
    Effect(EffectResponse),
    SyncLaunch(SyncLaunchResponse),
    Verification(SyncVerificationEvent),
    Job(JobEvent),
    Tick,
}

pub(super) struct TuiRuntime {
    pub(super) pane_loader: PaneLoader,
    pub(super) workspace_scanner: WorkspaceScanner,
    pub(super) effect_dispatcher: EffectDispatcher,
    pub(super) job_manager: JobManager,
    pub(super) sync: SyncUiRuntime,
    pane_load_rx: mpsc::UnboundedReceiver<PaneLoadResponse>,
    pane_next_page_rx: mpsc::UnboundedReceiver<PaneNextPageResponse>,
    workspace_scan_rx: mpsc::UnboundedReceiver<WorkspaceScanResponse>,
    effect_rx: mpsc::UnboundedReceiver<EffectResponse>,
    sync_launch_rx: mpsc::UnboundedReceiver<SyncLaunchResponse>,
    verification_rx: mpsc::UnboundedReceiver<SyncVerificationEvent>,
    job_rx: mpsc::UnboundedReceiver<JobEvent>,
    tick: Interval,
}

impl TuiRuntime {
    pub(super) fn new(state: &mut AppState, config: &arx::config::ArxConfig) -> Self {
        state.registry.register_s3_targets(&config.s3.targets);
        state
            .registry
            .register_webdav_targets(&config.webdav.targets);

        let (pane_loader, pane_load_rx, pane_next_page_rx) =
            PaneLoader::channel(state.registry.clone());
        let (workspace_scanner, workspace_scan_rx) =
            WorkspaceScanner::channel(state.registry.clone());
        let (effect_dispatcher, effect_rx) = EffectDispatcher::channel(state.registry.clone());

        // JobManager is the runtime source of truth. AppState.jobs is only a render snapshot.
        let job_manager = JobManager::new();
        let (job_tx, job_rx) = mpsc::unbounded_channel::<JobEvent>();
        // Bind the one runtime manager + channel into AppState; remote-edit observers
        // publish into the same authority that drives the Jobs UI.
        state.job_manager = Some(job_manager.clone());
        state.job_events = Some(job_tx.clone());

        let (verification_tx, verification_rx) = mpsc::unbounded_channel();
        let (sync_launch_tx, sync_launch_rx) = mpsc::unbounded_channel();
        let sync = SyncUiRuntime {
            controller: WorkspaceSyncController::new(state.registry.clone()),
            jobs: job_manager.clone(),
            job_events: job_tx.clone(),
            verification_events: verification_tx,
            launch_events: sync_launch_tx,
            transfers: arx::transfer_queue_runtime::TransferQueueRuntime::new(
                job_manager.clone(),
                job_tx,
                state.registry.clone(),
                arx::transfer_queue::TransferQueueConfig::new(config.transfer.concurrency)
                    .unwrap_or_default(),
            ),
        };

        Self {
            pane_loader,
            workspace_scanner,
            effect_dispatcher,
            job_manager,
            sync,
            pane_load_rx,
            pane_next_page_rx,
            workspace_scan_rx,
            effect_rx,
            sync_launch_rx,
            verification_rx,
            job_rx,
            tick: tokio::time::interval(std::time::Duration::from_millis(50)),
        }
    }

    pub(super) async fn next_event(&mut self) -> RuntimeEvent {
        tokio::select! {
            Some(response) = self.workspace_scan_rx.recv() => RuntimeEvent::WorkspaceScan(response),
            Some(response) = self.pane_load_rx.recv() => RuntimeEvent::PaneLoad(response),
            Some(response) = self.pane_next_page_rx.recv() => RuntimeEvent::PaneNextPage(response),
            Some(response) = self.effect_rx.recv() => RuntimeEvent::Effect(response),
            Some(response) = self.sync_launch_rx.recv() => RuntimeEvent::SyncLaunch(response),
            Some(event) = self.verification_rx.recv() => RuntimeEvent::Verification(event),
            Some(event) = self.job_rx.recv() => RuntimeEvent::Job(event),
            _ = self.tick.tick() => RuntimeEvent::Tick,
        }
    }
}

#[cfg(test)]
mod tests {
    const SOURCE: &str = include_str!("runtime.rs");
    const TUI_SOURCE: &str = include_str!("../tui.rs");

    fn production_source() -> &'static str {
        SOURCE
            .split("#[cfg(test)]\nmod tests")
            .next()
            .expect("production runtime source")
    }

    #[test]
    fn runtime_event_contract_has_all_eight_variants() {
        let production = production_source();
        for variant in [
            "WorkspaceScan(WorkspaceScanResponse)",
            "PaneLoad(PaneLoadResponse)",
            "PaneNextPage(PaneNextPageResponse)",
            "Effect(EffectResponse)",
            "SyncLaunch(SyncLaunchResponse)",
            "Verification(SyncVerificationEvent)",
            "Job(JobEvent)",
            "Tick,",
        ] {
            assert!(
                production.contains(variant),
                "missing RuntimeEvent::{variant}"
            );
        }
    }

    #[test]
    fn runtime_source_only_multiplexes_events() {
        let production = production_source();
        assert_eq!(production.matches("tokio::select!").count(), 1);
        assert_eq!(production.matches(".recv()").count(), 7);
        for forbidden in [
            "pane_responses::",
            "effect_responses::",
            "workspace_responses::",
            "job_responses::",
            "event::read",
            "event::poll",
            "DesktopService::notify",
        ] {
            assert!(!production.contains(forbidden), "runtime calls {forbidden}");
        }
    }

    #[test]
    fn production_has_one_runtime_authority() {
        let production_tui = TUI_SOURCE
            .split("#[cfg(test)]\nmod tests")
            .next()
            .expect("production tui source");
        assert_eq!(production_tui.matches("TuiRuntime::new(").count(), 1);
        for constructor in [
            "JobManager::new()",
            "PaneLoader::channel",
            "WorkspaceScanner::channel",
            "EffectDispatcher::channel",
            "WorkspaceSyncController::new",
            "TransferQueueRuntime::new",
        ] {
            assert_eq!(production_source().matches(constructor).count(), 1);
            assert_eq!(production_tui.matches(constructor).count(), 0);
        }
    }
}
