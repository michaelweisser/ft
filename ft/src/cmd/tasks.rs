use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use chrono::{Local, NaiveDate};
use clap::{Args, Subcommand, ValueEnum};
use ft_core::{
    query::{dsl, expr::Expr, filter::Filter, preset, sort::sort_by_keys, SortKey, SortOrder},
    task::{Priority, Status, Task},
    vault::Vault,
};

use crate::output::{self, Format, GroupBy};

#[derive(Args)]
pub struct TasksArgs {
    #[command(subcommand)]
    pub command: TasksCommand,
}

#[derive(Subcommand)]
pub enum TasksCommand {
    /// List tasks across the vault, optionally filtered.
    List(ListArgs),
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum StatusFlag {
    Open,
    Done,
    #[value(name = "in-progress")]
    InProgress,
    Cancelled,
}

impl From<StatusFlag> for Status {
    fn from(s: StatusFlag) -> Self {
        match s {
            StatusFlag::Open => Status::Open,
            StatusFlag::Done => Status::Done,
            StatusFlag::InProgress => Status::InProgress,
            StatusFlag::Cancelled => Status::Cancelled,
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum PriorityFlag {
    Highest,
    High,
    Medium,
    Low,
    Lowest,
}

impl From<PriorityFlag> for Priority {
    fn from(p: PriorityFlag) -> Self {
        match p {
            PriorityFlag::Highest => Priority::Highest,
            PriorityFlag::High => Priority::High,
            PriorityFlag::Medium => Priority::Medium,
            PriorityFlag::Low => Priority::Low,
            PriorityFlag::Lowest => Priority::Lowest,
        }
    }
}

#[derive(Args, Debug)]
pub struct ListArgs {
    /// Preset name (built-in or from config). If no preset of this name
    /// exists, the value is parsed as a query DSL string instead.
    #[arg(value_name = "PRESET_OR_QUERY")]
    pub preset_or_query: Option<String>,

    /// Explicit query DSL (composed with flags and any positional query as
    /// additional `and` clauses). See docs/query-dsl.md for the supported
    /// subset.
    #[arg(long, value_name = "DSL")]
    pub query: Option<String>,

    /// Filter by status (repeatable).
    #[arg(long, value_enum)]
    pub status: Vec<StatusFlag>,

    /// Filter by priority (repeatable).
    #[arg(long, value_enum)]
    pub priority: Vec<PriorityFlag>,

    /// Filter by tag (repeatable). Leading `#` is optional.
    #[arg(long)]
    pub tag: Vec<String>,

    /// Substring filter on the source file path (repeatable; all must match).
    #[arg(long)]
    pub path: Vec<String>,

    /// Only tasks due strictly before this date (YYYY-MM-DD).
    #[arg(long, value_name = "DATE")]
    pub due_before: Option<NaiveDate>,

    /// Only tasks due strictly after this date (YYYY-MM-DD).
    #[arg(long, value_name = "DATE")]
    pub due_after: Option<NaiveDate>,

    /// Only tasks scheduled strictly before this date (YYYY-MM-DD).
    #[arg(long, value_name = "DATE")]
    pub scheduled_before: Option<NaiveDate>,

    /// Only tasks scheduled strictly after this date (YYYY-MM-DD).
    #[arg(long, value_name = "DATE")]
    pub scheduled_after: Option<NaiveDate>,

    /// Only tasks that have a due date.
    #[arg(long, conflicts_with = "no_due")]
    pub has_due: bool,

    /// Only tasks without a due date.
    #[arg(long)]
    pub no_due: bool,

    /// Sort keys, comma-separated or repeated (e.g. `--sort priority,due` or
    /// `--sort priority --sort due`). Suffix `:reverse` to invert a key
    /// (e.g. `--sort due:reverse`). Overrides any DSL `sort by` clause.
    #[arg(long)]
    pub sort: Vec<String>,

    /// Group rows in the table output. Has no effect on JSON / NDJSON / markdown.
    #[arg(long, value_enum)]
    pub group_by: Option<GroupBy>,

    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Table)]
    pub format: Format,

    /// Disable colored output (also honored: `NO_COLOR` env var).
    #[arg(long)]
    pub no_color: bool,

    /// Treat an empty result set as a successful run. Default: exit 1 when
    /// nothing matches (useful in scripting).
    #[arg(long)]
    pub allow_empty: bool,
}

pub fn run(args: TasksArgs, vault_flag: Option<PathBuf>) -> Result<ExitCode> {
    match args.command {
        TasksCommand::List(list_args) => run_list(list_args, vault_flag),
    }
}

fn run_list(args: ListArgs, vault_flag: Option<PathBuf>) -> Result<ExitCode> {
    let vault = Vault::discover(vault_flag).context("could not locate an Obsidian vault")?;
    let scan = vault.scan();

    for err in &scan.errors {
        tracing::warn!("{}", err);
    }

    if args.has_due && args.no_due {
        return Err(anyhow!("--has-due and --no-due are mutually exclusive"));
    }

    let filter = build_filter(&args);
    // `FT_TODAY=YYYY-MM-DD` overrides the system clock so DSL date keywords
    // (`today`/`tomorrow`/`yesterday`) and presets like `today` / `overdue`
    // are deterministic in tests and reproducible scripts.
    let today = std::env::var("FT_TODAY")
        .ok()
        .and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok())
        .unwrap_or_else(|| Local::now().date_naive());

