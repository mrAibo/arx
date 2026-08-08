use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::mpsc;

use crate::app::Pane;
use crate::vfs::{Entry, Location, ProviderRegistry};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PaneLoadId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneLoadPurpose {
    Refresh,
    Navigate { remember_current: bool },
    HistoryBack,
}

#[derive(Debug)]
pub struct PaneLoadResponse {
    pub id: PaneLoadId,
    pub pane: Pane,
    pub location: Location,
    pub purpose: PaneLoadPurpose,
    pub result: std::io::Result<Vec<Entry>>,
}

/// Async VFS directory loader.
///
/// Every request owns a concrete location snapshot. AppState applies a result
/// only when both its generation id and location still match the pane. This
/// prevents a slow SFTP response from overwriting a newer navigation target.
#[derive(Clone)]
pub struct PaneLoader {
    registry: ProviderRegistry,
    next_id: Arc<AtomicU64>,
    tx: mpsc::UnboundedSender<PaneLoadResponse>,
}

impl PaneLoader {
    pub fn channel(
        registry: ProviderRegistry,
    ) -> (Self, mpsc::UnboundedReceiver<PaneLoadResponse>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                registry,
                next_id: Arc::new(AtomicU64::new(1)),
                tx,
            },
            rx,
        )
    }

    pub fn load(&self, pane: Pane, location: Location, purpose: PaneLoadPurpose) -> PaneLoadId {
        let id = PaneLoadId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let registry = self.registry.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = registry.list_location_async(&location).await;
            let _ = tx.send(PaneLoadResponse {
                id,
                pane,
                location,
                purpose,
                result,
            });
        });
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_load_is_async_and_correlated() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("a.txt"), b"a")
            .await
            .unwrap();

        let (loader, mut rx) = PaneLoader::channel(crate::vfs::default_registry());
        let location = Location::Local(dir.path().to_path_buf());
        let id = loader.load(Pane::Left, location.clone(), PaneLoadPurpose::Refresh);
        let response = rx.recv().await.expect("pane response");

        assert_eq!(response.id, id);
        assert_eq!(response.pane, Pane::Left);
        assert_eq!(response.location, location);
        assert_eq!(response.purpose, PaneLoadPurpose::Refresh);
        assert_eq!(response.result.unwrap().len(), 1);
    }
}
