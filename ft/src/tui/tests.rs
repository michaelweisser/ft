use std::sync::Arc;

use anyhow::Result;
use assert_fs::TempDir;
use chrono::{DateTime, Local, TimeZone};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ft_core::recents::RecentsLog;
use ft_core::vault::Vault;
use ratatui::{backend::TestBackend, Terminal};

use crate::tui::{event::Event, tab::AppRequest, App};

fn fixed_clock() -> DateTime<Local> {
    // Sun 10 May 2026, 14:32:05 — matches the FT_TODAY used elsewhere.
    Local
        .with_ymd_and_hms(2026, 5, 10, 14, 32, 5)
        .single()
        .expect("fixed test clock must be unambiguous")
}

fn test_vault() -> (TempDir, Vault) {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("test-vault");
    std::fs::create_dir_all(vault_path.join(".obsidian")).unwrap();
    let vault = Vault::discover(Some(vault_path)).unwrap();
    (dir, vault)
}

/// Vault with a known set of tasks: two overdue (priority high/medium), three
/// upcoming (within 7 days), one outside the default-query window. Dates are
/// anchored to `fixed_clock` so the snapshot is stable.
fn populated_vault() -> (TempDir, Vault) {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("test-vault");
    std::fs::create_dir_all(vault_path.join(".obsidian")).unwrap();
    let body = "\
- [ ] Pay rent ⏫ 📅 2026-05-08
- [ ] Renew passport 🔼 📅 2026-05-09
- [ ] Reply to Sara 📅 2026-05-10
- [ ] Submit Q2 report ⏫ 📅 2026-05-12 ⏳ 2026-05-11
- [ ] Buy birthday gift 🔽 📅 2026-05-15
- [ ] Plan vacation 📅 2026-08-01
- [x] Old task 📅 2026-05-01 ✅ 2026-05-02
";
    std::fs::write(vault_path.join("tasks.md"), body).unwrap();
    let vault = Vault::discover(Some(vault_path)).unwrap();
    (dir, vault)
}

/// Snapshot helper that redacts the wall-clock part of the status bar's
/// `refreshed HH:MM:SS` cell, so snapshots don't depend on real time.
macro_rules! assert_tui_snapshot {
    ($name:literal, $value:expr) => {{
        insta::with_settings!({
            filters => vec![(r"refreshed \d\d:\d\d:\d\d", "refreshed [HH:MM:SS]")],
        }, {
            insta::assert_snapshot!($name, $value);
        });
    }};
}

fn render(app: &mut App, w: u16, h: u16) -> String {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render_to(f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    buffer_to_string(&buf)
}

fn buffer_to_string(buf: &ratatui::buffer::Buffer) -> String {
    let area = buf.area();
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            let cell = &buf[(x, y)];
            out.push_str(cell.symbol());
        }
        out.push('\n');
    }
    out
}

fn key(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
}

#[test]
fn welcome_tab_renders_at_minimum_terminal() {
    let (_dir, vault) = test_vault();
    let mut app = App::for_test(vault);
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("welcome_tab_80x24", frame);
}

#[test]
fn help_overlay_renders_over_welcome() {
    let (_dir, vault) = test_vault();
    let mut app = App::for_test(vault);
    app.enter_help();
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("help_overlay_80x24", frame);
}

#[test]
fn tasks_tab_empty_vault_renders_no_matches() -> Result<()> {
    let (_dir, vault) = test_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("tasks_tab_empty_80x24", frame);
    Ok(())
}

#[test]
fn tasks_tab_populated_renders_overdue_and_upcoming() -> Result<()> {
    let (_dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("tasks_tab_populated_80x24", frame);
    Ok(())
}

/// Vault with a long task description, used to verify the description column
/// expands when the terminal is wider than the 80x24 minimum.
fn long_description_vault() -> (TempDir, Vault) {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("test-vault");
    std::fs::create_dir_all(vault_path.join(".obsidian")).unwrap();
    let body = "\
- [ ] This is a fairly long task description that would not fit at 80 cols 📅 2026-05-12
";
    std::fs::write(vault_path.join("tasks.md"), body).unwrap();
    let vault = Vault::discover(Some(vault_path)).unwrap();
    (dir, vault)
}

#[test]
fn tasks_tab_wide_terminal_uses_available_width() -> Result<()> {
    let (_dir, vault) = long_description_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    let narrow = render(&mut app, 80, 24);
    assert!(
        narrow.contains("This is a fairl") && narrow.contains('…'),
        "narrow terminal should truncate long description: {narrow}"
    );

    let wide = render(&mut app, 160, 24);
    assert!(
        wide.contains("This is a fairly long task description that would not fit at 80 cols"),
        "wide terminal should show full description without truncation: {wide}"
    );
    Ok(())
}

#[test]
fn tasks_tab_query_edit_mode_renders() -> Result<()> {
    let (_dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('/'))?;
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("tasks_tab_editing_80x24", frame);
    Ok(())
}

#[test]
fn tasks_tab_query_parse_error_renders() -> Result<()> {
    let (_dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    // Open the editor, clear it, type garbage, apply.
    app.dispatch(key('/'))?;
    // Select all + delete: simulate by pressing End then Backspace many times.
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)))?;
    for _ in 0..200 {
        app.dispatch(Event::Key(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )))?;
    }
    for c in "totally bogus".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("tasks_tab_parse_error_80x24", frame);
    Ok(())
}

#[test]
fn arrow_keys_navigate_view_dropdown_without_panic() -> Result<()> {
    let (_dir, vault) = test_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    let down = Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let up = Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    // Single-view list — these wrap to themselves but must not panic or
    // change the active tab.
    app.dispatch(down.clone())?;
    app.dispatch(up.clone())?;
    assert_eq!(app.active_title(), "Tasks");
    Ok(())
}

#[test]
fn enter_on_dropdown_is_consumed_by_tasks_tab() -> Result<()> {
    let (_dir, vault) = test_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    let enter = Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.dispatch(enter)?;
    // Tasks tab consumed Enter — global keymap (which has no Enter binding)
    // should not have run, and the app must still be alive.
    assert!(!app.is_quit());
    assert_eq!(app.active_title(), "Tasks");
    Ok(())
}

#[test]
fn welcome_any_key_switches_to_tasks() -> Result<()> {
    let (_dir, vault) = test_vault();
    let mut app = App::for_test(vault);
    assert_eq!(app.active_index(), 0);
    app.dispatch(key('x'))?;
    assert_eq!(app.active_index(), 1);
    assert_eq!(app.active_title(), "Tasks");
    Ok(())
}

#[test]
fn welcome_digit_jumps_directly_to_target_tab() -> Result<()> {
    // `3` from the splash screen should land on Notes in one keypress,
    // not redirect to Tasks first.
    let (_dir, vault) = test_vault();
    let mut app = App::for_test(vault);
    assert_eq!(app.active_index(), 0);
    app.dispatch(key('3'))?;
    assert_eq!(app.active_index(), 2);
    assert_eq!(app.active_title(), "Notes");
    Ok(())
}

#[test]
fn welcome_q_quits_directly() -> Result<()> {
    let (_dir, vault) = test_vault();
    let mut app = App::for_test(vault);
    assert_eq!(app.active_index(), 0);
    app.dispatch(key('q'))?;
    assert!(app.is_quit());
    Ok(())
}

#[test]
fn q_quits_from_tasks_tab() -> Result<()> {
    let (_dir, vault) = test_vault();
    let mut app = App::for_test(vault);
    app.switch_to(1)?;
    app.dispatch(key('q'))?;
    assert!(app.is_quit());
    Ok(())
}

#[test]
fn ctrl_c_quits() -> Result<()> {
    let (_dir, vault) = test_vault();
    let mut app = App::for_test(vault);
    app.switch_to(1)?;
    let ev = Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    app.dispatch(ev)?;
    assert!(app.is_quit());
    Ok(())
}

#[test]
fn tab_key_cycles_tabs() -> Result<()> {
    let (_dir, vault) = test_vault();
    let mut app = App::for_test(vault);
    app.switch_to(1)?; // start on Tasks so Tab key isn't intercepted by Welcome
    let tab_ev = Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.dispatch(tab_ev.clone())?;
    assert_eq!(app.active_title(), "Notes");
    app.dispatch(tab_ev.clone())?;
    assert_eq!(app.active_title(), "Welcome");
    app.dispatch(tab_ev)?;
    assert_eq!(app.active_title(), "Tasks");
    Ok(())
}

#[test]
fn search_arrow_navigation_wraps() -> Result<()> {
    let (_dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    let down = Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let up = Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    // 5 matches in default window — going up from selection 0 should wrap.
    let initial = render(&mut app, 80, 24);
    assert!(initial.contains("▶"), "selected indicator missing");
    for _ in 0..6 {
        app.dispatch(down.clone())?;
    }
    for _ in 0..7 {
        app.dispatch(up.clone())?;
    }
    let frame = render(&mut app, 80, 24);
    assert!(frame.contains("▶"));
    Ok(())
}

#[test]
fn search_query_edit_apply_updates_list() -> Result<()> {
    let (_dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;

    // Replace query with one that only matches a single task.
    app.dispatch(key('/'))?;
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)))?;
    for _ in 0..200 {
        app.dispatch(Event::Key(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )))?;
    }
    for c in "priority is high".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;

    let frame = render(&mut app, 80, 24);
    // "Pay rent" and "Submit Q2 report" are the two High-priority tasks.
    assert!(frame.contains("Pay rent"), "expected high-pri task in list");
    assert!(
        !frame.contains("Reply to Sara"),
        "non-matching task should be filtered out: \n{frame}"
    );
    Ok(())
}

#[test]
fn search_esc_cancels_edit_without_changing_query() -> Result<()> {
    let (_dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    let before = render(&mut app, 80, 24);

    app.dispatch(key('/'))?;
    for c in "garbage".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)))?;
    let after = render(&mut app, 80, 24);
    assert_eq!(
        before.lines().nth(1).unwrap(),
        after.lines().nth(1).unwrap(),
        "query bar should revert on Esc"
    );
    Ok(())
}

#[test]
fn search_capital_r_reloads_and_picks_up_disk_changes() -> Result<()> {
    let (dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    let before = render(&mut app, 80, 24);
    assert!(before.contains("Pay rent"));

    // Mutate disk: append a new overdue task to tasks.md.
    let path = dir.path().join("test-vault").join("tasks.md");
    let mut existing = std::fs::read_to_string(&path).unwrap();
    existing.push_str("- [ ] Brand new urgent task 🔺 📅 2026-05-07\n");
    std::fs::write(&path, existing).unwrap();

    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('R'),
        KeyModifiers::SHIFT,
    )))?;
    let after = render(&mut app, 80, 24);
    assert!(
        after.contains("Brand new urgent"),
        "R should pick up disk changes:\n{after}"
    );
    Ok(())
}

/// Path to the markdown file inside `populated_vault`. Tests use this to
/// inspect on-disk state after a quick-key mutation.
fn populated_tasks_path(dir: &TempDir) -> std::path::PathBuf {
    dir.path().join("test-vault").join("tasks.md")
}

