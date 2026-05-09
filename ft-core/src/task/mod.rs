use std::path::PathBuf;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

pub mod emoji;
pub mod format;
pub mod hierarchy;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Open,
    Done,
    InProgress,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Priority {
    Highest,
    High,
    Medium,
    Low,
    Lowest,
}

impl Priority {
    pub fn emoji(self) -> &'static str {
        match self {
            Priority::Highest => "🔺",
            Priority::High => "⏫",
            Priority::Medium => "🔼",
            Priority::Low => "🔽",
            Priority::Lowest => "⏬",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub description: String,
    pub status: Status,
    pub priority: Option<Priority>,
    /// Hashtags extracted from the description (e.g. `#work`). The tags remain
    /// in `description` as well; this field is a convenience index.
    pub tags: Vec<String>,
    pub created: Option<NaiveDate>,
    /// 🛫 start date — earliest date to begin working on the task.
    pub start: Option<NaiveDate>,
    /// ⏳ scheduled date — when the task is scheduled to be worked on.
    pub scheduled: Option<NaiveDate>,
    pub due: Option<NaiveDate>,
    pub done: Option<NaiveDate>,
    pub cancelled: Option<NaiveDate>,
    /// Recurrence rule preserved verbatim (e.g. `"every month on the 18th"`).
    pub recurrence: Option<String>,
    pub id: Option<String>,
    pub depends_on: Vec<String>,
    /// Reserved: on-completion action preserved verbatim (not yet parsed).
    pub on_completion: Option<String>,
    /// Obsidian block identifier (the part after `^`).
    pub block_link: Option<String>,
    /// Unknown emoji fields preserved verbatim so no data is lost on rewrite.
    pub raw_trailing: Option<String>,
    pub source_file: PathBuf,
    /// 1-indexed line number within `source_file`.
    pub source_line: usize,
    /// Leading-whitespace byte count (used for hierarchy detection).
    pub indent_level: usize,
    /// `source_line` of the nearest ancestor task with smaller `indent_level`.
    pub parent: Option<usize>,
}
