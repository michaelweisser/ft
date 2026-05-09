use std::path::PathBuf;

use super::Task;

pub struct ParseContext {
    pub source_file: PathBuf,
    pub source_line: usize,
}

pub trait TaskFormat {
    /// Parse a single raw line into a `Task`, or return `None` if the line is
    /// not a task (e.g. plain prose, headings, blank lines).
    fn parse_line(&self, line: &str, ctx: ParseContext) -> Option<Task>;

    /// Serialize a task back to the single-line format it was parsed from.
    fn serialize_line(&self, task: &Task) -> String;
}
