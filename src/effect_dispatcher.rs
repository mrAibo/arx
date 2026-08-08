use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::mpsc;

use crate::effects::{Effect, EffectEvent};
use crate::process::ProcessService;
use crate::vfs::Location;

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
}

impl EffectDispatcher {
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<EffectResponse>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                next_id: Arc::new(AtomicU64::new(1)),
                tx,
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
        tokio::spawn(async move {
            let event = ProcessService::execute(request.effect).await;
            let _ = tx.send(EffectResponse {
                id: request.id,
                lane: request.lane,
                scope: request.scope,
                event,
            });
        });
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ids_are_monotonic() {
        let (dispatcher, _rx) = EffectDispatcher::channel();
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
}
