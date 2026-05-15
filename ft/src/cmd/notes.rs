//! `ft notes` subcommands — open and section-move.
//!
//! Wraps the pure primitives in [`ft_core::notes`] with the shell/UI
//! concerns: editor spawning, Obsidian URL dispatch, diff preview, and
//! TTY-aware confirmation. Both flows are also reachable from the TUI
//! (plan 003 sessions 3 + 4) via the same library calls.

use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcCommand, ExitCode};
use std::str::FromStr;

use anyhow::{anyhow, Context, Result};
use chrono::{NaiveDate, NaiveDateTime};
use clap::{Args, Subcommand, ValueEnum};
use ft_core::fs::write_atomic;
use ft_core::graph::rename::{apply_rename_plan, plan_rename, RenamePlan};
use ft_core::graph::{Graph, NodeKind, NoteId};
use ft_core::markdown::{extract_headings, Heading};
use ft_core::notes::template::{render as render_template, TemplateContext};
use ft_core::notes::{
    move_sections, obsidian_url as core_obsidian_url, write_pair, Placement, SectionPick,
};
use ft_core::periodic::{create_or_get_periodic_path, Period};
use ft_core::recents::RecentsLog;
use ft_core::search::{fuzzy_find, Query, SearchOptions};
use ft_core::vault::Vault;
use regex::Regex;

use crate::output::links::{
    render_json as render_links_json, render_markdown as render_links_markdown,
    render_ndjson as render_links_ndjson, render_table as render_links_table, Direction, LinkRow,
    TableOpts as LinkTableOpts,
};
use crate::output::Format;

#[derive(Args)]
pub struct NotesArgs {
    #[command(subcommand)]
    pub command: NotesCommand,
}

#[derive(Subcommand)]
pub enum NotesCommand {
    /// Open a note (or a specific heading) in `$EDITOR` or Obsidian.
    Open(OpenArgs),
    /// Move sections from one note into another.
    #[command(name = "move-section")]
    MoveSection(MoveSectionArgs),
    /// Create a new note from a template (or a blank `# <title>` stub).
    Create(CreateArgs),
    /// Open today's daily note (alias for `ft notes periodic daily`).
    Today(TodayArgs),
    /// Open a periodic note (daily/weekly/monthly/quarterly/yearly),
    /// creating it from the configured template if missing.
    Periodic(PeriodicArgs),
    /// List notes that link **to** the given note (incoming edges).
    Backlinks(LinksArgs),
    /// List notes the given note links **to** (outgoing edges,
    /// including unresolved targets).
    Links(LinksArgs),
    /// Rename a note (or unresolved `[[Phantom]]` target) and rewrite
    /// every link in the vault to point at the new name.
    Rename(RenameArgs),
}

pub fn run(args: NotesArgs, vault_flag: Option<PathBuf>) -> Result<ExitCode> {
    match args.command {
        NotesCommand::Open(o) => run_open(o, vault_flag),
        NotesCommand::MoveSection(m) => run_move_section(m, vault_flag),
        NotesCommand::Create(c) => run_create(c, vault_flag),
        NotesCommand::Today(t) => run_today(t, vault_flag),
        NotesCommand::Periodic(p) => run_periodic(p, vault_flag),
        NotesCommand::Backlinks(a) => run_links(a, vault_flag, Direction::Backlinks),
        NotesCommand::Links(a) => run_links(a, vault_flag, Direction::Forward),
        NotesCommand::Rename(a) => run_rename(a, vault_flag),
    }
}

// ── ft notes open ────────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct OpenArgs {
    /// Fuzzy query. Same syntax as `ft find` — `text` matches filenames,
    /// `text#heading` also picks a heading, `#heading` searches every
    /// note. The top hit is opened.
    #[arg(value_name = "QUERY", required = true)]
    pub query: Vec<String>,

    /// Open in Obsidian via the `obsidian://open` URL scheme instead of
    /// `$EDITOR`. Best-effort: the `&heading=` parameter only resolves
    /// to a heading when Obsidian's Advanced URI plugin is installed;
    /// plain Obsidian falls back to opening the file.
    #[arg(long)]
    pub obsidian: bool,

    /// Override `$EDITOR` for this invocation (e.g. `--editor code`).
    #[arg(long, value_name = "BIN")]
    pub editor: Option<String>,

    /// Override the vault basename used in the `obsidian://` URL. The
    /// default is the basename of the vault root, which usually matches
    /// the Obsidian vault registration.
    #[arg(long, value_name = "NAME")]
    pub vault_name: Option<String>,
}

fn run_open(args: OpenArgs, vault_flag: Option<PathBuf>) -> Result<ExitCode> {
    let vault = Vault::discover(vault_flag).context("could not locate an Obsidian vault")?;

    let query_str = args.query.join(" ");
    let query = Query::parse(&query_str);
    if query.is_empty() {
        return Err(anyhow!(
            "query is empty — `ft notes open QUERY` requires a fuzzy pattern"
        ));
    }

    let opts = SearchOptions {
        limit: 1,
        include_headings: true,
    };
    let hits = fuzzy_find(&vault, &query, opts);
    let Some(hit) = hits.into_iter().next() else {
        eprintln!("no match for `{query_str}`");
        return Ok(ExitCode::from(1));
    };

    let abs_path = vault.path.join(&hit.path);
    let heading_line = hit.heading.as_ref().map(|h| h.line).unwrap_or(1);

    // Record the open in the per-vault recents log so the next picker
    // invocation (TUI or CLI) surfaces this note at the top. Best-effort.
    RecentsLog::for_vault(&vault).record_open(&hit.path);

    if args.obsidian {
        // FT_OBSIDIAN_DRY_RUN=1 short-circuits the OS handoff and just
        // prints the URL — keeps integration tests hermetic.
        let url = obsidian_url(
            args.vault_name.as_deref(),
            &vault.path,
            &hit.path,
            hit.heading.as_ref(),
        );
        if std::env::var_os("FT_OBSIDIAN_DRY_RUN").is_some() {
            println!("{url}");
            return Ok(ExitCode::SUCCESS);
        }
        open_url(&url)?;
        println!("{url}");
        return Ok(ExitCode::SUCCESS);
    }

    let editor = resolve_editor(args.editor.as_deref());
    spawn_editor(&editor, &abs_path, heading_line)?;
    Ok(ExitCode::SUCCESS)
}

