//! Integration tests for `ft find` — the CLI surface for the fuzzy
//! file + heading search from plan 005.

use assert_cmd::Command;
use predicates::prelude::*;

fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("ft crate must have a parent (workspace root)")
        .to_path_buf()
}

fn realistic_vault() -> std::path::PathBuf {
    workspace_root().join("tests/fixtures/realistic")
}

fn ft() -> Command {
    Command::cargo_bin("ft").unwrap()
}

#[test]
fn find_filename_matches_print_path() {
    let v = realistic_vault();
    ft().args([
        "--vault",
        v.to_str().unwrap(),
        "find",
        "finance",
        "--no-color",
    ])
    .assert()
    .success()
    .stdout(predicate::str::contains("Areas/finance.md"));
}

#[test]
fn find_with_heading_part_matches_inside_file() {
    let v = realistic_vault();
    // The website-redesign note has a `## Tasks` heading.
    let out = ft()
        .args([
            "--vault",
            v.to_str().unwrap(),
            "find",
            "redesign#tasks",
            "--no-color",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    assert!(
        stdout.contains("website-redesign.md"),
        "missing expected file:\n{stdout}"
    );
    assert!(
        stdout.contains("Tasks"),
        "heading should appear in plain output:\n{stdout}"
    );
    // Plain output format is `path:line\theading`.
    let first_line = stdout.lines().next().unwrap();
    assert!(
        first_line.contains(':') && first_line.contains('\t'),
        "first line should have `path:line<tab>heading` shape: {first_line:?}"
    );
}

#[test]
fn find_no_matches_exits_one() {
    let v = realistic_vault();
    ft().args([
        "--vault",
        v.to_str().unwrap(),
        "find",
        "zzzzzzzzz",
        "--no-color",
    ])
    .assert()
    .code(1)
    .stdout(predicate::str::is_empty());
}

#[test]
fn find_ndjson_emits_one_object_per_line() {
    let v = realistic_vault();
    let out = ft()
        .args([
            "--vault",
            v.to_str().unwrap(),
            "find",
            "finance",
            "--format",
            "ndjson",
            "--no-color",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    assert!(!stdout.trim().is_empty());
    for line in stdout.lines() {
        let v: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("invalid JSON: {line:?}: {e}"));
        assert!(v.get("path").is_some(), "missing path field: {line}");
        assert!(v.get("score").is_some(), "missing score field: {line}");
    }
}

#[test]
fn find_ndjson_includes_heading_fields_when_query_targets_headings() {
    let v = realistic_vault();
    let out = ft()
        .args([
            "--vault",
            v.to_str().unwrap(),
            "find",
            "redesign#tasks",
            "--format",
            "ndjson",
            "--no-color",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    let first: serde_json::Value = serde_json::from_str(stdout.lines().next().unwrap()).unwrap();
    assert!(first["heading"].is_string(), "heading missing: {first}");
    assert!(first["level"].is_number(), "level missing: {first}");
    assert!(first["line"].is_number(), "line missing: {first}");
}

#[test]
fn find_limit_truncates_output() {
    let v = realistic_vault();
    let out = ft()
        .args([
            "--vault",
            v.to_str().unwrap(),
            "find",
            ".md",
            "--limit",
            "2",
            "--no-color",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    assert_eq!(
        stdout.lines().count(),
        2,
        "expected exactly 2 lines with --limit=2:\n{stdout}"
    );
}

#[test]
fn find_include_headings_attaches_first_heading_without_hash_in_query() {
    let v = realistic_vault();
    let out = ft()
        .args([
            "--vault",
            v.to_str().unwrap(),
            "find",
            "finance",
            "--include-headings",
            "--format",
            "ndjson",
            "--no-color",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    let first: serde_json::Value = serde_json::from_str(stdout.lines().next().unwrap()).unwrap();
    assert!(
        first["heading"].is_string(),
        "expected a heading attached when --include-headings is set: {first}"
    );
}

#[test]
fn find_plain_no_color_has_no_ansi_escape_sequences() {
    let v = realistic_vault();
    let out = ft()
        .args([
            "--vault",
            v.to_str().unwrap(),
            "find",
            "finance",
            "--no-color",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    assert!(
        !stdout.contains('\x1b'),
        "--no-color output should not contain ANSI escapes: {stdout:?}"
    );
}

// --- real-vault gated smoke check ------------------------------------------

const REAL_VAULT: &str = "/Users/cmw/git/fortytwo";

fn real_vault_enabled() -> bool {
    if std::env::var("FT_REAL_VAULT_TESTS").as_deref() != Ok("1") {
        return false;
    }
    std::path::Path::new(REAL_VAULT).exists()
}

#[test]
fn find_against_real_vault_runs() {
    if !real_vault_enabled() {
        return;
    }
    // Pick a query that's nearly certain to find something in any non-empty
    // vault: every Obsidian vault has at least one markdown file. The empty
    // query is rejected by clap so use `.md` which fuzzy-matches anything
    // with an extension token.
    ft().args([
        "--vault",
        REAL_VAULT,
        "find",
        ".md",
        "--limit",
        "5",
        "--no-color",
    ])
    .assert()
    .success();
}
