---
id: 003
name: notes
title: Notes tab — open & section-move
status: implementing
created: 2026-05-09
updated: 2026-05-11
---

# Notes tab — open & section-move

## Goal
Add a Notes tab to the TUI with two tightly-scoped abilities and matching
CLI commands:

1. **Open** any note or section in `$EDITOR` or Obsidian, via a fuzzy
   picker that uses the existing `file#heading` syntax.
2. **Move sections** from a source note to a target note with a clipboard +
   compose UX: multi-select headings from the source (with implicit
   hierarchical inclusion), then arrange each clipboard item interactively
   in the target — choosing both **position** and **target heading level**
   per item — before committing.

Both abilities ship as CLI commands first so they're scriptable and
independently testable; the TUI tab wraps the same library primitives.

## Motivation and Context
The vault accumulates content in the wrong place — meeting notes land in
the daily note because there's no time to decide the right target while
writing. A fast fuzzy-open and a section-move tool with level reshaping
remove the two biggest friction points without requiring a general link-
rewriting engine. Link rewriting (file rename/move, backlink maintenance)
is explicitly out of scope here; it belongs in a future plan once these
simpler flows prove out the tab framework.

`ft_core::search` (plan 005) and the `FuzzyPicker` widget (plan 005
session 3) are already built. `AppRequest::OpenInEditor` plus
`suspend_terminal`/`spawn_editor` (ft/src/tui/app.rs:256, app.rs:328) already
handle the editor-suspend path. The Notes tab is primarily wiring plus three
new library primitives: section extraction, level-shift transform, and a
sequential pair-write.

## Acceptance Criteria

### Library — `ft_core::notes`

- [x] `Section` struct: `heading: Heading, body: String`. `body` is the
      content from the heading line (inclusive) to the next heading of
      equal-or-higher level (exclusive), or end of file. This matches
      Obsidian's fold behavior and means moving an H2 always drags its
      nested H3/H4/etc with it.
- [x] `extract_sections(content: &str) -> Vec<Section>` — returns sections
      in document order. Content before the first heading is excluded.
      Frontmatter is skipped (the existing `extract_headings` rule).
- [x] `shift_section_level(section: &Section, new_top_level: u8) -> Result<String>` —
      returns the section's body with every heading level shifted by
      `new_top_level - section.heading.level`. If any nested heading would
      shift outside the ATX range `1..=6`, returns `Err`. The shift is
      applied only to lines that `extract_headings` recognizes as
      headings (i.e., skips fenced/indented code and frontmatter blocks
      inside the section body — though frontmatter inside a section body
      is exotic and treated like any other line).
- [x] `validate_disjoint(headings: &[&Heading]) -> Result<()>` — returns
      `Err` if any selected heading is contained within another selected
      heading's section. Identified by line range (not text).
- [x] `move_sections(source: &str, picks: &[SectionPick], target: &str, plan: &[Placement]) -> Result<(String, String)>` —
      where `SectionPick { line: usize, new_level: u8 }` identifies a
      heading in the source by line number and the target level for the
      move, and `Placement { pick_idx: usize, after_line: Option<usize> }`
      says where in the target each pick goes (`after_line=None` means
      "top of file, before all headings"). Returns
      `(new_source_content, new_target_content)`. Multiple picks may share
      placements that interleave with the target's existing headings.
- [x] `write_pair(target_path, target_content, source_path, source_content) -> Result<()>` —
      writes target first, then source, each via `fs::write_atomic`. Order
      matters: a crash between the two writes leaves the moved sections
      duplicated (recoverable by hand) rather than lost.
- [x] `obsidian_url(vault_name: &str, rel_path: &Path, heading: Option<&Heading>) -> String` —
      lives in `ft_core::notes` (not the CLI module) so both `ft notes
      open --obsidian` and the TUI's `Ctrl+O` branch call the same builder.
      Hoisted out of `ft/src/cmd/notes.rs` as part of session 3; the CLI's
      private helper becomes a one-line delegation. `vault_name` is whatever
      override the caller has (CLI flag, future TUI config) falling back to
      the basename of the vault root.

