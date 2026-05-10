use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use ft_core::search::{fuzzy_find, Hit, Query, SearchOptions};
use ft_core::vault::Vault;

/// Output format for `ft find`.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum Format {
    /// Human-readable, one hit per line: `path[:line]\theading`.
    /// Colorized when stdout is a TTY (and neither `--no-color` nor
    /// `NO_COLOR` is set).
    Plain,
    /// One JSON object per line: `{path, line?, heading?, level?, score}`.
    Ndjson,
}

#[derive(Args, Debug)]
pub struct FindArgs {
    /// Fuzzy query. Plain text fuzzy-matches filenames; `text#heading`
    /// also fuzzy-matches headings inside each candidate file; `#heading`
    /// searches headings across the whole vault.
    #[arg(value_name = "QUERY", required = true)]
    pub query: Vec<String>,

    /// Maximum number of hits to return. Default: 25.
    #[arg(long, default_value_t = 25)]
    pub limit: usize,

    /// Extract and rank headings even when QUERY has no `#`. Useful for
    /// jump-list style results (file + the file's first heading).
    #[arg(long)]
    pub include_headings: bool,

    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Plain)]
    pub format: Format,

    /// Disable colored output (also honored: `NO_COLOR` env var).
    #[arg(long)]
    pub no_color: bool,
}

pub fn run(args: FindArgs, vault_flag: Option<PathBuf>) -> Result<ExitCode> {
    let vault = Vault::discover(vault_flag).context("could not locate an Obsidian vault")?;

    let query_str = args.query.join(" ");
    let query = Query::parse(&query_str);
    if query.is_empty() {
        // clap's `required = true` makes the empty-args case impossible,
        // but a query string of only whitespace is reachable.
        anyhow::bail!("query is empty");
    }

    let opts = SearchOptions {
        limit: args.limit,
        include_headings: args.include_headings,
    };
    let hits = fuzzy_find(&vault, &query, opts);

    let use_color = !args.no_color
        && std::env::var_os("NO_COLOR").is_none()
        && is_terminal::IsTerminal::is_terminal(&io::stdout());

    let stdout = io::stdout();
    let mut out = stdout.lock();
    match args.format {
        Format::Plain => write_plain(&mut out, &hits, use_color)?,
        Format::Ndjson => write_ndjson(&mut out, &hits)?,
    }
    out.flush().ok();

    if hits.is_empty() {
        Ok(ExitCode::from(1))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn write_plain(out: &mut impl Write, hits: &[Hit], use_color: bool) -> io::Result<()> {
    for hit in hits {
        let path = hit.path.display().to_string();
        match (&hit.heading, use_color) {
            (Some(h), true) => {
                writeln!(
                    out,
                    "{blue}{path}{reset}:{dim}{line}{reset}\t{yellow}{text}{reset}",
                    blue = ANSI_BLUE,
                    reset = ANSI_RESET,
                    dim = ANSI_DIM,
                    yellow = ANSI_YELLOW,
                    path = path,
                    line = h.line,
                    text = h.text,
                )?;
            }
            (Some(h), false) => {
                writeln!(out, "{path}:{}\t{}", h.line, h.text)?;
            }
            (None, true) => {
                writeln!(
                    out,
                    "{blue}{path}{reset}",
                    blue = ANSI_BLUE,
                    reset = ANSI_RESET,
                    path = path,
                )?;
            }
            (None, false) => {
                writeln!(out, "{path}")?;
            }
        }
    }
    Ok(())
}

fn write_ndjson(out: &mut impl Write, hits: &[Hit]) -> io::Result<()> {
    for hit in hits {
        let obj = match &hit.heading {
            Some(h) => serde_json::json!({
                "path": hit.path.display().to_string(),
                "line": h.line,
                "heading": h.text,
                "level": h.level,
                "score": hit.total_score,
            }),
            None => serde_json::json!({
                "path": hit.path.display().to_string(),
                "score": hit.total_score,
            }),
        };
        writeln!(out, "{obj}")?;
    }
    Ok(())
}

// Minimal ANSI palette — matches the same blue/yellow/dim used by the
// task-list table renderer so the two surfaces feel coherent.
const ANSI_RESET: &str = "\x1b[0m";
const ANSI_DIM: &str = "\x1b[2m";
const ANSI_BLUE: &str = "\x1b[34m";
const ANSI_YELLOW: &str = "\x1b[33m";
