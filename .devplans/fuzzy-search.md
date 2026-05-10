---
id: 005
name: fuzzy-search
title: Fuzzy file + heading search (ft-core capability)
status: ready
created: 2026-05-10
updated: 2026-05-10
---

# Fuzzy file + heading search (ft-core capability)

## Goal
Add a reusable fuzzy search primitive to `ft-core` that lets callers find
notes and headings inside a vault with a single tokenized query string. The
query is split on `#`: the part before matches filenames fuzzily, the part
after matches markdown headings fuzzily inside the candidate files. Example:
`gen consid#Firs` matches both `General Considerations about Food.md` and
`initial general considerations.md`, and if the latter contains a section
`### First Try`, that section becomes the heading hit on that file.

The capability ships in three layers:
1. **`ft_core::search`** — pure library: query parser, heading extractor,
   nucleo-backed scoring, `Vault::fuzzy_find` returning a ranked `Vec<Hit>`.
2. **`ft find QUERY`** — a thin CLI surface so the capability is usable
   outside the TUI (scripting, manual lookups, integration tests).
3. **TUI `FuzzyPicker` widget** — a reusable component (text input + ranked
   result list with highlighting + keyboard navigation) that plan 004's
   target-file selector and plan 003's future notes browser both consume.

## Motivation and Context
Plan 004 (Create tasks from the TUI) needs a target-file picker for the
expanded popup's `target` field — typing exact paths is brittle, especially
in vaults with hundreds of files. Plan 003 (Notes commands) will need the
same primitive to jump into a note or section. Building a fuzzy
file+heading search once, in `ft-core`, means every future surface
(open-by-name, link-insertion, daily-note jump) sits on the same engine
instead of growing one-off implementations.

User-driven scoping decisions:
- **Algorithm:** `nucleo` (the fzf rewrite used by Helix). Fast, parallel,
  SIMD-accelerated, fzf-parity scoring. One new workspace dep.
- **Indexing:** scan-on-query, no cache. With rayon + the existing
  `markdown_files()` walker, 5k notes ranks in <100ms — well below the
  threshold where caching starts paying off, and zero invalidation bugs.
- **Tui-create coupling:** plan 004 waits for this plan's sessions 1–3 to
  land before its session 4 (the expanded popup's target field consumes
  the `FuzzyPicker` widget). Avoids throwaway code and gives both plans a
  cleaner shape.

## Acceptance Criteria

### Query language
- [ ] `text` (no `#`) — fuzzy-match filenames only; results have no heading
- [ ] `text#heading` — fuzzy-match filenames with `text`, then within each
      candidate file fuzzy-match headings with `heading`; results carry a
      `Heading` payload
- [ ] `#heading` (empty file part) — fuzzy-match headings across the whole
      vault; rank by heading score alone
- [ ] `text#` (empty heading part) — same as `text` (the trailing `#` is
      treated as a no-op, not an error, so the user can type progressively)
- [ ] Query parsing handles whitespace at the boundaries: leading /
      trailing whitespace in each part is trimmed but inner whitespace is
      kept (it contributes to the fuzzy score as a subsequence char)
- [ ] Empty query returns an empty result (no error)

### Heading extraction
- [ ] Recognize ATX headings (`#`, `##`, `###`, `####`, `#####`, `######`)
      followed by a space and text on a single line
- [ ] Capture level (1–6), text (trimmed, trailing `#` stripped), and
      1-indexed line number
