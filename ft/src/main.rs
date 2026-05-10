use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod cmd;
mod output;

#[derive(Parser)]
#[command(
    name = "ft",
    version,
    about = "Command-line interface to your Obsidian vault"
)]
struct Cli {
    /// Obsidian vault root (overrides $FT_VAULT and auto-discovery)
    #[arg(long, global = true, value_name = "DIR")]
    vault: Option<std::path::PathBuf>,

    /// Increase verbosity: -v = info, -vv = debug, -vvv = trace
    #[arg(short, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show resolved vault path, active config files, and merged configuration
    Vault(cmd::vault::VaultArgs),
    /// Task operations: list, create, complete, move
    Tasks(cmd::tasks::TasksArgs),
}

fn main() -> Result<ExitCode> {
    let cli = Cli::parse();

    let level = match cli.verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level)),
        )
        .with_writer(std::io::stderr)
        .init();

    match cli.command {
        Commands::Vault(args) => {
            cmd::vault::run(args, cli.vault)?;
            Ok(ExitCode::SUCCESS)
        }
        Commands::Tasks(args) => cmd::tasks::run(args, cli.vault),
    }
}
