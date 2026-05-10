---
id: 006
name: picker-target
title: Wire FuzzyPicker into the new-task target field
status: ready
created: 2026-05-10
updated: 2026-05-10
---

# Wire FuzzyPicker into the new-task target field

## Goal
Replace the plain-text `target` input in plan-004's new-task popup with
the `FuzzyPicker` widget from plan-005, so users can fuzzy-select a file
(and optionally a heading) instead of typing exact paths. The user's
typed query drives the picker; selecting a result fills the field as
`path` or `path#heading text`, and that value drives the same
`Position::Append` / `Position::UnderHeading(...)` semantics the field
already supports — just with discoverability instead of typing.

This plan is a follow-up to plan-004 session 4, which deferred the
picker integration because of an `App` self-borrow problem (the popup
state outlives a single event-loop iteration but the picker held
`&'v Vault` borrowed from the same `App` that owns the popup). Session 1
here flips `App.vault` to `Arc<Vault>` so that goes away; session 2
wires the picker into the popup.

## Motivation and Context
Plan-004's new-task popup ships a `target` text input. Users can type
`Inbox.md#Triage` and it works, but in a real vault with hundreds of
files the typing-from-memory path is brittle:
- You have to remember exact filenames and directory structure
- Heading text needs to match exactly (no fuzzy completion)
- One typo and the create lands in a fresh file or fails

Plan-005 already shipped a generic `FuzzyPicker<S: PickerSource>` widget
with a concrete `VaultFilePickerSource` that runs `Vault::fuzzy_find`
and highlights matched chars. It's snapshot-tested in isolation but has
no caller yet. This plan wires that surface in.

User-driven scoping decisions (from the follow-up discussion):
- **Trigger:** picker opens on Enter OR when the user starts typing in
  the target field. Closer to the original plan-004 wording and makes
  the field effectively pick-only — typing a literal path is rare
  enough that the friction of pressing Esc first is acceptable.
- **Layout:** picker renders as a floating popup-in-popup centered
  over the form. Gives the result list room to breathe; the user
  briefly loses sight of the other fields, but those values are
  preserved.

## Acceptance Criteria

### Arc<Vault> refactor (session 1)
- [ ] `App.vault` becomes `Arc<Vault>`
- [ ] `TabCtx.vault` becomes `&'a Arc<Vault>` (or another shape that
      lets a tab clone the `Arc` without colliding with the rest of
      `TabCtx`'s borrows)
- [ ] `ft/src/tui/cmd/tui.rs` (and any other construction site) wraps
      the discovered `Vault` in `Arc::new(...)` before handing it to
      `App::new`
- [ ] `VaultFilePickerSource` drops its lifetime parameter and holds
      `Arc<Vault>` instead of `&'v Vault`. `VaultFilePickerSource::new`
      takes `Arc<Vault>`.
- [ ] Every existing `&Vault` consumer (vault.scan / vault.path /
      vault.config / `vault.fuzzy_find` etc.) keeps working through
      auto-deref — no signature changes to `ft_core`
- [ ] All existing tests pass unchanged (refactor is behavior-neutral)
- [ ] No new warnings; no clippy regressions

### Target picker wire-up (session 2)
- [ ] `EditPopup` (in `tabs/tasks/search.rs`) gains an
      `Option<FuzzyPicker<VaultFilePickerSource>>` slot — `Some` only
      while the picker is open
- [ ] Trigger:
  - [ ] **Enter** on the target field opens the picker; the picker's
        input is seeded with the field's current text so a partial
        query the user already typed becomes the starting query
  - [ ] **First non-navigation char typed** while target is focused
        and picker is closed: opens the picker, routes the keypress
        as the picker's first input char (so `i` opens the picker
        with `i` already in the input)
  - [ ] Other keys behave as before: Tab/Shift+Tab move focus, Esc
        closes the popup, Backspace/Delete on an empty field
        does nothing (no picker open)
