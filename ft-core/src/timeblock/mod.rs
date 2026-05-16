//! Day-planner timeblock model, parser, and serializer.
//!
//! A "timeblock" is one entry under a daily note's configurable section
//! heading (default `"Time Blocks"`), in the format compatible with
//! Obsidian's Day Planner plugin and with `blockary`:
//!
//! ```text
//! - HH:MM - HH:MM <desc>
//! - HH:MM - HH:MM <desc> @tag
//! - HH:MM - HH:MM <desc> @group/tag
//! - HH:MM - HH:MM <desc> @group/tag/subtag
//! ```
//!
//! The short form `- HH:MM <desc>` is also accepted; the parser fills in
//! `end = start + 30m` and marks [`Timeblock::end_explicit`] false so a
//! caller can choose whether to normalize on write. The serializer always
//! emits the explicit end time.
//!
//! Tags are extracted inline from `desc` for queryability but the original
//! `@…` substring stays in `desc` so round-trip writes preserve the user's
//! exact authoring.

use chrono::NaiveTime;
use thiserror::Error;

pub mod doc;
pub mod ops;
pub mod report;

/// One day-planner block. See the module-level documentation for the
/// supported source format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeblock {
    pub start: NaiveTime,
    /// Derived as `start + 30m` when the source line omits the end time.
    pub end: NaiveTime,
    /// `false` when the source line was the short form `- HH:MM <desc>`.
    /// [`serialize_line`] always emits an explicit end, so a freshly
    /// parsed block round-trips with `end_explicit == true`.
    pub end_explicit: bool,
    /// Description with `@tag` text preserved inline so round-trip writes
    /// keep the user's exact authoring.
    pub desc: String,
    /// Tags parsed from `desc`. Invalid `@…` tokens (bracket / parens /
    /// 4-deep levels) are skipped — the underlying text still lives in
    /// `desc`, it just isn't surfaced as a tag.
    pub tags: Vec<Tag>,
    /// 1-indexed position within the section block list. `0` when the
    /// block hasn't been placed in a [`doc::Document`] yet.
    pub source_line: usize,
}

/// Hierarchical tag with at most 3 levels (e.g. `@work/meeting/1on1`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Tag {
    /// 1..=3 levels. `@a/b/c` → `["a","b","c"]`.
    pub levels: Vec<String>,
}

impl Tag {
    /// Serialize back to `@a/b/c` form.
    pub fn to_string_form(&self) -> String {
        let mut s = String::from("@");
        for (i, l) in self.levels.iter().enumerate() {
            if i > 0 {
                s.push('/');
            }
            s.push_str(l);
        }
        s
    }
}

/// Structured parse error for [`parse_line`] and [`parse_tag_string`].
///
/// Note that [`parse_tags`] is lenient — it filters out tokens that would
/// produce these errors rather than surfacing them. The strict variants
/// here are used by [`parse_tag_string`] when validating a tag passed
/// via a CLI flag.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ParseError {
    #[error("expected `HH:MM` start time at position {pos}")]
    BadStart { pos: usize },
    #[error("expected `HH:MM` end time after `-` separator")]
    BadEnd,
    #[error("end time {end} is not after start {start}")]
    EndBeforeStart { start: String, end: String },
    #[error("tag has more than 3 levels")]
    TagTooDeep,
    #[error("tag contains disallowed character {ch:?}")]
    TagBadChar { ch: char },
    #[error("empty input")]
    Empty,
}

