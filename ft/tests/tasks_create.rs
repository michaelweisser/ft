use assert_cmd::Command;
use assert_fs::prelude::*;
use predicates::prelude::*;

/// Build a temp vault with a `.obsidian/daily-notes.json` config so the
/// default `--file` resolution works.
fn vault_with_daily(folder: &str, format: Option<&str>) -> assert_fs::TempDir {
    let dir = assert_fs::TempDir::new().unwrap();
    dir.child(".obsidian").create_dir_all().unwrap();
    let cfg = match format {
        Some(f) => format!(
            r#"{{"folder":"{folder}","format":"{f}","autorun":true}}"#,
            folder = folder,
            f = f
        ),
        None => format!(r#"{{"folder":"{folder}","autorun":true}}"#, folder = folder),
    };
    dir.child(".obsidian/daily-notes.json")
        .write_str(&cfg)
        .unwrap();
    dir
}

fn run(vault: &std::path::Path, args: &[&str]) -> assert_cmd::assert::Assert {
    let mut full = vec!["--vault", vault.to_str().unwrap(), "tasks", "create"];
    full.extend(args);
    Command::cargo_bin("ft")
        .unwrap()
        .env("FT_TODAY", "2026-05-09")
        .args(&full)
        .assert()
}

#[test]
fn create_simple_task_in_daily_note() {
    let dir = vault_with_daily("journal", Some("YYYY-MM-DD"));
    run(dir.path(), &["Buy milk", "--due", "tomorrow"]).success();

    let content = std::fs::read_to_string(dir.path().join("journal/2026-05-09.md")).unwrap();
    assert_eq!(content, "- [ ] Buy milk 📅 2026-05-10\n");
}

#[test]
fn create_with_priority_and_tags() {
    let dir = vault_with_daily("journal", Some("YYYY-MM-DD"));
    run(
        dir.path(),
        &[
            "Read book",
            "--priority",
            "high",
            "--tag",
            "work",
            "--tag",
            "books",
        ],
    )
    .success();
    let content = std::fs::read_to_string(dir.path().join("journal/2026-05-09.md")).unwrap();
    assert_eq!(content, "- [ ] Read book #work #books ⏫\n");
}

#[test]
fn create_with_explicit_file_relative_to_vault() {
    let dir = vault_with_daily("journal", None);
    run(
        dir.path(),
        &["Take call", "--file", "inbox/calls.md", "--due", "+1w"],
    )
    .success();
    let content = std::fs::read_to_string(dir.path().join("inbox/calls.md")).unwrap();
    assert_eq!(content, "- [ ] Take call 📅 2026-05-16\n");
}

#[test]
fn create_under_heading_existing() {
    let dir = vault_with_daily("journal", None);
    let f = dir.child("notes.md");
    f.write_str("# Notes\n\n## Tasks\n- [ ] existing\n\n## Other\n")
        .unwrap();
    run(
        dir.path(),
        &["New task", "--file", "notes.md", "--under-heading", "Tasks"],
    )
    .success();
    let content = std::fs::read_to_string(f.path()).unwrap();
    assert!(content.contains("- [ ] existing\n- [ ] New task"));
}

#[test]
fn create_under_heading_creates_missing_heading() {
    let dir = vault_with_daily("journal", None);
    let f = dir.child("notes.md");
    f.write_str("# Notes\n").unwrap();
    run(
        dir.path(),
        &["New task", "--file", "notes.md", "--under-heading", "Tasks"],
    )
    .success();
    let content = std::fs::read_to_string(f.path()).unwrap();
    assert!(content.contains("## Tasks"));
    assert!(content.contains("- [ ] New task"));
}

#[test]
fn create_at_line_inserts_at_position() {
    let dir = vault_with_daily("journal", None);
    let f = dir.child("notes.md");
    f.write_str("a\nb\nc\n").unwrap();
    run(
        dir.path(),
        &["New task", "--file", "notes.md", "--at-line", "2"],
    )
    .success();
    let content = std::fs::read_to_string(f.path()).unwrap();
    assert_eq!(content, "a\n- [ ] New task\nb\nc\n");
}

#[test]
fn duplicate_refused() {
    let dir = vault_with_daily("journal", Some("YYYY-MM-DD"));
    run(dir.path(), &["Buy milk", "--due", "tomorrow"]).success();
    run(dir.path(), &["Buy milk", "--due", "tomorrow"])
        .failure()
        .stderr(predicate::str::contains("duplicate task"));
}

#[test]
fn duplicate_inserted_with_force() {
    let dir = vault_with_daily("journal", Some("YYYY-MM-DD"));
    run(dir.path(), &["Buy milk", "--due", "tomorrow"]).success();
    run(dir.path(), &["Buy milk", "--due", "tomorrow", "--force"]).success();
    let content = std::fs::read_to_string(dir.path().join("journal/2026-05-09.md")).unwrap();
    assert_eq!(content.matches("- [ ] Buy milk 📅 2026-05-10").count(), 2);
}

#[test]
fn invalid_date_rejected() {
    let dir = vault_with_daily("journal", Some("YYYY-MM-DD"));
    run(dir.path(), &["Bad", "--due", "zzznotadate"])
        .failure()
        .stderr(predicate::str::contains("--due"));
}

#[test]
fn duplicate_error_uses_relative_path() {
    let dir = vault_with_daily("journal", Some("YYYY-MM-DD"));
    run(dir.path(), &["Buy milk", "--due", "tomorrow"]).success();
    let assert = run(dir.path(), &["Buy milk", "--due", "tomorrow"]).failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("journal/2026-05-09.md"),
        "expected vault-relative path; got: {stderr}"
    );
    assert!(
        !stderr.contains(dir.path().to_str().unwrap()),
        "stderr should not contain absolute path; got: {stderr}"
    );
}