### CLI — `ft notes open`

- [x] `ft notes open <QUERY>` — runs `Vault::fuzzy_find` and opens the top
      hit. Honors the `file#heading` query syntax. `QUERY` is required;
      passing no query exits 2 with a message pointing at `ft tui`.
- [x] Opens in `$EDITOR` (resolving `VISUAL` → `EDITOR` → `vi`, same as
      `spawn_editor` in app.rs). When the hit carries a heading, passes
      the heading's line as `+<line>` so the editor jumps to the section.
- [x] `--obsidian` flag — prints (and `open`s on macOS) an
      `obsidian://open?vault=<name>&file=<url-encoded-path>` URL. When the
      hit has a heading, appends `&heading=<url-encoded-text>` (Obsidian's
      advanced-URI plugin honors it; vanilla Obsidian falls back to
      opening the file). Document this as best-effort.
- [x] `--editor <bin>` — overrides `$EDITOR` for this invocation.
- [x] Exit codes: 0 success, 1 no match, 2 bad args / error.

### CLI — `ft notes move-section`

- [x] Required: `--from <path>`, `--to <path>`, and at least one of
      `--heading TEXT` / `--heading-regex PATTERN` / `--from-query QUERY`.
- [x] `--heading TEXT` — exact match against heading text (trimmed,
      case-insensitive). Repeatable to pick multiple distinct sections.
- [x] `--heading-regex PATTERN` — Rust regex matched against heading text.
      Repeatable; results combine with `--heading` results.
- [x] `--match-policy first|all|error` — what to do when a `--heading` or
      `--heading-regex` matches more than one heading in the source.
      Default: `error` (refuse to write, list the line numbers of the
      ambiguous matches). `first` takes the first match in document order.
      `all` takes every match.
- [x] `--from-query QUERY` — convenience; uses `file#heading` fuzzy syntax
      via `Vault::fuzzy_find`, picks the top hit. Mutually exclusive with
      `--from` + `--heading`.
- [x] `--at-level N` — drop the moved sections at heading level `N` in the
      target (the cascade scales nested headings). Default: preserve source
      level. Errors if the cascade would push any nested heading past
      level 6.
- [x] `--after TEXT` — place moved sections after the named heading in the
      target. Uses the same `--match-policy` for disambiguation. Omitting
      `--after` inserts at the top of the target (before its first
      heading). When multiple sections are moved with one CLI invocation,
      they all share the same insertion point and `--at-level`.
- [x] Disjoint-section validation: if the picked sections overlap (a
      parent and a child both selected), error with the line numbers of
      the offenders. The TUI prevents this UX-side; the CLI catches it.
- [x] Same-file move (source path == target path) errors as out of scope.
- [x] Prints a unified diff of both files to stdout before writing, then
      prompts `Apply? [y/N]`. `--yes` / `-y` skips the prompt. On a non-
      TTY (piped) invocation without `--yes`, exits 2 with a message
      requesting `--yes`.
- [x] Exit codes: 0 success, 1 nothing matched, 2 bad args / error.

### TUI — Notes tab

- [x] Registered as the third tab in the tab bar (after Welcome and
      Tasks). Accessible via `Tab` cycling and number keys (e.g. `3`)
      using the existing tab-switch keybinding from plan 002.
- [x] **Idle state**: a help panel listing the available shortcuts —
      `o` open · `m` move sections · `?` help. No vault listing or
      auto-loaded preview; the panel is the only thing on screen until a
      flow starts. Mirrors the empty-state pattern of the Tasks tab.
- [x] **`?` help overlay** — modal panel rendered over the tab when `?`
      is pressed (idle state only; flows own their own keybindings).
      Lists the Open-flow and (placeholder) section-move bindings.
      Dismissed by `?`, `Esc`, or `q`. (Section-move bindings are stubs
      until sessions 4-5 wire that flow.)
