//! `ft notes` subcommands — open and section-move.
//!
//! Wraps the pure primitives in [`ft_core::notes`] with the shell/UI
//! concerns: editor spawning, Obsidian URL dispatch, diff preview, and
//! TTY-aware confirmation. Both flows are also reachable from the TUI
//! (plan 003 sessions 3 + 4) via the same library calls.

use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcCommand, ExitCode};

use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use ft_core::markdown::{extract_headings, Heading};
use ft_core::notes::{
    move_sections, obsidian_url as core_obsidian_url, write_pair, Placement, SectionPick,
};
use ft_core::recents::RecentsLog;
use ft_core::search::{fuzzy_find, Query, SearchOptions};
use ft_core::vault::Vault;
use regex::Regex;

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
}

pub fn run(args: NotesArgs, vault_flag: Option<PathBuf>) -> Result<ExitCode> {
    match args.command {
        NotesCommand::Open(o) => run_open(o, vault_flag),
        NotesCommand::MoveSection(m) => run_move_section(m, vault_flag),
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
