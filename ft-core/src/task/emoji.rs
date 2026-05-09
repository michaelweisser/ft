use chrono::NaiveDate;

use super::{
    format::{ParseContext, TaskFormat},
    Priority, Status, Task,
};

// ── emoji constants ──────────────────────────────────────────────────────────

const PRIORITY_HIGHEST: &str = "🔺";
const PRIORITY_HIGH: &str = "⏫";
const PRIORITY_MEDIUM: &str = "🔼";
const PRIORITY_LOW: &str = "🔽";
const PRIORITY_LOWEST: &str = "⏬";
const RECURRENCE: &str = "🔁";
const CREATED: &str = "➕";
const START: &str = "🛫";
const SCHEDULED: &str = "⏳";
const DUE: &str = "📅";
const DONE: &str = "✅";
const CANCELLED: &str = "❌";
const ID: &str = "🆔";
const DEPENDS_ON: &str = "⛔";

/// All field-marker emoji strings, used to detect field boundaries.
const ALL_FIELD_EMOJIS: &[&str] = &[
    PRIORITY_HIGHEST,
    PRIORITY_HIGH,
    PRIORITY_MEDIUM,
    PRIORITY_LOW,
    PRIORITY_LOWEST,
    RECURRENCE,
    CREATED,
    START,
    SCHEDULED,
    DUE,
    DONE,
    CANCELLED,
    ID,
    DEPENDS_ON,
];

// ── public format struct ─────────────────────────────────────────────────────

/// Emoji-format task parser / serializer, compatible with Obsidian Tasks
/// plugin v7.22 canonical output.
pub struct EmojiFormat;

impl TaskFormat for EmojiFormat {
    fn parse_line(&self, line: &str, ctx: ParseContext) -> Option<Task> {
        parse_line(line, ctx)
    }

    fn serialize_line(&self, task: &Task) -> String {
        serialize_line(task)
    }
}

// ── parser ───────────────────────────────────────────────────────────────────

fn parse_line(line: &str, ctx: ParseContext) -> Option<Task> {
    let trimmed_line = line.trim_end_matches('\n').trim_end_matches('\r');

    // Measure leading whitespace (indent) in bytes; spaces and tabs are
    // single-byte, so bytes == characters here.
    let content_start = trimmed_line
        .find(|c: char| !c.is_ascii_whitespace())
        .unwrap_or(trimmed_line.len());
    let indent_level = content_start;
    let s = &trimmed_line[content_start..];

    // Must start with "- [" to be a task line.
    if !s.starts_with("- [") {
        return None;
    }
    let s = &s[3..]; // after "- ["

    // Extract the single status character.
    let status_char = s.chars().next()?;
    let s = &s[status_char.len_utf8()..];

    // Must be followed by "] " (bracket + space).
    if !s.starts_with("] ") {
        return None;
    }
    let s = &s[2..]; // after "] "

    let status = match status_char {
        ' ' => Status::Open,
        'x' | 'X' => Status::Done,
        '/' => Status::InProgress,
        '-' => Status::Cancelled,
        c => {
            tracing::warn!(marker = %c, "unknown task status marker, treating as Open");
            Status::Open
        }
    };

    // Strip trailing block link " ^identifier" before further parsing.
    let (content, block_link) = strip_block_link(s);

    // Split content into description and fields.
    let boundary = fields_boundary(content);
    let description = content[..boundary].trim_end().to_string();
    let fields_str = content[boundary..].trim_start();

    // Parse the fields section.
    let mut fields = ParsedFields::default();
    parse_field_section(fields_str, &mut fields);

    // Extract inline hashtags from the description (convenience index).
    let tags = extract_tags(&description);

    Some(Task {
        description,
        status,
        priority: fields.priority,
        tags,
        created: fields.created,
        start: fields.start,
        scheduled: fields.scheduled,
        due: fields.due,
        done: fields.done,
        cancelled: fields.cancelled,
        recurrence: fields.recurrence,
        id: fields.id,
        depends_on: fields.depends_on,
        on_completion: None,
        block_link: block_link.map(str::to_string),
        raw_trailing: fields.raw_trailing,
        source_file: ctx.source_file,
        source_line: ctx.source_line,
        indent_level,
        parent: None,
    })
}