/// Parse one timeblock from a source line.
///
/// Accepts the canonical full form `HH:MM - HH:MM <desc>` and the short
/// form `HH:MM <desc>` (with `end = start + 30m`). A leading list marker
/// (`- ` / `* ` / `+ `) and surrounding whitespace are tolerated so callers
/// don't have to strip them.
///
/// Tag parsing is lenient: malformed `@…` substrings stay in `desc` but
/// are not promoted to [`Tag`]s. Use [`parse_tag_string`] when you need
/// strict tag validation (e.g. for `ft timeblocks edit --add-tag X`).
pub fn parse_line(input: &str) -> Result<Timeblock, ParseError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ParseError::Empty);
    }
    // Strip leading list marker (`- `, `* `, `+ `) so callers can hand us
    // either the raw `- HH:MM …` line or the post-marker substring.
    let body = strip_list_marker(trimmed);
    let body = body.trim_start();

    let (start, after_start) = parse_hhmm(body, 0)?;
    let rest = &body[after_start..];
    // Look for ` - HH:MM` after the start time.
    let (end, end_explicit, after_end) = match parse_dash_end(rest) {
        Some((end, consumed)) => {
            if end <= start {
                return Err(ParseError::EndBeforeStart {
                    start: format_hhmm(start),
                    end: format_hhmm(end),
                });
            }
            (end, true, consumed)
        }
        None => (start + chrono::Duration::minutes(30), false, 0),
    };

    let desc_raw = rest[after_end..].trim();
    let desc = desc_raw.to_string();
    let tags = parse_tags(&desc);

    Ok(Timeblock {
        start,
        end,
        end_explicit,
        desc,
        tags,
        source_line: 0,
    })
}

/// Serialize a timeblock in canonical form: `HH:MM - HH:MM <desc>` (no
/// leading list marker). [`doc::Document::write`] adds the `- ` prefix
/// when emitting the section.
pub fn serialize_line(b: &Timeblock) -> String {
    if b.desc.is_empty() {
        format!("{} - {}", format_hhmm(b.start), format_hhmm(b.end))
    } else {
        format!(
            "{} - {} {}",
            format_hhmm(b.start),
            format_hhmm(b.end),
            b.desc
        )
    }
}

/// Extract every well-formed `@tag` from `desc`. Malformed `@…` tokens
/// (brackets, parens, 4-deep levels, disallowed chars) are skipped — the
/// underlying text remains in `desc` but doesn't surface as a tag.
pub fn parse_tags(desc: &str) -> Vec<Tag> {
    parse_tags_collect(desc, false)
        .into_iter()
        .flatten()
        .collect()
}

/// Strict single-tag parser used by CLI flags like `--add-tag @work/meeting`.
/// The leading `@` is optional. Returns a [`ParseError`] on any violation.
pub fn parse_tag_string(s: &str) -> Result<Tag, ParseError> {
    let s = s.trim();
    let s = s.strip_prefix('@').unwrap_or(s);
    if s.is_empty() {
        return Err(ParseError::Empty);
    }
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() > 3 {
        return Err(ParseError::TagTooDeep);
    }
    let mut levels = Vec::with_capacity(parts.len());
    for level in &parts {
        if level.is_empty() {
            return Err(ParseError::Empty);
        }
        for c in level.chars() {
            if !is_tag_char(c) {
                return Err(ParseError::TagBadChar { ch: c });
            }
        }
        levels.push((*level).to_string());
    }
    Ok(Tag { levels })
}

// ── internals ──────────────────────────────────────────────────────────────

fn strip_list_marker(s: &str) -> &str {
    let trimmed = s.trim_start();
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return rest;
        }
    }
    // Bare `-`/`*`/`+` followed by tab also acceptable.
    for marker in ["-\t", "*\t", "+\t"] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return rest;
        }
    }
    trimmed
}

fn parse_hhmm(s: &str, pos: usize) -> Result<(NaiveTime, usize), ParseError> {
    // Require `HH:MM` with zero-padded two-digit hour and two-digit minute.
    let bytes = s.as_bytes();
    if bytes.len() < 5 {
        return Err(ParseError::BadStart { pos });
    }
    let h0 = bytes[0];
    let h1 = bytes[1];
    let colon = bytes[2];
    let m0 = bytes[3];
    let m1 = bytes[4];
    if !(h0.is_ascii_digit()
        && h1.is_ascii_digit()
        && colon == b':'
        && m0.is_ascii_digit()
        && m1.is_ascii_digit())
    {
        return Err(ParseError::BadStart { pos });
    }
    let h = (h0 - b'0') * 10 + (h1 - b'0');
    let m = (m0 - b'0') * 10 + (m1 - b'0');
    if h >= 24 || m >= 60 {
        return Err(ParseError::BadStart { pos });
    }
    let t = NaiveTime::from_hms_opt(h as u32, m as u32, 0).ok_or(ParseError::BadStart { pos })?;
    Ok((t, 5))
}

