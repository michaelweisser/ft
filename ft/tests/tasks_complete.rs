use assert_cmd::Command;
use assert_fs::prelude::*;
use predicates::prelude::*;

/// Build a minimal vault with `.obsidian/` so vault discovery works. The
/// daily-notes config isn't required for `ft tasks complete`.
fn vault() -> assert_fs::TempDir {
    let dir = assert_fs::TempDir::new().unwrap();
    dir.child(".obsidian").create_dir_all().unwrap();
    dir
}

fn run(vault: &std::path::Path, args: &[&str]) -> assert_cmd::assert::Assert {
    let mut full = vec!["--vault", vault.to_str().unwrap(), "tasks", "complete"];
    full.extend(args);
    Command::cargo_bin("ft")
        .unwrap()
        .env("FT_TODAY", "2026-05-10")
        .args(&full)
        .assert()
}

#[test]
fn complete_by_id_marks_done_with_today() {
    let dir = vault();
    // Source line is in canonical order so re-serialization is a no-op
    // aside from the appended ✅ done date.
    dir.child("notes.md")
        .write_str("- [ ] Buy milk 📅 2026-05-10 🆔 abc123\n")
        .unwrap();

    run(dir.path(), &["abc123"]).success();

    let content = std::fs::read_to_string(dir.path().join("notes.md")).unwrap();
    assert_eq!(
        content,
        "- [x] Buy milk 📅 2026-05-10 ✅ 2026-05-10 🆔 abc123\n"
    );
}

#[test]
fn complete_by_file_line_marks_done() {
    let dir = vault();
    dir.child("notes/inbox.md")
        .write_str("# Inbox\n- [ ] First task\n- [ ] Second task\n")
        .unwrap();

    run(dir.path(), &["notes/inbox.md:3"]).success();

    let content = std::fs::read_to_string(dir.path().join("notes/inbox.md")).unwrap();
    assert_eq!(
        content,
        "# Inbox\n- [ ] First task\n- [x] Second task ✅ 2026-05-10\n"
    );
}

#[test]
fn complete_with_on_date_overrides_today() {
    let dir = vault();
    dir.child("notes.md")
        .write_str("- [ ] Task 🆔 zzz\n")
        .unwrap();
    run(dir.path(), &["zzz", "--on", "2025-12-31"]).success();
    let content = std::fs::read_to_string(dir.path().join("notes.md")).unwrap();
    assert_eq!(content, "- [x] Task ✅ 2025-12-31 🆔 zzz\n");
}

#[test]
fn complete_with_on_relative_date() {
    let dir = vault();
    dir.child("notes.md")
        .write_str("- [ ] Task 🆔 rel\n")
        .unwrap();
    // FT_TODAY=2026-05-10, --on yesterday → 2026-05-09
    run(dir.path(), &["rel", "--on", "yesterday"]).success();
    let content = std::fs::read_to_string(dir.path().join("notes.md")).unwrap();
    assert_eq!(content, "- [x] Task ✅ 2026-05-09 🆔 rel\n");
}

#[test]
fn complete_recurring_task_writes_next_instance() {
    let dir = vault();
    // Canonical order: priority? recurrence due done id depends_on
    dir.child("notes.md")
        .write_str("- [ ] Pay rent 🔁 every month on the 1st 📅 2026-05-01 🆔 rent\n")
        .unwrap();
    run(dir.path(), &["rent", "--on", "2026-05-02"]).success();

    let content = std::fs::read_to_string(dir.path().join("notes.md")).unwrap();
    assert_eq!(
        content,
        "- [ ] Pay rent 🔁 every month on the 1st 📅 2026-06-01 🆔 rent\n\
         - [x] Pay rent 🔁 every month on the 1st 📅 2026-05-01 ✅ 2026-05-02 🆔 rent\n"
    );
}

#[test]
fn complete_no_match_errors() {
    let dir = vault();
    dir.child("notes.md")
        .write_str("- [ ] something else\n")
        .unwrap();
    run(dir.path(), &["nonexistent"])
        .failure()
        .stderr(predicate::str::contains("no tasks match"));
}

#[test]
fn complete_already_done_errors() {
    let dir = vault();
    dir.child("notes.md")
        .write_str("- [x] Already done 🆔 done1 ✅ 2026-05-09\n")
        .unwrap();
    run(dir.path(), &["done1"])
        .failure()
        .stderr(predicate::str::contains("already done"));
}

#[test]
fn complete_ambiguous_with_yes_lists_candidates() {
    let dir = vault();
    dir.child("a.md").write_str("- [ ] Buy milk\n").unwrap();
    dir.child("b.md")
        .write_str("- [ ] Buy milkshakes\n")
        .unwrap();
    run(dir.path(), &["milk", "--yes"]).failure().stderr(
        predicate::str::contains("candidates match").and(predicate::str::contains("Buy milk")),
    );
}

#[test]
fn complete_unique_fuzzy_match_succeeds() {
    let dir = vault();
    dir.child("a.md").write_str("- [ ] Buy milk\n").unwrap();
    dir.child("b.md").write_str("- [ ] Walk dog\n").unwrap();
    run(dir.path(), &["dog"]).success();
    let content = std::fs::read_to_string(dir.path().join("b.md")).unwrap();
    assert_eq!(content, "- [x] Walk dog ✅ 2026-05-10\n");
}

#[test]
fn complete_unsupported_recurrence_errors_and_does_not_modify_file() {
    let dir = vault();
    let original = "- [ ] Yearly thing 🔁 every year 📅 2026-05-10 🆔 yr\n";
    dir.child("notes.md").write_str(original).unwrap();
    run(dir.path(), &["yr"])
        .failure()
        .stderr(predicate::str::contains("unsupported"));
    // File unchanged.
    let content = std::fs::read_to_string(dir.path().join("notes.md")).unwrap();
    assert_eq!(content, original);
}

#[test]
fn complete_round_trip_listed_then_done() {
    // Create → list shows open → complete → list with --status done shows it.
    let dir = vault();
    dir.child("notes.md")
        .write_str("- [ ] Round trip 🆔 rt\n")
        .unwrap();

    run(dir.path(), &["rt"]).success();

    let listed = Command::cargo_bin("ft")
        .unwrap()
        .env("FT_TODAY", "2026-05-10")
        .args([
            "--vault",
            dir.path().to_str().unwrap(),
            "tasks",
            "list",
            "--status",
            "done",
            "--format",
            "markdown",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8(listed.stdout).unwrap();
    assert!(
        stdout.contains("Round trip"),
        "list --status done should include the completed task; got:\n{stdout}"
    );
}
