/// Real-vault round-trip smoke test.
/// Gated on `FT_REAL_VAULT_TESTS=1` so CI never depends on a local vault.
/// Run with:  FT_REAL_VAULT_TESTS=1 cargo test -p ft-core --test real_vault
use ft_core::task::{
    emoji::EmojiFormat,
    format::{ParseContext, TaskFormat},
};
use std::path::{Path, PathBuf};

fn walk_md(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name.starts_with('.') {
            continue; // skip .obsidian, .git, etc.
        }
        if path.is_dir() {
            walk_md(&path, out);
        } else if path.extension().map(|e| e == "md").unwrap_or(false) {
            out.push(path);
        }
    }
}

#[test]
fn real_vault_round_trip() {
    if std::env::var("FT_REAL_VAULT_TESTS").as_deref() != Ok("1") {
        return;
    }

    let vault = PathBuf::from("/Users/cmw/git/fortytwo");
    let mut files = Vec::new();
    walk_md(&vault, &mut files);

    let mut parsed = 0usize;
    let mut skipped = 0usize;
    let mut mismatches: Vec<(String, String, PathBuf, usize)> = Vec::new();

    for file in &files {
        let content = match std::fs::read_to_string(file) {
            Ok(c) => c,
            Err(_) => continue,
        };
        for (lineno, line) in content.lines().enumerate() {
            let trimmed = line.trim_start();
            if !trimmed.starts_with("- [") {
                continue;
            }
            // Skip Templater template variables (not real task lines).
            if line.contains("<%") {
                skipped += 1;
                continue;
            }
            let ctx = ParseContext {
                source_file: file.clone(),
                source_line: lineno + 1,
            };
            let Some(task) = EmojiFormat.parse_line(line, ctx) else {
                skipped += 1;
                continue;
            };
            let serialized = EmojiFormat.serialize_line(&task);
            if serialized != line {
                // Known acceptable mismatches:
                // 1. Trailing whitespace: editing artifacts in the source file;
                //    the parser trims them (insignificant).
                // 2. Unknown status markers (e.g. `[!]`): parsed as Open per
                //    spec; the original marker is not preserved.
                let trailing_space_only = line.trim_end() == serialized;
                let status_mismatch = {
                    let marker = line.trim_start().get(3..4).unwrap_or("");
                    !matches!(marker, " " | "x" | "X" | "/" | "-")
                };
                if !trailing_space_only && !status_mismatch {
                    mismatches.push((line.to_string(), serialized, file.clone(), lineno + 1));
                }
            }
            parsed += 1;
        }
    }

    if !mismatches.is_empty() {
        let mut msg = format!(
            "{} round-trip mismatches out of {} tasks ({} skipped):\n",
            mismatches.len(),
            parsed,
            skipped
        );
        for (orig, got, file, line) in mismatches.iter().take(20) {
            msg.push_str(&format!("  {}:{line}\n", file.display()));
            msg.push_str(&format!("    orig: {orig:?}\n"));
            msg.push_str(&format!("    got:  {got:?}\n"));
        }
        panic!("{msg}");
    }

    println!("real_vault_round_trip: {parsed} tasks OK, {skipped} skipped, 0 mismatches");
}