- [x] **Open flow** (`o` key while the Notes tab is focused):
      - Opens a `FuzzyPicker` over the vault, using `VaultFilePickerSource`.
      - `Enter` issues `AppRequest::OpenInEditor { path, line }` — line is
        the heading line if the hit carries one, else 1. The app suspends
        the TUI, spawns the editor, restores on exit (existing wiring).
      - `Ctrl+O` issues a new `AppRequest::OpenInObsidian { url }` (new
        variant) that hands the URL to the OS `open`/`xdg-open` handler.
      - `Esc` dismisses the picker and returns to the idle state.
- [ ] **Section-move flow** (`m` key while the Notes tab is focused):
      Driven by a `SectionMoveState` enum so transitions stay coherent:
      ```
      enum SectionMoveState {
          SourcePicking,
          HeadingMultiSelect { source_path, headings, selected, focus },
          TargetPicking { clipboard },
          Composing { clipboard, target_path, target_headings, layout, focus },
      }
      ```
      - **Step 1 — Source picker** (`SourcePicking`): `FuzzyPicker` over the
        vault. `Enter` advances to step 2; `Esc` cancels the flow.
      - **Step 2 — Heading multi-select** (`HeadingMultiSelect`): vertical
        list of the source's headings with indent matching ATX level.
        `↑`/`↓` moves focus. `Space` toggles selection on the focused
        heading. Selecting a parent **automatically and visibly** marks
        every descendant as included (rendered with a dimmed/secondary
        marker, e.g. `▣` vs `■`); descendants can't be toggled while the
        parent is selected. Deselecting the parent removes the implicit
        marker on its descendants. `Enter` accepts the selection and
        advances; `Esc` returns to step 1.
      - **Step 3 — Target picker** (`TargetPicking`): a second
        `FuzzyPicker`. Same-file pick (target equals source) is rejected
        inline with a footer error. `Enter` advances; `Esc` returns to
        step 2 with selection preserved.
      - **Step 4 — Compose** (`Composing`): a single list view interleaves
        the target's original headings (dimmed, non-movable, with their
        existing level shown) and the clipboard's pending insertions
        (highlighted, with their current proposed level). The pending
        items start in the order they were selected from the source,
        placed at the bottom of the target. With a pending item focused:
        - `↑`/`↓` moves the cursor between rows.
        - `Shift+↑` / `Shift+↓` reorders the focused pending item up/down
          relative to all rows (it can sit before the first target
          heading, between any two, or after the last).
        - `←` / `→` shifts the pending item's heading level by ±1, with
          the cascade applied to its nested headings. Blocked at level 1
          (can't go higher) and at the level that would push any nested
          heading past 6 (error toast, no change).
        - `Enter` commits: builds `picks` + `plan`, calls `move_sections`
          + `write_pair`, emits a success toast (`Moved N section(s):
          <src> → <dst>`), and returns to the Notes tab idle state.
        - `Esc` returns to step 3 with the compose layout preserved.
- [ ] A status-line footer under each step shows the step indicator
      (`1/4 source · 2/4 select · 3/4 target · 4/4 compose`) and the
      active keybindings for that step.
- [ ] **Clipboard contents and lifecycle**: at the step-2 → step-3
      transition, the picked sections' bodies are extracted from the
      in-memory source content (via `extract_sections`) and cached on the
      `Composing` state alongside each pick's original `(line, level,
      heading_text)`. From that point on the flow has the data it needs
      to render and commit without re-reading the source — *except* for
      the commit-time freshness check below.
- [ ] **Commit-time freshness check**: when `Enter` fires in `Composing`,
      the source file is re-read from disk. For every cached pick, verify
      a heading still exists at the recorded line with the recorded text
      and level. On mismatch, abort with an error toast (`source changed
      on disk — aborted`) and return to the idle state. No partial write.
      Successful path: build `picks` + `plan` from the freshly re-read
      source, call `move_sections` + `write_pair`.

### Testing

- [ ] Unit tests for `extract_sections`: single-section file, nested
      headings (H3 under H2 is part of the H2 section), sibling H2s, file
      with content before first heading (content excluded), frontmatter
      skip, empty file.
- [ ] Unit tests for `shift_section_level`: shift down (H2→H3 cascades
      H3→H4), shift up (H3→H2 cascades H4→H3), no-op shift, cascade
      that would exceed H6 returns Err, headings inside code fences not
      shifted.
- [ ] Unit tests for `validate_disjoint`: parent + child overlap rejected,
      siblings accepted, identical line rejected, empty input accepted.
- [ ] Unit tests for `move_sections`: single section preserving level,
      single section with level shift, multiple sections preserving
      relative order, insertion at top (no after_line), insertion between
      target headings, insertion after last target heading, source content
      remainder is contiguous (no orphaned blank lines between sibling
      remainders).
- [x] Integration tests for `ft notes open`: top-hit opens correct path
      (captured by overriding `EDITOR` with a recording shim),
      `--obsidian` emits the right URL on stdout when `FT_OBSIDIAN_DRY_RUN=1`
      is set, no-match exits 1, missing query exits 2.
- [x] Integration tests for `ft notes move-section`:
      - Single heading with `--heading` succeeds; ambiguous match with
        default policy errors and lists line numbers.
      - `--match-policy first` resolves ambiguity.
      - `--match-policy all` moves every match.
      - `--heading-regex "^Meeting"` works against multiple headings.
      - `--at-level 3` shifts levels; out-of-range cascade exits 2.
      - `--from-query "daily#notes"` resolves via fuzzy search.
      - `--after "Background"` positions correctly; missing `--after`
        inserts at top.
      - `--yes` skips the prompt; piped invocation without `--yes` exits 2.
      - Same-file move exits 2.
- [ ] Snapshot tests for the Notes tab, driven through the existing
      `tui::tests` harness (`App::for_test_with_clock` + a fixed
      `FT_TODAY` so the status bar is deterministic). One golden per
      scene under `ft/src/tui/snapshots/notes_*`:
      - `notes_idle.snap` — idle state with help panel.
      - `notes_help_overlay.snap` — `?` overlay over the idle state.
      - `notes_open_picker.snap` — open picker with a realistic
        multi-hit query (file + heading rows).
      - `notes_move_multiselect.snap` — heading multi-select with one
        explicit and one implicit-via-parent selection visible.
      - `notes_move_compose.snap` — compose view with target headings
        interleaved with two pending items at different levels.
- [ ] End-to-end flow test (using the `tui::tests` harness) that drives
      keys through all four steps of the section-move flow against a
      `TempDir` vault and verifies both files on disk match the expected
      post-move content (full string compare, not snapshot).

## Technical Notes

- `ft_core::notes` is a new module alongside `ft_core::task`. It imports
  `ft_core::markdown::Heading` and `extract_headings` — no duplication.
- **Section ownership**: a heading's section ends at the next heading of
  the same level or higher (lower number = higher level). H2 ends at the
  next H1 or H2; an H3 inside that H2 belongs to it. Mirrors Obsidian's
  fold rule.
- **Level-shift cascade**: every heading inside a section shifts by the
  same delta, including the top heading. So moving an H2 with H3/H4
  children and dropping at level 3 produces H3/H4/H5. The cascade is
  validated *before* writing — if any heading would exceed H6, the whole
  move errors.
- **Disjoint selection in CLI**: enforced by `validate_disjoint`. Parent +
  child combos are an error with both line numbers in the message. In the
  TUI the UX prevents the combination from being constructed (descendants
  show as implicitly selected when a parent is selected, and can't be
  toggled separately).
- **Pair-write ordering**: target first via `fs::write_atomic`, then
  source. If the target write fails, the source is untouched (clean
  failure). If the target write succeeds but the source write fails, the
  user has duplicate content (recoverable by hand). The reverse order
  would risk data loss, which is worse. Document this in the module doc.
- **Same-file move (source == target) is out of scope for v1.** Both CLI
  and TUI error before the picker stage.
- **Obsidian URL**: `obsidian://open?vault=<vault-name>&file=<url-encoded-path>`
  with optional `&heading=<url-encoded-text>`. The vault name is the
  basename of the vault root (`Vault::path.file_name()`); document as
  best-effort and add a `--vault-name` flag escape hatch if users report
  mismatch. The builder lives in `ft_core::notes::obsidian_url` (hoisted
  out of `ft/src/cmd/notes.rs` in session 3) so CLI and TUI share it.