    // Resolve positional argument: preset (built-in or user) → expand to DSL.
    // Anything else is treated as a DSL string.
    let positional_dsl = args
        .preset_or_query
        .as_deref()
        .map(|name| resolve_preset(name, &vault).unwrap_or_else(|| name.to_string()));

    let mut combined_expr: Option<Expr> = None;
    let mut dsl_sort: Vec<(SortKey, SortOrder)> = Vec::new();
    let mut dsl_limit: Option<usize> = None;

    for src in [positional_dsl.as_deref(), args.query.as_deref()]
        .into_iter()
        .flatten()
    {
        let q = dsl::parse(src, today).map_err(|e| anyhow!("invalid query `{src}`: {e}"))?;
        if let Some(e) = q.expr {
            combined_expr = Some(match combined_expr.take() {
                None => e,
                Some(prev) => Expr::And(vec![prev, e]),
            });
        }
        if !q.sort_keys.is_empty() {
            dsl_sort = q.sort_keys;
        }
        if let Some(l) = q.limit {
            dsl_limit = Some(l);
        }
    }

    let mut matches: Vec<&Task> = scan
        .tasks
        .iter()
        .filter(|t| filter.matches(t))
        .filter(|t| combined_expr.as_ref().is_none_or(|e| e.matches(t)))
        .collect();

    let cli_sort = parse_cli_sort_keys(&args.sort)?;
    let sort_keys: Vec<(SortKey, SortOrder)> = if !cli_sort.is_empty() {
        cli_sort
    } else {
        dsl_sort
    };
    sort_by_keys(&mut matches, &sort_keys);

    if let Some(limit) = dsl_limit {
        matches.truncate(limit);
    }

    let exit = if matches.is_empty() && !args.allow_empty {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    };

    match args.format {
        Format::Table => {
            let use_color = !args.no_color
                && std::env::var_os("NO_COLOR").is_none()
                && is_terminal::IsTerminal::is_terminal(&std::io::stdout());
            let opts = output::table::TableOpts { use_color };
            if let Some(group) = args.group_by {
                let groups = group_tasks(&matches, group);
                let out = output::table::render_grouped(&groups, opts);
                print!("{out}");
            } else {
                let out = output::table::render(&matches, opts);
                println!("{out}");
            }
        }
        Format::Json => output::json::render(&matches)?,
        Format::Ndjson => output::ndjson::render(&matches)?,
        Format::Markdown => print!("{}", output::markdown::render(&matches)),
    }

