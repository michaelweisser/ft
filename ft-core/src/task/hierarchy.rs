use super::Task;

/// Resolve `parent` pointers for a slice of tasks from the same file.
///
/// Tasks must be ordered by `source_line` (ascending). A task becomes the
/// child of the nearest preceding task whose `indent_level` is strictly
/// smaller. After this call every task's `parent` field is either `None`
/// (top-level) or the `source_line` of its direct parent.
pub fn resolve_hierarchy(tasks: &mut [Task]) {
    for i in 1..tasks.len() {
        let current_indent = tasks[i].indent_level;
        // Walk backwards looking for the nearest ancestor.
        for j in (0..i).rev() {
            if tasks[j].indent_level < current_indent {
                let parent_line = tasks[j].source_line;
                tasks[i].parent = Some(parent_line);
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{
        emoji::EmojiFormat,
        format::{ParseContext, TaskFormat},
    };
    use std::path::PathBuf;

    fn parse_tasks(lines: &[&str]) -> Vec<Task> {
        let path = PathBuf::from("test.md");
        lines
            .iter()
            .enumerate()
            .filter_map(|(i, line)| {
                EmojiFormat.parse_line(
                    line,
                    ParseContext {
                        source_file: path.clone(),
                        source_line: i + 1,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn no_children_when_all_same_indent() {
        let mut tasks = parse_tasks(&["- [ ] task A", "- [ ] task B", "- [ ] task C"]);
        resolve_hierarchy(&mut tasks);
        for t in &tasks {
            assert!(t.parent.is_none(), "flat tasks should have no parent");
        }
    }

    #[test]
    fn single_level_children() {
        let mut tasks = parse_tasks(&["- [ ] parent", "  - [ ] child A", "  - [ ] child B"]);
        resolve_hierarchy(&mut tasks);
        assert!(tasks[0].parent.is_none());
        // source_line of parent is 1 (1-indexed)
        assert_eq!(tasks[1].parent, Some(1));
        assert_eq!(tasks[2].parent, Some(1));
    }

    #[test]
    fn two_level_nesting() {
        let mut tasks = parse_tasks(&["- [ ] grandparent", "  - [ ] parent", "    - [ ] child"]);
        resolve_hierarchy(&mut tasks);
        assert!(tasks[0].parent.is_none());
        assert_eq!(tasks[1].parent, Some(1)); // parent's parent = grandparent (line 1)
        assert_eq!(tasks[2].parent, Some(2)); // child's parent = parent (line 2)
    }

    #[test]
    fn three_level_nesting() {
        let mut tasks = parse_tasks(&[
            "- [ ] L0",
            "  - [ ] L1",
            "    - [ ] L2a",
            "    - [ ] L2b",
            "  - [ ] L1b",
        ]);
        resolve_hierarchy(&mut tasks);
        assert!(tasks[0].parent.is_none()); // L0: no parent
        assert_eq!(tasks[1].parent, Some(1)); // L1 → L0 (line 1)
        assert_eq!(tasks[2].parent, Some(2)); // L2a → L1 (line 2)
        assert_eq!(tasks[3].parent, Some(2)); // L2b → L1 (line 2)
        assert_eq!(tasks[4].parent, Some(1)); // L1b → L0 (line 1)
    }

    #[test]
    fn mixed_statuses_in_hierarchy() {
        let mut tasks = parse_tasks(&[
            "- [ ] open parent",
            "  - [x] done child ✅ 2026-05-01",
            "  - [-] cancelled child ❌ 2026-05-02",
        ]);
        resolve_hierarchy(&mut tasks);
        assert_eq!(tasks[1].parent, Some(1));
        assert_eq!(tasks[2].parent, Some(1));
    }
}
