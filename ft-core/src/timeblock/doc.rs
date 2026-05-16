//! [`Document`] — one day's timeblock section in a daily note.
//!
//! [`Document::read`] parses out the blocks under the configured heading;
//! [`Document::write`] performs an atomic section-replace via
//! [`crate::fs::write_atomic`], preserving everything outside the target
//! section byte-for-byte (modulo the trailing newline normalization
//! described below).

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::fs::write_atomic;
use crate::markdown::LineSkipState;

use super::{parse_line, serialize_line, Timeblock};

/// In-memory view of the timeblock section in one daily note. Construct
/// with [`Document::read`]; mutate `blocks` directly or via the helpers
/// in [`super::ops`]; persist with [`Document::write`].
#[derive(Debug, Clone)]
pub struct Document {
    pub blocks: Vec<Timeblock>,
    /// Heading text the document was read under (without `#` markers).
    pub heading: String,
    /// Absolute path of the daily note.
    pub source_path: PathBuf,
    /// Full file content captured at read time. [`Document::write`]
    /// uses this as the substrate it splices the new section into.
    pub source_content: String,
    /// `#` count of the heading in the source file. Defaults to 2 when
    /// the heading is absent (i.e. `write` will append `## Heading`).
    pub heading_level: u8,
    /// `true` when [`Document::read`] found the heading in the source
    /// file. `false` means [`Document::write`] will append the heading
    /// at file end.
    pub heading_present: bool,
}

impl Document {
    /// Read the daily note at `daily_path` and parse every timeblock under
    /// the heading matching `heading` (case-insensitive on heading text,
    /// any ATX level matches).
    ///
    /// When the file or heading is missing, returns a [`Document`] with
    /// `blocks: vec![]` — [`Document::write`] will then create the file
    /// and/or append the heading on first write.
    pub fn read(daily_path: &Path, heading: &str) -> Result<Document> {
        let source_content = match std::fs::read_to_string(daily_path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                return Err(Error::Io {
                    path: daily_path.to_path_buf(),
                    source: e,
                })
            }
        };

        let Section {
            heading_present,
            heading_level,
            body_lines,
        } = locate_section(&source_content, heading);

        let mut blocks = Vec::new();
        for line in &body_lines {
            // A timeblock line must start (after optional indent) with a
            // list marker. Anything else under the heading is treated as
            // prose and dropped on write — matching blockary.
            let trimmed = line.trim_start();
            let after_marker = if let Some(rest) = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("* "))
                .or_else(|| trimmed.strip_prefix("+ "))
            {
                rest
            } else {
                continue;
            };
            if let Ok(b) = parse_line(after_marker) {
                blocks.push(b);
            }
        }
        // Sort by start time; assign 1-indexed source_line by display
        // order so the [`Line(N)`] selector matches what the user sees.
        blocks.sort_by_key(|b| b.start);
        for (i, b) in blocks.iter_mut().enumerate() {
            b.source_line = i + 1;
        }

        Ok(Document {
            blocks,
            heading: heading.to_string(),
            source_path: daily_path.to_path_buf(),
            source_content,
            heading_level: if heading_present { heading_level } else { 2 },
            heading_present,
        })
    }

    /// Render what [`Self::write`] would write, without touching disk.
    /// Used by `--dry-run` to produce a diff against the original.
    pub fn render(&self) -> String {
        render_section_replace(
            &self.source_content,
            &self.heading,
            self.heading_level,
            self.heading_present,
            &self.blocks,
        )
    }

    /// Write the document back to `source_path` atomically. Re-sorts
    /// blocks by start time before serialization.
    pub fn write(&self) -> Result<()> {
        let content = self.render();
        write_atomic(&self.source_path, &content)
    }
}

struct Section {
    heading_present: bool,
    heading_level: u8,
    body_lines: Vec<String>,
}

