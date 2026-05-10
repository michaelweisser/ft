use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod cmd;
mod output;
mod tui;

#[derive(Parser)]
#[command(
    name = "ft",
    version,
    about = "Command-line interface to your Obsidian vault"
)]
pub(crate) struct Cli {
    /// Obsidian vault root (overrides $FT_VAULT and auto-discovery)
    #[arg(long, global = true, value_name = "DIR")]
    vault: Option<std::path::PathBuf>,

    /// Increase verbosity: -v = info, -vv = debug, -vvv = trace
    #[arg(short, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// Emit errors as a JSON object on stderr (`{"error": ..., "chain": [...]}`)
    /// instead of human-readable text. Useful for scripting.
    #[arg(long, global = true)]
    json_errors: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)] // single-instance argv parse; size doesn't matter
enum Commands {
    /// Show resolved vault path, active config files, and merged configuration
    Vault(cmd::vault::VaultArgs),
    /// Task operations: list, create, complete, move
    Tasks(cmd::tasks::TasksArgs),
    /// Launch the interactive terminal UI
    Tui(cmd::tui::TuiArgs),
    /// Generate shell completion script
    Completions(cmd::completions::CompletionsArgs),
    /// Render man pages from the clap definition
    Man(cmd::man::ManArgs),
}

fn main() -> ExitCode {
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

    let json_errors = cli.json_errors;
    let vault = cli.vault;

    let result: Result<ExitCode> = match cli.command {
        Commands::Vault(args) => cmd::vault::run(args, vault).map(|_| ExitCode::SUCCESS),
        Commands::Tasks(args) => cmd::tasks::run(args, vault),
        Commands::Tui(args) => cmd::tui::run(args, vault).map(|_| ExitCode::SUCCESS),
        Commands::Completions(args) => cmd::completions::run(args).map(|_| ExitCode::SUCCESS),
        Commands::Man(args) => cmd::man::run(args).map(|_| ExitCode::SUCCESS),
    };

    match result {
        Ok(code) => code,
        Err(e) => {
            if json_errors {
                print_json_error(&e);
            } else {
                eprintln!("Error: {e:#}");
            }
            ExitCode::FAILURE
        }
    }
}

fn print_json_error(e: &anyhow::Error) {
    let chain: Vec<String> = e.chain().map(|c| c.to_string()).collect();
    let body = serde_json::json!({
        "error": e.to_string(),
        "chain": chain,
    });
    eprintln!("{body}");
}
