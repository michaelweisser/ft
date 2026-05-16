//! Time-spent reporting for timeblocks.
//!
//! Hierarchical aggregation: tags like `@work/meeting/1on1` contribute
//! their duration to `work`, `meeting`, and `1on1` at three nested
//! levels. The shape mirrors `blockary::time_summary` but uses ft's
//! `Timeblock` type (start/end times rather than a duration string) and
//! returns `u32` minutes throughout (vs blockary's `u16`).

use std::collections::HashMap;

use super::Timeblock;

/// One row of the per-tag time summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagTime {
    /// A single level segment (e.g. `"work"` for `@work/meeting`).
    pub tag: String,
    pub minutes: u32,
    pub children: Vec<TagTime>,
}

/// Aggregate blocks into a hierarchical tag tree, sorted descending by
/// minutes at every level.
///
/// Unlike `blockary::time_summary::time_per_tag`, `@break` IS included
/// as a top-level row here — the spec wants the user to see their break
/// time bucket in the TUI sidebar. `@break` blocks are still excluded
/// from [`total_minutes`].
pub fn time_per_tag(blocks: &[Timeblock]) -> Vec<TagTime> {
    time_per_tag_at(blocks, 0)
}

fn time_per_tag_at(blocks: &[Timeblock], level: usize) -> Vec<TagTime> {
    let mut groups: HashMap<String, Vec<&Timeblock>> = HashMap::new();
    for b in blocks {
        for tag in &b.tags {
            if let Some(seg) = tag.levels.get(level) {
                groups.entry(seg.clone()).or_default().push(b);
            }
        }
    }
    let mut out: Vec<TagTime> = groups
        .into_iter()
        .map(|(tag, group)| {
            // Recursion bound: at the deepest level (3), `levels.get(3)`
            // is None for every block, so `groups` is empty and the
            // recursive call returns an empty Vec.
            let group_blocks: Vec<Timeblock> = group.iter().map(|b| (*b).clone()).collect();
            let children = if level + 1 < 3 {
                time_per_tag_at(&group_blocks, level + 1)
            } else {
                Vec::new()
            };
            TagTime {
                tag,
                minutes: group.iter().map(|b| duration_minutes(b)).sum(),
                children,
            }
        })
        .collect();
    out.sort_by(|a, b| b.minutes.cmp(&a.minutes).then_with(|| a.tag.cmp(&b.tag)));
    out
}

/// Sum of block durations, excluding any block tagged with a top-level
/// `@break`. Matches blockary's `total_time_spent` semantics so the
/// "total" row in `ft timeblocks spent` lines up with the user's
/// existing reports.
pub fn total_minutes(blocks: &[Timeblock]) -> u32 {
    blocks
        .iter()
        .filter(|b| !is_break(b))
        .map(duration_minutes)
        .sum()
}

/// Convert raw minutes to `(hours, minutes)`.
pub fn minutes_to_hours_minutes(m: u32) -> (u32, u32) {
    (m / 60, m % 60)
}

fn duration_minutes(b: &Timeblock) -> u32 {
    use chrono::Timelike;
    let s = b.start.hour() * 60 + b.start.minute();
    let e = b.end.hour() * 60 + b.end.minute();
    e.saturating_sub(s)
}

fn is_break(b: &Timeblock) -> bool {
    b.tags
        .iter()
        .any(|t| t.levels.first().map(String::as_str) == Some("break"))
}

// ── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeblock::parse_line;

    fn parse(s: &str) -> Timeblock {
        parse_line(s).unwrap()
    }

    #[test]
    fn total_minutes_sums_blocks() {
        let blocks = vec![parse("09:00 - 10:00 a"), parse("10:00 - 10:30 b")];
        assert_eq!(total_minutes(&blocks), 90);
    }

    #[test]
    fn total_minutes_excludes_break() {
        let blocks = vec![
            parse("09:00 - 10:00 work thing @work"),
            parse("10:00 - 10:30 coffee @break"),
        ];
        assert_eq!(total_minutes(&blocks), 60);
    }

    #[test]
    fn time_per_tag_groups_by_top_level() {
        let blocks = vec![
            parse("09:00 - 10:00 a @work"),
            parse("10:00 - 11:00 b @work"),
            parse("11:00 - 11:30 c @personal"),
        ];
        let tags = time_per_tag(&blocks);
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].tag, "work");
        assert_eq!(tags[0].minutes, 120);
        assert_eq!(tags[1].tag, "personal");
        assert_eq!(tags[1].minutes, 30);
    }

    #[test]
    fn time_per_tag_nests_sub_levels() {
        let blocks = vec![
            parse("09:00 - 10:00 a @work/meeting"),
            parse("10:00 - 11:00 b @work/meeting/1on1"),
            parse("11:00 - 11:30 c @work/code"),
        ];
        let tags = time_per_tag(&blocks);
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].tag, "work");
        assert_eq!(tags[0].minutes, 150);

        let meeting = tags[0]
            .children
            .iter()
            .find(|c| c.tag == "meeting")
            .unwrap();
        assert_eq!(meeting.minutes, 120);
        let one_on_one = meeting.children.iter().find(|c| c.tag == "1on1").unwrap();
        assert_eq!(one_on_one.minutes, 60);
    }

    #[test]
    fn time_per_tag_sorts_descending_at_every_level() {
        let blocks = vec![
            parse("09:00 - 09:30 short @a"),
            parse("10:00 - 12:00 long @b"),
            parse("12:00 - 13:00 mid @c"),
        ];
        let tags = time_per_tag(&blocks);
        let names: Vec<&str> = tags.iter().map(|t| t.tag.as_str()).collect();
        assert_eq!(names, vec!["b", "c", "a"]);
    }

    #[test]
    fn time_per_tag_includes_break_unlike_blockary() {
        let blocks = vec![
            parse("09:00 - 10:00 work @work"),
            parse("10:00 - 10:30 coffee @break"),
        ];
        let tags = time_per_tag(&blocks);
        assert!(tags.iter().any(|t| t.tag == "break"));
    }

    #[test]
    fn minutes_to_hours_minutes_split() {
        assert_eq!(minutes_to_hours_minutes(0), (0, 0));
        assert_eq!(minutes_to_hours_minutes(59), (0, 59));
        assert_eq!(minutes_to_hours_minutes(60), (1, 0));
        assert_eq!(minutes_to_hours_minutes(125), (2, 5));
    }
}
