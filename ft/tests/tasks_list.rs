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

fn run_list(args: &[&str]) -> assert_cmd::assert::Assert {
    let vault = realistic_vault();
    let mut full = vec!["--vault", vault.to_str().unwrap(), "tasks", "list"];
    full.extend(args);
    Command::cargo_bin("ft")
        .unwrap()
        .env("FT_TODAY", "2026-05-09")
        .args(&full)
        .assert()
}

fn pathological_vault() -> std::path::PathBuf {
    workspace_root().join("tests/fixtures/pathological")
}

fn run_list_in(vault: &std::path::Path, args: &[&str]) -> assert_cmd::assert::Assert {
    let mut full = vec!["--vault", vault.to_str().unwrap(), "tasks", "list"];
    full.extend(args);
    Command::cargo_bin("ft")
        .unwrap()
        .env("FT_TODAY", "2026-05-09")
        .args(&full)
        .assert()
}

fn json_tasks(args: &[&str]) -> serde_json::Value {
    let mut full: Vec<&str> = vec!["--format", "json", "--no-color"];
    full.extend(args);
    let assert = run_list(&full).success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    serde_json::from_str(&stdout).expect("ft tasks list --format json must produce valid JSON")
}

#[test]
fn list_table_default_runs() {
    run_list(&["--no-color"])
        .success()
        .stdout(predicate::str::contains("Description"));
}

#[test]
fn list_json_is_parseable() {
    let v = json_tasks(&[]);
    assert!(v.is_array(), "JSON output must be an array");
    let arr = v.as_array().unwrap();
    assert!(!arr.is_empty(), "realistic fixture has tasks");
}

#[test]
fn filter_status_open_excludes_done() {
    let v = json_tasks(&["--status", "open"]);
    let arr = v.as_array().unwrap();
    assert!(!arr.is_empty());
    for t in arr {
        assert_eq!(t["status"], "Open", "every task must be Open");
    }
}

#[test]
fn filter_status_done_only() {
    let v = json_tasks(&["--status", "done"]);
    let arr = v.as_array().unwrap();
    for t in arr {
        assert_eq!(t["status"], "Done");
    }
}

#[test]
fn filter_priority_high() {
    let v = json_tasks(&["--priority", "high"]);
    let arr = v.as_array().unwrap();
    assert!(
        !arr.is_empty(),
        "realistic fixture has at least one ⏫ task"
    );
    for t in arr {
        assert_eq!(t["priority"], "High");
    }
}