- [ ] While the picker is open, **all** keys go to the picker:
  - [ ] Typing → picker input + live re-query
  - [ ] `↑` / `↓` / `Ctrl+J` / `Ctrl+K` → navigate results
  - [ ] `Enter` → select highlighted hit; fill target field as
        `path` (no heading) or `path#heading text` (with heading);
        close the picker, keep focus on target so the user can
        either tweak the result or Tab onward
  - [ ] `Esc` → close picker, leave target field unchanged
- [ ] Layout: picker renders as a floating popup over the edit
      popup. Centered with reasonable proportions (e.g. 60% wide,
      70% tall of the parent popup area). Backdrop should be a
      `Clear` so the underlying form isn't visible behind it.
- [ ] Match highlighting works as in plan-005 (chars in the matched
      `path` / `heading` are bolded + underlined)
- [ ] Help overlay gains a row noting the picker trigger; the
      existing target-field row is unchanged
- [ ] Tests cover: Enter trigger, first-char trigger, navigation,
      select-fills-target (file-only), select-fills-target
      (file+heading), Esc cancels, picker doesn't open from other
      fields

## Technical Notes

### Why `Arc<Vault>` and not `&'a Vault` through TabCtx
The popup state lives across event-loop iterations. If
`VaultFilePickerSource` held `&'v Vault`, that borrow would need to
outlive the popup — which sits inside `SearchView` inside `TasksTab`
inside `App.tabs`, alongside the `Vault` itself. Rust's borrow checker
doesn't allow a struct to hold both an owned value and a reference to
that same value (self-referential structs need `Pin` / `Rc` /
`ouroboros`). `Arc<Vault>` solves it by sharing ownership cheaply: a
clone is one atomic refcount, and the picker outlives the borrow by
owning its share.

`Vault` itself contains a `LayeredConfig` (which has owned strings and
a `Vec<ConfigSource>`) — it's not `Copy`, so `Arc` is the right
sharing primitive (vs. cloning the whole config each time a picker
opens).

### Why not change the `PickerSource` trait
Option B from the design discussion was to make `PickerSource::query`
take a context parameter (`&Vault`) so the source doesn't hold any
vault state. That works but pushes the ceremony into every
`FuzzyPicker::handle_key`/`render` call site. `Arc<Vault>` is a smaller
change with a wider payoff — any future widget that needs to outlive
a borrow gets the same fix for free.

### How the `path#heading` round-trips
When the picker returns a `Hit`, we format the target field as:
- `<hit.path.display()>` when `hit.heading` is `None`
- `<hit.path.display()>#<hit.heading.text>` when `hit.heading` is some

`submit_popup_new` already calls `ft_core::search::Query::parse` on
the target string and routes `#heading` to `Position::UnderHeading`,
so the round-trip is symmetric: the picker fills the field with the
exact text the user could have typed manually.

### Focus/restore semantics
On `Enter` selection: keep focus on target. Rationale: the user often
wants to tweak the heading suffix or confirm the path before moving
on. They can Tab to advance once they're satisfied.

On `Esc`: leave target field text unchanged. The picker had its own
input buffer; whatever the user typed inside the picker is discarded,
and the field reverts to whatever it held before the picker opened.

### Out of scope for v1
- A persistent picker mode that stays open while the user Tabs
  between fields
- Multi-select (picking several headings at once)
- Inline preview of the file's first few lines
- Configurable trigger keys
- Picker integration with the *quickline* `in:` token — quickline
  stays literal-text-only per plan-004's out-of-scope list

## Sessions

### Session 1 · 2026-05-10 · planned
**Goal:** Arc<Vault> refactor: App.vault becomes Arc<Vault>, TabCtx exposes &Arc<Vault>, VaultFilePickerSource drops its lifetime and holds Arc<Vault>. Behavior-neutral — every existing test passes unchanged.
**Outcome:** 

### Session 2 · 2026-05-10 · planned
**Goal:** FuzzyPicker wire-up on the target field: EditPopup gains target_picker slot, Enter/first-keystroke trigger, all keys route to picker while open, Enter fills target with path[#heading], Esc cancels. Floating popup-in-popup layout. Help overlay row + tests.
**Outcome:** 

