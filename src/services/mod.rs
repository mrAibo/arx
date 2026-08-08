mod desktop;
mod diff;
mod file_info;
mod git;
mod infrastructure;
mod mutation;
mod pane_loader;
mod preview;
mod tree;
mod workspace_scanner;

pub use desktop::DesktopService;
pub use diff::DiffService;
pub use file_info::FileInfoService;
pub use git::GitService;
pub use infrastructure::InfrastructureService;
pub use mutation::{MutationError, MutationProgress, MutationService, TrashOutcome};
pub use pane_loader::{PaneLoadId, PaneLoadPurpose, PaneLoadResponse, PaneLoader};
pub use preview::PreviewService;
pub use tree::TreeService;
pub use workspace_scanner::scan_workspace;
pub use workspace_scanner::{
    WorkspaceScanError, WorkspaceScanId, WorkspaceScanOptions, WorkspaceScanResponse,
    WorkspaceScanner,
};
