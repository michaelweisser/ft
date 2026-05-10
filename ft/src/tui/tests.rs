use anyhow::Result;
use assert_fs::TempDir;
use chrono::{DateTime, Local, TimeZone};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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
        narrow.contains("This is a fairly l…") || narrow.contains("This is a fairly lo…"),
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
    "Enter",
    "R",
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
