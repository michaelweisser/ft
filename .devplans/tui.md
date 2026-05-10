---
id: 002
name: tui
title: Interactive TUI for vault management
status: implementing
created: 2026-05-09
updated: 2026-05-10
---

# Interactive TUI for vault management

## Goal
Add an interactive `ft tui` subcommand that opens a tabbed full-screen terminal
UI built on `ratatui`. v1 ships two tabs: a Welcome splash and a Tasks tab with
a left sidebar + right viewport layout. The Tasks tab covers the core daily
workflow — scan what's due, nudge dates, adjust priorities, edit, complete, and
cancel — without leaving the terminal. Bulk operations, staleness detection, and
additional tabs are explicitly deferred.

## Motivation and Context
The CLI from plan 001 is great for scripting and quick lookups, but for the
daily "what should I work on now?" workflow the user wants to scan, quickly
triage, and update tasks without re-typing flags. The key insight is two speeds
of interaction: fast single-key nudges for the most common mutations (move due
date one day, bump priority) and a full edit popup for anything more involved.
The first view defaults to tasks that are overdue or due soon, sorted by due
date and priority — exactly what matters at the start of a work session.

## Acceptance Criteria

### Foundation
- [x] `ft tui` subcommand launches the TUI; exits cleanly on `q` or Ctrl+C
- [x] Single binary; the subcommand is registered alongside the others from plan 001
- [x] Renders correctly in 80x24 minimum, scales gracefully up to large terminals
- [x] Dark theme only (light mode and `--theme` flag are v2)
- [x] `ft tui` reuses the same vault discovery and config as the CLI from plan 001
- [x] `?` opens a static help overlay listing all keybindings; `Esc` or `?` closes it

### Tab system
- [x] Top bar shows tab names; active tab is visually highlighted
- [x] Switch tabs with `Tab` / `Shift-Tab` or number keys `1` / `2`
- [x] Tabs implement a `Tab` trait so adding new ones requires no surgery to the core loop:
  ```rust
  trait Tab {
      fn title(&self) -> &str;
      fn on_focus(&mut self, ctx: &mut TabCtx) -> Result<()>;
      fn on_blur(&mut self, ctx: &mut TabCtx) -> Result<()>;
      fn handle_event(&mut self, ev: Event, ctx: &mut TabCtx) -> Result<EventOutcome>;
      fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &TabCtx);
      fn refresh(&mut self, ctx: &mut TabCtx) -> Result<()>;
  }
  ```
- [x] Bottom status bar shows: vault name, current tab name, last-refresh timestamp, mode hint (normal / edit / help)

### Welcome tab
- [x] First tab shown on launch
- [x] Displays "Welcome" in ASCII art (or a styled block-letter banner)
- [x] No interactive elements; any key press switches to the Tasks tab

### Tasks tab — layout
- [x] Split layout: fixed-width left sidebar (~25 chars) + right viewport filling the rest
- [x] Left sidebar contains: current date and time (updated every second), and a view dropdown
- [x] View dropdown lists available views; first and only v1 view is "Search"; navigate with `↑`/`↓`, select with `Enter`
- [x] Right viewport renders the active view
- [x] Loads task data on first focus (lazy); shows a loading indicator while scanning

### Tasks tab — Search view
- [x] Viewport is split vertically: query bar on top (1–2 lines), task list filling the rest
- [x] Query bar shows the active query DSL expression and is editable (press `/` to focus it, `Enter` to apply, `Esc` to cancel edit and revert) [`q` dropped — collides with the global quit keybinding]
- [x] Default query on launch: tasks that are overdue or due within the next 7 days, sorted by due date ascending then priority descending
- [x] Task list has a visual divider between the overdue section and the upcoming section (e.g. a labelled separator row: `── overdue ──` above, `── upcoming ──` below); if one section is empty the divider for that section is omitted
- [x] Each task row displays: priority indicator, description, due date, scheduled date — all in a compact single line; use color or symbols to distinguish priority levels and flag overdue dates
- [x] Move selection up/down with `↑`/`↓` or `j`/`k`; selection wraps at list boundaries
- [x] `R` reloads all task data from disk and re-applies the current query

### Tasks tab — quick keybindings (selected task)
- [ ] `]` moves the due date forward one day; `[` moves it back one day
- [ ] `}` moves the scheduled date forward one day; `{` moves it back one day
- [ ] `p` cycles priority up (none → low → medium → high); `P` cycles down
- [ ] `x` completes the selected task (handles recurrence per plan 001 rules)
- [ ] `X` cancels the selected task
- [ ] All quick mutations write atomically via the `ft-core` atomic write helper and refresh the row in place

### Tasks tab — edit popup
- [ ] `e` opens a modal form for the selected task with fields: description, due date, scheduled date, priority, tags, recurrence
- [ ] Date fields accept ISO, relative, and natural-language input (same parser as the CLI)
- [ ] `Esc` cancels with no write; `Ctrl+S` submits and writes atomically
- [ ] On submit, the task row in the list updates in place without a full reload

