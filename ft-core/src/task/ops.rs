//! High-level task mutation primitives. Each entry point reads a file,
//! computes the new content, and writes atomically via `crate::fs::write_atomic`.

use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use thiserror::Error;

use super::{
    emoji::EmojiFormat,
    format::{ParseContext, TaskFormat},
    recurrence::{self, RecurrenceError},
    Priority, Status, Task,
};
use crate::fs::write_atomic;

#[derive(Debug, Error)]
pub enum CreateError {
    #[error("could not read {}: {source}", .path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "duplicate task already exists at {}:{} (use --force to insert anyway)",
        .path.display(),
        .line
    )]
    Duplicate { path: PathBuf, line: usize },
    #[error("invalid --at-line {line}: file has only {file_lines} lines")]
    LineOutOfRange { line: usize, file_lines: usize },
    #[error("write failed: {source}")]
    Write {
        #[from]
        source: crate::error::Error,
    },
}

/// Where to insert a new task within the target file.
#[derive(Debug, Clone)]
pub enum Position {
    /// Append at the end of the file (creating it if missing).
    Append,
    /// Insert at the end of the section under the given heading. The heading
    /// match is exact on the heading text (without `#` markers). If the
    /// heading is missing, it is created at the end of the file.
    UnderHeading(String),
    /// Insert at this 1-indexed line, pushing existing content down.
    AtLine(usize),
}

/// User-provided fields for a new task. `description` should contain only
/// the user's free text — `tags` are appended automatically.
#[derive(Debug, Clone, Default)]
pub struct CreateInput {
    pub description: String,
    pub status: Status,
    pub priority: Option<Priority>,
    pub tags: Vec<String>,
    pub created: Option<NaiveDate>,
    pub start: Option<NaiveDate>,
    pub scheduled: Option<NaiveDate>,
    pub due: Option<NaiveDate>,
    pub recurrence: Option<String>,
    pub id: Option<String>,
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CreateOptions {
    pub position: Position,
    /// If true, skip the duplicate check.
    pub force: bool,
}

#[derive(Debug)]
pub struct CreateOutcome {
    /// 1-indexed line number where the new task ended up.
    pub line: usize,
    /// The serialized task line (without trailing newline).
    pub serialized: String,
}

/// Build a `Task` value suitable for serialization from user input.
fn build_task(input: &CreateInput) -> Task {
    let mut description = input.description.trim_end().to_string();
    for tag in &input.tags {
        let bare = tag.trim_start_matches('#');
        let needle = format!("#{bare}");
        if !description.split_whitespace().any(|w| w == needle) {
            if !description.is_empty() {
                description.push(' ');
            }
            description.push_str(&needle);
        }
    }

    let tags = super::emoji::extract_tags(&description);

    Task {
        description,
        status: input.status,
        priority: input.priority,
        tags,
        created: input.created,
        start: input.start,
        scheduled: input.scheduled,
        due: input.due,
        done: None,
        cancelled: None,
        recurrence: input.recurrence.clone(),
        id: input.id.clone(),
        depends_on: input.depends_on.clone(),
        on_completion: None,
        block_link: None,
        raw_trailing: None,
        source_file: PathBuf::new(),
        source_line: 0,
        indent_level: 0,
        parent: None,
    }
}

/// Create a new task in `target_path`. The path must be absolute (the binary
/// resolves it against the vault root before calling).
pub fn create_task(
    target_path: &Path,
    input: CreateInput,
    opts: CreateOptions,
) -> Result<CreateOutcome, CreateError> {
    let task = build_task(&input);
    let serialized = EmojiFormat.serialize_line(&task);

    let existing = read_or_empty(target_path)?;

    if !opts.force {
        if let Some(line) = find_duplicate(&existing, &task) {
            return Err(CreateError::Duplicate {
                path: target_path.to_path_buf(),
                line,
            });
        }
    }

    let (new_content, line) = splice(&existing, &serialized, &opts.position)?;

    write_atomic(target_path, &new_content)?;
    Ok(CreateOutcome { line, serialized })
}

fn read_or_empty(path: &Path) -> Result<String, CreateError> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(CreateError::Read {
            path: path.to_path_buf(),
            source: e,
        }),
    }
}