/// Find the timeblock section under `heading`. Returns the lines between
/// the heading and the next heading of equal-or-higher level (or EOF).
fn locate_section(content: &str, heading: &str) -> Section {
    let mut state = LineSkipState::new();
    let mut heading_level: u8 = 0;
    let mut in_section = false;
    let mut body_lines: Vec<String> = Vec::new();
    let mut heading_present = false;
    let target = heading.trim().to_lowercase();

    for line in content.lines() {
        let skip = state.skip_line(line);
        if skip {
            // Inside fenced code / frontmatter: never a heading
            // boundary, and never a block candidate either — block-
            // shaped lines inside a fence are illustrative, not real.
            // Drop them.
            continue;
        }
        if let Some((level, text)) = parse_atx_heading(line) {
            if !in_section {
                if text.trim().to_lowercase() == target {
                    in_section = true;
                    heading_level = level;
                    heading_present = true;
                    continue;
                }
            } else if level <= heading_level {
                // hit the boundary — stop collecting
                break;
            } else {
                // a deeper sub-heading inside the section — keep it as
                // part of the body so it survives... wait, we don't.
                // Section-replace drops it. Keep it in body_lines for
                // completeness but the read filter discards it.
                body_lines.push(line.to_string());
                continue;
            }
        }
        if in_section {
            body_lines.push(line.to_string());
        }
    }

    Section {
        heading_present,
        heading_level: if heading_present { heading_level } else { 2 },
        body_lines,
    }
}

/// Parse `# heading` / `## heading` / etc. Returns `(level, text)` when
/// the line is an ATX heading (1-6 `#`s followed by whitespace or EOL).
/// `text` has trailing `#`s and whitespace stripped, matching the rule
/// in [`crate::markdown::extract_headings`].
fn parse_atx_heading(line: &str) -> Option<(u8, String)> {
    let trimmed = line.trim_start();
    let n = trimmed.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&n) {
        return None;
    }
    let after = &trimmed[n..];
    if !after.is_empty() && !after.starts_with(|c: char| c.is_whitespace()) {
        return None;
    }
    let mut text = after.trim().to_string();
    while text.ends_with('#') {
        text.pop();
    }
    Some((n as u8, text.trim_end().to_string()))
}