#[test]
fn quick_key_bracket_close_nudges_due_forward() -> Result<()> {
    let (dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    // Selection starts on "Pay rent" (overdue 2026-05-08).
    app.dispatch(key(']'))?;
    let body = std::fs::read_to_string(populated_tasks_path(&dir)).unwrap();
    assert!(
        body.contains("Pay rent ⏫ 📅 2026-05-09"),
        "due should bump to 2026-05-09: \n{body}"
    );
    Ok(())
}

#[test]
fn quick_key_bracket_open_nudges_due_back() -> Result<()> {
    let (dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('['))?;
    let body = std::fs::read_to_string(populated_tasks_path(&dir)).unwrap();
    assert!(
        body.contains("Pay rent ⏫ 📅 2026-05-07"),
        "due should bump back to 2026-05-07: \n{body}"
    );
    Ok(())
}

#[test]
fn quick_key_brace_close_nudges_scheduled_forward() -> Result<()> {
    let (dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    // Move down to "Submit Q2 report" which has a scheduled date.
    for _ in 0..3 {
        app.dispatch(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)))?;
    }
    app.dispatch(key('}'))?;
    let body = std::fs::read_to_string(populated_tasks_path(&dir)).unwrap();
    assert!(
        body.contains("⏳ 2026-05-12"),
        "scheduled should bump from 2026-05-11 to 2026-05-12: \n{body}"
    );
    Ok(())
}

#[test]
fn quick_key_p_cycles_priority_forward() -> Result<()> {
    let (dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    // Selection: "Pay rent" already has priority High (⏫). Cycle: high → none.
    app.dispatch(key('p'))?;
    let body = std::fs::read_to_string(populated_tasks_path(&dir)).unwrap();
    assert!(
        !body.contains("Pay rent ⏫"),
        "p should clear priority on a high-pri task: \n{body}"
    );
    assert!(
        body.contains("Pay rent 📅"),
        "Pay rent line should still exist sans priority: \n{body}"
    );
    Ok(())
}

#[test]
fn quick_key_capital_p_cycles_priority_backward() -> Result<()> {
    let (dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    // "Reply to Sara" has no priority — selection 2 (overdue 2 + first upcoming).
    for _ in 0..2 {
        app.dispatch(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('P'),
        KeyModifiers::SHIFT,
    )))?;
    let body = std::fs::read_to_string(populated_tasks_path(&dir)).unwrap();
    assert!(
        body.contains("Reply to Sara ⏫"),
        "P (reverse) on no-pri task should land on High: \n{body}"
    );
    Ok(())
}

#[test]
fn quick_key_x_completes_selected_task() -> Result<()> {
    let (dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('x'))?;
    let body = std::fs::read_to_string(populated_tasks_path(&dir)).unwrap();
    assert!(
        body.contains("- [x] Pay rent"),
        "x should mark Pay rent done: \n{body}"
    );
    assert!(
        body.contains("✅ 2026-05-10"),
        "completion date should be today: \n{body}"
    );
    let frame = render(&mut app, 80, 24);
    assert!(
        !frame.contains("Pay rent"),
        "completed task should disappear from default `not done` query: \n{frame}"
    );
    Ok(())
}

#[test]
fn quick_key_capital_x_cancels_selected_task() -> Result<()> {
    let (dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('X'),
        KeyModifiers::SHIFT,
    )))?;
    let body = std::fs::read_to_string(populated_tasks_path(&dir)).unwrap();
    assert!(
        body.contains("- [-] Pay rent"),
        "X should mark Pay rent cancelled: \n{body}"
    );
    assert!(
        body.contains("❌ 2026-05-10"),
        "cancellation date should be today: \n{body}"
    );
    Ok(())
}

#[test]
fn quick_key_t_sets_due_to_today() -> Result<()> {
    let (dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    // Selection starts on "Pay rent" (📅 2026-05-08, overdue).
    app.dispatch(key('t'))?;
    let body = std::fs::read_to_string(populated_tasks_path(&dir)).unwrap();
    assert!(
        body.contains("Pay rent ⏫ 📅 2026-05-10"),
        "t should set due to today (2026-05-10): \n{body}"
    );
    Ok(())
}

#[test]
fn edit_popup_opens_on_e_with_current_values() -> Result<()> {
    let (_dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('e'))?;
    let frame = render(&mut app, 80, 24);
    assert!(frame.contains("edit task"), "popup title missing:\n{frame}");
    assert!(
        frame.contains("Pay rent"),
        "description prefilled:\n{frame}"
    );
    assert!(frame.contains("2026-05-08"), "due prefilled:\n{frame}");
    assert!(frame.contains("high"), "priority prefilled:\n{frame}");
    Ok(())
}

#[test]
fn edit_popup_renders_at_80x24_snapshot() -> Result<()> {
    let (_dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('e'))?;
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("edit_popup_80x24", frame);
    Ok(())
}

#[test]
fn edit_popup_ctrl_s_saves_changes() -> Result<()> {
    let (dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('e'))?;
    // Tab to the due field, clear it, type "+3d" (CLI relative-date).
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)))?;
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)))?;
    for _ in 0..20 {
        app.dispatch(Event::Key(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )))?;
    }
    for c in "+3d".chars() {
        app.dispatch(key(c))?;
    }
    // Ctrl+S submit.
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('s'),
        KeyModifiers::CONTROL,
    )))?;
    let body = std::fs::read_to_string(populated_tasks_path(&dir)).unwrap();
    // 2026-05-10 + 3 days = 2026-05-13.
    assert!(
        body.contains("Pay rent ⏫ 📅 2026-05-13"),
        "+3d should resolve to 2026-05-13: \n{body}"
    );
    Ok(())
}

#[test]
fn edit_popup_esc_cancels_without_writing() -> Result<()> {
    let (dir, vault) = populated_vault();
    let before = std::fs::read_to_string(populated_tasks_path(&dir)).unwrap();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('e'))?;
    for c in "garbage".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)))?;
    let after = std::fs::read_to_string(populated_tasks_path(&dir)).unwrap();
    assert_eq!(before, after, "Esc must not touch disk");
    Ok(())
}

#[test]
fn edit_popup_invalid_date_keeps_popup_open_with_error() -> Result<()> {
    let (dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('e'))?;
    // Tab to due, clear, type garbage.
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)))?;
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)))?;
    for _ in 0..20 {
        app.dispatch(Event::Key(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )))?;
    }
    for c in "not-a-date-at-all".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('s'),
        KeyModifiers::CONTROL,
    )))?;
    let frame = render(&mut app, 80, 24);
    assert!(
        frame.contains("⚠"),
        "error indicator should appear:\n{frame}"
    );
    assert!(
        frame.contains("due:"),
        "error should call out the offending field:\n{frame}"
    );
    let body = std::fs::read_to_string(populated_tasks_path(&dir)).unwrap();
    assert!(
        body.contains("📅 2026-05-08"),
        "disk unchanged on parse error"
    );
    Ok(())
}

#[test]
fn enter_on_search_view_queues_editor_open_request() -> Result<()> {
    let (dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    let req = app
        .take_pending_request()
        .expect("Enter should queue an editor-open request");
    match req {
        AppRequest::OpenInEditor { path, line } => {
            // Both paths get canonicalized to compare reliably across
            // macOS' /var → /private/var symlink.
            let expected = dir
                .path()
                .join("test-vault/tasks.md")
                .canonicalize()
                .unwrap();
            let actual = path.canonicalize().unwrap();
            assert_eq!(actual, expected);
            assert_eq!(line, 1, "first selection should be at line 1");
        }
        other => panic!("expected OpenInEditor, got {other:?}"),
    }
    Ok(())
}

#[test]
fn quick_keys_recurring_complete_inserts_next_instance() -> Result<()> {
    // Spin up a fresh vault with a recurring task so we can exercise the
    // ft-core recurrence path without polluting populated_vault.
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("test-vault");
    std::fs::create_dir_all(vault_path.join(".obsidian")).unwrap();
    let body = "- [ ] Water plants 🔁 every week 📅 2026-05-09\n";
    std::fs::write(vault_path.join("tasks.md"), body).unwrap();
    let vault = Vault::discover(Some(vault_path)).unwrap();

    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('x'))?;
    let body = std::fs::read_to_string(dir.path().join("test-vault/tasks.md")).unwrap();
    assert!(
        body.contains("- [ ] Water plants 🔁 every week 📅 2026-05-16"),
        "next instance should be inserted with due = 2026-05-09 + 7d: \n{body}"
    );
    assert!(
        body.contains("- [x] Water plants"),
        "completed instance should remain: \n{body}"
    );
    Ok(())
}

#[test]
fn question_mark_toggles_help_overlay() -> Result<()> {
    let (_dir, vault) = test_vault();
    let mut app = App::for_test(vault);
    app.switch_to(1)?;
    app.dispatch(key('?'))?;
    let frame_with_help = render(&mut app, 80, 24);
    assert!(frame_with_help.contains("Keybindings"));
    app.dispatch(key('?'))?;
    let frame_after = render(&mut app, 80, 24);
    assert!(!frame_after.contains("Keybindings"));
    Ok(())
}

// --- session 6: snapshots ----------------------------------------------------

#[test]
fn help_overlay_over_tasks_tab_renders() -> Result<()> {
    let (_dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.enter_help();
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("help_overlay_over_tasks_80x24", frame);
    Ok(())
}

#[test]
fn edit_popup_error_state_renders() -> Result<()> {
    let (_dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('e'))?;
    // Tab to due, clear, type garbage, submit — popup stays open with ⚠.
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)))?;
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)))?;
    for _ in 0..20 {
        app.dispatch(Event::Key(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )))?;
    }
    for c in "not-a-date".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('s'),
        KeyModifiers::CONTROL,
    )))?;
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("edit_popup_error_80x24", frame);
    Ok(())
}