/// Returns the 1-indexed line number of any existing task whose description,
/// due, scheduled, and start dates all match `task`. The status is ignored
/// (a done duplicate is still a duplicate).
fn find_duplicate(content: &str, task: &Task) -> Option<usize> {
    for (idx, line) in content.lines().enumerate() {
        let ctx = ParseContext {
            source_file: PathBuf::new(),
            source_line: idx + 1,
        };
        if let Some(existing) = EmojiFormat.parse_line(line, ctx) {
            if existing.description == task.description
                && existing.due == task.due
                && existing.scheduled == task.scheduled
                && existing.start == task.start
            {
                return Some(idx + 1);
            }
        }
    }
    None
}

/// Insert `line` into `content` according to `pos`. Returns the new content
/// (always ending in `\n`) and the 1-indexed line number where `line` ended up.
fn splice(content: &str, line: &str, pos: &Position) -> Result<(String, usize), CreateError> {
    let mut lines: Vec<String> = if content.is_empty() {
        Vec::new()
    } else {
        content
            .split_inclusive('\n')
            .map(|s| s.trim_end_matches('\n').trim_end_matches('\r').to_string())
            .collect()
    };

    let inserted_at_idx = match pos {
        Position::Append => {
            lines.push(line.to_string());
            lines.len() - 1
        }
        Position::AtLine(n) => {
            let n = *n;
            if n == 0 || n > lines.len() + 1 {
                return Err(CreateError::LineOutOfRange {
                    line: n,
                    file_lines: lines.len(),
                });
            }
            lines.insert(n - 1, line.to_string());
            n - 1
        }
        Position::UnderHeading(heading) => match find_heading(&lines, heading) {
            Some((heading_idx, level)) => {
                let insert_at = section_end(&lines, heading_idx, level);
                lines.insert(insert_at, line.to_string());
                insert_at
            }
            None => {
                if !lines.is_empty() && !lines.last().unwrap().is_empty() {
                    lines.push(String::new());
                }
                lines.push(format!("## {heading}"));
                lines.push(line.to_string());
                lines.len() - 1
            }
        },
    };

    let mut joined = lines.join("\n");
    joined.push('\n');
    Ok((joined, inserted_at_idx + 1))
}

/// Find a heading by exact text match. Returns `(index, level)` where
/// `level` is the number of leading `#` characters.
fn find_heading(lines: &[String], target: &str) -> Option<(usize, usize)> {
    for (i, l) in lines.iter().enumerate() {
        if let Some((level, text)) = parse_heading(l) {
            if text == target {
                return Some((i, level));
            }
        }
    }
    None
}

fn parse_heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let hashes = trimmed.bytes().take_while(|b| *b == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let after = &trimmed[hashes..];
    let after = after.strip_prefix(' ')?;
    Some((hashes, after.trim_end()))
}

/// Find the index *just after* the last line of the section opened by
/// `heading_idx` at `level`. The section ends at the next heading whose
/// level is `<= level`, or at the end of the file. Trailing blank lines
/// belong to the *next* section, not this one — we insert before them so
/// the heading visually owns its tasks.
fn section_end(lines: &[String], heading_idx: usize, level: usize) -> usize {
    let mut end = lines.len();
    for (i, l) in lines.iter().enumerate().skip(heading_idx + 1) {
        if let Some((lvl, _)) = parse_heading(l) {
            if lvl <= level {
                end = i;
                break;
            }
        }
    }
    // Walk back over trailing blank lines — but never cross the heading itself.
    while end > heading_idx + 1 && lines[end - 1].is_empty() {
        end -= 1;
    }
    end
}

