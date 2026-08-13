mod desktop;
mod diff;
mod file_info;
mod git;
mod infrastructure;
mod mutation;
mod pane_loader;
pub mod preview;
mod tree;
mod workspace_scanner;
mod workspace_sync_controller;

pub use desktop::DesktopService;
pub use diff::DiffService;
pub use file_info::FileInfoService;
pub use git::GitService;
pub use infrastructure::InfrastructureService;
pub use mutation::{MutationError, MutationProgress, MutationService, TrashOutcome};
pub use pane_loader::{
    PaneListingContinuation, PaneLoadId, PaneLoadPage, PaneLoadPurpose, PaneLoadResponse,
    PaneLoader,
};
pub use preview::{PreviewService, format_bounded_preview};
pub use tree::TreeService;
pub use workspace_scanner::scan_workspace;
pub use workspace_scanner::{
    WorkspaceScanError, WorkspaceScanId, WorkspaceScanOptions, WorkspaceScanResponse,
    WorkspaceScanner,
};
pub use workspace_sync_controller::{
    SyncLaunchId, WorkspaceSyncController, WorkspaceSyncLaunchError,
};
