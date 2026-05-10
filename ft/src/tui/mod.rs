mod app;
mod event;
mod tab;
mod tabs;
#[cfg(test)]
mod tests;
mod ui;
mod widgets;

use std::io::{self, Stdout};

use anyhow::{Context, Result};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ft_core::vault::Vault;
use ratatui::{backend::CrosstermBackend, Terminal};

pub use app::App;

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Entry point for `ft tui`. Sets up the terminal, runs the event loop, and
/// always restores the terminal on exit (success or panic).
pub fn run(vault: Vault) -> Result<()> {
    let mut terminal = setup_terminal().context("failed to enter TUI mode")?;
    let mut app = App::new(vault);
    let result = app.run(&mut terminal);
    restore_terminal(&mut terminal).context("failed to restore terminal")?;
    result
}

fn setup_terminal() -> Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).map_err(Into::into)
}

fn restore_terminal(terminal: &mut Tui) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}