- **Obsidian URL dispatch**: macOS uses `open <url>`; Linux uses
  `xdg-open <url>`. A new `AppRequest::OpenInObsidian { url }` variant
  keeps the TUI flow uniform with the existing `OpenInEditor`. The
  command spawns and doesn't wait — Obsidian raises its own window.
- **State machine**: keep `SectionMoveState` as a single enum on the
  Notes tab, with `handle_event` matching the variant. Don't scatter
  transitions across helper methods — each transition is one place.
- **Compose layout** is a `Vec<ComposeRow>` where
  `ComposeRow { kind: Anchor { line, level } | Pending { idx, level } }`.
  The anchors are static once step 4 opens; reorder/level-shift only
  mutates `Pending` rows. At commit time, the layout is transformed into
  `picks` + `plan` for `move_sections`.
- **Clipboard payload**: the `Composing` state carries
  `clipboard: Vec<ClipboardItem>` where
  `ClipboardItem { source_line: usize, source_text: String, level: u8, body: String }`.
  `source_line` + `source_text` are the freshness-check identity; `body`
  is the extracted section content used for rendering the compose
  preview. At commit, `body` is *not* re-used — the source is re-read
  and `move_sections` re-extracts so the on-disk source is the
  source of truth for what actually moves.
- **Fixture vault** at `tests/fixtures/notes-move/` with:
  - A daily note with multiple H2 sections, some with H3 children.
  - A target project note with existing H1/H2/H3 hierarchy.
  - Edge cases: duplicate heading texts in the source (for match-policy
    tests), a heading at level 5 (for cascade-overflow tests), an empty
    section (heading with no body).

