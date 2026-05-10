---
id: 004
name: tui-create
title: Create tasks from the TUI
status: ready
created: 2026-05-10
updated: 2026-05-10
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
- [ ] Pressing `c` from the Search view opens a 3-row "new task" panel
      between the query bar and the task list: a 1-line input plus a 1-line
      live preview underneath
- [ ] The cursor lands in the quickline input and starts blank
- [ ] `Esc` cancels with no write; the panel closes and the cursor returns
      to wherever it was in the task list
- [ ] `Enter` writes the task and closes the panel
- [ ] `Ctrl+E` opens the full edit-style popup pre-populated from the
      parsed quickline state, transferring focus there; the quickline panel
      closes on popup open
- [ ] The query bar, list navigation, and global keymap are all suppressed
      while the quickline is open (panel swallows keys like the popup does)
- [ ] Backspace, arrow keys, Home/End, Ctrl+W / Ctrl+Backspace / Alt+Backspace
      (delete word) all work in the input — reuses `EditBuffer`

### Quickline token grammar
The parser walks the input left-to-right, peels off any token that matches
one of the prefix patterns below, and concatenates the rest as the
description (in original order). Tokens with invalid values surface as a
parse error in the preview line and block Enter; the user can fix in place
or fall through to the form.

- [ ] `due:VALUE` — due date (ISO, `+Nd`/`-Nw`/etc., `today`/`tomorrow`,
      natural language). Uses `ft_core::dates::parse`.
- [ ] `sched:VALUE` — scheduled date. Same parser.
- [ ] `start:VALUE` — start date. Same parser.
- [ ] `pri:VALUE` — priority (`none`, `low`, `med`, `medium`, `high`,
      `highest`, `lowest`). Matches the popup's `parse_priority` helper.
- [ ] `every WORDS...` — recurrence rule, consumes the rest of the line up
      to the next prefix token. Preserved verbatim (Obsidian Tasks plugin
      compatible).
- [ ] `in:PATH` — target file path, relative to vault root (e.g.
      `in:Inbox.md`, `in:Daily/2026-05-10.md`)
- [ ] `#tag` (anywhere in the line) — adds a tag; the literal `#tag` stays
      in the description (matches how the CLI's `--tag` flag merges)
- [ ] `id:WORD` — stable identifier (the 🆔 field)
- [ ] Tokens are case-insensitive on the prefix; values are passed through
      verbatim to the matching parser
- [ ] Backslash-escaped tokens (`\due:foo`) are treated as literal
      description text — escape hatch for descriptions that legitimately
      contain `key:value` shapes

### Quickline preview
- [ ] A second row under the input renders the parsed result as the
      canonical emoji-format line (what `ops::create_task` would serialize)
- [ ] Unparseable input shows the parser error in red, prefixed with `⚠`,
      in place of the preview; Enter is suppressed while in this state
- [ ] An empty input shows dim hint text like `Type a task. e.g. "Email
      Sarah due:tomorrow pri:high #work"`
- [ ] The target file appears in dim text at the right of the preview
      (e.g. `→ Inbox.md`) so it's visible without scrolling

### Default target
- [ ] No `in:` token → today's daily note, resolved the same way as the
      CLI's `resolve_target_path` (vault config + today's date)
- [ ] `in:PATH` overrides for that one task; the value is taken verbatim
      and resolved against the vault root (absolute paths are accepted but
      must be inside the vault — reject otherwise with a parse error)
- [ ] The target file is created if it doesn't exist (matches
      `ops::create_task` semantics for daily notes)

### Expanded popup form (Ctrl+E)
- [ ] Opens the existing edit popup component (or a near-clone) with title
      `new task`
- [ ] Six existing fields prefilled from the quickline parse:
      description, due, scheduled, priority, tags, recurrence
- [ ] One new `target` field showing the resolved target path. Pressing
      `Tab`/`Enter` on the field, or typing into it, opens the
      `FuzzyPicker` from **plan 005** (`gen consid#Firs` style query —
      file part fuzzy-matches filenames, optional `#heading` part fuzzy-
      matches headings inside the chosen file). Selecting a result fills
      the field; if a heading is chosen the new task is created
      `Position::UnderHeading(heading.text)` instead of `Append`.
