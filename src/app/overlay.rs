use super::AppState;

/// Exclusive top-level UI surface.
///
/// ARX still stores the legacy `show_*` booleans during the migration, but
/// all new transitions must go through this API. That gives us state-machine
/// semantics now and lets a later mechanical PR replace the booleans with a
/// single `overlay: Option<OverlayKind>` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OverlayKind {
    Help,
    Bookmarks,
    Hosts,
    Jobs,
    UserMenu,
    History,
    Hotlist,
    TabSwitcher,
    CommandCenter,
    Tree,
    Infrastructure,
    ContextMenu,
    SyncPreview,
    SshHosts,
}

impl AppState {
    pub fn active_overlay(&self) -> Option<OverlayKind> {
        // Keep precedence identical and explicit during migration.
        if self.show_command_center {
            Some(OverlayKind::CommandCenter)
        } else if self.show_help {
            Some(OverlayKind::Help)
        } else if self.show_bookmarks {
            Some(OverlayKind::Bookmarks)
        } else if self.show_hosts {
            Some(OverlayKind::Hosts)
        } else if self.show_jobs {
            Some(OverlayKind::Jobs)
        } else if self.show_menu {
            Some(OverlayKind::UserMenu)
        } else if self.show_history {
            Some(OverlayKind::History)
        } else if self.show_hotlist {
            Some(OverlayKind::Hotlist)
        } else if self.show_tab_switcher {
            Some(OverlayKind::TabSwitcher)
        } else if self.show_tree {
            Some(OverlayKind::Tree)
        } else if self.show_infra {
            Some(OverlayKind::Infrastructure)
        } else if self.remote_workspace.preview_open {
            Some(OverlayKind::SyncPreview)
        } else if self.show_context_menu {
            Some(OverlayKind::ContextMenu)
        } else if self.show_ssh_hosts {
            Some(OverlayKind::SshHosts)
        } else {
            None
        }
    }

    pub fn close_all_overlays(&mut self) {
        self.show_help = false;
        self.show_bookmarks = false;
        self.show_hosts = false;
        self.show_jobs = false;
        self.show_menu = false;
        self.show_history = false;
        self.show_hotlist = false;
        self.show_tab_switcher = false;
        self.show_command_center = false;
        self.show_tree = false;
        self.show_infra = false;
        self.show_context_menu = false;
        self.show_ssh_hosts = false;
    }

    pub fn open_overlay(&mut self, overlay: OverlayKind) {
        self.close_all_overlays();
        match overlay {
            OverlayKind::Help => self.show_help = true,
            OverlayKind::Bookmarks => {
                self.show_bookmarks = true;
                self.bookmark_cursor = 0;
            }
            OverlayKind::Hosts => {
                self.show_hosts = true;
                self.host_cursor = 0;
            }
            OverlayKind::Jobs => {
                self.show_jobs = true;
                self.job_cursor = 0;
            }
            OverlayKind::UserMenu => {
                self.show_menu = true;
                self.menu_cursor = 0;
            }
            OverlayKind::History => self.show_history = true,
            OverlayKind::Hotlist => {
                self.show_hotlist = true;
                self.hotlist_cursor = 0;
            }
            OverlayKind::TabSwitcher => {
                self.show_tab_switcher = true;
                self.tab_switcher_cursor = 0;
            }
            OverlayKind::CommandCenter => {
                self.show_command_center = true;
                self.overlay_list_state = ratatui::widgets::ListState::default();
            }
            OverlayKind::Tree => self.show_tree = true,
            OverlayKind::Infrastructure => self.show_infra = true,
            OverlayKind::ContextMenu => self.show_context_menu = true,
            OverlayKind::SshHosts => {
                self.show_ssh_hosts = true;
                self.ssh_host_cursor = 0;
                // Reload managed hosts on open
                self.ssh_hosts = crate::remote::ssh_config_manager::list_managed_hosts()
                    .into_values()
                    .collect();
                self.ssh_host_status = None;
            }
            // SyncPreview uses `active_overlay()` as an exclusive semantic
            // owner, but its visible state is the presence of a workspace
            // plan. No second mutable boolean is introduced.
            OverlayKind::SyncPreview => {
                self.show_diff = true;
                self.remote_workspace.preview_open = true;
                self.overlay_list_state.select(None);
            }
        }
    }

    pub fn close_overlay(&mut self, overlay: OverlayKind) {
        if self.active_overlay() != Some(overlay) {
            return;
        }
        if overlay == OverlayKind::SyncPreview {
            self.remote_workspace.preview_open = false;
        } else {
            self.close_all_overlays();
        }
    }

    pub fn toggle_overlay(&mut self, overlay: OverlayKind) {
        if self.active_overlay() == Some(overlay) {
            self.close_overlay(overlay);
        } else {
            self.open_overlay(overlay);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_overlay_closes_previous_overlay() {
        let mut state = AppState::default();
        state.open_overlay(OverlayKind::Help);
        assert_eq!(state.active_overlay(), Some(OverlayKind::Help));

        state.open_overlay(OverlayKind::Jobs);
        assert_eq!(state.active_overlay(), Some(OverlayKind::Jobs));
        assert!(!state.show_help);
        assert!(state.show_jobs);
    }

    #[test]
    fn toggling_active_overlay_closes_it() {
        let mut state = AppState::default();
        state.toggle_overlay(OverlayKind::Hosts);
        assert_eq!(state.active_overlay(), Some(OverlayKind::Hosts));
        state.toggle_overlay(OverlayKind::Hosts);
        assert_eq!(state.active_overlay(), None);
    }

    #[test]
    fn hiding_sync_overlay_preserves_sync_workflow_state() {
        let mut state = AppState::default();
        state.remote_workspace.ux = super::super::WorkspaceSyncUxState::Running {
            job_id: "sync-1".into(),
        };
        state.open_overlay(OverlayKind::SyncPreview);
        state.close_overlay(OverlayKind::SyncPreview);
        assert!(!state.remote_workspace.preview_open);
        assert!(matches!(
            state.remote_workspace.ux,
            super::super::WorkspaceSyncUxState::Running { .. }
        ));
    }

    #[test]
    fn opening_cursor_overlay_resets_cursor() {
        let mut state = AppState {
            job_cursor: 42,
            ..AppState::default()
        };
        state.open_overlay(OverlayKind::Jobs);
        assert_eq!(state.job_cursor, 0);
    }
}
