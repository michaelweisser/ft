//! `ft completions <shell>` — emit shell completion script to stdout.

use anyhow::Result;
use clap::{Args, CommandFactory, ValueEnum};
use clap_complete::{generate, Shell};

#[derive(Args, Debug)]
pub struct CompletionsArgs {
    /// Target shell.
    #[arg(value_enum)]
    pub shell: ShellArg,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum ShellArg {
    Bash,
    Zsh,
    Fish,
    Elvish,
    Powershell,
}

impl From<ShellArg> for Shell {
    fn from(s: ShellArg) -> Self {
        match s {
            ShellArg::Bash => Shell::Bash,
            ShellArg::Zsh => Shell::Zsh,
            ShellArg::Fish => Shell::Fish,
            ShellArg::Elvish => Shell::Elvish,
            ShellArg::Powershell => Shell::PowerShell,
        }
    }
}

pub fn run(args: CompletionsArgs) -> Result<()> {
    let mut cmd = crate::Cli::command();
    let bin = cmd.get_name().to_string();
    generate(
        Shell::from(args.shell),
        &mut cmd,
        bin,
        &mut std::io::stdout(),
    );
    Ok(())
}