#[test]
fn tasks_tab_wide_terminal_snapshot() -> Result<()> {
    let (_dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    let frame = render(&mut app, 120, 30);
    assert_tui_snapshot!("tasks_tab_populated_120x30", frame);
    Ok(())
}

// --- session 6: help-overlay audit ------------------------------------------

/// The set of keystroke labels that MUST appear in the help overlay. Updating
/// the help table requires updating this list so the doc never drifts from
/// what the code actually binds.
const EXPECTED_HELP_LABELS: &[&str] = &[
    "q / Ctrl+C",
    "?",
    "Tab / Shift+Tab",
    "1 / 2",
    "/",
    "↑ / ↓ · j / k",
    "] / [",
    "} / {",
    "t",
    "p / P",
    "x / X",
    "e",
    "c / Shift+C",
    "Ctrl+E",
    "Enter (target)",
    "Enter",
    "R",
    "Ctrl+W / Ctrl+⌫",
    "Esc",
];

#[test]
fn help_overlay_documents_every_canonical_binding() -> Result<()> {
    let (_dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.enter_help();
    let frame = render(&mut app, 80, 40); // tall enough to render every row
    for label in EXPECTED_HELP_LABELS {
        assert!(
            frame.contains(label),
            "help overlay is missing key binding `{label}`:\n{frame}"
        );
    }
    Ok(())
}

// --- session 6: real-vault smoke check ---------------------------------------

/// Gated smoke test: render the Tasks tab against the user's real vault.
/// Activates only with `FT_REAL_VAULT_TESTS=1` so CI never depends on a local
/// path. Mirrors the gating already used by `tests/real_vault_cli.rs`.
#[test]
fn real_vault_tasks_tab_renders_without_panic() -> Result<()> {
    if std::env::var("FT_REAL_VAULT_TESTS").as_deref() != Ok("1") {
        return Ok(());
    }
    let path = std::path::PathBuf::from("/Users/cmw/git/fortytwo");
    if !path.exists() {
        return Ok(()); // gracefully skip if the real vault isn't on this host
    }
    let vault = Vault::discover(Some(path))?;
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    // First render runs vault.scan() + filter + sort under the hood.
    let frame = render(&mut app, 120, 40);
    assert!(
        frame.contains("Tasks") || frame.contains("tasks"),
        "real-vault first render should still render the Tasks chrome:\n{frame}"
    );
    Ok(())
}

// --- session 6: performance budgets on a 5k-note vault -----------------------

/// Build a synthetic 5k-note vault for the perf budget tests. Each note has
/// one task with a varying due date and priority so the default query is
/// non-trivial. ~5000 files written to a tempdir — setup is slow but only
/// runs when `FT_PERF_TESTS=1` is set.
fn synthetic_5k_vault(today: chrono::NaiveDate) -> (TempDir, Vault) {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("test-vault");
    std::fs::create_dir_all(vault_path.join(".obsidian")).unwrap();

    for i in 0..5000u32 {
        // Spread dates across a 60-day window, half before today and half after.
        let offset = (i as i64 % 60) - 30;
        let due = today + chrono::Duration::days(offset);
        let priority = match i % 4 {
            0 => "⏫ ",
            1 => "🔼 ",
            2 => "🔽 ",
            _ => "",
        };
        let body = format!(
            "# Note {i}\n\n- [ ] Synthetic task {i} {priority}📅 {}\n",
            due.format("%Y-%m-%d")
        );
        std::fs::write(vault_path.join(format!("note_{i:05}.md")), body).unwrap();
    }
    let vault = Vault::discover(Some(vault_path)).unwrap();
    (dir, vault)
}

fn perf_tests_enabled() -> bool {
    std::env::var("FT_PERF_TESTS").as_deref() == Ok("1")
}

#[test]
fn perf_first_render_5k_vault_under_budget() -> Result<()> {
    if !perf_tests_enabled() {
        return Ok(());
    }
    let today = chrono::NaiveDate::from_ymd_opt(2026, 5, 10).unwrap();
    let (_dir, vault) = synthetic_5k_vault(today);

    let start = std::time::Instant::now();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    let _ = render(&mut app, 80, 24);
    let elapsed = start.elapsed();

    // Plan budget is 500ms; allow 4x for debug builds & noisy CI. Run with
    // --release for tight timing: `cargo test --release ... perf_first_render`.
    let budget_ms: u128 = 2000;
    assert!(
        elapsed.as_millis() < budget_ms,
        "first render took {:?}; budget {budget_ms}ms (4x of 500ms target). \
         Run --release for tight timing.",
        elapsed
    );
    Ok(())
}

// --- session 6 follow-ups: status indicator in rows ------------------------

/// Vault with one task per status, used to exercise the status-glyph column
/// when the query is broad enough to include done / cancelled / in-progress.
fn mixed_status_vault() -> (TempDir, Vault) {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("test-vault");
    std::fs::create_dir_all(vault_path.join(".obsidian")).unwrap();
    let body = "\
- [ ] Open task 📅 2026-05-15
- [x] Done task 📅 2026-05-15 ✅ 2026-05-09
- [-] Cancelled task 📅 2026-05-15 ❌ 2026-05-09
- [/] In-progress task 📅 2026-05-15
";
    std::fs::write(vault_path.join("tasks.md"), body).unwrap();
    let vault = Vault::discover(Some(vault_path)).unwrap();
    (dir, vault)
}

#[test]
fn search_view_renders_status_glyphs_when_query_includes_all_statuses() -> Result<()> {
    let (_dir, vault) = mixed_status_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    // Default query is `not done` — replace with a no-op filter so every
    // task shows up regardless of status.
    app.dispatch(key('/'))?;
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)))?;
    for _ in 0..200 {
        app.dispatch(Event::Key(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )))?;
    }
    // Empty query matches everything; the parser treats this as no filter.
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;

    let frame = render(&mut app, 100, 24);
    assert!(frame.contains("Open task"), "open row missing:\n{frame}");
    assert!(frame.contains("Done task"), "done row missing:\n{frame}");
    assert!(
        frame.contains("Cancelled task"),
        "cancelled row missing:\n{frame}"
    );
    assert!(
        frame.contains("In-progress task"),
        "in-progress row missing:\n{frame}"
    );

    // Each non-open status renders a unique glyph in the new column.
    let done_line = frame
        .lines()
        .find(|l| l.contains("Done task"))
        .expect("done line missing");
    assert!(
        done_line.contains('✓'),
        "done row should display ✓:\n{done_line}"
    );
    let cancelled_line = frame
        .lines()
        .find(|l| l.contains("Cancelled task"))
        .expect("cancelled line missing");
    assert!(
        cancelled_line.contains('✗'),
        "cancelled row should display ✗:\n{cancelled_line}"
    );
    let inprogress_line = frame
        .lines()
        .find(|l| l.contains("In-progress task"))
        .expect("in-progress line missing");
    assert!(
        inprogress_line.contains('▷'),
        "in-progress row should display ▷:\n{inprogress_line}"
    );
    Ok(())
}

// --- session 6 follow-ups: ctrl+backspace word-delete in edit fields --------

#[test]
fn ctrl_backspace_deletes_word_in_query_bar() -> Result<()> {
    let (_dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;

    // Open the query bar and replace its contents with a known string.
    app.dispatch(key('/'))?;
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)))?;
    for _ in 0..200 {
        app.dispatch(Event::Key(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )))?;
    }
    for c in "alpha beta gamma".chars() {
        app.dispatch(key(c))?;
    }

    // Ctrl+Backspace should remove "gamma" (the trailing word).
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Backspace,
        KeyModifiers::CONTROL,
    )))?;
    let frame = render(&mut app, 80, 24);
    assert!(
        frame.contains("alpha beta "),
        "after Ctrl+Backspace the trailing word should be gone:\n{frame}"
    );
    assert!(
        !frame.contains("gamma"),
        "gamma should be deleted:\n{frame}"
    );
    Ok(())
}

#[test]
fn ctrl_w_deletes_word_in_query_bar() -> Result<()> {
    let (_dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('/'))?;
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)))?;
    for _ in 0..200 {
        app.dispatch(Event::Key(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )))?;
    }
    for c in "foo bar".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('w'),
        KeyModifiers::CONTROL,
    )))?;
    let frame = render(&mut app, 80, 24);
    // The query bar is the line immediately after the top tab bar.
    let query_line = frame
        .lines()
        .find(|l| l.contains("foo"))
        .expect("query bar should still contain `foo`");
    assert!(
        !query_line.contains("bar"),
        "bar should be deleted from query bar:\n{query_line}"
    );
    Ok(())
}

#[test]
fn ctrl_backspace_deletes_word_in_edit_popup_field() -> Result<()> {
    let (_dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('e'))?;
    // Focus starts on description, which holds "Pay rent". Ctrl+Backspace
    // should erase "rent" but leave "Pay ".
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)))?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Backspace,
        KeyModifiers::CONTROL,
    )))?;
    let frame = render(&mut app, 80, 24);
    assert!(
        frame.contains("Pay "),
        "Pay should remain in the description field:\n{frame}"
    );
    // The word "rent" only appears in the description column of the
    // background task list (which is still visible to the left/right of
    // the popup). Make sure it's no longer inside the popup's
    // description value cell — check that the line right of "description :"
    // doesn't contain "rent".
    let popup_line = frame
        .lines()
        .find(|l| l.contains("description :"))
        .expect("popup description row missing");
    assert!(
        !popup_line.contains("rent"),
        "rent should be deleted from the description field:\n{popup_line}"
    );
    Ok(())
}

// --- session 6 follow-ups: tag round-trip + editor-exit drain ---------------

#[test]
fn edit_popup_saves_tag_changes_back_to_description() -> Result<()> {
    // Tags are derived from the description on parse, so the popup has to
    // rewrite the description to persist tag edits. Regression for: "tag
    // changes don't get saved" after using `e`.
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("test-vault");
    std::fs::create_dir_all(vault_path.join(".obsidian")).unwrap();
    std::fs::write(
        vault_path.join("tasks.md"),
        "- [ ] Pay rent #old 📅 2026-05-08\n",
    )
    .unwrap();
    let vault = Vault::discover(Some(vault_path)).unwrap();

    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('e'))?;
    // Jump straight to the tags field (description, due, scheduled, priority, tags).
    for _ in 0..4 {
        app.dispatch(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)))?;
    for _ in 0..20 {
        app.dispatch(Event::Key(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )))?;
    }
    for c in "work urgent".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('s'),
        KeyModifiers::CONTROL,
    )))?;

    let body = std::fs::read_to_string(dir.path().join("test-vault/tasks.md")).unwrap();
    assert!(
        body.contains("#work"),
        "new tag should be embedded in description: \n{body}"
    );
    assert!(
        body.contains("#urgent"),
        "new tag should be embedded in description: \n{body}"
    );
    assert!(
        !body.contains("#old"),
        "tag removed from popup tags field should be stripped: \n{body}"
    );
    Ok(())
}

#[test]
fn edit_popup_emptying_tags_field_removes_all_tags() -> Result<()> {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("test-vault");
    std::fs::create_dir_all(vault_path.join(".obsidian")).unwrap();
    std::fs::write(
        vault_path.join("tasks.md"),
        "- [ ] Pay rent #work #urgent 📅 2026-05-08\n",
    )
    .unwrap();
    let vault = Vault::discover(Some(vault_path)).unwrap();

    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('e'))?;
    for _ in 0..4 {
        app.dispatch(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)))?;
    for _ in 0..40 {
        app.dispatch(Event::Key(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('s'),
        KeyModifiers::CONTROL,
    )))?;

    let body = std::fs::read_to_string(dir.path().join("test-vault/tasks.md")).unwrap();
    assert!(
        !body.contains('#'),
        "clearing the tags field should strip inline tags: \n{body}"
    );
    assert!(
        body.contains("Pay rent"),
        "description text must survive:\n{body}"
    );
    Ok(())
}

#[test]
fn event_stream_drain_consumes_pending_events() {
    use crate::tui::event::EventStream;
    use std::time::Duration;

    // Standing up a real EventStream relies on a TTY for the crossterm
    // poll loop. In a non-tty test environment poll fails fast and the
    // background thread exits — drain still has to behave (no events to
    // consume, returns within the window without spinning).
    let stream = EventStream::new(Duration::from_secs(60)); // long tick so no ticks queue
    let start = std::time::Instant::now();
    stream.drain(Duration::from_millis(80));
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(60),
        "drain should consume the full window: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(400),
        "drain should not block past the window: {elapsed:?}"
    );
}

// --- session 6: perf budgets (re-anchor so file order is stable) ------------