#[test]
fn round_trip_create_then_list() {
    let dir = vault_with_daily("journal", Some("YYYY-MM-DD"));
    run(dir.path(), &["Buy milk", "--due", "tomorrow"]).success();

    let assert = Command::cargo_bin("ft")
        .unwrap()
        .env("FT_TODAY", "2026-05-09")
        .args([
            "--vault",
            dir.path().to_str().unwrap(),
            "tasks",
            "list",
            "--format",
            "json",
            "--no-color",
        ])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let arr = v.as_array().unwrap();
    let descs: Vec<&str> = arr
        .iter()
        .map(|t| t["description"].as_str().unwrap())
        .collect();
    assert!(descs.contains(&"Buy milk"));
}

#[test]
fn missing_daily_notes_config_explains_remedy() {
    // Vault with no daily-notes.json and no --file should fail with hint.
    let dir = assert_fs::TempDir::new().unwrap();
    dir.child(".obsidian").create_dir_all().unwrap();
    Command::cargo_bin("ft")
        .unwrap()
        .env("FT_TODAY", "2026-05-09")
        .args([
            "--vault",
            dir.path().to_str().unwrap(),
            "tasks",
            "create",
            "Stuff",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--file"));
}

#[test]
fn periodic_notes_source_resolves_daily_path() {
    let dir = assert_fs::TempDir::new().unwrap();
    dir.child(".obsidian/plugins/periodic-notes")
        .create_dir_all()
        .unwrap();
    dir.child(".obsidian/plugins/periodic-notes/data.json")
        .write_str(r#"{"daily":{"folder":"journal/2026","format":"","enabled":true}}"#)
        .unwrap();
    dir.child(".ft/config.toml")
        .write_str(
            r#"
[daily_notes]
source = "periodic-notes"
"#,
        )
        .unwrap();

    run(dir.path(), &["Buy milk", "--due", "tomorrow"]).success();
    let content = std::fs::read_to_string(dir.path().join("journal/2026/2026-05-09.md")).unwrap();
    assert_eq!(content, "- [ ] Buy milk 📅 2026-05-10\n");
}

#[test]
fn explicit_source_with_path_pattern() {
    // [daily_notes].source = "explicit" with a YYYY pattern in `path` keeps
    // working as the year rolls over without reconfiguring.
    let dir = assert_fs::TempDir::new().unwrap();
    dir.child(".obsidian").create_dir_all().unwrap();
    dir.child(".ft/config.toml")
        .write_str(
            r#"
[daily_notes]
source = "explicit"
path = "journal/YYYY"
format = "YYYY-MM-DD"
"#,
        )
        .unwrap();

    run(dir.path(), &["Buy milk", "--due", "tomorrow"]).success();
    let content = std::fs::read_to_string(dir.path().join("journal/2026/2026-05-09.md")).unwrap();
    assert_eq!(content, "- [ ] Buy milk 📅 2026-05-10\n");
}

#[test]
fn explicit_source_missing_path_errors() {
    let dir = assert_fs::TempDir::new().unwrap();
    dir.child(".obsidian").create_dir_all().unwrap();
    dir.child(".ft/config.toml")
        .write_str(
            r#"
[daily_notes]
source = "explicit"
"#,
        )
        .unwrap();
    Command::cargo_bin("ft")
        .unwrap()
        .env("FT_TODAY", "2026-05-09")
        .args([
            "--vault",
            dir.path().to_str().unwrap(),
            "tasks",
            "create",
            "Stuff",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("path"));
}

#[test]
fn periodic_notes_disabled_errors_with_hint() {
    let dir = assert_fs::TempDir::new().unwrap();
    dir.child(".obsidian/plugins/periodic-notes")
        .create_dir_all()
        .unwrap();
    dir.child(".obsidian/plugins/periodic-notes/data.json")
        .write_str(r#"{"daily":{"folder":"journal","format":"","enabled":false}}"#)
        .unwrap();
    dir.child(".ft/config.toml")
        .write_str(
            r#"[daily_notes]
source = "periodic-notes"
"#,
        )
        .unwrap();
    Command::cargo_bin("ft")
        .unwrap()
        .env("FT_TODAY", "2026-05-09")
        .args([
            "--vault",
            dir.path().to_str().unwrap(),
            "tasks",
            "create",
            "Stuff",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("disabled"));
}

#[test]
fn description_collected_from_multiple_args() {
    let dir = vault_with_daily("journal", Some("YYYY-MM-DD"));
    run(dir.path(), &["Buy", "milk", "and", "bread"]).success();
    let content = std::fs::read_to_string(dir.path().join("journal/2026-05-09.md")).unwrap();
    assert_eq!(content, "- [ ] Buy milk and bread\n");
}

#[test]
fn recurrence_id_and_depends_on() {
    let dir = vault_with_daily("journal", Some("YYYY-MM-DD"));
    run(
        dir.path(),
        &[
            "Pay tax",
            "--due",
            "2026-05-18",
            "--recurrence",
            "every month on the 18th",
            "--id",
            "tax42",
            "--depends-on",
            "abc",
            "--depends-on",
            "def",
        ],
    )
    .success();
    let content = std::fs::read_to_string(dir.path().join("journal/2026-05-09.md")).unwrap();
    assert!(content.contains("🔁 every month on the 18th"));
    assert!(content.contains("📅 2026-05-18"));
    assert!(content.contains("🆔 tax42"));
    assert!(content.contains("⛔ abc,def"));
}
