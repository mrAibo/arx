use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use arx::app::{AppState, CommandKind};

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

pub(super) fn render_help(frame: &mut Frame, area: Rect, state: &mut AppState) {
    let popup_area = centered_rect(68, 90, area);
    frame.render_widget(Clear, popup_area);

    // Full help content (Getting Started first, then full reference)
    let full_lines = help_full_lines();

    // Compute visible slice
    let content_height = popup_area.height.saturating_sub(2) as usize; // minus borders
    let total = full_lines.len();
    let scroll = state.help_scroll.min(total.saturating_sub(content_height));
    state.help_scroll = scroll;
    let visible: Vec<Line> = full_lines
        .iter()
        .skip(scroll)
        .take(content_height)
        .cloned()
        .collect();

    // Scroll hint
    let scroll_hint = if total > content_height {
        format!(
            " {}/{} | j/k/↑↓:scroll PgUp/PgDn:page Home/End:jump q/Esc/F1:close ",
            scroll + visible.len().min(total - scroll),
            total
        )
    } else {
        " q/Esc/F1/?:close ".into()
    };

    let help = Paragraph::new(visible)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Help ")
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .style(Style::default().fg(Color::White))
        .wrap(Wrap { trim: false });
    frame.render_widget(help, popup_area);

    // Scroll hint at bottom
    let hint_area = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(popup_area);
    let hint = Paragraph::new(Line::from(scroll_hint))
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    frame.render_widget(hint, hint_area[1]);
}

fn help_full_lines() -> Vec<Line<'static>> {
    use ratatui::style::{Color, Style};
    use ratatui::text::{Line, Span};

    vec![
        // Getting Started (short, first)
        Line::from(Span::styled(
            "ARX Help — Getting Started",
            Style::default().fg(Color::Yellow),
        )),
        Line::from(Span::styled(
            "F1/? or Esc to close  |  j/k/↑↓/PgUp/PgDn/Home/End to scroll",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Core Workflow",
            Style::default().fg(Color::Cyan),
        )),
        Line::from("  Tab           Switch pane (left ↔ right)"),
        Line::from("  F9            Hosts / SFTP (connect remote)"),
        Line::from("  Ctrl+D        Compare workspace (diff)"),
        Line::from("  Ctrl+X P      Sync Preview (after compare)"),
        Line::from("  Enter         Execute preview"),
        Line::from("  Ctrl+P        Command Center"),
        Line::from("  F1/?          This help"),
        Line::from("  F10 / q       Quit"),
        Line::from(""),
        Line::from(Span::styled("Navigation", Style::default().fg(Color::Cyan))),
        Line::from("  j / ↓ / k / ↑      Move cursor"),
        Line::from("  Enter              Enter directory / content diff"),
        Line::from("  Backspace          Parent directory"),
        Line::from("  Ctrl+G             Go to path"),
        Line::from("  Ctrl+U             Swap panes"),
        Line::from("  Alt+U              Storage Inspector (local, read-only)"),
        Line::from("  Alt+D              Filesystems (df++, read-only)"),
        Line::from("  Alt+O              Sync other pane to active"),
        Line::from("  Alt+Down           Go back in directory history"),
        Line::from("  Alt+/              Recursive file search (find)"),
        Line::from("  Ctrl+\\             Open in file manager"),
        Line::from(""),
        Line::from(Span::styled("Tabs", Style::default().fg(Color::Cyan))),
        Line::from("  Ctrl+T         New tab"),
        Line::from("  Ctrl+W         Close tab"),
        Line::from("  Ctrl+←/→       Previous / next tab"),
        Line::from("  Alt+1 … 9      Switch to tab N"),
        Line::from(""),
        Line::from(Span::styled(
            "Selection & Filter",
            Style::default().fg(Color::Cyan),
        )),
        Line::from("  Space          Toggle selection"),
        Line::from("  *              Invert selection"),
        Line::from("  +              Select by glob pattern"),
        Line::from("  /              Quick filter by name"),
        Line::from("  Ctrl+H         Toggle hidden (dot) files"),
        Line::from("  Alt+T          Toggle panel mode (Full/Brief)"),
        Line::from(""),
        Line::from(Span::styled(
            "File Operations",
            Style::default().fg(Color::Cyan),
        )),
        Line::from("  F5             Copy to other pane"),
        Line::from("  F6             Move to other pane"),
        Line::from("  F7             Create directory"),
        Line::from("  F8             Delete"),
        Line::from("  Shift+F6       Rename file"),
        Line::from("  Ctrl+I         File info (stat)"),
        Line::from("  Ctrl+Space     Directory size / free space"),
        Line::from(""),
        Line::from(Span::styled(
            "View & Edit",
            Style::default().fg(Color::Cyan),
        )),
        Line::from("  F3             View file (built-in)"),
        Line::from("  Shift+F3       View with bat (syntax highlight)"),
        Line::from("  F4             Edit (configurable editor)"),
        Line::from(""),
        Line::from(Span::styled(
            "Ctrl+X Prefix (MC-style)",
            Style::default().fg(Color::Cyan),
        )),
        Line::from("  Ctrl+X S       Symlink (ln -s)"),
        Line::from("  Ctrl+X L       Hard link (ln)"),
        Line::from("  Ctrl+X C       chmod"),
        Line::from("  Ctrl+X O       chown"),
        Line::from(""),
        Line::from(Span::styled(
            "Panels & Tools",
            Style::default().fg(Color::Cyan),
        )),
        Line::from("  F2             User menu (arx.menu)"),
        Line::from("  Ctrl+B         Bookmarks"),
        Line::from("  Ctrl+J         Background jobs"),
        Line::from("  Ctrl+O         Shell (drop to subshell)"),
        Line::from("  :              Command line"),
        Line::from(""),
        Line::from(Span::styled("Other", Style::default().fg(Color::Cyan))),
        Line::from("  Ctrl+R         Refresh"),
        Line::from("  q              Quit"),
        Line::from("  F1 / ?         This help"),
    ]
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

pub(super) fn render_bookmarks(frame: &mut Frame, area: Rect, state: &AppState) {
    let popup_area = centered_rect(60, 70, area);
    frame.render_widget(Clear, popup_area);

    let items: Vec<ListItem> = state
        .bookmarks
        .iter()
        .enumerate()
        .map(|(i, loc)| {
            let prefix = if i == state.bookmark_cursor {
                "> "
            } else {
                "  "
            };
            ListItem::new(Line::from(format!("{prefix}{loc}")))
        })
        .collect();

    let mut list_state = ListState::default();
    list_state.select(Some(state.bookmark_cursor));

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Bookmarks (Ctrl+B: close, Enter: go) "),
        )
        .highlight_style(Style::default().fg(Color::Black).bg(Color::White));
    frame.render_stateful_widget(list, popup_area, &mut list_state);
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