// ── serializer ───────────────────────────────────────────────────────────────

fn serialize_line(task: &Task) -> String {
    let indent = " ".repeat(task.indent_level);
    let status_char = match task.status {
        Status::Open => ' ',
        Status::Done => 'x',
        Status::InProgress => '/',
        Status::Cancelled => '-',
    };

    // Build the content portion: description then fields in canonical order.
    let mut parts: Vec<String> = vec![task.description.clone()];

    if let Some(priority) = task.priority {
        parts.push(priority.emoji().to_string());
    }
    if let Some(ref rec) = task.recurrence {
        parts.push(format!("{RECURRENCE} {rec}"));
    }
    if let Some(d) = task.created {
        parts.push(format!("{CREATED} {d}"));
    }
    if let Some(d) = task.start {
        parts.push(format!("{START} {d}"));
    }
    if let Some(d) = task.scheduled {
        parts.push(format!("{SCHEDULED} {d}"));
    }
    if let Some(d) = task.due {
        parts.push(format!("{DUE} {d}"));
    }
    if let Some(d) = task.done {
        parts.push(format!("{DONE} {d}"));
    }
    if let Some(d) = task.cancelled {
        parts.push(format!("{CANCELLED} {d}"));
    }
    if let Some(ref id) = task.id {
        parts.push(format!("{ID} {id}"));
    }
    if !task.depends_on.is_empty() {
        parts.push(format!("{DEPENDS_ON} {}", task.depends_on.join(",")));
    }
    if let Some(ref raw) = task.raw_trailing {
        parts.push(raw.clone());
    }

    let content = parts.join(" ");
    let mut result = format!("{indent}- [{status_char}] {content}");

    if let Some(ref bl) = task.block_link {
        result.push(' ');
        result.push('^');
        result.push_str(bl);
    }

    result
}

// ── internal helpers ─────────────────────────────────────────────────────────

/// Strip a trailing ` ^identifier` block link from `s`, returning the
/// remainder and the identifier (without the caret).
fn strip_block_link(s: &str) -> (&str, Option<&str>) {
    let s = s.trim_end();
    if let Some(space_pos) = s.rfind(' ') {
        let candidate = &s[space_pos + 1..];
        if let Some(id) = candidate.strip_prefix('^') {
            if !id.is_empty() && id.chars().all(|c| c.is_alphanumeric() || c == '-') {
                return (&s[..space_pos], Some(id));
            }
        }
    }
    (s, None)
}

/// Return the byte offset of the first recognized field emoji in `content`.
/// If no field emojis are found, returns `content.len()` (whole string is
/// the description).
///
/// For date-type fields the emoji must be followed by a valid ISO-8601 date
/// (`YYYY-MM-DD`), so that values like `📅 today` are not mistaken for fields
/// and remain in the description unchanged.  Priority emojis are recognized
/// whenever followed by a space, another field emoji, or end of string.
fn fields_boundary(content: &str) -> usize {
    const PRIORITY_EMOJIS: &[&str] = &[
        PRIORITY_HIGHEST,
        PRIORITY_HIGH,
        PRIORITY_MEDIUM,
        PRIORITY_LOW,
        PRIORITY_LOWEST,
    ];
    const DATE_EMOJIS: &[&str] = &[CREATED, START, SCHEDULED, DUE, DONE, CANCELLED];
    const TEXT_EMOJIS: &[&str] = &[RECURRENCE, ID, DEPENDS_ON];

    let mut min_pos = content.len();

    let mut check = |emoji: &str, is_valid: &dyn Fn(&str) -> bool| {
        let mut search = 0;
        while search < content.len() {
            match content[search..].find(emoji) {
                None => break,
                Some(rel) => {
                    let abs = search + rel;
                    let after = &content[abs + emoji.len()..];
                    if is_valid(after) && abs < min_pos {
                        min_pos = abs;
                    }
                    search = abs + emoji.len();
                }
            }
        }
    };

    for &emoji in PRIORITY_EMOJIS {
        check(emoji, &|after| {
            after.is_empty()
                || after.starts_with(' ')
                || ALL_FIELD_EMOJIS.iter().any(|e| after.starts_with(e))
        });
    }
    for &emoji in DATE_EMOJIS {
        check(emoji, &|after| {
            after
                .strip_prefix(' ')
                .and_then(|s| s.get(..10))
                .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
                .is_some()
        });
    }
    for &emoji in TEXT_EMOJIS {
        check(emoji, &|after| after.starts_with(' '));
    }

    min_pos
}

