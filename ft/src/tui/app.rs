use std::cell::Cell;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Local, NaiveDate};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ft_core::vault::Vault;
use ratatui::Frame;

#[cfg(test)]
use crate::tui::tabs::tasks::ClockFn;
use crate::tui::{
    event::{Event, EventStream},
    tab::{EventOutcome, Tab, TabCtx},
    tabs::{tasks::TasksTab, welcome::WelcomeTab},
    ui::{self, Mode},
    Tui,
};

pub struct App {
    vault: Vault,
    today: NaiveDate,
    tabs: Vec<Box<dyn Tab>>,
    active: usize,
    mode: Mode,
    last_refresh: Cell<Option<DateTime<Local>>>,
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
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let [tab_bar, body, status_bar] = ui::split_screen(frame.area());
        let titles: Vec<&str> = self.tabs.iter().map(|t| t.title()).collect();
        ui::render_tab_bar(frame, tab_bar, &titles, self.active);

        // Render the status bar before constructing the body's `TabCtx` —
        // the ctx borrows `&self.last_refresh` for the body to write through,
        // and the status bar reads the same Cell to display the timestamp.
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
        };
        ui::render_body(frame, body, self.tabs[self.active].as_mut(), &ctx);

        ui::render_status_bar(
            frame,
            status_bar,
            &vault_name,
            self.tabs[self.active].title(),
            self.last_refresh.get(),
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
        };
        self.tabs[self.active].on_blur(&mut ctx)?;
        self.active = idx;
        self.tabs[self.active].on_focus(&mut ctx)?;
        Ok(())
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
}
