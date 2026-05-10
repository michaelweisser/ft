//! Boolean predicate tree compiled from the DSL parser.
//!
//! Sits next to [`Filter`](super::filter::Filter): flag-based filtering
//! produces a `Filter`; query-string filtering produces an `Expr`. The CLI
//! `tasks list` command applies them in sequence (filter then expr) which is
//! semantically equivalent to and-composing them.

use chrono::NaiveDate;

use crate::task::{Priority, Status, Task};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Atom(Atom),
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Atom {
    Status(Status),
    Priority(Priority),
    /// Source-file path contains this substring.
    PathIncludes(String),
    /// Task carries this tag (leading `#` stripped during construction).
    HasTag(String),
    DueBefore(NaiveDate),
    DueAfter(NaiveDate),
    DueOn(NaiveDate),
    ScheduledBefore(NaiveDate),
    ScheduledAfter(NaiveDate),
    ScheduledOn(NaiveDate),
    CompletedBefore(NaiveDate),
    CompletedAfter(NaiveDate),
    CompletedOn(NaiveDate),
    /// Status == Done.
    Done,
    /// "Still actionable" — Status is Open or InProgress (excludes Done AND
    /// Cancelled). Matches plugin convention: cancelled tasks are no longer
    /// on your plate, so `not done` shouldn't list them.
    NotDone,
    HasDue,
    NoDue,
}

impl Expr {
    pub fn matches(&self, task: &Task) -> bool {
        match self {
            Expr::Atom(a) => a.matches(task),
            Expr::And(parts) => parts.iter().all(|p| p.matches(task)),
            Expr::Or(parts) => parts.iter().any(|p| p.matches(task)),
            Expr::Not(inner) => !inner.matches(task),
        }
    }
}

impl Atom {
    pub fn matches(&self, task: &Task) -> bool {
        match self {
            Atom::Status(s) => task.status == *s,
            Atom::Priority(p) => task.priority == Some(*p),
            Atom::PathIncludes(needle) => {
                task.source_file.to_string_lossy().contains(needle.as_str())
            }
            Atom::HasTag(tag) => task.tags.iter().any(|t| t == tag),
            Atom::DueBefore(d) => matches!(task.due, Some(td) if td < *d),
            Atom::DueAfter(d) => matches!(task.due, Some(td) if td > *d),
            Atom::DueOn(d) => matches!(task.due, Some(td) if td == *d),
            Atom::ScheduledBefore(d) => matches!(task.scheduled, Some(td) if td < *d),
            Atom::ScheduledAfter(d) => matches!(task.scheduled, Some(td) if td > *d),
            Atom::ScheduledOn(d) => matches!(task.scheduled, Some(td) if td == *d),
            Atom::CompletedBefore(d) => matches!(task.done, Some(td) if td < *d),
            Atom::CompletedAfter(d) => matches!(task.done, Some(td) if td > *d),
            Atom::CompletedOn(d) => matches!(task.done, Some(td) if td == *d),
            Atom::Done => task.status == Status::Done,
            Atom::NotDone => matches!(task.status, Status::Open | Status::InProgress),
            Atom::HasDue => task.due.is_some(),
            Atom::NoDue => task.due.is_none(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn task(desc: &str) -> Task {
        Task {
            description: desc.into(),
            status: Status::Open,
            priority: None,
            tags: Vec::new(),
            created: None,
            start: None,
            scheduled: None,
            due: None,
            done: None,
            cancelled: None,
            recurrence: None,
            id: None,
            depends_on: Vec::new(),
            on_completion: None,
            block_link: None,
            raw_trailing: None,
            source_file: PathBuf::from("notes/x.md"),
            source_line: 1,
            indent_level: 0,
            parent: None,
        }
    }

    #[test]
    fn atom_status_match() {
        let mut t = task("a");
        t.status = Status::Done;
        assert!(Atom::Done.matches(&t));
        assert!(!Atom::NotDone.matches(&t));
    }

    #[test]
    fn not_done_excludes_cancelled() {
        // Plugin convention: cancelled is "no longer on your plate", so
        // `not done` is "still actionable" = Open or InProgress only.
        let mut t = task("a");
        t.status = Status::Cancelled;
        assert!(!Atom::NotDone.matches(&t));
        assert!(!Atom::Done.matches(&t));

        t.status = Status::Open;
        assert!(Atom::NotDone.matches(&t));
        t.status = Status::InProgress;
        assert!(Atom::NotDone.matches(&t));
    }

    #[test]
    fn atom_due_comparisons() {
        let mut t = task("a");
        t.due = Some(NaiveDate::from_ymd_opt(2026, 5, 10).unwrap());
        assert!(Atom::DueOn(NaiveDate::from_ymd_opt(2026, 5, 10).unwrap()).matches(&t));
        assert!(Atom::DueBefore(NaiveDate::from_ymd_opt(2026, 5, 11).unwrap()).matches(&t));
        assert!(!Atom::DueBefore(NaiveDate::from_ymd_opt(2026, 5, 10).unwrap()).matches(&t));
        assert!(Atom::DueAfter(NaiveDate::from_ymd_opt(2026, 5, 9).unwrap()).matches(&t));
    }

    #[test]
    fn and_or_not() {
        let mut t = task("x");
        t.tags = vec!["work".into()];
        t.priority = Some(Priority::High);

        let work = Expr::Atom(Atom::HasTag("work".into()));
        let high = Expr::Atom(Atom::Priority(Priority::High));
        let low = Expr::Atom(Atom::Priority(Priority::Low));

        assert!(Expr::And(vec![work.clone(), high.clone()]).matches(&t));
        assert!(!Expr::And(vec![work.clone(), low.clone()]).matches(&t));
        assert!(Expr::Or(vec![low.clone(), high.clone()]).matches(&t));
        assert!(Expr::Not(Box::new(low.clone())).matches(&t));
        assert!(!Expr::Not(Box::new(high)).matches(&t));
    }
}