/// Find the byte offset of the next field emoji within `s` (used to bound
/// variable-length field values like recurrence text).
fn next_field_boundary(s: &str) -> usize {
    ALL_FIELD_EMOJIS
        .iter()
        .filter_map(|e| s.find(e))
        .min()
        .unwrap_or(s.len())
}

#[derive(Default)]
struct ParsedFields {
    priority: Option<Priority>,
    recurrence: Option<String>,
    created: Option<NaiveDate>,
    start: Option<NaiveDate>,
    scheduled: Option<NaiveDate>,
    due: Option<NaiveDate>,
    done: Option<NaiveDate>,
    cancelled: Option<NaiveDate>,
    id: Option<String>,
    depends_on: Vec<String>,
    raw_trailing: Option<String>,
}

fn parse_field_section(s: &str, fields: &mut ParsedFields) {
    let mut pos = 0;

    while pos < s.len() {
        let rest = &s[pos..];

        // Skip inter-field whitespace.  Once we've started accumulating
        // unknown content (raw_trailing is Some), spaces are part of that
        // content and must be preserved — otherwise content like
        // "(NFs 2072+2074)" would lose its internal spaces.
        if rest.starts_with(' ') {
            if let Some(ref mut raw) = fields.raw_trailing {
                raw.push(' ');
            }
            pos += 1;
            continue;
        }

        // Priority (standalone emoji, no following value).
        if let Some((priority, adv)) = try_priority(rest) {
            if fields.priority.is_none() {
                fields.priority = Some(priority);
            }
            pos += adv;
            continue;
        }

        // Date fields.
        macro_rules! try_date {
            ($emoji:expr, $field:ident) => {
                if let Some((date, adv)) = try_date_field(rest, $emoji) {
                    fields.$field = Some(date);
                    pos += adv;
                    continue;
                }
            };
        }
        try_date!(CREATED, created);
        try_date!(START, start);
        try_date!(SCHEDULED, scheduled);
        try_date!(DUE, due);
        try_date!(DONE, done);
        try_date!(CANCELLED, cancelled);

        // Recurrence: 🔁 <text until next field emoji>.
        if let Some(after) = rest.strip_prefix(RECURRENCE) {
            if let Some(text) = after.strip_prefix(' ') {
                let end = next_field_boundary(text);
                fields.recurrence = Some(text[..end].trim_end().to_string());
                pos += RECURRENCE.len() + 1 + end;
                continue;
            }
        }

        // ID: 🆔 <non-whitespace>.
        if let Some(after) = rest.strip_prefix(ID) {
            if let Some(value) = after.strip_prefix(' ') {
                let end = next_field_boundary(value);
                fields.id = Some(value[..end].trim_end().to_string());
                pos += ID.len() + 1 + end;
                continue;
            }
        }

        // Depends on: ⛔ <comma-separated IDs>.
        if let Some(after) = rest.strip_prefix(DEPENDS_ON) {
            if let Some(value) = after.strip_prefix(' ') {
                let end = next_field_boundary(value);
                let ids_str = value[..end].trim_end();
                fields.depends_on = ids_str
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
                pos += DEPENDS_ON.len() + 1 + end;
                continue;
            }
        }

        // Unknown content: accumulate into raw_trailing.
        let ch = rest.chars().next().unwrap();
        fields.raw_trailing.get_or_insert_with(String::new).push(ch);
        pos += ch.len_utf8();
    }
}

fn try_priority(s: &str) -> Option<(Priority, usize)> {
    const MAP: &[(&str, Priority)] = &[
        (PRIORITY_HIGHEST, Priority::Highest),
        (PRIORITY_HIGH, Priority::High),
        (PRIORITY_MEDIUM, Priority::Medium),
        (PRIORITY_LOW, Priority::Low),
        (PRIORITY_LOWEST, Priority::Lowest),
    ];
    MAP.iter()
        .find(|(emoji, _)| s.starts_with(emoji))
        .map(|(emoji, priority)| (*priority, emoji.len()))
}

