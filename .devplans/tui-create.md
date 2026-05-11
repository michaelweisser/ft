---
id: 004
name: tui-create
title: Create tasks from the TUI
status: finished
created: 2026-05-10
updated: 2026-05-11
---

# Create tasks from the TUI

## Goal
Add task creation to the `ft tui` Tasks tab so the daily workflow no longer
has to bounce out to the CLI for new entries. The primary surface is a
**quickline** — a single text input on top of the Search view that accepts
prefix tokens (`due:+2d`, `pri:high`, `#errands`, `in:Inbox.md`, `every week`)
so the user never has to type the Obsidian-Tasks emoji glyphs by hand. A live
preview shows the parsed task as it would land on disk. `Ctrl+E` from the
quickline expands to the same modal form as the edit popup, pre-populated
from the parsed tokens, so any field that the quickline doesn't comfortably
cover is still reachable.

## Motivation and Context
Plan 002 closed out with full read/edit/triage support but no way to create
tasks; users have to drop to `ft tasks create ...` in another terminal, which
breaks flow during a triage session and means the most common capture path
(quick "remember this") isn't covered.

Two pieces of user feedback shaped this plan:
- The emoji syntax (📅 ⏫ ⏳ 🔁) is great on disk but painful to type — keys
  are awkward to reach on most keyboards and Obsidian's autocomplete isn't
  available inside the TUI. So the input language has to be ASCII tokens
  (`due:`, `pri:`, `every`, `#tag`).
- Capture should be fast (a `c` keypress, type one line, Enter), but power
  users need to set advanced fields (recurrence, alternate target file,
  scheduled vs due) without leaving the TUI. `Ctrl+E expand to form` covers
  that without slowing the common case.

The target-file question came up in scoping: the CLI defaults to today's
daily note, which is the right default for journal-style users, but the
quickline gains an `in:PATH` token so per-task overrides don't require
opening the full form. The expanded form has a dedicated `target` field.

## Acceptance Criteria

### Quickline UX
- [x] Pressing `c` from the Search view opens a 3-row "new task" panel
      between the query bar and the task list (bordered) plus a 1-line
      preview directly underneath (4 rows total)
- [x] The cursor lands in the quickline input and starts blank
- [x] `Esc` cancels with no write; the panel closes and the cursor returns
      to wherever it was in the task list
- [x] `Enter` writes the task and closes the panel
- [x] `Ctrl+E` opens the full edit-style popup pre-populated from the
      parsed quickline state, transferring focus there; the quickline panel
      closes on popup open
- [x] The query bar, list navigation, and global keymap are all suppressed
      while the quickline is open (panel swallows keys like the popup does)
- [x] Backspace, arrow keys, Home/End, Ctrl+W / Ctrl+Backspace / Alt+Backspace
      (delete word) all work in the input — reuses `EditBuffer`

### Quickline token grammar
The parser walks the input left-to-right, peels off any token that matches
one of the prefix patterns below, and concatenates the rest as the
description (in original order). Tokens with invalid values surface as a
parse error in the preview line and block Enter; the user can fix in place
or fall through to the form.

- [x] `due:VALUE` — due date (ISO, `+Nd`/`-Nw`/etc., `today`/`tomorrow`,
      natural language). Uses `ft_core::dates::parse`.
- [x] `sched:VALUE` — scheduled date. Same parser.
- [x] `start:VALUE` — start date. Same parser.
- [x] `pri:VALUE` — priority (`none`, `low`, `med`, `medium`, `high`,
      `highest`, `lowest`). Matches the popup's `parse_priority` helper.
- [x] `every WORDS...` — recurrence rule, consumes the rest of the line up
      to the next prefix token. Preserved verbatim (Obsidian Tasks plugin
      compatible).
- [x] `in:PATH` — target file path, relative to vault root (e.g.
      `in:Inbox.md`, `in:Daily/2026-05-10.md`)
