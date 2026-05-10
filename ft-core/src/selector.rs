//! Selectors for picking a single task out of a vault scan.
//!
//! Three forms:
//! - [`Selector::Id`] — bare task id (matches the `🆔` field exactly)
//! - [`Selector::FileLine`] — `<path>:<line>` (relative path + 1-indexed line)
//! - [`Selector::Fuzzy`] — anything else; case-insensitive substring match
//!   against either description or path
//!
//! The CLI parses the user's argument with [`parse`], resolves matches with
//! [`resolve`], and (in the binary) prompts the user to disambiguate when
//! more than one task matches.

use std::path::PathBuf;

use crate::task::Task;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    Id(String),
    FileLine { file: PathBuf, line: usize },
    Fuzzy(String),
}

/// Heuristically classify a user-supplied selector string. Order:
/// 1. `<path>:<line>` if the suffix after the last `:` is a positive integer
/// 2. simple-token id (no whitespace, only alphanumerics / `_` / `-`)
/// 3. fuzzy substring otherwise
pub fn parse(s: &str) -> Selector {
    if let Some((file, line_str)) = s.rsplit_once(':') {
        if let Ok(line) = line_str.parse::<usize>() {
            if line > 0 && !file.is_empty() {
                return Selector::FileLine {
                    file: PathBuf::from(file),
                    line,
                };
            }
        }
    }
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return Selector::Id(s.to_string());
    }
    Selector::Fuzzy(s.to_string())
}

/// Find every task in `tasks` matching `selector`. Resolution rules:
/// - `Id` — exact match against `Task.id`.
/// - `FileLine` — exact match against `(source_file, source_line)`.
/// - `Fuzzy` — case-insensitive substring match against description or source
///   path; restricted to non-`Done` tasks (you don't normally want to
///   "complete" a done task by fuzzy match).
pub fn resolve<'a>(tasks: &'a [Task], selector: &Selector) -> Vec<&'a Task> {
    match selector {
        Selector::Id(id) => tasks
            .iter()
            .filter(|t| t.id.as_deref() == Some(id.as_str()))
            .collect(),
        Selector::FileLine { file, line } => tasks
            .iter()
            .filter(|t| t.source_line == *line && path_matches(&t.source_file, file))
            .collect(),
        Selector::Fuzzy(needle) => {
            let needle_lc = needle.to_ascii_lowercase();
            tasks
                .iter()
                .filter(|t| !matches!(t.status, crate::task::Status::Done))
                .filter(|t| {
                    t.description.to_ascii_lowercase().contains(&needle_lc)
                        || t.source_file
                            .to_string_lossy()
                            .to_ascii_lowercase()
                            .contains(&needle_lc)
                })
                .collect()
        }
    }
}

/// `file:line` selectors should match either an exact relative path or a
/// suffix of one (so `inbox.md:5` matches `notes/inbox.md:5`).
fn path_matches(actual: &std::path::Path, query: &std::path::Path) -> bool {
    if actual == query {
        return true;
    }
    let a = actual.to_string_lossy();
    let q = query.to_string_lossy();
    a == q || a.ends_with(q.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::Status;

    fn task(id: Option<&str>, file: &str, line: usize, desc: &str, status: Status) -> Task {
        Task {
            description: desc.into(),
            status,
            priority: None,
            tags: vec![],
            created: None,
            start: None,
            scheduled: None,
            due: None,
            done: None,
            cancelled: None,
            recurrence: None,
            id: id.map(str::to_string),
            depends_on: vec![],
            on_completion: None,
            block_link: None,
            raw_trailing: None,
            source_file: PathBuf::from(file),
            source_line: line,
            indent_level: 0,
            parent: None,
        }
    }

    // ── parse ─────────────────────────────────────────────────────────────────

    #[test]
    fn parse_file_colon_line() {
        assert_eq!(
            parse("notes/inbox.md:5"),
            Selector::FileLine {
                file: PathBuf::from("notes/inbox.md"),
                line: 5,
            }
        );
    }

    #[test]
    fn parse_bare_id() {
        assert_eq!(parse("abc123"), Selector::Id("abc123".into()));
        assert_eq!(parse("foo-bar_42"), Selector::Id("foo-bar_42".into()));
    }

    #[test]
    fn parse_fuzzy_for_text_with_space() {
        assert_eq!(parse("buy milk"), Selector::Fuzzy("buy milk".into()));
    }

    #[test]
    fn parse_fuzzy_when_colon_suffix_not_numeric() {
        assert_eq!(
            parse("notes/inbox.md:later"),
            Selector::Fuzzy("notes/inbox.md:later".into())
        );
    }

    #[test]
    fn parse_zero_line_falls_to_id_or_fuzzy() {
        // `:0` is not a valid line — we treat the whole string as fuzzy.
        assert_eq!(parse("foo:0"), Selector::Fuzzy("foo:0".into()));
    }

    // ── resolve ───────────────────────────────────────────────────────────────

    #[test]
    fn resolve_id_unique() {
        let tasks = vec![
            task(Some("abc"), "a.md", 1, "first", Status::Open),
            task(Some("xyz"), "b.md", 2, "second", Status::Open),
        ];
        let m = resolve(&tasks, &Selector::Id("xyz".into()));
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].description, "second");
    }

    #[test]
    fn resolve_id_no_match() {
        let tasks = vec![task(Some("abc"), "a.md", 1, "first", Status::Open)];
        let m = resolve(&tasks, &Selector::Id("nope".into()));
        assert!(m.is_empty());
    }

    #[test]
    fn resolve_file_line_exact() {
        let tasks = vec![
            task(None, "notes/inbox.md", 3, "x", Status::Open),
            task(None, "notes/inbox.md", 5, "y", Status::Open),
        ];
        let m = resolve(
            &tasks,
            &Selector::FileLine {
                file: PathBuf::from("notes/inbox.md"),
                line: 5,
            },
        );
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].description, "y");
    }

    #[test]
    fn resolve_file_line_suffix_match() {
        let tasks = vec![task(None, "notes/inbox.md", 5, "y", Status::Open)];
        let m = resolve(
            &tasks,
            &Selector::FileLine {
                file: PathBuf::from("inbox.md"),
                line: 5,
            },
        );
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn resolve_fuzzy_matches_description() {
        let tasks = vec![
            task(None, "a.md", 1, "Buy milk", Status::Open),
            task(None, "a.md", 2, "Walk dog", Status::Open),
        ];
        let m = resolve(&tasks, &Selector::Fuzzy("milk".into()));
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].description, "Buy milk");
    }

    #[test]
    fn resolve_fuzzy_skips_done() {
        let tasks = vec![
            task(None, "a.md", 1, "Buy milk", Status::Done),
            task(None, "a.md", 2, "Buy milk again", Status::Open),
        ];
        let m = resolve(&tasks, &Selector::Fuzzy("milk".into()));
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].description, "Buy milk again");
    }

    #[test]
    fn resolve_fuzzy_matches_path() {
        let tasks = vec![
            task(None, "projects/alpha.md", 1, "review", Status::Open),
            task(None, "projects/beta.md", 1, "review", Status::Open),
        ];
        let m = resolve(&tasks, &Selector::Fuzzy("alpha".into()));
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].source_file, PathBuf::from("projects/alpha.md"));
    }
}
