use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::mpsc;

use crate::app::Pane;
use crate::vfs::{
    ListedEntry, Location, ProviderContinuation, ProviderInstanceKey, ProviderRegistry,
};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneLoadPage {
    pub entries: Vec<ListedEntry>,
    pub continuation: Option<PaneListingContinuation>,
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
    pub result: std::io::Result<PaneLoadPage>,
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
            let result = registry
                .list_page(&location, None)
                .await
                .map(|page| PaneLoadPage {
                    entries: page.entries,
                    continuation: page.continuation.map(|provider_continuation| {
                        PaneListingContinuation {
                            provider_continuation,
                            provider_instance: ProviderRegistry::instance_key_for_location(
                                &location,
                            ),
                            location: location.clone(),
                            generation: id,
                        }
                    }),
                });
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
    use crate::vfs::{
        CapabilitySet, Entry, EntryIdentity, EntryKind, ProviderListingPage, VfsProvider,
    };

    #[derive(Debug)]
    struct PageProvider {
        page: ProviderListingPage,
    }

    #[async_trait::async_trait]
    impl VfsProvider for PageProvider {
        fn list(&self, _path: &str) -> std::io::Result<Vec<Entry>> {
            panic!("legacy list must not be called")
        }

        async fn list_page(
            &self,
            _location: &Location,
            continuation: Option<&ProviderContinuation>,
        ) -> std::io::Result<ProviderListingPage> {
            assert!(continuation.is_none());
            Ok(self.page.clone())
        }

        fn read_head(&self, _path: &str, _lines: usize) -> std::io::Result<Vec<String>> {
            panic!("read_head must not be called")
        }

        fn copy_files(&self, _src: &str, _dst: &str, _names: &[String]) -> std::io::Result<usize> {
            panic!("copy_files must not be called")
        }

        fn move_files(&self, _src: &str, _dst: &str, _names: &[String]) -> std::io::Result<usize> {
            panic!("move_files must not be called")
        }

        fn delete_files(&self, _dir: &str, _names: &[String]) -> std::io::Result<usize> {
            panic!("delete_files must not be called")
        }
    }

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
        let page = response.result.unwrap();
        assert_eq!(page.entries.len(), 1);
        assert!(
            page.entries
                .iter()
                .all(|listed| listed.identity == crate::vfs::EntryIdentity::Other)
        );
        assert!(page.continuation.is_none());
    }

    #[tokio::test]
    async fn provider_page_wrapper_preserves_exact_identity_and_continuation() {
        let provider_continuation = ProviderContinuation {
            token: "  opaque+/=token 日本語  ".into(),
        };
        let listed = ListedEntry {
            entry: Entry {
                name: "display".into(),
                kind: EntryKind::Directory,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::Other,
        };
        let registry = ProviderRegistry::new();
        registry.insert_sftp(
            "prod-host",
            Box::new(PageProvider {
                page: ProviderListingPage {
                    entries: vec![listed.clone()],
                    continuation: Some(provider_continuation.clone()),
                },
            }),
            CapabilitySet::NONE,
        );
        let (loader, mut rx) = PaneLoader::channel(registry);
        let location = Location::Sftp {
            host: "prod-host".into(),
            path: "/srv/exact".into(),
        };

        let id = loader.load(Pane::Right, location.clone(), PaneLoadPurpose::Refresh);
        let response = rx.recv().await.expect("pane response");
        let page = response.result.unwrap();
        let continuation = page.continuation.expect("provider continuation");

        assert_eq!(page.entries, vec![listed]);
        assert_eq!(continuation.provider_continuation, provider_continuation);
        assert_eq!(
            continuation.provider_instance,
            ProviderInstanceKey::SftpHost("prod-host".into())
        );
        assert_eq!(continuation.location, location);
        assert_eq!(continuation.generation, id);
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
