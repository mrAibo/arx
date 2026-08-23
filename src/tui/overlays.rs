use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};

use arx::app::{AppState, CommandKind};
use arx::vfs::Location;

use super::{centered_rect, centered_rect_lines};

pub(super) fn render_session_callout(frame: &mut Frame, area: Rect, text: &str) {
    frame.render_widget(
        Paragraph::new(Span::styled(
            text.to_string(),
            Style::default().fg(Color::Green),
        )),
        area,
    );
}

pub(super) fn render_viewer(frame: &mut Frame, area: Rect, state: &AppState) {
    let popup_area = centered_rect(80, 90, area);
    frame.render_widget(Clear, popup_area);

    let total = state.viewer_content.len();
    let pct = if total > 0 {
        ((state.viewer_scroll.min(total.saturating_sub(1)) as f64 / total as f64) * 100.0) as usize
    } else {
        0
    };
    let title = format!(" View ({} lines, {}%) ", total, pct);
    let max_scroll = state.viewer_content.len().saturating_sub(1);
    let visible: Vec<Line> = state
        .viewer_content
        .iter()
        .skip(state.viewer_scroll)
        .take(popup_area.height.saturating_sub(2) as usize)
        .map(|l| Line::from(l.as_str()))
        .collect();

    let scroll_hint = if max_scroll > 0 {
        format!(
            " {}/{} | j/k:scroll q/Esc:close ",
            state.viewer_scroll, max_scroll
        )
    } else {
        " q/Esc:close ".into()
    };

    let viewer = Paragraph::new(visible)
        .block(Block::default().borders(Borders::ALL).title(title.as_str()))
        .style(Style::default().fg(Color::White));
    frame.render_widget(viewer, popup_area);

    let hint_area = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(popup_area);
    let hint = Paragraph::new(Line::from(scroll_hint)).style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, hint_area[1]);
}

pub(super) fn render_infrastructure_center(frame: &mut Frame, area: Rect, state: &mut AppState) {
    let lines = &state.infrastructure_lines;
    let h = (lines.len().max(1) + 3).min(30) as u16;
    let popup = centered_rect_lines(80, h, area);
    frame.render_widget(Clear, popup);
    let items: Vec<ListItem> = lines.iter().map(|l| ListItem::new(l.as_str())).collect();
    let list = ratatui::widgets::List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Infrastructure Center — Ctrl+I toggle "),
        )
        .highlight_style(Style::default().fg(Color::Cyan));
    frame.render_stateful_widget(list, popup, &mut state.overlay_list_state);
}

pub(super) fn render_smart_tree(frame: &mut Frame, area: Rect, state: &mut AppState) {
    let tl = &state.tree_lines;
    let h = (tl.len().max(1) + 3).min(30) as u16;
    let popup = centered_rect_lines(80, h, area);
    frame.render_widget(Clear, popup);
    let items: Vec<ListItem> = tl.iter().map(|l| ListItem::new(l.as_str())).collect();
    let title = format!(
        " ARX Smart Tree — :{}_ | Ctrl+T toggle, Esc close ",
        state.tree_filter
    );
    let list = ratatui::widgets::List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().fg(Color::Green));
    frame.render_stateful_widget(list, popup, &mut state.overlay_list_state);
}

pub(super) fn render_command_center(frame: &mut Frame, area: Rect, state: &mut AppState) {
    let h = (state.command_matches.len().max(1) + 3).min(20) as u16;
    let popup = centered_rect_lines(70, h, area);
    frame.render_widget(Clear, popup);
    let items: Vec<ListItem> = state
        .command_matches
        .iter()
        .map(|item| {
            let style = if !item.availability.is_available() {
                Style::default().fg(Color::DarkGray)
            } else {
                match item.kind {
                    CommandKind::Action => Style::default().fg(Color::Cyan),
                    CommandKind::Host => Style::default().fg(Color::Green),
                    CommandKind::Bookmark => Style::default().fg(Color::Magenta),
                    CommandKind::History => Style::default(),
                    CommandKind::Session => Style::default().fg(Color::Yellow),
                    CommandKind::UserCommand => Style::default().fg(Color::Blue),
                }
            };
            let line = match item.availability.reason() {
                Some(reason) => format!("{}  —  unavailable: {reason}", item.display_line()),
                None => item.display_line(),
            };
            ListItem::new(line).style(style)
        })
        .collect();

    let list = ratatui::widgets::List::new(items)
        .block(Block::default().borders(Borders::ALL).title(format!(
            " ARX Command Center — :{}_ | bat chafa pdftotext ffprobe 7z ",
            state.filter
        )))
        .highlight_style(Style::default().fg(Color::Yellow));
    frame.render_stateful_widget(list, popup, &mut state.overlay_list_state);
}

