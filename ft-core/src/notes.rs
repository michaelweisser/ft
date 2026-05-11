//! Section-level operations on markdown notes.
//!
//! A *section* is a heading plus the body that follows it, up to the next
//! heading of equal or higher level. This matches Obsidian's fold rule —
//! moving an `##` heading drags its nested `###` / `####` headings with it.
//!
//! The primitives here are deliberately pure (string in, string out) so
//! the CLI and the TUI can share the same logic and the tests don't need
//! a filesystem. The single filesystem touchpoint is [`write_pair`].
//!
//! ## Pair-write ordering
//!
//! [`move_sections`] returns two new contents — a target and a source —
//! and [`write_pair`] writes the target first, then the source. POSIX has
//! no atomic two-file replace, so a crash between the two writes leaves
//! the moved sections duplicated rather than lost. Duplication is
//! recoverable by hand; data loss isn't.

use std::path::Path;

use crate::error::{Error, Result};
use crate::fs::write_atomic;
use crate::markdown::{extract_headings, Heading};

/// A heading and the body that belongs to it.
///
/// `body` includes the heading line itself and runs up to (but not
/// including) the next heading of equal-or-higher level — or end of
/// file. Trailing newline behavior follows the source: if the section
/// is followed by another heading, `body` ends in `\n`; if it's the
/// final section and the file ends without a trailing newline, `body`
/// has no trailing newline either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub heading: Heading,
    pub body: String,
}

/// One heading the caller wants to move out of the source, plus the
/// target heading level to drop it at.
///
/// `source_line` is the 1-indexed line number of the heading in the
/// source content — the same `line` field that [`extract_headings`] /
/// [`extract_sections`] produce. Line numbers (not heading text) key
/// the pick so duplicate headings are unambiguous.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionPick {
    pub source_line: usize,
    pub new_level: u8,
}

/// Where in the target each pick lands.
///
/// `pick_idx` indexes into the `picks` slice passed to [`move_sections`].
/// `after_line` is the 1-indexed line of the target heading the section
/// should be inserted *after* — i.e. immediately before the next heading
/// of equal-or-higher level (or end of file). `None` means "insert at the
/// very top of the file", before any existing content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub pick_idx: usize,
    pub after_line: Option<usize>,
}

/// Extract every section from `content`, in document order.
///
/// Content before the first heading is excluded (it has no owner heading
/// to attach to). Frontmatter, fenced code blocks, and indented code
/// blocks are skipped per [`extract_headings`]; a `#` that lands inside
/// one of those is not a heading and won't introduce a section.
pub fn extract_sections(content: &str) -> Vec<Section> {
    let headings = extract_headings(content);
    if headings.is_empty() {
        return Vec::new();
    }

    let line_offsets = line_byte_offsets(content);
    let mut sections = Vec::with_capacity(headings.len());

    for (i, h) in headings.iter().enumerate() {
        let start = line_offsets[h.line - 1];
        let end = headings[i + 1..]
            .iter()
            .find(|next| next.level <= h.level)
            .map(|next| line_offsets[next.line - 1])
            .unwrap_or(content.len());
        sections.push(Section {
            heading: h.clone(),
            body: content[start..end].to_string(),
        });
    }
    sections
}