- [ ] **Depends on plan 005 sessions 1–3** (the `FuzzyPicker` widget +
      `Vault::fuzzy_find` API). If 005 isn't ready when this session
      starts, fall back to a plain text input and file a follow-up issue
      to swap it in.
- [ ] Tab order: description → target → due → scheduled → priority → tags
      → recurrence → back to description (target up near the top because
      it's the highest-impact field after description)
- [ ] `Ctrl+S` validates and writes; `Esc` cancels
- [ ] Validation errors keep the popup open and focus the offending field,
      same UX as the edit popup
- [ ] The popup also opens directly without the quickline if the user
      presses `Shift+C` from the Search view (skip the quickline for users
      who want the full form immediately)

### Write + post-create UX
- [ ] On Enter (quickline) or Ctrl+S (popup): build a `CreateInput`,
      resolve the target path, call `ops::create_task` with
      `CreateOptions { position: Append, force: false }`
- [ ] Duplicate-detection error surfaces inline (`⚠ duplicate exists at
      Inbox.md:42`) and keeps the panel/popup open so the user can adjust
      or `Ctrl+Enter`-force-insert
- [ ] After a successful write the Search view re-scans (same path as `R`)
      and the cursor moves to the new task's row if it passes the active
      filter; if it doesn't pass, the cursor stays put
- [ ] A toast in the status bar's center cell shows `created PATH:LINE`
      for ~3 seconds (replaces the `refreshed HH:MM:SS` text during the
      toast window)
- [ ] Toast styling: green for success, red for error (the error toast
      shows after a write fails for IO reasons; duplicate detection stays
      inline because it's recoverable)

### Help overlay
- [ ] `c` — open quick-create
- [ ] `Shift+C` — open create popup directly
- [ ] `Ctrl+E` (in quickline) — expand to popup
- [ ] All other quickline / popup keys covered by the existing entries
      (Tab, Ctrl+S, Esc)

### Testing
- [ ] Unit tests for the quickline parser: every token in isolation, two
      tokens at once, escaped tokens, ordering, unicode in description,
      empty input, only-tokens input (no description), invalid date, invalid
      priority
- [ ] Behavioral test: press `c`, type a quickline, Enter → file on disk
      contains the expected serialized line in the expected target
- [ ] Behavioral test: `in:Custom.md` writes to the override path; default
      writes to today's daily note
- [ ] Behavioral test: invalid date in quickline keeps the panel open with
      the error preview and disk is unchanged
- [ ] Behavioral test: duplicate write surfaces inline error, no second
      write happens
- [ ] Behavioral test: `c` → `Ctrl+E` opens popup with all parsed fields
      pre-populated; submitting writes the same task
- [ ] Behavioral test: after create, cursor anchors to the new task if it
      matches the active filter, otherwise stays put
- [ ] Snapshot test: empty quickline (hint visible)
- [ ] Snapshot test: quickline with valid preview
- [ ] Snapshot test: quickline with parse error
- [ ] Snapshot test: expanded popup with prefilled values

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

### Session 1 · 2026-05-10 · planned
**Goal:** Pull resolve_target_path out of cmd/tasks.rs into ft_core::vault::daily_note. Write the quickline parser (every token, escapes, ordering, errors) with full unit-test coverage.
**Outcome:** 

### Session 2 · 2026-05-10 · planned
**Goal:** Quickline UI: open with c, 3-row panel (input + preview row + hint/error), EditBuffer reuse for input, live preview rendering with target path, Enter writes via ops::create_task, basic refresh on success, inline duplicate-detection error.
**Outcome:** 

### Session 3 · 2026-05-10 · planned
**Goal:** Post-create UX polish: Toast struct + status-bar cell rendering, AppRequest::TaskCreated, cursor-anchor-to-new-task when it matches the active filter, IO-error red toast, tick-based 3s toast expiry.
**Outcome:** 

### Session 4 · 2026-05-10 · planned
**Goal:** Expanded popup: Shift+C opens the popup directly, Ctrl+E from quickline expands with parsed fields pre-populated, new target field with vault-root path validation, refactor edit popup to share render path between new/edit modes.
**Outcome:** 

### Session 5 · 2026-05-10 · planned
**Goal:** Polish & audit: help overlay rows (c / Shift+C / Ctrl+E), snapshot coverage (empty quickline, valid preview, parse error, expanded popup, toast in status bar), no-warnings cleanup, real-vault smoke run.
**Outcome:** 

