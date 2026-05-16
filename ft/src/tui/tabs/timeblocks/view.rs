//! Rendering for the Timeblocks tab (read-only view, plan 015 session 4).

use chrono::Timelike;
use ft_core::timeblock::report::{minutes_to_hours_minutes, time_per_tag, total_minutes};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::tui::tab::TabCtx;
use ratatui::widgets::Clear;

use super::{FormField, Mode, Pane, TimeblocksTab, ViewMode, SIDEBAR_WIDTH};

pub(super) fn render(tab: &mut TimeblocksTab, frame: &mut Frame, area: Rect, _ctx: &TabCtx) {
    // Split off a single-row quickline strip from the bottom when the
    // tab is in Quickline / EditDesc mode. The form (`A`) renders as a
    // centered overlay instead.
    let bottom_strip = matches!(tab.mode, Mode::Quickline(_) | Mode::EditDesc { .. });
    let body_area = if bottom_strip {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(area);
        render_quickline_strip(tab, frame, split[1]);
        split[0]
    } else {
        area
    };

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(1)])
        .split(body_area);

    render_sidebar(tab, frame, chunks[0]);

    match tab.view {
        ViewMode::Split => {
            let panes = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(chunks[1]);
            render_pane(tab, frame, panes[0], Pane::Today);
            render_pane(tab, frame, panes[1], Pane::Tomorrow);
        }
        ViewMode::Single => {
            // Full-width single pane shows whichever day currently has
            // focus. `h`/`l` flip focus → flips which day is on screen.
            render_pane(tab, frame, chunks[1], tab.focus);
        }
    }

    if let Mode::Form(_) = &tab.mode {
        render_form_modal(tab, frame, area);
    }
}

fn render_quickline_strip(tab: &TimeblocksTab, frame: &mut Frame, area: Rect) {
    // ASCII-only prefixes so `chars().count()` matches the rendered
    // cell count (see the form-modal fix for the same reason).
    let (prefix, text, cursor) = match &tab.mode {
        Mode::Quickline(buf) => (" + ", buf.text.as_str(), buf.cursor),
        Mode::EditDesc { buf, .. } => (" edit desc > ", buf.text.as_str(), buf.cursor),
        _ => return,
    };
    let line = Line::from(vec![
        Span::styled(prefix, Style::default().fg(Color::Cyan)),
        Span::raw(text),
    ]);
    let para = Paragraph::new(line);
    frame.render_widget(para, area);
    let col = area.x + (prefix.chars().count() as u16) + (cursor as u16);
    let col = col.min(area.x + area.width.saturating_sub(1));
    frame.set_cursor_position((col, area.y));
}

fn render_form_modal(tab: &TimeblocksTab, frame: &mut Frame, area: Rect) {
    let Mode::Form(state) = &tab.mode else {
        return;
    };

    // Center a 50x10 modal inside the tab body area.
    let w = 50u16.min(area.width.saturating_sub(2));
    let h = 9u16.min(area.height.saturating_sub(2));
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let modal = Rect::new(x, y, w, h);

    frame.render_widget(Clear, modal);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" New timeblock ")
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(modal);
    frame.render_widget(block, modal);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Start row
            Constraint::Length(1), // End row
            Constraint::Length(1), // Desc row
            Constraint::Length(1), // blank
            Constraint::Length(1), // help
        ])
        .split(inner);

    // ASCII prefix so cursor positioning matches what the terminal
    // actually renders (the previous `▸` glyph was 2 cells wide in some
    // fonts, throwing the cursor offset off by one cell).
    let prefix_for = |label: &str, focused: bool| -> String {
        let marker = if focused { '>' } else { ' ' };
        format!("{marker} {label:<6}")
    };
    let row = |label: &str, buf_text: &str, focused: bool| -> Paragraph<'_> {
        let style = if focused {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        Paragraph::new(Line::from(vec![
            Span::styled(prefix_for(label, focused), style),
            Span::raw(buf_text.to_string()),
        ]))
    };

    let start_row = row("start", &state.start.text, state.focus == FormField::Start);
    let end_row = row("end", &state.end.text, state.focus == FormField::End);
    let desc_row = row("desc", &state.desc.text, state.focus == FormField::Desc);

    frame.render_widget(start_row, rows[0]);
    frame.render_widget(end_row, rows[1]);
    frame.render_widget(desc_row, rows[2]);
    frame.render_widget(
        Paragraph::new(Span::styled(
            "Tab / ↑↓ to cycle  ·  Enter on desc to commit  ·  Esc to cancel",
            Style::default().fg(Color::DarkGray),
        )),
        rows[4],
    );

    // Cursor offset = the focused row's actual prefix width. ASCII-only
    // so `chars().count()` equals the rendered cell count.
    let (buf_cursor, row_idx, label) = match state.focus {
        FormField::Start => (state.start.cursor, 0, "start"),
        FormField::End => (state.end.cursor, 1, "end"),
        FormField::Desc => (state.desc.cursor, 2, "desc"),
    };
    let prefix_width = prefix_for(label, true).chars().count() as u16;
    let col = rows[row_idx].x + prefix_width + buf_cursor as u16;
    let col = col.min(rows[row_idx].x + rows[row_idx].width.saturating_sub(1));
    frame.set_cursor_position((col, rows[row_idx].y));
}