/// Re-render a section's body with every heading shifted to `new_top_level`.
///
/// The delta is `new_top_level - section.heading.level`; every heading
/// inside the body — including the top heading — moves by the same
/// amount, so a nested H3 inside an H2 stays one level below the top
/// when the H2 becomes an H3. Headings inside fenced or indented code
/// blocks are *not* shifted (they aren't headings as far as the
/// extractor is concerned).
///
/// Errors with [`Error::Notes`] if any heading would shift outside
/// `1..=6`. Validation runs before any rewriting, so partial results
/// are never returned.
pub fn shift_section_level(section: &Section, new_top_level: u8) -> Result<String> {
    if !(1..=6).contains(&new_top_level) {
        return Err(Error::Notes(format!(
            "target level {new_top_level} is outside the ATX range 1..=6"
        )));
    }
    let from = section.heading.level as i16;
    let to = new_top_level as i16;
    let delta = to - from;
    if delta == 0 {
        return Ok(section.body.clone());
    }

    // Headings inside the body, keyed by 1-indexed line number relative
    // to the body itself (line 1 is the top heading).
    let body_headings = extract_headings(&section.body);
    for h in &body_headings {
        let shifted = h.level as i16 + delta;
        if !(1..=6).contains(&shifted) {
            return Err(Error::Notes(format!(
                "heading at body line {} (level {}) would shift to level {}, outside 1..=6",
                h.line, h.level, shifted
            )));
        }
    }

    let mut out = String::with_capacity(section.body.len());
    for (idx, line) in section.body.split_inclusive('\n').enumerate() {
        let lineno = idx + 1;
        if let Some(h) = body_headings.iter().find(|bh| bh.line == lineno) {
            let new_level = (h.level as i16 + delta) as u8;
            out.push_str(&rerender_heading_line(line, new_level));
        } else {
            out.push_str(line);
        }
    }
    Ok(out)
}

/// Reject a selection that mixes a heading with one of its descendants.
///
/// Two selections overlap when the byte range of one section contains
/// another's heading line. The check is text-agnostic — only line
/// numbers participate — so duplicate heading texts can't confuse it.
///
/// Returns `Err(Error::Notes(...))` on the first conflict, naming both
/// line numbers. Duplicate entries in `selected_lines` (two picks for
/// the same heading) are also rejected.
pub fn validate_disjoint(selected_lines: &[usize], all_headings: &[Heading]) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for &line in selected_lines {
        if !seen.insert(line) {
            return Err(Error::Notes(format!(
                "heading at line {line} is selected twice"
            )));
        }
    }

    // For each selected heading, walk forward until we hit a heading of
    // equal-or-higher level or run out. Anything in between that is
    // also selected is a descendant of the outer one.
    for (i, h) in all_headings.iter().enumerate() {
        if !selected_lines.contains(&h.line) {
            continue;
        }
        for inner in all_headings[i + 1..].iter() {
            if inner.level <= h.level {
                break;
            }
            if selected_lines.contains(&inner.line) {
                return Err(Error::Notes(format!(
                    "heading at line {} (level {}) is a descendant of selected heading at line {} (level {})",
                    inner.line, inner.level, h.line, h.level
                )));
            }
        }
    }
    Ok(())
}

/// Move the picked sections out of `source` and into `target`.
///
/// Returns `(new_source, new_target)`. The source has every picked
/// section removed (and the remaining content stitched together with no
/// orphaned blank lines beyond what the source already had). The target
/// has each pick inserted at the position named by its [`Placement`],
/// after a level-shift to the requested `new_level`.
///
/// Picks may share the same `after_line` — they'll be inserted in
/// `picks` order at that point. `Placement::after_line == None` means
/// "top of the target", before any existing content.
pub fn move_sections(
    source: &str,
    picks: &[SectionPick],
    target: &str,
    plan: &[Placement],
) -> Result<(String, String)> {
    if picks.is_empty() {
        return Ok((source.to_string(), target.to_string()));
    }
    if plan.len() != picks.len() {
        return Err(Error::Notes(format!(
            "plan length {} does not match picks length {}",
            plan.len(),
            picks.len()
        )));
    }

    let source_sections = extract_sections(source);
    let source_headings: Vec<Heading> = source_sections.iter().map(|s| s.heading.clone()).collect();
    let selected_lines: Vec<usize> = picks.iter().map(|p| p.source_line).collect();
    validate_disjoint(&selected_lines, &source_headings)?;

    // Resolve each pick to its source section + level-shifted body.
    let mut shifted: Vec<String> = Vec::with_capacity(picks.len());
    let mut picked_indices: Vec<usize> = Vec::with_capacity(picks.len());
    for pick in picks {
        let idx = source_sections
            .iter()
            .position(|s| s.heading.line == pick.source_line)
            .ok_or_else(|| {
                Error::Notes(format!(
                    "pick references line {} which is not a heading in the source",
                    pick.source_line
                ))
            })?;
        shifted.push(shift_section_level(&source_sections[idx], pick.new_level)?);
        picked_indices.push(idx);
    }

    let new_source = remove_sections(source, &source_sections, &picked_indices);
    let new_target = insert_sections(target, &shifted, plan)?;
    Ok((new_source, new_target))
}