fn try_date_field(s: &str, emoji: &str) -> Option<(NaiveDate, usize)> {
    let after = s.strip_prefix(emoji)?;
    let after = after.strip_prefix(' ')?;
    // A date is exactly 10 bytes: YYYY-MM-DD.
    let date_str = after.get(..10)?;
    let date = NaiveDate::parse_from_str(date_str, "%Y-%m-%d").ok()?;
    Some((date, emoji.len() + 1 + 10))
}

/// Extract inline hashtags from `description` (e.g. `#work`, `#t`).
/// Tags remain in the description string unchanged; this builds the index.
pub fn extract_tags(description: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let mut pos = 0;
    while pos < description.len() {
        if description.as_bytes()[pos] == b'#' {
            let start = pos + 1;
            let end = description[start..]
                .find(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '/')
                .map(|i| start + i)
                .unwrap_or(description.len());
            if end > start {
                tags.push(description[start..end].to_string());
            }
            pos = if end > start { end } else { pos + 1 };
        } else {
            let ch = description[pos..].chars().next().unwrap();
            pos += ch.len_utf8();
        }
    }
    tags
}

// ── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ctx() -> ParseContext {
        ParseContext {
            source_file: PathBuf::from("test.md"),
            source_line: 1,
        }
    }

    fn parse(line: &str) -> Task {
        EmojiFormat
            .parse_line(line, ctx())
            .unwrap_or_else(|| panic!("failed to parse task line: {line:?}"))
    }

    fn roundtrip(line: &str) {
        let task = parse(line);
        let serialized = EmojiFormat.serialize_line(&task);
        assert_eq!(
            line, serialized,
            "round-trip failed\n  original:   {line:?}\n  serialized: {serialized:?}"
        );
    }

    // ── status ────────────────────────────────────────────────────────────────

    #[test]
    fn status_open() {
        assert_eq!(parse("- [ ] task").status, Status::Open);
    }

    #[test]
    fn status_done_lower() {
        assert_eq!(parse("- [x] task").status, Status::Done);
    }

    #[test]
    fn status_done_upper() {
        assert_eq!(parse("- [X] task").status, Status::Done);
    }

    #[test]
    fn status_in_progress() {
        assert_eq!(parse("- [/] task").status, Status::InProgress);
    }

    #[test]
    fn status_cancelled() {
        assert_eq!(parse("- [-] task").status, Status::Cancelled);
    }

    #[test]
    fn unknown_status_parses_as_open() {
        assert_eq!(parse("- [?] weird task").status, Status::Open);
    }

    // ── description ──────────────────────────────────────────────────────────

    #[test]
    fn plain_description() {
        assert_eq!(parse("- [ ] Buy milk").description, "Buy milk");
    }

    #[test]
    fn description_with_wikilink() {
        assert_eq!(
            parse("- [x] FUP with [[John True]] 📅 2025-10-25 ✅ 2025-10-24").description,
            "FUP with [[John True]]"
        );
    }

    #[test]
    fn description_with_tag() {
        let task = parse("- [ ] Sell backpacks #t");
        assert_eq!(task.description, "Sell backpacks #t");
        assert_eq!(task.tags, vec!["t"]);
    }

    #[test]
    fn description_strips_nothing_when_no_fields() {
        let task = parse("- [ ] Just text, no fields");
        assert_eq!(task.description, "Just text, no fields");
        assert!(task.priority.is_none());
        assert!(task.due.is_none());
    }

    // ── indent / subtasks ────────────────────────────────────────────────────

    #[test]
    fn indent_level_zero() {
        assert_eq!(parse("- [ ] top level").indent_level, 0);
    }

    #[test]
    fn indent_level_two_spaces() {
        let task = EmojiFormat.parse_line("  - [ ] indented", ctx()).unwrap();
        assert_eq!(task.indent_level, 2);
    }

    #[test]
    fn indent_level_four_spaces() {
        let task = EmojiFormat
            .parse_line("    - [ ] deeply indented", ctx())
            .unwrap();
        assert_eq!(task.indent_level, 4);
    }

    // ── non-task lines ───────────────────────────────────────────────────────

    #[test]
    fn heading_returns_none() {
        assert!(EmojiFormat.parse_line("## Heading", ctx()).is_none());
    }

    #[test]
    fn blank_line_returns_none() {
        assert!(EmojiFormat.parse_line("", ctx()).is_none());
    }

    #[test]
    fn prose_returns_none() {
        assert!(EmojiFormat.parse_line("Some prose text.", ctx()).is_none());
    }

    #[test]
    fn list_item_without_checkbox_returns_none() {
        assert!(EmojiFormat.parse_line("- plain list item", ctx()).is_none());
    }

    // ── priority ─────────────────────────────────────────────────────────────

    #[test]
    fn priority_highest() {
        assert_eq!(parse("- [ ] task 🔺").priority, Some(Priority::Highest));
    }

    #[test]
    fn priority_high() {
        assert_eq!(parse("- [ ] task ⏫").priority, Some(Priority::High));
    }

    #[test]
    fn priority_medium() {
        assert_eq!(parse("- [ ] task 🔼").priority, Some(Priority::Medium));
    }

    #[test]
    fn priority_low() {
        assert_eq!(parse("- [ ] task 🔽").priority, Some(Priority::Low));
    }

    #[test]
    fn priority_lowest() {
        assert_eq!(parse("- [ ] task ⏬").priority, Some(Priority::Lowest));
    }

    #[test]
    fn no_priority() {
        assert!(parse("- [ ] task 📅 2026-05-10").priority.is_none());
    }

    // ── dates ─────────────────────────────────────────────────────────────────

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn due_date() {
        let task = parse("- [ ] task 📅 2026-05-10");
        assert_eq!(task.due, Some(date(2026, 5, 10)));
    }

    #[test]
    fn done_date() {
        let task = parse("- [x] task ✅ 2026-04-01");
        assert_eq!(task.done, Some(date(2026, 4, 1)));
    }

    #[test]
    fn cancelled_date() {
        let task = parse("- [-] task ❌ 2026-02-01");
        assert_eq!(task.cancelled, Some(date(2026, 2, 1)));
    }

    #[test]
    fn created_date() {
        let task = parse("- [ ] task ➕ 2025-11-17");
        assert_eq!(task.created, Some(date(2025, 11, 17)));
    }

    #[test]
    fn start_date() {
        let task = parse("- [ ] task 🛫 2025-10-01");
        assert_eq!(task.start, Some(date(2025, 10, 1)));
    }

    #[test]
    fn scheduled_date() {
        let task = parse("- [ ] task ⏳ 2025-12-29");
        assert_eq!(task.scheduled, Some(date(2025, 12, 29)));
    }

    #[test]
    fn all_dates() {
        let line =
            "- [x] task ➕ 2025-01-01 🛫 2025-01-02 ⏳ 2025-01-03 📅 2025-01-04 ✅ 2025-01-05";
        let task = parse(line);
        assert_eq!(task.created, Some(date(2025, 1, 1)));
        assert_eq!(task.start, Some(date(2025, 1, 2)));
        assert_eq!(task.scheduled, Some(date(2025, 1, 3)));
        assert_eq!(task.due, Some(date(2025, 1, 4)));
        assert_eq!(task.done, Some(date(2025, 1, 5)));
    }

    // ── recurrence ────────────────────────────────────────────────────────────

    #[test]
    fn recurrence_simple() {
        let task = parse("- [ ] task 🔁 every day 📅 2026-05-10");
        assert_eq!(task.recurrence.as_deref(), Some("every day"));
    }

    #[test]
    fn recurrence_monthly_with_day() {
        let task = parse("- [ ] Pay tax 🔼 🔁 every month on the 18th 📅 2026-05-18");
        assert_eq!(task.recurrence.as_deref(), Some("every month on the 18th"));
        assert_eq!(task.priority, Some(Priority::Medium));
        assert_eq!(task.due, Some(date(2026, 5, 18)));
    }

    #[test]
    fn recurrence_at_end_no_following_field() {
        let task = parse("- [ ] task 🔁 every 15 days");
        assert_eq!(task.recurrence.as_deref(), Some("every 15 days"));
    }

    // ── ID and depends_on ─────────────────────────────────────────────────────

    #[test]
    fn task_id() {
        let task = parse("- [ ] task 🆔 abc123");
        assert_eq!(task.id.as_deref(), Some("abc123"));
    }

    #[test]
    fn depends_on_single() {
        let task = parse("- [ ] task ⛔ abc123");
        assert_eq!(task.depends_on, vec!["abc123"]);
    }

    #[test]
    fn depends_on_multiple() {
        let task = parse("- [ ] task ⛔ id1,id2,id3");
        assert_eq!(task.depends_on, vec!["id1", "id2", "id3"]);
    }

    // ── block link ────────────────────────────────────────────────────────────

    #[test]
    fn block_link_stripped() {
        let task = parse("- [ ] task 📅 2026-05-10 ^blk1a");
        assert_eq!(task.block_link.as_deref(), Some("blk1a"));
        assert_eq!(task.due, Some(date(2026, 5, 10)));
    }

    #[test]
    fn no_block_link() {
        let task = parse("- [ ] task 📅 2026-05-10");
        assert!(task.block_link.is_none());
    }

    // ── tags ──────────────────────────────────────────────────────────────────

    #[test]
    fn multiple_tags() {
        let task = parse("- [ ] Do #work task #urgent 📅 2026-05-10");
        assert_eq!(task.tags, vec!["work", "urgent"]);
    }

    #[test]
    fn nested_tag_slash() {
        let task = parse("- [ ] task #area/work");
        assert_eq!(task.tags, vec!["area/work"]);
    }

    // ── round-trip ────────────────────────────────────────────────────────────

    #[test]
    fn roundtrip_simple_open() {
        roundtrip("- [ ] Buy milk");
    }

    #[test]
    fn roundtrip_done_with_date() {
        roundtrip("- [x] FUP with [[John True]] 📅 2025-10-25 ✅ 2025-10-24");
    }

    #[test]
    fn roundtrip_cancelled_with_scheduled() {
        roundtrip("- [-] ACK: At any point in time ⏳ 2025-12-29 📅 2025-12-29 ❌ 2026-02-01");
    }

    #[test]
    fn roundtrip_priority_and_recurrence() {
        roundtrip("- [ ] Pay tax (DAS) 🔼 🔁 every month on the 18th 📅 2026-05-18");
    }

    #[test]
    fn roundtrip_created_due_done() {
        roundtrip("- [x] Order flowers ➕ 2025-11-24 📅 2025-11-24 ✅ 2025-11-24");
    }

    #[test]
    fn roundtrip_highest_priority_all_dates() {
        roundtrip("- [x] Fix water leak 🔺 ➕ 2025-11-24 📅 2025-11-24 ✅ 2025-11-24");
    }

    #[test]
    fn roundtrip_indented() {
        roundtrip("  - [ ] subtask under parent");
    }

    #[test]
    fn roundtrip_with_block_link() {
        roundtrip("- [ ] task 📅 2026-05-10 ^abc123");
    }

    #[test]
    fn roundtrip_with_tag() {
        roundtrip("- [ ] Sell backpacks #t");
    }

    #[test]
    fn roundtrip_in_progress() {
        roundtrip("- [/] Working on it 📅 2026-05-09");
    }

    // ── pathological cases ────────────────────────────────────────────────────

    #[test]
    fn empty_description() {
        // Tasks with empty description are technically valid.
        let task = EmojiFormat
            .parse_line("- [ ]  📅 2026-05-10", ctx())
            .expect("should parse");
        assert_eq!(task.description, "");
        assert_eq!(task.due, Some(date(2026, 5, 10)));
    }

    #[test]
    fn description_only_spaces_trimmed() {
        let task = EmojiFormat
            .parse_line("- [ ] task   📅 2026-05-10", ctx())
            .unwrap();
        // Trailing space before field should not bleed into description.
        assert_eq!(task.description, "task");
    }

    #[test]
    fn wikilink_does_not_confuse_status_parser() {
        // The `[[` in a wikilink must not be mistaken for a task status.
        assert!(EmojiFormat.parse_line("See [[my note]]", ctx()).is_none());
    }

    // ── proptest round-trips ──────────────────────────────────────────────────

    use proptest::prelude::*;

    fn arb_status() -> impl Strategy<Value = Status> {
        prop_oneof![
            Just(Status::Open),
            Just(Status::Done),
            Just(Status::InProgress),
            Just(Status::Cancelled),
        ]
    }

    fn arb_priority() -> impl Strategy<Value = Option<Priority>> {
        prop_oneof![
            Just(None),
            Just(Some(Priority::Highest)),
            Just(Some(Priority::High)),
            Just(Some(Priority::Medium)),
            Just(Some(Priority::Low)),
            Just(Some(Priority::Lowest)),
        ]
    }

    fn arb_date() -> impl Strategy<Value = Option<NaiveDate>> {
        prop_oneof![
            Just(None),
            (1970i32..2100, 1u32..=12, 1u32..=28)
                .prop_map(|(y, m, d)| { Some(NaiveDate::from_ymd_opt(y, m, d).unwrap()) }),
        ]
    }

    /// Generate a description that won't be misidentified as containing field
    /// markers and has no leading/trailing whitespace (which the parser trims).
    fn arb_description() -> impl Strategy<Value = String> {
        proptest::string::string_regex("[a-zA-Z0-9 ,.:()/]{1,60}")
            .unwrap()
            .prop_filter("no leading/trailing whitespace", |s| {
                !s.starts_with(' ') && !s.ends_with(' ')
            })
    }

    fn arb_recurrence() -> impl Strategy<Value = Option<String>> {
        prop_oneof![
            Just(None),
            proptest::string::string_regex("every [a-z]+ [a-z0-9 ]{2,15}")
                .unwrap()
                .prop_map(|s| Some(s.trim().to_string())),
        ]
    }

    proptest! {
        #[test]
        fn prop_roundtrip_serialize_parse(
            status in arb_status(),
            priority in arb_priority(),
            description in arb_description(),
            created in arb_date(),
            start in arb_date(),
            scheduled in arb_date(),
            due in arb_date(),
            done in arb_date(),
            cancelled in arb_date(),
            recurrence in arb_recurrence(),
        ) {
            let tags = extract_tags(&description);
            let task = Task {
                description: description.clone(),
                status,
                priority,
                tags,
                created,
                start,
                scheduled,
                due,
                done,
                cancelled,
                recurrence,
                id: None,
                depends_on: vec![],
                on_completion: None,
                block_link: None,
                raw_trailing: None,
                source_file: PathBuf::from("test.md"),
                source_line: 1,
                indent_level: 0,
                parent: None,
            };

            let line = EmojiFormat.serialize_line(&task);
            let parsed = EmojiFormat
                .parse_line(&line, ctx())
                .expect("serialized line should parse back");

            prop_assert_eq!(&task.description, &parsed.description);
            prop_assert_eq!(task.status, parsed.status);
            prop_assert_eq!(task.priority, parsed.priority);
            prop_assert_eq!(task.created, parsed.created);
            prop_assert_eq!(task.start, parsed.start);
            prop_assert_eq!(task.scheduled, parsed.scheduled);
            prop_assert_eq!(task.due, parsed.due);
            prop_assert_eq!(task.done, parsed.done);
            prop_assert_eq!(task.cancelled, parsed.cancelled);
            prop_assert_eq!(task.recurrence, parsed.recurrence);
            prop_assert_eq!(task.id, parsed.id);
            prop_assert_eq!(task.depends_on, parsed.depends_on);
        }

        /// For any line that parses, re-serializing must produce the same line.
        #[test]
        fn prop_serialize_idempotent(
            status in arb_status(),
            priority in arb_priority(),
            description in arb_description(),
            due in arb_date(),
        ) {
            let tags = extract_tags(&description);
            let task = Task {
                description,
                status,
                priority,
                tags,
                created: None,
                start: None,
                scheduled: None,
                due,
                done: None,
                cancelled: None,
                recurrence: None,
                id: None,
                depends_on: vec![],
                on_completion: None,
                block_link: None,
                raw_trailing: None,
                source_file: PathBuf::from("test.md"),
                source_line: 1,
                indent_level: 0,
                parent: None,
            };

            let line1 = EmojiFormat.serialize_line(&task);
            let parsed = EmojiFormat.parse_line(&line1, ctx()).unwrap();
            let line2 = EmojiFormat.serialize_line(&parsed);
            prop_assert_eq!(line1, line2, "serialize(parse(serialize(t))) != serialize(t)");
        }
    }
}
