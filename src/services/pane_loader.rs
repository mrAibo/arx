use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::mpsc;

use crate::app::Pane;
use crate::vfs::{Entry, Location, ProviderContinuation, ProviderInstanceKey, ProviderRegistry};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PaneLoadId(pub u64);

/// Pane-layer continuation identity.
///
/// Wraps the provider-side opaque continuation together with the concrete
/// provider instance, the exact pane location, and the pane generation that
/// produced it. The provider never receives `PaneLoadId` — generation and
/// location correlation are pane-layer concerns only (see S3-14/S3-15).
// ponytail: data-model only; stale-page acceptance/rejection is S3-15
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneListingContinuation {
    pub provider_continuation: ProviderContinuation,
    pub provider_instance: ProviderInstanceKey,
    pub location: Location,
    pub generation: PaneLoadId,
}

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

    // ── S3-14: pane-layer continuation identity model ──

    fn s3_loc(target: &str, bucket: Option<&str>, prefix: &str) -> Location {
        Location::S3 {
            target: target.to_string(),
            bucket: bucket.map(|b| b.to_string()),
            prefix: prefix.to_string(),
        }
    }

    #[test]
    fn exact_wrapper_roundtrip() {
        // token + target + bucket + prefix must survive verbatim (no normalization)
        let cont = PaneListingContinuation {
            provider_continuation: ProviderContinuation {
                token: "  opaque+/=token 日本語  ".to_string(),
            },
            provider_instance: ProviderInstanceKey::S3Target(" prod-target ".to_string()),
            location: s3_loc(" prod-target ", Some(" my-bucket "), " photos/2024/"),
            generation: PaneLoadId(42),
        };
        assert_eq!(
            cont.provider_continuation.token,
            "  opaque+/=token 日本語  "
        );
        assert_eq!(
            cont.provider_instance,
            ProviderInstanceKey::S3Target(" prod-target ".to_string())
        );
        assert_eq!(
            cont.location,
            s3_loc(" prod-target ", Some(" my-bucket "), " photos/2024/")
        );
        assert_eq!(cont.generation, PaneLoadId(42));
    }

    #[test]
    fn concrete_provider_instance_distinction() {
        let base = PaneListingContinuation {
            provider_continuation: ProviderContinuation {
                token: "tok".to_string(),
            },
            provider_instance: ProviderInstanceKey::SftpHost("prod".to_string()),
            location: Location::Sftp {
                host: "prod".to_string(),
                path: "/srv".to_string(),
            },
            generation: PaneLoadId(42),
        };
        let other = PaneListingContinuation {
            provider_instance: ProviderInstanceKey::SftpHost("backup".to_string()),
            ..base.clone()
        };
        assert_ne!(base, other);
    }

    #[test]
    fn generation_distinction() {
        let base = PaneListingContinuation {
            provider_continuation: ProviderContinuation {
                token: "tok".to_string(),
            },
            provider_instance: ProviderInstanceKey::SftpHost("prod".to_string()),
            location: Location::Sftp {
                host: "prod".to_string(),
                path: "/srv".to_string(),
            },
            generation: PaneLoadId(42),
        };
        let other = PaneListingContinuation {
            generation: PaneLoadId(43),
            ..base.clone()
        };
        assert_ne!(base, other);
    }

    #[test]
    fn location_distinction() {
        let base = PaneListingContinuation {
            provider_continuation: ProviderContinuation {
                token: "tok".to_string(),
            },
            provider_instance: ProviderInstanceKey::SftpHost("prod".to_string()),
            location: Location::Sftp {
                host: "prod".to_string(),
                path: "/srv/a".to_string(),
            },
            generation: PaneLoadId(42),
        };
        let other = PaneListingContinuation {
            location: Location::Sftp {
                host: "prod".to_string(),
                path: "/srv/b".to_string(),
            },
            ..base.clone()
        };
        assert_ne!(base, other);
    }
}
