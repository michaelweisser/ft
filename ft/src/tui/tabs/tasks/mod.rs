mod search;
mod view;

use anyhow::Result;
use chrono::{DateTime, Local};
use crossterm::event::KeyCode;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::tui::{
    event::Event,
    tab::{EventOutcome, Tab, TabCtx},
};

use search::SearchView;
use view::View;

/// Function pointer for "what time is it now?". Production uses
/// [`Local::now`]; tests inject a fixed value for deterministic snapshots.
pub type ClockFn = fn() -> DateTime<Local>;

fn local_now() -> DateTime<Local> {
    Local::now()
}

const SIDEBAR_WIDTH: u16 = 24;

pub struct TasksTab {
    views: Vec<Box<dyn View>>,
    active_view: usize,
    clock: ClockFn,
}

impl TasksTab {
    pub fn new() -> Self {
        Self::with_clock(local_now)
    }

    pub fn with_clock(clock: ClockFn) -> Self {
        let views: Vec<Box<dyn View>> = vec![Box::new(SearchView::new())];
        Self {
            views,
            active_view: 0,
            clock,
        }
    }

    fn select_prev_view(&mut self) {
        if self.views.is_empty() {
            return;
        }
        if self.active_view == 0 {
            self.active_view = self.views.len() - 1;
        } else {
            self.active_view -= 1;
        }
    }

    fn select_next_view(&mut self) {
        if self.views.is_empty() {
            return;
        }
        self.active_view = (self.active_view + 1) % self.views.len();
    }

    fn render_sidebar(&self, frame: &mut Frame, area: Rect) {
        let now = (self.clock)();
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
                " ── views ──",
                Style::default().fg(Color::DarkGray),
            )),
        ];

        for (i, v) in self.views.iter().enumerate() {
            let (marker, style) = if i == self.active_view {
                (
                    " ▶ ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                ("   ", Style::default().fg(Color::White))
            };
            lines.push(Line::from(vec![
                Span::raw(marker),
                Span::styled(v.title().to_string(), style),
            ]));
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" sidebar ")
            .border_style(Style::default().fg(Color::DarkGray));
        let para = Paragraph::new(lines).block(block);
        frame.render_widget(para, area);
    }

    fn render_viewport(&mut self, frame: &mut Frame, area: Rect, ctx: &TabCtx) {
        if let Some(v) = self.views.get_mut(self.active_view) {
            v.render(frame, area, ctx);
        }
    }
}

impl Tab for TasksTab {
    fn title(&self) -> &str {
        "Tasks"
    }

    fn on_focus(&mut self, ctx: &mut TabCtx) -> Result<()> {
        if let Some(v) = self.views.get_mut(self.active_view) {
            v.on_focus(ctx)?;
        }
        Ok(())
    }

    fn handle_event(&mut self, ev: Event, ctx: &mut TabCtx) -> Result<EventOutcome> {
        // The active view gets first dibs — its selection model owns the same
        // keys (↑/↓/Enter) as the sidebar dropdown. The dropdown only handles
        // these keys when the view returns NotHandled (e.g. while the search
        // list is empty or the view has no opinion).
        let view_outcome = if let Some(v) = self.views.get_mut(self.active_view) {
            v.handle_event(ev.clone(), ctx)?
        } else {
            EventOutcome::NotHandled
        };
        if view_outcome != EventOutcome::NotHandled {
            return Ok(view_outcome);
        }

        if let Event::Key(k) = ev {
            match k.code {
                KeyCode::Up => {
                    self.select_prev_view();
                    return Ok(EventOutcome::Consumed);
                }
                KeyCode::Down => {
                    self.select_next_view();
                    return Ok(EventOutcome::Consumed);
                }
                KeyCode::Enter => {
                    return Ok(EventOutcome::Consumed);
                }
                _ => {}
            }
        }
        Ok(EventOutcome::NotHandled)
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &TabCtx) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(1)])
            .split(area);

        self.render_sidebar(frame, chunks[0]);
        self.render_viewport(frame, chunks[1], ctx);
    }

    fn refresh(&mut self, ctx: &mut TabCtx) -> Result<()> {
        if let Some(v) = self.views.get_mut(self.active_view) {
            v.refresh(ctx)?;
        }
        Ok(())
    }
}