#[test]
fn perf_keystrokes_5k_vault_under_budget() -> Result<()> {
    if !perf_tests_enabled() {
        return Ok(());
    }
    let today = chrono::NaiveDate::from_ymd_opt(2026, 5, 10).unwrap();
    let (_dir, vault) = synthetic_5k_vault(today);
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    let _ = render(&mut app, 80, 24);

    // Dispatch 100 down-arrows + redraw each time. In-memory navigation
    // should never re-scan; the cost is purely filter+layout.
    let down = Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let iterations = 100u32;
    let start = std::time::Instant::now();
    for _ in 0..iterations {
        app.dispatch(down.clone())?;
        let _ = render(&mut app, 80, 24);
    }
    let elapsed = start.elapsed();
    let per_key_ms = elapsed.as_millis() / u128::from(iterations);

    // Plan budget is 50ms per keystroke; allow 2x for debug. Release builds
    // typically come in well under 10ms.
    let budget_ms: u128 = 100;
    assert!(
        per_key_ms < budget_ms,
        "per-keystroke {per_key_ms}ms exceeded budget {budget_ms}ms \
         (target 50ms). Total: {:?} across {iterations} iters.",
        elapsed
    );
    Ok(())
}

// --- plan 004 session 2: quickline (new task) ------------------------------

/// Vault preconfigured to drop a daily note at `<root>/Daily/2026-05-10.md`
/// using the explicit `[daily_notes]` source, so quickline writes without
/// `in:` land somewhere predictable for assertions.
fn quickline_vault() -> (TempDir, Vault) {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("test-vault");
    std::fs::create_dir_all(vault_path.join(".obsidian")).unwrap();
    std::fs::create_dir_all(vault_path.join(".ft")).unwrap();
    // NB: in moment.js syntax (which the explicit resolver uses) bare
    // letters like `D` are tokens, so `Daily` would translate to
    // `10aily`. Wrap literal folder names in `[…]` to opt out.
    std::fs::write(
        vault_path.join(".ft/config.toml"),
        "[daily_notes]\nsource = \"explicit\"\npath = \"[Daily]\"\nformat = \"YYYY-MM-DD\"\n",
    )
    .unwrap();
    let vault = Vault::discover(Some(vault_path)).unwrap();
    (dir, vault)
}

#[test]
fn quickline_opens_with_c_and_closes_on_esc() -> Result<()> {
    let (_dir, vault) = quickline_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('c'))?;
    let frame = render(&mut app, 100, 24);
    assert!(
        frame.contains("new task"),
        "panel title missing after `c`:\n{frame}"
    );
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)))?;
    let after = render(&mut app, 100, 24);
    assert!(
        !after.contains("new task"),
        "panel should close on Esc:\n{after}"
    );
    Ok(())
}

#[test]
fn quickline_enter_writes_to_daily_note() -> Result<()> {
    let (dir, vault) = quickline_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('c'))?;
    for c in "buy milk due:tomorrow pri:high #grocery".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    let daily = dir.path().join("test-vault/Daily/2026-05-10.md");
    let body = std::fs::read_to_string(&daily)
        .unwrap_or_else(|e| panic!("daily note missing: {}: {e}", daily.display()));
    assert!(body.contains("buy milk"), "description missing:\n{body}");
    assert!(body.contains("⏫"), "high priority emoji missing:\n{body}");
    assert!(body.contains("📅 2026-05-11"), "due date missing:\n{body}");
    assert!(body.contains("#grocery"), "tag missing:\n{body}");
    // Panel should close on success.
    let frame = render(&mut app, 100, 24);
    assert!(
        !frame.contains("new task"),
        "panel should close after a successful write:\n{frame}"
    );
    Ok(())
}

#[test]
fn quickline_in_path_overrides_target() -> Result<()> {
    let (dir, vault) = quickline_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('c'))?;
    for c in "remember to call dentist in:Inbox.md".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    let inbox = dir.path().join("test-vault/Inbox.md");
    let body = std::fs::read_to_string(&inbox).unwrap();
    assert!(body.contains("call dentist"));
    // Daily note shouldn't have been touched.
    let daily = dir.path().join("test-vault/Daily/2026-05-10.md");
    assert!(
        !daily.exists()
            || !std::fs::read_to_string(&daily)
                .unwrap()
                .contains("call dentist"),
        "daily note shouldn't have the in:-overridden task"
    );
    Ok(())
}

#[test]
fn quickline_parse_error_blocks_write() -> Result<()> {
    let (dir, vault) = quickline_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('c'))?;
    for c in "draft due:not-a-date".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    let frame = render(&mut app, 100, 24);
    assert!(
        frame.contains("new task"),
        "panel should stay open on parse error:\n{frame}"
    );
    assert!(frame.contains("⚠"), "error indicator missing:\n{frame}");
    // Nothing landed on disk.
    let daily = dir.path().join("test-vault/Daily/2026-05-10.md");
    assert!(
        !daily.exists() || std::fs::read_to_string(&daily).unwrap().trim().is_empty(),
        "daily note should be empty when parse fails"
    );
    Ok(())
}

#[test]
fn quickline_duplicate_detection_surfaces_inline() -> Result<()> {
    let (dir, vault) = quickline_vault();
    // Pre-seed an identical task so the second create hits the duplicate
    // detector inside ops::create_task.
    let inbox = dir.path().join("test-vault/Inbox.md");
    std::fs::write(&inbox, "- [ ] follow up with team 📅 2026-05-11\n").unwrap();

    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('c'))?;
    for c in "follow up with team due:tomorrow in:Inbox.md".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;

    let frame = render(&mut app, 100, 24);
    assert!(
        frame.contains("duplicate"),
        "duplicate error should surface inline:\n{frame}"
    );
    assert!(
        frame.contains("new task"),
        "panel should stay open on duplicate:\n{frame}"
    );
    // Inbox unchanged (still only the pre-seeded line).
    let body = std::fs::read_to_string(&inbox).unwrap();
    assert_eq!(body.lines().filter(|l| l.contains("follow up")).count(), 1);
    Ok(())
}

#[test]
fn quickline_empty_description_blocks_write() -> Result<()> {
    let (dir, vault) = quickline_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('c'))?;
    // Only a tag — no description text.
    for c in "due:tomorrow".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    let frame = render(&mut app, 100, 24);
    assert!(frame.contains("new task"), "panel stays open: \n{frame}");
    assert!(
        frame.contains("description is empty"),
        "error missing: \n{frame}"
    );
    let daily = dir.path().join("test-vault/Daily/2026-05-10.md");
    assert!(!daily.exists() || std::fs::read_to_string(daily).unwrap().trim().is_empty());
    Ok(())
}

#[test]
fn quickline_success_raises_green_toast_request() -> Result<()> {
    let (_dir, vault) = quickline_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('c'))?;
    for c in "buy milk due:tomorrow".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    // Service the queued AppRequest::Toast so the App's toast slot
    // becomes populated (the run-loop does this between iterations).
    app.service_pending_for_test()?;
    let toast = app
        .current_toast()
        .expect("a toast should be active after a successful create");
    assert!(
        toast.text.starts_with("created "),
        "toast text: {}",
        toast.text
    );
    assert_eq!(toast.style, crate::tui::tab::ToastStyle::Success);
    Ok(())
}

#[test]
fn quickline_success_renders_toast_in_status_bar_center_cell() -> Result<()> {
    let (_dir, vault) = quickline_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('c'))?;
    for c in "draft report due:tomorrow".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    app.service_pending_for_test()?;
    let frame = render(&mut app, 120, 24);
    let status = frame.lines().last().expect("status bar row");
    assert!(
        status.contains("created"),
        "status bar should show the toast: {status}"
    );
    Ok(())
}

#[test]
fn quickline_success_anchors_cursor_to_new_task_when_it_matches_filter() -> Result<()> {
    // New task is due tomorrow → passes the default `not done and due
    // before today+8d` filter, so it should appear in the list AND the
    // cursor should land on its row.
    let (_dir, vault) = quickline_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('c'))?;
    for c in "anchor target due:tomorrow".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    let frame = render(&mut app, 120, 24);
    // Find the row with the new task and assert it carries the `▶` cursor.
    let row = frame
        .lines()
        .find(|l| l.contains("anchor target"))
        .expect("new task missing from list");
    assert!(
        row.contains('▶'),
        "cursor should anchor to the new task row: {row}"
    );
    Ok(())
}

#[test]
fn quickline_duplicate_does_not_raise_toast() -> Result<()> {
    // Duplicate detection stays inline (the user can edit and retry),
    // so it must NOT also fire a toast — that'd be redundant noise.
    let (dir, vault) = quickline_vault();
    let inbox = dir.path().join("test-vault/Inbox.md");
    std::fs::write(&inbox, "- [ ] dup task 📅 2026-05-11\n").unwrap();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('c'))?;
    for c in "dup task due:tomorrow in:Inbox.md".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    app.service_pending_for_test()?;
    assert!(
        app.current_toast().is_none(),
        "duplicate should stay inline, not fire a toast"
    );
    Ok(())
}

// --- session 4: expanded popup (Shift+C / Ctrl+E) -------------------------

#[test]
fn shift_c_opens_blank_new_task_popup() -> Result<()> {
    let (_dir, vault) = quickline_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('C'),
        KeyModifiers::SHIFT,
    )))?;
    let frame = render(&mut app, 100, 24);
    assert!(
        frame.contains("new task"),
        "popup title should be `new task`:\n{frame}"
    );
    // Target field is part of the New-mode form.
    assert!(
        frame.contains("target"),
        "target field should be in the New popup:\n{frame}"
    );
    Ok(())
}

#[test]
fn ctrl_e_in_quickline_opens_popup_with_pre_populated_fields() -> Result<()> {
    let (_dir, vault) = quickline_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('c'))?;
    for c in "review report due:tomorrow pri:high #work in:Inbox.md".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('e'),
        KeyModifiers::CONTROL,
    )))?;
    let frame = render(&mut app, 100, 24);
    assert!(frame.contains("new task"), "popup not open:\n{frame}");
    assert!(
        frame.contains("review report"),
        "description missing:\n{frame}"
    );
    assert!(frame.contains("2026-05-11"), "due missing:\n{frame}");
    assert!(frame.contains("high"), "priority missing:\n{frame}");
    assert!(frame.contains("Inbox.md"), "target missing:\n{frame}");
    assert!(frame.contains("work"), "tags missing:\n{frame}");
    Ok(())
}

#[test]
fn new_popup_ctrl_s_writes_to_in_target() -> Result<()> {
    let (dir, vault) = quickline_vault();
    // Inbox.md needs to exist so the picker can find it on the first
    // keystroke — the target field is now pick-driven, not type-literal.
    let inbox = dir.path().join("test-vault/Inbox.md");
    std::fs::write(&inbox, "# Inbox\n").unwrap();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('C'),
        KeyModifiers::SHIFT,
    )))?;
    for c in "kickoff sync".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)))?;
    // Type into target — first char opens the picker, subsequent chars
    // feed it. Enter selects the highlighted hit and fills the field as
    // `Inbox.md`.
    for c in "Inbox".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)))?;
    for c in "tomorrow".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('s'),
        KeyModifiers::CONTROL,
    )))?;

    let body = std::fs::read_to_string(&inbox)
        .unwrap_or_else(|e| panic!("inbox missing: {}: {e}", inbox.display()));
    assert!(
        body.contains("kickoff sync"),
        "description missing:\n{body}"
    );
    assert!(body.contains("📅 2026-05-11"), "due missing:\n{body}");
    Ok(())
}