/// Write the target first, then the source. Both atomic individually;
/// the pair as a whole isn't atomic — a crash between the two writes
/// leaves duplicated content rather than lost content.
pub fn write_pair(
    target_path: &Path,
    target_content: &str,
    source_path: &Path,
    source_content: &str,
) -> Result<()> {
    write_atomic(target_path, target_content)?;
    write_atomic(source_path, source_content)?;
    Ok(())
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Byte offset of the start of each 1-indexed line. `offsets[0]` is 0;
/// `offsets[i]` is the byte where line `i+1` starts. A virtual entry at
/// `offsets[line_count]` equals `content.len()` so callers can compute
/// section end-of-file naturally.
fn line_byte_offsets(content: &str) -> Vec<usize> {
    let mut offsets = vec![0usize];
    for (idx, c) in content.char_indices() {
        if c == '\n' {
            offsets.push(idx + 1);
        }
    }
    offsets.push(content.len());
    offsets
}

/// Re-render an ATX heading line with a new level, preserving leading
/// whitespace and trailing newline (if any). The input is expected to
/// be a real heading line — i.e. `extract_headings` recognized it.
fn rerender_heading_line(line: &str, new_level: u8) -> String {
    let (newline, body) = if let Some(stripped) = line.strip_suffix('\n') {
        ("\n", stripped)
    } else {
        ("", line)
    };
    let leading_ws_len = body.len() - body.trim_start().len();
    let leading_ws = &body[..leading_ws_len];
    let after_ws = &body[leading_ws_len..];
    let hash_run = after_ws.chars().take_while(|c| *c == '#').count();
    let rest = &after_ws[hash_run..];
    let new_hashes = "#".repeat(new_level as usize);
    format!("{leading_ws}{new_hashes}{rest}{newline}")
}

/// Drop the picked sections out of the source, returning the remainder.
fn remove_sections(source: &str, sections: &[Section], picked: &[usize]) -> String {
    if picked.is_empty() {
        return source.to_string();
    }
    let line_offsets = line_byte_offsets(source);
    // Build a sorted set of byte ranges to remove.
    let mut ranges: Vec<(usize, usize)> = picked
        .iter()
        .map(|&i| {
            let h = &sections[i].heading;
            let start = line_offsets[h.line - 1];
            let end = sections
                .get(i + 1..)
                .and_then(|tail| tail.iter().find(|s| s.heading.level <= h.level))
                .map(|next| line_offsets[next.heading.line - 1])
                .unwrap_or(source.len());
            (start, end)
        })
        .collect();
    ranges.sort_by_key(|r| r.0);

    let mut out = String::with_capacity(source.len());
    let mut cursor = 0;
    for (start, end) in ranges {
        out.push_str(&source[cursor..start]);
        cursor = end;
    }
    out.push_str(&source[cursor..]);
    out
}

/// Splice already-level-shifted sections into the target. The target is
/// not parsed again here — placements are resolved against
/// `extract_sections(target)` for boundary lookup.
fn insert_sections(target: &str, shifted: &[String], plan: &[Placement]) -> Result<String> {
    let target_sections = extract_sections(target);
    let line_offsets = line_byte_offsets(target);

    // Resolve every placement to a byte offset in the target.
    let mut offset_for_pick: Vec<usize> = Vec::with_capacity(plan.len());
    for p in plan {
        if p.pick_idx >= shifted.len() {
            return Err(Error::Notes(format!(
                "placement pick_idx {} is out of range for {} picks",
                p.pick_idx,
                shifted.len()
            )));
        }
        let offset = match p.after_line {
            None => 0,
            Some(line) => {
                let section = target_sections
                    .iter()
                    .find(|s| s.heading.line == line)
                    .ok_or_else(|| {
                        Error::Notes(format!(
                            "placement after_line {line} is not a heading in the target"
                        ))
                    })?;
                let section_end_line = section_end_line(&target_sections, section, target);
                section_end_offset_byte(&line_offsets, section_end_line, target.len())
            }
        };
        offset_for_pick.push(offset);
    }

    // Group inserts by byte offset, preserving plan order within a group.
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for (plan_idx, &offset) in offset_for_pick.iter().enumerate() {
        groups.entry(offset).or_default().push(plan_idx);
    }

    let mut out =
        String::with_capacity(target.len() + shifted.iter().map(|s| s.len()).sum::<usize>());
    let mut cursor = 0;
    for (offset, plan_idxs) in groups {
        out.push_str(&target[cursor..offset]);
        // If inserting at offset 0 and the target doesn't yet have a
        // trailing newline before the first existing content, we still
        // emit the section bodies as-is; downstream code or the user
        // can normalise. We do, however, want to guarantee that the
        // boundary between an inserted body and the following content
        // has a newline if the body itself doesn't end with one.
        for plan_idx in plan_idxs {
            let pick_idx = plan[plan_idx].pick_idx;
            let body = &shifted[pick_idx];
            out.push_str(body);
            if !body.ends_with('\n') && offset < target.len() {
                out.push('\n');
            }
        }
        cursor = offset;
    }
    out.push_str(&target[cursor..]);
    Ok(out)
}

/// Last source line owned by `section` (1-indexed). For the trailing
/// section of a file, this is the line of the final non-empty line of
/// `content`, computed indirectly via the section's byte length.
fn section_end_line(sections: &[Section], section: &Section, content: &str) -> usize {
    // Find the next sibling-or-higher heading; section ends on the
    // previous line.
    let idx = sections
        .iter()
        .position(|s| s.heading.line == section.heading.line)
        .expect("section comes from sections");
    if let Some(next) = sections[idx + 1..]
        .iter()
        .find(|s| s.heading.level <= section.heading.level)
    {
        next.heading.line - 1
    } else {
        // Trailing section: count the lines in `content`.
        content.lines().count()
    }
}

/// Byte offset at the start of the line *after* `end_line`. If `end_line`
/// is the final line of the file, returns `content_len`.
fn section_end_offset_byte(line_offsets: &[usize], end_line: usize, content_len: usize) -> usize {
    if end_line >= line_offsets.len() - 1 {
        content_len
    } else {
        line_offsets[end_line]
    }
}

// ── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── extract_sections ─────────────────────────────────────────────────

    #[test]
    fn extract_sections_empty_returns_empty() {
        assert_eq!(extract_sections(""), Vec::<Section>::new());
    }

    #[test]
    fn extract_sections_no_headings_returns_empty() {
        assert_eq!(
            extract_sections("just prose\nno headings here\n"),
            Vec::<Section>::new()
        );
    }

    #[test]
    fn extract_sections_single_h1() {
        let s = extract_sections("# Top\nbody line\n");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].heading.text, "Top");
        assert_eq!(s[0].body, "# Top\nbody line\n");
    }

    #[test]
    fn extract_sections_h2_includes_nested_h3() {
        let body = "\
## Section A
text under A
### Nested
nested body
## Section B
text under B
";
        let s = extract_sections(body);
        // Every heading produces a section. Section A's body includes
        // the nested H3 (lower level), but the H3 also has its own
        // scoped entry — both are valid pick targets for move_sections.
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].heading.text, "Section A");
        assert!(s[0].body.contains("### Nested"));
        assert!(s[0].body.contains("nested body"));
        assert!(!s[0].body.contains("Section B"));
        assert_eq!(s[1].heading.text, "Nested");
        assert_eq!(s[1].body, "### Nested\nnested body\n");
        assert_eq!(s[2].heading.text, "Section B");
        assert_eq!(s[2].body, "## Section B\ntext under B\n");
    }

    #[test]
    fn extract_sections_sibling_h2s() {
        let body = "## A\na body\n## B\nb body\n";
        let s = extract_sections(body);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].body, "## A\na body\n");
        assert_eq!(s[1].body, "## B\nb body\n");
    }

    #[test]
    fn extract_sections_content_before_first_heading_excluded() {
        let body = "prose first\nmore prose\n# Heading\nafter\n";
        let s = extract_sections(body);
        assert_eq!(s.len(), 1);
        assert!(!s[0].body.contains("prose first"));
        assert!(s[0].body.starts_with("# Heading"));
    }

    #[test]
    fn extract_sections_skips_frontmatter() {
        let body = "\
---
title: Foo
---
# Real
real body
";
        let s = extract_sections(body);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].heading.text, "Real");
    }

    #[test]
    fn extract_sections_trailing_section_without_newline() {
        let body = "# A\nfinal line";
        let s = extract_sections(body);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].body, "# A\nfinal line");
    }

    // ── shift_section_level ──────────────────────────────────────────────

    #[test]
    fn shift_no_op_returns_clone() {
        let body = "## A\ntext\n";
        let section = extract_sections(body).remove(0);
        let out = shift_section_level(&section, 2).unwrap();
        assert_eq!(out, body);
    }

    #[test]
    fn shift_down_cascades_nested() {
        let body = "## Top\n### Mid\n#### Inner\nbody\n";
        let section = extract_sections(body).remove(0);
        let out = shift_section_level(&section, 3).unwrap();
        assert_eq!(out, "### Top\n#### Mid\n##### Inner\nbody\n");
    }

    #[test]
    fn shift_up_cascades_nested() {
        let body = "### Top\n#### Mid\nbody\n";
        let section = extract_sections(body).remove(0);
        let out = shift_section_level(&section, 2).unwrap();
        assert_eq!(out, "## Top\n### Mid\nbody\n");
    }

    #[test]
    fn shift_overflow_past_h6_errors() {
        let body = "## Top\n###### Deep\n";
        let section = extract_sections(body).remove(0);
        let err = shift_section_level(&section, 3).unwrap_err();
        match err {
            Error::Notes(msg) => assert!(msg.contains("outside 1..=6"), "msg: {msg}"),
            _ => panic!("expected Notes error"),
        }
    }

    #[test]
    fn shift_below_h1_errors() {
        let body = "## Top\nbody\n";
        let section = extract_sections(body).remove(0);
        let err = shift_section_level(&section, 0).unwrap_err();
        assert!(matches!(err, Error::Notes(_)));
    }

    #[test]
    fn shift_preserves_code_fence_pseudo_headings() {
        let body = "\
## Top
```rust
# pretend heading
```
### Real Nested
done
";
        let section = extract_sections(body).remove(0);
        let out = shift_section_level(&section, 3).unwrap();
        assert!(out.starts_with("### Top\n"));
        // The line inside the fence keeps its single `#`.
        assert!(out.contains("\n# pretend heading\n"));
        // The real nested heading shifts.
        assert!(out.contains("\n#### Real Nested\n"));
    }

    #[test]
    fn shift_invalid_top_level_errors() {
        let body = "## Top\n";
        let section = extract_sections(body).remove(0);
        assert!(shift_section_level(&section, 7).is_err());
        assert!(shift_section_level(&section, 0).is_err());
    }

    // ── validate_disjoint ────────────────────────────────────────────────

    #[test]
    fn disjoint_empty_ok() {
        assert!(validate_disjoint(&[], &[]).is_ok());
    }

    #[test]
    fn disjoint_singleton_ok() {
        let headings = vec![Heading {
            text: "A".into(),
            level: 2,
            line: 1,
        }];
        assert!(validate_disjoint(&[1], &headings).is_ok());
    }

    #[test]
    fn disjoint_siblings_ok() {
        let headings = vec![
            Heading {
                text: "A".into(),
                level: 2,
                line: 1,
            },
            Heading {
                text: "B".into(),
                level: 2,
                line: 5,
            },
        ];
        assert!(validate_disjoint(&[1, 5], &headings).is_ok());
    }

    #[test]
    fn disjoint_parent_and_child_errors() {
        let headings = vec![
            Heading {
                text: "Parent".into(),
                level: 2,
                line: 1,
            },
            Heading {
                text: "Child".into(),
                level: 3,
                line: 3,
            },
            Heading {
                text: "Sibling".into(),
                level: 2,
                line: 6,
            },
        ];
        let err = validate_disjoint(&[1, 3], &headings).unwrap_err();
        match err {
            Error::Notes(msg) => {
                assert!(msg.contains("line 3"));
                assert!(msg.contains("line 1"));
            }
            _ => panic!("expected Notes error"),
        }
    }

    #[test]
    fn disjoint_duplicate_line_errors() {
        let headings = vec![Heading {
            text: "A".into(),
            level: 2,
            line: 1,
        }];
        let err = validate_disjoint(&[1, 1], &headings).unwrap_err();
        assert!(matches!(err, Error::Notes(_)));
    }

    // ── move_sections ────────────────────────────────────────────────────

    #[test]
    fn move_single_section_preserving_level() {
        let source = "\
## Keep me
keep body
## Move me
move body
";
        let target = "# Target\nbefore\n";
        let picks = [SectionPick {
            source_line: 3, // "## Move me"
            new_level: 2,
        }];
        let plan = [Placement {
            pick_idx: 0,
            after_line: Some(1), // after "# Target"
        }];
        let (new_source, new_target) = move_sections(source, &picks, target, &plan).unwrap();
        assert_eq!(new_source, "## Keep me\nkeep body\n");
        assert!(new_target.contains("## Move me\nmove body\n"));
        assert!(new_target.starts_with("# Target\nbefore\n## Move me"));
    }

    #[test]
    fn move_single_section_with_level_shift() {
        let source = "## Move\nbody\n### Nested\nnested\n";
        let target = "# Target\n";
        let picks = [SectionPick {
            source_line: 1,
            new_level: 3,
        }];
        let plan = [Placement {
            pick_idx: 0,
            after_line: Some(1),
        }];
        let (new_source, new_target) = move_sections(source, &picks, target, &plan).unwrap();
        assert_eq!(new_source, "");
        assert!(new_target.contains("### Move\nbody\n#### Nested\nnested\n"));
    }

    #[test]
    fn move_multiple_picks_preserves_relative_order() {
        let source = "\
## A
a body
## B
b body
## C
c body
";
        let target = "# T\n";
        let picks = [
            SectionPick {
                source_line: 1,
                new_level: 2,
            }, // A
            SectionPick {
                source_line: 5,
                new_level: 2,
            }, // C
        ];
        let plan = [
            Placement {
                pick_idx: 0,
                after_line: Some(1),
            },
            Placement {
                pick_idx: 1,
                after_line: Some(1),
            },
        ];
        let (new_source, new_target) = move_sections(source, &picks, target, &plan).unwrap();
        assert_eq!(new_source, "## B\nb body\n");
        // Both inserted after target line 1, in plan order.
        let a_idx = new_target.find("## A").unwrap();
        let c_idx = new_target.find("## C").unwrap();
        assert!(a_idx < c_idx);
    }

    #[test]
    fn move_insert_at_top_when_after_line_is_none() {
        let source = "## Move\nbody\n";
        let target = "# Existing\nexisting body\n";
        let picks = [SectionPick {
            source_line: 1,
            new_level: 2,
        }];
        let plan = [Placement {
            pick_idx: 0,
            after_line: None,
        }];
        let (_, new_target) = move_sections(source, &picks, target, &plan).unwrap();
        assert!(new_target.starts_with("## Move\nbody\n# Existing"));
    }

    #[test]
    fn move_insert_after_last_heading_appends() {
        let source = "## Move\nbody\n";
        let target = "# Existing\nexisting body\n";
        let picks = [SectionPick {
            source_line: 1,
            new_level: 2,
        }];
        let plan = [Placement {
            pick_idx: 0,
            after_line: Some(1),
        }];
        let (_, new_target) = move_sections(source, &picks, target, &plan).unwrap();
        assert_eq!(new_target, "# Existing\nexisting body\n## Move\nbody\n");
    }

    #[test]
    fn move_picks_empty_returns_unchanged() {
        let source = "## A\nbody\n";
        let target = "# T\n";
        let (s, t) = move_sections(source, &[], target, &[]).unwrap();
        assert_eq!(s, source);
        assert_eq!(t, target);
    }

    #[test]
    fn move_invalid_pick_line_errors() {
        let source = "## A\nbody\n";
        let target = "# T\n";
        let picks = [SectionPick {
            source_line: 99,
            new_level: 2,
        }];
        let plan = [Placement {
            pick_idx: 0,
            after_line: Some(1),
        }];
        let err = move_sections(source, &picks, target, &plan).unwrap_err();
        assert!(matches!(err, Error::Notes(_)));
    }

    #[test]
    fn move_invalid_after_line_errors() {
        let source = "## A\nbody\n";
        let target = "# T\n";
        let picks = [SectionPick {
            source_line: 1,
            new_level: 2,
        }];
        let plan = [Placement {
            pick_idx: 0,
            after_line: Some(99),
        }];
        let err = move_sections(source, &picks, target, &plan).unwrap_err();
        assert!(matches!(err, Error::Notes(_)));
    }

    #[test]
    fn move_disjoint_violation_errors() {
        let source = "## Parent\n### Child\nbody\n";
        let target = "# T\n";
        let picks = [
            SectionPick {
                source_line: 1,
                new_level: 2,
            },
            SectionPick {
                source_line: 2,
                new_level: 3,
            },
        ];
        let plan = [
            Placement {
                pick_idx: 0,
                after_line: Some(1),
            },
            Placement {
                pick_idx: 1,
                after_line: Some(1),
            },
        ];
        let err = move_sections(source, &picks, target, &plan).unwrap_err();
        assert!(matches!(err, Error::Notes(_)));
    }

    #[test]
    fn move_plan_length_mismatch_errors() {
        let source = "## A\nbody\n";
        let target = "# T\n";
        let picks = [SectionPick {
            source_line: 1,
            new_level: 2,
        }];
        let plan: [Placement; 0] = [];
        let err = move_sections(source, &picks, target, &plan).unwrap_err();
        assert!(matches!(err, Error::Notes(_)));
    }

    #[test]
    fn move_cascade_overflow_errors() {
        let source = "## Top\n###### Deep\n";
        let target = "# T\n";
        let picks = [SectionPick {
            source_line: 1,
            new_level: 3,
        }];
        let plan = [Placement {
            pick_idx: 0,
            after_line: Some(1),
        }];
        let err = move_sections(source, &picks, target, &plan).unwrap_err();
        assert!(matches!(err, Error::Notes(_)));
    }

    // ── write_pair ───────────────────────────────────────────────────────

    #[test]
    fn write_pair_writes_both_files() {
        let dir = assert_fs::TempDir::new().unwrap();
        let target_path = dir.path().join("target.md");
        let source_path = dir.path().join("source.md");
        std::fs::write(&target_path, "old target\n").unwrap();
        std::fs::write(&source_path, "old source\n").unwrap();
        write_pair(&target_path, "new target\n", &source_path, "new source\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&target_path).unwrap(),
            "new target\n"
        );
        assert_eq!(
            std::fs::read_to_string(&source_path).unwrap(),
            "new source\n"
        );
    }
}
