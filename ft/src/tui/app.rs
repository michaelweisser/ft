use std::cell::{Cell, RefCell};
use std::io;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDate};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ft_core::vault::Vault;
use ratatui::Frame;

#[cfg(test)]
use crate::tui::tabs::tasks::ClockFn;
use crate::tui::{
    event::{Event, EventStream},
    tab::{AppRequest, EventOutcome, Tab, TabCtx, ToastStyle},
    tabs::{tasks::TasksTab, welcome::WelcomeTab},
    ui::{self, Mode},
    Tui,
};

/// A transient status-bar message. The center cell of the status bar
/// shows the toast text in place of `refreshed HH:MM:SS` until the
/// deadline elapses; the 1-second tick already drives the redraw loop,
/// so expiry happens naturally without a separate timer.
#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    pub style: ToastStyle,
    pub deadline: std::time::Instant,
}

/// How long a toast stays on screen unless overwritten by a later one.
/// Picked to be long enough to read a short message but short enough not
/// to mask a subsequent action.
const TOAST_DURATION: Duration = Duration::from_secs(3);

pub struct App {
    vault: Vault,
    today: NaiveDate,
    tabs: Vec<Box<dyn Tab>>,
    active: usize,
    mode: Mode,
    last_refresh: Cell<Option<DateTime<Local>>>,
    pending_request: RefCell<Option<AppRequest>>,
    /// Active toast, if any. `RefCell` because `Toast` is `!Copy`.
    toast: RefCell<Option<Toast>>,
    should_quit: bool,
}

impl App {
    pub fn new(vault: Vault) -> Self {
        let today = resolve_today();
        let tabs: Vec<Box<dyn Tab>> = vec![Box::new(WelcomeTab::new()), Box::new(TasksTab::new())];
        Self::with_tabs(vault, today, tabs)
    }

    fn with_tabs(vault: Vault, today: NaiveDate, tabs: Vec<Box<dyn Tab>>) -> Self {
        Self {
            vault,
            today,
            tabs,
            active: 0,
            mode: Mode::Normal,
            last_refresh: Cell::new(None),
            pending_request: RefCell::new(None),
            toast: RefCell::new(None),
            should_quit: false,
        }
    }

    pub fn run(&mut self, terminal: &mut Tui) -> Result<()> {
        let events = EventStream::new(Duration::from_secs(1));

        // Initial focus event so the first tab can lazily load if needed.
        {
            let mut ctx = TabCtx {
                vault: &self.vault,
                today: self.today,
                last_refresh: &self.last_refresh,
                pending_request: &self.pending_request,
            };
            self.tabs[self.active].on_focus(&mut ctx)?;
        }

        loop {
            terminal.draw(|f| self.draw(f))?;
            let ev = events.next()?;
            self.handle_event(ev)?;
            if self.should_quit {
                return Ok(());
            }
            // Service any side-effect requests the view raised. Done outside
            // `handle_event` so the App owns the Terminal during suspend.
            if let Some(req) = self.pending_request.take() {
                self.service_request(terminal, &events, req)?;
            }
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let [tab_bar, body, status_bar] = ui::split_screen(frame.area());
        let titles: Vec<&str> = self.tabs.iter().map(|t| t.title()).collect();
        ui::render_tab_bar(frame, tab_bar, &titles, self.active);

        // Render the status bar after the body so the body can update
        // `last_refresh` (via the Cell) before we read it back.
        let vault_name = self
            .vault
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.vault.path.display().to_string());

        let ctx = TabCtx {
            vault: &self.vault,
            today: self.today,
            last_refresh: &self.last_refresh,
            pending_request: &self.pending_request,
        };
        ui::render_body(frame, body, self.tabs[self.active].as_mut(), &ctx);

        // Expire stale toasts before drawing so the cell falls back to
        // the refresh time on the very tick the deadline passes.
        let toast_now = std::time::Instant::now();
        let active_toast = {
            let mut slot = self.toast.borrow_mut();
            if let Some(t) = slot.as_ref() {
                if t.deadline <= toast_now {
                    *slot = None;
                }
            }
            slot.clone()
        };
        ui::render_status_bar(
            frame,
            status_bar,
            &vault_name,
            self.tabs[self.active].title(),
            self.last_refresh.get(),
            active_toast.as_ref(),
            self.mode,
        );

        if self.mode == Mode::Help {
            ui::render_help_overlay(frame, frame.area());
        }
    }

