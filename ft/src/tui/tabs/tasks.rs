use anyhow::Result;
use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::tui::{
    event::Event,
    tab::{EventOutcome, Tab, TabCtx},
};

/// Placeholder Tasks tab. Session 2 builds out the sidebar + viewport split,
/// the Search view, and the live clock; for now we render a stub so the tab
/// framework can switch to it.
pub struct TasksTab;

impl TasksTab {
    pub fn new() -> Self {
        Self
    }
}

impl Tab for TasksTab {
    fn title(&self) -> &str {
        "Tasks"
    }

    fn handle_event(&mut self, _ev: Event, _ctx: &mut TabCtx) -> Result<EventOutcome> {
        Ok(EventOutcome::NotHandled)
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, _ctx: &TabCtx) {
        let body = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled("Tasks", Style::default().fg(Color::Cyan))),
            Line::from(""),
            Line::from(Span::styled(
                "(coming next session — sidebar, Search view, quick keys)",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
        frame.render_widget(body, area);
    }
}
