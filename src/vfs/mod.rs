use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NamespaceId(String);

impl NamespaceId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum VfsPath {
    Native(PathBuf),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Location {
    pub namespace: NamespaceId,
    pub path: VfsPath,
}

impl Location {
    pub fn new(namespace: NamespaceId, path: VfsPath) -> Self {
        Self { namespace, path }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EntryName {
    Native(OsString),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: EntryName,
    pub kind: EntryKind,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Capability {
    List,
    Read,
    Write,
    CreateDirectory,
    Rename,
    Remove,
    Metadata,
    Permissions,
    Symlink,
    FreeSpace,
    ServerSideCopy,
    ServerSideMove,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities(BTreeSet<Capability>);

impl Capabilities {
    pub fn from_iter(capabilities: impl IntoIterator<Item = Capability>) -> Self {
        Self(capabilities.into_iter().collect())
    }

    pub fn supports(&self, capability: Capability) -> bool {
        self.0.contains(&capability)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_does_not_encode_provider_kind() {
        let location = Location::new(
            NamespaceId::new("host:db-prod"),
            VfsPath::Bytes(b"/var/log".to_vec()),
        );

        assert_eq!(location.namespace.as_str(), "host:db-prod");
    }

    #[test]
    fn capabilities_are_explicit() {
        let capabilities = Capabilities::from_iter([Capability::List, Capability::Read]);

        assert!(capabilities.supports(Capability::Read));
        assert!(!capabilities.supports(Capability::Write));
    }
}