/// Produce the new file content by replacing the targeted section's body
/// with freshly-serialized blocks. When the heading is missing, append
/// `## <heading>\n\n<blocks>\n` at file end.
fn render_section_replace(
    source: &str,
    heading: &str,
    heading_level: u8,
    heading_present: bool,
    blocks: &[Timeblock],
) -> String {
    // Sort blocks defensively so the file always reflects ascending
    // start times even if a caller hands us out-of-order input.
    let mut sorted: Vec<&Timeblock> = blocks.iter().collect();
    sorted.sort_by_key(|b| b.start);

    let trailing_nl = source.ends_with('\n') || source.is_empty();
    let block_text = serialize_block_lines(&sorted);

    if !heading_present {
        // Append a new section at file end.
        let mut out = source.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        let hashes = "#".repeat(heading_level as usize);
        out.push_str(&hashes);
        out.push(' ');
        out.push_str(heading);
        out.push_str("\n\n");
        out.push_str(&block_text);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        return out;
    }

    // Section exists — splice. Walk lines, write everything outside the
    // section verbatim; at the heading line, emit heading + blank + blocks
    // + blank, then skip until we see a boundary heading.
    let target = heading.trim().to_lowercase();
    let mut state = LineSkipState::new();
    let mut out = String::with_capacity(source.len());
    let mut in_section = false;
    let mut found_boundary = false;

    for line in source.lines() {
        let skip = state.skip_line(line);
        if skip {
            if in_section && !found_boundary {
                // drop section body
                continue;
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if let Some((level, text)) = parse_atx_heading(line) {
            if !in_section && text.trim().to_lowercase() == target {
                in_section = true;
                // emit heading + blank + blocks + blank
                out.push_str(line);
                out.push('\n');
                out.push('\n');
                out.push_str(&block_text);
                out.push('\n');
                continue;
            }
            if in_section && !found_boundary && level <= heading_level {
                found_boundary = true;
                out.push_str(line);
                out.push('\n');
                continue;
            }
        }
        if in_section && !found_boundary {
            // drop section body (incl. nested deeper headings & prose)
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }

    // Preserve the original trailing-newline convention. `str::lines` strips
    // a trailing newline; the loop above re-adds one per line. If the
    // source didn't end with a newline, drop the final one we added.
    if !trailing_nl && out.ends_with('\n') {
        out.pop();
    }
    out
}

fn serialize_block_lines(blocks: &[&Timeblock]) -> String {
    let mut s = String::new();
    for b in blocks {
        s.push_str("- ");
        s.push_str(&serialize_line(b));
        s.push('\n');
    }
    s
}

// ── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;
    use assert_fs::TempDir;

    fn write_file(dir: &TempDir, name: &str, body: &str) -> PathBuf {
        let f = dir.child(name);
        f.write_str(body).unwrap();
        f.path().to_path_buf()
    }

    #[test]
    fn read_returns_empty_blocks_when_file_missing() {
        let tmp = TempDir::new().unwrap();
        let doc = Document::read(&tmp.path().join("missing.md"), "Time Blocks").unwrap();
        assert!(doc.blocks.is_empty());
        assert!(!doc.heading_present);
    }

    #[test]
    fn read_returns_empty_blocks_when_heading_missing() {
        let tmp = TempDir::new().unwrap();
        let p = write_file(&tmp, "day.md", "# Day\n\nNothing here yet.\n");
        let doc = Document::read(&p, "Time Blocks").unwrap();
        assert!(doc.blocks.is_empty());
        assert!(!doc.heading_present);
    }

    #[test]
    fn read_parses_blocks_under_heading() {
        let tmp = TempDir::new().unwrap();
        let p = write_file(
            &tmp,
            "day.md",
            "# Day\n\n## Time Blocks\n- 09:00 - 10:00 standup\n- 10:00 - 11:00 review @work\n\n## Notes\n- skip this\n",
        );
        let doc = Document::read(&p, "Time Blocks").unwrap();
        assert_eq!(doc.blocks.len(), 2);
        assert_eq!(doc.blocks[0].desc, "standup");
        assert_eq!(doc.blocks[1].desc, "review @work");
        assert!(doc.heading_present);
        assert_eq!(doc.heading_level, 2);
        // 1-indexed source_line by display order
        assert_eq!(doc.blocks[0].source_line, 1);
        assert_eq!(doc.blocks[1].source_line, 2);
    }

    #[test]
    fn read_is_case_insensitive_and_level_insensitive() {
        let tmp = TempDir::new().unwrap();
        let p = write_file(&tmp, "day.md", "### time blocks\n- 09:00 - 10:00 foo\n");
        let doc = Document::read(&p, "Time Blocks").unwrap();
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.heading_level, 3);
    }

    #[test]
    fn read_ignores_blocks_inside_fenced_code() {
        let tmp = TempDir::new().unwrap();
        let p = write_file(
            &tmp,
            "day.md",
            "## Time Blocks\n```\n- 09:00 - 10:00 fenced not real\n```\n- 10:00 - 11:00 real\n",
        );
        let doc = Document::read(&p, "Time Blocks").unwrap();
        // Both lines appear under the heading, but the one inside the
        // fence is skipped by LineSkipState before we try to parse it.
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.blocks[0].desc, "real");
    }

    #[test]
    fn read_stops_at_equal_or_higher_heading() {
        let tmp = TempDir::new().unwrap();
        let p = write_file(
            &tmp,
            "day.md",
            "## Time Blocks\n- 09:00 - 10:00 in section\n## Next\n- 10:00 - 11:00 out\n",
        );
        let doc = Document::read(&p, "Time Blocks").unwrap();
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.blocks[0].desc, "in section");
    }

    #[test]
    fn read_descends_into_sub_headings_in_section() {
        // A `### Sub` heading under `## Time Blocks` is deeper, so it
        // stays "inside" the section. Sub headings get discarded on
        // write (section-replace) — read filters them out via the
        // list-marker check.
        let tmp = TempDir::new().unwrap();
        let p = write_file(
            &tmp,
            "day.md",
            "## Time Blocks\n- 09:00 - 10:00 a\n### Sub\n- 10:00 - 11:00 b\n",
        );
        let doc = Document::read(&p, "Time Blocks").unwrap();
        assert_eq!(doc.blocks.len(), 2);
    }

    #[test]
    fn read_sorts_out_of_order_blocks_by_start_time() {
        let tmp = TempDir::new().unwrap();
        let p = write_file(
            &tmp,
            "day.md",
            "## Time Blocks\n- 11:00 - 12:00 second\n- 09:00 - 10:00 first\n",
        );
        let doc = Document::read(&p, "Time Blocks").unwrap();
        assert_eq!(doc.blocks[0].desc, "first");
        assert_eq!(doc.blocks[1].desc, "second");
    }

    #[test]
    fn write_appends_heading_when_missing() {
        let tmp = TempDir::new().unwrap();
        let p = write_file(&tmp, "day.md", "# Day\n\nprose\n");
        let mut doc = Document::read(&p, "Time Blocks").unwrap();
        doc.blocks.push(parse_line("09:00 - 10:00 first").unwrap());
        doc.write().unwrap();
        let body = std::fs::read_to_string(&p).unwrap();
        assert!(body.contains("## Time Blocks\n"));
        assert!(body.contains("- 09:00 - 10:00 first"));
        assert!(body.starts_with("# Day\n\nprose\n"));
    }

    #[test]
    fn write_replaces_existing_section_in_place() {
        let tmp = TempDir::new().unwrap();
        let p = write_file(
            &tmp,
            "day.md",
            "# Day\n\n## Time Blocks\n- 09:00 - 10:00 old\n\n## Notes\nkeep\n",
        );
        let mut doc = Document::read(&p, "Time Blocks").unwrap();
        doc.blocks.clear();
        doc.blocks.push(parse_line("10:00 - 11:00 new").unwrap());
        doc.write().unwrap();
        let body = std::fs::read_to_string(&p).unwrap();
        assert!(body.contains("- 10:00 - 11:00 new"));
        assert!(!body.contains("old"));
        assert!(body.contains("## Notes"));
        assert!(body.contains("keep"));
    }

    #[test]
    fn write_preserves_content_outside_section() {
        let tmp = TempDir::new().unwrap();
        let original = "# Day\n\nintro prose\n\n## Time Blocks\n- 09:00 - 10:00 old\n\n## Notes\n- keep me\n\n## Trailing\nfinal\n";
        let p = write_file(&tmp, "day.md", original);
        let mut doc = Document::read(&p, "Time Blocks").unwrap();
        doc.blocks.clear();
        doc.blocks.push(parse_line("08:00 - 09:00 fresh").unwrap());
        doc.write().unwrap();
        let body = std::fs::read_to_string(&p).unwrap();
        assert!(body.starts_with("# Day\n\nintro prose\n"));
        assert!(body.contains("- keep me"));
        assert!(body.contains("## Trailing\nfinal\n"));
    }

    #[test]
    fn write_is_atomic_via_write_atomic() {
        // Indirect: write_atomic is exercised by the success path above.
        // The contract is that the destination is replaced, not appended
        // — a second write with a different block list should leave only
        // the new state on disk.
        let tmp = TempDir::new().unwrap();
        let p = write_file(&tmp, "day.md", "## Time Blocks\n- 09:00 - 10:00 a\n");
        let mut doc = Document::read(&p, "Time Blocks").unwrap();
        doc.blocks.clear();
        doc.blocks.push(parse_line("11:00 - 12:00 z").unwrap());
        doc.write().unwrap();
        let body = std::fs::read_to_string(&p).unwrap();
        assert!(!body.contains("09:00"));
        assert!(body.contains("11:00 - 12:00 z"));
    }

    #[test]
    fn write_creates_file_when_missing() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("fresh.md");
        let mut doc = Document::read(&p, "Time Blocks").unwrap();
        doc.blocks.push(parse_line("09:00 - 10:00 first").unwrap());
        doc.write().unwrap();
        assert!(p.exists());
        let body = std::fs::read_to_string(&p).unwrap();
        assert!(body.contains("## Time Blocks"));
        assert!(body.contains("- 09:00 - 10:00 first"));
    }

    #[test]
    fn render_produces_what_write_would_write() {
        let tmp = TempDir::new().unwrap();
        let p = write_file(&tmp, "day.md", "## Time Blocks\n- 09:00 - 10:00 a\n");
        let mut doc = Document::read(&p, "Time Blocks").unwrap();
        doc.blocks.push(parse_line("10:00 - 11:00 b").unwrap());
        let rendered = doc.render();
        doc.write().unwrap();
        let body = std::fs::read_to_string(&p).unwrap();
        assert_eq!(rendered, body);
    }
}
