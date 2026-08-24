use super::{
    VisiblePaneRow, apply_filter_with_parent_and_continuation, load_more_entry, normalize_entries,
    virtual_parent_entry,
};
use arx::app::{AppState, Pane, PaneLoadUiError};
use arx::services::{PaneLoadPurpose, PaneLoadResponse, PaneNextPageResponse};
use arx::vfs::{EntryIdentity, ListedEntry};

#[derive(Clone, PartialEq, Eq)]
enum VisibleRowSelection {
    Parent,
    Listed(EntryIdentity),
    LoadMore,
}

fn selected_visible_row(
    state: &AppState,
    pane: Pane,
    entries: &[ListedEntry],
    cursor: usize,
) -> Option<VisibleRowSelection> {
    let pane_state = match pane {
        Pane::Left => &state.left,
        Pane::Right => &state.right,
    };
    let parent = virtual_parent_entry();
    let load_more = load_more_entry();
    apply_filter_with_parent_and_continuation(
        entries,
        &state.filter,
        &pane_state.location,
        &state.registry,
        &parent,
        &load_more,
        state.pane_listing_continuations.get(&pane),
    )
    .get(cursor)
    .map(|row| match row {
        VisiblePaneRow::Parent(_) => VisibleRowSelection::Parent,
        VisiblePaneRow::Listed(listed) => VisibleRowSelection::Listed(listed.identity.clone()),
        VisiblePaneRow::LoadMore(_) => VisibleRowSelection::LoadMore,
    })
}

fn visible_index_for_selection(
    state: &AppState,
    pane: Pane,
    entries: &[ListedEntry],
    selection: &VisibleRowSelection,
) -> Option<usize> {
    let pane_state = match pane {
        Pane::Left => &state.left,
        Pane::Right => &state.right,
    };
    let parent = virtual_parent_entry();
    let load_more = load_more_entry();
    apply_filter_with_parent_and_continuation(
        entries,
        &state.filter,
        &pane_state.location,
        &state.registry,
        &parent,
        &load_more,
        state.pane_listing_continuations.get(&pane),
    )
    .iter()
    .position(|row| match (selection, row) {
        (VisibleRowSelection::Parent, VisiblePaneRow::Parent(_))
        | (VisibleRowSelection::LoadMore, VisiblePaneRow::LoadMore(_)) => true,
        (VisibleRowSelection::Listed(identity), VisiblePaneRow::Listed(listed)) => {
            identity == &listed.identity
        }
        _ => false,
    })
}

pub(super) fn apply_next_page_response(
    response: PaneNextPageResponse,
    state: &mut AppState,
    left_entries: &mut Vec<ListedEntry>,
    right_entries: &mut Vec<ListedEntry>,
) {
    if !state.accepts_next_page(
        response.pane,
        response.request_id,
        &response.initiating_continuation,
    ) {
        return;
    }
    state.finish_next_page(response.pane, response.request_id);
    match response.result {
        Ok(page) => {
            let entries = match response.pane {
                Pane::Left => left_entries,
                Pane::Right => right_entries,
            };
            let pane_state = match response.pane {
                Pane::Left => &state.left,
                Pane::Right => &state.right,
            };
            let primary_selection =
                selected_visible_row(state, response.pane, entries, pane_state.cursor);
            let split_selection = pane_state
                .split
                .then(|| {
                    selected_visible_row(state, response.pane, entries, pane_state.split_cursor)
                })
                .flatten();
            entries.extend(page.entries);
            *entries =
                normalize_entries(std::mem::take(entries), state.show_hidden, state.sort_mode);
            state.apply_pane_listing_continuation(response.pane, page.continuation);
            let primary_index = primary_selection.as_ref().and_then(|selection| {
                visible_index_for_selection(state, response.pane, entries, selection)
            });
            let split_index = split_selection.as_ref().and_then(|selection| {
                visible_index_for_selection(state, response.pane, entries, selection)
            });
            let pane_state = match response.pane {
                Pane::Left => &mut state.left,
                Pane::Right => &mut state.right,
            };
            if let Some(index) = primary_index {
                pane_state.cursor = index;
            }
            if let Some(index) = split_index {
                pane_state.split_cursor = index;
            }
        }
        Err(error) => {
            state.message = Some(format!("Load next page failed: {error}"));
        }
    }
}

pub(super) fn apply_pane_load_response(
    response: PaneLoadResponse,
    state: &mut AppState,
    left_entries: &mut Vec<ListedEntry>,
    right_entries: &mut Vec<ListedEntry>,
) {
    if !state.accepts_pane_load(response.pane, response.id, &response.location) {
        return;
    }
    state.finish_pane_load(response.pane, response.id);

    match response.result {
        Ok(page) => {
            state.pane_load_errors.remove(&response.pane);
            let entries = normalize_entries(page.entries, state.show_hidden, state.sort_mode);
            let active = state.active == response.pane;
            match response.pane {
                Pane::Left => {
                    if response.purpose != PaneLoadPurpose::Refresh {
                        let old = state.left.location.clone();
                        match response.purpose {
                            PaneLoadPurpose::Navigate {
                                remember_current: true,
                            } => state.left.dir_history.push(old),
                            PaneLoadPurpose::HistoryBack => {
                                let _ = state.left.dir_history.pop();
                            }
                            _ => {}
                        }
                        state.left.location = response.location.clone();
                        state.left.cursor = 0;
                        // #16: both subviews show the new shared listing.
                        state.left.split_cursor = 0;
                    }
                    *left_entries = entries;
                    state.left.cursor = state.left.cursor.min(left_entries.len().saturating_sub(1));
                }
                Pane::Right => {
                    if response.purpose != PaneLoadPurpose::Refresh {
                        let old = state.right.location.clone();
                        match response.purpose {
                            PaneLoadPurpose::Navigate {
                                remember_current: true,
                            } => state.right.dir_history.push(old),
                            PaneLoadPurpose::HistoryBack => {
                                let _ = state.right.dir_history.pop();
                            }
                            _ => {}
                        }
                        state.right.location = response.location.clone();
                        state.right.cursor = 0;
                        // #16: both subviews show the new shared listing.
                        state.right.split_cursor = 0;
                    }
                    *right_entries = entries;
                    state.right.cursor = state
                        .right
                        .cursor
                        .min(right_entries.len().saturating_sub(1));
                }
            }
            if response.purpose != PaneLoadPurpose::Refresh {
                state.clear_selection_for_pane(response.pane);
            }
            if active && response.purpose != PaneLoadPurpose::Refresh {
                state.remote_workspace.disable();
                state.show_diff = false;
            }
            state.apply_pane_listing_continuation(response.pane, page.continuation);
        }
        Err(error) => {
            // Transactional navigation: current pane location is intentionally
            // untouched on error. Persist the accepted failure so the pane can
            // explain what failed after the one-shot status message is gone.
            let message = error.to_string();
            state.pane_load_errors.insert(
                response.pane,
                PaneLoadUiError {
                    attempted: response.location.clone(),
                    message: message.clone(),
                },
            );
            state.message = Some(format!("{}: {message}", response.location));
        }
    }
}
