use chrono::Local;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Tabs},
    Frame,
};

use crate::tui::tab::{Tab, TabCtx};

/// Whether the help overlay is open and which mode tag to render in the
/// status bar's right cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Help,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "normal",
            Mode::Help => "help",
        }
    }
}

/// Compute the screen layout: top tab bar (1 line) + body + status bar (1 line).
pub fn split_screen(area: Rect) -> [Rect; 3] {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);
    [chunks[0], chunks[1], chunks[2]]
}

pub fn render_tab_bar(frame: &mut Frame, area: Rect, titles: &[&str], selected: usize) {
    let spans: Vec<Line> = titles
        .iter()
        .enumerate()
        .map(|(i, t)| Line::from(format!(" {} {} ", i + 1, t)))
        .collect();
    let widget = Tabs::new(spans)
        .select(selected)
        .style(Style::default().fg(Color::Gray))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .divider("│");
    frame.render_widget(widget, area);
}

pub fn render_status_bar(
    frame: &mut Frame,
    area: Rect,
    vault_name: &str,
    tab_title: &str,
    last_refresh: Option<chrono::DateTime<Local>>,
    mode: Mode,
) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Percentage(30),
            Constraint::Percentage(20),
        ])
        .split(area);

    let left = Line::from(vec![
        Span::styled(" vault: ", Style::default().fg(Color::DarkGray)),
        Span::styled(vault_name, Style::default().fg(Color::White)),
        Span::raw("  ·  "),
        Span::styled("tab: ", Style::default().fg(Color::DarkGray)),
        Span::styled(tab_title, Style::default().fg(Color::White)),
    ]);

    let refresh_text = match last_refresh {
        Some(ts) => format!("refreshed {}", ts.format("%H:%M:%S")),
        None => "not yet refreshed".to_string(),
    };
    let center = Line::from(Span::styled(
        refresh_text,
        Style::default().fg(Color::DarkGray),
    ))
    .alignment(Alignment::Center);

    let right = Line::from(vec![
        Span::styled("mode: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            mode.label(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ])
    .alignment(Alignment::Right);

    let bg = Style::default().bg(Color::Rgb(28, 28, 32));
    frame.render_widget(Paragraph::new(left).style(bg), chunks[0]);
    frame.render_widget(Paragraph::new(center).style(bg), chunks[1]);
    frame.render_widget(Paragraph::new(right).style(bg), chunks[2]);
}

pub fn render_body(frame: &mut Frame, area: Rect, tab: &mut dyn Tab, ctx: &TabCtx) {
    tab.render(frame, area, ctx);
}

const HELP_LINES: &[(&str, &str)] = &[
    ("q / Ctrl+C", "quit"),
    ("?", "toggle this help"),
    ("Tab / Shift+Tab", "next / previous tab"),
    ("1 / 2", "jump to tab N"),
    ("Esc", "close overlay"),
];

pub fn render_help_overlay(frame: &mut Frame, area: Rect) {
    let popup = centered_rect(60, 60, area);
    frame.render_widget(Clear, popup);

    let mut lines: Vec<Line> = Vec::with_capacity(HELP_LINES.len() + 2);
    lines.push(Line::from(Span::styled(
        "Keybindings",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
    for (key, desc) in HELP_LINES {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {key:<18}"),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(*desc, Style::default().fg(Color::White)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  press ? or Esc to close",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" help ")
        .style(Style::default().bg(Color::Black));
    let para = Paragraph::new(lines).block(block);
    frame.render_widget(para, popup);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
