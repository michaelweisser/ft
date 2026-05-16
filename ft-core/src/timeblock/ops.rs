//! High-level timeblock mutation primitives. Each entry point reads a
//! daily note, applies the change, and writes atomically via
//! [`super::doc::Document::write`].

use std::path::Path;

use chrono::NaiveTime;

use crate::error::{Error, Result};

use super::doc::Document;
use super::{Tag, Timeblock};

/// Knobs for [`add_block`].
#[derive(Debug, Clone, Default)]
pub struct AddOptions {
    /// When `true`, bypass the duplicate check (same start + end + desc).
    pub force: bool,
}

/// Partial update applied by [`edit_block`]. Every field is optional;
/// `None` means "leave alone".
#[derive(Debug, Clone, Default)]
pub struct EditMutation {
    pub start: Option<TimeChange>,
    pub end: Option<TimeChange>,
    pub desc: Option<String>,
    pub add_tags: Vec<Tag>,
    pub remove_tags: Vec<Tag>,
}

/// Absolute or relative time mutation. Used by both `--start` and `--end`
/// in the CLI; the TUI's `]`/`[`/`}`/`{` chords construct
/// [`TimeChange::ShiftMinutes`] values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeChange {
    Absolute(NaiveTime),
    /// Positive shifts move forward; negative shifts move backward. Clamps
    /// at `00:00` and `23:59` respectively (no overflow / wrap-around).
    ShiftMinutes(i32),
}

/// How to identify one block in a [`Document`] for [`edit_block`] /
/// [`delete_block`].
#[derive(Debug, Clone)]
pub enum Selector {
    /// 1-indexed position in the section block list, matching
    /// [`Timeblock::source_line`].
    Line(usize),
    /// Exact match against [`Timeblock::start`].
    Time(NaiveTime),
    /// Case-insensitive substring match against [`Timeblock::desc`].
    Fuzzy(String),
}

/// Result of resolving a [`Selector`] against a list of blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorResult {
    /// Exactly one block matched. Holds the index into the block slice.
    Found(usize),
    /// Zero blocks matched.
    None,
    /// More than one block matched. Holds the candidate indices.
    Ambiguous(Vec<usize>),
}

impl Selector {
    pub fn resolve(&self, blocks: &[Timeblock]) -> SelectorResult {
        match self {
            Selector::Line(n) => {
                let matches: Vec<usize> = blocks
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| b.source_line == *n)
                    .map(|(i, _)| i)
                    .collect();
                from_matches(matches)
            }
            Selector::Time(t) => {
                let matches: Vec<usize> = blocks
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| b.start == *t)
                    .map(|(i, _)| i)
                    .collect();
                from_matches(matches)
            }
            Selector::Fuzzy(needle) => {
                let n = needle.to_ascii_lowercase();
                let matches: Vec<usize> = blocks
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| b.desc.to_ascii_lowercase().contains(&n))
                    .map(|(i, _)| i)
                    .collect();
                from_matches(matches)
            }
        }
    }
}

fn from_matches(matches: Vec<usize>) -> SelectorResult {
    match matches.len() {
        0 => SelectorResult::None,
        1 => SelectorResult::Found(matches[0]),
        _ => SelectorResult::Ambiguous(matches),
    }
}

/// Insert `new` into the daily note at `daily_path`, under `heading`.
/// Inserts in start-time order. Refuses exact duplicates unless
/// `opts.force` is set.
pub fn add_block(
    daily_path: &Path,
    heading: &str,
    new: Timeblock,
    opts: AddOptions,
) -> Result<Document> {
    let mut doc = Document::read(daily_path, heading)?;
    if !opts.force {
        if let Some(dup) = doc
            .blocks
            .iter()
            .find(|b| b.start == new.start && b.end == new.end && b.desc == new.desc)
        {
            return Err(Error::Timeblock(format!(
                "duplicate block at line {}: {} - {} {} (use --force to insert anyway)",
                dup.source_line,
                super::format_hhmm(dup.start),
                super::format_hhmm(dup.end),
                dup.desc,
            )));
        }
    }
    doc.blocks.push(new);
    reindex(&mut doc);
    doc.write()?;
    Ok(doc)
}