/// Editor resolution mirrors `tui::app::spawn_editor`: explicit override
/// → `VISUAL` → `EDITOR` → `vi`.
fn resolve_editor(override_: Option<&str>) -> String {
    if let Some(bin) = override_ {
        return bin.to_string();
    }
    if let Ok(v) = std::env::var("VISUAL") {
        if !v.trim().is_empty() {
            return v;
        }
    }
    if let Ok(e) = std::env::var("EDITOR") {
        if !e.trim().is_empty() {
            return e;
        }
    }
    "vi".to_string()
}

/// Spawn the editor against `path`, jumping to `line`. The editor string
/// may contain shell-style space-separated arguments — splitting matches
/// what `tui::app::spawn_editor` does.
fn spawn_editor(editor: &str, path: &Path, line: usize) -> Result<()> {
    let mut parts = editor.split_whitespace();
    let bin = parts
        .next()
        .ok_or_else(|| anyhow!("EDITOR / --editor is empty"))?;
    let extra: Vec<&str> = parts.collect();

    let mut cmd = ProcCommand::new(bin);
    cmd.args(extra).arg(format!("+{line}")).arg(path);
    let status = cmd
        .status()
        .with_context(|| format!("could not run editor `{bin}`"))?;
    if !status.success() {
        return Err(anyhow!("editor `{bin}` exited with status {status}"));
    }
    Ok(())
}

/// Resolve `--vault-name` override → vault-root basename → "vault", then
/// delegate to [`ft_core::notes::obsidian_url`] for the actual URL build.
/// Both CLI and TUI use the core builder; this thin wrapper handles the
/// CLI-only `vault_root` fallback so call sites stay one-liners.
fn obsidian_url(
    vault_name_override: Option<&str>,
    vault_root: &Path,
    rel_path: &Path,
    heading: Option<&Heading>,
) -> String {
    let vault_name = vault_name_override.map(str::to_string).unwrap_or_else(|| {
        vault_root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "vault".to_string())
    });
    core_obsidian_url(&vault_name, rel_path, heading)
}