### Tasks tab — editor handoff
- [ ] `Enter` on a selected task suspends the TUI (disables raw mode, leaves alternate screen), opens the source file in `$EDITOR` at the correct line, then restores the TUI and forces a full refresh of the current view on return

### Performance
- [ ] First render of the task list under 500ms on a 5k-note vault (same scan path as the CLI)
- [ ] Query edits and navigation remain responsive under 50ms per keystroke (in-memory filter, no re-scan)
- [ ] Memory ceiling: under 200MB for the 5k-note vault baseline

### Testing
- [ ] Unit tests on the tab framework's event dispatch and state machine
- [ ] Snapshot tests for rendered frames using `ratatui`'s `TestBackend` — at minimum: Welcome tab, empty task list, populated task list with overdue divider, edit popup open, help overlay
- [ ] `cargo test` passes with no warnings

## Technical Notes

### Library boundaries
The TUI crate depends only on `ft-core`. It does not call `ft` (the binary)
internally. Anything the TUI needs that `ft-core` does not yet expose gets added
to `ft-core` first, so the CLI benefits too.

### Architecture
A single `App` struct holds the tab list, current tab index, and global state
(vault handle, config). Events from crossterm are translated to a typed `Event`
enum and dispatched to the focused tab via `handle_event`. `TabCtx` carries the
vault handle, config, and status-bar setters.

The left sidebar's view dropdown is internal state of the Tasks tab, not a
top-level concern. The Tasks tab owns a `Vec<Box<dyn View>>` and delegates
rendering and event handling to the active view.

### Editor handoff
`disable_raw_mode()` + `LeaveAlternateScreen`, spawn `$EDITOR` via
`std::process::Command::new(...).status()`, then `enable_raw_mode()` +
`EnterAlternateScreen` + force `refresh()` on the current tab. Same primitive
as `ft tasks create --edit` but wrapped in suspend/restore.

### Date/time display
The sidebar clock updates on a 1-second tick event injected into the event
loop alongside crossterm events (use a background thread or `tokio::time` tick
sending into a channel). The task list itself does not re-render on each tick —
only the sidebar clock cell redraws.

### Out of scope for v1
- Mouse support
- Configurable keybindings
- Light mode / `--theme` flag
- Move dialog (`m`) and target file picker
- Multi-select and bulk operations
- Group-by cycle (`g`)
- Undo (`u`)
- Staleness detection and auto-refresh
- inotify / FSEvents watcher
- Additional views beyond "Search" (e.g. Board, Calendar)
- Notes tab (plan 003)
- `docs/tui.md` reference and manual test checklist

## Sessions
### Session 1 · 2026-05-10 · done
**Goal:** TUI foundation: ft tui subcommand, event loop, Tab framework, top tab bar, status bar, Welcome tab with ASCII art, exit/switch/help keybindings
**Outcome:** Added `ratatui 0.29` + `crossterm 0.28` to the workspace and wired
the `ft tui` subcommand. New module tree under `ft/src/tui/`: `app.rs` (App
struct, event loop, global keymap), `event.rs` (typed Event enum + channel-
backed `EventStream` with 1s tick), `tab.rs` (Tab trait with `title`,
`on_focus`, `on_blur`, `handle_event`, `render`, `refresh`; `EventOutcome` with
`Consumed`/`NotHandled`/`SwitchTab(idx)`/`Quit`; `TabCtx` carrying the vault),
`ui.rs` (top tab bar, three-cell status bar, centered help overlay with
`Clear`), and `tabs/{welcome,tasks}.rs`. Welcome tab shows a cyan block-letter
"WELCOME" banner with vault name and any-key forwards to the Tasks tab.
Tasks tab is a placeholder for session 2. Global keys: `q`/`Ctrl+C` quit, `?`
toggles help, `Tab`/`Shift+Tab` cycle tabs, `1`/`2` jump by index. Help overlay
swallows all keys except its own dismiss keys. Reserved API surface (`Quit`,
`Consumed`, `refresh`, `last_refresh`) is annotated with `#[allow(dead_code)]`
to keep the build warning-free without removing the contract. 8 tests added:
3 snapshot tests via `TestBackend` + `insta` (welcome 80x24, help overlay,
tasks placeholder) plus 5 behavioural tests for the global keymap and tab
switching. `cargo test --workspace`, `cargo clippy --workspace --all-targets`,
and `cargo fmt --all -- --check` all clean.

