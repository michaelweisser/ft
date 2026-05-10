use anyhow::Result;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::{
    event::Event,
    tab::{EventOutcome, Tab, TabCtx},
};

/// Index of the Tasks tab — used by Welcome to redirect on any keypress.
const TASKS_TAB_INDEX: usize = 1;

const BANNER: &[&str] = &[
    "██╗    ██╗███████╗██╗      ██████╗ ██████╗ ███╗   ███╗███████╗",
    "██║    ██║██╔════╝██║     ██╔════╝██╔═══██╗████╗ ████║██╔════╝",
    "██║ █╗ ██║█████╗  ██║     ██║     ██║   ██║██╔████╔██║█████╗  ",
    "██║███╗██║██╔══╝  ██║     ██║     ██║   ██║██║╚██╔╝██║██╔══╝  ",
    "╚███╔███╔╝███████╗███████╗╚██████╗╚██████╔╝██║ ╚═╝ ██║███████╗",
    " ╚══╝╚══╝ ╚══════╝╚══════╝ ╚═════╝ ╚═════╝ ╚═╝     ╚═╝╚══════╝",
];

pub struct WelcomeTab;

impl WelcomeTab {
    pub fn new() -> Self {
        Self
    }
}

impl Tab for WelcomeTab {
    fn title(&self) -> &str {
        "Welcome"
    }

    fn handle_event(&mut self, ev: Event, _ctx: &mut TabCtx) -> Result<EventOutcome> {
        match ev {
            Event::Key(_) => Ok(EventOutcome::SwitchTab(TASKS_TAB_INDEX)),
            _ => Ok(EventOutcome::NotHandled),
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &TabCtx) {
        let banner_lines: Vec<Line> = BANNER
            .iter()
            .map(|l| {
                Line::from(Span::styled(
                    *l,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ))
            })
            .collect();

        let banner_height = banner_lines.len() as u16;
        let subtitle = Line::from(vec![Span::styled(
            "to ft — your terminal companion for Obsidian",
            Style::default().fg(Color::Gray),
        )]);
        let vault_hint = Line::from(vec![
            Span::styled("vault: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                ctx.vault
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| ctx.vault.path.display().to_string()),
                Style::default().fg(Color::White),
            ),
        ]);
        let hint = Line::from(Span::styled(
            "press any key to continue · q to quit · ? for help",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ));

        // Center the banner block vertically, with the hints below it.
        let block_height = banner_height + 4; // banner + spacer + subtitle + spacer + hint(2)
        let vertical_pad = area.height.saturating_sub(block_height) / 2;

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(vertical_pad),
                Constraint::Length(banner_height),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(area);

        frame.render_widget(
            Paragraph::new(banner_lines).alignment(Alignment::Center),
            chunks[1],
        );
        frame.render_widget(
            Paragraph::new(subtitle).alignment(Alignment::Center),
            chunks[3],
        );
        frame.render_widget(
            Paragraph::new(vault_hint).alignment(Alignment::Center),
            chunks[4],
        );
        frame.render_widget(Paragraph::new(hint).alignment(Alignment::Center), chunks[5]);
    }
}