#[cfg(target_os = "macos")]
fn open_url(url: &str) -> Result<()> {
    let status = ProcCommand::new("open")
        .arg(url)
        .status()
        .with_context(|| format!("could not run `open {url}`"))?;
    if !status.success() {
        return Err(anyhow!("`open` exited with status {status}"));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn open_url(url: &str) -> Result<()> {
    let status = ProcCommand::new("xdg-open")
        .arg(url)
        .status()
        .with_context(|| format!("could not run `xdg-open {url}`"))?;
    if !status.success() {
        return Err(anyhow!("`xdg-open` exited with status {status}"));
    }
    Ok(())
}

// ── ft notes move-section ────────────────────────────────────────────────────

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchPolicy {
    /// Take the first match in document order.
    First,
    /// Take every match.
    All,
    /// Refuse to write when a heading text matches more than once.
    Error,
}

#[derive(Args, Debug)]
pub struct MoveSectionArgs {
    /// Source note (vault-relative or absolute). Required unless
    /// `--from-query` is used.
    #[arg(long, value_name = "PATH")]
    pub from: Option<PathBuf>,

    /// Convenience resolver: pick the source via fuzzy search. Mutually
    /// exclusive with `--from`.
    #[arg(long, value_name = "QUERY", conflicts_with = "from")]
    pub from_query: Option<String>,

    /// Target note (vault-relative or absolute).
    #[arg(long, value_name = "PATH", required = true)]
    pub to: PathBuf,

    /// Exact heading text to move (trimmed, case-insensitive). Repeatable.
    #[arg(long, value_name = "TEXT")]
    pub heading: Vec<String>,

    /// Regex matched against heading text. Repeatable.
    #[arg(long, value_name = "PATTERN")]
    pub heading_regex: Vec<String>,

    /// How to resolve ambiguous heading matches. Defaults to `error`.
    #[arg(long, value_enum, default_value_t = MatchPolicy::Error)]
    pub match_policy: MatchPolicy,

    /// Drop the moved section(s) at this ATX level (1-6). Cascading
    /// nested headings shift by the same delta. Defaults to preserving
    /// the source level.
    #[arg(long, value_name = "N")]
    pub at_level: Option<u8>,

    /// Place the moved section(s) after the named heading in the target.
    /// Uses `--match-policy` for ambiguity. Omit to insert at the top.
    #[arg(long, value_name = "TEXT")]
    pub after: Option<String>,

    /// Skip the interactive confirmation. Required on a non-TTY stdin.
    #[arg(long, short = 'y')]
    pub yes: bool,
}

fn run_move_section(args: MoveSectionArgs, vault_flag: Option<PathBuf>) -> Result<ExitCode> {
    let vault = Vault::discover(vault_flag).context("could not locate an Obsidian vault")?;

    if args.heading.is_empty() && args.heading_regex.is_empty() && args.from_query.is_none() {
        return Err(anyhow!(
            "supply at least one of --heading / --heading-regex / --from-query"
        ));
    }
    if args.from.is_none() && args.from_query.is_none() {
        return Err(anyhow!("--from PATH or --from-query QUERY is required"));
    }

    // Resolve source path and (optionally) a seed heading from --from-query.
    let (source_abs, mut seed_from_query): (PathBuf, Option<Heading>) =
        match (&args.from, &args.from_query) {
            (Some(from), None) => (resolve_under_vault(from, &vault.path), None),
            (None, Some(q)) => {
                let query = Query::parse(q);
                if query.is_empty() {
                    return Err(anyhow!("--from-query is empty"));
                }
                let opts = SearchOptions {
                    limit: 1,
                    include_headings: true,
                };
                let hit = fuzzy_find(&vault, &query, opts)
                    .into_iter()
                    .next()
                    .ok_or_else(|| anyhow!("no match for --from-query `{q}`"))?;
                (vault.path.join(&hit.path), hit.heading.clone())
            }
            (Some(_), Some(_)) => unreachable!("clap conflicts_with prevents both"),
            (None, None) => unreachable!("guard above ensures at least one"),
        };
    let target_abs = resolve_under_vault(&args.to, &vault.path);

    if same_file(&source_abs, &target_abs)? {
        return Err(anyhow!(
            "source and target resolve to the same file ({}) — same-file moves are not yet supported",
            source_abs.display()
        ));
    }

    let source_content = std::fs::read_to_string(&source_abs)
        .with_context(|| format!("could not read source `{}`", source_abs.display()))?;
    let target_content = std::fs::read_to_string(&target_abs)
        .with_context(|| format!("could not read target `{}`", target_abs.display()))?;

    let source_headings = extract_headings(&source_content);

    // Collect candidate heading lines, in document order, with no
    // duplicates. The seed from --from-query is appended as the first
    // explicit match if its text is present and no other selector was
    // given. (When --heading or --heading-regex are also passed, those
    // take precedence and the seed is dropped.)
    if !args.heading.is_empty() || !args.heading_regex.is_empty() {
        seed_from_query = None;
    }

    let mut picked_lines: Vec<usize> = Vec::new();
    for needle in &args.heading {
        let matches = match_headings_by_text(needle, &source_headings);
        let resolved = apply_match_policy(
            &matches,
            args.match_policy,
            &format!("--heading {needle:?}"),
        )?;
        for line in resolved {
            if !picked_lines.contains(&line) {
                picked_lines.push(line);
            }
        }
    }
    for pattern in &args.heading_regex {
        let re =
            Regex::new(pattern).with_context(|| format!("invalid --heading-regex `{pattern}`"))?;
        let matches: Vec<usize> = source_headings
            .iter()
            .filter(|h| re.is_match(&h.text))
            .map(|h| h.line)
            .collect();
        let resolved = apply_match_policy(
            &matches,
            args.match_policy,
            &format!("--heading-regex {pattern:?}"),
        )?;
        for line in resolved {
            if !picked_lines.contains(&line) {
                picked_lines.push(line);
            }
        }
    }
    if let Some(seed) = &seed_from_query {
        if !picked_lines.contains(&seed.line) {
            picked_lines.push(seed.line);
        }
    }

    if picked_lines.is_empty() {
        eprintln!("no source headings matched");
        return Ok(ExitCode::from(1));
    }

    // Sort picks in document order so the source rewrite is stable.
    picked_lines.sort_unstable();

    // Resolve --after (target placement). Missing --after → top of file.
    let after_line: Option<usize> = if let Some(needle) = &args.after {
        let target_headings = extract_headings(&target_content);
        let matches = match_headings_by_text(needle, &target_headings);
        let resolved =
            apply_match_policy(&matches, args.match_policy, &format!("--after {needle:?}"))?;
        let line = *resolved
            .first()
            .ok_or_else(|| anyhow!("--after {needle:?} did not match any heading in the target"))?;
        Some(line)
    } else {
        None
    };

    // Build picks / placements. Every pick shares the same after_line.
    let picks: Vec<SectionPick> = picked_lines
        .iter()
        .map(|&source_line| SectionPick {
            source_line,
            new_level: args.at_level.unwrap_or_else(|| {
                source_headings
                    .iter()
                    .find(|h| h.line == source_line)
                    .map(|h| h.level)
                    .unwrap_or(2)
            }),
            new_text: None,
        })
        .collect();
    let plan: Vec<Placement> = (0..picks.len())
        .map(|idx| Placement {
            pick_idx: idx,
            after_line,
        })
        .collect();

    let (new_source, new_target) = move_sections(&source_content, &picks, &target_content, &plan)
        .map_err(|e| anyhow!("{e}"))?;

    let source_rel = source_abs
        .strip_prefix(&vault.path)
        .unwrap_or(&source_abs)
        .to_path_buf();
    let target_rel = target_abs
        .strip_prefix(&vault.path)
        .unwrap_or(&target_abs)
        .to_path_buf();
    print_diff(&source_rel, &source_content, &new_source);
    print_diff(&target_rel, &target_content, &new_target);

    // Confirm.
    if !args.yes {
        let stdin_is_tty = io::stdin().is_terminal();
        if !stdin_is_tty {
            return Err(anyhow!(
                "non-TTY stdin: pass --yes to apply, or redirect this output through a pager"
            ));
        }
        eprint!("Apply? [y/N] ");
        use std::io::Write;
        io::stderr().flush().ok();
        let mut buf = [0u8; 1];
        let n = io::stdin().read(&mut buf).unwrap_or(0);
        let confirmed = n == 1 && (buf[0] == b'y' || buf[0] == b'Y');
        if !confirmed {
            eprintln!("aborted");
            return Ok(ExitCode::from(2));
        }
    }

    write_pair(&target_abs, &new_target, &source_abs, &new_source).map_err(|e| anyhow!("{e}"))?;

    println!(
        "Moved {} section(s): {} → {}",
        picks.len(),
        source_rel.display(),
        target_rel.display()
    );
    Ok(ExitCode::SUCCESS)
}

/// Resolve `path` against the vault root when relative; otherwise pass
/// through. Trailing canonicalisation is intentionally avoided — the
/// file might not exist yet (e.g. a brand new target).
fn resolve_under_vault(path: &Path, vault_root: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        vault_root.join(path)
    }
}

/// Best-effort same-file check. Falls back to a logical-path comparison
/// when canonicalize fails (a missing target file is common).
fn same_file(a: &Path, b: &Path) -> Result<bool> {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => Ok(ca == cb),
        _ => Ok(a == b),
    }
}

