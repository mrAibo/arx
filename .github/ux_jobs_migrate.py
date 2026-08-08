from pathlib import Path

path = Path('src/jobs/mod.rs')
text = path.read_text()


def once(old: str, new: str, label: str) -> None:
    global text
    count = text.count(old)
    if count != 1:
        raise SystemExit(f'{label}: expected 1 match, found {count}')
    text = text.replace(old, new, 1)

once(
    'use crate::vfs::Location;\n',
    'use crate::vfs::Location;\nuse crate::workspace_sync::{SyncDirection, SyncMode};\n',
    'sync display imports',
)

once(
    '// ── Job model ──\n\n#[derive(Debug, Clone)]\npub struct Job {',
    '''// ── Job model ──\n\n#[derive(Debug, Clone, PartialEq, Eq)]\npub struct WorkspaceSyncJobContext {\n    pub left_root: Location,\n    pub right_root: Location,\n    pub direction: SyncDirection,\n    pub mode: SyncMode,\n}\n\nimpl WorkspaceSyncJobContext {\n    pub fn source(&self) -> &Location {\n        match self.direction {\n            SyncDirection::LeftToRight => &self.left_root,\n            SyncDirection::RightToLeft => &self.right_root,\n        }\n    }\n\n    pub fn destination(&self) -> &Location {\n        match self.direction {\n            SyncDirection::LeftToRight => &self.right_root,\n            SyncDirection::RightToLeft => &self.left_root,\n        }\n    }\n}\n\n#[derive(Debug, Clone)]\npub struct Job {''',
    'sync job context type',
)

once(
    '    pub destination: Option<Location>,\n    /// The one cancellation flag owned by this job and shared with executors.',
    '    pub destination: Option<Location>,\n    /// Workspace roots stay canonical left/right for verification. Presentation\n    /// direction lives here instead of overloading generic source/destination.\n    pub sync_context: Option<WorkspaceSyncJobContext>,\n    /// The one cancellation flag owned by this job and shared with executors.',
    'job context field',
)

once(
    '            source,\n            destination,\n            cancel: job_token(),',
    '            source,\n            destination,\n            sync_context: None,\n            cancel: job_token(),',
    'job context default',
)

once(
    '    pub fn status_icon(&self) -> &str {',
    '''    pub fn display_source(&self) -> Option<&Location> {\n        self.sync_context\n            .as_ref()\n            .map(WorkspaceSyncJobContext::source)\n            .or(self.source.as_ref())\n    }\n\n    pub fn display_destination(&self) -> Option<&Location> {\n        self.sync_context\n            .as_ref()\n            .map(WorkspaceSyncJobContext::destination)\n            .or(self.destination.as_ref())\n    }\n\n    pub fn status_icon(&self) -> &str {''',
    'job display helpers',
)

once(
    '''        let description = format!(\n            "Sync {} → {}",\n            compiled_plan.left_root(),\n            compiled_plan.right_root()\n        );\n        let job = self.create_job(\n            "sync",\n            JobKind::Synchronize,\n            description,\n            Some(compiled_plan.left_root().clone()),\n            Some(compiled_plan.right_root().clone()),\n        );''',
    '''        let sync_context = WorkspaceSyncJobContext {\n            left_root: compiled_plan.left_root().clone(),\n            right_root: compiled_plan.right_root().clone(),\n            direction: compiled_plan.direction(),\n            mode: compiled_plan.mode(),\n        };\n        let description = format!(\n            "Sync {} → {}",\n            sync_context.source(),\n            sync_context.destination()\n        );\n        let mut job = self.create_job(\n            "sync",\n            JobKind::Synchronize,\n            description,\n            Some(compiled_plan.left_root().clone()),\n            Some(compiled_plan.right_root().clone()),\n        );\n        job.sync_context = Some(sync_context.clone());\n        {\n            let mut state = self\n                .state\n                .lock()\n                .unwrap_or_else(|poisoned| poisoned.into_inner());\n            if let Some(stored) = state.jobs.get_mut(&job.id) {\n                stored.sync_context = Some(sync_context);\n            }\n        }''',
    'sync job creation context',
)

marker = '''    #[test]\n    fn sync_progress_prefers_byte_percent_without_losing_step_counts() {'''
insert = '''    #[test]\n    fn right_to_left_sync_context_reverses_display_without_reversing_workspace_roots() {\n        let context = WorkspaceSyncJobContext {\n            left_root: Location::Local("/left".into()),\n            right_root: Location::Local("/right".into()),\n            direction: SyncDirection::RightToLeft,\n            mode: SyncMode::Update,\n        };\n        assert_eq!(context.source(), &Location::Local("/right".into()));\n        assert_eq!(context.destination(), &Location::Local("/left".into()));\n        assert_eq!(context.left_root, Location::Local("/left".into()));\n        assert_eq!(context.right_root, Location::Local("/right".into()));\n    }\n\n'''
if text.count(marker) != 1:
    raise SystemExit('jobs test anchor mismatch')
text = text.replace(marker, insert + marker, 1)

path.write_text(text)