#[test]
fn new_popup_target_with_heading_uses_under_heading_position() -> Result<()> {
    let (dir, vault) = quickline_vault();
    let inbox = dir.path().join("test-vault/Inbox.md");
    std::fs::write(
        &inbox,
        "# Inbox\n\n## Triage\n- [ ] existing 📅 2026-05-12\n\n## Done\n",
    )
    .unwrap();

    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('C'),
        KeyModifiers::SHIFT,
    )))?;
    for c in "new triage item".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)))?;
    // Typing the file+heading query opens the picker on the first char
    // and feeds the rest. Picker matches the `## Triage` heading inside
    // Inbox.md. Enter selects it; the target field gets filled with
    // `Inbox.md#Triage`.
    for c in "Inbox.md#Triage".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('s'),
        KeyModifiers::CONTROL,
    )))?;

    let body = std::fs::read_to_string(&inbox).unwrap();
    let triage_line = body
        .lines()
        .position(|l| l.contains("## Triage"))
        .expect("Triage section missing");
    let done_line = body
        .lines()
        .position(|l| l.contains("## Done"))
        .expect("Done section missing");
    let new_line = body
        .lines()
        .position(|l| l.contains("new triage item"))
        .expect("new task missing");
    assert!(
        triage_line < new_line && new_line < done_line,
        "new task should be under Triage, before Done:\n{body}"
    );
    Ok(())
}

#[test]
fn new_popup_empty_description_blocks_write() -> Result<()> {
    let (dir, vault) = quickline_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('C'),
        KeyModifiers::SHIFT,
    )))?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('s'),
        KeyModifiers::CONTROL,
    )))?;
    let frame = render(&mut app, 100, 24);
    assert!(frame.contains("new task"), "popup stays open:\n{frame}");
    assert!(
        frame.contains("description is empty"),
        "error missing:\n{frame}"
    );
    let daily = dir.path().join("test-vault/Daily/2026-05-10.md");
    assert!(!daily.exists() || std::fs::read_to_string(&daily).unwrap().trim().is_empty());
    Ok(())
}

#[test]
fn edit_popup_still_works_after_refactor() -> Result<()> {
    // Regression check: refactoring EditPopup to support both modes
    // mustn't break the existing `e`-on-selected-task edit flow.
    let (dir, vault) = populated_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('e'))?;
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)))?;
    for c in " (updated)".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('s'),
        KeyModifiers::CONTROL,
    )))?;
    let body = std::fs::read_to_string(dir.path().join("test-vault/tasks.md")).unwrap();
    assert!(
        body.contains("Pay rent (updated)"),
        "edit should still write:\n{body}"
    );
    Ok(())
}

#[test]
fn new_popup_snapshot_80x24() -> Result<()> {
    let (_dir, vault) = quickline_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('C'),
        KeyModifiers::SHIFT,
    )))?;
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("new_popup_blank_80x24", frame);
    Ok(())
}

#[test]
fn quickline_empty_snapshot_80x24() -> Result<()> {
    let (_dir, vault) = quickline_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('c'))?;
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("quickline_empty_80x24", frame);
    Ok(())
}

#[test]
fn quickline_valid_preview_snapshot_80x24() -> Result<()> {
    let (_dir, vault) = quickline_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('c'))?;
    for c in "buy milk due:tomorrow pri:high #grocery".chars() {
        app.dispatch(key(c))?;
    }
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("quickline_valid_preview_80x24", frame);
    Ok(())
}

#[test]
fn quickline_parse_error_snapshot_80x24() -> Result<()> {
    let (_dir, vault) = quickline_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('c'))?;
    for c in "draft due:not-a-date".chars() {
        app.dispatch(key(c))?;
    }
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("quickline_parse_error_80x24", frame);
    Ok(())
}

#[test]
fn new_popup_prefilled_snapshot_80x24() -> Result<()> {
    let (_dir, vault) = quickline_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('c'))?;
    for c in "review report due:tomorrow pri:high #work".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('e'),
        KeyModifiers::CONTROL,
    )))?;
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("new_popup_prefilled_80x24", frame);
    Ok(())
}

#[test]
fn quickline_toast_success_snapshot_80x24() -> Result<()> {
    let (_dir, vault) = quickline_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('c'))?;
    for c in "ship feature due:tomorrow".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    app.service_pending_for_test()?;
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("quickline_toast_success_80x24", frame);
    Ok(())
}

#[test]
fn quickline_ctrl_w_works_in_input() -> Result<()> {
    let (_dir, vault) = quickline_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(key('c'))?;
    for c in "foo bar".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('w'),
        KeyModifiers::CONTROL,
    )))?;
    let frame = render(&mut app, 100, 24);
    // Pick the input row from the new-task panel specifically; the rest
    // of the frame contains "sidebar"/"Inbox.md" etc. that would yield
    // false positives for the "bar" substring check.
    let input_row = frame
        .lines()
        .find(|l| l.contains("foo"))
        .expect("input row with `foo` missing");
    assert!(
        !input_row.contains("bar"),
        "bar deleted from input row: {input_row}"
    );
    Ok(())
}

// --- target-field fuzzy picker (plan 006) -------------------------------

/// Test fixture: a vault with a couple of pickable files so the target
/// picker has something to match. Mirrors `quickline_vault` but adds two
/// known notes — `Areas/General Considerations.md` (with a `## Triage`
/// heading) and `Inbox.md` — so we don't have to repeat the boilerplate
/// in every picker test.
fn target_picker_vault() -> (TempDir, Vault) {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("test-vault");
    std::fs::create_dir_all(vault_path.join(".obsidian")).unwrap();
    std::fs::create_dir_all(vault_path.join(".ft")).unwrap();
    std::fs::create_dir_all(vault_path.join("Areas")).unwrap();
    std::fs::write(
        vault_path.join(".ft/config.toml"),
        "[daily_notes]\nsource = \"explicit\"\npath = \"[Daily]\"\nformat = \"YYYY-MM-DD\"\n",
    )
    .unwrap();
    std::fs::write(vault_path.join("Inbox.md"), "# Inbox\n").unwrap();
    std::fs::write(
        vault_path.join("Areas/General Considerations.md"),
        "# Intro\n\n## Triage\n",
    )
    .unwrap();
    let vault = Vault::discover(Some(vault_path)).unwrap();
    (dir, vault)
}

/// Open the new-task popup with target focused and the picker ready
/// to be triggered. Shared setup for the picker tests below.
fn open_new_popup_on_target(app: &mut App) -> Result<()> {
    app.switch_to(1)?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('C'),
        KeyModifiers::SHIFT,
    )))?;
    // Description → Target (single Tab).
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)))?;
    Ok(())
}

#[test]
fn target_picker_opens_on_enter_with_field_text_as_seed() -> Result<()> {
    let (_dir, vault) = target_picker_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    open_new_popup_on_target(&mut app)?;
    // Press Enter on the empty target field — picker opens with empty
    // input, so the header is visible but no rows.
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    let frame = render(&mut app, 100, 30);
    assert!(
        frame.contains("pick target"),
        "picker title missing:\n{frame}"
    );
    Ok(())
}

#[test]
fn target_picker_opens_on_first_keystroke_and_seeds_input() -> Result<()> {
    let (_dir, vault) = target_picker_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    open_new_popup_on_target(&mut app)?;
    // `g` opens the picker with `g` already in the input, narrowing
    // the result list to `General Considerations.md`.
    app.dispatch(key('g'))?;
    // Wider terminal so the path doesn't get truncated inside the
    // popup-in-popup column.
    let frame = render(&mut app, 140, 30);
    assert!(frame.contains("pick target"), "picker not open:\n{frame}");
    assert!(
        frame.contains("General Considerations"),
        "expected file match after seeding `g`:\n{frame}"
    );
    Ok(())
}

#[test]
fn target_picker_enter_fills_field_with_path_only() -> Result<()> {
    let (_dir, vault) = target_picker_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    open_new_popup_on_target(&mut app)?;
    for c in "Inbox".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    let frame = render(&mut app, 100, 30);
    assert!(
        !frame.contains("pick target"),
        "picker should close after select:\n{frame}"
    );
    assert!(
        frame.contains("Inbox.md"),
        "target field should hold `Inbox.md`:\n{frame}"
    );
    Ok(())
}

#[test]
fn target_picker_enter_fills_field_with_path_and_heading() -> Result<()> {
    let (_dir, vault) = target_picker_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    open_new_popup_on_target(&mut app)?;
    // Heading-query syntax mirrors the literal text the field
    // would have accepted before plan 006 — the round-trip stays
    // symmetric.
    for c in "gen consid#Tri".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    let frame = render(&mut app, 120, 30);
    assert!(
        !frame.contains("pick target"),
        "picker should close after select:\n{frame}"
    );
    assert!(
        frame.contains("General Considerations.md#Triage"),
        "target field should hold path#heading:\n{frame}"
    );
    Ok(())
}

#[test]
fn target_picker_navigation_changes_selection() -> Result<()> {
    let (_dir, vault) = target_picker_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    open_new_popup_on_target(&mut app)?;
    // Query that hits multiple files so navigation has an effect:
    // `.md` matches every markdown file in the vault.
    for c in ".md".chars() {
        app.dispatch(key(c))?;
    }
    let initial = render(&mut app, 100, 30);
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)))?;
    let after_down = render(&mut app, 100, 30);
    assert!(
        initial != after_down,
        "Down arrow should change the highlighted row:\nbefore:\n{initial}\nafter:\n{after_down}"
    );
    Ok(())
}

#[test]
fn target_picker_esc_cancels_and_leaves_field_unchanged() -> Result<()> {
    let (_dir, vault) = target_picker_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    open_new_popup_on_target(&mut app)?;
    for c in "Inbox".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)))?;
    let frame = render(&mut app, 100, 30);
    assert!(
        !frame.contains("pick target"),
        "picker should be closed:\n{frame}"
    );
    assert!(
        frame.contains("new task"),
        "popup should still be open:\n{frame}"
    );
    Ok(())
}

#[test]
fn target_picker_does_not_open_from_description_field() -> Result<()> {
    let (_dir, vault) = target_picker_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(1)?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('C'),
        KeyModifiers::SHIFT,
    )))?;
    // Description is the default focus — typing here must not open
    // the picker, it should insert into the description buffer.
    for c in "Inbox".chars() {
        app.dispatch(key(c))?;
    }
    let frame = render(&mut app, 100, 30);
    assert!(
        !frame.contains("pick target"),
        "picker must not open from description focus:\n{frame}"
    );
    assert!(
        frame.contains("Inbox"),
        "description should show typed text:\n{frame}"
    );
    Ok(())
}

// ── Notes tab (plan 003 · session 3) ─────────────────────────────────────

/// Notes-tab snapshot vault: a couple of files with headings so the
/// fuzzy picker has something to surface.
fn notes_vault() -> (TempDir, Vault) {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("test-vault");
    std::fs::create_dir_all(vault_path.join(".obsidian")).unwrap();
    std::fs::write(
        vault_path.join("project.md"),
        "# Project\n\n## Background\n\nIntro.\n\n## Tasks\n\n- Do thing\n",
    )
    .unwrap();
    std::fs::write(vault_path.join("inbox.md"), "# Inbox\n\nNotes.\n").unwrap();
    let vault = Vault::discover(Some(vault_path)).unwrap();
    (dir, vault)
}