#[test]
fn filter_tag_strips_hash() {
    let v_with_hash = json_tasks(&["--tag", "#area/health"]);
    let v_without = json_tasks(&["--tag", "area/health"]);
    assert_eq!(v_with_hash, v_without, "leading # should be optional");
    let arr = v_with_hash.as_array().unwrap();
    assert!(!arr.is_empty());
    for t in arr {
        let tags: Vec<&str> = t["tags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap())
            .collect();
        assert!(tags.contains(&"area/health"));
    }
}

#[test]
fn filter_path_substring() {
    let v = json_tasks(&["--path", "Projects/"]);
    let arr = v.as_array().unwrap();
    assert!(!arr.is_empty());
    for t in arr {
        assert!(
            t["source_file"].as_str().unwrap().contains("Projects/"),
            "task in {:?} should not match Projects/",
            t["source_file"]
        );
    }
}

#[test]
fn filter_due_before() {
    let v = json_tasks(&["--due-before", "2026-05-15"]);
    let arr = v.as_array().unwrap();
    assert!(!arr.is_empty());
    for t in arr {
        let due = t["due"].as_str().expect("filter requires a due date");
        assert!(due < "2026-05-15", "due {due} should be before cutoff");
    }
}

#[test]
fn filter_due_after() {
    let v = json_tasks(&["--due-after", "2026-06-01"]);
    let arr = v.as_array().unwrap();
    assert!(!arr.is_empty());
    for t in arr {
        let due = t["due"].as_str().unwrap();
        assert!(due > "2026-06-01");
    }
}

#[test]
fn filter_has_due() {
    let v = json_tasks(&["--has-due"]);
    let arr = v.as_array().unwrap();
    for t in arr {
        assert!(!t["due"].is_null());
    }
}

#[test]
fn filter_no_due() {
    let v = json_tasks(&["--no-due"]);
    let arr = v.as_array().unwrap();
    assert!(!arr.is_empty(), "fixture has at least one undated task");
    for t in arr {
        assert!(t["due"].is_null());
    }
}

#[test]
fn gitignored_files_excluded_from_scan() {
    let v = json_tasks(&[]);
    let arr = v.as_array().unwrap();
    for t in arr {
        let path = t["source_file"].as_str().unwrap();
        assert!(
            !path.starts_with("private/"),
            "private/ is gitignored; should not be scanned: {path}"
        );
    }
}

#[test]
fn attachments_dir_excluded_from_scan() {
    let v = json_tasks(&[]);
    let arr = v.as_array().unwrap();
    for t in arr {
        let path = t["source_file"].as_str().unwrap();
        assert!(
            !path.starts_with("attachments/"),
            "attachments/ should be excluded: {path}"
        );
    }
}

#[test]
fn obsidian_dir_excluded_from_scan() {
    let v = json_tasks(&[]);
    let arr = v.as_array().unwrap();
    for t in arr {
        let path = t["source_file"].as_str().unwrap();
        assert!(!path.starts_with(".obsidian/"));
    }
}

#[test]
fn default_sort_due_asc_then_priority_desc() {
    // Due-bearing tasks come first, ordered by date asc; ties break by
    // priority desc.
    let v = json_tasks(&["--has-due"]);
    let arr = v.as_array().unwrap();
    let mut prev_due: Option<String> = None;
    for t in arr {
        let due = t["due"].as_str().unwrap().to_string();
        if let Some(p) = &prev_due {
            assert!(p.as_str() <= due.as_str(), "due not ascending: {p} > {due}");
        }
        prev_due = Some(due);
    }
}

#[test]
fn flags_compose_as_and() {
    // status=open AND tag=project/website → only open project/website tasks
    let v = json_tasks(&["--status", "open", "--tag", "project/website"]);
    let arr = v.as_array().unwrap();
    assert!(!arr.is_empty());
    for t in arr {
        assert_eq!(t["status"], "Open");
        let tags: Vec<&str> = t["tags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap())
            .collect();
        assert!(tags.contains(&"project/website"));
    }
}

#[test]
fn tasks_list_help_works() {
    Command::cargo_bin("ft")
        .unwrap()
        .args(["tasks", "list", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--status"))
        .stdout(predicate::str::contains("--priority"));
}

// ── Session 4: DSL queries ───────────────────────────────────────────────────

#[test]
fn dsl_status_predicate() {
    let v = json_tasks(&["--query", "status is open"]);
    let arr = v.as_array().unwrap();
    assert!(!arr.is_empty());
    for t in arr {
        assert_eq!(t["status"], "Open");
    }
}

#[test]
fn dsl_priority_predicate() {
    let v = json_tasks(&["--query", "priority is highest"]);
    let arr = v.as_array().unwrap();
    assert!(!arr.is_empty());
    for t in arr {
        assert_eq!(t["priority"], "Highest");
    }
}

#[test]
fn dsl_path_includes() {
    let v = json_tasks(&["--query", r#"path includes "Areas/""#]);
    let arr = v.as_array().unwrap();
    assert!(!arr.is_empty());
    for t in arr {
        assert!(t["source_file"].as_str().unwrap().contains("Areas/"));
    }
}

#[test]
fn dsl_tag_predicate() {
    let v = json_tasks(&["--query", "tag is #project/website"]);
    let arr = v.as_array().unwrap();
    assert!(!arr.is_empty());
    for t in arr {
        let tags: Vec<&str> = t["tags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap())
            .collect();
        assert!(tags.contains(&"project/website"));
    }
}

#[test]
fn dsl_due_before_today() {
    let v = json_tasks(&["--query", "due before today"]);
    let arr = v.as_array().unwrap();
    assert!(
        !arr.is_empty(),
        "fixture has overdue tasks vs FT_TODAY=2026-05-09"
    );
    for t in arr {
        let due = t["due"].as_str().unwrap();
        assert!(due < "2026-05-09");
    }
}

#[test]
fn dsl_or_combinator() {
    let v = json_tasks(&["--query", "priority is highest or priority is high"]);
    let arr = v.as_array().unwrap();
    assert!(!arr.is_empty());
    for t in arr {
        let p = t["priority"].as_str().unwrap();
        assert!(matches!(p, "Highest" | "High"), "got priority {p}");
    }
}

#[test]
fn dsl_and_combinator() {
    let v = json_tasks(&["--query", "not done and tag is #area/finance"]);
    let arr = v.as_array().unwrap();
    assert!(!arr.is_empty());
    for t in arr {
        assert_ne!(t["status"], "Done");
        let tags: Vec<&str> = t["tags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap())
            .collect();
        assert!(tags.contains(&"area/finance"));
    }
}

#[test]
fn dsl_not_combinator() {
    let v = json_tasks(&["--query", "not (priority is high)"]);
    let arr = v.as_array().unwrap();
    for t in arr {
        assert_ne!(t["priority"], "High");
    }
}

#[test]
fn dsl_parens_change_precedence() {
    // Without parens: (done and X) or done, with parens: done and (X or done)
    let q = "(done or not done) and tag is #area/health";
    let v = json_tasks(&["--query", q]);
    let arr = v.as_array().unwrap();
    assert!(!arr.is_empty());
    for t in arr {
        let tags: Vec<&str> = t["tags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap())
            .collect();
        assert!(tags.contains(&"area/health"));
    }
}

#[test]
fn dsl_limit_truncates() {
    let v = json_tasks(&["--query", "not done limit 3"]);
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 3);
}

#[test]
fn dsl_sort_by_due_reverse() {
    let v = json_tasks(&["--query", "has due sort by due reverse"]);
    let arr = v.as_array().unwrap();
    let mut prev: Option<String> = None;
    for t in arr {
        let due = t["due"].as_str().unwrap().to_string();
        if let Some(p) = &prev {
            assert!(
                p.as_str() >= due.as_str(),
                "due not descending: {p} < {due}"
            );
        }
        prev = Some(due);
    }
}

#[test]
fn dsl_invalid_syntax_is_clear_error() {
    run_list(&["--query", "foo bar baz"])
        .failure()
        .stderr(predicate::str::contains("invalid query"));
}

#[test]
fn dsl_unsupported_feature_rejected() {
    run_list(&["--query", "group by path"])
        .failure()
        .stderr(predicate::str::contains("unsupported"));
}

// ── Session 4: presets ───────────────────────────────────────────────────────

#[test]
fn preset_overdue_against_realistic() {
    // FT_TODAY=2026-05-09 → "Annual checkup" 📅 2026-05-04 is overdue.
    let v = json_tasks(&["overdue"]);
    let arr = v.as_array().unwrap();
    assert!(!arr.is_empty(), "expected at least one overdue task");
    for t in arr {
        assert_ne!(t["status"], "Done");
        let due = t["due"].as_str().expect("overdue requires due");
        assert!(due < "2026-05-09");
    }
}

#[test]
fn preset_today_against_realistic() {
    // "Review PRs" 📅 2026-05-09 should be in `today`.
    let v = json_tasks(&["today"]);
    let arr = v.as_array().unwrap();
    let descs: Vec<&str> = arr
        .iter()
        .map(|t| t["description"].as_str().unwrap())
        .collect();
    assert!(
        descs.iter().any(|d| d.contains("Review PRs")),
        "expected Review PRs in today preset; got {descs:?}"
    );
}

#[test]
fn preset_upcoming_against_realistic() {
    let v = json_tasks(&["upcoming"]);
    let arr = v.as_array().unwrap();
    assert!(!arr.is_empty());
    for t in arr {
        assert_ne!(t["status"], "Done");
        let due = t["due"].as_str().expect("upcoming requires due");
        assert!(due > "2026-05-09");
    }
}

#[test]
fn user_preset_shadows_builtin() {
    use std::fs;
    let tmp = assert_fs::TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join(".obsidian")).unwrap();
    fs::create_dir_all(tmp.path().join(".ft")).unwrap();
    fs::write(
        tmp.path().join(".ft/config.toml"),
        // Shadow the built-in `today` preset with one that matches a tag
        // present in our notes.
        r#"
[presets]
today = "tag is #marker"
"#,
    )
    .unwrap();
    fs::write(tmp.path().join("note.md"), "- [ ] one #marker\n- [ ] two\n").unwrap();

    let assert = run_list_in(tmp.path(), &["today", "--format", "json", "--no-color"]).success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 1, "user preset should shadow built-in today");
    assert_eq!(arr[0]["description"], "one #marker");
}

// ── Session 4: --sort, --group-by, formats, --allow-empty ────────────────────

#[test]
fn cli_sort_priority_then_due() {
    let v = json_tasks(&["--has-due", "--sort", "priority,due"]);
    let arr = v.as_array().unwrap();
    let prio_rank = |s: &str| -> i32 {
        match s {
            "Highest" => 0,
            "High" => 1,
            "Medium" => 2,
            "Low" => 3,
            "Lowest" => 4,
            _ => 5,
        }
    };
    let mut prev_rank = -1i32;
    for t in arr {
        let r = prio_rank(t["priority"].as_str().unwrap_or("none"));
        assert!(r >= prev_rank, "priority not ascending: {r} < {prev_rank}");
        prev_rank = r;
    }
}

#[test]
fn cli_sort_reverse_modifier() {
    let v = json_tasks(&["--has-due", "--sort", "due:reverse"]);
    let arr = v.as_array().unwrap();
    let mut prev: Option<String> = None;
    for t in arr {
        let due = t["due"].as_str().unwrap().to_string();
        if let Some(p) = &prev {
            assert!(p.as_str() >= due.as_str());
        }
        prev = Some(due);
    }
}

#[test]
fn cli_sort_overrides_dsl_sort() {
    // DSL says sort by due; CLI says sort by description. CLI wins.
    let v = json_tasks(&["--query", "not done sort by due", "--sort", "description"]);
    let arr = v.as_array().unwrap();
    let mut prev: Option<String> = None;
    for t in arr {
        let d = t["description"].as_str().unwrap().to_string();
        if let Some(p) = &prev {
            assert!(p.as_str() <= d.as_str(), "description not ascending");
        }
        prev = Some(d);
    }
}

#[test]
fn group_by_folder_table_has_section_headings() {
    run_list(&["--no-color", "--group-by", "folder"])
        .success()
        .stdout(predicate::str::contains("## Areas"))
        .stdout(predicate::str::contains("## Projects"));
}

#[test]
fn format_markdown_emits_round_trippable_lines() {
    let assert = run_list(&["--no-color", "--format", "markdown", "--status", "done"]).success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(!stdout.is_empty());
    for line in stdout.lines() {
        assert!(
            line.starts_with("- [x]") || line.starts_with("- [X]"),
            "markdown format should emit task lines starting with `- [x]`; got: {line}"
        );
    }
}

#[test]
fn format_ndjson_one_object_per_line() {
    let assert = run_list(&["--no-color", "--format", "ndjson"]).success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(!lines.is_empty());
    for line in lines {
        let v: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("each ndjson line must be valid JSON: {e} on `{line}`"));
        assert!(v.is_object());
    }
}

#[test]
fn allow_empty_flag_returns_zero_when_no_match() {
    run_list(&[
        "--no-color",
        "--query",
        "tag is #nonexistent",
        "--allow-empty",
    ])
    .success();
}

#[test]
fn empty_match_exits_one_by_default() {
    let assert = run_list(&["--no-color", "--query", "tag is #nonexistent"]);
    let output = assert.get_output();
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn flags_compose_with_query_as_and() {
    // --query says priority is high; --status open further restricts.
    let v = json_tasks(&[
        "--query",
        "priority is high or priority is highest",
        "--status",
        "open",
    ]);
    let arr = v.as_array().unwrap();
    assert!(!arr.is_empty());
    for t in arr {
        assert_eq!(t["status"], "Open");
        let p = t["priority"].as_str().unwrap();
        assert!(matches!(p, "High" | "Highest"));
    }
}

// ── Session 4: pathological fixture exercise ─────────────────────────────────

#[test]
fn pathological_scan_does_not_crash() {
    run_list_in(&pathological_vault(), &["--no-color", "--allow-empty"]).success();
}

#[test]
fn pathological_deep_subtasks_have_correct_parents() {
    let assert = run_list_in(
        &pathological_vault(),
        &["--no-color", "--format", "json", "--allow-empty"],
    )
    .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let arr = v.as_array().unwrap();
    let stake_a = arr
        .iter()
        .find(|t| t["description"] == "Stakeholder A")
        .expect("Stakeholder A present");
    assert!(
        stake_a["parent"].is_number(),
        "Stakeholder A should have a parent line, got {:?}",
        stake_a["parent"]
    );
}

#[test]
fn has_due_and_no_due_are_mutually_exclusive() {
    Command::cargo_bin("ft")
        .unwrap()
        .args([
            "--vault",
            realistic_vault().to_str().unwrap(),
            "tasks",
            "list",
            "--has-due",
            "--no-due",
        ])
        .assert()
        .failure();
}
