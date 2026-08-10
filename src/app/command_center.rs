use std::cmp::Reverse;

use super::{
    ALL_ACTIONS, Action, ActionAvailability, ActionContext, ActionId, AppState,
    action_availability, action_meta,
};
use crate::vfs::{EntryKind, Location};

/// Typed destination of a Command Center entry.
///
/// This replaces the old string protocol (`tmux:foo`, `sftp://host`, shell
/// command, or path) so presentation code never has to guess what a string
/// means before executing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandTarget {
    Action(Action),
    Location(Location),
    Host { ssh_alias: String, path: String },
    TmuxSession(String),
    ScreenSession(String),
    ShellCommand(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandKind {
    Action,
    Host,
    Bookmark,
    History,
    Session,
    UserCommand,
}

impl CommandKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Action => "ACTION",
            Self::Host => "HOST",
            Self::Bookmark => "BOOKMARK",
            Self::History => "HISTORY",
            Self::Session => "SESSION",
            Self::UserCommand => "COMMAND",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandItem {
    pub title: String,
    pub subtitle: Option<String>,
    pub kind: CommandKind,
    pub target: CommandTarget,
    pub score: i64,
    pub availability: ActionAvailability,
}

impl CommandItem {
    pub fn display_line(&self) -> String {
        match &self.subtitle {
            Some(subtitle) if !subtitle.is_empty() => {
                format!("[{}] {}  —  {}", self.kind.label(), self.title, subtitle)
            }
            _ => format!("[{}] {}", self.kind.label(), self.title),
        }
    }
}

fn text_score(query: &str, title: &str, subtitle: Option<&str>) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }

    let title = title.to_lowercase();
    let subtitle = subtitle.unwrap_or_default().to_lowercase();

    if title == query {
        Some(1_000)
    } else if title.starts_with(query) {
        Some(800)
    } else if title.contains(query) {
        Some(600)
    } else if subtitle.starts_with(query) {
        Some(400)
    } else if subtitle.contains(query) {
        Some(250)
    } else {
        None
    }
}

fn kind_bias(kind: CommandKind) -> i64 {
    match kind {
        CommandKind::Action => 60,
        CommandKind::Host => 50,
        CommandKind::Bookmark => 40,
        CommandKind::Session => 35,
        CommandKind::History => 20,
        CommandKind::UserCommand => 10,
    }
}

/// Empty Command Center is a discovery surface, not an alphabetic dump.
///
/// This is ranking only: labels and execution still come from the shared
/// Action Catalog and typed `Action` targets. Once the user types a query,
/// normal text relevance owns ranking again.
fn empty_query_action_bias(id: ActionId, state: &AppState) -> i64 {
    if state.remote_workspace.plan.is_some() {
        match id {
            ActionId::PreviewWorkspaceSync => 500,
            ActionId::ToggleWorkspaceComparison => 400,
            ActionId::OpenHosts => 300,
            ActionId::OpenHelp => 200,
            ActionId::OpenJobs => 100,
            _ => 0,
        }
    } else {
        match id {
            ActionId::ToggleWorkspaceComparison => 500,
            ActionId::OpenHosts => 400,
            ActionId::OpenHelp => 300,
            ActionId::OpenJobs => 200,
            ActionId::OpenBookmarks => 100,
            _ => 0,
        }
    }
}

fn discovery_bias(
    query: &str,
    id: ActionId,
    state: &AppState,
    availability: &ActionAvailability,
) -> i64 {
    if query.is_empty() && availability.is_available() {
        empty_query_action_bias(id, state)
    } else {
        0
    }
}

/// Build a deterministic, typed Command Center result list.
///
/// The state is the single source for already-loaded hosts/bookmarks/history;
/// opening Command Center must not re-read configuration from disk.
pub fn build_command_items(filter: &str, state: &AppState) -> Vec<CommandItem> {
    build_command_items_with_file_context(filter, state, None, false)
}

