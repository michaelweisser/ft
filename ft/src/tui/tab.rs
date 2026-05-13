use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, Local, NaiveDate};
use ft_core::recents::RecentsLog;
use ft_core::vault::Vault;
use ratatui::{layout::Rect, Frame};

use crate::tui::event::Event;

/// Side-effect a tab/view can request from the App. Lets the App orchestrate
/// surface-level concerns (suspending the alt-screen for `$EDITOR`, pushing
/// a status-bar toast) without each tab reaching for terminal state.
#[derive(Debug, Clone)]
pub enum AppRequest {
    OpenInEditor {
        path: PathBuf,
        line: usize,
    },
    /// Launch the OS handler for an `obsidian://...` URL. Unlike
    /// [`OpenInEditor`], the app does NOT suspend the alt-screen — Obsidian
    /// raises its own window, so the TUI keeps drawing underneath.
    OpenInObsidian {
        url: String,
    },
    /// Show a transient status-bar message — replaces the
    /// `refreshed HH:MM:SS` cell for ~3 seconds.
    Toast {
        text: String,
        style: ToastStyle,
    },
}

/// Visual styling for a [`Toast`]. Green for success (create, save),
/// red for errors (IO failures, validation fallout). The middle of the
/// status bar runs all toasts through one renderer, so adding a new
/// shade later is a single match arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastStyle {
    Success,
    Error,
}

/// What the App should do after a tab handles an event. `Consumed` and `Quit`
/// are part of the contract but unused in session 1; sessions 2+ surface them
/// (e.g. a tab swallowing `q` while editing a query, or a tab signalling exit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventOutcome {
    Consumed,
    NotHandled,
    SwitchTab(usize),
    /// Tab signals the app should exit. Currently unused — `q`/`Ctrl+C` are
    /// handled by the global keymap — but kept so a future tab (e.g. a modal
    /// confirm-quit dialog) can request exit without reaching for app state.
    #[allow(dead_code)]
    Quit,
}

/// Shared context passed to tabs on every event/render.
///
/// `today` is the date used to resolve DSL keywords (`today` / `tomorrow`)
/// and to bucket overdue vs upcoming tasks; it is fixed for the lifetime of
/// the App so a long-running session has stable bucketing. The clock for
/// the live sidebar display is separate (see `tabs::tasks::ClockFn`).
///
/// `last_refresh` is wrapped in a `Cell` so views can update it through
/// the shared `&TabCtx` they receive in `render` and `handle_event` —
/// the App reads it back when drawing the status bar.
pub struct TabCtx<'a> {
    /// `&Arc<Vault>` (rather than `&Vault`) so a tab can `Arc::clone(ctx.vault)`
    /// to hand a vault handle to a widget whose lifetime outlives the
    /// borrow of `App` — e.g. the fuzzy picker tucked inside a popup.
    /// Existing `ctx.vault.scan()` / `ctx.vault.path` callers keep working
    /// through `Arc`'s auto-deref to `&Vault`.
    pub vault: &'a Arc<Vault>,
    /// Per-vault "recently opened notes" log (plan 008). Shared across
    /// the four picker sites so an open recorded by one shows up in the
    /// others, and shared with the open-chokepoint sites so opens get
    /// recorded as the user navigates.
    pub recents: &'a Arc<RecentsLog>,
    pub today: NaiveDate,
    pub last_refresh: &'a Cell<Option<DateTime<Local>>>,
    /// Pending side-effect for the App to handle after `handle_event` returns.
    /// `RefCell` rather than `Cell` because [`AppRequest`] isn't `Copy`.
    pub pending_request: &'a RefCell<Option<AppRequest>>,
}

/// A top-level tab in the TUI. New tabs slot in by adding a `Box<dyn Tab>` to
/// the App's tab list — no surgery on the core loop.
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