fn render_sidebar(tab: &TimeblocksTab, frame: &mut Frame, area: Rect) {
    let now = (tab.clock)();
    let date = now.format("%a %d %b").to_string();
    let time = now.format("%H:%M:%S").to_string();

    let mut lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!(" {date}"),
            Style::default().fg(Color::White),
        )),
        Line::from(Span::styled(
            format!(" {time}"),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            " ── totals (today) ──",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let total = total_minutes(&tab.today.blocks);
    let (th, tm) = minutes_to_hours_minutes(total);
    lines.push(Line::from(Span::styled(
        format!(" total {th:02}:{tm:02}"),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )));
    for tt in time_per_tag(&tab.today.blocks) {
        let (h, m) = minutes_to_hours_minutes(tt.minutes);
        let style = if tt.tag == "break" {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::from(Span::styled(
            format!(" @{}  {h:02}:{m:02}", tt.tag),
            style,
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " ── focus ──",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(Span::styled(
        match tab.focus {
            Pane::Today => " ▶ today",
            Pane::Tomorrow => " ▶ tomorrow",
        },
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));
    // Show the view mode so users have a visible cue that `f` is doing
    // something — split vs single-day full-width.
    lines.push(Line::from(Span::styled(
        match tab.view {
            ViewMode::Split => " view: split",
            ViewMode::Single => " view: single (f)",
        },
        Style::default().fg(Color::DarkGray),
    )));
    if matches!(tab.mode, Mode::DeleteConfirm { .. }) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " d again = delete",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" sidebar ")
        .border_style(Style::default().fg(Color::DarkGray));
    let para = Paragraph::new(lines).block(block);
    frame.render_widget(para, area);
}

fn render_pane(tab: &TimeblocksTab, frame: &mut Frame, area: Rect, which: Pane) {
    let pane = match which {
        Pane::Today => &tab.today,
        Pane::Tomorrow => &tab.tomorrow,
    };
    let focused = tab.focus == which;
    let title_text = match which {
        Pane::Today => format!(" Today  {} ", pane.date),
        Pane::Tomorrow => format!(" Tomorrow  {} ", pane.date),
    };
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title_text)
        .border_style(border_style);

    if !pane.present {
        // Tomorrow pane (or today) with no daily-note file on disk yet.
        // Session 5 binds `c` to create the file via the daily-template;
        // for now we just surface the placeholder.
        let body = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  no daily note yet.",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "  press `c` to create (session 5)",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        let para = Paragraph::new(body).block(block);
        frame.render_widget(para, area);
        return;
    }

    if pane.blocks.is_empty() {
        let body = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  no timeblocks for this day yet.",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        let para = Paragraph::new(body).block(block);
        frame.render_widget(para, area);
        return;
    }

    let items: Vec<ListItem> = pane
        .blocks
        .iter()
        .map(|b| {
            let line_text = format!("{:>3}  {}  {}", b.source_line, period_str(b), b.desc.trim());
            ListItem::new(line_text)
        })
        .collect();

    let mut state = ListState::default();
    state.select(Some(pane.selection));

    let highlight_style = if focused {
        Style::default()
            .bg(Color::Cyan)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(Color::DarkGray).fg(Color::White)
    };
    let list = List::new(items)
        .block(block)
        .highlight_style(highlight_style)
        .highlight_symbol(if focused { "▶ " } else { "  " });

    frame.render_stateful_widget(list, area, &mut state);
}

fn period_str(b: &ft_core::timeblock::Timeblock) -> String {
    format!(
        "{:02}:{:02} - {:02}:{:02}",
        b.start.hour(),
        b.start.minute(),
        b.end.hour(),
        b.end.minute()
    )
}
