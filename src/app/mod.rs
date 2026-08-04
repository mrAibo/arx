#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    Refresh,
    OpenJobs,
    OpenHosts,
}

#[derive(Debug, Default)]
pub struct AppState {
    pub should_quit: bool,
}

impl AppState {
    pub fn apply(&mut self, action: Action) {
        if action == Action::Quit {
            self.should_quit = true;
        }
    }
}