- [x] `#tag` (anywhere in the line) — adds a tag; the literal `#tag` stays
      in the description (matches how the CLI's `--tag` flag merges)
- [x] `id:WORD` — stable identifier (the 🆔 field)
- [x] Tokens are case-insensitive on the prefix; values are passed through
      verbatim to the matching parser
- [x] Backslash-escaped tokens (`\due:foo`) are treated as literal
      description text — escape hatch for descriptions that legitimately
      contain `key:value` shapes

### Quickline preview
- [x] A second row under the input renders the parsed result as the
      canonical emoji-format line (uses `ops::build_task` +
      `EmojiFormat::serialize_line` so the preview matches what
      `create_task` would actually write)
- [x] Unparseable input shows the parser error in red, prefixed with `⚠`,
      in place of the preview; Enter is suppressed while in this state
- [x] An empty input shows a dim hint `Enter to save · Esc to cancel`
      under a `type a task — e.g. "email Sarah due:tomorrow pri:high
      #work"` placeholder in the input row itself
- [x] The target file appears in dim text at the right of the preview
      (e.g. `→ Inbox.md`) so it's visible without scrolling

### Default target
- [x] No `in:` token → today's daily note, resolved the same way as the
      CLI's `resolve_target_path` (via the shared
      `Vault::resolve_target` from session 1)
- [x] `in:PATH` overrides for that one task; the value is taken verbatim
      and resolved against the vault root
- [ ] Absolute paths are accepted but must be inside the vault — reject
      otherwise with a parse error *(deferred: currently absolute paths
      pass through unchecked; safety check belongs in
      `Vault::resolve_target` and applies to the CLI too)*
- [x] The target file is created if it doesn't exist (matches
      `ops::create_task` semantics for daily notes)

### Expanded popup form (Ctrl+E)
- [x] Opens the existing edit popup component (or a near-clone) with title
      `new task`
- [x] Six existing fields prefilled from the quickline parse:
      description, due, scheduled, priority, tags, recurrence
- [x] One new `target` field showing the target path. Plain text input
      that supports `Path` and `Path#heading text`; the `#heading`
      suffix translates to `Position::UnderHeading(heading)` on submit
      so users can seed a task into a specific section.
- [ ] **Picker UI deferred.** Plan 005's `FuzzyPicker` is shipped, but
      wiring its `&Vault`-borrowing source into a long-lived popup
      state runs into a self-borrow on `App`. v1 keeps the target
      field as a plain text input; a follow-up will introduce the
      picker (likely via `Arc<Vault>` on App, or a context-on-query
      variant of `PickerSource`).
- [x] Tab order: description → target → due → scheduled → priority → tags
      → recurrence → back to description
- [x] `Ctrl+S` validates and writes; `Esc` cancels
- [x] Validation errors keep the popup open and focus the offending field,
      same UX as the edit popup
- [x] The popup also opens directly without the quickline if the user
      presses `Shift+C` from the Search view

### Write + post-create UX
- [x] On Enter (quickline) or Ctrl+S (popup): build a `CreateInput`,
      resolve the target path, call `ops::create_task` with
      `CreateOptions { position: Append, force: false }`
- [x] Duplicate-detection error surfaces inline (`⚠ duplicate exists at
      Inbox.md:42`) and keeps the panel open so the user can adjust
      *(Ctrl+Enter force-insert is deferred per the plan)*
- [x] After a successful write the Search view re-scans and the cursor
      moves to the new task's row if it passes the active filter; if it
      doesn't, the cursor returns to where it sat before the write
- [x] A toast in the status bar's center cell shows `created PATH:LINE`
      for ~3 seconds (green for success, red for IO error)
