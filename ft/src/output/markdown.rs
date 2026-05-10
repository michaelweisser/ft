//! Markdown output: emit each task as a serialized source line so the result
//! can be piped back into a vault as a valid markdown task list.

use ft_core::task::emoji::EmojiFormat;
use ft_core::task::{format::TaskFormat, Task};

pub fn render(tasks: &[&Task]) -> String {
    let fmt = EmojiFormat;
    let mut out = String::new();
    for task in tasks {
        out.push_str(&fmt.serialize_line(task));
        out.push('\n');
    }
    out
}