## Future (explicitly out of scope for this plan)

- Multi-source clipboard (accumulating sections from more than one source
  file before composing).
- Link rewriting on file move/rename.
- `ft notes list`, `ft notes create` from template.
- Backlinks pane in the TUI.
- Block-level move (moving content that isn't under a heading).
- Same-file section reorder (moving within one file).

## Sessions

### Session 1 · 2026-05-11 · done
**Goal:** Library primitives. `ft_core::notes` module with `Section`,
`extract_sections`, `shift_section_level`, `validate_disjoint`,
`move_sections`, and `write_pair`. Full unit-test coverage for each
primitive, including cascade-overflow, disjoint validation, and the
multi-section move case. No CLI, no TUI.
**Outcome:** Added `ft-core/src/notes.rs` with the full primitive API:
`Section`, `SectionPick`, `Placement`, `extract_sections`,
`shift_section_level`, `validate_disjoint`, `move_sections`,
`write_pair`. Added `Error::Notes(String)` and registered the module in
`lib.rs`. 32 unit tests cover empty inputs, frontmatter skip, nested
H2/H3 folding, sibling H2s, sub-heading-as-pick (validate_disjoint
rejects parent+child, accepts siblings), cascade shifts in both
directions with H6/H1 overflow, code-fence pseudo-headings preserved
during shift, multi-pick moves with shared placement, top-of-file
insert (`after_line: None`), and append-after-last-heading. Key design
choice: `extract_sections` returns *every* heading (not just top-level)
so any nested heading is a valid pick target — the `body` field still
extends to the next equal-or-higher heading, so an outer H2 contains
its inner H3, and `validate_disjoint` catches the overlap when both
are picked. All workspace tests (286 + integration + the 32 new) pass;
`cargo clippy --workspace --all-targets -- -D warnings` and
`cargo fmt --check` clean.

### Session 2 · 2026-05-11 · done
**Goal:** CLI commands. `ft notes open` (top-hit + `$EDITOR` + `--obsidian`)
and `ft notes move-section` with the full flag set (`--heading`,
`--heading-regex`, `--match-policy`, `--from-query`, `--at-level`,
`--after`, `--yes`). Unified-diff preview, TTY/non-TTY prompt handling.
Integration tests against a `tests/fixtures/notes-move/` vault.
**Outcome:** Added `ft/src/cmd/notes.rs` (`Open` + `MoveSection`
subcommands) wired into `Cli`. `open` resolves `VISUAL`→`EDITOR`→`vi`,
spawns with `+<line>` jump, and prints/dispatches an `obsidian://open`
URL with optional `&heading=` when `--obsidian` is set (gated through
`FT_OBSIDIAN_DRY_RUN=1` for hermetic tests). `move-section` collects
picks via `--heading` (trimmed/case-insensitive) and `--heading-regex`,
disambiguates via `--match-policy first|all|error` (default `error`
lists offending line numbers), resolves source via `--from` or
`--from-query`, places picks via `--after` (same policy) or at the top,
applies `--at-level` with cascade-overflow detection, prints a unified
diff for both files via `similar`, prompts on a TTY (or errors on
piped stdin without `--yes`), then commits with `write_pair`. Switched
the integration tests to per-test `assert_fs::TempDir` vaults (the
pattern already used by `tasks_move.rs`) instead of a shared fixture
directory — more hermetic, no risk of cross-test mutation. Added 6
`notes_open` tests (top hit + line jump, heading line, obsidian URL,
obsidian heading param, no-match → 1, missing query → 2) and 13
`notes_move_section` tests covering the full matrix. Workspace tests
clean (286 lib + 14 integration files all green); `cargo clippy
--workspace --all-targets -- -D warnings` and `cargo fmt --check`
silent. Three workspace dependencies added: `regex` and `urlencoding`
(declared on the `ft` and `ft-core` crates as workspace deps already).

### Session 3 · 2026-05-11 · done
**Goal:** TUI Notes tab skeleton + open flow. Concretely:
1. Hoist `obsidian_url` from `ft/src/cmd/notes.rs` to
   `ft_core::notes::obsidian_url` (keep the CLI helper as a one-line
   delegate; add a focused unit test for the heading-encoding path).
2. Add `AppRequest::OpenInObsidian { url }` variant + a
   `service_request` arm that spawns `open` (macOS) / `xdg-open` (else)
   without suspending the alt screen (Obsidian raises its own window).
3. Create `ft/src/tui/tabs/notes/{mod.rs,view.rs}` with the idle help
   panel and the `?` help overlay.
4. Register `NotesTab` as the third tab in `App::new` *and*
   `App::for_test_with_clock`.
5. Wire the `o` open flow: `FuzzyPicker<VaultFilePickerSource>` (same
   pattern as `tabs/tasks/search.rs`), `Enter` → `OpenInEditor`,
   `Ctrl+O` → `OpenInObsidian`, `Esc` → idle.
6. Snapshots: `notes_idle`, `notes_help_overlay`, `notes_open_picker`.
**Outcome:** Hoisted `obsidian_url` into `ft_core::notes` (new
`urlencoding` dep on `ft-core`, removed from `ft`); the CLI's
`obsidian_url` helper in `ft/src/cmd/notes.rs` collapsed to a one-line
delegate that handles only the vault-root basename fallback. Added
`AppRequest::OpenInObsidian { url }` variant in `ft/src/tui/tab.rs`
and a `service_request` arm in `app.rs` that fires `open` (macOS) /
`xdg-open` (else) without suspending the alt-screen — Obsidian raises
its own window. Created `ft/src/tui/tabs/notes/{mod.rs,view.rs}` with
a `NotesState::{Idle, OpenPicking}` state machine and a tab-local `?`
help overlay; registered the tab in both `App::new` and
`App::for_test_with_clock`. Wired the `o` open flow:
`FuzzyPicker<VaultFilePickerSource>` (same pattern as
`tabs/tasks/search.rs`), `Enter` → `OpenInEditor` (line = hit's
heading line if any, else 1), `Ctrl+O` → `OpenInObsidian` with the
URL built via `ft_core::notes::obsidian_url`, `Esc` → idle. Added 6
focused tests in `tui::tests` (3 snapshots: `notes_idle_80x24`,
`notes_help_overlay_80x24`, `notes_open_picker_80x24`; 3 behavior:
Enter queues `OpenInEditor`, Ctrl+O queues `OpenInObsidian` URL with
correct `vault=` + `file=` params, Esc returns to idle). Updated the
existing `tab_key_cycles_tabs` test for the new 3-tab order
(Welcome/Tasks/Notes). All 17 pre-existing snapshots that included
the tab bar needed re-acceptance (sole diff: new `3 Notes` tab title).
Full workspace test suite passes (289 tests + integration files);
`cargo clippy --workspace --all-targets -- -D warnings` and
`cargo fmt --check` clean.

### Session 4 · 2026-05-11 · done
**Goal:** Section-move flow — steps 1-3 (source pick, heading
multi-select, target pick). Implement `SectionMoveState` enum with
`SourcePicking`, `HeadingMultiSelect`, `TargetPicking` variants and the
transitions between them. Build the `ClipboardItem` payload at the
step-2 → step-3 transition (extract bodies via `extract_sections`).
Same-file target rejection in step 3 with inline footer error. No
compose view yet — `Enter` in `TargetPicking` lands on a placeholder
`Composing` state that immediately falls back to idle with a toast
(`compose view lands in session 5`). Snapshots: source picker, heading
multi-select with mixed explicit/implicit selection, target picker.
**Outcome:** Extended `NotesState` with `MoveSection(SectionMoveState)` and
added the three-variant `SectionMoveState::{SourcePicking,
HeadingMultiSelect, TargetPicking}` plus the `ClipboardItem` payload
(`#[allow(dead_code)]` on the fields session 5 will consume). Key dispatch
split into per-step handlers returning a `MoveAction::{Stay, NotHandled,
Set(Box<NotesState>)}` (the Box is to keep variants similarly sized for
clippy's `large_enum_variant`); `std::mem::take` carries state across
`Esc TargetPicking → HeadingMultiSelect` so prior picks survive a target
re-pick. Implicit-descendant cascade computed on the fly
(`is_implicitly_selected` + `descendant_lines`) so deselecting a parent
restores child idle state with no bookkeeping; `Space` on a focused
heading is a no-op when an ancestor is explicitly picked, with a toast
explaining why. `Enter` in step 2 builds the clipboard via
`extract_sections` (skipping any heading already in an ancestor's
extracted body), sorted by source line. Same-file target pick rejected
inline by stashing a footer error on `TargetPicking`; the error clears
on the next picker keystroke. Successful target `Enter` queues a toast
preview (`compose view lands in session 5 — would move N section(s):
<src> → <dst>`) and returns to idle. View layer: shared
`render_picker_popup` (used by Open/Source/Target with their own
per-step titles + keymap footers), `render_multiselect_popup` with
`■` explicit / `▣` implicit / `□` unselected glyphs and level-tagged
indent. Added 11 new tests in `tui::tests`: 3 snapshots
(`notes_move_source_picker_80x24`, `notes_move_multiselect_80x24`
with mixed explicit/implicit selection, `notes_move_target_picker_80x24`)
and 8 behavior tests covering `m` opens source picker, Esc back to idle,
multi-select renders + cascade + toggle-blocked + Enter-on-empty stays,
Esc back to source picker, Enter advances to target picker, same-file
inline reject, target Enter queues placeholder toast + returns to idle,
target Esc returns to multi-select with picks preserved. Workspace tests
all green (144 TUI + 289 ft_core + integration); `cargo clippy --workspace
--all-targets -- -D warnings` and `cargo fmt --check` clean.

### Session 5 · 2026-05-11 · planned
**Goal:** Compose view + commit. Implement the `Composing` variant with
the interleaved `Vec<ComposeRow>` layout, `Shift+↑/↓` pending-row
reorder, `←/→` level shift with cascade-overflow blocking, and `Enter`
commit. Commit re-reads the source for the freshness check, builds
`picks` + `plan`, calls `move_sections` + `write_pair`, emits the
success toast, and returns to idle. `Esc` returns to step 3 with the
compose layout preserved. Snapshot: `notes_move_compose`. End-to-end
test through all four steps against a `TempDir` vault verifies both
files on disk.
**Outcome:**
