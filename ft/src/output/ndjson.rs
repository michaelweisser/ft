//! Newline-delimited JSON output: one task per line, each line a self-contained
//! JSON object. Pipeable into `jq`, `gron`, log processors, etc.

use anyhow::Result;
use ft_core::task::Task;
use std::io::Write;

pub fn render(tasks: &[&Task]) -> Result<()> {
    let mut out = std::io::stdout().lock();
    for task in tasks {
        serde_json::to_writer(&mut out, task)?;
        writeln!(out)?;
    }
    Ok(())
}
