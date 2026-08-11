use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::mpsc;

use crate::effects::{Effect, EffectEvent};
use crate::process::ProcessService;
use crate::vfs::{CancellationFlag, Location, ProviderRegistry};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EffectId(pub u64);

/// Logical slot where only the newest request matters.
///
/// Two previews can be in flight at once, but when Preview #42 is replaced by
/// Preview #43, response #42 is stale even if it finishes later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EffectLane {
    GlobalProcess,
    TmuxDiscovery,
    GitStatus,
    Preview,
    RemoteEdit,
    Workspace,
    LeftPane,
    RightPane,
    Infrastructure,
    Tree,
}

/// Snapshot of the state an effect was requested for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectScope {
    Global,
    Location(Location),
    Workspace { left: Location, right: Location },
}

#[derive(Debug, Clone)]
pub struct EffectRequest {
    pub id: EffectId,
    pub lane: EffectLane,
    pub scope: EffectScope,
    pub effect: Effect,
}

#[derive(Debug, Clone)]
pub struct EffectResponse {
    pub id: EffectId,
    pub lane: EffectLane,
    pub scope: EffectScope,
    pub event: EffectEvent,
}

#[derive(Clone)]
pub struct EffectDispatcher {
    next_id: Arc<AtomicU64>,
    tx: mpsc::UnboundedSender<EffectResponse>,
    registry: ProviderRegistry,
    cancellations: Arc<Mutex<BTreeMap<EffectId, CancellationFlag>>>,
}

impl EffectDispatcher {
    pub fn channel(registry: ProviderRegistry) -> (Self, mpsc::UnboundedReceiver<EffectResponse>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                next_id: Arc::new(AtomicU64::new(1)),
                tx,
                registry,
                cancellations: Arc::new(Mutex::new(BTreeMap::new())),
            },
            rx,
        )
    }

    pub fn dispatch(&self, lane: EffectLane, scope: EffectScope, effect: Effect) -> EffectId {
        let id = EffectId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let request = EffectRequest {
            id,
            lane,
            scope,
            effect,
        };
        let tx = self.tx.clone();
        let registry = self.registry.clone();
        let cancellation = CancellationFlag::default();
        self.cancellations
            .lock()
            .expect("effect cancellation lock")
            .insert(id, cancellation.clone());
        let cancellations = Arc::clone(&self.cancellations);
        tokio::spawn(async move {
            let event = ProcessService::execute_with_registry_cancellable(
                request.effect,
                &registry,
                &cancellation,
            )
            .await;
            if tx
                .send(EffectResponse {
                    id: request.id,
                    lane: request.lane,
                    scope: request.scope,
                    event,
                })
                .is_err()
            {
                cancellations
                    .lock()
                    .expect("effect cancellation lock")
                    .remove(&request.id);
            }
        });
        id
    }

    pub fn cancel(&self, id: EffectId) -> bool {
        let cancellation = self
            .cancellations
            .lock()
            .expect("effect cancellation lock")
            .get(&id)
            .cloned();
        if let Some(cancellation) = cancellation {
            cancellation.cancel();
            true
        } else {
            false
        }
    }

    /// Removes cancellation state only after the event loop receives the response.
    pub fn finish(&self, id: EffectId) -> Option<CancellationFlag> {
        self.cancellations
            .lock()
            .expect("effect cancellation lock")
            .remove(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ids_are_monotonic() {
        let registry = ProviderRegistry::new();
        let (dispatcher, _rx) = EffectDispatcher::channel(registry);
        let first = dispatcher.dispatch(
            EffectLane::GlobalProcess,
            EffectScope::Global,
            Effect::SpawnShell {
                command: "true".into(),
            },
        );
        let second = dispatcher.dispatch(
            EffectLane::GlobalProcess,
            EffectScope::Global,
            Effect::SpawnShell {
                command: "true".into(),
            },
        );
        assert!(second.0 > first.0);
    }

    #[tokio::test]
    async fn completed_response_remains_cancellable_until_finish() {
        let registry = ProviderRegistry::new();
        let (dispatcher, mut rx) = EffectDispatcher::channel(registry);
        let id = dispatcher.dispatch(
            EffectLane::GlobalProcess,
            EffectScope::Global,
            Effect::SpawnShell {
                command: "true".into(),
            },
        );

        let response = rx.recv().await.expect("effect response");
        assert_eq!(response.id, id);
        assert!(dispatcher.cancel(id));
        assert!(dispatcher.finish(id).unwrap().is_cancelled());
        assert!(!dispatcher.cancel(id));
    }
}
