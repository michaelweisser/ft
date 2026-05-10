use anyhow::Result;
use assert_fs::TempDir;
use chrono::{DateTime, Local, TimeZone};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ft_core::vault::Vault;
use ratatui::{backend::TestBackend, Terminal};

use crate::tui::{event::Event, App};

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