// ── complete_task ────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum CompleteError {
    #[error("could not read {}: {source}", .path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("line {line} not found in {} ({file_lines} lines)", .path.display())]
    LineMissing {
        path: PathBuf,
        line: usize,
        file_lines: usize,
    },
    #[error("line {line} in {} is not a task", .path.display())]
    NotATask { path: PathBuf, line: usize },
    #[error("task at {}:{} is already done (on {})", .path.display(), .line, .done)]
    AlreadyDone {
        path: PathBuf,
        line: usize,
        done: NaiveDate,
    },
    #[error(transparent)]
    Recurrence(#[from] RecurrenceError),
    #[error("write failed: {source}")]
    Write {
        #[from]
        source: crate::error::Error,
    },
}

#[derive(Debug, Clone)]
pub struct CompleteOptions {
    /// Date to record as the done date.
    pub on: NaiveDate,
}

#[derive(Debug)]
pub struct CompleteOutcome {
    /// 1-indexed line of the now-done task in the rewritten file.
    pub completed_line: usize,
    /// Serialized form of the completed task line.
    pub completed_serialized: String,
    /// If the task was recurring, the new instance's 1-indexed line and
    /// serialized form.
    pub next_instance: Option<NextInstance>,
}

#[derive(Debug)]
pub struct NextInstance {
    pub line: usize,
    pub serialized: String,
}

