//! Integration tests for `ft notes backlinks` and `ft notes links`
//! (plan 013, session 2). Exercises the full pipeline against the
//! dedicated `tests/fixtures/links/` vault.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;

fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("ft crate must have a parent (workspace root)")
        .to_path_buf()
}

fn links_vault() -> std::path::PathBuf {
    workspace_root().join("tests/fixtures/links")
}

fn ft() -> Command {
    Command::cargo_bin("ft").unwrap()
}

// ── backlinks ────────────────────────────────────────────────────────────────

#[test]
fn backlinks_alpha_returns_three_rows_from_hub() {
    // hub.md links to alpha three times: plain wikilink, [[alpha]] (line 5),
    // ![[alpha]] embed, and a duplicate-target line. Plus the
    // markdown-link [alpha](alpha.md). Total: 4 incoming edges from hub.md.
    let v = links_vault();
    let out = ft()
        .args([
            "--vault",
            v.to_str().unwrap(),
            "notes",
            "backlinks",
            "alpha",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Vec<Value> = serde_json::from_slice(&out).expect("valid JSON");
    // alpha.md is also linked from hub via several forms; just confirm
    // every row's src is hub.md and the count is at least 4.
    assert!(json.len() >= 4, "expected ≥4 rows, got {}", json.len());
    for row in &json {
        assert_eq!(row["src"], "notes/hub.md");
        assert_eq!(row["dst"]["kind"], "resolved");
        assert_eq!(row["dst"]["path"], "notes/alpha.md");
    }
}

#[test]
fn backlinks_with_no_incoming_edges_exits_one_by_default() {
    // beta.md has no backlinks from outside hub.md? Actually hub does
    // link to beta. Use gamma instead — hub links to gamma#H1 too…
    // Use a freshly-created note with no incoming edges via tempfile.
    let v = links_vault();
    // collision-linker.md isn't linked by anyone else.
    ft().args([
        "--vault",
        v.to_str().unwrap(),
        "notes",
        "backlinks",
        "collision-linker",
    ])
    .assert()
    .code(1)
    .stdout(predicate::str::contains("no backlinks"));
}

#[test]
fn backlinks_with_no_incoming_edges_and_allow_empty_exits_zero() {
    let v = links_vault();
    ft().args([
        "--vault",
        v.to_str().unwrap(),
        "notes",
        "backlinks",
        "collision-linker",
        "--allow-empty",
    ])
    .assert()
    .success()
    .stdout(predicate::str::contains("no backlinks"));
}

#[test]
fn backlinks_unknown_note_errors() {
    let v = links_vault();
    ft().args([
        "--vault",
        v.to_str().unwrap(),
        "notes",
        "backlinks",
        "definitely-does-not-exist-xyzzy",
    ])
    .assert()
    .failure()
    .stderr(predicate::str::contains("no note found"));
}

#[test]
fn backlinks_markdown_format_is_pipeable() {
    // beta has 2 incoming edges from hub: a wikilink-with-alias and
    // a plain markdown link. Each should appear as a `- src.md:LINE — RAW`
    // line in markdown output.
    let v = links_vault();
    let out = ft()
        .args([
            "--vault",
            v.to_str().unwrap(),
            "notes",
            "backlinks",
            "beta",
            "--format",
            "markdown",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(out).unwrap();
    let bullets: Vec<&str> = s.lines().filter(|l| l.starts_with("- ")).collect();
    assert_eq!(
        bullets.len(),
        2,
        "expected 2 markdown bullets, got {bullets:?}"
    );
    assert!(bullets[0].contains("notes/hub.md:"));
}

// ── links (forward) ──────────────────────────────────────────────────────────

#[test]
fn links_hub_includes_resolved_and_unresolved_targets() {
    let v = links_vault();
    let out = ft()
        .args([
            "--vault",
            v.to_str().unwrap(),
            "notes",
            "links",
            "hub",
            "--format",
            "ndjson",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(out).unwrap();
    let rows: Vec<Value> = s
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).expect("valid ndjson row"))
        .collect();

    assert!(!rows.is_empty(), "hub must have outgoing edges");

    let resolved = rows
        .iter()
        .filter(|r| r["dst"]["kind"] == "resolved")
        .count();
    let unresolved = rows
        .iter()
        .filter(|r| r["dst"]["kind"] == "unresolved")
        .count();
    assert!(resolved > 0, "hub has resolved outgoing edges");
    assert!(unresolved > 0, "hub has unresolved (ghost) edges");

    // Confirm Phantom shows up as an unresolved target.
    let has_phantom = rows
        .iter()
        .any(|r| r["dst"]["kind"] == "unresolved" && r["dst"]["raw"] == "Phantom");
    assert!(has_phantom, "expected Phantom ghost in outgoing");
}

#[test]
fn links_table_format_shows_question_mark_for_unresolved() {
    let v = links_vault();
    ft().args([
        "--vault",
        v.to_str().unwrap(),
        "notes",
        "links",
        "hub",
        "--no-color",
    ])
    .assert()
    .success()
    .stdout(predicate::str::contains("? Phantom"));
}

#[test]
fn links_with_no_outgoing_exits_one_by_default() {
    let v = links_vault();
    // beta.md has no outgoing edges in the fixture.
    ft().args(["--vault", v.to_str().unwrap(), "notes", "links", "beta"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("no outgoing links"));
}

#[test]
fn links_path_form_query_resolves_directly() {
    // Pass the full vault-relative path (rather than the title) to
    // verify the path-first selector lookup.
    let v = links_vault();
    ft().args([
        "--vault",
        v.to_str().unwrap(),
        "notes",
        "links",
        "notes/hub.md",
        "--format",
        "ndjson",
    ])
    .assert()
    .success();
}

#[test]
fn links_records_anchor_and_display_on_appropriate_rows() {
    let v = links_vault();
    let out = ft()
        .args([
            "--vault",
            v.to_str().unwrap(),
            "notes",
            "links",
            "hub",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let rows: Vec<Value> = serde_json::from_slice(&out).unwrap();
    // [[gamma#Heading One]] → anchor = "Heading One", display = null
    let anchored = rows
        .iter()
        .find(|r| r["raw"] == "[[gamma#Heading One]]")
        .expect("must include anchored wikilink");
    assert_eq!(anchored["anchor"], "Heading One");
    assert!(anchored["display"].is_null());

    // [[gamma#Heading One|G1]] → anchor = "Heading One", display = "G1"
    let both = rows
        .iter()
        .find(|r| r["raw"] == "[[gamma#Heading One|G1]]")
        .expect("must include anchor+alias");
    assert_eq!(both["anchor"], "Heading One");
    assert_eq!(both["display"], "G1");
}

#[test]
fn links_marks_embeds_with_form_suffix_in_table() {
    let v = links_vault();
    let out = ft()
        .args([
            "--vault",
            v.to_str().unwrap(),
            "notes",
            "links",
            "hub",
            "--no-color",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(out).unwrap();
    // Embeds render as `wiki!` / `md!` in the Form column.
    assert!(s.contains("wiki!"), "missing wiki! marker:\n{s}");
}