/// Apply `mutation` to the block matched by `selector`. Errors on no
/// match, on ambiguous match, or when the resulting block would have
/// `end <= start`.
pub fn edit_block(
    daily_path: &Path,
    heading: &str,
    selector: &Selector,
    mutation: EditMutation,
) -> Result<Document> {
    let mut doc = Document::read(daily_path, heading)?;
    let idx = resolve_or_err(selector, &doc.blocks)?;
    apply_mutation(&mut doc.blocks[idx], mutation)?;
    reindex(&mut doc);
    doc.write()?;
    Ok(doc)
}

/// Remove the block matched by `selector`.
pub fn delete_block(daily_path: &Path, heading: &str, selector: &Selector) -> Result<Document> {
    let mut doc = Document::read(daily_path, heading)?;
    let idx = resolve_or_err(selector, &doc.blocks)?;
    doc.blocks.remove(idx);
    reindex(&mut doc);
    doc.write()?;
    Ok(doc)
}

fn resolve_or_err(selector: &Selector, blocks: &[Timeblock]) -> Result<usize> {
    match selector.resolve(blocks) {
        SelectorResult::Found(i) => Ok(i),
        SelectorResult::None => Err(Error::Timeblock(format!("no block matched {selector:?}"))),
        SelectorResult::Ambiguous(candidates) => {
            let mut lines: Vec<String> = candidates
                .iter()
                .take(5)
                .map(|i| {
                    let b = &blocks[*i];
                    format!(
                        "  {}: {} - {} {}",
                        b.source_line,
                        super::format_hhmm(b.start),
                        super::format_hhmm(b.end),
                        b.desc,
                    )
                })
                .collect();
            if candidates.len() > 5 {
                lines.push(format!("  ... and {} more", candidates.len() - 5));
            }
            Err(Error::Timeblock(format!(
                "ambiguous selector — {} blocks matched:\n{}",
                candidates.len(),
                lines.join("\n"),
            )))
        }
    }
}

fn apply_mutation(b: &mut Timeblock, m: EditMutation) -> Result<()> {
    if let Some(change) = m.start {
        b.start = apply_change(b.start, change);
    }
    if let Some(change) = m.end {
        b.end = apply_change(b.end, change);
        b.end_explicit = true;
    }
    if b.end <= b.start {
        return Err(Error::Timeblock(format!(
            "end {} must be after start {}",
            super::format_hhmm(b.end),
            super::format_hhmm(b.start),
        )));
    }
    if let Some(desc) = m.desc {
        b.desc = desc;
        b.tags = super::parse_tags(&b.desc);
    }
    if !m.add_tags.is_empty() {
        for tag in m.add_tags {
            let token = tag.to_string_form();
            // Append to desc if not already present, then refresh tags.
            let already_in_desc = b.desc.split_whitespace().any(|w| w == token);
            if !already_in_desc {
                if !b.desc.is_empty() {
                    b.desc.push(' ');
                }
                b.desc.push_str(&token);
            }
        }
        b.tags = super::parse_tags(&b.desc);
        // Dedupe by levels — last-write-wins on insertion order.
        let mut seen = std::collections::HashSet::new();
        b.tags.retain(|t| seen.insert(t.levels.clone()));
    }
    if !m.remove_tags.is_empty() {
        for tag in m.remove_tags {
            let token = tag.to_string_form();
            b.desc = strip_token(&b.desc, &token);
        }
        b.tags = super::parse_tags(&b.desc);
    }
    Ok(())
}

fn apply_change(t: NaiveTime, change: TimeChange) -> NaiveTime {
    use chrono::Timelike;
    match change {
        TimeChange::Absolute(t) => t,
        TimeChange::ShiftMinutes(m) => {
            let cur = (t.hour() as i32) * 60 + (t.minute() as i32);
            let new = (cur + m).clamp(0, 23 * 60 + 59);
            NaiveTime::from_hms_opt((new / 60) as u32, (new % 60) as u32, 0).unwrap()
        }
    }
}

