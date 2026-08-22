use std::path::PathBuf;

/// Frozen input context for local Quick Actions that require one text value.
///
/// The active directory and selected/focused entry names are captured when the
/// action starts. Editing the prompt cannot silently retarget the operation if
/// the user later changes panes or selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuickActionPrompt {
    Touch { dir: PathBuf },
    CompressTarGz { dir: PathBuf, names: Vec<String> },
}

impl QuickActionPrompt {
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Touch { .. } => " touch: ",
            Self::CompressTarGz { .. } => " tar.gz: ",
        }
    }
}
