//! Integration tests for `ft notes rename` (plan 013, session 3).
//!
//! Each test builds a tiny scratch vault in a `TempDir` so we can
//! assert on the post-rename file layout and contents without
//! depending on the shared `tests/fixtures/links/` vault (which is
//! exercised by other test files).

use assert_cmd::Command;
use assert_fs::prelude::*;
use assert_fs::TempDir;
use predicates::prelude::*;

fn ft() -> Command {
    Command::cargo_bin("ft").unwrap()
}

fn make_vault(files: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().unwrap();
    dir.child(".obsidian").create_dir_all().unwrap();
    for (rel, content) in files {
        dir.child(rel).write_str(content).unwrap();
    }
    dir
}

fn read(dir: &TempDir, rel: &str) -> String {
    std::fs::read_to_string(dir.child(rel).path()).unwrap()
}

// ── happy path ───────────────────────────────────────────────────────────────

#[test]
fn rename_simple_wikilink_renames_file_and_updates_linker() {
    let v = make_vault(&[("foo.md", "# Foo\n"), ("a.md", "see [[foo]] now\n")]);
    ft().args([
        "--vault",
        v.path().to_str().unwrap(),
        "notes",
        "rename",
        "foo",
        "bar",
    ])
    .assert()
    .success()
    .stdout(predicate::str::contains("renamed foo.md → bar.md"));
    assert!(!v.child("foo.md").path().exists());
    assert!(v.child("bar.md").path().exists());
    assert_eq!(read(&v, "a.md"), "see [[bar]] now\n");
}

#[test]
fn rename_multi_link_in_one_file_handles_descending_order() {
    let v = make_vault(&[
        ("foo.md", "# Foo\n"),
        ("a.md", "[[foo]] one [[foo]] two [[foo]] three\n"),
    ]);
    ft().args([
        "--vault",
        v.path().to_str().unwrap(),
        "notes",
        "rename",
        "foo",
        "a-much-longer-name",
    ])
    .assert()
    .success();
    assert_eq!(
        read(&v, "a.md"),
        "[[a-much-longer-name]] one [[a-much-longer-name]] two [[a-much-longer-name]] three\n"
    );
}

#[test]
fn rename_preserves_alias_anchor_and_both() {
    let v = make_vault(&[
        ("foo.md", "# Foo\n## H1\n"),
        ("a.md", "[[foo|My Foo]] [[foo#H1]] [[foo#H1|D]]\n"),
    ]);
    ft().args([
        "--vault",
        v.path().to_str().unwrap(),
        "notes",
        "rename",
        "foo",
        "bar",
    ])
    .assert()
    .success();
    assert_eq!(read(&v, "a.md"), "[[bar|My Foo]] [[bar#H1]] [[bar#H1|D]]\n");
}

#[test]
fn rename_path_form_wikilink_keeps_path_form() {
    let v = make_vault(&[
        ("notes/foo.md", "# Foo\n"),
        ("a.md", "see [[notes/foo]] please\n"),
    ]);
    ft().args([
        "--vault",
        v.path().to_str().unwrap(),
        "notes",
        "rename",
        "notes/foo.md",
        "notes/bar.md",
    ])
    .assert()
    .success();
    assert_eq!(read(&v, "a.md"), "see [[notes/bar]] please\n");
}

#[test]
fn rename_md_link_updates_url() {
    let v = make_vault(&[
        ("foo.md", "# Foo\n"),
        ("a.md", "see [Click](foo.md) please\n"),
    ]);
    ft().args([
        "--vault",
        v.path().to_str().unwrap(),
        "notes",
        "rename",
        "foo",
        "bar",
    ])
    .assert()
    .success();
    assert_eq!(read(&v, "a.md"), "see [Click](bar.md) please\n");
}

#[test]
fn rename_embed_keeps_bang_prefix() {
    let v = make_vault(&[("foo.md", "# Foo\n"), ("a.md", "![[foo]]\n")]);
    ft().args([
        "--vault",
        v.path().to_str().unwrap(),
        "notes",
        "rename",
        "foo",
        "bar",
    ])
    .assert()
    .success();
    assert_eq!(read(&v, "a.md"), "![[bar]]\n");
}

#[test]
fn rename_self_link_edits_then_renames() {
    let v = make_vault(&[("foo.md", "see [[foo]] for self-ref\n")]);
    ft().args([
        "--vault",
        v.path().to_str().unwrap(),
        "notes",
        "rename",
        "foo",
        "bar",
    ])
    .assert()
    .success();
    assert!(!v.child("foo.md").path().exists());
    assert_eq!(read(&v, "bar.md"), "see [[bar]] for self-ref\n");
}

// ── ergonomics ───────────────────────────────────────────────────────────────