const NOTES_TAB_INDEX: usize = 2;

#[test]
fn notes_tab_idle_renders_keymap_panel() -> Result<()> {
    let (_dir, vault) = notes_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(NOTES_TAB_INDEX)?;
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("notes_idle_80x24", frame);
    Ok(())
}

#[test]
fn notes_tab_help_overlay_renders_over_idle() -> Result<()> {
    let (_dir, vault) = notes_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(NOTES_TAB_INDEX)?;
    app.dispatch(key('?'))?;
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("notes_help_overlay_80x24", frame);
    Ok(())
}

#[test]
fn notes_tab_open_picker_renders_results() -> Result<()> {
    let (_dir, vault) = notes_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(NOTES_TAB_INDEX)?;
    app.dispatch(key('o'))?;
    for c in "project".chars() {
        app.dispatch(key(c))?;
    }
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("notes_open_picker_80x24", frame);
    Ok(())
}

#[test]
fn notes_tab_open_picker_enter_queues_editor_open() -> Result<()> {
    let (dir, vault) = notes_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(NOTES_TAB_INDEX)?;
    app.dispatch(key('o'))?;
    for c in "project".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    let req = app
        .take_pending_request()
        .expect("Enter should queue OpenInEditor");
    match req {
        AppRequest::OpenInEditor { path, line: _ } => {
            let expected = dir
                .path()
                .join("test-vault/project.md")
                .canonicalize()
                .unwrap();
            assert_eq!(path.canonicalize().unwrap(), expected);
        }
        other => panic!("expected OpenInEditor, got {other:?}"),
    }
    Ok(())
}

#[test]
fn notes_tab_open_picker_ctrl_o_queues_obsidian_url() -> Result<()> {
    let (_dir, vault) = notes_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(NOTES_TAB_INDEX)?;
    app.dispatch(key('o'))?;
    for c in "project".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )))?;
    let req = app
        .take_pending_request()
        .expect("Ctrl+O should queue OpenInObsidian");
    match req {
        AppRequest::OpenInObsidian { url } => {
            assert!(
                url.starts_with("obsidian://open?vault=") && url.contains("file=project.md"),
                "unexpected URL: {url}"
            );
        }
        other => panic!("expected OpenInObsidian, got {other:?}"),
    }
    Ok(())
}

#[test]
fn notes_tab_open_picker_esc_returns_to_idle() -> Result<()> {
    let (_dir, vault) = notes_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(NOTES_TAB_INDEX)?;
    app.dispatch(key('o'))?;
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)))?;
    let frame = render(&mut app, 80, 24);
    assert!(
        !frame.contains("pick file / heading"),
        "picker should be closed:\n{frame}"
    );
    Ok(())
}

// ── Notes tab · section-move flow (plan 003 · session 4) ─────────────────

/// Vault tailored for the section-move flow. Two notes with a few headings
/// and a known nested structure: `project.md` has H1 + two H2s, one of
/// which has an H3 child — the nested heading lets us exercise the
/// implicit-selection cascade.
fn notes_move_vault() -> (TempDir, Vault) {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("test-vault");
    std::fs::create_dir_all(vault_path.join(".obsidian")).unwrap();
    std::fs::write(
        vault_path.join("project.md"),
        "# Project\n\n## Background\n\nIntro.\n\n### Details\n\nMore.\n\n## Tasks\n\n- Do thing\n",
    )
    .unwrap();
    std::fs::write(
        vault_path.join("archive.md"),
        "# Archive\n\n## Old\n\nStale notes.\n",
    )
    .unwrap();
    let vault = Vault::discover(Some(vault_path)).unwrap();
    (dir, vault)
}

/// Drive the Notes tab into the heading-multi-select step with
/// `project.md` as the source. Returns the populated App.
fn drive_to_multiselect(vault: Vault) -> Result<App> {
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(NOTES_TAB_INDEX)?;
    app.dispatch(key('m'))?;
    for c in "project".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    Ok(app)
}

#[test]
fn notes_move_source_picker_opens_on_m() -> Result<()> {
    let (_dir, vault) = notes_move_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(NOTES_TAB_INDEX)?;
    app.dispatch(key('m'))?;
    for c in "project".chars() {
        app.dispatch(key(c))?;
    }
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("notes_move_source_picker_80x24", frame);
    Ok(())
}

#[test]
fn notes_move_source_picker_esc_returns_to_idle() -> Result<()> {
    let (_dir, vault) = notes_move_vault();
    let mut app = App::for_test_with_clock(vault, fixed_clock);
    app.switch_to(NOTES_TAB_INDEX)?;
    app.dispatch(key('m'))?;
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)))?;
    let frame = render(&mut app, 80, 24);
    assert!(
        !frame.contains("1/4 source"),
        "source picker should be closed:\n{frame}"
    );
    Ok(())
}

#[test]
fn notes_move_multiselect_renders_after_source_pick() -> Result<()> {
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_multiselect(vault)?;
    let frame = render(&mut app, 80, 24);
    assert!(
        frame.contains("2/4 select"),
        "should land on multi-select step:\n{frame}"
    );
    assert!(
        frame.contains("Background"),
        "headings should be listed:\n{frame}"
    );
    Ok(())
}

#[test]
fn notes_move_multiselect_implicit_descendants_dim() -> Result<()> {
    // Select the H2 "Background" — its H3 child "Details" should show
    // as implicitly included, with the dimmed marker glyph.
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_multiselect(vault)?;
    // Focus is on heading 0 (H1 "Project"). Move to "Background" (idx 1).
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)))?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char(' '),
        KeyModifiers::NONE,
    )))?;
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("notes_move_multiselect_80x24", frame);
    Ok(())
}

#[test]
fn notes_move_multiselect_descendant_toggle_blocked_by_parent() -> Result<()> {
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_multiselect(vault)?;
    // Select Background (idx 1) — Details (idx 2) becomes implicit.
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)))?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char(' '),
        KeyModifiers::NONE,
    )))?;
    // Move to Details and try to toggle — should be a no-op.
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)))?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char(' '),
        KeyModifiers::NONE,
    )))?;
    // Now deselect Background — Details should return to unselected (no
    // implicit, no explicit).
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)))?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char(' '),
        KeyModifiers::NONE,
    )))?;
    let frame = render(&mut app, 80, 24);
    // After deselecting Background, no row should carry an explicit or
    // implicit marker — only the empty box.
    assert!(
        !frame.contains('■') && !frame.contains('▣'),
        "all selection markers should be cleared:\n{frame}"
    );
    Ok(())
}

#[test]
fn notes_move_multiselect_enter_advances_to_target_picker() -> Result<()> {
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_multiselect(vault)?;
    // Pick H1 Project (focus starts here).
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char(' '),
        KeyModifiers::NONE,
    )))?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    for c in "archive".chars() {
        app.dispatch(key(c))?;
    }
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("notes_move_target_picker_80x24", frame);
    Ok(())
}

#[test]
fn notes_move_multiselect_enter_with_no_selection_stays() -> Result<()> {
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_multiselect(vault)?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    let frame = render(&mut app, 80, 24);
    assert!(
        frame.contains("2/4 select"),
        "should remain on multi-select with no picks:\n{frame}"
    );
    Ok(())
}

#[test]
fn notes_move_multiselect_esc_returns_to_source_picker() -> Result<()> {
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_multiselect(vault)?;
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)))?;
    let frame = render(&mut app, 80, 24);
    assert!(
        frame.contains("1/4 source"),
        "should return to source picker:\n{frame}"
    );
    Ok(())
}

#[test]
fn notes_move_target_same_file_rejected_inline() -> Result<()> {
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_multiselect(vault)?;
    // Pick H1 and advance to target picker.
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char(' '),
        KeyModifiers::NONE,
    )))?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    // Type a query that matches the source file.
    for c in "project".chars() {
        app.dispatch(key(c))?;
    }
    // Press Enter on the source file (same path) — should be rejected.
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    let frame = render(&mut app, 80, 24);
    assert!(
        frame.contains("same-file move"),
        "footer should explain the rejection:\n{frame}"
    );
    assert!(
        frame.contains("3/4 target"),
        "should still be on target step:\n{frame}"
    );
    Ok(())
}

#[test]
fn notes_move_target_enter_advances_to_compose() -> Result<()> {
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_multiselect(vault)?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char(' '),
        KeyModifiers::NONE,
    )))?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    for c in "archive".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    let frame = render(&mut app, 80, 24);
    assert!(
        frame.contains("4/4 compose"),
        "should advance to compose:\n{frame}"
    );
    assert!(
        frame.contains("archive.md"),
        "target file should appear in title:\n{frame}"
    );
    Ok(())
}

#[test]
fn notes_move_target_esc_returns_to_multiselect_preserving_picks() -> Result<()> {
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_multiselect(vault)?;
    // Pick H1 Project (focus starts here) then advance.
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char(' '),
        KeyModifiers::NONE,
    )))?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    // Back out without picking a target.
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)))?;
    let frame = render(&mut app, 80, 24);
    assert!(
        frame.contains("2/4 select"),
        "should be back on multi-select:\n{frame}"
    );
    // The explicit-pick marker should still be visible for `Project`.
    assert!(
        frame.contains('■'),
        "selection should be preserved:\n{frame}"
    );
    Ok(())
}

// ── Notes tab · section-move flow (plan 003 · session 5) ─────────────────

/// Drive the Notes tab all the way to the compose step, with the H1
/// "Project" picked as the only section. Source is `project.md`, target
/// is `archive.md`.
fn drive_to_compose(vault: Vault) -> Result<App> {
    let mut app = drive_to_multiselect(vault)?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char(' '),
        KeyModifiers::NONE,
    )))?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    for c in "archive".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    Ok(app)
}

#[test]
fn notes_move_compose_renders_interleaved_layout() -> Result<()> {
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_compose(vault)?;
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("notes_move_compose_80x24", frame);
    Ok(())
}

#[test]
fn notes_move_compose_esc_returns_to_target_picker() -> Result<()> {
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_compose(vault)?;
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)))?;
    let frame = render(&mut app, 80, 24);
    assert!(
        frame.contains("3/4 target"),
        "Esc should return to target picker:\n{frame}"
    );
    Ok(())
}

#[test]
fn notes_move_compose_level_shift_clamps_at_one() -> Result<()> {
    // Pending starts at H1; Left should be ignored (already min).
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_compose(vault)?;
    let before = render(&mut app, 80, 24);
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)))?;
    let after = render(&mut app, 80, 24);
    assert_eq!(before, after, "Left at H1 should be a no-op");
    Ok(())
}

#[test]
fn notes_move_compose_level_shift_right_increments() -> Result<()> {
    // Move Pending from H1 to H2. Note: source content has H1 (Project)
    // with H2/H3 descendants, so shifting from 1→2 would cascade an H3
    // to H4, which is fine (no overflow). Shifting to H2 should succeed.
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_compose(vault)?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Right,
        KeyModifiers::NONE,
    )))?;
    let frame = render(&mut app, 80, 24);
    // After shift the focused Pending row should be H2.
    assert!(
        frame.contains("H2  Project"),
        "Pending row should now be H2:\n{frame}"
    );
    Ok(())
}