fn parse_dash_end(rest: &str) -> Option<(NaiveTime, usize)> {
    // Pattern: zero-or-more spaces, `-`, zero-or-more spaces, HH:MM.
    let mut idx = 0;
    let bytes = rest.as_bytes();
    while idx < bytes.len() && (bytes[idx] == b' ' || bytes[idx] == b'\t') {
        idx += 1;
    }
    if idx >= bytes.len() || bytes[idx] != b'-' {
        return None;
    }
    idx += 1;
    while idx < bytes.len() && (bytes[idx] == b' ' || bytes[idx] == b'\t') {
        idx += 1;
    }
    let (end, consumed) = parse_hhmm(&rest[idx..], idx).ok()?;
    Some((end, idx + consumed))
}

fn format_hhmm(t: NaiveTime) -> String {
    use chrono::Timelike;
    format!("{:02}:{:02}", t.hour(), t.minute())
}

fn is_tag_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Walk `desc` looking for `@…` substrings. Returns one entry per `@`
/// found — `Some(Tag)` when the token parses cleanly, `None` when it
/// doesn't (brackets, parens, 4-deep, disallowed chars, bare `@`).
///
/// `strict` is unused by the public API today but kept as a future hook
/// for a `parse_tags_strict(desc) -> Result<Vec<Tag>, ParseError>` should
/// the CLI need it.
fn parse_tags_collect(desc: &str, _strict: bool) -> Vec<Option<Tag>> {
    let chars: Vec<char> = desc.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '@' {
            i += 1;
            continue;
        }
        i += 1; // consume `@`
        let mut levels: Vec<String> = Vec::new();
        let mut malformed = false;
        loop {
            let mut level = String::new();
            while i < chars.len() {
                let c = chars[i];
                if c == '/' || c.is_whitespace() || c == '@' {
                    break;
                }
                if is_tag_char(c) {
                    level.push(c);
                    i += 1;
                } else {
                    malformed = true;
                    while i < chars.len() && !chars[i].is_whitespace() {
                        i += 1;
                    }
                    break;
                }
            }
            if malformed {
                break;
            }
            if level.is_empty() {
                malformed = true;
                break;
            }
            levels.push(level);
            if i < chars.len() && chars[i] == '/' {
                if levels.len() >= 3 {
                    malformed = true;
                    while i < chars.len() && !chars[i].is_whitespace() {
                        i += 1;
                    }
                    break;
                }
                i += 1; // consume `/`
                continue;
            }
            break;
        }
        if malformed || levels.is_empty() {
            out.push(None);
        } else {
            out.push(Some(Tag { levels }));
        }
    }
    out
}

// ── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    // ── parse_line: time grammar ─────────────────────────────────────────

    #[test]
    fn parses_full_form_with_desc() {
        let b = parse_line("10:00 - 11:30 Buy coffee").unwrap();
        assert_eq!(b.start, t(10, 0));
        assert_eq!(b.end, t(11, 30));
        assert!(b.end_explicit);
        assert_eq!(b.desc, "Buy coffee");
    }

    #[test]
    fn parses_short_form_with_derived_end() {
        let b = parse_line("10:00 Quick check").unwrap();
        assert_eq!(b.start, t(10, 0));
        assert_eq!(b.end, t(10, 30));
        assert!(!b.end_explicit);
        assert_eq!(b.desc, "Quick check");
    }

    #[test]
    fn tolerates_leading_list_marker() {
        let b = parse_line("- 08:00 - 09:00 Standup").unwrap();
        assert_eq!(b.start, t(8, 0));
        assert_eq!(b.end, t(9, 0));
        assert_eq!(b.desc, "Standup");

        let b = parse_line("  *  09:00 - 10:00 Pair").unwrap();
        assert_eq!(b.start, t(9, 0));
        assert_eq!(b.desc, "Pair");
    }

    #[test]
    fn allows_empty_description() {
        let b = parse_line("10:00 - 10:30").unwrap();
        assert_eq!(b.desc, "");
        assert!(b.tags.is_empty());
    }

    #[test]
    fn description_can_start_with_digit() {
        let b = parse_line("10:00 - 11:00 1on1 with Hans").unwrap();
        assert_eq!(b.desc, "1on1 with Hans");
    }

    #[test]
    fn rejects_missing_start_time() {
        assert!(matches!(
            parse_line("desc only"),
            Err(ParseError::BadStart { .. })
        ));
    }

    #[test]
    fn rejects_non_zero_padded_hour() {
        // `9:00` is rejected — canonical format is `HH:MM`.
        assert!(matches!(
            parse_line("9:00 - 10:00 nope"),
            Err(ParseError::BadStart { .. })
        ));
    }

    #[test]
    fn rejects_out_of_range_time() {
        assert!(matches!(
            parse_line("25:00 - 26:00 nope"),
            Err(ParseError::BadStart { .. })
        ));
        assert!(matches!(
            parse_line("12:60 - 13:00 nope"),
            Err(ParseError::BadStart { .. })
        ));
    }

    #[test]
    fn rejects_end_at_or_before_start() {
        assert!(matches!(
            parse_line("11:00 - 11:00 zero-length"),
            Err(ParseError::EndBeforeStart { .. })
        ));
        assert!(matches!(
            parse_line("11:00 - 10:00 backwards"),
            Err(ParseError::EndBeforeStart { .. })
        ));
    }

    #[test]
    fn dash_without_end_falls_back_to_desc() {
        // `10:00 - foo` has no second HH:MM, so the dash is treated as
        // part of the description. We currently report this as a desc
        // starting with `- foo` (start = 10:00, end = 10:30 derived).
        let b = parse_line("10:00 - foo").unwrap();
        assert_eq!(b.start, t(10, 0));
        assert!(!b.end_explicit);
        assert_eq!(b.desc, "- foo");
    }

    #[test]
    fn empty_input_is_empty_error() {
        assert!(matches!(parse_line(""), Err(ParseError::Empty)));
        assert!(matches!(parse_line("   "), Err(ParseError::Empty)));
    }

    // ── parse_tags: lenient inline tag extraction ────────────────────────

    #[test]
    fn parses_one_level_tag() {
        let b = parse_line("10:00 - 11:00 Buy coffee @chores").unwrap();
        assert_eq!(b.tags.len(), 1);
        assert_eq!(b.tags[0].levels, vec!["chores"]);
    }

    #[test]
    fn parses_three_level_tag() {
        let b = parse_line("10:00 - 11:00 do thing @work/proj/sub").unwrap();
        assert_eq!(b.tags[0].levels, vec!["work", "proj", "sub"]);
    }

    #[test]
    fn parses_multiple_tags() {
        let b = parse_line("10:00 - 11:00 Plan @work/meeting @personal").unwrap();
        assert_eq!(b.tags.len(), 2);
        assert_eq!(b.tags[0].levels, vec!["work", "meeting"]);
        assert_eq!(b.tags[1].levels, vec!["personal"]);
    }

    #[test]
    fn four_level_tag_is_skipped_not_errored() {
        // Lenient inline parser: malformed tag stays in desc but isn't
        // surfaced as a Tag. Strict validation lives in parse_tag_string.
        let b = parse_line("10:00 - 11:00 thing @a/b/c/d more").unwrap();
        assert_eq!(b.desc, "thing @a/b/c/d more");
        assert!(b.tags.is_empty());
    }

    #[test]
    fn bracket_tag_is_skipped_not_errored() {
        let b = parse_line("10:00 - 11:00 thing @p/[[Proj]]/x").unwrap();
        assert_eq!(b.desc, "thing @p/[[Proj]]/x");
        assert!(b.tags.is_empty());
    }

    #[test]
    fn paren_tag_is_skipped_not_errored() {
        let b = parse_line("10:00 - 11:00 thing @p/(Hi)/x").unwrap();
        assert!(b.tags.is_empty());
    }

    #[test]
    fn email_at_token_is_skipped() {
        // `@example.com` has `.` which isn't a valid tag char — drop it.
        let b = parse_line("10:00 - 11:00 mail @example.com about thing").unwrap();
        assert!(b.tags.is_empty());
    }

    #[test]
    fn bare_at_sign_is_ignored() {
        let b = parse_line("10:00 - 11:00 hello @ world").unwrap();
        assert!(b.tags.is_empty());
    }

    // ── parse_tag_string: strict validation ──────────────────────────────

    #[test]
    fn strict_tag_string_parses_with_and_without_prefix() {
        assert_eq!(parse_tag_string("@work").unwrap().levels, vec!["work"]);
        assert_eq!(parse_tag_string("work").unwrap().levels, vec!["work"]);
        assert_eq!(
            parse_tag_string("@a/b/c").unwrap().levels,
            vec!["a", "b", "c"]
        );
    }

    #[test]
    fn strict_tag_string_rejects_four_levels() {
        assert_eq!(parse_tag_string("a/b/c/d"), Err(ParseError::TagTooDeep));
    }

    #[test]
    fn strict_tag_string_rejects_brackets() {
        assert!(matches!(
            parse_tag_string("a/[[x]]"),
            Err(ParseError::TagBadChar { ch: '[' })
        ));
    }

    #[test]
    fn strict_tag_string_rejects_empty_level() {
        assert_eq!(parse_tag_string("a//b"), Err(ParseError::Empty));
        assert_eq!(parse_tag_string(""), Err(ParseError::Empty));
        assert_eq!(parse_tag_string("@"), Err(ParseError::Empty));
    }

    // ── serialize_line / round-trip ──────────────────────────────────────

    #[test]
    fn serialize_emits_canonical_form() {
        let b = Timeblock {
            start: t(9, 5),
            end: t(10, 0),
            end_explicit: true,
            desc: "Standup".into(),
            tags: vec![],
            source_line: 0,
        };
        assert_eq!(serialize_line(&b), "09:05 - 10:00 Standup");
    }

    #[test]
    fn serialize_normalizes_short_form_to_full_form() {
        let b = parse_line("10:00 quick").unwrap();
        assert_eq!(serialize_line(&b), "10:00 - 10:30 quick");
    }

    #[test]
    fn serialize_empty_desc_emits_no_trailing_space() {
        let b = Timeblock {
            start: t(9, 0),
            end: t(9, 30),
            end_explicit: true,
            desc: String::new(),
            tags: vec![],
            source_line: 0,
        };
        assert_eq!(serialize_line(&b), "09:00 - 09:30");
    }

    #[test]
    fn serialize_preserves_inline_tag_text() {
        let b = parse_line("10:00 - 11:00 review @work/code").unwrap();
        assert_eq!(serialize_line(&b), "10:00 - 11:00 review @work/code");
    }

    // proptest: serialize-then-parse-then-serialize is idempotent on the
    // serialized form. We don't claim block-level byte equality because
    // `end_explicit` may flip from false → true after the first
    // serialization.
    proptest! {
        #[test]
        fn round_trip_serialize_parse_serialize(
            h1 in 0u32..23, m1 in 0u32..59,
            dur in 1u32..60,
            desc in "[a-zA-Z][a-zA-Z0-9 ]{0,20}",
        ) {
            let start = NaiveTime::from_hms_opt(h1, m1, 0).unwrap();
            let total = h1 * 60 + m1 + dur;
            prop_assume!(total < 24 * 60);
            let end = NaiveTime::from_hms_opt(total / 60, total % 60, 0).unwrap();
            let b = Timeblock {
                start, end,
                end_explicit: true,
                desc: desc.trim().to_string(),
                tags: vec![],
                source_line: 0,
            };
            let s1 = serialize_line(&b);
            let parsed = parse_line(&s1).unwrap();
            let s2 = serialize_line(&parsed);
            prop_assert_eq!(s1, s2);
        }
    }

    // ── Tag::to_string_form ──────────────────────────────────────────────

    #[test]
    fn tag_string_form_roundtrips_with_parse() {
        for input in ["@work", "@work/meeting", "@a/b/c"] {
            let t = parse_tag_string(input).unwrap();
            assert_eq!(t.to_string_form(), input);
        }
    }
}
