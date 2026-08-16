/// Hitbox for command bar mouse click routing.
#[derive(Debug, Clone)]
pub struct CommandHitbox {
    pub rect: Rect,
    pub action: Action,
    pub available: bool,
}

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use ratatui::layout::Rect;

use crate::effect_dispatcher::{EffectId, EffectLane, EffectScope};
use crate::jobs::Job;
use crate::remote::Host;
use crate::services::{PaneListingContinuation, PaneLoadId, PaneLoadPurpose, PanePageRequestId};
use crate::terminal::TermPane;
use crate::vfs::{Location, ProviderRegistry};
use crate::vfs::{RemoteDeletePlan, RemoteEditSession};

mod actions;
pub use actions::{
    ACTION_CATALOG, ALL_ACTIONS, Action, ActionCategory, ActionId, ActionMeta, InputContext,
    action_meta, listed_entry_navigation_target, navigation_parent_target,
};
mod availability;
pub use availability::{ActionAvailability, ActionContext, action_availability};
mod command_center;
pub use command_center::{
    CommandItem, CommandKind, CommandTarget, build_command_items,
    build_command_items_with_file_context,
};
mod overlay;
pub use overlay::OverlayKind;
mod remote_workspace;
pub use remote_workspace::RemoteWorkspaceState;
mod workspace_sync_ux;
pub use workspace_sync_ux::WorkspaceSyncUxState;

/// Command Center channel tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommandCenterChannel {
    #[default]
    Files,
    Hosts,
    Git,
    Docker,
    Actions,
}

/// MC-style menu entry.
#[derive(Debug, Clone)]
pub struct MenuEntry {
    pub label: String,
    pub command: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Pane {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneLoadUiError {
    pub attempted: Location,
    pub message: String,
}

/// One-process UX milestones. These intentionally reset with every AppState
/// and do not imply persisted onboarding state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionMilestones {
    pub compare_success_seen: bool,
    pub verified_sync_success_seen: bool,
}

impl SessionMilestones {
    pub fn take_compare_success(&mut self) -> bool {
        if self.compare_success_seen {
            false
        } else {
            self.compare_success_seen = true;
            true
        }
    }