#[test]
fn notes_move_compose_enter_commits_and_writes_files() -> Result<()> {
    let (dir, vault) = notes_move_vault();
    let vault_path = vault.path.clone();
    let mut app = drive_to_compose(vault)?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    let req = app
        .take_pending_request()
        .expect("commit should queue a success toast");
    match req {
        AppRequest::Toast { text, .. } => {
            assert!(
                text.starts_with("Moved 1 section(s):"),
                "success toast text: {text}"
            );
        }
        other => panic!("expected Toast, got {other:?}"),
    }
    let new_source = std::fs::read_to_string(vault_path.join("project.md"))?;
    let new_target = std::fs::read_to_string(vault_path.join("archive.md"))?;
    assert!(
        !new_source.contains("# Project"),
        "H1 should be moved out of source:\n{new_source}"
    );
    assert!(
        new_target.contains("# Project"),
        "H1 should appear in target:\n{new_target}"
    );
    // Returned to idle.
    let frame = render(&mut app, 80, 24);
    assert!(
        !frame.contains("4/4 compose"),
        "should leave compose after commit:\n{frame}"
    );
    drop(dir);
    Ok(())
}

#[test]
fn notes_move_compose_reorder_shift_down_swaps_with_anchor() -> Result<()> {
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_compose(vault)?;
    // Focus starts on the first Pending row, which sits after the target's
    // anchors. Shift+Up swaps the Pending up past one Anchor.
    let before = render(&mut app, 80, 24);
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT)))?;
    let after = render(&mut app, 80, 24);
    assert_ne!(
        before, after,
        "Shift+Up on the first Pending should reorder it past an Anchor"
    );
    Ok(())
}

// ── Notes tab · section-move flow (plan 007 · rename) ────────────────────

/// Drive into compose with two H2 picks (Background + Tasks). Source
/// `project.md` keeps H1 Project; target is `archive.md`. Two Pending
/// rows in the compose layout.
fn drive_to_compose_two_h2_picks(vault: Vault) -> Result<App> {
    let mut app = drive_to_multiselect(vault)?;
    // Focus starts on heading 0 (H1 Project). Move down to H2 Background.
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)))?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char(' '),
        KeyModifiers::NONE,
    )))?;
    // Down past H3 Details (implicit) to H2 Tasks.
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)))?;
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)))?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Char(' '),
        KeyModifiers::NONE,
    )))?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    for c in "archive".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    Ok(app)
}

#[test]
fn notes_move_compose_r_opens_rename_buffer_prefilled() -> Result<()> {
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_compose(vault)?;
    app.dispatch(key('r'))?;
    let frame = render(&mut app, 80, 24);
    assert!(
        frame.contains("rename → Project"),
        "buffer should be pre-filled with source text:\n{frame}"
    );
    assert!(
        frame.contains("commit rename"),
        "footer should switch to the rename-buffer keymap:\n{frame}"
    );
    Ok(())
}

#[test]
fn notes_move_compose_rename_enter_commits_override() -> Result<()> {
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_compose(vault)?;
    app.dispatch(key('r'))?;
    // Clear pre-filled text and type a new title.
    for _ in 0..32 {
        app.dispatch(Event::Key(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )))?;
    }
    for c in "Sprint 1".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    let frame = render(&mut app, 80, 24);
    assert!(
        frame.contains("→ Sprint 1"),
        "Pending row should show the rename override:\n{frame}"
    );
    assert!(
        !frame.contains("rename → "),
        "edit field should be gone after commit:\n{frame}"
    );
    Ok(())
}

#[test]
fn notes_move_compose_rename_empty_keeps_buffer_open_with_toast() -> Result<()> {
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_compose(vault)?;
    app.dispatch(key('r'))?;
    // Empty out the pre-filled text.
    for _ in 0..32 {
        app.dispatch(Event::Key(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    let req = app
        .take_pending_request()
        .expect("empty rename should queue a toast");
    match req {
        AppRequest::Toast { text, style } => {
            assert_eq!(text, "rename cannot be empty");
            assert_eq!(style, crate::tui::tab::ToastStyle::Error);
        }
        other => panic!("expected Toast, got {other:?}"),
    }
    let frame = render(&mut app, 80, 24);
    assert!(
        frame.contains("rename → "),
        "buffer should stay open after invalid Enter:\n{frame}"
    );
    Ok(())
}

#[test]
fn notes_move_compose_rename_whitespace_only_keeps_buffer_open_with_toast() -> Result<()> {
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_compose(vault)?;
    app.dispatch(key('r'))?;
    for _ in 0..32 {
        app.dispatch(Event::Key(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )))?;
    }
    app.dispatch(key(' '))?;
    app.dispatch(key(' '))?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    let req = app
        .take_pending_request()
        .expect("whitespace-only rename should queue a toast");
    match req {
        AppRequest::Toast { text, .. } => {
            assert_eq!(text, "rename cannot be empty");
        }
        other => panic!("expected Toast, got {other:?}"),
    }
    let frame = render(&mut app, 80, 24);
    assert!(
        frame.contains("rename → "),
        "buffer should stay open after whitespace-only Enter:\n{frame}"
    );
    Ok(())
}

#[test]
fn notes_move_compose_rename_esc_discards_buffer() -> Result<()> {
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_compose(vault)?;
    app.dispatch(key('r'))?;
    for c in "garbage".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)))?;
    let frame = render(&mut app, 80, 24);
    assert!(
        !frame.contains("rename → "),
        "buffer should be closed after Esc:\n{frame}"
    );
    assert!(
        !frame.contains("→ garbage"),
        "row should not carry the discarded override:\n{frame}"
    );
    assert!(
        frame.contains("4/4 compose"),
        "should still be on compose step:\n{frame}"
    );
    Ok(())
}

#[test]
fn notes_move_compose_rename_buffer_swallows_shift_up() -> Result<()> {
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_compose(vault)?;
    app.dispatch(key('r'))?;
    let before = render(&mut app, 80, 24);
    // Shift+Up would reorder a Pending row in normal compose; the
    // buffer must swallow it so the layout stays put.
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT)))?;
    let after = render(&mut app, 80, 24);
    assert_eq!(
        before, after,
        "Shift+Up should be a no-op while the rename buffer is open"
    );
    Ok(())
}

#[test]
fn notes_move_compose_rename_preserved_after_shift_up() -> Result<()> {
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_compose(vault)?;
    app.dispatch(key('r'))?;
    for _ in 0..32 {
        app.dispatch(Event::Key(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )))?;
    }
    for c in "Renamed".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    // Now reorder the renamed Pending row up past an Anchor.
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT)))?;
    let frame = render(&mut app, 80, 24);
    assert!(
        frame.contains("→ Renamed"),
        "rename override should survive a reorder:\n{frame}"
    );
    Ok(())
}

#[test]
fn notes_move_compose_rename_preserved_after_level_shift() -> Result<()> {
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_compose(vault)?;
    app.dispatch(key('r'))?;
    for _ in 0..32 {
        app.dispatch(Event::Key(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )))?;
    }
    for c in "Renamed".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    // Bump level from H1 to H2; cascade is safe (H3 → H4 still in range).
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Right,
        KeyModifiers::NONE,
    )))?;
    let frame = render(&mut app, 80, 24);
    assert!(
        frame.contains("H2  Project"),
        "level shift should apply:\n{frame}"
    );
    assert!(
        frame.contains("→ Renamed"),
        "rename override should survive a level shift:\n{frame}"
    );
    Ok(())
}

#[test]
fn notes_move_compose_rename_writes_renamed_heading_to_disk() -> Result<()> {
    let (dir, vault) = notes_move_vault();
    let vault_path = vault.path.clone();
    let mut app = drive_to_compose(vault)?;
    app.dispatch(key('r'))?;
    for _ in 0..32 {
        app.dispatch(Event::Key(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )))?;
    }
    for c in "Renamed Project".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    // Commit the move.
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    let req = app
        .take_pending_request()
        .expect("commit should queue a success toast");
    match req {
        AppRequest::Toast { text, .. } => {
            assert!(
                text.starts_with("Moved 1 section(s):"),
                "success toast: {text}"
            );
        }
        other => panic!("expected Toast, got {other:?}"),
    }
    let new_target = std::fs::read_to_string(vault_path.join("archive.md"))?;
    assert!(
        new_target.contains("# Renamed Project"),
        "target should contain the renamed H1:\n{new_target}"
    );
    assert!(
        !new_target.contains("# Project\n"),
        "target should NOT contain the original H1 line:\n{new_target}"
    );
    drop(dir);
    Ok(())
}

#[test]
fn notes_move_compose_renamed_snapshot() -> Result<()> {
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_compose(vault)?;
    app.dispatch(key('r'))?;
    for _ in 0..32 {
        app.dispatch(Event::Key(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )))?;
    }
    for c in "Renamed Project".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("notes_move_compose_renamed_80x24", frame);
    Ok(())
}

#[test]
fn notes_move_compose_renaming_snapshot() -> Result<()> {
    let (_dir, vault) = notes_move_vault();
    let mut app = drive_to_compose(vault)?;
    app.dispatch(key('r'))?;
    for _ in 0..32 {
        app.dispatch(Event::Key(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )))?;
    }
    for c in "Sprint".chars() {
        app.dispatch(key(c))?;
    }
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("notes_move_compose_renaming_80x24", frame);
    Ok(())
}

#[test]
fn notes_move_compose_rename_e2e_two_h2_picks() -> Result<()> {
    let (dir, vault) = notes_move_vault();
    let vault_path = vault.path.clone();
    let mut app = drive_to_compose_two_h2_picks(vault)?;
    // Layout: anchors [Archive, Old] then pending [Background, Tasks].
    // Focus lands on the first Pending (Background). Move focus down to
    // the second Pending (Tasks) and rename only that one.
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)))?;
    app.dispatch(key('r'))?;
    for _ in 0..32 {
        app.dispatch(Event::Key(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )))?;
    }
    for c in "Sprint 1".chars() {
        app.dispatch(key(c))?;
    }
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    // Commit.
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    let req = app
        .take_pending_request()
        .expect("commit should queue a success toast");
    match req {
        AppRequest::Toast { text, .. } => {
            assert!(
                text.starts_with("Moved 2 section(s):"),
                "success toast: {text}"
            );
        }
        other => panic!("expected Toast, got {other:?}"),
    }
    let new_source = std::fs::read_to_string(vault_path.join("project.md"))?;
    let new_target = std::fs::read_to_string(vault_path.join("archive.md"))?;
    // Source loses both moved sections.
    assert!(
        !new_source.contains("## Background"),
        "Background should be removed from source:\n{new_source}"
    );
    assert!(
        !new_source.contains("## Tasks"),
        "Tasks should be removed from source:\n{new_source}"
    );
    // Target keeps prior content and gains both sections; Tasks is renamed.
    assert!(
        new_target.contains("# Archive"),
        "Archive H1 preserved in target:\n{new_target}"
    );
    assert!(
        new_target.contains("## Background"),
        "un-renamed pending should land verbatim:\n{new_target}"
    );
    assert!(
        new_target.contains("### Details"),
        "nested H3 should cascade with its parent:\n{new_target}"
    );
    assert!(
        new_target.contains("## Sprint 1"),
        "renamed pending should land with the new title:\n{new_target}"
    );
    assert!(
        !new_target.contains("## Tasks"),
        "original 'Tasks' title should NOT appear:\n{new_target}"
    );
    drop(dir);
    Ok(())
}

