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

- [ ] `Section` struct: `heading: Heading, body: String`. `body` is the
      content from the heading line (inclusive) to the next heading of
      equal-or-higher level (exclusive), or end of file. This matches
      Obsidian's fold behavior and means moving an H2 always drags its
      nested H3/H4/etc with it.
- [ ] `extract_sections(content: &str) -> Vec<Section>` — returns sections
      in document order. Content before the first heading is excluded.
      Frontmatter is skipped (the existing `extract_headings` rule).
- [ ] `shift_section_level(section: &Section, new_top_level: u8) -> Result<String>` —
      returns the section's body with every heading level shifted by
      `new_top_level - section.heading.level`. If any nested heading would
      shift outside the ATX range `1..=6`, returns `Err`. The shift is
      applied only to lines that `extract_headings` recognizes as
      headings (i.e., skips fenced/indented code and frontmatter blocks
      inside the section body — though frontmatter inside a section body
      is exotic and treated like any other line).
- [ ] `validate_disjoint(headings: &[&Heading]) -> Result<()>` — returns
      `Err` if any selected heading is contained within another selected
      heading's section. Identified by line range (not text).
- [ ] `move_sections(source: &str, picks: &[SectionPick], target: &str, plan: &[Placement]) -> Result<(String, String)>` —
      where `SectionPick { line: usize, new_level: u8 }` identifies a
      heading in the source by line number and the target level for the
      move, and `Placement { pick_idx: usize, after_line: Option<usize> }`
      says where in the target each pick goes (`after_line=None` means
      "top of file, before all headings"). Returns
      `(new_source_content, new_target_content)`. Multiple picks may share
      placements that interleave with the target's existing headings.
- [ ] `write_pair(target_path, target_content, source_path, source_content) -> Result<()>` —
      writes target first, then source, each via `fs::write_atomic`. Order
      matters: a crash between the two writes leaves the moved sections
      duplicated (recoverable by hand) rather than lost.

### CLI — `ft notes open`

- [ ] `ft notes open <QUERY>` — runs `Vault::fuzzy_find` and opens the top
      hit. Honors the `file#heading` query syntax. `QUERY` is required;
      passing no query exits 2 with a message pointing at `ft tui`.
- [ ] Opens in `$EDITOR` (resolving `VISUAL` → `EDITOR` → `vi`, same as
      `spawn_editor` in app.rs). When the hit carries a heading, passes
      the heading's line as `+<line>` so the editor jumps to the section.
- [ ] `--obsidian` flag — prints (and `open`s on macOS) an
      `obsidian://open?vault=<name>&file=<url-encoded-path>` URL. When the
      hit has a heading, appends `&heading=<url-encoded-text>` (Obsidian's
      advanced-URI plugin honors it; vanilla Obsidian falls back to
      opening the file). Document this as best-effort.
- [ ] `--editor <bin>` — overrides `$EDITOR` for this invocation.
- [ ] Exit codes: 0 success, 1 no match, 2 bad args / error.

### CLI — `ft notes move-section`

- [ ] Required: `--from <path>`, `--to <path>`, and at least one of
      `--heading TEXT` / `--heading-regex PATTERN` / `--from-query QUERY`.
- [ ] `--heading TEXT` — exact match against heading text (trimmed,
      case-insensitive). Repeatable to pick multiple distinct sections.
- [ ] `--heading-regex PATTERN` — Rust regex matched against heading text.
      Repeatable; results combine with `--heading` results.
- [ ] `--match-policy first|all|error` — what to do when a `--heading` or
      `--heading-regex` matches more than one heading in the source.
      Default: `error` (refuse to write, list the line numbers of the
      ambiguous matches). `first` takes the first match in document order.
      `all` takes every match.
- [ ] `--from-query QUERY` — convenience; uses `file#heading` fuzzy syntax
      via `Vault::fuzzy_find`, picks the top hit. Mutually exclusive with
      `--from` + `--heading`.
- [ ] `--at-level N` — drop the moved sections at heading level `N` in the
      target (the cascade scales nested headings). Default: preserve source
      level. Errors if the cascade would push any nested heading past
      level 6.