- [x] Toast styling: green for success, red for error (the error toast
      shows after a write fails for IO reasons; duplicate detection
      stays inline because it's recoverable)

### Help overlay
- [x] `c` — open quick-create *(rendered as `c / Shift+C — new task (line
      / form)` to keep the row count under the 80x24 budget)*
- [x] `Shift+C` — open create popup directly *(same combined row as above)*
- [x] `Ctrl+E` (in quickline) — expand to popup
- [x] All other quickline / popup keys covered by the existing entries
      (Tab, Ctrl+S, Esc)

### Testing
- [x] Unit tests for the quickline parser: every token in isolation, two
      tokens at once, escaped tokens, ordering, unicode in description,
      empty input, only-tokens input (no description), invalid date, invalid
      priority
- [x] Behavioral test: press `c`, type a quickline, Enter → file on disk
      contains the expected serialized line in the expected target
- [x] Behavioral test: `in:Custom.md` writes to the override path; default
      writes to today's daily note
- [x] Behavioral test: invalid date in quickline keeps the panel open with
      the error preview and disk is unchanged
- [x] Behavioral test: duplicate write surfaces inline error, no second
      write happens
- [x] Behavioral test: `c` → `Ctrl+E` opens popup with all parsed fields
      pre-populated; submitting writes the same task
- [x] Behavioral test: after create, cursor anchors to the new task if it
      matches the active filter, otherwise stays put
- [x] Snapshot test: empty quickline (hint visible)
- [x] Snapshot test: quickline with valid preview
- [x] Snapshot test: quickline with parse error
- [x] Snapshot test: expanded popup with prefilled values

## Technical Notes

### Library boundaries
Mirrors plan 002 — the TUI crate depends only on `ft-core`. The quickline
parser lives in `ft/src/tui/tabs/tasks/quickline.rs` because it's
TUI-specific syntax, not part of the public task model. The actual write
goes through `ft_core::task::ops::create_task` — same code path as the CLI's
`ft tasks create`.

### Parser shape
```rust
struct QuicklineParse {
    description: String,
    due: Option<NaiveDate>,
    scheduled: Option<NaiveDate>,
    start: Option<NaiveDate>,
    priority: Option<Priority>,
    tags: Vec<String>,
    recurrence: Option<String>,
    id: Option<String>,
    target: Option<PathBuf>, // None = default to daily note
    errors: Vec<String>,
}

fn parse_quickline(input: &str, today: NaiveDate) -> QuicklineParse;
```
The parser collects errors instead of short-circuiting so the preview can
show the first error while the user keeps typing.

### AppRequest extension
The toast and "auto-select new task" both need to happen after the write
returns. Re-use the `AppRequest` enum (introduced in plan 002 session 5
for the editor handoff) so the view raises a `TaskCreated { path, line }`
event that the App services: refresh the active tab, then push a toast
message into a `Cell<Option<Toast>>` field on `App`, with a tick-based
expiry.

### Toast plumbing
A `Toast` struct holds `text`, `style`, and a `deadline: Instant`. The
status bar's center cell renders the toast in place of the refresh time
when one is active. The 1s tick event already drives the redraw loop, so
no extra threading is needed; the cell just checks `deadline > now` on
each render. Toasts expire by getting `take()`-ed when expired.

### Daily-note resolver reuse
Pull `resolve_target_path` out of `ft/src/cmd/tasks.rs` into
`ft_core::vault::daily_note(today)` (or similar). Both the CLI and TUI
then share one source of truth for "today's daily note" resolution.

### `in:` path safety
Reject `in:` values that resolve outside the vault root (use
`Path::canonicalize` and compare prefix). Without this, a user typing
`in:../../etc/passwd` could escape — unlikely in practice, but cheap to
guard.

### Out of scope for v1
- Picker autocomplete inside the *quickline* `in:` token — quickline
  takes literal paths only; users who want fuzzy picking use the popup
  (`Shift+C` or `Ctrl+E`). Keeps the quickline parser pure-text.
- Autocomplete for tags / priorities / dates
- Token highlighting in the input as the user types (the preview row
  already shows the parsed structure)
- Multi-task batch create (one line, multiple tasks)
- Template support (`c t` → pre-fill with a saved template)
- Smart "remind me" natural language across the whole input (e.g. "buy
  milk tomorrow at 5pm" auto-extracting "tomorrow")
- Inserting at a position other than `Append` — except via the popup's
  fuzzy-picker heading hit, which translates to
  `Position::UnderHeading(heading.text)`. The CLI's `--at-line` flag has
  no TUI equivalent in v1.
- Force-insert duplicate via `Ctrl+Enter` — duplicate detection blocks
  with an inline error and the user has to clear/edit; force is deferred
  to keep v1 simple
- Subtask creation (creating a task as a child of the selected row with
  proper indent)
- Voice input or any non-keyboard surface

## Sessions

### Session 1 · 2026-05-10 · done
**Goal:** Pull resolve_target_path out of cmd/tasks.rs into ft_core::vault::daily_note. Write the quickline parser (every token, escapes, ordering, errors) with full unit-test coverage.
**Outcome:** Two parallel pieces landed.

1. **Shared target resolver.** Added `Vault::resolve_target(today,
   file_override) -> Result<PathBuf, DailyError>` on `ft_core::vault`.
   Logic: if the override is set, return it (joined against the vault
   root when relative); otherwise resolve today's daily note via the
   existing `daily::resolve_daily_path`. The CLI's
   `cmd/tasks.rs::resolve_target_path` collapsed into a 3-line wrapper
   that just delegates and maps `DailyError → anyhow::Error`. The CLI's
   `daily` import dropped out as a side effect; the new wrapper goes
   through the inherent method instead. All existing tests still pass —
   `resolve_target` keeps the exact same semantics, just hoisted so the
   TUI quickline (and any future surface) can share it.

2. **Quickline parser.** New module
   `ft/src/tui/tabs/tasks/quickline.rs` shipping
   `QuicklineParse { description, due, scheduled, start, priority,
   tags, recurrence, id, target, errors }` plus
   `parse_quickline(input, today)`.

   Token grammar implemented (case-insensitive on prefixes; values
   verbatim):
   - `due:VAL` / `sched:VAL` / `start:VAL` — date fields via
     `ft_core::dates::parse`
   - `pri:VAL` — priority with the same vocabulary as the edit popup
     (`none`/`low`/`med`/`medium`/`high`/`highest`/`lowest`)
   - `in:PATH` — vault-relative or absolute target path (caller does
     the inside-vault validation)
   - `id:WORD` — stable identifier
   - `#tag` — alphanumeric + `_-/`; populates `tags` AND stays inline
     in the description so the final markdown carries the hashtag
   - `every WORDS...` — recurrence; greedily consumes tokens until end
     of input or the next prefix-bearing token (so
     `every weekday due:tomorrow` parses correctly)
   - `\due:foo` / `\every-day` — backslash escape leaves the rest as
     literal description text

   Errors accumulate in `errors: Vec<String>` rather than short-
   circuiting, so the UI can keep showing a live preview as the user
   types. Double assignments (`due:X due:Y`) keep the first value and
   record an error; unknown `key:value` tokens (e.g. `re:invoice`) stay
   in the description untouched. Word order is preserved across the
   parse — `send the budget review #finance to Alice` round-trips
   intact even with mixed-in tokens.

   30 unit tests cover: every token in isolation; mixed token + prose;
   ordering preservation; unicode descriptions (the parser previously
   panicked on Cyrillic tokens whose byte length aligned with `due:` —
   fixed by switching `strip_prefix_ci` to `str::get`); escapes;
   `every` consumption boundaries; double-assign error; empty-value
   error; case-insensitive prefix matching; tag dedup; bare `#`
   doesn't become a tag.

Module-scope `#![allow(dead_code)]` keeps the parser warning-free
until session 2 wires it into the UI. `cargo test --workspace` (456
tests) passes; clippy `-D warnings` and fmt clean.

### Session 2 · 2026-05-10 · done
**Goal:** Quickline UI: open with c, 3-row panel (input + preview row + hint/error), EditBuffer reuse for input, live preview rendering with target path, Enter writes via ops::create_task, basic refresh on success, inline duplicate-detection error.
**Outcome:** End-to-end quickline create flow works.

**SearchView state.** Added a `quickline: Option<Quickline>` field
holding an `EditBuffer` + `Option<String>` for post-submit errors. `c`
from normal mode opens it; the panel takes precedence over the query
bar's edit state in `handle_event`, swallowing every key. Esc clears
state and returns focus to the list.

**Layout.** When the quickline is open, the search view's vertical
layout switches from `[3 query, Min 1 list]` to `[3 query, 4 quickline,
Min 1 list]`. The quickline area is itself `[3 input panel, 1
preview]`. Cursor renders as a yellow `│`; placeholder text is the
hint `type a task — e.g. "email Sarah due:tomorrow pri:high #work"`.

**Preview.** `build_quickline_preview` re-parses on every render via
`parse_quickline` (cheap), then builds a Task through `ops::build_task`
and serializes via `EmojiFormat::serialize_line` so the preview is
byte-identical to what `create_task` will write. Three states: post-
submit error → red `⚠ <msg>`; empty input → dim hint; valid parse →
`→ <serialized line>   → <target>` with the target rendered relative
to the vault root.

To make this work, `ops::build_task` was promoted from private to
`pub` in `ft-core::task::ops` with a doc comment explaining the
preview-vs-write parity guarantee.

**Write flow.** Enter:
1. Re-parse the input.
2. Block if `errors` is non-empty (parse error → red banner).
3. Block on an empty description (separate error: "description is
   empty"; tags-only input doesn't count as a valid task).
4. Resolve the target via `Vault::resolve_target(today,
   parse.target.as_deref())` — the shared resolver from session 1.
5. Build `CreateInput` from the parse and call `ops::create_task` with
   `Position::Append, force: false`.
6. Success → close panel, `reload(ctx)` so the new row is in the
   matches list. (Cursor anchor + green toast = session 3.)
7. `CreateError::Duplicate { path, line }` → keep panel open with
   `⚠ duplicate exists at <relpath>:<line>` inline.
8. Any other error → same inline display; nothing lands on disk.

**Help overlay.** New row: `c — new task (quickline)`. The help
binding count grew past what an 80%-height popup could contain at
80x24, so `render_help_overlay` now uses 90% height. Snapshot updates
for `help_overlay_80x24` and `help_overlay_over_tasks_80x24` accepted.

**Tests (7 new in tui::tests, plus updated help label list).**
- `quickline_opens_with_c_and_closes_on_esc` — open/close lifecycle
- `quickline_enter_writes_to_daily_note` — full write path against an
  explicit `[daily_notes]` config that resolves to `Daily/2026-05-10.md`
  (caveat documented inline: moment.js translates bare letters, so the
  config uses `path = "[Daily]"` to keep the literal folder name)
- `quickline_in_path_overrides_target` — `in:Inbox.md` redirects the
  write
- `quickline_parse_error_blocks_write` — `due:not-a-date` shows ⚠ and
  leaves disk untouched
- `quickline_duplicate_detection_surfaces_inline` — second create of
  the same task hits `CreateError::Duplicate` and surfaces inline
- `quickline_empty_description_blocks_write` — tokens-only input
  rejected with "description is empty"
- `quickline_ctrl_w_works_in_input` — EditBuffer's word-delete is
  hooked up (input row asserts target the panel's input line, not
  the surrounding chrome)

99 tui tests pass; workspace `cargo test` (490 tests), clippy
`-D warnings`, and `cargo fmt --check` all clean.

### Session 3 · 2026-05-10 · done
**Goal:** Post-create UX polish: Toast struct + status-bar cell rendering, AppRequest::TaskCreated, cursor-anchor-to-new-task when it matches the active filter, IO-error red toast, tick-based 3s toast expiry.
**Outcome:** Post-create UX polish landed.

**Toast plumbing.** New types in `tab.rs`:
- `AppRequest::Toast { text, style }` — generic toast request (used by
  the quickline now; future surfaces can fire toasts the same way).
- `ToastStyle::{Success, Error}` — green / red.

And in `app.rs`:
- `Toast { text, style, deadline: Instant }` — owned data + expiry.
- `App.toast: RefCell<Option<Toast>>` — single slot; new toasts
  overwrite the old (no queue; the most recent message is the one the
  user wants to see).
- `service_request` services `Toast` by setting the slot with `now +
  TOAST_DURATION` (3 s).
- `draw` expires stale toasts before rendering so the status-bar cell
  flips back to `refreshed HH:MM:SS` on the very tick the deadline
  passes — the existing 1-second tick is the only timer needed.

`render_status_bar` now takes `Option<&Toast>` and the center cell
shows the toast text in green or red (bold) when active, falling back
to the refresh timestamp otherwise.

**Naming deviation from the plan.** The plan sketched
`AppRequest::TaskCreated { path, line }` and had the App orchestrate
the post-create work. I went with a more general
`AppRequest::Toast { text, style }` instead because (a) the cursor
anchor is search-view-specific knowledge that fits better inside the
view than the App, and (b) future surfaces (rename, move, etc.) can
fire toasts without each one adding a new AppRequest variant.
SearchView does its own anchor + reload before firing the toast.

**Cursor anchor.** New `SearchView::refresh_and_anchor_to_create`:
- Captures the prior selection's (path, line) before the write
- Reloads, then tries the *new task's* (path, line) first
- Falls back to the prior anchor if the new task didn't pass the
  active filter
- Falls through to `selected = 0` when neither anchor matches,
  saturating to the last row when the list shrank

`submit_quickline` builds the relative target path from the resolved
absolute path (so the toast and anchor agree with how rows are keyed
in the matches list).

**IO-error vs duplicate.** Duplicate stays inline (panel stays open,
user retypes); any other `CreateError` closes the panel and surfaces
as a red toast — those errors aren't fixable from inside the
quickline so blocking the user there would just trap them.

**Test helpers.** Added to `App`:
- `service_pending_for_test()` — services queued `Toast` requests
  without spinning the real event loop (other variants are re-queued
  so editor-handoff tests still work)
- `current_toast()` — clones the active toast for assertions

**Tests (4 new in tui::tests, plus 1 match-arm update in the
editor-handoff test).**
- `quickline_success_raises_green_toast_request` — submit → toast
  text starts with `created ` and style is `Success`
- `quickline_success_renders_toast_in_status_bar_center_cell` —
  status-bar row contains the toast text
- `quickline_success_anchors_cursor_to_new_task_when_it_matches_filter`
  — the new task's list row carries `▶` after the create
- `quickline_duplicate_does_not_raise_toast` — duplicate stays inline,
  toast slot remains empty

The pre-existing `enter_on_search_view_queues_editor_open_request`
test got a new `AppRequest::Toast { .. }` match arm that panics so
the exhaustive match keeps working.

103 tui tests; full workspace `cargo test` (494 tests); clippy
`-D warnings` + fmt --check all clean.

### Session 4 · 2026-05-10 · done
**Goal:** Expanded popup: Shift+C opens the popup directly, Ctrl+E from quickline expands with parsed fields pre-populated, new target field with vault-root path validation, refactor edit popup to share render path between new/edit modes.
**Outcome:** Expanded popup now serves both edit and new flows.

**Popup refactor.** `EditPopup` gained:
- `target: EditBuffer` — only rendered in `New` mode
- `mode: PopupMode { Edit, New }` — drives the title (`edit task` vs
  `new task`) and the field traversal order
- `fields() -> &'static [EditField]` — single source of truth for
  the ordered field list per mode. `Edit` skips `target`; `New`
  weaves it in right after `description`. Removed the old hard-coded
  `next()`/`prev()` matches in favor of `next_field()` /
  `prev_field()` that index into `fields()`.
- Constructors: `from_task` (existing, sets `mode = Edit`),
  `new_blank` (Shift+C: empty `New` popup),
  `from_quickline(parse)` (Ctrl+E: pre-fills every field from a
  `QuicklineParse`).

The renderer reads `popup.fields()` to lay out exactly the rows that
belong in the current mode, so the rendered popup naturally has the
extra `target` row for New and stays compact for Edit.

**Submit path split.** `submit_popup` validates once and dispatches:
- `submit_popup_edit` — the existing `update_task_line` path
- `submit_popup_new` — parses the target field (treating `#heading`
  via `ft_core::search::Query::parse` so the user can target a
  section), resolves through `Vault::resolve_target` for the
  daily-note default, builds a `CreateInput`, and calls
  `ops::create_task`. `Position::UnderHeading(heading)` when a
  heading suffix is supplied; `Position::Append` otherwise.
- Captures the prior selection's `(path, line)` so the cursor anchor
  falls back gracefully when the new task doesn't pass the filter.
- Fires the same green/red toast and cursor-anchor logic as the
  quickline submit path (refactored to share `refresh_and_anchor_to_create`).

**Keybindings.**
- `Shift+C` in search-view normal mode → opens a blank `New` popup
  (skip the quickline for users who already know they want the full
  form).
- `Ctrl+E` inside the quickline → parses what's typed, opens a
  pre-populated `New` popup, closes the quickline. Lets users start
  fast in the quickline and fall through to the form when they want
  to tweak something the quickline doesn't comfortably express.

**Help overlay** gains `c / Shift+C — new task (line / form)` and
`Ctrl+E — expand quickline → form`. Original labels overflowed the
26-col description budget at 80x24, so both got tightened. Help
snapshots refreshed; expected-label list in the audit test updated.

**Picker integration deferred.** Plan 005 shipped the `FuzzyPicker`
widget + `VaultFilePickerSource`, but wiring its `&'v Vault`-
borrowing source into a popup state that lives across event-loop
iterations runs into a self-borrow on App (`App.vault` and
`App.popup.picker.source: &'v Vault` both held simultaneously). The
cleanest fix is either `Arc<Vault>` on App or a context-on-query
variant of `PickerSource` — both bigger changes than this session's
scope. The target field accepts `Path#heading` directly via text
input, so all functional capability is present; the picker is purely
ergonomic and lands in a follow-up.

**Tests (5 new + 1 regression + 1 snapshot).**
- `shift_c_opens_blank_new_task_popup` — title + target field present
- `ctrl_e_in_quickline_opens_popup_with_pre_populated_fields` —
  every parsed field appears in the popup after Ctrl+E
- `new_popup_ctrl_s_writes_to_in_target` — full Tab-through-form +
  Ctrl+S writes to the override target
- `new_popup_target_with_heading_uses_under_heading_position` —
  `Inbox.md#Triage` writes inside the Triage section, before Done
- `new_popup_empty_description_blocks_write` — validation traps the
  empty-description case
- `edit_popup_still_works_after_refactor` — regression on the
  existing `e`-on-task flow
- `new_popup_blank_80x24` snapshot — captures the 7-row form

110 tui tests pass; workspace `cargo test` (498 tests); clippy
`-D warnings` + fmt --check clean.

### Session 5 · 2026-05-11 · done
**Goal:** Polish & audit: help overlay rows (c / Shift+C / Ctrl+E), snapshot coverage (empty quickline, valid preview, parse error, expanded popup, toast in status bar), no-warnings cleanup, real-vault smoke run.
**Outcome:** Polish + audit pass — feature is shippable.

**Help overlay.** Sessions 2 and 4 already landed the rows
(`c / Shift+C — new task (line / form)` and `Ctrl+E — expand
quickline → form`). Confirmed via `help_overlay_80x24` snapshot.
Marked the four help-overlay acceptance items checked, noting that
`c` and `Shift+C` share one row to keep the binding list inside the
80x24 budget.

**Snapshot coverage.** Added five new snapshot tests in
`ft/src/tui/tests.rs`:
- `quickline_empty_80x24` — placeholder text in the input row + the
  dim `Enter to save · Esc to cancel` hint underneath
- `quickline_valid_preview_80x24` — input `buy milk due:tomorrow
  pri:high #grocery` renders the canonical
  `→ - [ ] buy milk #grocery ⏫ 📅 2026-05-11 → Daily/2…` preview
- `quickline_parse_error_80x24` — `draft due:not-a-date` shows
  `⚠ due: could not parse 'not-a-date' as a date …` in red
- `new_popup_prefilled_80x24` — Ctrl+E from a populated quickline
  pre-fills description / due / priority / tags in the expanded
  popup; the target row stays empty when no `in:` was typed
- `quickline_toast_success_80x24` — after Enter, the new task lands
  in the list with the cursor anchored to its row and the status
  bar's center cell shows `created Daily/2026-05-10`

Note: ratatui's 80-col status bar truncates the toast text to
`created Daily/2026-05-10` (no `:1` suffix); 120-col captures the
full path. The 80x24 snapshot is the agreed-on width for snapshots,
and the existing behavioural test
`quickline_success_renders_toast_in_status_bar_center_cell` already
asserts the full text at 120 cols, so leaving it at 80 here.

**No-warnings audit.** `cargo build --workspace`, `cargo clippy
--workspace --all-targets -- -D warnings`, `cargo fmt --check`,
`cargo test --workspace` all clean. Fmt picked up a one-line nit in
`tests.rs::target_picker_navigation_changes_selection` (a
single-line `KeyEvent::new(KeyCode::Down, …)` that had been spread
across three lines) — applied via `cargo fmt`.

**Real-vault smoke run.** The plan called for a manual smoke against
a real Obsidian vault; the upstream gate is `FT_REAL_VAULT_TESTS=1`
pointed at `/Users/cmw/git/fortytwo`, which doesn't exist on this
machine. Substituted a CLI smoke: spun up `/tmp/smoke-vault` with an
empty `.obsidian/` dir, ran `ft tasks list --allow-empty` (renders
empty table), then `ft tasks create "smoke task" --due 2026-05-20
--priority high --file Inbox.md` (writes
`- [ ] smoke task ⏫ 📅 2026-05-20`). This exercises the same shared
`Vault::resolve_target` path that the TUI quickline calls into, so
it validates the cross-surface contract from session 1. The
TUI-against-real-vault smoke is left as a manual step for the
plan-owner's machine.

**Test counts.** TUI: 117 → 122 (+5 snapshots). Full workspace:
498 → 503. Clippy / fmt / build clean.

Marked all remaining acceptance criteria done except for the two
explicitly-deferred items (absolute-path validation, picker UI in
the popup target field). Both have explanatory notes in the plan
already and are out-of-scope follow-ups.
