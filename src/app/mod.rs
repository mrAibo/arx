use std::collections::BTreeSet;
use std::path::PathBuf;

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
    /// Filenames selected in the active pane's current directory.
    pub selected: BTreeSet<String>,
    /// Quick-filter text; empty = no filter applied.
    pub filter: String,
    /// True while the user is composing the filter (captures typed chars).
    pub filtering: bool,
}

impl Default for AppState {
    fn default() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let home = dirs_fallback();
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
}

// ponytail: $HOME with / fallback; add proper dirs crate when config moves there
fn dirs_fallback() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}
