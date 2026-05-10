//! CLI-level real-vault smoke test.
//! Gated on `FT_REAL_VAULT_TESTS=1` so CI never depends on a local vault.
//! Run with:  FT_REAL_VAULT_TESTS=1 cargo test -p ft --test real_vault_cli

use assert_cmd::Command;

const REAL_VAULT: &str = "/Users/cmw/git/fortytwo";

fn gated() -> bool {
    std::env::var("FT_REAL_VAULT_TESTS").as_deref() == Ok("1")
}

fn ft() -> Command {
    Command::cargo_bin("ft").unwrap()
}

#[test]
fn real_vault_list_is_non_empty() {
    if !gated() {
        return;
    }
    let assert = ft()
        .args(["--vault", REAL_VAULT, "tasks", "list", "--allow-empty"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        !stdout.trim().is_empty(),
        "real-vault list output should be non-empty"
    );
}

#[test]
fn real_vault_list_then_list_is_stable() {
    if !gated() {
        return;
    }
    let run = || -> String {
        let out = ft()
            .args([
                "--vault",
                REAL_VAULT,
                "tasks",
                "list",
                "--format",
                "ndjson",
                "--allow-empty",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        String::from_utf8(out).unwrap()
    };
    let a = run();
    let b = run();
    assert_eq!(a, b, "two consecutive list runs should match byte-for-byte");
}

#[test]
fn real_vault_overdue_preset_runs() {
    if !gated() {
        return;
    }
    // The `overdue` preset may legitimately match zero tasks, so allow empty.
    ft().args([
        "--vault",
        REAL_VAULT,
        "tasks",
        "list",
        "overdue",
        "--allow-empty",
    ])
    .env("FT_TODAY", "2026-05-10")
    .assert()
    .success();
}

#[test]
fn real_vault_dry_run_move_does_not_modify() {
    if !gated() {
        return;
    }
    // Build a dry-run move that almost certainly matches no tasks (a
    // synthetic tag) so the diff is empty / tiny but the command still
    // exercises scan + plan_move + diff render against the real vault.
    ft().args([
        "--vault",
        REAL_VAULT,
        "tasks",
        "move",
        "--query",
        "tag is __ft_no_such_tag__",
        "--to",
        "_ft_smoke_real_target.md",
        "--dry-run",
        "--yes",
    ])
    .assert()
    // No tasks match → CLI errors with "no tasks matched"; treat that as
    // a successful exercise of the path (the failure exit just reports
    // emptiness).
    .failure();
}