/// Trimmed, case-insensitive heading text match. Returns the line
/// numbers (1-indexed) of every match in document order.
fn match_headings_by_text(needle: &str, headings: &[Heading]) -> Vec<usize> {
    let n = needle.trim().to_lowercase();
    headings
        .iter()
        .filter(|h| h.text.trim().to_lowercase() == n)
        .map(|h| h.line)
        .collect()
}

/// Apply the match policy to a list of candidate lines. Returns the
/// selected lines or an error message that names the ambiguous lines.
fn apply_match_policy(matches: &[usize], policy: MatchPolicy, label: &str) -> Result<Vec<usize>> {
    match matches.len() {
        0 => Ok(Vec::new()),
        1 => Ok(matches.to_vec()),
        _ => match policy {
            MatchPolicy::First => Ok(vec![matches[0]]),
            MatchPolicy::All => Ok(matches.to_vec()),
            MatchPolicy::Error => Err(anyhow!(
                "{label} matched {} headings (lines {}). Pass --match-policy first|all to disambiguate.",
                matches.len(),
                matches
                    .iter()
                    .map(|l| l.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        },
    }
}

fn print_diff(path: &Path, original: &str, new: &str) {
    use similar::{ChangeTag, TextDiff};
    println!("--- {} (before)", path.display());
    println!("+++ {} (after)", path.display());
    let diff = TextDiff::from_lines(original, new);
    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            ChangeTag::Delete => "-",
            ChangeTag::Insert => "+",
            ChangeTag::Equal => " ",
        };
        print!("{sign}{change}");
    }
}

// ── ft notes create ──────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct CreateArgs {
    /// Destination path. Vault-relative or absolute. `.md` is appended
    /// when missing; intermediate folders are created as needed.
    #[arg(value_name = "PATH", required = true)]
    pub path: PathBuf,

    /// Template source. Resolution order:
    /// (1) absolute path used as-is,
    /// (2) path relative to the configured templates folder
    ///     (default `templates-ft/`),
    /// (3) path relative to the current working directory.
    /// `.md` is auto-appended at each step when missing.
    #[arg(long, value_name = "PATH")]
    pub template: Option<PathBuf>,

    /// Override the auto-derived title (the destination basename
    /// without `.md`). Useful when the on-disk filename differs from
    /// the heading text the template should emit.
    #[arg(long, value_name = "TEXT")]
    pub title: Option<String>,

    /// Custom template variable, surfaced as `vars.KEY` inside the
    /// template. Repeatable.
    #[arg(long = "var", value_name = "KEY=VAL", value_parser = parse_var_kv)]
    pub vars: Vec<(String, String)>,

    /// After creating, print (and on macOS, `open`) an
    /// `obsidian://open?vault=...&file=...` URL. `FT_OBSIDIAN_DRY_RUN=1`
    /// suppresses the OS handoff and just prints.
    #[arg(long)]
    pub obsidian: bool,

    /// Suppress the default behavior of opening the new file in `$EDITOR`.
    #[arg(long)]
    pub no_open: bool,

    /// Override `$EDITOR` for this invocation.
    #[arg(long, value_name = "BIN")]
    pub editor: Option<String>,

    /// Overwrite the destination if it already exists. Without `--force`,
    /// a collision exits 2 without touching the file.
    #[arg(long)]
    pub force: bool,

    /// Override the vault basename used in the `obsidian://` URL.
    #[arg(long, value_name = "NAME")]
    pub vault_name: Option<String>,
}

fn parse_var_kv(s: &str) -> std::result::Result<(String, String), String> {
    let (k, v) = s
        .split_once('=')
        .ok_or_else(|| format!("--var expects KEY=VAL, got {s:?} (no '=' found)"))?;
    let key = k.trim();
    if key.is_empty() {
        return Err(format!("--var KEY is empty in {s:?}"));
    }
    Ok((key.to_string(), v.to_string()))
}

fn run_create(args: CreateArgs, vault_flag: Option<PathBuf>) -> Result<ExitCode> {
    let vault = Vault::discover(vault_flag).context("could not locate an Obsidian vault")?;

    // 1. Resolve destination: vault-relative or absolute, append `.md`.
    let abs_dest = resolve_create_dest(&vault.path, &args.path);

    // 2. Collision check (before any rendering / I/O).
    if abs_dest.exists() && !args.force {
        eprintln!(
            "error: destination already exists: {} (pass --force to overwrite or `ft notes open` to edit it)",
            abs_dest.display()
        );
        return Ok(ExitCode::from(2));
    }

    // 3. Derive title.
    let derived_title = abs_dest
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let title = args.title.clone().unwrap_or(derived_title);

    // 4. Resolve template path (if any) and render content.
    let content = match args.template.as_deref() {
        None => format!("# {title}\n"),
        Some(tpl) => {
            let tpl_path = match resolve_template_path(&vault, tpl) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("error: {e}");
                    return Ok(ExitCode::from(2));
                }
            };
            let source = std::fs::read_to_string(&tpl_path)
                .with_context(|| format!("reading template {}", tpl_path.display()))?;
            let ctx = build_template_context(title.clone(), &args.vars);
            match render_template(&source, &ctx) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!(
                        "error: template render failed ({}): {e}",
                        tpl_path.display()
                    );
                    return Ok(ExitCode::from(2));
                }
            }
        }
    };

    // 5. Create intermediate directories.
    if let Some(parent) = abs_dest.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("mkdir -p {}", parent.display()))?;
        }
    }

    // 6. Write atomically.
    write_atomic(&abs_dest, &content).map_err(|e| anyhow!("write {}: {e}", abs_dest.display()))?;

    // 7. Tell the user what happened.
    let rel_for_msg = abs_dest
        .strip_prefix(&vault.path)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| abs_dest.display().to_string());
    eprintln!("created {rel_for_msg}");

    // 8. Post-create handoff: obsidian URL or editor.
    if args.obsidian {
        let rel = abs_dest
            .strip_prefix(&vault.path)
            .unwrap_or(&abs_dest)
            .to_path_buf();
        let url = obsidian_url(args.vault_name.as_deref(), &vault.path, &rel, None);
        if std::env::var_os("FT_OBSIDIAN_DRY_RUN").is_some() {
            println!("{url}");
            return Ok(ExitCode::SUCCESS);
        }
        open_url(&url)?;
        println!("{url}");
        return Ok(ExitCode::SUCCESS);
    }

    if !args.no_open {
        let editor = resolve_editor(args.editor.as_deref());
        spawn_editor(&editor, &abs_dest, 1)?;
    }

    Ok(ExitCode::SUCCESS)
}