// ── plan 008: empty-input picker shows recents ───────────────────────────────

/// Build a notes vault with explicit deterministic mtimes so recents tests
/// can assert ordering by recency rather than relying on file-system
/// resolution. `files` is `(rel_path, body, mtime_offset_seconds_from_base)`
/// — bigger offset = newer file.
fn recents_vault(files: &[(&str, &str, u64)]) -> (TempDir, Vault) {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("test-vault");
    std::fs::create_dir_all(vault_path.join(".obsidian")).unwrap();
    let base = std::time::SystemTime::now();
    for (rel, body, offset) in files {
        let abs = vault_path.join(rel);
        if let Some(p) = abs.parent() {
            std::fs::create_dir_all(p).unwrap();
        }
        std::fs::write(&abs, body).unwrap();
        let mt = base + std::time::Duration::from_secs(*offset);
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&abs) {
            let _ = f.set_times(std::fs::FileTimes::new().set_modified(mt));
        }
    }
    let vault = Vault::discover(Some(vault_path)).unwrap();
    (dir, vault)
}

fn make_test_recents(vault: &Vault, tmp: &TempDir) -> Arc<RecentsLog> {
    let log_path = tmp.path().join("recents.jsonl");
    Arc::new(RecentsLog::with_log_path(vault.path.clone(), log_path))
}

#[test]
fn notes_open_picker_shows_logged_open_first() -> Result<()> {
    let (dir, vault) = recents_vault(&[
        ("alpha.md", "# Alpha\n", 100),
        ("beta.md", "# Beta\n", 200),
        ("gamma.md", "# Gamma\n", 300),
    ]);
    let recents = make_test_recents(&vault, &dir);
    // Log an open on alpha — even though gamma has the newest mtime,
    // alpha should lead the recents list because opens beat mtime.
    recents.record_open(std::path::Path::new("alpha.md"));

    let mut app = App::for_test_with_recents(vault, recents);
    app.switch_to(NOTES_TAB_INDEX)?;
    app.dispatch(key('o'))?;

    let frame = render(&mut app, 80, 24);
    // The "recent" title flips on for empty input + populated items.
    assert!(
        frame.contains("recent"),
        "expected `recent` in title for empty-input picker:\n{frame}"
    );
    // All three files appear; alpha is on the first row (after the
    // input-box rows).
    assert!(frame.contains("alpha.md"));
    assert!(frame.contains("beta.md"));
    assert!(frame.contains("gamma.md"));
    let alpha_pos = frame.find("alpha.md").unwrap();
    let beta_pos = frame.find("beta.md").unwrap();
    let gamma_pos = frame.find("gamma.md").unwrap();
    assert!(
        alpha_pos < beta_pos && alpha_pos < gamma_pos,
        "alpha (opened) must appear above beta and gamma (mtime only)"
    );
    Ok(())
}

#[test]
fn notes_open_picker_empty_log_falls_back_to_mtime() -> Result<()> {
    let (dir, vault) = recents_vault(&[
        ("oldest.md", "# O\n", 10),
        ("middle.md", "# M\n", 100),
        ("newest.md", "# N\n", 1000),
    ]);
    let recents = make_test_recents(&vault, &dir);
    let mut app = App::for_test_with_recents(vault, recents);
    app.switch_to(NOTES_TAB_INDEX)?;
    app.dispatch(key('o'))?;

    let frame = render(&mut app, 80, 24);
    let newest_pos = frame.find("newest.md").unwrap();
    let middle_pos = frame.find("middle.md").unwrap();
    let oldest_pos = frame.find("oldest.md").unwrap();
    assert!(
        newest_pos < middle_pos && middle_pos < oldest_pos,
        "expected mtime order newest→middle→oldest; got positions {newest_pos}, {middle_pos}, {oldest_pos}\n{frame}"
    );
    Ok(())
}

#[test]
fn notes_open_picker_cold_start_shows_type_to_search_hint() -> Result<()> {
    // Vault has zero `.md` files — recents list is empty, picker should
    // fall back to the legacy "type to search…" hint.
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("test-vault");
    std::fs::create_dir_all(vault_path.join(".obsidian")).unwrap();
    let vault = Vault::discover(Some(vault_path)).unwrap();
    let recents = make_test_recents(&vault, &dir);
    let mut app = App::for_test_with_recents(vault, recents);
    app.switch_to(NOTES_TAB_INDEX)?;
    app.dispatch(key('o'))?;

    let frame = render(&mut app, 80, 24);
    assert!(
        frame.contains("type to search"),
        "cold-start picker should show legacy hint:\n{frame}"
    );
    Ok(())
}

#[test]
fn notes_open_picker_typing_transitions_from_recents_to_results() -> Result<()> {
    let (dir, vault) = recents_vault(&[("alpha.md", "# A\n", 100), ("beta.md", "# B\n", 200)]);
    let recents = make_test_recents(&vault, &dir);
    let mut app = App::for_test_with_recents(vault, recents);
    app.switch_to(NOTES_TAB_INDEX)?;
    app.dispatch(key('o'))?;

    // Empty input → "recent · type to search" title.
    let frame = render(&mut app, 80, 24);
    assert!(frame.contains("recent"));

    // Typing one char flips to fuzzy mode → title is " results ".
    app.dispatch(key('a'))?;
    let frame = render(&mut app, 80, 24);
    assert!(
        frame.contains("results"),
        "typing should switch to results mode:\n{frame}"
    );
    Ok(())
}

#[test]
fn notes_open_picker_backspace_returns_to_recents() -> Result<()> {
    let (dir, vault) = recents_vault(&[("alpha.md", "# A\n", 100), ("beta.md", "# B\n", 200)]);
    let recents = make_test_recents(&vault, &dir);
    let mut app = App::for_test_with_recents(vault, recents);
    app.switch_to(NOTES_TAB_INDEX)?;
    app.dispatch(key('o'))?;
    // Type then immediately erase — should land back in recents mode.
    app.dispatch(key('a'))?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Backspace,
        KeyModifiers::NONE,
    )))?;
    let frame = render(&mut app, 80, 24);
    assert!(
        frame.contains("recent"),
        "backspace to empty input should restore recents mode:\n{frame}"
    );
    Ok(())
}

#[test]
fn notes_open_picker_enter_on_recent_records_and_reopens_at_top() -> Result<()> {
    // End-to-end: open picker, select gamma (mtime-newest), the open is
    // recorded, then re-open picker and assert gamma still leads — but
    // now because it was *opened* (its log entry beats any mtime tail).
    let (dir, vault) = recents_vault(&[
        ("alpha.md", "# A\n", 100),
        ("beta.md", "# B\n", 200),
        ("gamma.md", "# G\n", 300),
    ]);
    let recents = make_test_recents(&vault, &dir);
    // Pre-seed with alpha so it's "second" in the merged list — gamma's
    // mtime puts it first. After the user opens gamma, we should still
    // see gamma at top (now via the opens slice).
    recents.record_open(std::path::Path::new("alpha.md"));
    let recents_clone = Arc::clone(&recents);

    let mut app = App::for_test_with_recents(vault, recents_clone);
    app.switch_to(NOTES_TAB_INDEX)?;

    // First open: pick gamma (rendered at top thanks to mtime).
    app.dispatch(key('o'))?;
    // Navigate: with opens-first, alpha is row 0, then mtime-ordered
    // gamma (row 1) → beta (row 2). Press Down to land on gamma.
    app.dispatch(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)))?;
    app.dispatch(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))?;
    let req = app
        .take_pending_request()
        .expect("Enter should queue OpenInEditor");
    match req {
        AppRequest::OpenInEditor { path, .. } => {
            assert!(
                path.to_string_lossy().ends_with("gamma.md"),
                "expected gamma.md got {path:?}"
            );
        }
        other => panic!("expected OpenInEditor, got {other:?}"),
    }

    // After the open, both alpha and gamma should be in the recents
    // log, with gamma most recent.
    let logged = recents.load_recent(10);
    assert_eq!(
        logged,
        vec![
            std::path::PathBuf::from("gamma.md"),
            std::path::PathBuf::from("alpha.md")
        ],
        "recents log should reflect both opens with gamma newest"
    );

    // Re-open picker. gamma is now top of the opens slice.
    app.dispatch(key('o'))?;
    let frame = render(&mut app, 80, 24);
    let gamma_pos = frame.find("gamma.md").unwrap();
    let alpha_pos = frame.find("alpha.md").unwrap();
    let beta_pos = frame.find("beta.md").unwrap();
    assert!(
        gamma_pos < alpha_pos && alpha_pos < beta_pos,
        "after open, expected gamma → alpha → beta order; got positions {gamma_pos}, {alpha_pos}, {beta_pos}\n{frame}"
    );
    Ok(())
}

#[test]
fn notes_open_picker_recents_snapshot_80x24() -> Result<()> {
    let (dir, vault) = recents_vault(&[
        ("project.md", "# Project\n", 100),
        ("inbox.md", "# Inbox\n", 200),
        ("notes/daily.md", "# Daily\n", 300),
    ]);
    let recents = make_test_recents(&vault, &dir);
    // Mixed signals: project is opened (top); inbox + daily fill via
    // mtime tail (daily newer than inbox).
    recents.record_open(std::path::Path::new("project.md"));

    let mut app = App::for_test_with_clock_and_recents(vault, fixed_clock, recents);
    app.switch_to(NOTES_TAB_INDEX)?;
    app.dispatch(key('o'))?;
    let frame = render(&mut app, 80, 24);
    assert_tui_snapshot!("notes_open_picker_recents_80x24", frame);
    Ok(())
}

#[test]
fn cli_record_open_through_recents_log() -> Result<()> {
    // Verify the CLI path: `RecentsLog::for_vault(&vault).record_open(...)`
    // writes to the per-vault log. Uses an isolated XDG_STATE_HOME so we
    // don't touch the user's real state dir.
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("test-vault");
    std::fs::create_dir_all(vault_path.join(".obsidian")).unwrap();
    std::fs::write(vault_path.join("note.md"), "# N\n").unwrap();
    let vault = Vault::discover(Some(vault_path.clone())).unwrap();

    let state_root = dir.path().join("state");
    let prev = std::env::var_os("XDG_STATE_HOME");
    std::env::set_var("XDG_STATE_HOME", &state_root);
    let log = RecentsLog::for_vault(&vault);
    log.record_open(std::path::Path::new("note.md"));
    // Read it back via the same construction to confirm round-trip.
    let log2 = RecentsLog::for_vault(&vault);
    let entries = log2.load_recent(10);
    match prev {
        Some(v) => std::env::set_var("XDG_STATE_HOME", v),
        None => std::env::remove_var("XDG_STATE_HOME"),
    }
    assert_eq!(entries, vec![std::path::PathBuf::from("note.md")]);
    Ok(())
}
