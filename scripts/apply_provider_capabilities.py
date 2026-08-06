from pathlib import Path

path = Path("src/vfs/mod.rs")
text = path.read_text()

replacements = [
    (
        "pub mod archive;\npub mod local;",
        "pub mod archive;\npub mod capabilities;\npub mod local;\n\npub use capabilities::{Capability, CapabilitySet};",
    ),
    (
        "pub enum VfsError {\n    NotFound(String),\n    PermissionDenied(String),",
        "pub enum VfsError {\n    NotFound(String),\n    PermissionDenied(String),\n    UnsupportedOperation { provider: ProviderId, capability: Capability },\n    ReadOnlyProvider(ProviderId),",
    ),
    (
        "            Self::PermissionDenied(msg) => write!(f, \"permission denied: {msg}\"),\n            Self::Timeout(msg) => write!(f, \"timeout: {msg}\"),",
        "            Self::PermissionDenied(msg) => write!(f, \"permission denied: {msg}\"),\n            Self::UnsupportedOperation { provider, capability } => {\n                write!(f, \"provider {provider:?} does not support {capability:?}\")\n            }\n            Self::ReadOnlyProvider(provider) => write!(f, \"provider {provider:?} is read-only\"),\n            Self::Timeout(msg) => write!(f, \"timeout: {msg}\"),",
    ),
    (
        "#[derive(Debug)]\npub struct ProviderRegistry(HashMap<ProviderId, Box<dyn VfsProvider>>);",
        "#[derive(Debug)]\nstruct RegisteredProvider {\n    provider: Box<dyn VfsProvider>,\n    capabilities: CapabilitySet,\n}\n\n#[derive(Debug)]\npub struct ProviderRegistry(HashMap<ProviderId, RegisteredProvider>);",
    ),
    (
        "    pub fn insert(&mut self, id: ProviderId, provider: Box<dyn VfsProvider>) {\n        self.0.insert(id, provider);\n    }\n    pub fn get(&self, id: &ProviderId) -> Option<&dyn VfsProvider> {\n        self.0.get(id).map(|b| b.as_ref())\n    }",
        "    pub fn insert(\n        &mut self,\n        id: ProviderId,\n        provider: Box<dyn VfsProvider>,\n        capabilities: CapabilitySet,\n    ) {\n        self.0.insert(\n            id,\n            RegisteredProvider {\n                provider,\n                capabilities,\n            },\n        );\n    }\n    pub fn get(&self, id: &ProviderId) -> Option<&dyn VfsProvider> {\n        self.0.get(id).map(|registered| registered.provider.as_ref())\n    }\n    pub fn capabilities(&self, id: &ProviderId) -> Option<CapabilitySet> {\n        self.0.get(id).map(|registered| registered.capabilities)\n    }\n    pub fn supports(&self, id: &ProviderId, capability: Capability) -> bool {\n        self.capabilities(id)\n            .is_some_and(|capabilities| capabilities.supports(capability))\n    }\n    pub fn require(&self, id: &ProviderId, capability: Capability) -> Result<(), VfsError> {\n        if self.supports(id, capability) {\n            Ok(())\n        } else {\n            Err(VfsError::UnsupportedOperation {\n                provider: *id,\n                capability,\n            })\n        }\n    }",
    ),
    (
        "    r.insert(ProviderId::Local, Box::new(local::LocalProvider));",
        "    r.insert(\n        ProviderId::Local,\n        Box::new(local::LocalProvider),\n        capabilities::LOCAL_CAPABILITIES,\n    );",
    ),
    (
        "                    self.insert(ProviderId::Sftp, Box::new(sftp::SftpProvider { host: h }));",
        "                    self.insert(\n                        ProviderId::Sftp,\n                        Box::new(sftp::SftpProvider { host: h }),\n                        capabilities::SFTP_CAPABILITIES,\n                    );",
    ),
]

for old, new in replacements:
    if old not in text:
        raise SystemExit(f"migration anchor not found:\n{old}")
    text = text.replace(old, new, 1)

path.write_text(text)
