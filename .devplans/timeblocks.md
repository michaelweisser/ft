---
id: 015
name: timeblocks
title: Timeblocks: ft_core library + ft timeblocks CLI + TUI tab
status: implementing
created: 2026-05-16
updated: 2026-05-16
---

# Timeblocks: ft_core library + ft timeblocks CLI + TUI tab

## Goal
Make managing the daily timeblocks (Obsidian Day Planner-compatible
`- HH:MM - HH:MM <desc>` list items) a first-class capability of `ft`.
Today the user opens the daily note, scrolls to the configured heading,
and types/edits blocks by hand — there is no scriptable surface and no
ergonomic TUI for the most common moves (add a block, extend the end
by 5 minutes, retag).

This plan ships:

1. **Library — `ft_core::timeblock`**: parser, serializer, and section-
   aware mutation primitives for `- HH:MM - HH:MM <desc> @tag` lines
   under a configurable daily-note heading. Round-trip safe; uses ft's
   existing `periodic_notes` to resolve the daily-note path for a given
   date. Hierarchical tags (`@group`, `@group/tag`, `@group/tag/subtag`,
   up to 3 levels) modelled with a structured type and parsed via a
   port of blockary's `tag.rs`.
2. **CLI — `ft timeblocks`**: `list`, `add`, `edit`, `delete`, `spent`.
   Mirrors `ft tasks` ergonomics — flag-based filters, selectors,
   `table`/`json`/`ndjson`/`markdown` outputs, vault-relative paths in
   messages, `--date` defaulting to today, `--dry-run` for mutations.
3. **TUI — Timeblocks tab**: split layout showing **today + tomorrow**.
   Tomorrow's missing daily note renders as a placeholder with a `c`
   chord to create it via the `periodic_notes.daily` template (reuses
   `periodic::create_or_get_periodic_path`). Create via `a` (quickline)
   or `A` (form); navigate with `j`/`k`; adjust end-time with `]`/`[`,
   start-time with `}`/`{` (5-minute intervals); edit description with
   `e`; delete with `d d`. Sidebar shows live clock + per-top-level-tag
   totals for today.

The library is the source of truth for parse/serialize/mutate; the CLI
and TUI are thin call sites on top of it, mirroring the
`task::ops` → `ft tasks` → Tasks-tab shape from plans 001/002.

## Motivation and Context
The user already maintains a `## Time Blocks` section in each daily note
that's compatible with Obsidian's Day Planner plugin and with `blockary`
(`~/git/blockary`), a separate Rust CLI that reads the same format for
sync + time-spent reporting. The current friction is **authoring** those
lines: opening the file, jumping to the right heading, typing
`- 14:00 - 14:30 1on1 with Hans @meetings`, and revisiting that line
every time the meeting overruns by 5 minutes. That overhead is high
enough that the user often skips logging the block at all, which then
breaks the downstream `blockary spent` reports.

`ft` already owns:

- Vault discovery and atomic writes (`ft_core::vault`, `ft_core::fs`).
- Daily-note path resolution via `periodic_notes.daily` config
  (`ft_core::periodic`), including template-driven creation
  (`create_or_get_periodic_path`).
- A working TUI tab framework (`tui::tab::Tab` trait) with an editor-
  handoff sidecar (`AppRequest::OpenInEditor`), a status-bar toast
  pipeline, an `EditBuffer` widget, a `FuzzyPicker`, and a quickline
  pattern in the Tasks tab (`ft/src/tui/tabs/tasks/quickline.rs`).
- Markdown access primitives for "items under heading" — but only via
  `pulldown-cmark` heading extraction (`markdown::extract_headings`).
  Blockary's `markdown_access.rs` has a section-replace helper that
  walks lines directly; we'll port a more conservative version that
  preserves blank lines and handles fenced code blocks via
  `LineSkipState` (already in `ft_core::markdown`).

Why a single library module rather than reusing blockary's crate:

- Blockary models a `Block` with a `period_str: String` plus a derived
  `duration: u16`, which is fine for read/report but awkward for edit
  flows that need to mutate just the start or end. ft's `Timeblock`
  models the period as `NaiveTime` pairs, which gives us correct
  arithmetic for "extend by 5 minutes" and "shift start by 5 minutes."