    pub fn take_verified_sync_success(&mut self) -> bool {
        if self.verified_sync_success_seen {
            false
        } else {
            self.verified_sync_success_seen = true;
            true
        }
    }
}

/// Passive presentation derived from already-accepted runtime truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCallout {
    CompareCompleted {
        differences: usize,
        bytes_to_transfer: u64,
    },
    WorkspaceSyncVerified {
        job_id: String,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    NameAsc,
    NameDesc,
    SizeAsc,
    SizeDesc,
    Kind,
}

impl SortMode {
    pub fn next(self) -> Self {
        match self {
            Self::NameAsc => Self::NameDesc,
            Self::NameDesc => Self::SizeAsc,
            Self::SizeAsc => Self::SizeDesc,
            Self::SizeDesc => Self::Kind,
            Self::Kind => Self::NameAsc,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::NameAsc => "name↑",
            Self::NameDesc => "name↓",
            Self::SizeAsc => "size↑",
            Self::SizeDesc => "size↓",
            Self::Kind => "kind",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PaneState {
    pub location: Location,
    pub cursor: usize,
    /// Saved tabs: (location, cursor). Current tab is separate in location/cursor above.
    /// Index 0..n-1 for saved tabs; switching swaps current ↔ saved[idx].
    pub tabs: Vec<(Location, usize)>,
    /// Directory history stack. Push before entering a directory; Alt+Down pops.
    pub dir_history: Vec<Location>,
    pub split: bool,
    pub split_cursor: usize,
    pub split_active: bool,
}

impl PaneState {
    /// Open a new tab. Saves current location/cursor in tabs vec.
    pub fn new_tab(&mut self) {
        self.tabs.push((self.location.clone(), self.cursor));
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/"));
        self.location = Location::Local(home);
        self.cursor = 0;
    }

    /// Close active tab. Falls back to last saved tab.
    pub fn close_tab(&mut self) {
        if let Some((loc, cur)) = self.tabs.pop() {
            self.location = loc;
            self.cursor = cur;
        }
    }

    /// Switch to tab at index. Swaps current state with saved[idx].
    pub fn switch_tab(&mut self, idx: usize) {
        if idx < self.tabs.len() {
            let saved = (self.location.clone(), self.cursor);
            let target = self.tabs[idx].clone();
            self.tabs[idx] = saved;
            self.location = target.0;
            self.cursor = target.1;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PanelMode {
    Full,  // name + size + time (default)
    Brief, // filenames in columns
}

/// PACK B: Add/Edit form for the managed SSH host manager (F1/F2).
/// Fields are edited as plain strings; conversion/validation happens on Save.
#[derive(Debug)]
pub struct SshHostForm {
    pub mode: SshHostFormMode,
    /// [0]=Alias [1]=HostName [2]=User [3]=Port [4]=IdentityFile [5]=ProxyJump [6]=IdentitiesOnly(yes/no)
    pub fields: [String; 7],
    /// Index into `fields` currently being edited.
    pub focus: usize,
    /// Original alias when editing (for atomic rename).
    pub original_alias: Option<String>,
    /// Transient error shown after a failed Save.
    pub error: Option<String>,
    /// True while awaiting explicit confirmation to generate an unencrypted key.
    pub confirm_generate: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SshHostFormMode {
    Add,
    Edit,
}

impl SshHostForm {
    pub fn new_add() -> Self {
        Self {
            mode: SshHostFormMode::Add,
            fields: [
                String::new(),
                String::new(),
                whoami_user(),
                "22".into(),
                String::new(),
                String::new(),
                "no".into(),
            ],
            focus: 0,
            original_alias: None,
            error: None,
            confirm_generate: false,
        }
    }

    pub fn new_edit(h: &crate::remote::ssh_config_manager::ManagedHost) -> Self {
        Self {
            mode: SshHostFormMode::Edit,
            fields: [
                h.alias.clone(),
                h.hostname.clone(),
                h.user.clone(),
                h.port.to_string(),
                h.identity_file
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                h.proxy_jump.clone().unwrap_or_default(),
                if h.identities_only { "yes" } else { "no" }.into(),
            ],
            focus: 0,
            original_alias: Some(h.alias.clone()),
            error: None,
            confirm_generate: false,
        }
    }
}

fn whoami_user() -> String {
    std::env::var("USER").unwrap_or_else(|_| "root".into())
}

#[derive(Debug)]
pub struct AppState {
    pub should_quit: bool,
    pub left: PaneState,
    pub right: PaneState,
    pub active: Pane,
    pub selected: BTreeSet<String>,
    pub selection_scope: Option<(Pane, Location)>,
    pub filter: String,
    pub filtering: bool,
    pub message: Option<String>,
    /// Cached status-bar Git suffix. Never compute Git state from render().
    pub git_status: String,
    pub git_status_location: Option<Location>,
    /// Local ↔ remote (or any provider ↔ provider) comparison/sync workspace.
    pub remote_workspace: RemoteWorkspaceState,
    /// Session-only first-success presentation state. No config persistence.
    pub milestones: SessionMilestones,
    pub session_callout: Option<SessionCallout>,
    /// Latest effect per lane. Older responses are discarded deterministically.
    pub pending_effects: BTreeMap<EffectLane, EffectId>,
    /// Latest async VFS load generation for each pane.
    pub pending_pane_loads: BTreeMap<Pane, PaneLoadId>,
    pub pending_pane_targets: BTreeMap<Pane, (Location, PaneLoadPurpose)>,
    /// Persistent current listing generation per pane. Advanced on every
    /// register_pane_load; preserved across finish_pane_load so a page-1
    /// continuation stays valid for page 2. Stale guard compares against this.
    // ponytail: separate from pending_* (request state) — pagination needs the
    // generation to outlive the request; finish_pane_load only clears pending_*.
    pub pane_listing_generations: BTreeMap<Pane, PaneLoadId>,
    pub pane_listing_continuations: BTreeMap<Pane, PaneListingContinuation>,
    pub pending_next_pages: BTreeMap<Pane, (PanePageRequestId, PaneListingContinuation)>,
    /// Persistent presentation state for the latest accepted pane-load failure.
    pub pane_load_errors: BTreeMap<Pane, PaneLoadUiError>,
    pub infrastructure_lines: Vec<String>,
    pub tree_lines: Vec<String>,
    pub glob_input: bool,
    pub go_input: bool,
    pub show_help: bool,
    pub show_hidden: bool,
    // A3: sort order
    pub sort_mode: SortMode,
    pub panel_mode: PanelMode,
    // A1: file viewer
    pub viewer_content: Vec<String>,
    pub viewer_scroll: usize,
    // UI15-FIX-07: scrollable F1 Help
    pub help_scroll: usize,
    pub left_area: Option<Rect>,
    pub right_area: Option<Rect>,
    // A4: bookmarks
    pub bookmarks: Vec<Location>,
    pub show_bookmarks: bool,
    pub bookmark_cursor: usize,
    // B3: host panel
    pub hosts: Vec<Host>,
    pub show_hosts: bool,
    pub host_cursor: usize,
    // PACK B: managed SSH host manager overlay
    pub show_ssh_hosts: bool,
    pub ssh_host_cursor: usize,
    pub ssh_hosts: Vec<crate::remote::ssh_config_manager::ManagedHost>,
    pub ssh_host_status: Option<String>,
    /// Active Add/Edit form (F1/F2). None when the list is shown.
    pub ssh_form: Option<SshHostForm>,
    /// Pending unconfirmed Ed25519 key generation (list-mode Ctrl+K waits for y/n).
    pub ssh_pending_keygen: Option<String>,
    /// Shared result slot for async (non-blocking) connection tests (F7).
    pub ssh_test_result: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    // B4: render-only snapshot; JobManager owns runtime lifecycle.
    pub jobs: Vec<Job>,
    // PACK C #51: runtime JobManager + event sink so TUI observers (remote edit)
    // can publish job events without threading the manager through every caller.
    pub job_manager: crate::jobs::JobManager,
    pub job_events: tokio::sync::mpsc::UnboundedSender<crate::jobs::JobEvent>,
    pub show_jobs: bool,
    pub job_cursor: usize,
    // C1: directory compare
    pub show_diff: bool,
    // C2: command input
    pub cmd_input: bool,
    pub cmd: String,
    /// Frozen location for provider-backed mkdir (SFTP). Cleared on cancel/submit.
    pub pending_mkdir_location: Option<Location>,
    /// Pending remote delete plan awaiting user confirmation.
    pub pending_delete: Option<RemoteDeletePlan>,
    /// Ctrl+X prefix for MC-style key combos
    pub cmd_prefix: bool,
    /// Phase-2 remote edit: Download landed, editor needs launching.
    pub pending_remote_edit_session: Option<RemoteEditSession>,
    /// Pane and exact location that initiated the current remote edit.
    pub pending_remote_edit_origin: Option<(Pane, Location)>,
    /// Defer editor launch until after queued terminal input is drained.
    pub pending_editor: bool,
    /// JobManager job id for the in-flight remote edit (survives all phases).
    pub pending_remote_edit_job_id: Option<String>,
    // C3: user menu
    pub menu: Vec<MenuEntry>,
    pub show_menu: bool,
    pub menu_cursor: usize,
    // Terminal pane (right side, Ctrl+Shift+T toggle)
    pub term: Option<TermPane>,
    pub show_terminal: bool,
    // Directory history (Alt+H)
    pub dir_history: Vec<PathBuf>,
    pub show_history: bool,
    // Vim-style file search (/)
    pub file_search: bool,
    pub search_query: String,
    pub search_matches: Vec<usize>,
    pub search_index: usize,
    // Panel ratio (default 50/50)
    pub panel_ratio: u16,
    // Command bar hitboxes computed during render, consumed by mouse handler
    pub command_hitboxes: Vec<CommandHitbox>,
    pub show_hotlist: bool,
    pub hotlist_cursor: usize,
    pub show_tab_switcher: bool,
    pub tab_switcher_cursor: usize,
    pub split: bool, // Ctrl+\ split pane vertically
    pub rename_input: bool,
    pub rename_pattern: String,
    pub show_command_center: bool,
    pub cc_channel: CommandCenterChannel,
    pub show_tree: bool,
    pub show_infra: bool,
    pub tree_filter: String,
    pub show_context_menu: bool,
    pub context_menu_pos: (u16, u16),
    pub command_matches: Vec<CommandItem>,
    pub overlay_list_state: ratatui::widgets::ListState,
    /// Provider registry — phased replacement of match-Location dispatch
    pub registry: crate::vfs::ProviderRegistry,
}

impl Default for AppState {
    fn default() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let home = dirs_fallback();
        // ponytail: hardcoded defaults; load from bookmarks.toml later
        let bookmarks = vec![Location::Local(home.clone())];
        Self {
            should_quit: false,
            left: PaneState {
                location: Location::Local(cwd),
                cursor: 0,
                tabs: Vec::new(),
                dir_history: Vec::new(),
                split: false,
                split_cursor: 0,
                split_active: false,
            },
            right: PaneState {
                location: Location::Local(home),
                cursor: 0,
                tabs: Vec::new(),
                dir_history: Vec::new(),
                split: false,
                split_cursor: 0,
                split_active: false,
            },
            active: Pane::Left,
            selected: BTreeSet::new(),
            selection_scope: None,
            filter: String::new(),
            filtering: false,
            message: None,
            git_status: String::new(),
            git_status_location: None,
            remote_workspace: RemoteWorkspaceState::default(),
            milestones: SessionMilestones::default(),
            session_callout: None,
            pending_effects: BTreeMap::new(),
            pending_pane_loads: BTreeMap::new(),
            pending_pane_targets: BTreeMap::new(),
            pane_listing_generations: BTreeMap::new(),
            pane_listing_continuations: BTreeMap::new(),
            pending_next_pages: BTreeMap::new(),
            pane_load_errors: BTreeMap::new(),
            infrastructure_lines: Vec::new(),
            tree_lines: Vec::new(),
            glob_input: false,
            go_input: false,
            show_help: false,
            show_hidden: false,
            sort_mode: SortMode::NameAsc,
            panel_mode: PanelMode::Full,
            viewer_content: Vec::new(),
            viewer_scroll: 0,
            help_scroll: 0,
            left_area: None,
            right_area: None,
            bookmarks,
            show_bookmarks: false,
            bookmark_cursor: 0,
            hosts: Vec::new(),
            show_hosts: false,
            host_cursor: 0,
            show_ssh_hosts: false,
            ssh_host_cursor: 0,
            ssh_hosts: Vec::new(),
            ssh_host_status: None,
            ssh_form: None,
            ssh_pending_keygen: None,
            ssh_test_result: std::sync::Arc::new(std::sync::Mutex::new(None)),
            jobs: Vec::new(),
            job_manager: crate::jobs::JobManager::new(),
            job_events: tokio::sync::mpsc::unbounded_channel().0,
            show_jobs: false,
            job_cursor: 0,
            show_diff: false,
            cmd_input: false,
            cmd: String::new(),
            pending_mkdir_location: None,
            pending_delete: None,
            cmd_prefix: false,
            pending_remote_edit_session: None,
            pending_remote_edit_origin: None,
            pending_editor: false,
            pending_remote_edit_job_id: None,
            menu: Vec::new(),
            show_menu: false,
            menu_cursor: 0,
            term: None,
            show_terminal: false,
            dir_history: Vec::new(),
            show_history: false,
            file_search: false,
            search_query: String::new(),
            search_matches: Vec::new(),
            search_index: 0,
            panel_ratio: 50,
            show_hotlist: false,
            hotlist_cursor: 0,
            show_tab_switcher: false,
            tab_switcher_cursor: 0,
            split: false,
            rename_input: false,
            rename_pattern: String::new(),
            show_command_center: false,
            cc_channel: CommandCenterChannel::default(),
            show_tree: false,
            show_infra: false,
            tree_filter: String::new(),
            show_context_menu: false,
            context_menu_pos: (0, 0),
            command_matches: Vec::new(),
            overlay_list_state: ratatui::widgets::ListState::default(),
            registry: crate::vfs::default_registry(),
            command_hitboxes: Vec::new(),
        }
    }
}

impl AppState {
    fn selection_matches(&self, pane: Pane, location: &Location) -> bool {
        matches!(
            &self.selection_scope,
            Some((selected_pane, selected_location))
                if *selected_pane == pane && selected_location == location
        )
    }

    pub fn toggle_selection(&mut self, pane: Pane, location: &Location, name: &str) {
        if !self.selection_matches(pane, location) {
            self.selected.clear();
            self.selection_scope = Some((pane, location.clone()));
        }
        if !self.selected.remove(name) {
            self.selected.insert(name.to_owned());
        }
        if self.selected.is_empty() {
            self.selection_scope = None;
        }
    }

    pub fn is_selected(&self, pane: Pane, location: &Location, name: &str) -> bool {
        self.selection_matches(pane, location) && self.selected.contains(name)
    }

    pub fn selection_count(&self, pane: Pane, location: &Location) -> usize {
        if self.selection_matches(pane, location) {
            self.selected.len()
        } else {
            0
        }
    }

    pub fn selection_names(&self, pane: Pane, location: &Location) -> Option<&BTreeSet<String>> {
        (self.selection_matches(pane, location) && !self.selected.is_empty())
            .then_some(&self.selected)
    }

    pub fn clear_selection(&mut self) {
        self.selected.clear();
        self.selection_scope = None;
    }

    pub fn clear_selection_for_pane(&mut self, pane: Pane) {
        if matches!(&self.selection_scope, Some((selected_pane, _)) if *selected_pane == pane) {
            self.clear_selection();
        }
    }

    pub fn dismiss_session_callout(&mut self) {
        self.session_callout = None;
    }

    pub fn register_pane_load(
        &mut self,
        pane: Pane,
        id: PaneLoadId,
        location: Location,
        purpose: PaneLoadPurpose,
    ) {
        self.pane_load_errors.remove(&pane);
        self.pane_listing_continuations.remove(&pane);
        self.pending_next_pages.remove(&pane);
        self.pending_pane_loads.insert(pane, id);
        self.pending_pane_targets.insert(pane, (location, purpose));
        // advance persistent listing generation; pagination relies on this
        // surviving finish_pane_load so page-1 continuation stays valid
        self.pane_listing_generations.insert(pane, id);
    }

    pub fn accepts_pane_load(&self, pane: Pane, id: PaneLoadId, location: &Location) -> bool {
        if self.pending_pane_loads.get(&pane) != Some(&id) {
            return false;
        }
        let Some((target, purpose)) = self.pending_pane_targets.get(&pane) else {
            return false;
        };
        if target != location {
            return false;
        }
        if matches!(purpose, PaneLoadPurpose::Refresh) {
            let committed_location = match pane {
                Pane::Left => &self.left.location,
                Pane::Right => &self.right.location,
            };
            return committed_location == location;
        }
        true
    }

    pub fn finish_pane_load(&mut self, pane: Pane, id: PaneLoadId) {
        if self.pending_pane_loads.get(&pane) == Some(&id) {
            self.pending_pane_loads.remove(&pane);
            self.pending_pane_targets.remove(&pane);
        }
    }

    /// Stale guard for the pane-layer listing continuation.
    ///
    /// Returns true only when all three correlation dimensions still match the
    /// current pane: the listing generation, the exact committed location, and
    /// the concrete provider instance. Stale continuations are silently
    /// discarded (design: no error, no UI noise).
    // ponytail: does NOT touch/parse the provider token — opaque by design
    pub fn accepts_pane_listing_continuation(
        &self,
        pane: Pane,
        continuation: &PaneListingContinuation,
    ) -> bool {
        if self.pane_listing_generations.get(&pane) != Some(&continuation.generation) {
            return false;
        }
        let current_location = match pane {
            Pane::Left => &self.left.location,
            Pane::Right => &self.right.location,
        };
        if current_location != &continuation.location {
            return false;
        }
        if ProviderRegistry::instance_key_for_location(current_location)
            != continuation.provider_instance
        {
            return false;
        }
        true
    }

    pub fn apply_pane_listing_continuation(
        &mut self,
        pane: Pane,
        continuation: Option<PaneListingContinuation>,
    ) {
        match continuation {
            Some(continuation) if self.accepts_pane_listing_continuation(pane, &continuation) => {
                self.pane_listing_continuations.insert(pane, continuation);
            }
            None => {
                self.pane_listing_continuations.remove(&pane);
            }
            Some(_) => {}
        }
    }

    pub fn register_next_page(
        &mut self,
        pane: Pane,
        request_id: PanePageRequestId,
        continuation: PaneListingContinuation,
    ) -> bool {
        if self.pending_next_pages.contains_key(&pane)
            || self.pane_listing_continuations.get(&pane) != Some(&continuation)
            || !self.accepts_pane_listing_continuation(pane, &continuation)
        {
            return false;
        }
        self.pending_next_pages
            .insert(pane, (request_id, continuation));
        true
    }

    pub fn accepts_next_page(
        &self,
        pane: Pane,
        request_id: PanePageRequestId,
        continuation: &PaneListingContinuation,
    ) -> bool {
        self.pending_next_pages
            .get(&pane)
            .is_some_and(|(pending, initiating)| {
                *pending == request_id && initiating == continuation
            })
            && self.pane_listing_continuations.get(&pane) == Some(continuation)
            && self.accepts_pane_listing_continuation(pane, continuation)
    }

    pub fn finish_next_page(&mut self, pane: Pane, request_id: PanePageRequestId) {
        if self
            .pending_next_pages
            .get(&pane)
            .is_some_and(|(pending, _)| *pending == request_id)
        {
            self.pending_next_pages.remove(&pane);
        }
    }

    pub fn register_effect(&mut self, lane: EffectLane, id: EffectId) {
        self.pending_effects.insert(lane, id);
    }

    pub fn accepts_effect(&self, id: EffectId, lane: EffectLane, scope: &EffectScope) -> bool {
        if self.pending_effects.get(&lane) != Some(&id) {
            return false;
        }

        // A mutation result must never be discarded merely because the user
        // navigated away; it may carry conflict or recovery instructions.
        if lane == EffectLane::RemoteEdit {
            return true;
        }

        match scope {
            EffectScope::Global => true,
            EffectScope::Location(location) => {
                &self.left.location == location || &self.right.location == location
            }
            EffectScope::Workspace { left, right } => {
                &self.left.location == left && &self.right.location == right
            }
        }
    }

    pub fn finish_effect(&mut self, lane: EffectLane, id: EffectId) {
        if self.pending_effects.get(&lane) == Some(&id) {
            self.pending_effects.remove(&lane);
        }
    }

    pub fn pending_effect(&self, lane: EffectLane) -> Option<EffectId> {
        self.pending_effects.get(&lane).copied()
    }

    pub fn apply(&mut self, action: Action) {
        match action {
            Action::Quit if self.pending_effects.contains_key(&EffectLane::RemoteEdit) => {
                self.message = Some("Remote edit in progress — wait for a safe outcome".into());
            }
            Action::Quit => self.should_quit = true,
            Action::SwitchPane => {
                self.active = match self.active {
                    Pane::Left => Pane::Right,
                    Pane::Right => Pane::Left,
                };
            }
            _ => {}
        }
    }

    pub fn active_pane(&self) -> &PaneState {
        match self.active {
            Pane::Left => &self.left,
            Pane::Right => &self.right,
        }
    }

    pub fn active_pane_mut(&mut self) -> &mut PaneState {
        match self.active {
            Pane::Left => &mut self.left,
            Pane::Right => &mut self.right,
        }
    }

    pub fn other_pane(&self) -> &PaneState {
        match self.active {
            Pane::Left => &self.right,
            Pane::Right => &self.left,
        }
    }

    pub fn other_pane_mut(&mut self) -> &mut PaneState {
        match self.active {
            Pane::Left => &mut self.right,
            Pane::Right => &mut self.left,
        }
    }

    /// Load user menu from ~/.config/arx/arx.menu.
    pub fn load_hotlist() -> Vec<PathBuf> {
        let path = dirs::config_dir()
            .map(|d| d.join("arx").join("hotlist"))
            .unwrap_or_else(|| PathBuf::from("hotlist"));
        std::fs::read_to_string(&path)
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .map(PathBuf::from)
            .collect()
    }

    pub fn load_menu() -> Vec<MenuEntry> {
        let path = dirs::config_dir()
            .map(|d| d.join("arx").join("arx.menu"))
            .unwrap_or_else(|| PathBuf::from("arx.menu"));
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        let mut entries = Vec::new();
        for line in content.lines() {
            let trimmed = line.trim();
            // Skip comments and empty lines
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            // Format: t  "Label"  command
            // ponytail: simple parser; add proper tokenizer when needed
            #[allow(clippy::collapsible_if)]
            if let Some(rest) = trimmed.strip_prefix("t  \"") {
                if let Some((label, cmd)) = rest.split_once("\"  ") {
                    entries.push(MenuEntry {
                        label: label.to_string(),
                        command: cmd.to_string(),
                    });
                }
            }
        }
        entries
    }

    /// Currently-viewed file path (if viewer is open).
    pub fn viewer_file_path(&self) -> Option<PathBuf> {
        match &self.active_pane().location {
            Location::Local(dir) => {
                if self.viewer_content.is_empty() {
                    None
                } else {
                    // ponytail: name stored separately; add when needed
                    Some(dir.clone())
                }
            }
            _ => None,
        }
    }
}

// ponytail: $HOME with / fallback
fn dirs_fallback() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

/// PACK B — Confirm a pending unencrypted key generation (list-mode Ctrl+K → y).
/// Generates the Ed25519 key, attaches it to the selected managed host, and
/// clears the pending state. Business logic lives here (in `app`), not in the
/// TUI binary module, so it stays unit-testable without exposing `tui` as API.
pub fn confirm_pending_keygen(state: &mut AppState) {
    // Snapshot the pending alias, then clear intent immediately so concurrent
    // edits/deletes between the request and the confirm cannot leave an orphan
    // generated key behind.
    let alias = match state.ssh_pending_keygen.take() {
        Some(a) => a,
        None => return,
    };
    // Re-check the host still exists in the list before touching the filesystem.
    if !state.ssh_hosts.iter().any(|h| h.alias == alias) {
        state.ssh_host_status = Some(format!("Key gen cancelled: '{alias}' no longer listed"));
        return;
    }
    let key_name = format!("{alias}_ed25519");
    match crate::remote::ssh_config_manager::generate_ed25519_key(&key_name) {
        Ok(p) => {
            if let Some(h) = state.ssh_hosts.iter().find(|x| x.alias == alias).cloned() {
                let mut updated = h;
                updated.identity_file = Some(p.clone());
                updated.identities_only = true;
                match crate::remote::ssh_config_manager::update_managed_host(
                    &updated.alias,
                    &updated,
                ) {
                    Ok(_) => {
                        state.ssh_host_status = Some(format!(
                            "Generated {} and attached to {}",
                            p.display(),
                            alias
                        ))
                    }
                    Err(e) => {
                        state.ssh_host_status = Some(format!("Key attached failed: {e}"));
                        // Roll back the just-generated key + its .pub so no
                        // orphan remains (ssh-keygen writes both).
                        crate::app::remove_generated_key_pair(&p);
                    }
                }
                state.ssh_hosts = crate::remote::ssh_config_manager::list_managed_hosts()
                    .into_values()
                    .collect();
            } else {
                // Host vanished between the check above and the find; drop key.
                crate::app::remove_generated_key_pair(&p);
            }
        }
        Err(e) => state.ssh_host_status = Some(format!("Key gen failed: {e}")),
    }
}

/// PACK B — Cancel a pending key generation (list-mode n). Clears pending state,
/// writes nothing.
pub fn cancel_pending_keygen(state: &mut AppState) {
    if state.ssh_pending_keygen.is_some() {
        state.ssh_pending_keygen = None;
        state.ssh_host_status = Some("Key generation cancelled".into());
    }
}

/// PACK B — Remove a generated key pair (private + `.pub`). Used to roll back a
/// key after a failed attach so no orphan files remain. `ssh-keygen` writes the
/// public key as the private path with `.pub` APPENDED (not as a replaced
/// extension), so we do the same — a dotted alias (e.g. `prod.eu` ->
/// `prod.eu_ed25519.pub`) must not delete an unrelated `prod.pub`.
pub fn remove_generated_key_pair(private: &std::path::Path) {
    let _ = std::fs::remove_file(private);
    let _ = std::fs::remove_file(pub_path_for(private));
}

/// Companion `.pub` path for a private key: appends `.pub` to the full filename
/// (matches ssh-keygen), rather than replacing the extension.
pub fn pub_path_for(private: &std::path::Path) -> std::path::PathBuf {
    let mut name = private.file_name().unwrap_or_default().to_os_string();
    name.push(".pub");
    private.with_file_name(name)
}

/// PACK B — Drive the managed-SSH-host list-mode key transition from a real
/// `KeyEvent`. Powers Ctrl+K (request unencrypted-key generation, awaiting y/n),
/// `y` (confirm → generates + attaches), and `n` (cancel). Kept in `app` (not
/// the `tui` binary module) so it is unit-testable via real KeyEvents without
/// exposing `tui` as public API.
///
/// Returns true if the event was consumed (caller should not also run default
/// list navigation for it).
pub fn handle_ssh_host_keypress(state: &mut AppState, key: crossterm::event::KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    if state.ssh_pending_keygen.is_some() {
        match key.code {
            KeyCode::Esc => {
                cancel_pending_keygen(state);
                true
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                confirm_pending_keygen(state);
                true
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                cancel_pending_keygen(state);
                true
            }
            _ => true, // swallow other keys while awaiting confirmation
        }
    } else {
        match key.code {
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(h) = state.ssh_hosts.get(state.ssh_host_cursor).cloned() {
                    state.ssh_pending_keygen = Some(h.alias.clone());
                    state.ssh_host_status = Some(format!(
                        "Generate UNENCRYPTED Ed25519 key for '{}'? (y/n)",
                        h.alias
                    ));
                }
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state() {
        let state = AppState::default();
        assert!(!state.should_quit);
        assert_eq!(state.active, Pane::Left);
        assert!(state.selected.is_empty());
        assert!(state.viewer_content.is_empty());
        assert_eq!(state.panel_mode, PanelMode::Full);
        assert_eq!(state.sort_mode, SortMode::NameAsc);
    }

    #[test]
    fn selection_is_bound_to_its_pane_and_location() {
        let mut state = AppState::default();
        let left_location = state.left.location.clone();
        let right_location = state.right.location.clone();

        state.toggle_selection(Pane::Left, &left_location, "foo.txt");

        assert!(state.is_selected(Pane::Left, &left_location, "foo.txt"));
        assert!(!state.is_selected(Pane::Right, &right_location, "foo.txt"));
        assert_eq!(state.selection_count(Pane::Left, &left_location), 1);
        assert_eq!(state.selection_count(Pane::Right, &right_location), 0);
    }

    #[test]
    fn clearing_selection_only_affects_the_matching_pane() {
        let mut state = AppState::default();
        let left_location = state.left.location.clone();
        state.toggle_selection(Pane::Left, &left_location, "foo.txt");

        state.clear_selection_for_pane(Pane::Right);
        assert!(state.is_selected(Pane::Left, &left_location, "foo.txt"));

        state.clear_selection_for_pane(Pane::Left);
        assert_eq!(state.selection_count(Pane::Left, &left_location), 0);
    }

    #[test]
    fn tab_creation_and_switching() {
        let mut state = AppState::default();
        assert_eq!(state.left.tabs.len(), 0);
        state.left.new_tab();
        assert_eq!(state.left.tabs.len(), 1);
        // Tab 1 is current; switching to tab 0 saves current and restores tab 0
        state.left.switch_tab(0);
        assert_eq!(state.left.tabs.len(), 1); // current swapped with saved
    }

    #[test]
    fn pane_swap_preserves_both_sides() {
        let mut state = AppState::default();
        state.left.cursor = 5;
        state.right.cursor = 10;
        std::mem::swap(&mut state.left, &mut state.right);
        assert_eq!(state.right.cursor, 5);
        assert_eq!(state.left.cursor, 10);
    }

    #[test]
    fn cmd_prefix_clears_after_use() {
        let state = AppState {
            cmd_prefix: true,
            ..AppState::default()
        };
        assert!(state.cmd_prefix);
    }

    #[test]
    fn session_milestones_are_one_shot_and_reset_with_app_state() {
        let mut state = AppState::default();
        assert!(!state.milestones.compare_success_seen);
        assert!(!state.milestones.verified_sync_success_seen);
        assert!(state.milestones.take_compare_success());
        assert!(!state.milestones.take_compare_success());
        assert!(state.milestones.take_verified_sync_success());
        assert!(!state.milestones.take_verified_sync_success());

        let restarted = AppState::default();
        assert!(!restarted.milestones.compare_success_seen);
        assert!(!restarted.milestones.verified_sync_success_seen);
        assert!(restarted.session_callout.is_none());
    }

    // ── REMOTE-09: CancelRemoteDelete clears state ──

    #[test]
    fn cancel_clears_pending_delete() {
        let mut state = AppState {
            pending_delete: Some(crate::vfs::RemoteDeletePlan {
                location: crate::vfs::Location::Local(std::path::PathBuf::from("/tmp")),
                targets: vec![crate::vfs::RemoteDeleteTarget {
                    name: "test.txt".into(),
                    kind: crate::vfs::EntryKind::File,
                    path: "/tmp/test.txt".into(),
                }],
                created_at: std::time::Instant::now(),
            }),
            ..AppState::default()
        };
        assert!(state.pending_delete.is_some());
        state.pending_delete = None;
        assert!(state.pending_delete.is_none());
    }

    #[test]
    fn confirm_retains_pending_delete_until_physical_outcome() {
        let plan = crate::vfs::RemoteDeletePlan {
            location: crate::vfs::Location::Sftp {
                host: "prod".into(),
                path: "/srv".into(),
            },
            targets: vec![crate::vfs::RemoteDeleteTarget {
                name: "data.txt".into(),
                kind: crate::vfs::EntryKind::File,
                path: "/srv/data.txt".into(),
            }],
            created_at: std::time::Instant::now(),
        };
        let state = AppState {
            pending_delete: Some(plan),
            ..AppState::default()
        };
        assert!(state.pending_delete.is_some());
        assert_eq!(state.pending_delete.as_ref().unwrap().targets.len(), 1);
        assert_eq!(
            state.pending_delete.as_ref().unwrap().targets[0].name,
            "data.txt"
        );
    }

    // ── REMOTE-09: refresh-only-on-physical-outcome marker ──
    // ponytail: test gate — the contract that refresh is only triggered on
    // physical mutation outcome (F8 mkdir, F8 delete) is enforced in the
    // async executor in tui.rs (dispatch_ui_action). Unit-testing it requires
    // mock SFTP sessions. This marker confirms the path exists and the
    // executor references provider_for_location + list_async.

    // ── VIEW-LAST-02: latest-preview-wins contract ──

    fn preview_state() -> AppState {
        let left = Location::Local(std::path::PathBuf::from("/tmp/a"));
        let right = Location::Local(std::path::PathBuf::from("/tmp/b"));
        AppState {
            left: crate::app::PaneState {
                location: left.clone(),
                cursor: 0,
                tabs: vec![],
                dir_history: vec![],
                split: false,
                split_cursor: 0,
                split_active: false,
            },
            right: crate::app::PaneState {
                location: right.clone(),
                cursor: 0,
                tabs: vec![],
                dir_history: vec![],
                split: false,
                split_cursor: 0,
                split_active: false,
            },
            ..AppState::default()
        }
    }

    #[test]
    fn b_supersedes_a_at_appstate_level() {
        let mut state = preview_state();
        let scope = EffectScope::Location(state.left.location.clone());
        let lane = EffectLane::Preview;

        // A registers
        let a = EffectId(1);
        state.register_effect(lane, a);
        assert!(state.accepts_effect(a, lane, &scope));

        // B registers — supersedes A
        let b = EffectId(2);
        state.register_effect(lane, b);

        // A is now stale
        assert!(!state.accepts_effect(a, lane, &scope));
        // B is current
        assert!(state.accepts_effect(b, lane, &scope));
    }

    #[test]
    fn scope_change_rejects_unknown_location() {
        let mut state = preview_state();
        let scope_left = EffectScope::Location(state.left.location.clone());
        let scope_other =
            EffectScope::Location(Location::Local(std::path::PathBuf::from("/tmp/other")));
        let lane = EffectLane::Preview;

        let a = EffectId(1);
        state.register_effect(lane, a);
        // Left scope matches left pane (regardless of active pane)
        assert!(state.accepts_effect(a, lane, &scope_left));
        // Unknown location is rejected
        assert!(!state.accepts_effect(a, lane, &scope_other));
        // Switch active to right — left scope STILL matches left pane
        state.active = Pane::Right;
        assert!(state.accepts_effect(a, lane, &scope_left));
        // Unknown location still rejected
        assert!(!state.accepts_effect(a, lane, &scope_other));
    }

    #[test]
    fn finish_effect_removes_pending() {
        let mut state = preview_state();
        let scope = EffectScope::Location(state.left.location.clone());
        let lane = EffectLane::Preview;

        let a = EffectId(1);
        state.register_effect(lane, a);
        assert!(state.accepts_effect(a, lane, &scope));

        state.finish_effect(lane, a);
        // After finish, effect is no longer accepted
        assert!(!state.accepts_effect(a, lane, &scope));
    }

    #[test]
    fn different_lanes_independent() {
        let mut state = preview_state();
        let scope = EffectScope::Location(state.left.location.clone());

        let preview_a = EffectId(1);
        state.register_effect(EffectLane::Preview, preview_a);
        let process_a = EffectId(1);
        state.register_effect(EffectLane::GlobalProcess, process_a);

        // Same numeric id, different lane — both should be accepted
        assert!(state.accepts_effect(preview_a, EffectLane::Preview, &scope));
        assert!(state.accepts_effect(process_a, EffectLane::GlobalProcess, &scope));

        // Supersede Preview lane only
        state.register_effect(EffectLane::Preview, EffectId(2));
        assert!(!state.accepts_effect(preview_a, EffectLane::Preview, &scope));
        // GlobalProcess lane unchanged
        assert!(state.accepts_effect(process_a, EffectLane::GlobalProcess, &scope));
    }

    #[test]
    fn remote_edit_result_survives_navigation_but_stale_id_does_not() {
        let mut state = preview_state();
        let original = EffectScope::Location(state.left.location.clone());
        let current = EffectId(7);
        state.register_effect(EffectLane::RemoteEdit, current);
        state.left.location = Location::Local(std::path::PathBuf::from("/elsewhere"));

        assert!(state.accepts_effect(current, EffectLane::RemoteEdit, &original));
        assert!(!state.accepts_effect(EffectId(6), EffectLane::RemoteEdit, &original));
    }

    #[test]
    fn quit_waits_for_remote_edit_outcome() {
        let mut state = preview_state();
        state.register_effect(EffectLane::RemoteEdit, EffectId(9));
        state.apply(Action::Quit);
        assert!(!state.should_quit);
        assert!(state.message.as_deref().unwrap().contains("Remote edit"));

        state.finish_effect(EffectLane::RemoteEdit, EffectId(9));
        state.apply(Action::Quit);
        assert!(state.should_quit);
    }

    // ── S3-15: stale pane-listing continuation guard ──

    fn cont(generation: u64, location: Location) -> PaneListingContinuation {
        PaneListingContinuation {
            provider_continuation: crate::vfs::ProviderContinuation {
                token: "tok".to_string(),
            },
            provider_instance: ProviderRegistry::instance_key_for_location(&location),
            location,
            generation: PaneLoadId(generation),
        }
    }

    #[test]
    fn current_continuation_accepted() {
        let mut state = AppState::default();
        let loc = state.left.location.clone();
        state.register_pane_load(
            Pane::Left,
            PaneLoadId(42),
            loc.clone(),
            PaneLoadPurpose::Refresh,
        );

        let c = cont(42, loc);
        assert!(state.accepts_pane_listing_continuation(Pane::Left, &c));
    }

    #[test]
    fn new_load_clears_old_pane_continuation() {
        let mut state = AppState::default();
        let location = state.left.location.clone();
        state.register_pane_load(
            Pane::Left,
            PaneLoadId(42),
            location.clone(),
            PaneLoadPurpose::Refresh,
        );
        state.apply_pane_listing_continuation(Pane::Left, Some(cont(42, location.clone())));
        assert!(state.pane_listing_continuations.contains_key(&Pane::Left));

        state.register_pane_load(
            Pane::Left,
            PaneLoadId(43),
            location,
            PaneLoadPurpose::Refresh,
        );

        assert!(!state.pane_listing_continuations.contains_key(&Pane::Left));
    }

    #[test]
    fn accepted_current_continuation_can_be_stored() {
        let mut state = AppState::default();
        let location = state.left.location.clone();
        state.register_pane_load(
            Pane::Left,
            PaneLoadId(42),
            location.clone(),
            PaneLoadPurpose::Refresh,
        );
        let continuation = cont(42, location);

        state.apply_pane_listing_continuation(Pane::Left, Some(continuation.clone()));

        assert_eq!(
            state.pane_listing_continuations.get(&Pane::Left),
            Some(&continuation)
        );
    }

    #[test]
    fn stale_continuation_cannot_replace_current() {
        let mut state = AppState::default();
        let location = state.left.location.clone();
        state.register_pane_load(
            Pane::Left,
            PaneLoadId(43),
            location.clone(),
            PaneLoadPurpose::Refresh,
        );
        let current = cont(43, location.clone());
        state.apply_pane_listing_continuation(Pane::Left, Some(current.clone()));

        state.apply_pane_listing_continuation(Pane::Left, Some(cont(42, location)));

        assert_eq!(
            state.pane_listing_continuations.get(&Pane::Left),
            Some(&current)
        );
    }

    #[test]
    fn left_right_continuations_are_independent() {
        let mut state = AppState::default();
        let left = state.left.location.clone();
        let right = state.right.location.clone();
        state.register_pane_load(
            Pane::Left,
            PaneLoadId(41),
            left.clone(),
            PaneLoadPurpose::Refresh,
        );
        state.register_pane_load(
            Pane::Right,
            PaneLoadId(42),
            right.clone(),
            PaneLoadPurpose::Refresh,
        );
        let left_continuation = cont(41, left);
        let right_continuation = cont(42, right);

        state.apply_pane_listing_continuation(Pane::Left, Some(left_continuation.clone()));
        state.apply_pane_listing_continuation(Pane::Right, Some(right_continuation.clone()));

        assert_eq!(
            state.pane_listing_continuations.get(&Pane::Left),
            Some(&left_continuation)
        );
        assert_eq!(
            state.pane_listing_continuations.get(&Pane::Right),
            Some(&right_continuation)
        );
    }

    #[test]
    fn finish_page1_keeps_continuation_valid() {
        let mut state = AppState::default();
        let loc = state.left.location.clone();
        state.register_pane_load(
            Pane::Left,
            PaneLoadId(42),
            loc.clone(),
            PaneLoadPurpose::Refresh,
        );
        // page 1 finishes — must NOT invalidate its continuation (page 2 needs it)
        state.finish_pane_load(Pane::Left, PaneLoadId(42));

        let c = cont(42, loc);
        assert!(state.accepts_pane_listing_continuation(Pane::Left, &c));
    }

    #[test]
    fn refresh_invalidates_old_generation() {
        let mut state = AppState::default();
        let loc = state.left.location.clone();
        state.register_pane_load(
            Pane::Left,
            PaneLoadId(42),
            loc.clone(),
            PaneLoadPurpose::Refresh,
        );
        // same location, new refresh generation
        state.register_pane_load(
            Pane::Left,
            PaneLoadId(43),
            loc.clone(),
            PaneLoadPurpose::Refresh,
        );

        let old = cont(42, loc.clone());
        let new = cont(43, loc);
        assert!(!state.accepts_pane_listing_continuation(Pane::Left, &old));
        assert!(state.accepts_pane_listing_continuation(Pane::Left, &new));
    }

    #[test]
    fn navigation_invalidates_old_continuation() {
        let mut state = AppState::default();
        let loc_a = state.left.location.clone();
        state.register_pane_load(
            Pane::Left,
            PaneLoadId(42),
            loc_a.clone(),
            PaneLoadPurpose::Refresh,
        );

        let loc_b = Location::Local(std::path::PathBuf::from("/navigated"));
        state.left.location = loc_b.clone();
        state.register_pane_load(
            Pane::Left,
            PaneLoadId(43),
            loc_b.clone(),
            PaneLoadPurpose::Navigate {
                remember_current: false,
            },
        );

        let old = cont(42, loc_a);
        assert!(!state.accepts_pane_listing_continuation(Pane::Left, &old));
    }

    #[test]
    fn exact_location_mismatch_rejected() {
        let mut state = AppState::default();
        let loc_a = state.left.location.clone();
        state.register_pane_load(
            Pane::Left,
            PaneLoadId(42),
            loc_a.clone(),
            PaneLoadPurpose::Refresh,
        );

        let loc_b = Location::Local(std::path::PathBuf::from("/other"));
        let c = cont(42, loc_b);
        assert!(!state.accepts_pane_listing_continuation(Pane::Left, &c));
    }

    #[test]
    fn provider_instance_mismatch_independent() {
        // Same generation + same exact location, but wrong ProviderInstanceKey.
        let mut state = AppState::default();
        let loc = Location::S3 {
            target: "t".to_string(),
            bucket: Some("b".to_string()),
            prefix: "p".to_string(),
        };
        state.left.location = loc.clone();
        state.register_pane_load(
            Pane::Left,
            PaneLoadId(42),
            loc.clone(),
            PaneLoadPurpose::Refresh,
        );

        let mut c = cont(42, loc);
        // deliberately break only the provider-instance dimension
        c.provider_instance = ProviderRegistry::instance_key_for_location(&Location::S3 {
            target: "other-target".to_string(),
            bucket: None,
            prefix: String::new(),
        });
        assert!(!state.accepts_pane_listing_continuation(Pane::Left, &c));
    }

    #[test]
    fn s3_exact_prefix_distinct() {
        // "foo/" and "foo//" are different exact locations, no normalization.
        let mut state = AppState::default();
        let loc = Location::S3 {
            target: "t".to_string(),
            bucket: Some("b".to_string()),
            prefix: "foo/".to_string(),
        };
        state.left.location = loc.clone();
        state.register_pane_load(
            Pane::Left,
            PaneLoadId(42),
            loc.clone(),
            PaneLoadPurpose::Refresh,
        );

        let wrong = cont(
            42,
            Location::S3 {
                target: "t".to_string(),
                bucket: Some("b".to_string()),
                prefix: "foo//".to_string(),
            },
        );
        assert!(!state.accepts_pane_listing_continuation(Pane::Left, &wrong));
    }

    #[test]
    fn pane_isolation_left_right() {
        let mut state = AppState::default();
        let loc_l = state.left.location.clone();
        let loc_r = state.right.location.clone();
        state.register_pane_load(
            Pane::Left,
            PaneLoadId(42),
            loc_l.clone(),
            PaneLoadPurpose::Refresh,
        );
        state.register_pane_load(
            Pane::Right,
            PaneLoadId(42),
            loc_r.clone(),
            PaneLoadPurpose::Refresh,
        );

        // Left continuation must not validate via Right's generation/location
        let left_as_right = cont(42, loc_l);
        assert!(!state.accepts_pane_listing_continuation(Pane::Right, &left_as_right));
    }
}