- [ ] `--after TEXT` — place moved sections after the named heading in the
      target. Uses the same `--match-policy` for disambiguation. Omitting
      `--after` inserts at the top of the target (before its first
      heading). When multiple sections are moved with one CLI invocation,
      they all share the same insertion point and `--at-level`.
- [ ] Disjoint-section validation: if the picked sections overlap (a
      parent and a child both selected), error with the line numbers of
      the offenders. The TUI prevents this UX-side; the CLI catches it.
- [ ] Same-file move (source path == target path) errors as out of scope.
- [ ] Prints a unified diff of both files to stdout before writing, then
      prompts `Apply? [y/N]`. `--yes` / `-y` skips the prompt. On a non-
      TTY (piped) invocation without `--yes`, exits 2 with a message
      requesting `--yes`.
- [ ] Exit codes: 0 success, 1 nothing matched, 2 bad args / error.

### TUI — Notes tab

- [ ] Registered as the third tab in the tab bar (after Welcome and
      Tasks). Accessible via `Tab` cycling and number keys (e.g. `3`)
      using the existing tab-switch keybinding from plan 002.
- [ ] **Idle state**: a help panel listing the available shortcuts —
      `o` open · `m` move sections · `?` help. No vault listing or
      auto-loaded preview; the panel is the only thing on screen until a
      flow starts. Mirrors the empty-state pattern of the Tasks tab.
- [ ] **Open flow** (`o` key while the Notes tab is focused):
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
- [ ] If the source file changes on disk between step 2 and step 4 (e.g.
      the user edited it externally) and the picked headings no longer
      exist, the commit errors with a toast and the user returns to the
      idle state — no partial write.

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
- [ ] Integration tests for `ft notes open`: top-hit opens correct path
      (captured by overriding `EDITOR` with a recording shim),
      `--obsidian` emits the right URL on stdout when `FT_OBSIDIAN_DRY_RUN=1`
      is set, no-match exits 1, missing query exits 2.
- [ ] Integration tests for `ft notes move-section`:
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
- [ ] Snapshot tests for the Notes tab:
      - Idle state with help panel.
      - Open picker showing realistic vault hits.
      - Heading multi-select with one explicit and one implicit-via-parent
        selection visible.
      - Compose view with target headings interleaved with two pending
        items at different levels.
- [ ] End-to-end flow test (using the `tui::tests` harness) that drives
      keys through all four steps and verifies both files on disk.

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
  mismatch.
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

### Session 1 · 2026-05-11 · planned
**Goal:** Library primitives. `ft_core::notes` module with `Section`,
`extract_sections`, `shift_section_level`, `validate_disjoint`,
`move_sections`, and `write_pair`. Full unit-test coverage for each
primitive, including cascade-overflow, disjoint validation, and the
multi-section move case. No CLI, no TUI.
**Outcome:**

### Session 2 · planned
**Goal:** CLI commands. `ft notes open` (top-hit + `$EDITOR` + `--obsidian`)
and `ft notes move-section` with the full flag set (`--heading`,
`--heading-regex`, `--match-policy`, `--from-query`, `--at-level`,
`--after`, `--yes`). Unified-diff preview, TTY/non-TTY prompt handling.
Integration tests against a `tests/fixtures/notes-move/` vault.
**Outcome:**

### Session 3 · planned
**Goal:** TUI Notes tab skeleton + open flow. Register the Notes tab as
the third tab. Implement the idle help panel. Implement the `o` open
flow: `FuzzyPicker` → `Enter` issues `OpenInEditor`, `Ctrl+O` issues a
new `OpenInObsidian` request (wire the variant + the OS-dispatch handler
in `app.rs::service_request`). Snapshot tests for idle and picker states.
**Outcome:**

### Session 4 · planned
**Goal:** TUI section-move flow. Implement `SectionMoveState` and drive
all four steps: source picker, hierarchical heading multi-select, target
picker, compose view with `Shift+↑/↓` reorder + `←/→` level-shift.
Commit calls `move_sections` + `write_pair` and emits a success toast.
Snapshot tests for each step plus a full end-to-end key-driven test.
**Outcome:**

