from pathlib import Path


def rep(path: str, old: str, new: str, count: int = 1) -> None:
    p = Path(path)
    text = p.read_text()
    if text.count(old) < count:
        raise SystemExit(f"anchor not found in {path}: {old[:100]!r}")
    p.write_text(text.replace(old, new, count))


rep(
    "src/lib.rs",
    "pub mod remote;\npub mod services;\n",
    "pub mod remote;\n#[cfg(target_os = \"linux\")]\npub mod s3_inspector;\n#[cfg(target_os = \"linux\")]\npub mod s3_inspector_ui;\npub mod services;\n",
)

rep(
    "src/storage_inspector_ui.rs",
    "use crate::app::{AppState, OverlayKind};\nuse crate::jobs::{JobProgress, JobResult, JobStatus};\n",
    "use crate::app::{AppState, OverlayKind};\nuse crate::jobs::{JobProgress, JobResult, JobStatus};\nuse crate::vfs::ListedEntry;\n",
)
rep(
    "src/storage_inspector_ui.rs",
    "    pub view: StorageView,\n}\n",
    "    pub view: StorageView,\n    /// S3 shares the exclusive Storage Inspector overlay while keeping its own read-only state.\n    pub s3: Option<crate::s3_inspector_ui::S3InspectorUiState>,\n}\n",
)
rep(
    "src/storage_inspector_ui.rs",
    "        self.view = StorageView::Directory;\n    }\n",
    "        self.view = StorageView::Directory;\n        self.s3 = None;\n    }\n",
)
rep(
    "src/storage_inspector_ui.rs",
    "pub fn launch_storage_inspector(state: &mut AppState) -> Result<String, String> {\n    let root = match &state.active_pane().location {\n        crate::vfs::Location::Local(path) => path.clone(),\n        _ => return Err(\"Storage Inspector is available for local paths only\".into()),\n    };\n\n    let manager = state\n",
    "pub fn launch_storage_inspector(\n    state: &mut AppState,\n    focused_listed: Option<&ListedEntry>,\n) -> Result<String, String> {\n    match &state.active_pane().location {\n        crate::vfs::Location::S3 { .. } => {\n            return crate::s3_inspector_ui::launch_s3_inspector(state, focused_listed);\n        }\n        crate::vfs::Location::Local(_) => {}\n        _ => return Err(\"Storage Inspector is available for Local and S3 paths\".into()),\n    }\n    launch_local_storage_inspector(state)\n}\n\nfn launch_local_storage_inspector(state: &mut AppState) -> Result<String, String> {\n    let root = match &state.active_pane().location {\n        crate::vfs::Location::Local(path) => path.clone(),\n        _ => return Err(\"Local Storage Inspector requires a local path\".into()),\n    };\n\n    let manager = state\n",
)
rep(
    "src/storage_inspector_ui.rs",
    "    let events = state\n        .job_events\n        .clone()\n        .ok_or_else(|| \"Storage Inspector: job event channel is not bound\".to_string())?;\n\n    if let Some(id) = state.storage_inspector.job_id.clone()\n",
    "    let events = state\n        .job_events\n        .clone()\n        .ok_or_else(|| \"Storage Inspector: job event channel is not bound\".to_string())?;\n\n    if let Some(s3) = state.storage_inspector.s3.take()\n        && manager\n            .get(&s3.job_id)\n            .is_some_and(|job| !job.status.is_terminal())\n    {\n        manager.cancel(&s3.job_id);\n    }\n\n    if let Some(id) = state.storage_inspector.job_id.clone()\n",
)
rep(
    "src/storage_inspector_ui.rs",
    "pub fn handle_storage_inspector_key(state: &mut AppState, key: KeyEvent) {\n    match key.code {\n",
    "pub fn handle_storage_inspector_key(state: &mut AppState, key: KeyEvent) {\n    if state.storage_inspector.s3.is_some() {\n        crate::s3_inspector_ui::handle_s3_inspector_key(state, key);\n        return;\n    }\n    match key.code {\n",
)
rep(
    "src/storage_inspector_ui.rs",
    "pub fn render_storage_inspector(frame: &mut ratatui::Frame, area: Rect, state: &mut AppState) {\n    let popup = centered_rect(90, 86, area);\n",
    "pub fn render_storage_inspector(frame: &mut ratatui::Frame, area: Rect, state: &mut AppState) {\n    if state.storage_inspector.s3.is_some() {\n        crate::s3_inspector_ui::render_s3_inspector(frame, area, state);\n        return;\n    }\n    let popup = centered_rect(90, 86, area);\n",
)

rep("src/tui/feature_registry.rs", "use arx::vfs::Entry;\n", "use arx::vfs::{Entry, ListedEntry};\n")
rep(
    "src/tui/feature_registry.rs",
    "    pub focused: Option<&'a Entry>,\n    pub active_entries: &'a [&'a Entry],\n",
    "    pub focused: Option<&'a Entry>,\n    pub focused_listed: Option<&'a ListedEntry>,\n    pub active_entries: &'a [&'a Entry],\n",
)
rep(
    "src/tui/feature_registry.rs",
    "        focused,\n        active_entries,\n        effect_dispatcher,\n",
    "        focused,\n        focused_listed: _,\n        active_entries,\n        effect_dispatcher,\n",
)
rep(
    "src/tui/feature_registry.rs",
    "fn storage_handler(ctx: &mut FeatureActionContext, _action: &Action) -> bool {\n    match arx::storage_inspector_ui::launch_storage_inspector(ctx.state) {\n",
    "fn storage_handler(ctx: &mut FeatureActionContext, _action: &Action) -> bool {\n    match arx::storage_inspector_ui::launch_storage_inspector(ctx.state, ctx.focused_listed) {\n",
)
rep(
    "src/tui/feature_registry.rs",
    "            focused: None,\n            active_entries: &[],\n",
    "            focused: None,\n            focused_listed: None,\n            active_entries: &[],\n",
    2,
)

rep(
    "src/tui.rs",
    "    let focused = focused_row\n        .and_then(|row| row.listed())\n        .map(|listed| &listed.entry);\n    // PACK R: proof-feature activation through the registered controller seam.\n    {\n        let focused_ref = focused;\n        let mut ctx = feature_registry::FeatureActionContext {\n            state,\n            focused: focused_ref,\n            active_entries,\n            effect_dispatcher,\n        };\n",
    "    // Exact ListedEntry identity is frozen before feature dispatch so S3\n    // inspection never reconstructs remote identity from presentation names.\n    let focused_listed = focused_row.and_then(|row| row.listed());\n    let focused = focused_listed.map(|listed| &listed.entry);\n    // PACK R: proof-feature activation through the registered controller seam.\n    {\n        let focused_ref = focused;\n        let mut ctx = feature_registry::FeatureActionContext {\n            state,\n            focused: focused_ref,\n            focused_listed,\n            active_entries,\n            effect_dispatcher,\n        };\n",
)
rep(
    "src/tui.rs",
    "    // ponytail: keep the ListedEntry (exact identity) for preview, not &Entry\n    let focused_listed = focused_row.and_then(|row| row.listed());\n",
    "    // `focused_listed` above remains the exact identity authority for preview/transfers too.\n",
)

rep(
    ".github/workflows/ci.yml",
    "          cargo test --locked --test transfer_queue_s3_retry_physical -- --nocapture\n",
    "          cargo test --locked --test transfer_queue_s3_retry_physical -- --nocapture\n          cargo test --locked --test s3_inspector_minio -- --nocapture\n",
)
