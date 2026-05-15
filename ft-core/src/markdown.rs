//! Markdown structure parsers used by the search layer.
//!
//! Today this module only ships a heading extractor used by
//! [`crate::search`]. The task line parser lives in [`crate::task::emoji`] —
//! the two are kept separate because they answer different questions
//! (`- [ ]` lines vs `#` headings) and a future contributor wiring up,
//! say, a backlink resolver should be able to add markdown helpers here
//! without touching the task code.

/// A markdown heading found inside a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    pub text: String,
    /// ATX level — 1 for `#`, 2 for `##`, … up to 6.
    pub level: u8,
    /// 1-indexed line number within the source file.
    pub line: usize,
}

/// Extract every ATX heading (`#` … `######`) from `content`.
///
/// Headings inside fenced code blocks (``` and ~~~), inside indented
/// code blocks (4-space indent at column 0), and inside the leading
/// YAML/TOML frontmatter (the `---` block at the very top of the file)
/// are skipped. Setext headings (`===` / `---` underlines) are out of
/// scope — they're rare in modern Obsidian vaults.
pub fn extract_headings(content: &str) -> Vec<Heading> {
    let mut out = Vec::new();
    let mut state = LineSkipState::new();

    for (idx, line) in content.lines().enumerate() {
        let lineno = idx + 1;
        if state.skip_line(line) {
            continue;
        }
        if let Some(h) = parse_atx(line, lineno) {
            out.push(h);
        }
    }
    out
}

/// Tracks frontmatter / fenced code block / indented code block state
/// across a line-by-line scan of a markdown file. Both the heading
/// extractor (above) and the link parser (`crate::graph::parser`) use
/// this so the "what counts as content vs. structure" rules stay in
/// one place.
///
/// Inline code spans (single/double/triple backticks within a line)
/// are *not* handled here — they're a within-line concern that each
/// consumer handles with its own intra-line scanner. This struct only
/// answers the per-line question "should I skip this whole line?"
#[derive(Debug, Default)]
pub(crate) struct LineSkipState {
    /// Are we still inside the leading frontmatter block? Set on the
    /// first line if it's `---`; cleared when we hit the closing `---`.
    in_frontmatter: bool,
    /// Have we seen any line yet? Used to detect the frontmatter opener
    /// — frontmatter only counts when `---` is the very first line.
    started: bool,
    /// Fence character active for a fenced code block: `'`'` or `'~'`.
    /// `None` when we're not inside a fenced block.
    fence: Option<char>,
    /// Number of fence chars the opener used. The closer needs to match
    /// or exceed this count (per CommonMark).
    fence_len: usize,
}

impl LineSkipState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Advance one line. Returns `true` when this line is structural
    /// (frontmatter delimiter, frontmatter body, code-fence delimiter,
    /// inside a fenced code block, or an indented code block) and
    /// should be skipped by the consumer; `false` when this line
    /// carries content the consumer should examine.
    pub(crate) fn skip_line(&mut self, line: &str) -> bool {
        // Frontmatter handling: only relevant on line 1 and during the
        // block. CommonMark doesn't define frontmatter; we follow the
        // Obsidian / Jekyll convention of a `---` block at the very top.
        if !self.started {
            self.started = true;
            if line.trim_end() == "---" {
                self.in_frontmatter = true;
                return true;
            }
        } else if self.in_frontmatter {
            if line.trim_end() == "---" || line.trim_end() == "..." {
                self.in_frontmatter = false;
            }
            return true;
        }

        // Fenced code blocks: opening fence pattern is N≥3 of `'`'` or
        // `'~'` chars at the start of the line (possibly preceded by up
        // to 3 spaces of indent, per CommonMark — we accept any leading
        // whitespace for robustness).
        let trimmed = line.trim_start();
        if let Some(fence_char) = self.fence {
            // Inside a fence — only the matching close fence ends it.
            if let Some((c, n)) = leading_fence(trimmed) {
                if c == fence_char && n >= self.fence_len {
                    self.fence = None;
                    self.fence_len = 0;
                }
            }
            return true;
        }
        if let Some((c, n)) = leading_fence(trimmed) {
            self.fence = Some(c);
            self.fence_len = n;
            return true;
        }

        // Indented code block: 4+ leading spaces (or a tab) and we're
        // not inside a list context. Without a full block parser we
        // approximate by skipping any 4-space-indented line. False
        // positives on deeply-nested list items are accepted in v1;
        // they would never start with `#` to begin with.
        if starts_with_indent(line, 4) {
            return true;
        }

        false
    }
}