fn resolve_create_dest(vault_root: &Path, raw: &Path) -> PathBuf {
    let with_ext = if raw.extension().is_some_and(|e| e == "md") {
        raw.to_path_buf()
    } else {
        let mut p = raw.as_os_str().to_owned();
        p.push(".md");
        PathBuf::from(p)
    };
    if with_ext.is_absolute() {
        with_ext
    } else {
        vault_root.join(with_ext)
    }
}

/// Resolve a `--template` argument to an absolute path.
///
/// Tries: (1) absolute as-is, (2) `<vault>/<templates_dir>/<arg>`,
/// (3) CWD/<arg>. At each step, also tries the variant with `.md`
/// appended. Errors with a clear message listing the attempted paths
/// when none exist.
fn resolve_template_path(vault: &Vault, arg: &Path) -> Result<PathBuf> {
    let mut attempts: Vec<PathBuf> = Vec::new();

    let try_candidate = |candidate: PathBuf, out: &mut Vec<PathBuf>| -> Option<PathBuf> {
        if candidate.is_file() {
            return Some(candidate);
        }
        out.push(candidate.clone());
        if candidate.extension().is_none() {
            let mut with_ext = candidate.clone().into_os_string();
            with_ext.push(".md");
            let cand = PathBuf::from(with_ext);
            if cand.is_file() {
                return Some(cand);
            }
            out.push(cand);
        }
        None
    };

    if arg.is_absolute() {
        if let Some(p) = try_candidate(arg.to_path_buf(), &mut attempts) {
            return Ok(p);
        }
    } else {
        if let Some(p) = try_candidate(vault.templates_dir().join(arg), &mut attempts) {
            return Ok(p);
        }
        if let Some(p) = try_candidate(
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(arg),
            &mut attempts,
        ) {
            return Ok(p);
        }
    }

    Err(anyhow!(
        "template not found: {}\ntried:\n  {}",
        arg.display(),
        attempts
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n  ")
    ))
}

fn build_template_context(title: String, vars: &[(String, String)]) -> TemplateContext {
    let (today, now) = today_now_from_env();
    let mut ctx = TemplateContext::new(title, today, now);
    for (k, v) in vars {
        ctx.vars.insert(k.clone(), v.clone());
    }
    ctx
}

/// Resolve the `(today, now)` pair for template rendering. Honors the
/// `FT_TODAY=YYYY-MM-DD` override (used by integration tests and pinned
/// runs); otherwise reads the local wall clock.
fn today_now_from_env() -> (NaiveDate, NaiveDateTime) {
    use chrono::{Local, NaiveTime};
    if let Ok(s) = std::env::var("FT_TODAY") {
        if let Ok(d) = NaiveDate::parse_from_str(&s, "%Y-%m-%d") {
            return (d, d.and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap()));
        }
    }
    let local = Local::now();
    (local.date_naive(), local.naive_local())
}

// ── ft notes periodic / ft notes today ───────────────────────────────────────

#[derive(Args, Debug)]
pub struct PeriodicArgs {
    /// One of `daily | weekly | monthly | quarterly | yearly` (also
    /// accepts the short forms `d | w | m | q | y`).
    #[arg(value_name = "PERIOD", required = true)]
    pub period: String,

    /// Target date in `YYYY-MM-DD`. Defaults to `FT_TODAY` (when set) or
    /// today's local date. `--offset` is applied on top of this date.
    #[arg(long, value_name = "YYYY-MM-DD")]
    pub date: Option<String>,

    /// Shift the target date by N period units. `--offset -1` on
    /// `weekly` is "last week"; `--offset 1` on `monthly --date
    /// 2026-01-31` resolves to Feb 28/29 (month-end clamp).
    #[arg(
        long,
        value_name = "N",
        default_value_t = 0,
        allow_negative_numbers = true
    )]
    pub offset: i32,

    /// Suppress the default behavior of opening the note in `$EDITOR`.
    #[arg(long)]
    pub no_open: bool,

    /// Open via the `obsidian://open` URL scheme instead of `$EDITOR`.
    /// `FT_OBSIDIAN_DRY_RUN=1` suppresses the OS handoff and just prints.
    #[arg(long)]
    pub obsidian: bool,

    /// Override `$EDITOR` for this invocation.
    #[arg(long, value_name = "BIN")]
    pub editor: Option<String>,

    /// Override the vault basename used in the `obsidian://` URL.
    #[arg(long, value_name = "NAME")]
    pub vault_name: Option<String>,
}

