use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::jobs::Job;
use crate::remote::Host;
use crate::vfs::Location;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    Up,
    Down,
    Enter,
    Back,
    SwitchPane,
    ToggleSelect,
    Refresh,
    OpenJobs,
    OpenHosts,
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
    pub glob_input: bool,
    pub go_input: bool,
    pub show_help: bool,
    pub show_hidden: bool,
    // A3: sort order
    pub sort_mode: SortMode,
    // A1: file viewer
    pub viewer_content: Vec<String>,
    pub viewer_scroll: usize,
    // A4: bookmarks
    pub bookmarks: Vec<Location>,
    pub show_bookmarks: bool,
    pub bookmark_cursor: usize,
    // B3: host panel
    pub hosts: Vec<Host>,
    pub show_hosts: bool,
    pub host_cursor: usize,
    // B4: job queue
    pub jobs: Vec<Job>,
    pub show_jobs: bool,
    pub job_cursor: usize,
    // C1: directory compare
    pub show_diff: bool,
    // C2: command input
    pub cmd_input: bool,
    pub cmd: String,
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
            },
            right: PaneState {
                location: Location::Local(home),
                cursor: 0,
            },
            active: Pane::Left,
            selected: BTreeSet::new(),
            filter: String::new(),
            filtering: false,
            message: None,
            glob_input: false,
            go_input: false,
            show_help: false,
            show_hidden: false,
            sort_mode: SortMode::NameAsc,
            viewer_content: Vec::new(),
            viewer_scroll: 0,
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
        }
    }
}

impl AppState {
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

    pub fn other_pane_mut(&mut self) -> &mut PaneState {
        match self.active {
            Pane::Left => &mut self.right,
            Pane::Right => &mut self.left,
        }
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