- Blockary supports `(Origin)` labels to merge blocks across vaults;
  `ft` operates on a single vault, so this complexity isn't needed and
  the format simplifies to `- HH:MM - HH:MM <desc> @tag`.
- Round-trip safety is a strong invariant for `ft` (see plan 001); a
  fresh implementation lets us test it via proptest from day one.

Why ship CLI before TUI:

- The library + CLI lets us verify every mutation on real daily notes
  before any TUI keymap design is sunk. The TUI then becomes a thin
  call site for `add_block`/`edit_block`/`delete_block`.
- `ft timeblocks spent` is independently useful for replacing
  `blockary spent` reports in shell pipelines.

## Acceptance Criteria

### Library — `ft_core::timeblock`

- [x] New module `ft_core::timeblock` with public surface:
      ```rust
      pub struct Timeblock {
          pub start: NaiveTime,
          pub end: NaiveTime,           // derived: start + 30m when not in source
          pub end_explicit: bool,        // tracks whether the source had `- HH:MM` only
          pub desc: String,              // description with tags inline (round-trip)
          pub tags: Vec<Tag>,            // parsed from desc; up to 3 levels
          pub source_line: usize,        // 1-indexed within the section block
      }

      pub struct Tag {
          pub levels: Vec<String>,       // 1..=3 levels; `@a/b/c` → ["a","b","c"]
      }

      pub fn parse_line(s: &str) -> Result<Timeblock, ParseError>;
      pub fn serialize_line(b: &Timeblock) -> String;
      pub fn parse_tags(desc: &str) -> Vec<Tag>;
      ```
- [x] Hierarchical tag parser ported from `blockary::tag` with the
      following restrictions: maximum **3 levels** (parser errors on a
      4th level segment); each level is `[A-Za-z0-9_-]+` with no
      whitespace, no `@`, no `/` (matching dayplanner / Obsidian
      conventions). Brackets/parens (`@p/[[PROJ]]/x`) are explicitly
      out of scope for v1 — reject with a clear `ParseError` pointing
      at the offending substring.
- [x] Round-trip property (proptest): for any generator-produced
      `Timeblock`, `parse_line(serialize_line(b)) == b` byte-for-byte
      modulo the canonical end-time materialization rule.
- [x] Canonical serialization: `HH:MM - HH:MM <desc>` (zero-padded
      times, single ASCII space around the dash, single space before
      desc). When `end_explicit == false`, serializer still emits the
      derived `HH:MM` end (we always normalize on write — input like
      `- 10:00 Foo` round-trips to `- 10:00 - 10:30 Foo` and that's
      considered a feature).
- [x] `ParseError` enum with structured variants: `BadStart`,
      `BadEnd`, `EndBeforeStart`, `TagTooDeep`, `TagBadChar { ch }`,
      `Empty`. Each carries enough context to surface a useful CLI
      error message.

### Library — `ft_core::timeblock::doc`

- [x] `Document` type representing one day's section in a daily note:
      ```rust
      pub struct Document {
          pub blocks: Vec<Timeblock>,
          pub heading: String,           // e.g. "Time Blocks"
          pub source_path: PathBuf,      // absolute path to the daily note
          pub source_content: String,    // full file content at read time
      }

      pub fn read(daily_path: &Path, heading: &str) -> Result<Document>;
      pub fn write(doc: &Document) -> Result<()>;  // atomic, section-replace
      ```
- [x] `read` returns `Document { blocks: vec![] }` when the heading is
      not present in the file (lets the TUI render the empty state and
      the CLI `add` create the section). `write` inserts the heading
      at file end when missing — matching the dayplanner / blockary
      conventions.