pub fn build_command_items_with_file_context(
    filter: &str,
    state: &AppState,
    focused_kind: Option<EntryKind>,
    editor_available: bool,
) -> Vec<CommandItem> {
    let query = filter.trim().to_lowercase();
    let mut items = Vec::new();
    let action_context =
        ActionContext::from_state(state).with_file_context(focused_kind, editor_available);

    for action in ALL_ACTIONS.iter().copied() {
        // Invoking Command Center from inside itself adds no value and creates
        // a surprising close/reopen cycle, so it is intentionally hidden.
        if action.id() == ActionId::OpenCommandCenter {
            continue;
        }

        let Some(meta) = action_meta(action.id()) else {
            continue;
        };
        if let Some(score) = text_score(&query, meta.label, Some(meta.description)) {
            let availability = action_availability(action.id(), &action_context);
            let discovery_bias = discovery_bias(&query, action.id(), state, &availability);
            let subtitle = if discovery_bias > 0 {
                format!("Recommended · {}", meta.description)
            } else {
                meta.description.to_string()
            };
            items.push(CommandItem {
                title: meta.label.to_string(),
                subtitle: Some(subtitle),
                kind: CommandKind::Action,
                target: CommandTarget::Action(action),
                score: score + kind_bias(CommandKind::Action) + discovery_bias,
                availability,
            });
        }
    }

    for host in &state.hosts {
        let subtitle = format!(
            "{}@{}:{}{}",
            host.user,
            host.hostname,
            host.port,
            host.default_path
                .as_deref()
                .map(|path| format!("  {path}"))
                .unwrap_or_default()
        );
        let searchable = format!(
            "{} {} {} {}",
            host.name, host.ssh_alias, host.hostname, subtitle
        );
        if let Some(score) = text_score(&query, &host.name, Some(&searchable)) {
            items.push(CommandItem {
                title: host.name.clone(),
                subtitle: Some(subtitle),
                kind: CommandKind::Host,
                target: CommandTarget::Host {
                    ssh_alias: host.ssh_alias.clone(),
                    path: host.default_path.clone().unwrap_or_else(|| "/".into()),
                },
                score: score + kind_bias(CommandKind::Host),
                availability: ActionAvailability::Available,
            });
        }
    }

    for location in &state.bookmarks {
        let label = location.to_string();
        if let Some(score) = text_score(&query, &label, None) {
            items.push(CommandItem {
                title: label,
                subtitle: None,
                kind: CommandKind::Bookmark,
                target: CommandTarget::Location(location.clone()),
                score: score + kind_bias(CommandKind::Bookmark),
                availability: ActionAvailability::Available,
            });
        }
    }

    for path in &state.dir_history {
        let label = path.to_string_lossy().into_owned();
        if let Some(score) = text_score(&query, &label, None) {
            items.push(CommandItem {
                title: label,
                subtitle: None,
                kind: CommandKind::History,
                target: CommandTarget::Location(Location::Local(path.clone())),
                score: score + kind_bias(CommandKind::History),
                availability: ActionAvailability::Available,
            });
        }
    }

    for entry in &state.menu {
        if let Some(score) = text_score(&query, &entry.label, Some(&entry.command)) {
            items.push(CommandItem {
                title: entry.label.clone(),
                subtitle: Some(entry.command.clone()),
                kind: CommandKind::UserCommand,
                target: CommandTarget::ShellCommand(entry.command.clone()),
                score: score + kind_bias(CommandKind::UserCommand),
                availability: ActionAvailability::Available,
            });
        }
    }

    items.sort_by_key(|item| {
        (
            Reverse(item.score),
            item.title.to_lowercase(),
            item.kind.label(),
        )
    });
    items.truncate(50);
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::MenuEntry;

    #[test]
    fn action_search_returns_typed_action() {
        let state = AppState::default();
        let items = build_command_items("jobs", &state);
        assert!(items.iter().any(|item| {
            item.target == CommandTarget::Action(Action::OpenJobs)
                && item.kind == CommandKind::Action
        }));
    }

    #[test]
    fn bookmark_is_not_reparsed_from_its_display_string() {
        let location = Location::Sftp {
            host: "prod".into(),
            path: "/srv/app".into(),
        };
        let state = AppState {
            bookmarks: vec![location.clone()],
            ..AppState::default()
        };

        let items = build_command_items("prod", &state);
        assert!(items.iter().any(|item| {
            item.target == CommandTarget::Location(location.clone())
                && item.kind == CommandKind::Bookmark
        }));
    }

    #[test]
    fn user_command_that_looks_like_a_protocol_stays_a_shell_command() {
        let state = AppState {
            menu: vec![MenuEntry {
                label: "Fake tmux command".into(),
                command: "tmux:not-a-session".into(),
            }],
            ..AppState::default()
        };

        let items = build_command_items("fake", &state);
        assert!(items.iter().any(|item| {
            item.target == CommandTarget::ShellCommand("tmux:not-a-session".into())
        }));
    }

    #[test]
    fn exact_action_match_ranks_before_weaker_matches() {
        let state = AppState::default();
        let items = build_command_items("help", &state);
        assert_eq!(
            items.first().map(|item| &item.target),
            Some(&CommandTarget::Action(Action::OpenHelp))
        );
    }

    #[test]
    fn empty_query_starts_with_recommended_workspace_action() {
        let items = build_command_items("", &AppState::default());
        let first = items.first().expect("Command Center should not be empty");

        assert_eq!(
            first.target,
            CommandTarget::Action(Action::ToggleWorkspaceComparison)
        );
        assert!(
            first
                .subtitle
                .as_deref()
                .is_some_and(|subtitle| subtitle.starts_with("Recommended · "))
        );
    }

    #[test]
    fn typed_query_does_not_keep_discovery_bias() {
        let items = build_command_items("help", &AppState::default());
        let first = items.first().expect("help action should be present");

        assert_eq!(first.target, CommandTarget::Action(Action::OpenHelp));
        assert!(
            !first
                .subtitle
                .as_deref()
                .is_some_and(|subtitle| subtitle.starts_with("Recommended · "))
        );
    }

    #[test]
    fn file_actions_use_the_shared_target_availability() {
        let state = AppState::default();
        let local_file =
            build_command_items_with_file_context("", &state, Some(EntryKind::File), true);
        for action in [Action::ViewFile, Action::EditFile] {
            assert!(
                local_file
                    .iter()
                    .find(|item| item.target == CommandTarget::Action(action))
                    .unwrap()
                    .availability
                    .is_available()
            );
        }

        let directory =
            build_command_items_with_file_context("", &state, Some(EntryKind::Directory), true);
        assert!(
            !directory
                .iter()
                .find(|item| item.target == CommandTarget::Action(Action::ViewFile))
                .unwrap()
                .availability
                .is_available()
        );

        let no_editor =
            build_command_items_with_file_context("", &state, Some(EntryKind::File), false);
        assert!(
            !no_editor
                .iter()
                .find(|item| item.target == CommandTarget::Action(Action::EditFile))
                .unwrap()
                .availability
                .is_available()
        );
    }

    #[test]
    fn unavailable_action_never_receives_discovery_bias() {
        let state = AppState::default();
        let unavailable = ActionAvailability::Disabled {
            reason: "not available here".into(),
        };

        assert_eq!(
            discovery_bias(
                "",
                ActionId::ToggleWorkspaceComparison,
                &state,
                &unavailable,
            ),
            0
        );
    }
}