#[test]
fn rename_bare_name_keeps_source_directory() {
    // foo.md lives in notes/. Pass a bare new name → should land in notes/bar.md.
    let v = make_vault(&[
        ("notes/foo.md", "# Foo\n"),
        ("a.md", "see [[foo]] please\n"),
    ]);
    ft().args([
        "--vault",
        v.path().to_str().unwrap(),
        "notes",
        "rename",
        "notes/foo.md",
        "bar",
    ])
    .assert()
    .success();
    assert!(v.child("notes/bar.md").path().exists());
    assert!(!v.child("bar.md").path().exists());
}

#[test]
fn rename_full_path_moves_file_across_directories() {
    let v = make_vault(&[("foo.md", "# Foo\n"), ("a.md", "[[foo]]\n")]);
    ft().args([
        "--vault",
        v.path().to_str().unwrap(),
        "notes",
        "rename",
        "foo",
        "archive/foo.md",
    ])
    .assert()
    .success();
    assert!(!v.child("foo.md").path().exists());
    assert!(v.child("archive/foo.md").path().exists());
    // Title (filename stem) didn't change → wikilink target is still `foo`.
    assert_eq!(read(&v, "a.md"), "[[foo]]\n");
}

#[test]
fn rename_appends_md_extension_automatically() {
    let v = make_vault(&[("foo.md", "# Foo\n")]);
    ft().args([
        "--vault",
        v.path().to_str().unwrap(),
        "notes",
        "rename",
        "foo",
        "bar", // no .md
    ])
    .assert()
    .success();
    assert!(v.child("bar.md").path().exists());
}

// ── ghost rename ─────────────────────────────────────────────────────────────

#[test]
fn rename_ghost_rewrites_linkers_without_creating_a_file() {
    let v = make_vault(&[
        ("a.md", "see [[Phantom]]\n"),
        ("b.md", "also [[Phantom]]\n"),
    ]);
    ft().args([
        "--vault",
        v.path().to_str().unwrap(),
        "notes",
        "rename",
        "[[Phantom]]",
        "Real",
    ])
    .assert()
    .success()
    .stdout(predicate::str::contains("rewrote 2 ghost link(s)"));
    assert!(!v.child("Real.md").path().exists());
    assert_eq!(read(&v, "a.md"), "see [[Real]]\n");
    assert_eq!(read(&v, "b.md"), "also [[Real]]\n");
}

#[test]
fn rename_unknown_ghost_errors() {
    let v = make_vault(&[("a.md", "no ghosts here\n")]);
    ft().args([
        "--vault",
        v.path().to_str().unwrap(),
        "notes",
        "rename",
        "[[NobodyLinksToThis]]",
        "Real",
    ])
    .assert()
    .failure()
    .stderr(predicate::str::contains("no ghost node"));
}

// ── dry-run ──────────────────────────────────────────────────────────────────

#[test]
fn rename_dry_run_writes_nothing_and_prints_plan() {
    let v = make_vault(&[("foo.md", "# Foo\n"), ("a.md", "[[foo]]\n")]);
    ft().args([
        "--vault",
        v.path().to_str().unwrap(),
        "notes",
        "rename",
        "foo",
        "bar",
        "--dry-run",
    ])
    .assert()
    .success()
    .stdout(predicate::str::contains("would rename: foo.md → bar.md"))
    .stdout(predicate::str::contains("would update 1 link(s)"));
    // File was NOT renamed.
    assert!(v.child("foo.md").path().exists());
    assert!(!v.child("bar.md").path().exists());
    assert_eq!(read(&v, "a.md"), "[[foo]]\n");
}

// ── error paths ──────────────────────────────────────────────────────────────

#[test]
fn rename_to_existing_path_errors_before_any_writes() {
    let v = make_vault(&[
        ("foo.md", "# Foo\n"),
        ("bar.md", "# Bar\n"),
        ("a.md", "[[foo]]\n"),
    ]);
    ft().args([
        "--vault",
        v.path().to_str().unwrap(),
        "notes",
        "rename",
        "foo",
        "bar",
    ])
    .assert()
    .failure()
    .stderr(predicate::str::contains("target already exists"));
    assert!(v.child("foo.md").path().exists());
    assert_eq!(read(&v, "bar.md"), "# Bar\n");
    assert_eq!(read(&v, "a.md"), "[[foo]]\n");
}

#[test]
fn rename_unknown_note_errors() {
    let v = make_vault(&[("foo.md", "# Foo\n")]);
    ft().args([
        "--vault",
        v.path().to_str().unwrap(),
        "notes",
        "rename",
        "definitely-no-such-note-xyzzy",
        "bar",
    ])
    .assert()
    .failure()
    .stderr(predicate::str::contains("no note found"));
}