#[derive(Args, Debug)]
pub struct TodayArgs {
    /// Target date in `YYYY-MM-DD`. Defaults to `FT_TODAY` (when set) or
    /// today's local date.
    #[arg(long, value_name = "YYYY-MM-DD")]
    pub date: Option<String>,

    /// Suppress the default behavior of opening the note in `$EDITOR`.
    #[arg(long)]
    pub no_open: bool,

    /// Open via the `obsidian://open` URL scheme instead of `$EDITOR`.
    #[arg(long)]
    pub obsidian: bool,

    /// Override `$EDITOR` for this invocation.
    #[arg(long, value_name = "BIN")]
    pub editor: Option<String>,

    /// Override the vault basename used in the `obsidian://` URL.
    #[arg(long, value_name = "NAME")]
    pub vault_name: Option<String>,
}

fn run_periodic(args: PeriodicArgs, vault_flag: Option<PathBuf>) -> Result<ExitCode> {
    let period = match Period::from_str(&args.period) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("error: {msg}");
            return Ok(ExitCode::from(2));
        }
    };
    run_periodic_inner(
        vault_flag,
        period,
        args.date.as_deref(),
        args.offset,
        args.no_open,
        args.obsidian,
        args.editor.as_deref(),
        args.vault_name.as_deref(),
    )
}

fn run_today(args: TodayArgs, vault_flag: Option<PathBuf>) -> Result<ExitCode> {
    run_periodic_inner(
        vault_flag,
        Period::Daily,
        args.date.as_deref(),
        0,
        args.no_open,
        args.obsidian,
        args.editor.as_deref(),
        args.vault_name.as_deref(),
    )
}

#[allow(clippy::too_many_arguments)]
fn run_periodic_inner(
    vault_flag: Option<PathBuf>,
    period: Period,
    date_override: Option<&str>,
    offset: i32,
    no_open: bool,
    obsidian: bool,
    editor: Option<&str>,
    vault_name: Option<&str>,
) -> Result<ExitCode> {
    let vault = Vault::discover(vault_flag).context("could not locate an Obsidian vault")?;

    // 1. Pull the per-period config — exit 2 with a hint when missing.
    let cfg_opt = match period {
        Period::Daily => vault.config.config.periodic_notes.daily.as_ref(),
        Period::Weekly => vault.config.config.periodic_notes.weekly.as_ref(),
        Period::Monthly => vault.config.config.periodic_notes.monthly.as_ref(),
        Period::Quarterly => vault.config.config.periodic_notes.quarterly.as_ref(),
        Period::Yearly => vault.config.config.periodic_notes.yearly.as_ref(),
    };
    let Some(cfg) = cfg_opt else {
        eprintln!(
            "error: {period} not configured — add `[periodic_notes.{period}]` to your config",
            period = period.as_str()
        );
        return Ok(ExitCode::from(2));
    };

    // 2. Resolve invocation `today`/`now` (FT_TODAY-aware).
    let (today, now) = today_now_from_env();

    // 3. Target date: --date if given, else `today`; then shift by --offset.
    let base_date = match date_override {
        Some(s) => match NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            Ok(d) => d,
            Err(_) => {
                eprintln!("error: --date must be YYYY-MM-DD, got {s:?}");
                return Ok(ExitCode::from(2));
            }
        },
        None => today,
    };
    let Some(target_date) = period.offset_date(base_date, offset) else {
        eprintln!(
            "error: --offset {offset} on {} overflows the representable date range",
            period.as_str()
        );
        return Ok(ExitCode::from(2));
    };

    // 4. Create-or-get the note. Errors here (template render, write
    //    failure) surface as exit 2 with the library's user-readable
    //    message.
    let (abs_path, created) = match create_or_get_periodic_path(
        &vault.path,
        &vault.templates_dir(),
        cfg,
        target_date,
        today,
        now,
    ) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("error: {e}");
            return Ok(ExitCode::from(2));
        }
    };

    let rel = abs_path
        .strip_prefix(&vault.path)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| abs_path.display().to_string());

    let verb = if created { "Created" } else { "Opened" };
    println!("{verb} {rel}");

    // 5. Post-create handoff: obsidian URL or editor (skipped under --no-open).
    if obsidian {
        let rel_p = abs_path
            .strip_prefix(&vault.path)
            .unwrap_or(&abs_path)
            .to_path_buf();
        let url = obsidian_url(vault_name, &vault.path, &rel_p, None);
        if std::env::var_os("FT_OBSIDIAN_DRY_RUN").is_some() {
            println!("{url}");
            return Ok(ExitCode::SUCCESS);
        }
        open_url(&url)?;
        println!("{url}");
        return Ok(ExitCode::SUCCESS);
    }

    if !no_open {
        let editor_bin = resolve_editor(editor);
        spawn_editor(&editor_bin, &abs_path, 1)?;
    }

    Ok(ExitCode::SUCCESS)
}

// ── ft notes backlinks / links ───────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct LinksArgs {
    /// Note to query. Vault-relative path (e.g. `Areas/finance.md`),
    /// bare title (e.g. `finance` — falls back to fuzzy search), or
    /// fuzzy query when nothing exact matches.
    #[arg(value_name = "NOTE", required = true)]
    pub note: Vec<String>,

    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Table)]
    pub format: Format,

    /// Disable colored output (also honored: `NO_COLOR` env var).
    #[arg(long)]
    pub no_color: bool,

    /// Treat an empty result set as a successful run. Default: exit 1
    /// when there are no edges to show.
    #[arg(long)]
    pub allow_empty: bool,
}

