use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use chrono::{Local, NaiveDate};
use clap::{Args, Subcommand, ValueEnum};
use ft_core::{
    daily, dates,
    query::{dsl, expr::Expr, filter::Filter, preset, sort::sort_by_keys, SortKey, SortOrder},
    selector,
    task::{
        ops::{
            self, CompleteError, CompleteOptions, CreateError, CreateInput, CreateOptions, Position,
        },
        Priority, Status, Task,
    },
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
    /// Create a new task.
    Create(CreateArgs),
    /// Mark a task complete (and write the next instance if recurring).
    Complete(CompleteArgs),
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
        TasksCommand::Create(create_args) => run_create(create_args, vault_flag),
        TasksCommand::Complete(complete_args) => run_complete(complete_args, vault_flag),
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

// ── ft tasks create ──────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct CreateArgs {
    /// Task description (free text). Tags from `--tag` are appended.
    #[arg(value_name = "DESCRIPTION", required = true)]
    pub description: Vec<String>,

    /// Due date. Accepts ISO (`2026-05-10`), keywords (`today`, `tomorrow`),
    /// relative (`+3d`, `-1w`), or natural language (`next monday`).
    #[arg(long, value_name = "DATE")]
    pub due: Option<String>,

    /// Scheduled date.
    #[arg(long, value_name = "DATE")]
    pub scheduled: Option<String>,

    /// Start date.
    #[arg(long, value_name = "DATE")]
    pub start: Option<String>,

    /// Priority.
    #[arg(long, value_enum)]
    pub priority: Option<PriorityFlag>,

    /// Tag (repeatable). Leading `#` is optional.
    #[arg(long)]
    pub tag: Vec<String>,

    /// Recurrence rule, preserved verbatim (e.g. `"every month on the 18th"`).
    #[arg(long)]
    pub recurrence: Option<String>,

    /// Stable identifier for this task (the 🆔 field).
    #[arg(long)]
    pub id: Option<String>,

    /// Other task IDs this one depends on (repeatable).
    #[arg(long = "depends-on")]
    pub depends_on: Vec<String>,

    /// Target file (relative to vault root). Defaults to today's daily note.
    #[arg(long, value_name = "PATH")]
    pub file: Option<PathBuf>,

    /// Insert at the end of the section under this heading; create the
    /// heading at file end if missing.
    #[arg(long, value_name = "HEADING", conflicts_with_all = ["at_line", "append"])]
    pub under_heading: Option<String>,

    /// Insert at this 1-indexed line.
    #[arg(long, value_name = "N", conflicts_with_all = ["under_heading", "append"])]
    pub at_line: Option<usize>,

    /// Append at file end (the default for daily notes; explicit for clarity).
    #[arg(long, conflicts_with_all = ["under_heading", "at_line"])]
    pub append: bool,

    /// After writing, open `$EDITOR` on the new task line.
    #[arg(long)]
    pub edit: bool,

    /// Insert even if a duplicate task (same description + dates) already exists.
    #[arg(long)]
    pub force: bool,
}

