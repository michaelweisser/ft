//! Recurrence engine for the v1 supported subset.
//!
//! Patterns recognised (case-insensitive):
//! - `every day` / `every N day[s]`
//! - `every week` / `every N week[s]` / `every week on <weekday>`
//! - `every month` / `every N month[s]` / `every month on the Nth`
//!
//! Anything outside that whitelist returns [`RecurrenceError::Unsupported`]
//! naming the offending substring so users can see exactly what isn't handled.
//!
//! Each parsed [`Rule`] computes the next instance date from an anchor, and
//! `next_dates` shifts a task's optional due/scheduled/start dates by the same
//! delta. The "anchor" is the task's primary date (due > scheduled > start).

use chrono::{Datelike, Days, Months, NaiveDate, Weekday};
use thiserror::Error;

use super::Task;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RecurrenceError {
    #[error("recurring task has no due/scheduled/start date to anchor on")]
    NoAnchor,
    #[error("unsupported recurrence rule `{rule}`: unsupported token `{token}`")]
    Unsupported { rule: String, token: String },
    #[error("malformed recurrence rule `{rule}`: {reason}")]
    Malformed { rule: String, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rule {
    Days(u32),
    Weeks(u32),
    Months(u32),
    WeekOnWeekday(Weekday),
    MonthOnDay(u32),
}

/// Parse a recurrence rule string. Returns the structured rule or an error
/// naming the unsupported token.
pub fn parse_rule(rule: &str) -> Result<Rule, RecurrenceError> {
    let original = rule;
    let normalised = rule.trim().to_ascii_lowercase();
    let tokens: Vec<&str> = normalised.split_whitespace().collect();

    let malformed = |reason: &str| RecurrenceError::Malformed {
        rule: original.to_string(),
        reason: reason.to_string(),
    };
    let unsupported = |token: &str| RecurrenceError::Unsupported {
        rule: original.to_string(),
        token: token.to_string(),
    };

    if tokens.first().copied() != Some("every") {
        return Err(unsupported(tokens.first().copied().unwrap_or("")));
    }

    // Parse the count (optional) and unit.
    // Forms: "every <unit>" or "every <N> <unit>"
    let (count, unit_idx) = match tokens.get(1) {
        None => return Err(malformed("missing time unit")),
        Some(s) => match s.parse::<u32>() {
            Ok(0) => return Err(malformed("count must be positive")),
            Ok(n) => (n, 2),
            Err(_) => (1, 1),
        },
    };

    let unit = tokens
        .get(unit_idx)
        .ok_or_else(|| malformed("missing time unit"))?;
    let tail = &tokens[unit_idx + 1..];

    match *unit {
        "day" | "days" => {
            if !tail.is_empty() {
                return Err(unsupported(&tail.join(" ")));
            }
            Ok(Rule::Days(count))
        }
        "week" | "weeks" => parse_week_tail(count, tail, original, &unsupported, &malformed),
        "month" | "months" => parse_month_tail(count, tail, original, &unsupported, &malformed),
        other => Err(unsupported(other)),
    }
}

fn parse_week_tail(
    count: u32,
    tail: &[&str],
    _rule: &str,
    unsupported: &dyn Fn(&str) -> RecurrenceError,
    malformed: &dyn Fn(&str) -> RecurrenceError,
) -> Result<Rule, RecurrenceError> {
    if tail.is_empty() {
        return Ok(Rule::Weeks(count));
    }
    // Only `on <weekday>` supported.
    if tail.first().copied() != Some("on") {
        return Err(unsupported(&tail.join(" ")));
    }
    if count != 1 {
        // `every 2 weeks on monday` is unsupported in v1 — defer.
        return Err(unsupported(&tail.join(" ")));
    }
    let day = tail.get(1).ok_or_else(|| malformed("missing weekday"))?;
    let weekday = parse_weekday(day).ok_or_else(|| unsupported(day))?;
    if tail.len() > 2 {
        return Err(unsupported(&tail[2..].join(" ")));
    }
    Ok(Rule::WeekOnWeekday(weekday))
}

fn parse_month_tail(
    count: u32,
    tail: &[&str],
    _rule: &str,
    unsupported: &dyn Fn(&str) -> RecurrenceError,
    malformed: &dyn Fn(&str) -> RecurrenceError,
) -> Result<Rule, RecurrenceError> {
    if tail.is_empty() {
        return Ok(Rule::Months(count));
    }
    if tail.first().copied() != Some("on") {
        return Err(unsupported(&tail.join(" ")));
    }
    if count != 1 {
        return Err(unsupported(&tail.join(" ")));
    }
    // `on the Nth`
    if tail.get(1).copied() != Some("the") {
        return Err(unsupported(&tail[1..].join(" ")));
    }
    let n_token = tail
        .get(2)
        .ok_or_else(|| malformed("missing day-of-month"))?;
    let n = parse_ordinal(n_token).ok_or_else(|| unsupported(n_token))?;
    if !(1..=31).contains(&n) {
        return Err(malformed("day-of-month must be 1..=31"));
    }
    if tail.len() > 3 {
        return Err(unsupported(&tail[3..].join(" ")));
    }
    Ok(Rule::MonthOnDay(n))
}

fn parse_weekday(s: &str) -> Option<Weekday> {
    match s {
        "monday" | "mon" => Some(Weekday::Mon),
        "tuesday" | "tue" | "tues" => Some(Weekday::Tue),
        "wednesday" | "wed" => Some(Weekday::Wed),
        "thursday" | "thu" | "thur" | "thurs" => Some(Weekday::Thu),
        "friday" | "fri" => Some(Weekday::Fri),
        "saturday" | "sat" => Some(Weekday::Sat),
        "sunday" | "sun" => Some(Weekday::Sun),
        _ => None,
    }
}

/// Parse `1`, `1st`, `2nd`, `3rd`, `4th`, … into a `u32`.
fn parse_ordinal(s: &str) -> Option<u32> {
    if let Ok(n) = s.parse::<u32>() {
        return Some(n);
    }
    let trimmed = s
        .strip_suffix("st")
        .or_else(|| s.strip_suffix("nd"))
        .or_else(|| s.strip_suffix("rd"))
        .or_else(|| s.strip_suffix("th"))?;
    trimmed.parse().ok()
}

/// Compute the next date for `rule` given `anchor`.
pub fn next_after(rule: &Rule, anchor: NaiveDate) -> NaiveDate {
    match rule {
        Rule::Days(n) => anchor
            .checked_add_days(Days::new(u64::from(*n)))
            .expect("date arithmetic overflow"),
        Rule::Weeks(n) => anchor
            .checked_add_days(Days::new(u64::from(n * 7)))
            .expect("date arithmetic overflow"),
        Rule::Months(n) => anchor
            .checked_add_months(Months::new(*n))
            .expect("date arithmetic overflow"),
        Rule::WeekOnWeekday(target) => next_weekday_after(anchor, *target),
        Rule::MonthOnDay(n) => month_on_day_after(anchor, *n),
    }
}

fn next_weekday_after(anchor: NaiveDate, target: Weekday) -> NaiveDate {
    let cur = anchor.weekday().num_days_from_monday() as i64;
    let tgt = target.num_days_from_monday() as i64;
    let mut delta = tgt - cur;
    if delta <= 0 {
        delta += 7;
    }
    anchor
        .checked_add_days(Days::new(delta as u64))
        .expect("date arithmetic overflow")
}

fn month_on_day_after(anchor: NaiveDate, day_of_month: u32) -> NaiveDate {
    // Start from the first of next month so we don't get bitten by chrono's
    // automatic end-of-month clamping when the anchor day is > target month's
    // last day.
    let next_month_first = anchor
        .with_day(1)
        .and_then(|d| d.checked_add_months(Months::new(1)))
        .expect("date arithmetic overflow");
    let last = days_in_month(next_month_first.year(), next_month_first.month());
    let day = day_of_month.min(last);
    NaiveDate::from_ymd_opt(next_month_first.year(), next_month_first.month(), day)
        .expect("constructed date is valid by clamp")
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let first_next = NaiveDate::from_ymd_opt(next_year, next_month, 1).unwrap();
    let last_this = first_next.pred_opt().unwrap();
    last_this.day()
}

/// The dates of the next instance of a recurring task, given the current
/// task's dates and the parsed rule. Returns updated `(start, scheduled, due)`.
///
/// The "primary" date is the first defined of `due`, `scheduled`, `start`. The
/// rule is applied to the primary date to compute its next value; the other
/// dates shift by the same number of days.
pub fn next_dates(rule: &Rule, task: &Task) -> Result<NextDates, RecurrenceError> {
    let primary = task
        .due
        .or(task.scheduled)
        .or(task.start)
        .ok_or(RecurrenceError::NoAnchor)?;

    let new_primary = next_after(rule, primary);
    let delta = new_primary.signed_duration_since(primary).num_days();

    let shift = |d: Option<NaiveDate>| -> Option<NaiveDate> {
        d.and_then(|d| d.checked_add_signed(chrono::Duration::days(delta)))
    };

    Ok(NextDates {
        start: shift(task.start),
        scheduled: shift(task.scheduled),
        due: shift(task.due),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextDates {
    pub start: Option<NaiveDate>,
    pub scheduled: Option<NaiveDate>,
    pub due: Option<NaiveDate>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn task_with(
        due: Option<NaiveDate>,
        sched: Option<NaiveDate>,
        start: Option<NaiveDate>,
    ) -> Task {
        Task {
            description: "x".into(),
            status: super::super::Status::Open,
            priority: None,
            tags: vec![],
            created: None,
            start,
            scheduled: sched,
            due,
            done: None,
            cancelled: None,
            recurrence: None,
            id: None,
            depends_on: vec![],
            on_completion: None,
            block_link: None,
            raw_trailing: None,
            source_file: PathBuf::from("x.md"),
            source_line: 1,
            indent_level: 0,
            parent: None,
        }
    }

    // ── parse ─────────────────────────────────────────────────────────────────

    #[test]
    fn parse_every_day() {
        assert_eq!(parse_rule("every day").unwrap(), Rule::Days(1));
    }

    #[test]
    fn parse_every_n_days() {
        assert_eq!(parse_rule("every 3 days").unwrap(), Rule::Days(3));
        assert_eq!(parse_rule("every 1 day").unwrap(), Rule::Days(1));
    }

    #[test]
    fn parse_every_week() {
        assert_eq!(parse_rule("every week").unwrap(), Rule::Weeks(1));
        assert_eq!(parse_rule("every 2 weeks").unwrap(), Rule::Weeks(2));
    }

    #[test]
    fn parse_every_week_on_weekday() {
        assert_eq!(
            parse_rule("every week on monday").unwrap(),
            Rule::WeekOnWeekday(Weekday::Mon)
        );
        assert_eq!(
            parse_rule("every week on Fri").unwrap(),
            Rule::WeekOnWeekday(Weekday::Fri)
        );
    }

    #[test]
    fn parse_every_month() {
        assert_eq!(parse_rule("every month").unwrap(), Rule::Months(1));
        assert_eq!(parse_rule("every 6 months").unwrap(), Rule::Months(6));
    }

    #[test]
    fn parse_every_month_on_the_nth() {
        assert_eq!(
            parse_rule("every month on the 1st").unwrap(),
            Rule::MonthOnDay(1)
        );
        assert_eq!(
            parse_rule("every month on the 18th").unwrap(),
            Rule::MonthOnDay(18)
        );
        assert_eq!(
            parse_rule("every month on the 31").unwrap(),
            Rule::MonthOnDay(31)
        );
    }

    #[test]
    fn parse_case_insensitive() {
        assert_eq!(
            parse_rule("Every Month On The 18th").unwrap(),
            Rule::MonthOnDay(18)
        );
    }

    #[test]
    fn parse_rejects_year() {
        let e = parse_rule("every year").unwrap_err();
        assert!(
            matches!(e, RecurrenceError::Unsupported { ref token, .. } if token == "year"),
            "{e}"
        );
    }

    #[test]
    fn parse_rejects_when_done_modifier() {
        // Plugin supports `every day when done`; we don't yet.
        let e = parse_rule("every day when done").unwrap_err();
        assert!(
            matches!(e, RecurrenceError::Unsupported { ref token, .. } if token.contains("when")),
            "{e}"
        );
    }

    #[test]
    fn parse_rejects_every_2_weeks_on_monday() {
        let e = parse_rule("every 2 weeks on monday").unwrap_err();
        assert!(matches!(e, RecurrenceError::Unsupported { .. }), "{e:?}");
    }

    #[test]
    fn parse_rejects_unknown_weekday() {
        let e = parse_rule("every week on funday").unwrap_err();
        assert!(
            matches!(e, RecurrenceError::Unsupported { ref token, .. } if token == "funday"),
            "{e}"
        );
    }

    #[test]
    fn parse_rejects_zero_count() {
        let e = parse_rule("every 0 days").unwrap_err();
        assert!(matches!(e, RecurrenceError::Malformed { .. }), "{e:?}");
    }

    #[test]
    fn parse_rejects_empty() {
        let e = parse_rule("").unwrap_err();
        assert!(matches!(e, RecurrenceError::Unsupported { .. }), "{e:?}");
    }

    // ── next_after: days / weeks / months ─────────────────────────────────────

    #[test]
    fn next_every_day() {
        assert_eq!(next_after(&Rule::Days(1), d(2026, 5, 10)), d(2026, 5, 11));
        assert_eq!(next_after(&Rule::Days(7), d(2026, 5, 10)), d(2026, 5, 17));
    }

    #[test]
    fn next_every_week() {
        assert_eq!(next_after(&Rule::Weeks(1), d(2026, 5, 10)), d(2026, 5, 17));
        assert_eq!(next_after(&Rule::Weeks(2), d(2026, 5, 10)), d(2026, 5, 24));
    }

    #[test]
    fn next_every_month_normal() {
        assert_eq!(next_after(&Rule::Months(1), d(2026, 5, 10)), d(2026, 6, 10));
        assert_eq!(
            next_after(&Rule::Months(6), d(2026, 5, 10)),
            d(2026, 11, 10)
        );
    }

    #[test]
    fn next_every_month_clamps_eom() {
        // Jan 31 + 1 month → Feb 28 (chrono clamp).
        assert_eq!(next_after(&Rule::Months(1), d(2026, 1, 31)), d(2026, 2, 28));
    }

    #[test]
    fn next_every_month_leap_day() {
        // Feb 29 2024 + 1 month → Mar 29 2024 (no clamping needed).
        assert_eq!(next_after(&Rule::Months(1), d(2024, 2, 29)), d(2024, 3, 29));
    }

    // ── next_after: weekday ────────────────────────────────────────────────────

    #[test]
    fn next_weekday_strictly_after() {
        // 2026-05-10 is a Sunday (verified). Next Monday is 5-11.
        assert_eq!(
            next_after(&Rule::WeekOnWeekday(Weekday::Mon), d(2026, 5, 10)),
            d(2026, 5, 11)
        );
        // Same weekday → +7.
        assert_eq!(
            next_after(&Rule::WeekOnWeekday(Weekday::Sun), d(2026, 5, 10)),
            d(2026, 5, 17)
        );
        // Next Friday from Sunday May 10 = May 15.
        assert_eq!(
            next_after(&Rule::WeekOnWeekday(Weekday::Fri), d(2026, 5, 10)),
            d(2026, 5, 15)
        );
    }

    // ── next_after: month-on-day ──────────────────────────────────────────────

    #[test]
    fn next_month_on_day_simple() {
        // Anchor 2026-05-18, rule monthly on the 18th → 2026-06-18.
        assert_eq!(
            next_after(&Rule::MonthOnDay(18), d(2026, 5, 18)),
            d(2026, 6, 18)
        );
    }

    #[test]
    fn next_month_on_day_clamp_then_recover() {
        // anchor=2026-01-31 with rule "the 31st":
        //   anchor.with_day(1)=2026-01-01 +1mo=2026-02-01, last=28, min(31,28)=28 → 2026-02-28
        //   then 2026-02-28 +1mo=2026-03-01, last=31, min(31,31)=31 → 2026-03-31
        assert_eq!(
            next_after(&Rule::MonthOnDay(31), d(2026, 1, 31)),
            d(2026, 2, 28)
        );
        assert_eq!(
            next_after(&Rule::MonthOnDay(31), d(2026, 2, 28)),
            d(2026, 3, 31)
        );
    }

    #[test]
    fn next_month_on_day_anchor_in_middle() {
        // If anchor is 2026-05-05 and rule is "on the 18th", next is the 18th
        // of next month (June 18), not May 18.
        assert_eq!(
            next_after(&Rule::MonthOnDay(18), d(2026, 5, 5)),
            d(2026, 6, 18)
        );
    }

    #[test]
    fn next_month_on_day_year_rollover() {
        assert_eq!(
            next_after(&Rule::MonthOnDay(15), d(2026, 12, 20)),
            d(2027, 1, 15)
        );
    }

    // ── next_dates: anchor preference + delta shift ───────────────────────────

    #[test]
    fn next_dates_due_anchor_shifts_other_dates() {
        let task = task_with(
            Some(d(2026, 5, 10)),
            Some(d(2026, 5, 8)),
            Some(d(2026, 5, 1)),
        );
        let n = next_dates(&Rule::Days(7), &task).unwrap();
        // delta = 7
        assert_eq!(n.due, Some(d(2026, 5, 17)));
        assert_eq!(n.scheduled, Some(d(2026, 5, 15)));
        assert_eq!(n.start, Some(d(2026, 5, 8)));
    }

    #[test]
    fn next_dates_uses_scheduled_when_due_absent() {
        let task = task_with(None, Some(d(2026, 5, 8)), None);
        let n = next_dates(&Rule::Days(1), &task).unwrap();
        assert_eq!(n.due, None);
        assert_eq!(n.scheduled, Some(d(2026, 5, 9)));
    }

    #[test]
    fn next_dates_uses_start_when_others_absent() {
        let task = task_with(None, None, Some(d(2026, 5, 1)));
        let n = next_dates(&Rule::Weeks(1), &task).unwrap();
        assert_eq!(n.start, Some(d(2026, 5, 8)));
    }

    #[test]
    fn next_dates_no_anchor_errors() {
        let task = task_with(None, None, None);
        let e = next_dates(&Rule::Days(1), &task).unwrap_err();
        assert_eq!(e, RecurrenceError::NoAnchor);
    }

    #[test]
    fn next_dates_month_clamp_delta_consistent() {
        // due=2026-01-31, scheduled=2026-01-29, monthly. Primary delta:
        // 2026-02-28 − 2026-01-31 = 28 days. Scheduled shifts by the same 28
        // days: 2026-01-29 + 28d = 2026-02-26. (Plugin behavior: shift others
        // by the same delta the primary date moved.)
        let task = task_with(Some(d(2026, 1, 31)), Some(d(2026, 1, 29)), None);
        let n = next_dates(&Rule::Months(1), &task).unwrap();
        assert_eq!(n.due, Some(d(2026, 2, 28)));
        assert_eq!(n.scheduled, Some(d(2026, 2, 26)));
    }
}