    Ok(exit)
}

/// Look up a preset by name, preferring the user's config over built-ins.
fn resolve_preset(name: &str, vault: &Vault) -> Option<String> {
    if let Some(user) = vault.config.config.presets.get(name) {
        return Some(user.clone());
    }
    preset::builtin(name).map(|s| s.to_string())
}

fn build_filter(args: &ListArgs) -> Filter {
    let has_due = if args.has_due {
        Some(true)
    } else if args.no_due {
        Some(false)
    } else {
        None
    };

    Filter {
        statuses: args.status.iter().copied().map(Into::into).collect(),
        priorities: args.priority.iter().copied().map(Into::into).collect(),
        tags: args.tag.clone(),
        paths: args.path.clone(),
        due_before: args.due_before,
        due_after: args.due_after,
        scheduled_before: args.scheduled_before,
        scheduled_after: args.scheduled_after,
        has_due,
    }
}

/// Parse `--sort` values: each value can be a comma-separated list of keys,
/// each key optionally suffixed with `:reverse` or `:desc` for descending.
fn parse_cli_sort_keys(values: &[String]) -> Result<Vec<(SortKey, SortOrder)>> {
    let mut out = Vec::new();
    for v in values {
        for part in v.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let (name, order) = match part.rsplit_once(':') {
                Some((n, "reverse" | "desc" | "rev")) => (n, SortOrder::Desc),
                Some((n, "asc")) => (n, SortOrder::Asc),
                Some((_, other)) => {
                    return Err(anyhow!(
                        "unknown sort modifier `:{other}` in `--sort {part}` (use `:reverse` or `:asc`)"
                    ));
                }
                None => (part, SortOrder::Asc),
            };
            let key = dsl::parse_sort_key(name).map_err(|e| anyhow!("bad sort key: {e}"))?;
            out.push((key, order));
        }
    }
    Ok(out)
}

/// Group tasks by the given key, returning sorted groups.
fn group_tasks<'a>(tasks: &[&'a Task], by: GroupBy) -> Vec<(String, Vec<&'a Task>)> {
    let mut buckets: BTreeMap<String, Vec<&Task>> = BTreeMap::new();
    for t in tasks {
        for label in group_labels(t, by) {
            buckets.entry(label).or_default().push(t);
        }
    }
    buckets.into_iter().collect()
}

/// One task may belong to multiple groups (only `Tag` produces > 1 today).
fn group_labels(t: &Task, by: GroupBy) -> Vec<String> {
    match by {
        GroupBy::Path => vec![t.source_file.display().to_string()],
        GroupBy::Folder => {
            let folder = t
                .source_file
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            vec![if folder.is_empty() {
                ".".into()
            } else {
                folder
            }]
        }
        GroupBy::Due => vec![t
            .due
            .map(|d| d.to_string())
            .unwrap_or_else(|| "(no due date)".into())],
        GroupBy::Priority => vec![match t.priority {
            Some(Priority::Highest) => "highest".into(),
            Some(Priority::High) => "high".into(),
            Some(Priority::Medium) => "medium".into(),
            Some(Priority::Low) => "low".into(),
            Some(Priority::Lowest) => "lowest".into(),
            None => "(no priority)".into(),
        }],
        GroupBy::Tag => {
            if t.tags.is_empty() {
                vec!["(no tags)".into()]
            } else {
                t.tags.iter().map(|s| format!("#{s}")).collect()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_sort_parses_compound() {
        let v = vec!["priority,due:reverse".to_string()];
        let parsed = parse_cli_sort_keys(&v).unwrap();
        assert_eq!(
            parsed,
            vec![
                (SortKey::Priority, SortOrder::Asc),
                (SortKey::Due, SortOrder::Desc)
            ]
        );
    }

    #[test]
    fn cli_sort_parses_repeated() {
        let v = vec!["priority".into(), "due:reverse".into()];
        let parsed = parse_cli_sort_keys(&v).unwrap();
        assert_eq!(
            parsed,
            vec![
                (SortKey::Priority, SortOrder::Asc),
                (SortKey::Due, SortOrder::Desc)
            ]
        );
    }

    #[test]
    fn cli_sort_rejects_unknown_key() {
        let v = vec!["nonsense".to_string()];
        assert!(parse_cli_sort_keys(&v).is_err());
    }

    #[test]
    fn cli_sort_rejects_unknown_modifier() {
        let v = vec!["due:sideways".to_string()];
        assert!(parse_cli_sort_keys(&v).is_err());
    }
}