/// Mark the task at `target_path:line` complete. If the task is recurring,
/// the next instance is inserted *above* the now-completed line (matching
/// plugin behavior).
pub fn complete_task(
    target_path: &Path,
    line: usize,
    opts: CompleteOptions,
) -> Result<CompleteOutcome, CompleteError> {
    let content = std::fs::read_to_string(target_path).map_err(|e| CompleteError::Read {
        path: target_path.to_path_buf(),
        source: e,
    })?;

    let mut lines: Vec<String> = content
        .split_inclusive('\n')
        .map(|s| s.trim_end_matches('\n').trim_end_matches('\r').to_string())
        .collect();

    if line == 0 || line > lines.len() {
        return Err(CompleteError::LineMissing {
            path: target_path.to_path_buf(),
            line,
            file_lines: lines.len(),
        });
    }

    let idx = line - 1;
    let original = &lines[idx];
    let ctx = ParseContext {
        source_file: PathBuf::new(),
        source_line: line,
    };
    let task = EmojiFormat
        .parse_line(original, ctx)
        .ok_or_else(|| CompleteError::NotATask {
            path: target_path.to_path_buf(),
            line,
        })?;

    if task.status == Status::Done {
        if let Some(done) = task.done {
            return Err(CompleteError::AlreadyDone {
                path: target_path.to_path_buf(),
                line,
                done,
            });
        }
    }

    let next_task = if let Some(rule_str) = task.recurrence.as_deref() {
        let rule = recurrence::parse_rule(rule_str)?;
        let next = recurrence::next_dates(&rule, &task)?;
        let mut t = task.clone();
        t.status = Status::Open;
        t.start = next.start;
        t.scheduled = next.scheduled;
        t.due = next.due;
        t.done = None;
        t.cancelled = None;
        Some(t)
    } else {
        None
    };

    let mut completed = task;
    completed.status = Status::Done;
    completed.done = Some(opts.on);
    let completed_line = EmojiFormat.serialize_line(&completed);

    let mut next_instance: Option<NextInstance> = None;
    if let Some(t) = next_task {
        let serialized = EmojiFormat.serialize_line(&t);
        // Insert the new instance above the completed line.
        lines.insert(idx, serialized.clone());
        next_instance = Some(NextInstance {
            // The just-inserted line takes over the original `line` slot.
            line,
            serialized,
        });
        // The completed task is now at idx+1 (1-indexed: line+1).
        lines[idx + 1] = completed_line.clone();
    } else {
        lines[idx] = completed_line.clone();
    }

    let mut joined = lines.join("\n");
    joined.push('\n');
    write_atomic(target_path, &joined)?;

    let completed_line_no = if next_instance.is_some() {
        line + 1
    } else {
        line
    };

    Ok(CompleteOutcome {
        completed_line: completed_line_no,
        completed_serialized: completed_line,
        next_instance,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn input(desc: &str) -> CreateInput {
        CreateInput {
            description: desc.into(),
            ..Default::default()
        }
    }

    #[test]
    fn build_task_appends_tags() {
        let mut i = input("Buy milk");
        i.tags = vec!["work".into(), "#urgent".into()];
        let t = build_task(&i);
        assert_eq!(t.description, "Buy milk #work #urgent");
        assert_eq!(t.tags, vec!["work", "urgent"]);
    }

    #[test]
    fn build_task_does_not_double_existing_tag() {
        let mut i = input("Buy milk #work");
        i.tags = vec!["work".into()];
        let t = build_task(&i);
        assert_eq!(t.description, "Buy milk #work");
        assert_eq!(t.tags, vec!["work"]);
    }

    #[test]
    fn append_to_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("daily.md");
        let mut i = input("Buy milk");
        i.due = Some(d(2026, 5, 10));
        let outcome = create_task(
            &p,
            i,
            CreateOptions {
                position: Position::Append,
                force: false,
            },
        )
        .unwrap();
        assert_eq!(outcome.line, 1);
        let content = std::fs::read_to_string(&p).unwrap();
        assert_eq!(content, "- [ ] Buy milk 📅 2026-05-10\n");
    }

    #[test]
    fn append_to_existing_file_preserves_content() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notes.md");
        std::fs::write(&p, "# Notes\n\nSome prose.\n").unwrap();
        let outcome = create_task(
            &p,
            input("Buy milk"),
            CreateOptions {
                position: Position::Append,
                force: false,
            },
        )
        .unwrap();
        assert_eq!(outcome.line, 4);
        let content = std::fs::read_to_string(&p).unwrap();
        assert_eq!(content, "# Notes\n\nSome prose.\n- [ ] Buy milk\n");
    }

    #[test]
    fn at_line_inserts_in_middle() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notes.md");
        std::fs::write(&p, "line1\nline2\nline3\n").unwrap();
        let outcome = create_task(
            &p,
            input("Buy milk"),
            CreateOptions {
                position: Position::AtLine(2),
                force: false,
            },
        )
        .unwrap();
        assert_eq!(outcome.line, 2);
        let content = std::fs::read_to_string(&p).unwrap();
        assert_eq!(content, "line1\n- [ ] Buy milk\nline2\nline3\n");
    }

    #[test]
    fn at_line_zero_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notes.md");
        std::fs::write(&p, "line1\n").unwrap();
        let err = create_task(
            &p,
            input("X"),
            CreateOptions {
                position: Position::AtLine(0),
                force: false,
            },
        )
        .unwrap_err();
        assert!(matches!(err, CreateError::LineOutOfRange { .. }));
    }

    #[test]
    fn under_heading_existing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notes.md");
        std::fs::write(&p, "## Tasks\n- [ ] existing\n\n## Other\nstuff\n").unwrap();
        let outcome = create_task(
            &p,
            input("Buy milk"),
            CreateOptions {
                position: Position::UnderHeading("Tasks".into()),
                force: false,
            },
        )
        .unwrap();
        // Inserted right after "- [ ] existing" — before the blank line, so
        // it visually belongs to the Tasks section.
        let content = std::fs::read_to_string(&p).unwrap();
        assert_eq!(
            content,
            "## Tasks\n- [ ] existing\n- [ ] Buy milk\n\n## Other\nstuff\n"
        );
        assert_eq!(outcome.line, 3);
    }

    #[test]
    fn under_heading_missing_creates_heading() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notes.md");
        std::fs::write(&p, "# Notes\n\nProse.\n").unwrap();
        create_task(
            &p,
            input("Buy milk"),
            CreateOptions {
                position: Position::UnderHeading("Tasks".into()),
                force: false,
            },
        )
        .unwrap();
        let content = std::fs::read_to_string(&p).unwrap();
        assert_eq!(content, "# Notes\n\nProse.\n\n## Tasks\n- [ ] Buy milk\n");
    }

    #[test]
    fn under_heading_at_top_of_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notes.md");
        create_task(
            &p,
            input("Buy milk"),
            CreateOptions {
                position: Position::UnderHeading("Tasks".into()),
                force: false,
            },
        )
        .unwrap();
        let content = std::fs::read_to_string(&p).unwrap();
        assert_eq!(content, "## Tasks\n- [ ] Buy milk\n");
    }

    #[test]
    fn duplicate_refused_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notes.md");
        std::fs::write(&p, "- [ ] Buy milk 📅 2026-05-10\n").unwrap();
        let mut i = input("Buy milk");
        i.due = Some(d(2026, 5, 10));
        let err = create_task(
            &p,
            i,
            CreateOptions {
                position: Position::Append,
                force: false,
            },
        )
        .unwrap_err();
        assert!(matches!(err, CreateError::Duplicate { .. }));
    }

    #[test]
    fn duplicate_inserted_with_force() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notes.md");
        std::fs::write(&p, "- [ ] Buy milk 📅 2026-05-10\n").unwrap();
        let mut i = input("Buy milk");
        i.due = Some(d(2026, 5, 10));
        create_task(
            &p,
            i,
            CreateOptions {
                position: Position::Append,
                force: true,
            },
        )
        .unwrap();
        let content = std::fs::read_to_string(&p).unwrap();
        let count = content.matches("- [ ] Buy milk 📅 2026-05-10").count();
        assert_eq!(count, 2);
    }

    #[test]
    fn duplicate_check_distinguishes_dates() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notes.md");
        std::fs::write(&p, "- [ ] Buy milk 📅 2026-05-10\n").unwrap();
        // Different due date → not a duplicate.
        let mut i = input("Buy milk");
        i.due = Some(d(2026, 5, 11));
        create_task(
            &p,
            i,
            CreateOptions {
                position: Position::Append,
                force: false,
            },
        )
        .unwrap();
    }

    #[test]
    fn parse_heading_levels() {
        assert_eq!(parse_heading("# Top"), Some((1, "Top")));
        assert_eq!(parse_heading("### Three"), Some((3, "Three")));
        assert_eq!(parse_heading("###### Six"), Some((6, "Six")));
        assert_eq!(parse_heading("####### Seven"), None);
        assert_eq!(parse_heading("not a heading"), None);
        assert_eq!(parse_heading("#NoSpace"), None);
    }

    // ── complete_task ─────────────────────────────────────────────────────────

    #[test]
    fn complete_simple_task_marks_done_with_date() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notes.md");
        std::fs::write(&p, "# Notes\n- [ ] Buy milk 📅 2026-05-10\n").unwrap();
        let outcome = complete_task(&p, 2, CompleteOptions { on: d(2026, 5, 10) }).unwrap();
        assert_eq!(outcome.completed_line, 2);
        assert!(outcome.next_instance.is_none());
        let content = std::fs::read_to_string(&p).unwrap();
        assert_eq!(
            content,
            "# Notes\n- [x] Buy milk 📅 2026-05-10 ✅ 2026-05-10\n"
        );
    }

    #[test]
    fn complete_already_done_errors() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notes.md");
        std::fs::write(&p, "- [x] task ✅ 2026-05-09\n").unwrap();
        let err = complete_task(&p, 1, CompleteOptions { on: d(2026, 5, 10) }).unwrap_err();
        assert!(matches!(err, CompleteError::AlreadyDone { .. }), "{err:?}");
    }

    #[test]
    fn complete_non_task_line_errors() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notes.md");
        std::fs::write(&p, "# Heading\nProse\n").unwrap();
        let err = complete_task(&p, 1, CompleteOptions { on: d(2026, 5, 10) }).unwrap_err();
        assert!(matches!(err, CompleteError::NotATask { .. }), "{err:?}");
    }

    #[test]
    fn complete_line_out_of_range_errors() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notes.md");
        std::fs::write(&p, "- [ ] x\n").unwrap();
        let err = complete_task(&p, 5, CompleteOptions { on: d(2026, 5, 10) }).unwrap_err();
        assert!(matches!(err, CompleteError::LineMissing { .. }), "{err:?}");
    }

    #[test]
    fn complete_recurring_task_inserts_next_instance_above() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notes.md");
        std::fs::write(
            &p,
            "- [ ] Pay tax 🔁 every month on the 18th 📅 2026-05-18\n",
        )
        .unwrap();
        let outcome = complete_task(&p, 1, CompleteOptions { on: d(2026, 5, 18) }).unwrap();
        // The next instance lives where the original task was; the completed
        // task moved down one line.
        assert_eq!(outcome.completed_line, 2);
        let next = outcome.next_instance.expect("recurrence creates next");
        assert_eq!(next.line, 1);

        let content = std::fs::read_to_string(&p).unwrap();
        assert_eq!(
            content,
            "- [ ] Pay tax 🔁 every month on the 18th 📅 2026-06-18\n\
             - [x] Pay tax 🔁 every month on the 18th 📅 2026-05-18 ✅ 2026-05-18\n"
        );
    }

    #[test]
    fn complete_recurring_weekly_shifts_all_dates() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notes.md");
        std::fs::write(
            &p,
            "- [ ] Standup 🔁 every week ⏳ 2026-05-08 📅 2026-05-10\n",
        )
        .unwrap();
        complete_task(&p, 1, CompleteOptions { on: d(2026, 5, 10) }).unwrap();
        let content = std::fs::read_to_string(&p).unwrap();
        // delta = +7 days, so scheduled and due both shift by 7.
        assert_eq!(
            content,
            "- [ ] Standup 🔁 every week ⏳ 2026-05-15 📅 2026-05-17\n\
             - [x] Standup 🔁 every week ⏳ 2026-05-08 📅 2026-05-10 ✅ 2026-05-10\n"
        );
    }

    #[test]
    fn complete_recurring_with_unsupported_pattern_errors() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notes.md");
        std::fs::write(&p, "- [ ] Yearly thing 🔁 every year 📅 2026-05-10\n").unwrap();
        let err = complete_task(&p, 1, CompleteOptions { on: d(2026, 5, 10) }).unwrap_err();
        assert!(
            matches!(
                err,
                CompleteError::Recurrence(RecurrenceError::Unsupported { .. })
            ),
            "{err:?}"
        );
        // File must be untouched.
        let content = std::fs::read_to_string(&p).unwrap();
        assert_eq!(content, "- [ ] Yearly thing 🔁 every year 📅 2026-05-10\n");
    }

    #[test]
    fn complete_recurring_with_no_anchor_errors() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notes.md");
        std::fs::write(&p, "- [ ] No-anchor 🔁 every day\n").unwrap();
        let err = complete_task(&p, 1, CompleteOptions { on: d(2026, 5, 10) }).unwrap_err();
        assert!(
            matches!(err, CompleteError::Recurrence(RecurrenceError::NoAnchor)),
            "{err:?}"
        );
    }

    #[test]
    fn complete_preserves_indentation_and_other_lines() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notes.md");
        std::fs::write(
            &p,
            "## Tasks\n- [ ] parent\n  - [ ] child to complete\n  - [ ] sibling\n",
        )
        .unwrap();
        complete_task(&p, 3, CompleteOptions { on: d(2026, 5, 10) }).unwrap();
        let content = std::fs::read_to_string(&p).unwrap();
        assert_eq!(
            content,
            "## Tasks\n- [ ] parent\n  - [x] child to complete ✅ 2026-05-10\n  - [ ] sibling\n"
        );
    }
}