pub(super) fn render_context_menu(frame: &mut Frame, area: Rect) {
    let popup = centered_rect_lines(18, 8, area);
    frame.render_widget(Clear, popup);
    let items: Vec<ListItem> = [
        "Copy   F5",
        "Move   F6",
        "Mkdir  F7",
        "Delete F8",
        "View   F3",
        "Edit   F4",
    ]
    .iter()
    .map(|s| ListItem::new(*s))
    .collect();
    let list = ratatui::widgets::List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Menu "))
        .highlight_style(Style::default().fg(Color::Yellow));
    frame.render_widget(list, popup);
}

pub(super) fn render_directory_history(frame: &mut Frame, area: Rect, state: &AppState) {
    let h = (state.dir_history.len() + 2).min(20) as u16;
    let popup = centered_rect_lines(60, h, area);
    frame.render_widget(Clear, popup);
    let mut items: Vec<ListItem> = state
        .dir_history
        .iter()
        .rev()
        .enumerate()
        .map(|(i, p)| {
            ListItem::new(format!(
                "{:2}  {}",
                state.dir_history.len() - i,
                p.display()
            ))
        })
        .collect();
    if items.is_empty() {
        items.push(ListItem::new("(empty)"));
    }
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Directory History (Alt+H) ")
                .border_style(Style::default().fg(Color::Cyan)),
        ),
        popup,
    );
}

pub(super) fn render_tab_switcher(frame: &mut Frame, area: Rect, state: &AppState) {
    let mut items: Vec<ListItem> = vec![ListItem::new("── Left pane ──")];
    for (i, tab) in state.left.tabs.iter().enumerate() {
        let idx = items.len();
        let pre = if idx == state.tab_switcher_cursor {
            "> "
        } else {
            "  "
        };
        let loc = match &tab.0 {
            Location::Local(p) => p.display().to_string(),
            o => o.to_string(),
        };
        items.push(ListItem::new(format!("{pre}L{i}: {loc}")));
    }
    items.push(ListItem::new("── Right pane ──"));
    for (i, tab) in state.right.tabs.iter().enumerate() {
        let idx = items.len();
        let pre = if idx == state.tab_switcher_cursor {
            "> "
        } else {
            "  "
        };
        let loc = match &tab.0 {
            Location::Local(p) => p.display().to_string(),
            o => o.to_string(),
        };
        items.push(ListItem::new(format!("{pre}R{i}: {loc}")));
    }
    let h = (items.len() + 2) as u16;
    let popup = centered_rect_lines(60, h, area);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Tabs (Alt+`) ")
                .border_style(Style::default().fg(Color::Cyan)),
        ),
        popup,
    );
}

pub(super) fn render_rename_input(frame: &mut Frame, area: Rect, state: &AppState) {
    frame.render_widget(
        Paragraph::new(format!(" Rename: {}_", state.rename_pattern))
            .style(Style::default().fg(Color::Yellow).bg(Color::DarkGray)),
        Rect {
            x: area.x,
            y: area.y + area.height.saturating_sub(2),
            width: area.width,
            height: 1,
        },
    );
}

pub(super) fn render_file_search(frame: &mut Frame, area: Rect, state: &AppState) {
    frame.render_widget(
        Paragraph::new(format!(
            " /{}_  ({})",
            state.search_query,
            state.search_matches.len()
        ))
        .style(Style::default().fg(Color::Yellow).bg(Color::DarkGray)),
        Rect {
            x: area.x,
            y: area.y + area.height.saturating_sub(2),
            width: area.width,
            height: 1,
        },
    );
}