fn run_links(args: LinksArgs, vault_flag: Option<PathBuf>, dir: Direction) -> Result<ExitCode> {
    let vault = Vault::discover(vault_flag).context("could not locate an Obsidian vault")?;
    let graph = Graph::build(&vault).context("building note graph")?;

    let query = args.note.join(" ");
    let id = resolve_note_query(&graph, &vault, &query)?;
    let queried_path = match graph.node(id) {
        NodeKind::Note(n) => n.path.clone(),
        // resolve_note_query never returns a ghost id from CLI input.
        NodeKind::Ghost(_) => unreachable!("ghost nodes are not selectable from the CLI yet"),
    };

    let rows: Vec<LinkRow> = match dir {
        Direction::Backlinks => {
            let mut rows: Vec<LinkRow> = graph
                .incoming(id)
                .map(|(src, edge)| LinkRow::from_incoming(&graph, src, &queried_path, edge))
                .collect();
            // Stable order: linker path, then line.
            rows.sort_by(|a, b| a.src.cmp(&b.src).then_with(|| a.src_line.cmp(&b.src_line)));
            rows
        }
        Direction::Forward => {
            let mut rows: Vec<LinkRow> = graph
                .outgoing(id)
                .map(|(dst, edge)| LinkRow::from_outgoing(&graph, &queried_path, dst, edge))
                .collect();
            // Outgoing edges are already in document order; sort by
            // (line, raw) for determinism in the face of multiple links
            // on the same line.
            rows.sort_by(|a, b| a.src_line.cmp(&b.src_line).then_with(|| a.raw.cmp(&b.raw)));
            rows
        }
    };

    let exit = if rows.is_empty() && !args.allow_empty {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    };

    match args.format {
        Format::Table => {
            let use_color = !args.no_color
                && std::env::var_os("NO_COLOR").is_none()
                && io::stdout().is_terminal();
            let opts = LinkTableOpts {
                use_color,
                direction: dir,
            };
            if rows.is_empty() {
                let msg = match dir {
                    Direction::Backlinks => "no backlinks",
                    Direction::Forward => "no outgoing links",
                };
                println!("{msg}");
            } else {
                let out = render_links_table(&rows, opts);
                println!("{out}");
            }
        }
        Format::Json => render_links_json(&rows)?,
        Format::Ndjson => render_links_ndjson(&rows)?,
        Format::Markdown => print!("{}", render_links_markdown(&rows)),
    }

    Ok(exit)
}

/// Resolve a `<note>` argument to a [`NoteId`] in the graph.
///
/// Order of attempts:
/// 1. **Exact vault-relative path** (with `.md` auto-appended if missing).
/// 2. **Title** lookup via the graph's `title_index`. When multiple
///    titles match, this defers to the parser/resolver tiebreak — i.e.
///    pick the shortest path; the message lists all candidates if you
///    want to disambiguate by passing the path directly.
/// 3. **Fuzzy** match via `fuzzy_find` against the vault, taking the
///    top hit (matches `ft notes open`'s ergonomics).
fn resolve_note_query(graph: &Graph, vault: &Vault, query: &str) -> Result<NoteId> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Err(anyhow!(
            "<note> is empty — pass a path, title, or fuzzy query"
        ));
    }

    // 1. Exact path with optional `.md`.
    let with_md = if std::path::Path::new(trimmed)
        .extension()
        .is_some_and(|e| e == "md")
    {
        PathBuf::from(trimmed)
    } else {
        PathBuf::from(format!("{trimmed}.md"))
    };
    if let Some(id) = graph
        .note_by_path(std::path::Path::new(trimmed))
        .or_else(|| graph.note_by_path(&with_md))
    {
        return Ok(id);
    }

    // 2. Title (filename stem) — pick the shortest path on collision.
    // Strip `.md` for the title lookup so `foo.md` and `foo` both work.
    let title = std::path::Path::new(trimmed)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| trimmed.to_string());
    let candidates = graph.note_by_title(&title);
    if let Some(&id) = candidates.first() {
        // If the user passed an unambiguous title, pick directly. With
        // multiple, take the shortest path / alphabetical winner per
        // the Obsidian convention — same code path as wikilink resolution.
        if candidates.len() == 1 {
            return Ok(id);
        }
        let best = candidates
            .iter()
            .min_by(|&&a, &&b| {
                let pa = match graph.node(a) {
                    NodeKind::Note(n) => n.path.clone(),
                    _ => PathBuf::new(),
                };
                let pb = match graph.node(b) {
                    NodeKind::Note(n) => n.path.clone(),
                    _ => PathBuf::new(),
                };
                pa.components()
                    .count()
                    .cmp(&pb.components().count())
                    .then_with(|| pa.cmp(&pb))
            })
            .copied()
            .unwrap();
        return Ok(best);
    }

    // 3. Fuzzy fallback.
    let q = Query::parse(trimmed);
    if !q.is_empty() {
        let opts = SearchOptions {
            limit: 1,
            include_headings: false,
        };
        if let Some(hit) = fuzzy_find(vault, &q, opts).into_iter().next() {
            if let Some(id) = graph.note_by_path(&hit.path) {
                return Ok(id);
            }
        }
    }

    Err(anyhow!(
        "no note found for `{trimmed}` (tried path, title, and fuzzy match)"
    ))
}

// ── ft notes rename ──────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct RenameArgs {
    /// Note to rename. Vault-relative path (e.g. `Areas/finance.md`),
    /// bare title (`finance`), fuzzy query, or — for an unresolved
    /// link target — the explicit `[[Phantom]]` form.
    #[arg(value_name = "NOTE", required = true)]
    pub note: String,

    /// New name or path. `mv` ergonomics:
    ///
    /// - bare name (no `/`): keep the same directory, swap the stem.
    /// - path with `/`: vault-relative full target path.
    ///
    /// `.md` is appended automatically when missing.
    #[arg(value_name = "NEW", required = true)]
    pub new: String,

    /// Print the plan and exit without writing anything.
    #[arg(long)]
    pub dry_run: bool,
}

