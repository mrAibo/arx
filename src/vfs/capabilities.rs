use super::ProviderId;

/// Filesystem-like operations that a registered VFS provider can perform.
///
/// A capability is a promise about the provider's current implementation, not
/// an aspirational protocol feature. The transfer planner may rely on this
/// metadata when choosing a safe execution strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Capability {
    List = 0,
    Read = 1,
    Write = 2,
    Mkdir = 3,
    Rename = 4,
    Delete = 5,
    Copy = 6,
    Move = 7,
    Symlink = 8,
    Chmod = 9,
    ServerSideCopy = 10,
}

/// Compact, allocation-free capability set suitable for provider metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CapabilitySet(u16);

impl CapabilitySet {
    pub const NONE: Self = Self(0);

    pub const fn with(self, capability: Capability) -> Self {
        Self(self.0 | (1u16 << capability as u8))
    }

    pub const fn supports(self, capability: Capability) -> bool {
        self.0 & (1u16 << capability as u8) != 0
    }

    pub const fn contains_all(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }

    pub const fn bits(self) -> u16 {
        self.0
    }
}

/// Capabilities currently implemented by the built-in local provider.
pub const LOCAL_CAPABILITIES: CapabilitySet = CapabilitySet::NONE
    .with(Capability::List)
    .with(Capability::Read)
    .with(Capability::Mkdir)
    .with(Capability::Delete)
    .with(Capability::Copy)
    .with(Capability::Move);

/// SFTP now exposes directory creation and deletion through async VfsProvider
/// primitives. Transfers remain delegated to the transfer layer.
pub const SFTP_CAPABILITIES: CapabilitySet = CapabilitySet::NONE
    .with(Capability::List)
    .with(Capability::Read)
    .with(Capability::Mkdir)
    .with(Capability::Delete);

/// Archive browsing currently supports listing only. Extraction/mutation stay
/// outside the provider contract until they have transactional semantics.
pub const ARCHIVE_CAPABILITIES: CapabilitySet = CapabilitySet::NONE.with(Capability::List);

/// The current WebDAV provider implements PROPFIND and GET only.
pub const WEBDAV_CAPABILITIES: CapabilitySet = CapabilitySet::NONE
    .with(Capability::List)
    .with(Capability::Read);

/// S3 is still a stub and must not advertise operations it cannot execute.
pub const S3_CAPABILITIES: CapabilitySet = CapabilitySet::NONE;

/// Built-in capability declaration for a provider kind.
///
/// The registry remains authoritative when a concrete provider instance is
/// registered. This fallback is useful during the ongoing registry migration,
/// where `AppState.registry` and the legacy thread-local registry can differ.
pub const fn builtin_capabilities(provider: ProviderId) -> CapabilitySet {
    match provider {
        ProviderId::Local => LOCAL_CAPABILITIES,
        ProviderId::Sftp => SFTP_CAPABILITIES,
        ProviderId::S3 => S3_CAPABILITIES,
        ProviderId::WebDAV => WEBDAV_CAPABILITIES,
        // Archive capabilities are not yet declared as a stable contract.
        ProviderId::Archive => CapabilitySet::NONE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_sets_are_composable() {
        let set = CapabilitySet::NONE
            .with(Capability::List)
            .with(Capability::Read);

        assert!(set.supports(Capability::List));
        assert!(set.supports(Capability::Read));
        assert!(!set.supports(Capability::Delete));
        assert!(set.contains_all(CapabilitySet::NONE.with(Capability::List)));
    }

    #[test]
    fn builtins_do_not_overpromise() {
        assert!(LOCAL_CAPABILITIES.supports(Capability::Move));
        assert!(!SFTP_CAPABILITIES.supports(Capability::Move));
        assert!(ARCHIVE_CAPABILITIES.supports(Capability::List));
        assert!(WEBDAV_CAPABILITIES.supports(Capability::Read));
        assert!(!WEBDAV_CAPABILITIES.supports(Capability::Delete));
        assert_eq!(S3_CAPABILITIES, CapabilitySet::NONE);
    }

    #[test]
    fn sftp_has_read_capability() {
        assert!(SFTP_CAPABILITIES.supports(Capability::Read));
    }

    #[test]
    fn local_read_unchanged() {
        assert!(LOCAL_CAPABILITIES.supports(Capability::Read));
        assert!(LOCAL_CAPABILITIES.supports(Capability::List));
        assert!(LOCAL_CAPABILITIES.supports(Capability::Move));
    }

    #[test]
    fn s3_no_read() {
        assert!(!S3_CAPABILITIES.supports(Capability::Read));
        assert!(!S3_CAPABILITIES.supports(Capability::List));
        assert_eq!(S3_CAPABILITIES, CapabilitySet::NONE);
    }
}