/// Detect a fenced code block opener / closer at the start of `s`. Returns
/// the fence char (`'`'` or `'~'`) and the number of consecutive fence
/// chars when 3 or more are present, otherwise `None`.
pub(crate) fn leading_fence(s: &str) -> Option<(char, usize)> {
    let first = s.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let n = s.chars().take_while(|c| *c == first).count();
    (n >= 3).then_some((first, n))
}

/// True if `line` starts with at least `n` columns of whitespace (a tab
/// counts as advancing to the next multiple of 4, per CommonMark).
fn starts_with_indent(line: &str, n: usize) -> bool {
    let mut col = 0usize;
    for c in line.chars() {
        match c {
            ' ' => col += 1,
            '\t' => col = (col / 4 + 1) * 4,
            _ => return col >= n,
        }
        if col >= n {
            return true;
        }
    }
    false
}

/// Parse an ATX heading from `line` if it matches the pattern; `lineno`
/// is the 1-indexed source line.
fn parse_atx(line: &str, lineno: usize) -> Option<Heading> {
    let trimmed = line.trim_start();
    let level = trimmed.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let after = &trimmed[level..];
    // CommonMark requires a space or end-of-line after the `#` run.
    if !after.is_empty() && !after.starts_with(|c: char| c.is_whitespace()) {
        return None;
    }
    let mut text = after.trim().to_string();
    // CommonMark: closing `#`s are optional and stripped (along with the
    // single space that separates them from the heading text).
    while text.ends_with('#') {
        text.pop();
    }
    let text = text.trim_end().to_string();
    Some(Heading {
        text,
        level: level as u8,
        line: lineno,
    })
}

// ── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_atx_levels_one_through_six() {
        let body = "\
# H1
## H2
### H3
#### H4
##### H5
###### H6
####### not a heading
";
        let headings = extract_headings(body);
        assert_eq!(headings.len(), 6);
        for (i, h) in headings.iter().enumerate() {
            assert_eq!(h.level as usize, i + 1);
            assert_eq!(h.text, format!("H{}", i + 1));
            assert_eq!(h.line, i + 1);
        }
    }

    #[test]
    fn skips_headings_in_fenced_code_blocks_backticks() {
        let body = "\
# Real heading
```rust
# fake heading inside backtick fence
## also fake
```
## Real again
";
        let headings = extract_headings(body);
        assert_eq!(headings.len(), 2);
        assert_eq!(headings[0].text, "Real heading");
        assert_eq!(headings[1].text, "Real again");
        assert_eq!(headings[1].line, 6);
    }

    #[test]
    fn skips_headings_in_fenced_code_blocks_tildes() {
        let body = "\
~~~
# fake
~~~
# real
";
        let headings = extract_headings(body);
        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].text, "real");
    }

    #[test]
    fn skips_indented_code_blocks() {
        // NB: don't use `"\<newline>"` continuation here — it eats the
        // leading whitespace of the next line, defeating the test.
        let body = "    # not a heading (4-space indent)\n\
                    \t# also not a heading (tab indent)\n\
                    # real heading\n";
        let headings = extract_headings(body);
        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].text, "real heading");
    }

    #[test]
    fn skips_frontmatter_block() {
        let body = "\
---
title: Foo
# this is yaml, not a heading
---
# Actual heading
";
        let headings = extract_headings(body);
        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].text, "Actual heading");
        assert_eq!(headings[0].line, 5);
    }

    #[test]
    fn frontmatter_only_counts_at_file_top() {
        let body = "\
some prose
---
title: not frontmatter
---
# heading
";
        let headings = extract_headings(body);
        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].text, "heading");
    }

    #[test]
    fn rejects_hash_without_space() {
        let body = "\
#nospace not a heading
# spaced is a heading
";
        let headings = extract_headings(body);
        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].text, "spaced is a heading");
    }

    #[test]
    fn strips_trailing_hashes() {
        let body = "\
# Hello ###
## Goodbye ##
";
        let headings = extract_headings(body);
        assert_eq!(headings[0].text, "Hello");
        assert_eq!(headings[1].text, "Goodbye");
    }

    #[test]
    fn empty_input_returns_empty_vec() {
        assert_eq!(extract_headings(""), Vec::<Heading>::new());
    }

    #[test]
    fn heading_with_no_text_is_kept_as_empty_string() {
        let body = "# \n## also empty\n";
        let headings = extract_headings(body);
        assert_eq!(headings.len(), 2);
        assert_eq!(headings[0].text, "");
        assert_eq!(headings[1].text, "also empty");
    }
}
