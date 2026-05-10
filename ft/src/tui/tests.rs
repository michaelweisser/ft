use anyhow::Result;
use assert_fs::TempDir;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ft_core::vault::Vault;
use ratatui::{backend::TestBackend, Terminal};

use crate::tui::{event::Event, App};

fn test_vault() -> (TempDir, Vault) {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("test-vault");
    std::fs::create_dir_all(vault_path.join(".obsidian")).unwrap();
    let vault = Vault::discover(Some(vault_path)).unwrap();
    (dir, vault)
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
    insta::assert_snapshot!("welcome_tab_80x24", frame);
}

#[test]
fn help_overlay_renders_over_welcome() {
    let (_dir, vault) = test_vault();
    let mut app = App::for_test(vault);
    app.enter_help();
    let frame = render(&mut app, 80, 24);
    insta::assert_snapshot!("help_overlay_80x24", frame);
}

#[test]
fn tasks_placeholder_tab_renders() -> Result<()> {
    let (_dir, vault) = test_vault();
    let mut app = App::for_test(vault);
    app.switch_to(1)?;
    let frame = render(&mut app, 80, 24);
    insta::assert_snapshot!("tasks_placeholder_80x24", frame);
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