fn run_rename(args: RenameArgs, vault_flag: Option<PathBuf>) -> Result<ExitCode> {
    let vault = Vault::discover(vault_flag).context("could not locate an Obsidian vault")?;
    let graph = Graph::build(&vault).context("building note graph")?;

    let id = resolve_rename_source(&graph, &vault, &args.note)?;

    // Determine the source's current directory for `mv`-style ergonomics.
    let source_rel: Option<PathBuf> = match graph.node(id) {
        NodeKind::Note(n) => Some(n.path.clone()),
        NodeKind::Ghost(_) => None,
    };

    let new_path = parse_new_path(&args.new, source_rel.as_deref())?;

    let plan = plan_rename(&graph, &vault.path, id, &new_path).map_err(|e| anyhow!("{e}"))?;

    if args.dry_run {
        print_rename_plan_summary(&plan, source_rel.as_deref(), &new_path);
        return Ok(ExitCode::SUCCESS);
    }

    apply_rename_plan(&vault.path, &plan).map_err(|e| anyhow!("{e}"))?;

    let edit_files = plan
        .edits
        .iter()
        .map(|e| e.path.as_path())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let edit_count = plan.edits.len();
    match (&plan.rename, source_rel.as_deref()) {
        (Some(r), _) => println!(
            "renamed {} → {}, updated {} link(s) in {} file(s)",
            r.from.display(),
            r.to.display(),
            edit_count,
            edit_files
        ),
        (None, _) => println!(
            "rewrote {} ghost link(s) in {} file(s) — pass `ft notes create {}` to create the new file",
            edit_count,
            edit_files,
            new_path.display()
        ),
    }
    Ok(ExitCode::SUCCESS)
}

/// Resolve `<note>` for rename. Same precedence as `resolve_note_query`
/// (path → title → fuzzy), with one extra path: a literal `[[Phantom]]`
/// form selects the matching ghost node by its raw target string.
fn resolve_rename_source(graph: &Graph, vault: &Vault, query: &str) -> Result<NoteId> {
    let trimmed = query.trim();
    if let Some(stripped) = trimmed
        .strip_prefix("[[")
        .and_then(|s| s.strip_suffix("]]"))
    {
        let raw = stripped.trim();
        if raw.is_empty() {
            return Err(anyhow!("[[ ]] selector is empty"));
        }
        return graph
            .ghost_by_raw(raw)
            .ok_or_else(|| anyhow!("no ghost node found for `{raw}` (is anyone linking to it?)"));
    }
    resolve_note_query(graph, vault, trimmed)
}

/// Translate the user's `<new>` arg into a vault-relative target path.
/// Rules: bare name (no `/`) inherits `source_rel`'s directory; path
/// with `/` is vault-relative; `.md` is appended when missing.
fn parse_new_path(raw: &str, source_rel: Option<&Path>) -> Result<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("<new> is empty"));
    }
    let with_md = if std::path::Path::new(trimmed)
        .extension()
        .is_some_and(|e| e == "md")
    {
        PathBuf::from(trimmed)
    } else {
        PathBuf::from(format!("{trimmed}.md"))
    };
    let has_slash = trimmed.contains('/');
    if has_slash {
        Ok(with_md)
    } else if let Some(src) = source_rel {
        let dir = src.parent().unwrap_or_else(|| Path::new(""));
        Ok(dir.join(with_md))
    } else {
        // Ghost rename, bare name → vault root.
        Ok(with_md)
    }
}

fn print_rename_plan_summary(plan: &RenamePlan, source_rel: Option<&Path>, new_path: &Path) {
    match (&plan.rename, source_rel) {
        (Some(r), _) => println!("would rename: {} → {}", r.from.display(), r.to.display()),
        (None, _) => println!(
            "would rewrite ghost links to point at: {}",
            new_path.display()
        ),
    }
    let mut by_file: std::collections::BTreeMap<&Path, usize> = std::collections::BTreeMap::new();
    for edit in &plan.edits {
        *by_file.entry(edit.path.as_path()).or_default() += 1;
    }
    if by_file.is_empty() {
        println!("no link rewrites needed");
    } else {
        println!(
            "would update {} link(s) in {} file(s):",
            plan.edits.len(),
            by_file.len()
        );
        for (path, n) in by_file {
            println!("  {} ({n} edit(s))", path.display());
        }
    }
}

// ── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_editor_prefers_override() {
        std::env::set_var("VISUAL", "vim");
        std::env::set_var("EDITOR", "nano");
        assert_eq!(resolve_editor(Some("code")), "code");
        // env order is checked but kept best-effort given test parallelism.
    }

    #[test]
    fn obsidian_url_falls_back_to_vault_basename() {
        // Encoding paths are tested in `ft_core::notes::obsidian_url`. This
        // wrapper-only behavior is the `Option<&str>` fallback chain:
        // override → vault_root.basename → "vault".
        let url_override = obsidian_url(
            Some("Override"),
            Path::new("/tmp/IgnoredBase"),
            Path::new("a.md"),
            None,
        );
        assert!(url_override.contains("vault=Override"));

        let url_basename = obsidian_url(None, Path::new("/tmp/My Vault"), Path::new("a.md"), None);
        assert!(url_basename.contains("vault=My%20Vault"));
    }

    #[test]
    fn match_policy_error_lists_lines() {
        let err =
            apply_match_policy(&[2, 5], MatchPolicy::Error, "--heading \"Notes\"").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("matched 2 headings"));
        assert!(msg.contains("lines 2, 5"));
    }

    #[test]
    fn match_policy_first_takes_first() {
        let out = apply_match_policy(&[2, 5], MatchPolicy::First, "label").unwrap();
        assert_eq!(out, vec![2]);
    }

    #[test]
    fn match_policy_all_takes_all() {
        let out = apply_match_policy(&[2, 5], MatchPolicy::All, "label").unwrap();
        assert_eq!(out, vec![2, 5]);
    }
}