fn run_create(args: CreateArgs, vault_flag: Option<PathBuf>) -> Result<ExitCode> {
    let vault = Vault::discover(vault_flag).context("could not locate an Obsidian vault")?;
    let today = std::env::var("FT_TODAY")
        .ok()
        .and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok())
        .unwrap_or_else(|| Local::now().date_naive());

    let target = resolve_target_path(&args, &vault, today)?;

    let parse_date = |s: &str, label: &str| -> Result<NaiveDate> {
        dates::parse(s, today).map_err(|e| anyhow!("--{label}: {e}"))
    };

    let description = args.description.join(" ");
    let input = CreateInput {
        description,
        status: Status::Open,
        priority: args.priority.map(Into::into),
        tags: args.tag,
        created: None,
        start: args
            .start
            .as_deref()
            .map(|s| parse_date(s, "start"))
            .transpose()?,
        scheduled: args
            .scheduled
            .as_deref()
            .map(|s| parse_date(s, "scheduled"))
            .transpose()?,
        due: args
            .due
            .as_deref()
            .map(|s| parse_date(s, "due"))
            .transpose()?,
        recurrence: args.recurrence,
        id: args.id,
        depends_on: args.depends_on,
    };

    let position = if let Some(h) = args.under_heading {
        Position::UnderHeading(h)
    } else if let Some(n) = args.at_line {
        Position::AtLine(n)
    } else {
        Position::Append
    };

    let outcome = ops::create_task(
        &target,
        input,
        CreateOptions {
            position,
            force: args.force,
        },
    )
    .map_err(|e| match e {
        CreateError::Duplicate { path, line } => {
            let rel = path.strip_prefix(&vault.path).unwrap_or(&path);
            anyhow!(
                "duplicate task already exists at {}:{} (use --force to insert anyway)",
                rel.display(),
                line
            )
        }
        other => anyhow!("{other}"),
    })?;

    let display_path = target.strip_prefix(&vault.path).unwrap_or(&target);
    println!(
        "Created task at {}:{}\n  {}",
        display_path.display(),
        outcome.line,
        outcome.serialized
    );

    if args.edit {
        open_editor(&target, outcome.line)?;
    }

    Ok(ExitCode::SUCCESS)
}

/// Resolve `--file` against the vault root, or fall back to today's daily
/// note. Returns an absolute path.
fn resolve_target_path(args: &CreateArgs, vault: &Vault, today: NaiveDate) -> Result<PathBuf> {
    if let Some(file) = &args.file {
        let p = if file.is_absolute() {
            file.clone()
        } else {
            vault.path.join(file)
        };
        return Ok(p);
    }

    daily::resolve_daily_path(&vault.path, &vault.config.config.daily_notes, today)
        .map_err(|e| anyhow!("{e}"))
}

fn open_editor(file: &std::path::Path, line: usize) -> Result<()> {
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
    let basename = std::path::Path::new(&editor)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let supports_line_flag = matches!(
        basename,
        "vi" | "vim" | "nvim" | "view" | "nano" | "less" | "more"
    );

    let status = if supports_line_flag {
        std::process::Command::new(&editor)
            .arg(format!("+{line}"))
            .arg(file)
            .status()
    } else {
        std::process::Command::new(&editor).arg(file).status()
    }
    .with_context(|| format!("failed to launch editor `{editor}`"))?;

    if !status.success() {
        return Err(anyhow!(
            "editor `{editor}` exited with status {}",
            status.code().unwrap_or(-1)
        ));
    }
    Ok(())
}

// ── ft tasks complete ────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct CompleteArgs {
    /// Selector: task id (`abc123`), `<file>:<line>`, or fuzzy substring.
    /// If omitted, all open tasks are presented in an interactive picker.
    #[arg(value_name = "SELECTOR")]
    pub selector: Option<String>,

    /// Date to record as the done date. Accepts ISO, keywords, relative,
    /// and natural language (same forms as `ft tasks create --due`).
    /// Defaults to today.
    #[arg(long, value_name = "DATE")]
    pub on: Option<String>,

    /// Skip the interactive picker even when there are multiple matches.
    /// With `--yes`, the picker is replaced by an error listing candidates.
    #[arg(long)]
    pub yes: bool,
}

fn run_complete(args: CompleteArgs, vault_flag: Option<PathBuf>) -> Result<ExitCode> {
    let vault = Vault::discover(vault_flag).context("could not locate an Obsidian vault")?;

    let today = std::env::var("FT_TODAY")
        .ok()
        .and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok())
        .unwrap_or_else(|| Local::now().date_naive());
    let on = match args.on.as_deref() {
        Some(s) => dates::parse(s, today).map_err(|e| anyhow!("--on: {e}"))?,
        None => today,
    };

    let scan = vault.scan();
    for err in &scan.errors {
        tracing::warn!("{}", err);
    }

    let chosen = pick_task(&args, &scan.tasks)?;

    let absolute_path = vault.path.join(&chosen.source_file);
    let outcome = ops::complete_task(&absolute_path, chosen.source_line, CompleteOptions { on })
        .map_err(|e| translate_complete_error(e, &vault.path))?;

    let rel = absolute_path
        .strip_prefix(&vault.path)
        .unwrap_or(&absolute_path);
    println!(
        "Completed {}:{}\n  {}",
        rel.display(),
        outcome.completed_line,
        outcome.completed_serialized
    );
    if let Some(next) = outcome.next_instance {
        println!(
            "Recurring: next instance at {}:{}\n  {}",
            rel.display(),
            next.line,
            next.serialized
        );
    }

    Ok(ExitCode::SUCCESS)
}