- [x] `write` preserves everything outside the target section
      byte-for-byte except for trailing whitespace on the section's
      last line (where blockary's helper introduces drift). Lines
      between heading and next heading-of-equal-or-higher-level are
      replaced wholesale with the freshly-serialized block list.
- [x] Fenced code blocks (``` and ~~~) and frontmatter are honored —
      a heading-shaped line inside a code fence is NOT treated as the
      section boundary. Reuses `crate::markdown::LineSkipState`.
- [x] Section-replace is atomic via `fs::write_atomic`.

### Library — `ft_core::timeblock::ops`

- [x] `add_block(daily_path, heading, new: Timeblock) -> Result<Document>`
      reads, validates (no exact-same-start-and-end-and-desc duplicate
      unless `force: bool` opt-in via an `AddOptions` struct),
      inserts in sorted order (by start time), writes atomically,
      returns the new document.
- [x] `edit_block(daily_path, heading, selector, mutation) -> Result<Document>`
      where `mutation` is an `EditMutation` struct with optional
      `start`, `end`, `desc`, `add_tag`, `remove_tag` fields.
      Time mutations validate `end > start`; tag mutations preserve
      tag order and dedupe.
- [x] `delete_block(daily_path, heading, selector) -> Result<Document>`
      removes the matched block. Errors when no block matches.
- [x] `Selector` enum (mirrors `ft_core::selector` for tasks):
      `Line(usize)` (1-indexed within the section block list, matches
      `source_line`), `Time(NaiveTime)` (matches `start`), `Fuzzy(String)`
      (case-insensitive substring match against description; errors
      with an ambiguous-match list when multiple match).
- [ ] All ops use vault-relative paths in error messages.

### Library — `ft_core::timeblock::report`

- [x] `Spent` summary type:
      ```rust
      pub struct TagTime {
          pub tag: String,            // single level segment
          pub minutes: u32,
          pub children: Vec<TagTime>,
      }
      pub fn time_per_tag(blocks: &[Timeblock]) -> Vec<TagTime>;
      ```
      Ports blockary's `time_summary::time_per_tag` with the same
      hierarchical aggregation, sorted by descending minutes at every
      level.
- [x] `total_minutes(blocks: &[Timeblock]) -> u32` and
      `minutes_to_hours_minutes(m: u32) -> (u32, u32)` helpers.
- [x] Blocks tagged with a top-level `@break` are excluded from
      `total_minutes` totals (matching blockary), but still appear in
      `time_per_tag` so the user can see their break time bucket.

### Library — config

- [x] New `[timeblocks]` block in `Config`:
      ```toml
      [timeblocks]
      heading = "Time Blocks"          # optional; default "Time Blocks"
      ```
      Modelled as `pub struct Timeblocks { pub heading: Option<String> }`
      with `#[serde(deny_unknown_fields)]`. Accessor
      `Config::timeblocks_heading(&self) -> &str` returns
      `"Time Blocks"` when unset.
- [ ] Daily-note path resolution piggybacks on the existing
      `periodic_notes.daily` config (plan 010). When that block is
      missing, `ft timeblocks` errors with the same remedy hint the
      tasks CLI already shows (configure `periodic_notes.daily` or
      pass `--file`). No new daily-note resolver.

### CLI — `ft timeblocks list`

- [ ] `ft timeblocks list [--date YYYY-MM-DD] [--tag X] [--format F] [--file PATH]`
- [ ] Date parsing reuses `ft_core::dates::parse` (ISO, `+3d`,
      `tomorrow`, `yesterday`, etc.); default is today.
- [ ] `--tag` filter matches any block whose tag list contains a tag
      with `@<X>` as a prefix path (so `--tag work` matches
      `@work/meeting` and `@work`). Repeatable; multiple tags
      compose as `or`.
- [ ] `--format table|json|ndjson|markdown` — `markdown` emits source
      lines so output is round-trippable through `ft timeblocks add`.
- [ ] Default exit code 1 on no matches, configurable via
      `--allow-empty` (matching `ft tasks list`).

### CLI — `ft timeblocks add`

- [ ] `ft timeblocks add "<blockstring>" [--date YYYY-MM-DD] [--file PATH] [--force]`
      where blockstring is `HH:MM - HH:MM <desc> [@tag...]` or the
      short form `HH:MM <desc>` (end derived as start + 30m).
- [ ] Alternative form: `ft timeblocks add --start HH:MM --end HH:MM --desc "..." [--tag X]...`
      flags compose with each other but **not** with the positional
      blockstring (clap mutual exclusion).
- [ ] Refuses exact duplicates (same start, end, desc) unless `--force`.
- [ ] Creates the configured heading at file end when missing.
- [ ] `--dry-run` prints the resulting file diff via `similar` (same
      formatter as `ft tasks move`).
- [ ] Edits via `fs::write_atomic`.

### CLI — `ft timeblocks edit`

- [ ] `ft timeblocks edit <selector> [--date ...] [--start ...] [--end ...] [--desc ...] [--add-tag X] [--remove-tag X] [--file PATH] [--dry-run]`
- [ ] Selector forms: `<N>` (1-indexed line in the section),
      `<HH:MM>` (exact start match), or a free-text fuzzy match
      against descriptions. Fuzzy ambiguous error lists up to 5
      candidates with line numbers + descriptions.
- [ ] `--start` / `--end` accept absolute (`HH:MM`) or relative
      (`+15m`, `-5m`); relative values shift the existing time.
      Validation: `end > start`.
- [ ] `--add-tag` and `--remove-tag` are repeatable; preserve tag
      order; dedupe.
- [ ] `--dry-run` semantics identical to `add`.

### CLI — `ft timeblocks delete`

- [ ] `ft timeblocks delete <selector> [--date ...] [--file PATH] [--yes] [--dry-run]`
- [ ] Same selector grammar as `edit`.
- [ ] Prompts for confirmation unless `--yes` (matches the bulk-move
      confirmation pattern from plan 001 session 7); non-TTY without
      `--yes` errors with a hint.

### CLI — `ft timeblocks spent`

- [ ] `ft timeblocks spent [PERIOD] [--format text|json] [--tag X]`
      where `PERIOD` is one of `today` (default), `this-week`,
      `this-month`, `this-year`, `last-week`, or `--from YYYY-MM-DD
      --to YYYY-MM-DD`.
- [ ] Walks every daily-note path resolvable from `periodic_notes.daily`
      within the period (skipping missing files); aggregates per-tag
      via `report::time_per_tag`.
- [ ] `--tag` filter restricts the input set to blocks containing the
      tag prefix.
- [ ] `text` format prints the hierarchical tree with `Hh MMm` formatting
      and a "total" row matching blockary's UX.
- [ ] `json` format emits `{ "total_minutes": N, "tags": [ { "tag":
      "...", "minutes": N, "children": [...] } ] }`.

### TUI — Timeblocks tab

- [ ] New tab `TimeblocksTab` registered after `NotesTab` in `App::new`
      (plan 002 framework). Tab title `"Timeblocks"`.
- [ ] **Layout**: sidebar (24 cols) + main split horizontally between
      "Today" and "Tomorrow" panes (50/50). When tomorrow's daily note
      doesn't exist, its pane renders a placeholder
      (`"Tomorrow (YYYY-MM-DD) — no daily note yet. Press 'c' to
      create."`) and the `c` chord (only when tomorrow pane has focus)
      triggers `create_or_get_periodic_path`.
- [ ] **Sidebar**: live clock (1-second tick), today's date, blank
      line, "── totals (today) ──" header, hierarchical totals from
      `time_per_tag` for today's blocks (top-level only, indented
      sub-levels collapsed), then the active-focus indicator.
- [ ] **Selection**: each pane has its own selected-index. Focus
      toggles between panes with `Tab` / `Shift+Tab` (or `h`/`l`).
- [ ] **Movement**: `j`/`k` or `↓`/`↑` move selection within the
      focused pane. `g` / `G` jump first/last (vim convention,
      already used in other tabs).
- [ ] **Create — quickline** (`a`): bottom-line edit buffer (port the
      tasks-tab `quickline.rs` pattern). Parses input as a blockstring
      on `Enter`; errors render in the status bar as a toast for 3s
      and the buffer is preserved so the user can fix it. Escape
      cancels. Successful create toasts `"+ HH:MM - HH:MM <desc>"`.
- [ ] **Create — form** (`A`): modal popup with three rows
      (Start, End, Desc) plus optional Tags row. Arrow keys cycle
      fields; `Tab` advances; `Enter` on the last field commits;
      `Esc` cancels. Defaults: start = nearest 5-minute boundary
      from clock; end = start + 30m.
- [ ] **Edit description** (`e`): opens an inline edit buffer
      pre-filled with the current description.
- [ ] **Time adjustment chords**:
      - `]` — extend end-time by 5 minutes (clamps at 23:59).
      - `[` — shrink end-time by 5 minutes (clamps at start + 5m).
      - `}` — push start-time by +5 minutes (clamps at end - 5m).
      - `{` — pull start-time by -5 minutes (clamps at 00:00).
      Each chord writes atomically and re-renders. Adjacent-block
      collisions are allowed (blockary doesn't enforce non-overlap;
      neither do we — overlap is sometimes useful for "called away
      mid-task" entries).
- [ ] **Delete** (`d d`): two-stroke chord (status-bar prompt
      `"press d again to delete, Esc to cancel"`); commits on the
      second `d`. `Esc` cancels.
- [ ] **Tag management**: `t` opens a small modal showing the current
      tags and a quickline for adding/removing (`+@tag`/`-@tag`
      syntax). Defer multi-modal interaction to a later session if
      complex.
- [ ] **Refresh** (`r`): re-read both daily notes from disk and rebuild.
- [ ] **Sync compatibility**: writes go through `fs::write_atomic` and
      `Document::write`, so the TUI never corrupts a daily note even
      when the user has the file open in Obsidian (matching plan
      001's atomic-write invariant).
- [ ] **Status-bar toasts**: every mutation emits a success or error
      toast through `AppRequest::Toast`. Errors include the offending
      detail (parse error position, conflict description).

### Output / error model

- [ ] CLI uses `anyhow` + `Context`; library uses `thiserror`-flavoured
      `Error::Timeblock(...)` variant added to the existing
      `ft_core::error::Error`.
- [ ] All paths in user-facing messages are vault-relative.
- [ ] `--json-errors` (existing global flag from plan 001) covers
      every error path through `ft timeblocks`.

### Testing

- [ ] Unit tests: `parse_line` × every valid blockstring shape; every
      invalid shape with the expected `ParseError`; `parse_tags` × the
      full grammar table; serializer round-trip (proptest); `Document::
      read` / `write` × fixture-based snapshot tests for section-
      preserving writes; ops × duplicate detection, sorting, missing-
      heading insertion.
- [ ] Integration tests (CLI) under `ft/tests/timeblocks_*.rs`:
      `list`, `add` (positional + flag form + duplicate refusal +
      `--force`), `edit` (each mutation, relative time shifts,
      ambiguous selector error), `delete` (with `--yes`, non-TTY
      missing-yes error), `spent` (each preset period, JSON shape).
      Use temp-vault fixtures via `assert_fs` to avoid touching the
      real vault.
- [ ] TUI snapshot tests using `ratatui::backend::TestBackend` for:
      empty today, populated today + missing tomorrow, populated
      today + tomorrow, focused tomorrow pane, quickline open, form
      open, delete-confirm prompt, sidebar totals, time-adjustment
      after `]` / `}` chord.
- [ ] Real-vault smoke gated on `FT_REAL_VAULT_TESTS=1` against
      `/Users/cmw/git/fortytwo`: read today's timeblock section,
      `ft timeblocks add --dry-run` produces a sensible diff, and a
      round-trip read/write of the existing section leaves the file
      byte-identical.

### Documentation

- [ ] `docs/timeblocks.md` covering: block format, configurable
      heading, tag grammar (3 levels, allowed chars), the supported
      blockstring forms, CLI subcommand reference, TUI keymap, and
      a "compatibility" section noting that this format is a strict
      subset of blockary's so reports continue to work.
- [ ] README quick-start adds a section under the existing "Quick
      start" tasks example.
- [ ] `docs/architecture.md` gains a `timeblock` module entry next
      to `task` describing the same library/CLI/TUI seam pattern.
- [ ] Man pages regenerate to include `ft-timeblocks*`.

## Technical Notes

### Module layout

```
ft-core/src/
├── timeblock/
│   ├── mod.rs            # Timeblock, Tag, parse_line, serialize_line, parse_tags
│   ├── doc.rs            # Document::read/write (section-aware atomic writes)
│   ├── ops.rs            # add_block, edit_block, delete_block, Selector
│   └── report.rs         # time_per_tag, total_minutes
└── lib.rs                # pub mod timeblock;

ft/src/
├── cmd/timeblocks.rs     # clap subcommand wiring
└── tui/tabs/timeblocks/
    ├── mod.rs            # Tab impl, state machine, layout
    ├── view.rs           # render_today/tomorrow/sidebar
    ├── quickline.rs      # blockstring quickline (port from tasks)
    └── form.rs           # modal form for create
```

### Block format (canonical)

```
- HH:MM - HH:MM <desc>
- HH:MM - HH:MM <desc> @tag
- HH:MM - HH:MM <desc> @group/tag
- HH:MM - HH:MM <desc> @group/tag/subtag
- HH:MM - HH:MM <desc> @a @b/c                  # multiple tags
```

Tags are parsed inline from `<desc>` and stored separately for
queryability, but `desc` retains the raw `@…` text so round-trip
preserves the user's exact authoring (modulo time normalization). When
`edit --add-tag @x` is invoked, the new tag is appended at the end of
the desc with a single leading space; `--remove-tag @x` strips the
exact `@x` token plus its preceding space.

### Hierarchical tag grammar

```
tag        ::= '@' level ('/' level)*               # max 3 levels
level      ::= [A-Za-z0-9_-]+
```

Brackets and parens (blockary supports `@p/[[Project]]/x`) are out of
scope for v1 — they introduce unbounded grammar that interacts poorly
with Obsidian's wikilink syntax in `desc`. Reject them with
`ParseError::TagBadChar { ch: '[' }`.

### Day resolution

The CLI and TUI both ask `Config.periodic_notes.daily` for the daily-
note path, via `ft_core::periodic::resolve_periodic_path`. When the
config is missing we error with:

```
no daily-note source configured.
add a [periodic_notes.daily] block to .ft/config.toml or pass --file
```

— matching the `ft tasks create` remedy hint from plan 001 session 5.
Tomorrow's missing daily note is allowed; the CLI's `add --date
tomorrow` runs `create_or_get_periodic_path` (which writes the
template), the TUI's `c` chord does the same.

### Section replace algorithm

Port a leaner version of blockary's `markdown_access::update_section_lines`:

1. Walk the file line-by-line, tracking `LineSkipState` (fences +
   frontmatter from `ft_core::markdown`).
2. Identify the **first** heading line matching the configured title
   case-insensitively, ignoring leading `#` count (i.e. `## Time
   Blocks` and `### time blocks` both match — we re-emit at the
   matched heading's own level).
3. Replace every line from `(heading + 1)` through "the line before
   the next heading of equal-or-higher level, or EOF" with: blank
   line, serialized blocks (`- HH:MM - HH:MM <desc>`), blank line.
4. When no matching heading is found: append `\n## Time Blocks\n\n
   <blocks>\n` at file end.

Atomic write via `crate::fs::write_atomic`.

### Edit / shift semantics

`edit --start +5m` reads the current start, adds 5 minutes (clamping
to 23:59:00 and never below 00:00), validates `end > start`, and
writes. Similarly for `--end`. `--start HH:MM` is absolute.

In the TUI, `]` is implemented by constructing an `EditMutation {
end: Some(EndShift::Plus(5)), ..default() }` and calling
`ops::edit_block`. This keeps the source-of-truth in the library and
the TUI as a thin keymap.

### Selector resolution

Library `Selector::resolve(&self, blocks: &[Timeblock]) -> SelectorResult`
where `SelectorResult` is `Found(usize) | None | Ambiguous(Vec<usize>)`.
The CLI maps `Ambiguous` to a printable list using `comfy-table` (one
row per candidate). The TUI doesn't expose selectors directly —
selection is via the focused list index — but the same resolver is
reused for fuzzy-edit modal flows.

### TUI state machine

```
enum TimeblocksState {
    Idle,
    Quickline(QuicklineState),
    Form(FormState),
    EditDesc(EditDescState),
    Tagging(TagState),
    DeleteConfirm { pane: Pane, block_idx: usize },
    CreateTomorrowConfirm,   // optional — may collapse `c` to one-shot
}
```

`Pane` is `Today | Tomorrow`. Focus is tracked separately from the
mode. Mutations always go through `ops::*` then a focused-pane
re-read.

### Compatibility with blockary

This plan stays strictly within the format blockary already reads:

- `- HH:MM - HH:MM <desc>` list items.
- `@tag` / `@parent/child` tags (we restrict to 3 levels; blockary
  doesn't enforce a max but most fixtures use ≤ 3).
- Configurable section heading (blockary hard-codes `Time Blocks`;
  ours defaults to the same and is overridable).

The user can continue to run `blockary sync` and `blockary spent`
unchanged on the same daily notes.

### Out of scope for this plan

- iCalendar pull / multi-vault sync (blockary's `sync` and `pull`).
  ft is single-vault by design; if needed later, that's a separate
  plan.
- Block completion state (`- [x] HH:MM - HH:MM ...`) — user explicitly
  opted for the plain dayplanner format.
- Block tags via Obsidian `#tag` syntax — tags use `@…` exclusively.
- Drag-and-drop reordering in the TUI (the source order is always
  ascending start-time, so reorder is not user-controlled).
- Non-overlap enforcement (overlap is allowed; future plan can add a
  warning surface if it becomes painful).
- "Now" cursor / current-time indicator in the today pane (nice-to-
  have; deferred to a polish session if appetite remains).

## Sessions

### Session 1 · 2026-05-16 · done
**Goal:** ft_core::timeblock library — model, parser, serializer, tag grammar,
Document read/write with section-aware atomic edits, full unit + proptest
coverage. No CLI, no TUI yet.

**Scope:**
- `ft_core::timeblock::{Timeblock, Tag, ParseError}` types
- `parse_line` / `serialize_line` / `parse_tags`
- `Document::read` / `Document::write` with `LineSkipState`-aware
  section replace
- `ops::{add_block, edit_block, delete_block, Selector}`
- `report::{time_per_tag, total_minutes, minutes_to_hours_minutes}`
- `[timeblocks]` config block + accessor on `Config`
- `Error::Timeblock` variant added to `ft_core::error::Error`

**Tests:**
- Unit tests per module (parse_line × valid/invalid shapes; tag grammar
  including 3-level limit and bad-char rejection; document read/write
  round-trip on a multi-section fixture; ops × duplicate detection,
  sorting, missing-heading append, selector resolution incl.
  ambiguous case; report × hierarchical aggregation, `@break`
  exclusion from totals)
- Proptest round-trip: `parse_line(serialize_line(b)) == b`
- Snapshot test (`insta`) on `Document::write` against a real-shaped
  daily-note fixture

**Done means:** library can read and rewrite a daily note's timeblock
section atomically; round-trip safety verified by proptest; no
binary changes yet.

**Outcome:** Shipped `ft_core::timeblock` with `mod.rs` (types +
parser + serializer), `doc.rs` (Document read/write), `ops.rs`
(add/edit/delete + Selector + TimeChange + EditMutation +
AddOptions), and `report.rs` (TagTime + time_per_tag +
total_minutes + minutes_to_hours_minutes). 75 new tests, all
green; full `cargo test -p ft-core` reports 534 passed. Round-trip
property verified via proptest (idempotent on the serialized form
rather than block-equal, since `end_explicit` flips on first
serialization — noted in module docs). Tag parser is lenient
inline (`parse_tags` skips malformed `@…` tokens so legacy
blockary notes like `@p/[[PROJ]]` still read) but strict per-tag
via `parse_tag_string` for CLI flag validation. Document
read/write honors fenced code via `LineSkipState`. Config gained a
`[timeblocks]` block + `Config::timeblocks_heading()` accessor;
`Error::Timeblock(String)` variant added. Clippy and fmt clean.
Deferred to later sessions: `insta` snapshot test on Document::write
(direct assertions covered the same behavior — adding a snapshot
is cheap polish), and vault-relative paths in ops error messages
(ops doesn't see the vault root yet; CLI layer in session 2 will
wrap with vault-relative formatting).

### Session 2 · planned
**Goal:** `ft timeblocks list|add|edit|delete` CLI on top of the
library, with `--dry-run`, selectors, formats, integration tests.

**Scope:**
- `ft::cmd::timeblocks` module + clap wiring in `ft/src/main.rs`
- `list` flag set (`--date`, `--tag`, `--format`, `--file`,
  `--allow-empty`); reuses `ft_core::dates::parse`
- `add` with positional blockstring + flag-form (mutually exclusive);
  `--force`, `--dry-run`, idempotency
- `edit` with the selector grammar and `--start`/`--end`/`--desc`/
  `--add-tag`/`--remove-tag`; relative time shifts (`+5m`, `-5m`)
- `delete` with confirmation (`--yes` / non-TTY error)
- Output formats: table (`comfy-table`), json, ndjson, markdown
- Vault-relative paths in messages

**Tests:**
- Integration tests using temp-vault fixtures
- Snapshot tests per output format on a populated daily note
- Idempotency / force / dry-run mtime-unchanged tests

**Done means:** every mutation is scriptable from the shell; CLI
verified against a temp vault end-to-end. Real-vault smoke optional
here (full smoke in session 6).

### Session 3 · planned
**Goal:** `ft timeblocks spent` reporting across configurable date
ranges, with hierarchical tag breakdown.

**Scope:**
- `spent` subcommand: `today` (default), `this-week`, `this-month`,
  `this-year`, `last-week`, plus `--from`/`--to`
- Walks every daily-note path resolvable from `periodic_notes.daily`
  within the range
- Hierarchical print format ports blockary's tree rendering
- JSON format

**Tests:**
- Unit tests on the range-bound helpers (ported from blockary's
  `cli::get_*_bounds`)
- Integration test with a multi-day temp vault covering each preset
- JSON snapshot

**Done means:** the user can replace `blockary spent` invocations with
`ft timeblocks spent` against the same vault.

### Session 4 · planned
**Goal:** Read-only TUI Timeblocks tab — today + tomorrow split,
sidebar, navigation, refresh; no mutations yet.

**Scope:**
- `TimeblocksTab` registered in `App::new`
- `view.rs` renders sidebar (clock + totals) + today/tomorrow split
- Tomorrow placeholder when daily note missing
- `j`/`k`/`g`/`G`/`Tab`/`Shift+Tab` navigation
- `r` refresh
- Snapshot tests for empty, populated, and missing-tomorrow states

**Tests:**
- `TestBackend` snapshots for the layout cases listed in Acceptance
  Criteria (TUI)

**Done means:** can read both days at a glance; no mutations.

### Session 5 · planned
**Goal:** TUI mutations — quickline (`a`), form (`A`), description
edit (`e`), time chords (`]`/`[`/`}`/`{`), delete (`d d`),
create-tomorrow (`c`).

**Scope:**
- Quickline state + parser hookup; status-bar errors
- Modal form (Start/End/Desc/Tags) — port the form-row pattern from
  the notes-tab compose view
- Description edit via inline `EditBuffer`
- Time adjustment chords (5-minute steps, clamping rules)
- Two-stroke `d d` delete with status-bar prompt
- `c` chord in the tomorrow pane: invokes `create_or_get_periodic_path`
  and re-reads
- Toasts on every success/failure

**Tests:**
- Snapshot tests for each modal state and post-mutation render
- Behavior tests for the chord sequencing (quickline open → text →
  Enter → success toast, delete d-then-d, time clamping at 00:00 /
  23:59 / start+5m)

**Done means:** every mutation specified in the Acceptance Criteria
(TUI) is reachable from the keyboard, with success/error feedback.

### Session 6 · planned
**Goal:** Polish — `t` tag modal, `--json-errors` coverage check,
docs, man-page regeneration, real-vault smoke.

**Scope:**
- `t` tag modal (deferred from session 5 if it grew complex)
- Doc page `docs/timeblocks.md` + README update + architecture entry
- Regenerate man pages via existing `ft man`
- Real-vault smoke (`FT_REAL_VAULT_TESTS=1`) — dry-run add, read,
  spent against the real vault; assert no file modifications occur
- Final coverage / clippy / fmt pass

**Done means:** the plan's acceptance criteria are all ticked; docs
shipped; real vault round-tripped cleanly.