    fn handle_event(&mut self, ev: Event) -> Result<()> {
        // Help overlay swallows everything except its own dismiss keys.
        if self.mode == Mode::Help {
            if let Event::Key(k) = ev {
                if matches!(
                    k.code,
                    KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q')
                ) {
                    self.mode = Mode::Normal;
                }
            }
            return Ok(());
        }

        // Route to the active tab first.
        let outcome = {
            let mut ctx = TabCtx {
                vault: &self.vault,
                today: self.today,
                last_refresh: &self.last_refresh,
                pending_request: &self.pending_request,
            };
            self.tabs[self.active].handle_event(ev.clone(), &mut ctx)?
        };
        match outcome {
            EventOutcome::Consumed => return Ok(()),
            EventOutcome::Quit => {
                self.should_quit = true;
                return Ok(());
            }
            EventOutcome::SwitchTab(idx) => {
                self.switch_tab(idx)?;
                return Ok(());
            }
            EventOutcome::NotHandled => {}
        }

        // Tab didn't handle it — fall back to global keybindings.
        if let Event::Key(k) = ev {
            self.handle_global_key(k)?;
        }
        Ok(())
    }

    fn handle_global_key(&mut self, k: KeyEvent) -> Result<()> {
        match (k.code, k.modifiers) {
            (KeyCode::Char('q'), KeyModifiers::NONE) => {
                self.should_quit = true;
            }
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            (KeyCode::Char('?'), _) => {
                self.mode = Mode::Help;
            }
            (KeyCode::Tab, _) => {
                let next = (self.active + 1) % self.tabs.len();
                self.switch_tab(next)?;
            }
            (KeyCode::BackTab, _) => {
                let prev = (self.active + self.tabs.len() - 1) % self.tabs.len();
                self.switch_tab(prev)?;
            }
            (KeyCode::Char(c), _) if c.is_ascii_digit() => {
                let idx = (c as u8 - b'1') as usize;
                if idx < self.tabs.len() {
                    self.switch_tab(idx)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn switch_tab(&mut self, idx: usize) -> Result<()> {
        if idx == self.active || idx >= self.tabs.len() {
            return Ok(());
        }
        let mut ctx = TabCtx {
            vault: &self.vault,
            today: self.today,
            last_refresh: &self.last_refresh,
            pending_request: &self.pending_request,
        };
        self.tabs[self.active].on_blur(&mut ctx)?;
        self.active = idx;
        self.tabs[self.active].on_focus(&mut ctx)?;
        Ok(())
    }

    fn service_request(
        &mut self,
        terminal: &mut Tui,
        events: &EventStream,
        req: AppRequest,
    ) -> Result<()> {
        match req {
            AppRequest::OpenInEditor { path, line } => {
                suspend_terminal(terminal).context("could not suspend terminal for $EDITOR")?;
                let status = spawn_editor(&path, line);
                restore_terminal(terminal).context("could not restore terminal after $EDITOR")?;
                // Terminals often emit response sequences (DA1, DCS replies
                // for XTGETTCAP) when raw mode flips back on, and the user
                // may have typed during the editor session. Drain so the
                // next `events.next()` returns a genuine keypress and not
                // a `/` from a DCS reply that puts us into query-edit mode.
                events.drain(Duration::from_millis(120));
                terminal.clear()?;
                // Whatever the editor did, force a refresh so the row reflects
                // the on-disk state.
                {
                    let mut ctx = TabCtx {
                        vault: &self.vault,
                        today: self.today,
                        last_refresh: &self.last_refresh,
                        pending_request: &self.pending_request,
                    };
                    self.tabs[self.active].refresh(&mut ctx)?;
                }
                status?;
                Ok(())
            }
            AppRequest::Toast { text, style } => {
                *self.toast.borrow_mut() = Some(Toast {
                    text,
                    style,
                    deadline: std::time::Instant::now() + TOAST_DURATION,
                });
                Ok(())
            }
        }
    }
}

/// Resolve "today" for the current run. Honors `FT_TODAY=YYYY-MM-DD` to keep
/// the TUI deterministic in tests and reproducible with the CLI.
fn resolve_today() -> NaiveDate {
    std::env::var("FT_TODAY")
        .ok()
        .and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok())
        .unwrap_or_else(|| Local::now().date_naive())
}

// --- editor handoff ----------------------------------------------------------

fn suspend_terminal(terminal: &mut Tui) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn restore_terminal(terminal: &mut Tui) -> Result<()> {
    enable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableMouseCapture
    )?;
    terminal.hide_cursor()?;
    Ok(())
}

/// Spawn `$EDITOR` (or `$VISUAL`, falling back to `vi`) on `path`, jumping to
/// `line` if the editor supports the `+N` flag (vim/nvim/nano/emacs all do).
fn spawn_editor(path: &Path, line: usize) -> Result<()> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());