/// Resolve the selector argument into exactly one task. The selector can be
/// missing (use the interactive picker over open tasks), produce zero matches
/// (error), one match (use it directly), or many (interactive picker, or error
/// under `--yes` / non-TTY).
fn pick_task<'a>(args: &CompleteArgs, tasks: &'a [Task]) -> Result<&'a Task> {
    let candidates: Vec<&Task> = match args.selector.as_deref() {
        None => tasks
            .iter()
            .filter(|t| !matches!(t.status, Status::Done))
            .collect(),
        Some(s) => {
            // Try the structured form first. If a bare-id-shaped selector
            // matches no task by id, fall through to fuzzy matching so users
            // can type a single word and have it match a description.
            let sel = selector::parse(s);
            let mut matches = selector::resolve(tasks, &sel);
            if matches.is_empty() && matches!(sel, ft_core::selector::Selector::Id(_)) {
                let fuzzy = ft_core::selector::Selector::Fuzzy(s.to_string());
                matches = selector::resolve(tasks, &fuzzy);
            }
            if matches.is_empty() {
                return Err(anyhow!("no tasks match selector `{s}`"));
            }
            matches
        }
    };

    if candidates.len() == 1 {
        return Ok(candidates[0]);
    }

    if candidates.is_empty() {
        return Err(anyhow!("no open tasks in vault"));
    }

    let stdin_is_tty = is_terminal::IsTerminal::is_terminal(&std::io::stdin());
    if args.yes || !stdin_is_tty {
        let preview: Vec<String> = candidates
            .iter()
            .take(5)
            .map(|t| {
                format!(
                    "  {}:{}  {}",
                    t.source_file.display(),
                    t.source_line,
                    t.description
                )
            })
            .collect();
        let extra = if candidates.len() > 5 {
            format!("\n  … and {} more", candidates.len() - 5)
        } else {
            String::new()
        };
        return Err(anyhow!(
            "{} candidates match — be more specific:\n{}{extra}",
            candidates.len(),
            preview.join("\n")
        ));
    }

    let labels: Vec<String> = candidates
        .iter()
        .map(|t| {
            format!(
                "{}:{}  {}",
                t.source_file.display(),
                t.source_line,
                t.description
            )
        })
        .collect();
    let chosen = dialoguer::FuzzySelect::new()
        .with_prompt("complete which task?")
        .items(&labels)
        .default(0)
        .interact_opt()
        .map_err(|e| anyhow!("picker failed: {e}"))?
        .ok_or_else(|| anyhow!("no task selected"))?;
    Ok(candidates[chosen])
}

fn translate_complete_error(e: CompleteError, vault_root: &std::path::Path) -> anyhow::Error {
    use CompleteError::*;
    match e {
        Read { path, source } => {
            let rel = path.strip_prefix(vault_root).unwrap_or(&path);
            anyhow!("could not read {}: {source}", rel.display())
        }
        LineMissing {
            path,
            line,
            file_lines,
        } => {
            let rel = path.strip_prefix(vault_root).unwrap_or(&path);
            anyhow!(
                "line {line} not found in {} ({file_lines} lines)",
                rel.display()
            )
        }
        NotATask { path, line } => {
            let rel = path.strip_prefix(vault_root).unwrap_or(&path);
            anyhow!("line {line} in {} is not a task", rel.display())
        }
        AlreadyDone { path, line, done } => {
            let rel = path.strip_prefix(vault_root).unwrap_or(&path);
            anyhow!(
                "task at {}:{} is already done (on {done})",
                rel.display(),
                line
            )
        }
        other => anyhow!("{other}"),
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
