//! Tests for session-8 polish features: shell completions, man pages, and
//! the global `--json-errors` flag.

use assert_cmd::Command;
use predicates::prelude::*;

fn ft() -> Command {
    Command::cargo_bin("ft").unwrap()
}

#[test]
fn completions_bash_emits_recognizable_script() {
    ft().args(["completions", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("_ft()").and(predicate::str::contains("COMPREPLY")));
}

#[test]
fn completions_zsh_emits_compdef() {
    ft().args(["completions", "zsh"])
        .assert()
        .success()
        .stdout(predicate::str::contains("#compdef ft"));
}

#[test]
fn completions_fish_emits_complete_directives() {
    ft().args(["completions", "fish"])
        .assert()
        .success()
        .stdout(predicate::str::contains("complete -c ft"));
}

#[test]
fn completions_unknown_shell_errors() {
    ft().args(["completions", "klingon"]).assert().failure();
}

#[test]
fn man_default_emits_top_level_page_to_stdout() {
    ft().arg("man")
        .assert()
        .success()
        .stdout(predicate::str::contains(".TH ft 1").and(predicate::str::contains(".SH NAME")));
}

#[test]
fn man_out_dir_writes_pages_per_subcommand() {
    let dir = assert_fs::TempDir::new().unwrap();
    ft().args(["man", "--out"])
        .arg(dir.path())
        .assert()
        .success();

    // Top-level + each subcommand (excluding meta-subcommands).
    for expected in &[
        "ft.1",
        "ft-vault.1",
        "ft-tasks.1",
        "ft-tasks-list.1",
        "ft-tasks-create.1",
        "ft-tasks-complete.1",
        "ft-tasks-move.1",
    ] {
        let p = dir.path().join(expected);
        assert!(p.exists(), "expected {} to exist", p.display());
        let contents = std::fs::read_to_string(&p).unwrap();
        assert!(contents.contains(".TH "), "{expected} missing .TH header");
    }
    // Meta-subcommands should NOT have their own man pages.
    for skipped in &["ft-completions.1", "ft-man.1"] {
        let p = dir.path().join(skipped);
        assert!(!p.exists(), "did not expect {} to exist", p.display());
    }
}

#[test]
fn json_errors_emits_object_with_error_and_chain() {
    let assert = ft()
        .args(["--json-errors", "--vault", "/tmp/__ft_no_such_vault__"])
        .args(["vault"])
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    let line = stderr
        .lines()
        .find(|l| l.starts_with('{'))
        .unwrap_or_else(|| panic!("expected a JSON object on stderr; got:\n{stderr}"));
    let parsed: serde_json::Value = serde_json::from_str(line)
        .unwrap_or_else(|e| panic!("stderr line is not JSON ({e}):\n{line}"));
    assert!(parsed.get("error").and_then(|v| v.as_str()).is_some());
    assert!(parsed.get("chain").and_then(|v| v.as_array()).is_some());
}

#[test]
fn human_errors_path_unchanged_without_flag() {
    let assert = ft()
        .args(["--vault", "/tmp/__ft_no_such_vault__", "vault"])
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    // Should NOT be a single JSON line — it's the human-formatted anyhow chain.
    assert!(
        stderr.contains("Error:"),
        "expected human error; got:\n{stderr}"
    );
    assert!(
        !stderr.trim().starts_with('{'),
        "should not be JSON without the flag; got:\n{stderr}"
    );
}