    // Best-effort split for "editor with args" (e.g. EDITOR="code -w").
    let mut parts = editor.split_whitespace();
    let program = parts.next().unwrap_or("vi");
    let extra_args: Vec<&str> = parts.collect();

    let line_arg = format!("+{line}");
    let mut cmd = Command::new(program);
    cmd.args(&extra_args).arg(&line_arg).arg(path);

    let status = cmd.status().with_context(|| {
        format!("failed to launch $EDITOR ({program}); set EDITOR or VISUAL to a working editor")
    })?;
    if !status.success() {
        // Editor exited non-zero — surface as warning via the `last_refresh`
        // log... actually we can't log to TUI yet. Just write to stderr; main
        // routes that to a sink so it's harmless. Future: a status toast.
        let _ = io::Write::write_all(&mut io::stderr(), b"editor exited non-zero\n");
    }
    Ok(())
}

// --- test-only helpers ---------------------------------------------------

#[cfg(test)]
impl App {
    /// Construct an App without starting the event loop. Useful for
    /// snapshot tests that drive `draw` directly with a TestBackend.
    pub fn for_test(vault: Vault) -> Self {
        Self::new(vault)
    }

    /// Like [`for_test`], but injects a fixed clock and derives `today` from
    /// it so snapshots are deterministic without relying on `FT_TODAY`.
    pub fn for_test_with_clock(vault: Vault, clock: ClockFn) -> Self {
        let today = clock().date_naive();
        let tabs: Vec<Box<dyn Tab>> = vec![
            Box::new(WelcomeTab::new()),
            Box::new(TasksTab::with_clock(clock)),
        ];
        Self::with_tabs(vault, today, tabs)
    }

    pub fn render_to(&mut self, frame: &mut Frame) {
        self.draw(frame);
    }

    pub fn enter_help(&mut self) {
        self.mode = Mode::Help;
    }

    pub fn switch_to(&mut self, idx: usize) -> Result<()> {
        self.switch_tab(idx)
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn active_title(&self) -> &str {
        self.tabs[self.active].title()
    }

    pub fn dispatch(&mut self, ev: Event) -> Result<()> {
        self.handle_event(ev)
    }

    pub fn is_quit(&self) -> bool {
        self.should_quit
    }

    /// Inspect or take any pending request that the active tab/view raised.
    /// Used by tests to assert that an Enter keypress queued an editor open.
    pub fn take_pending_request(&self) -> Option<AppRequest> {
        self.pending_request.borrow_mut().take()
    }

    /// Service whatever pending `AppRequest` is queued (or do nothing if
    /// none). Mirrors what `run` does between iterations — tests use this
    /// to drive the toast / refresh side-effects without spinning up a
    /// real event loop.
    pub fn service_pending_for_test(&mut self) -> Result<()> {
        if let Some(req) = self.pending_request.borrow_mut().take() {
            match req {
                AppRequest::Toast { text, style } => {
                    *self.toast.borrow_mut() = Some(Toast {
                        text,
                        style,
                        deadline: std::time::Instant::now() + TOAST_DURATION,
                    });
                }
                // Other variants need terminal state; tests that exercise
                // them go through the real `service_request` path.
                _ => {
                    *self.pending_request.borrow_mut() = Some(req);
                }
            }
        }
        Ok(())
    }

    /// Currently-active toast, if any. Used by tests to assert the
    /// post-create UX.
    pub fn current_toast(&self) -> Option<Toast> {
        self.toast.borrow().clone()
    }
}