/// Strip an exact `@tag` token from `desc`. Removes the preceding space
/// when present so we don't leave a double-space behind.
fn strip_token(desc: &str, token: &str) -> String {
    let mut out = String::with_capacity(desc.len());
    let mut chars = desc.char_indices().peekable();
    while let Some((i, _c)) = chars.next() {
        if desc[i..].starts_with(token) {
            let next = i + token.len();
            // Only strip if the match ends at a word boundary
            // (whitespace, end-of-string, or `/` followed by more).
            let ends_clean = desc[next..]
                .chars()
                .next()
                .map(|c| c.is_whitespace())
                .unwrap_or(true);
            if ends_clean {
                // Drop the trailing whitespace too (or leading if at end).
                if out.ends_with(' ') {
                    out.pop();
                }
                // skip past the token
                while chars.peek().map(|(j, _)| *j < next).unwrap_or(false) {
                    chars.next();
                }
                continue;
            }
        }
        out.push(desc[i..].chars().next().unwrap());
    }
    out
}

/// Sort blocks by start time and re-assign 1-indexed `source_line`.
fn reindex(doc: &mut Document) {
    doc.blocks.sort_by_key(|b| b.start);
    for (i, b) in doc.blocks.iter_mut().enumerate() {
        b.source_line = i + 1;
    }
}

// ── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeblock::parse_line;
    use assert_fs::prelude::*;
    use assert_fs::TempDir;
    use std::path::PathBuf;

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    fn vault_with(body: &str) -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let f = tmp.child("day.md");
        f.write_str(body).unwrap();
        let p = f.path().to_path_buf();
        (tmp, p)
    }

    // ── Selector / SelectorResult ────────────────────────────────────────

    #[test]
    fn selector_line_matches_by_source_line() {
        let blocks = vec![mkblock(1, 9, 0, 10, 0, "a"), mkblock(2, 10, 0, 11, 0, "b")];
        assert_eq!(Selector::Line(2).resolve(&blocks), SelectorResult::Found(1));
        assert_eq!(Selector::Line(9).resolve(&blocks), SelectorResult::None);
    }

    #[test]
    fn selector_time_matches_by_start() {
        let blocks = vec![mkblock(1, 9, 0, 10, 0, "a"), mkblock(2, 10, 0, 11, 0, "b")];
        assert_eq!(
            Selector::Time(t(10, 0)).resolve(&blocks),
            SelectorResult::Found(1)
        );
    }

    #[test]
    fn selector_fuzzy_case_insensitive() {
        let blocks = vec![
            mkblock(1, 9, 0, 10, 0, "Standup"),
            mkblock(2, 10, 0, 11, 0, "code review"),
        ];
        assert_eq!(
            Selector::Fuzzy("REVIEW".into()).resolve(&blocks),
            SelectorResult::Found(1),
        );
    }

    #[test]
    fn selector_fuzzy_ambiguous_returns_all() {
        let blocks = vec![
            mkblock(1, 9, 0, 10, 0, "review code"),
            mkblock(2, 10, 0, 11, 0, "review pr"),
        ];
        assert_eq!(
            Selector::Fuzzy("review".into()).resolve(&blocks),
            SelectorResult::Ambiguous(vec![0, 1]),
        );
    }

    fn mkblock(line: usize, sh: u32, sm: u32, eh: u32, em: u32, desc: &str) -> Timeblock {
        Timeblock {
            start: t(sh, sm),
            end: t(eh, em),
            end_explicit: true,
            desc: desc.into(),
            tags: super::super::parse_tags(desc),
            source_line: line,
        }
    }

    // ── add_block ────────────────────────────────────────────────────────

    #[test]
    fn add_block_inserts_into_existing_section() {
        let (_tmp, p) = vault_with("## Time Blocks\n- 09:00 - 10:00 a\n");
        let new = parse_line("10:00 - 11:00 b").unwrap();
        let doc = add_block(&p, "Time Blocks", new, AddOptions::default()).unwrap();
        assert_eq!(doc.blocks.len(), 2);
        let body = std::fs::read_to_string(&p).unwrap();
        assert!(body.contains("- 09:00 - 10:00 a"));
        assert!(body.contains("- 10:00 - 11:00 b"));
    }

    #[test]
    fn add_block_preserves_sort_order() {
        let (_tmp, p) = vault_with("## Time Blocks\n- 10:00 - 11:00 second\n");
        let new = parse_line("09:00 - 10:00 first").unwrap();
        let doc = add_block(&p, "Time Blocks", new, AddOptions::default()).unwrap();
        assert_eq!(doc.blocks[0].desc, "first");
        assert_eq!(doc.blocks[1].desc, "second");
        let body = std::fs::read_to_string(&p).unwrap();
        let idx_first = body.find("first").unwrap();
        let idx_second = body.find("second").unwrap();
        assert!(idx_first < idx_second);
    }

    #[test]
    fn add_block_creates_section_when_missing() {
        let (_tmp, p) = vault_with("# Day\n\nprose\n");
        let new = parse_line("09:00 - 10:00 fresh").unwrap();
        add_block(&p, "Time Blocks", new, AddOptions::default()).unwrap();
        let body = std::fs::read_to_string(&p).unwrap();
        assert!(body.contains("## Time Blocks"));
        assert!(body.contains("- 09:00 - 10:00 fresh"));
    }

    #[test]
    fn add_block_rejects_exact_duplicate() {
        let (_tmp, p) = vault_with("## Time Blocks\n- 09:00 - 10:00 same\n");
        let dup = parse_line("09:00 - 10:00 same").unwrap();
        let err = add_block(&p, "Time Blocks", dup, AddOptions::default()).unwrap_err();
        assert!(matches!(err, Error::Timeblock(_)));
    }

    #[test]
    fn add_block_force_inserts_duplicate() {
        let (_tmp, p) = vault_with("## Time Blocks\n- 09:00 - 10:00 same\n");
        let dup = parse_line("09:00 - 10:00 same").unwrap();
        add_block(&p, "Time Blocks", dup, AddOptions { force: true }).unwrap();
        let body = std::fs::read_to_string(&p).unwrap();
        let count = body.matches("- 09:00 - 10:00 same").count();
        assert_eq!(count, 2);
    }

    // ── edit_block ───────────────────────────────────────────────────────

    #[test]
    fn edit_block_absolute_start_and_end() {
        let (_tmp, p) = vault_with("## Time Blocks\n- 09:00 - 10:00 a\n");
        let doc = edit_block(
            &p,
            "Time Blocks",
            &Selector::Line(1),
            EditMutation {
                start: Some(TimeChange::Absolute(t(9, 30))),
                end: Some(TimeChange::Absolute(t(11, 0))),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(doc.blocks[0].start, t(9, 30));
        assert_eq!(doc.blocks[0].end, t(11, 0));
    }

    #[test]
    fn edit_block_relative_shifts() {
        let (_tmp, p) = vault_with("## Time Blocks\n- 09:00 - 10:00 a\n");
        let doc = edit_block(
            &p,
            "Time Blocks",
            &Selector::Time(t(9, 0)),
            EditMutation {
                end: Some(TimeChange::ShiftMinutes(15)),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(doc.blocks[0].end, t(10, 15));
    }

    #[test]
    fn edit_block_shift_clamps_at_zero() {
        let (_tmp, p) = vault_with("## Time Blocks\n- 09:00 - 10:00 a\n");
        let doc = edit_block(
            &p,
            "Time Blocks",
            &Selector::Line(1),
            EditMutation {
                start: Some(TimeChange::ShiftMinutes(-1000)),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(doc.blocks[0].start, t(0, 0));
    }

    #[test]
    fn edit_block_shift_clamps_at_2359() {
        let (_tmp, p) = vault_with("## Time Blocks\n- 09:00 - 10:00 a\n");
        let doc = edit_block(
            &p,
            "Time Blocks",
            &Selector::Line(1),
            EditMutation {
                end: Some(TimeChange::ShiftMinutes(10_000)),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(doc.blocks[0].end, t(23, 59));
    }

    #[test]
    fn edit_block_rejects_end_at_or_before_start() {
        let (_tmp, p) = vault_with("## Time Blocks\n- 09:00 - 10:00 a\n");
        let err = edit_block(
            &p,
            "Time Blocks",
            &Selector::Line(1),
            EditMutation {
                end: Some(TimeChange::Absolute(t(9, 0))),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(matches!(err, Error::Timeblock(_)));
    }

    #[test]
    fn edit_block_add_tag_appends_to_desc() {
        let (_tmp, p) = vault_with("## Time Blocks\n- 09:00 - 10:00 standup\n");
        let doc = edit_block(
            &p,
            "Time Blocks",
            &Selector::Line(1),
            EditMutation {
                add_tags: vec![Tag {
                    levels: vec!["work".into(), "meeting".into()],
                }],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(doc.blocks[0].desc, "standup @work/meeting");
        assert_eq!(doc.blocks[0].tags.len(), 1);
    }

    #[test]
    fn edit_block_add_tag_dedupes_existing() {
        let (_tmp, p) = vault_with("## Time Blocks\n- 09:00 - 10:00 standup @work\n");
        let doc = edit_block(
            &p,
            "Time Blocks",
            &Selector::Line(1),
            EditMutation {
                add_tags: vec![Tag {
                    levels: vec!["work".into()],
                }],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(doc.blocks[0].desc, "standup @work");
        assert_eq!(doc.blocks[0].tags.len(), 1);
    }

    #[test]
    fn edit_block_remove_tag_strips_token() {
        let (_tmp, p) = vault_with("## Time Blocks\n- 09:00 - 10:00 standup @work @later\n");
        let doc = edit_block(
            &p,
            "Time Blocks",
            &Selector::Line(1),
            EditMutation {
                remove_tags: vec![Tag {
                    levels: vec!["work".into()],
                }],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(doc.blocks[0].desc, "standup @later");
    }

    #[test]
    fn edit_block_change_desc_refreshes_tags() {
        let (_tmp, p) = vault_with("## Time Blocks\n- 09:00 - 10:00 old @prev\n");
        let doc = edit_block(
            &p,
            "Time Blocks",
            &Selector::Line(1),
            EditMutation {
                desc: Some("new desc @work/x".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(doc.blocks[0].desc, "new desc @work/x");
        assert_eq!(doc.blocks[0].tags[0].levels, vec!["work", "x"]);
    }

    #[test]
    fn edit_block_no_match_errors() {
        let (_tmp, p) = vault_with("## Time Blocks\n- 09:00 - 10:00 a\n");
        let err = edit_block(
            &p,
            "Time Blocks",
            &Selector::Line(99),
            EditMutation::default(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Timeblock(_)));
    }

    #[test]
    fn edit_block_ambiguous_errors() {
        let (_tmp, p) = vault_with(
            "## Time Blocks\n- 09:00 - 10:00 review pr\n- 10:00 - 11:00 review issues\n",
        );
        let err = edit_block(
            &p,
            "Time Blocks",
            &Selector::Fuzzy("review".into()),
            EditMutation::default(),
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("ambiguous"));
        assert!(msg.contains("review pr"));
    }

    // ── delete_block ─────────────────────────────────────────────────────

    #[test]
    fn delete_block_removes_matched_block() {
        let (_tmp, p) = vault_with("## Time Blocks\n- 09:00 - 10:00 a\n- 10:00 - 11:00 b\n");
        let doc = delete_block(&p, "Time Blocks", &Selector::Line(1)).unwrap();
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.blocks[0].desc, "b");
        let body = std::fs::read_to_string(&p).unwrap();
        assert!(!body.contains("- 09:00 - 10:00 a"));
        assert!(body.contains("- 10:00 - 11:00 b"));
    }

    #[test]
    fn delete_block_no_match_errors() {
        let (_tmp, p) = vault_with("## Time Blocks\n- 09:00 - 10:00 a\n");
        let err = delete_block(&p, "Time Blocks", &Selector::Line(9)).unwrap_err();
        assert!(matches!(err, Error::Timeblock(_)));
    }
}