### Session 2 · 2026-05-10 · done
**Goal:** Tasks tab skeleton: sidebar layout with live clock and view dropdown, viewport split, stub Search view, inner view abstraction
**Outcome:** Promoted `tabs/tasks.rs` to a `tabs/tasks/` module folder. Defined
the inner `View` trait (`title`, `render`, `handle_event`, `on_focus`,
`refresh`) so the Tasks tab can own a `Vec<Box<dyn View>>`. `TasksTab` now
renders a horizontal split: a 24-char sidebar block (date `%a %d %b`, clock
`%H:%M:%S`, "── views ──" header, dropdown with a ▶ marker on the active
entry) and a viewport that delegates to the active view. Sidebar dropdown
is driven by `↑`/`↓` (wrap-around) and `Enter` (consumed; no-op until a
second view exists). All other keys forward to the active view; if it
returns `NotHandled`, the App's global keymap still applies. The Tick event
already triggers a redraw on the next loop iteration, so the clock updates
once per second without per-cell tracking. Clock is injected via a
`ClockFn = fn() -> DateTime<Local>` field — production uses `Local::now`,
tests pass a fixed 2026-05-10 14:32:05 closure for deterministic snapshots.
Added `App::for_test_with_clock` to wire the test clock through. Stub
`SearchView` shows a bordered `query` bar and a `tasks` placeholder; the
real query DSL, lazy load, and overdue/upcoming divider land in session 3.
Replaced the old `tasks_placeholder_80x24` snapshot with a richer
`tasks_tab_80x24` snapshot covering the full sidebar + viewport layout, and
added behavioural tests that `↑`/`↓` and `Enter` are consumed by the
Tasks tab without panic. 10 tui tests pass; full workspace `cargo test`
(345 tests), `cargo clippy --workspace --all-targets`, and
`cargo fmt --check` all clean.

### Session 3 · 2026-05-10 · done
**Goal:** Search view: lazy task load, default overdue+upcoming query, row rendering with priority/due/scheduled, overdue/upcoming divider, navigation, editable query bar, R to reload
**Outcome:** Replaced the SearchView stub with a full implementation. State:
loaded `Vec<Task>`, sorted+filtered `matches: Vec<usize>`, `overdue_count`,
selection cursor, scroll offset, `query_text` plus a `parse_state` cache, and
an optional `EditBuffer` with character-level cursor + horizontal scroll for
the bar. On first focus the view scans the vault and bumps `last_refresh`;
`R` (Shift+r) re-scans. Default query is `not done and due before <today+8d>
sort by due, priority reverse` (literal date so the bar is round-trippable
through the existing DSL parser). Rows render single-line with priority
label (`!!!`/`!!`/`!`/`v`/`vv`), description (truncated to 22 cols at 80x24),
due date, scheduled date — overdue dates in red, scheduled in cyan, selected
row gets a `▶` cursor and a darker bg. Section dividers (`── overdue (N) ──`,
`── upcoming (N) ──`) appear only when their section has entries. Navigation:
`↑`/`↓`/`j`/`k` wrap the selection; `/` opens the editor; `Enter` applies and
re-runs filter+sort; `Esc` cancels with no write. Long queries scroll
horizontally so the cursor stays visible. Parse errors short-circuit the
list area with a visible error message. To unblock this:

1. Refactored `TabCtx` — added `today: NaiveDate` (resolved from `FT_TODAY`
   or `Local::now`) and switched `last_refresh` to `&Cell<Option<DateTime>>`
   so views can write through `&TabCtx` and the App reads back when drawing
   the status bar. The `now` for the timestamp is `Local::now()` at write
   time; tests redact it via an `insta::with_settings!{filters}` helper.
2. Reversed Tasks-tab key precedence: the active view gets first dibs on
   `↑`/`↓`/`Enter`; the sidebar dropdown only handles them if the view
   returns `NotHandled` (so the Search list owns its own selection without
   colliding with the dropdown).
3. Resolved a contradiction in the plan (`q` listed as both quit and
   edit-mode trigger) — kept `q` as the global quit key and dropped the
   `q`-to-edit binding; `/` alone (vi/less convention) opens the bar.
4. Added the `filters` feature to the `insta` workspace dep so snapshots
   can redact the wall-clock timestamp.

7 new tests (5 new behavioural + 2 new snapshots): empty-vault, populated
vault with overdue/upcoming divider, edit-mode rendering, parse-error
rendering, arrow-key wrap, query-apply filter, Esc cancels edit, `R` picks
up disk changes. 17 tui tests pass; full workspace `cargo test` (350+
tests), `cargo clippy --workspace --all-targets`, and `cargo fmt --check`
all clean.

### Session 4 · 2026-05-10 · planned
**Goal:** Quick keybindings: []{}p P x X for date nudges, priority cycle, complete (with recurrence), cancel; atomic writes and in-place row updates
**Outcome:** 

### Session 5 · 2026-05-10 · planned
**Goal:** Edit popup (e) with all task fields and CLI date parser; Enter editor handoff with TUI suspend/restore and forced refresh
**Outcome:** 

### Session 6 · 2026-05-10 · planned
**Goal:** Performance budgets on 5k-note fixture, fill remaining snapshot tests, help overlay audit, no-warnings cleanup, real-vault smoke check
**Outcome:** 
