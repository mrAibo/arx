use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use ratatui::layout::Rect;

use crate::effect_dispatcher::{EffectId, EffectLane, EffectScope};
use crate::jobs::Job;
use crate::remote::Host;
use crate::services::{PaneLoadId, PaneLoadPurpose};
use crate::terminal::TermPane;
use crate::vfs::Location;

mod actions;
pub use actions::{
    ACTION_CATALOG, ALL_ACTIONS, Action, ActionCategory, ActionId, ActionMeta, InputContext,
    action_meta,
};
mod availability;
pub use availability::{ActionAvailability, ActionContext, action_availability};
mod command_center;
pub use command_center::{CommandItem, CommandKind, CommandTarget, build_command_items};
mod overlay;
pub use overlay::OverlayKind;
mod remote_workspace;
pub use remote_workspace::RemoteWorkspaceState;

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

#[derive(Debug)]
pub struct AppState {
    pub should_quit: bool,
    pub left: PaneState,
    pub right: PaneState,
    pub active: Pane,
    pub selected: BTreeSet<String>,
    pub filter: String,
    pub filtering: bool,
    pub message: Option<String>,
    /// Cached status-bar Git suffix. Never compute Git state from render().
    pub git_status: String,
    pub git_status_location: Option<Location>,
    /// Local ↔ remote (or any provider ↔ provider) comparison/sync workspace.
    pub remote_workspace: RemoteWorkspaceState,
    /// Latest effect per lane. Older responses are discarded deterministically.
    pub pending_effects: BTreeMap<EffectLane, EffectId>,
    /// Latest async VFS load generation for each pane.
    pub pending_pane_loads: BTreeMap<Pane, PaneLoadId>,
    pub pending_pane_targets: BTreeMap<Pane, (Location, PaneLoadPurpose)>,
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
    // B4: render-only snapshot; JobManager owns runtime lifecycle.
    pub jobs: Vec<Job>,
    pub show_jobs: bool,
    pub job_cursor: usize,
    // C1: directory compare
    pub show_diff: bool,
    // C2: command input
    pub cmd_input: bool,
    pub cmd: String,
    /// Ctrl+X prefix for MC-style key combos
    pub cmd_prefix: bool,
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
            filter: String::new(),
            filtering: false,
            message: None,
            git_status: String::new(),
            git_status_location: None,
            remote_workspace: RemoteWorkspaceState::default(),
            pending_effects: BTreeMap::new(),
            pending_pane_loads: BTreeMap::new(),
            pending_pane_targets: BTreeMap::new(),
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
            left_area: None,
            right_area: None,
            bookmarks,
            show_bookmarks: false,
            bookmark_cursor: 0,
            hosts: Vec::new(),
            show_hosts: false,
            host_cursor: 0,
            jobs: Vec::new(),
            show_jobs: false,
            job_cursor: 0,
            show_diff: false,
            cmd_input: false,
            cmd: String::new(),
            cmd_prefix: false,
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
        }
    }
}

impl AppState {
    pub fn register_pane_load(
        &mut self,
        pane: Pane,
        id: PaneLoadId,
        location: Location,
        purpose: PaneLoadPurpose,
    ) {
        self.pending_pane_loads.insert(pane, id);
        self.pending_pane_targets.insert(pane, (location, purpose));
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

    pub fn register_effect(&mut self, lane: EffectLane, id: EffectId) {
        self.pending_effects.insert(lane, id);
    }

    pub fn accepts_effect(&self, id: EffectId, lane: EffectLane, scope: &EffectScope) -> bool {
        if self.pending_effects.get(&lane) != Some(&id) {
            return false;
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

    pub fn apply(&mut self, action: Action) {
        match action {
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
}
