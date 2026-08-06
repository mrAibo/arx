use std::path::PathBuf;

use crate::vfs::Location;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    Up,
    Down,
    Enter,
    Back,
    Refresh,
    OpenJobs,
    OpenHosts,
}

#[derive(Debug)]
pub struct AppState {
    pub should_quit: bool,
    pub current_location: Location,
    /// 0-based index of highlighted entry in the active pane
    pub cursor: usize,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            should_quit: false,
            current_location: Location::Local(
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            ),
            cursor: 0,
        }
    }
}

impl AppState {
    pub fn apply(&mut self, action: Action) {
        if action == Action::Quit {
            self.should_quit = true;
        }
    }
}