- [ ] Skip headings inside fenced code blocks (``` and ~~~) and inside
      indented code blocks (4-space)
- [ ] Skip lines that look like headings but are actually inside frontmatter
      (the leading `---` block at the very top of the file)
- [ ] Setext headings (`===` / `---` underlines) explicitly out of scope —
      they're rare in modern Obsidian vaults

### Scoring & ranking
- [ ] Filename fuzzy score uses `nucleo::Matcher` (or equivalent); higher =
      better; non-matches return `None`
- [ ] Heading score uses the same matcher
- [ ] Combined score (when both parts are present) = `file_score +
      heading_score`; files with no matching heading are filtered out of
      the heading-query case
- [ ] Bonus weighting: filename matches that hit the basename (not just
      the directory path) get a small bonus; heading matches at level 1
      (`#`) get a small bonus over deeper levels
- [ ] Stable tiebreaker: when scores tie, sort by path lexicographic asc
      so results don't reshuffle between identical queries
- [ ] Configurable `limit` (default 25) — only the top N hits are returned

### Performance
- [ ] First query on a 5k-note vault returns in <100ms in release (rayon-
      parallel file walk + heading extraction)
- [ ] Subsequent queries (no caching, but OS file cache warm) return in
      <50ms in release
- [ ] No allocation per file in the hot path beyond what nucleo needs
      (extract headings into a reusable buffer where practical)

### Public API
```rust
// ft-core/src/search.rs
pub struct Query {
    pub file_part: String,
    pub heading_part: Option<String>,
}
impl Query {
    pub fn parse(input: &str) -> Self;
    pub fn is_empty(&self) -> bool;
}

pub struct Heading {
    pub text: String,
    pub level: u8,
    pub line: usize,
}

pub struct Hit {
    pub path: PathBuf,            // relative to vault root
    pub file_score: i32,
    pub heading: Option<Heading>,
    pub heading_score: Option<i32>,
    pub total_score: i32,         // sum used for ranking
}

pub struct SearchOptions {
    pub limit: usize,
    pub include_headings: bool,   // default false; auto-true if heading_part present
}

impl Vault {
    pub fn fuzzy_find(&self, query: &Query, opts: SearchOptions) -> Vec<Hit>;
}
```

### CLI surface — `ft find`
- [ ] `ft find QUERY` prints up to 25 hits, one per line, format
      `PATH:LINE  heading text` (or just `PATH` if no heading)
- [ ] Path styled blue, heading yellow, dim score on the right when stdout
      is a TTY (use the existing color machinery from plan 001 session 8)
- [ ] `--limit N` overrides the default
- [ ] `--include-headings` forces heading extraction even when the query
      has no `#` (useful when you want a broad jump-list)
- [ ] `--format ndjson` emits one JSON object per line for scripting; each
      object has `{path, line?, heading?, level?, score}`
- [ ] Exit codes: 0 with results, 1 with no matches (callers can chain
      with `&&`)
- [ ] Help and man page entries via clap's existing derive

### TUI `FuzzyPicker` widget
The widget lives at `ft/src/tui/widgets/picker.rs` and is constructed by
callers with a list source so it can serve more than just file search in
the future.
- [ ] `FuzzyPicker::new(source: Box<dyn PickerSource>)` where
      `PickerSource` provides `query(&str, limit) -> Vec<PickerItem>` and
      `display(&PickerItem) -> Line<'static>` for custom row rendering
- [ ] Renders inside a caller-supplied `Rect`: 1-line input on top,
      scrollable result list below
- [ ] Input handling reuses `EditBuffer` so Ctrl+W / Ctrl+⌫ already work
- [ ] `↑` / `↓` (and `j` / `k`) move the selection within the result list
- [ ] `Enter` returns the highlighted `PickerItem`; the caller decides
      what to do with it
- [ ] `Esc` cancels and returns `None`
- [ ] Match highlight: the chars in each result that contributed to the
      fuzzy score are bolded / colored so the user can see why a row
      matched
- [ ] Empty input shows a "type to search" hint
- [ ] No matches shows "no matches" centered in the list area
- [ ] Layout adapts to the caller's `Rect` — narrow renders stay readable
      down to ~40 cols wide
- [ ] A concrete `VaultFilePickerSource` ships in the same module so the
      picker is usable out of the box for note / heading selection

### Testing
- [ ] Unit tests for `Query::parse`: bare text, `text#heading`, `#heading`,
      `text#`, empty, whitespace handling
- [ ] Unit tests for heading extraction: ATX 1–6, code-fence skip,
      indented-code skip, frontmatter skip, malformed `#` lines
- [ ] Unit tests for `fuzzy_find` on a synthetic vault: file-only query,
      file+heading query, heading-only query, no matches, limit honored,
      tiebreaker stable
- [ ] Behavioral test for `ft find` CLI: tab-separated stdout shape,
      ndjson shape, exit codes
- [ ] Snapshot tests for the `FuzzyPicker` widget: empty input, populated
      list with highlight, no-matches state, narrow-rect render
- [ ] Perf test (gated on `FT_PERF_TESTS=1`): 5k-note synthetic vault,
      assert <500ms in debug / <100ms in release

## Technical Notes

### Library boundaries
The core (`ft_core::search`) depends only on `nucleo`, `rayon`, `ignore`
(already a workspace dep), and the existing markdown walker. It does not
touch ratatui, crossterm, or any TUI concern.

The CLI (`ft find`) lives in `ft/src/cmd/find.rs` and calls
`vault.fuzzy_find` directly.

The TUI widget (`ft/src/tui/widgets/picker.rs`) is generic over a
`PickerSource` trait so it doesn't hard-depend on `ft_core::search` —
that keeps the widget reusable for non-search pickers in the future
(e.g. target-status picker, command palette).

### Heading extractor location
Headings aren't a task concept, so the extractor lives in a new
`ft_core::markdown` module rather than under `task::`. The task parser
remains the source of truth for `- [ ]` lines; this module is the source
of truth for `#` headings, code blocks, and frontmatter detection. Both
modules can be used independently.

### Why `nucleo` not `fuzzy-matcher`
- Native parallel matching that plays nicely with rayon
- SIMD-accelerated inner loop (~3-5x faster than skim's algorithm on
  real vaults)
- Used by Helix, so well-exercised in the Rust ecosystem
- Trade-off: API is slightly more ceremonial (`Pattern::parse` +
  `Matcher` + `Atom`), but only used inside one module

### Score normalization
nucleo returns scores roughly in the range 0..1000 for short queries. We
keep them in `i32` and sum file+heading scores for the combined ranking;
no need to normalize because all scores come from the same matcher.

### File walker reuse
`Vault::markdown_files()` already exists from plan 001. We call it once
per `fuzzy_find` invocation. For scan-on-query at 5k notes the walk is
~5ms; the bulk of the time is heading extraction on the filename-matching
candidates.

### Two-stage filtering
1. Score every filename against `query.file_part`. Discard non-matches.
2. If a `heading_part` is set: read each file that survived stage 1, run
   the heading extractor, score each heading, keep the best one per file.
3. Sort by `total_score` desc, take top `limit`.

This avoids reading file contents when the user is only filename-searching
(the common case for plan 004's target field).

### `ft find` exit codes
Matches the conventions in plan 001 session 8:
- 0 → at least one match printed
- 1 → no matches (legitimate empty result)
- 2 → bad query / IO error (reserved)

### Coupling with plan 004
Plan 004 session 4 (expanded popup) consumes `FuzzyPicker` from this
plan's session 3. Plan 004 sessions 1–3 (quickline parser, UI, toast) have
no dependency on this plan, so a contributor could parallelize, but the
natural order is:
  005-S1 → 005-S2 → 005-S3 → 004-S1 → 004-S2 → 004-S3 → 004-S4 → 004-S5

### Out of scope for v1
- Persistent on-disk index (revisit if scan-on-query crosses 200ms on
  realistic vaults)
- Aliases / Obsidian frontmatter `aliases:` field as additional match
  sources
- Backlinks / outgoing-link awareness (filenames only)
- Block IDs (`^block-id`) as match targets — they're a less common
  navigation surface
- Setext headings (`===` / `---` underlines)
- Smart casing / camelCase splitting beyond what nucleo gives us for
  free
- Fuzzy match on file contents (full-text search) — that's a separate
  capability
- Picker enhancements: preview pane, multi-select, custom keymaps —
  v1 ships the minimal one-line input + result list

## Sessions

### Session 1 · 2026-05-10 · planned
**Goal:** ft_core foundation: add nucleo dep, ft_core::search module (Query::parse, Hit, SearchOptions), ft_core::markdown module (heading extractor with code-fence + frontmatter skip), Vault::fuzzy_find scan-on-query implementation using rayon. Full unit-test coverage on query parser, heading extractor, and fuzzy_find against a synthetic vault.
**Outcome:** 

### Session 2 · 2026-05-10 · planned
**Goal:** ft find CLI command: ft/src/cmd/find.rs with clap derive, tab-separated stdout + ndjson format, color-on-TTY, --limit / --include-headings flags, exit codes per plan-001 conventions, integration tests + real-vault gated test.
**Outcome:** 

### Session 3 · 2026-05-10 · planned
**Goal:** TUI FuzzyPicker widget at ft/src/tui/widgets/picker.rs: PickerSource trait, EditBuffer-backed input, scrollable result list with match highlighting, arrow/jk navigation, Enter returns selection, Esc cancels. Concrete VaultFilePickerSource for the file+heading case. Snapshot tests covering empty / populated / no-match / narrow-width.
**Outcome:** 

### Session 4 · 2026-05-10 · planned
**Goal:** Polish & audit: 5k-vault perf test gated on FT_PERF_TESTS=1 (<100ms release / <500ms debug budgets), snapshot completeness, help / man-page entries for ft find, no-warnings cleanup, real-vault smoke check.
**Outcome:** 

