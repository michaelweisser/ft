use anyhow::Result;
use chrono::{DateTime, Local};
use ft_core::vault::Vault;
use ratatui::{layout::Rect, Frame};

use crate::tui::event::Event;

/// What the App should do after a tab handles an event. `Consumed` and `Quit`
/// are part of the contract but unused in session 1; sessions 2+ surface them
/// (e.g. a tab swallowing `q` while editing a query, or a tab signalling exit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum EventOutcome {
    Consumed,
    NotHandled,
    SwitchTab(usize),
    Quit,
}

/// Shared context passed to tabs on every event/render. v1 carries only the
/// vault and a refresh timestamp; sessions 2+ will extend this.
#[allow(dead_code)]
pub struct TabCtx<'a> {
    pub vault: &'a Vault,
    pub last_refresh: Option<DateTime<Local>>,
}

/// A top-level tab in the TUI. New tabs slot in by adding a `Box<dyn Tab>` to
/// the App's tab list — no surgery on the core loop. `refresh` is part of the
/// contract; sessions 2+ wire it to the `R` keybinding in the Search view.
#[allow(dead_code)]
pub trait Tab {
    fn title(&self) -> &str;

    fn on_focus(&mut self, _ctx: &mut TabCtx) -> Result<()> {
        Ok(())
    }

    fn on_blur(&mut self, _ctx: &mut TabCtx) -> Result<()> {
        Ok(())
    }

    fn handle_event(&mut self, ev: Event, ctx: &mut TabCtx) -> Result<EventOutcome>;

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &TabCtx);

    fn refresh(&mut self, _ctx: &mut TabCtx) -> Result<()> {
        Ok(())
    }
}
