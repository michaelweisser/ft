//! Notes tab — Obsidian-flavoured editing surface.
//!
//! Session 3 (plan 003) wired the tab into the App and added the open
//! flow. Session 4 added steps 1-3 of the section-move flow (source pick →
//! heading multi-select → target pick). Session 5 lands the compose view
//! and commit: an interleaved layout of target anchors + pending picks,
//! per-row level shift, drag-to-reorder, and a final freshness-checked
//! `move_sections` + `write_pair` commit.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ft_core::markdown::{extract_headings, Heading};
use ft_core::notes::{
    extract_sections, move_sections, shift_section_level, write_pair, Placement, Section,
    SectionPick,
};
use ft_core::search::Hit;
use ratatui::{layout::Rect, Frame};

use crate::tui::{
    event::Event,
    tab::{AppRequest, EventOutcome, Tab, TabCtx, ToastStyle},
    widgets::{EditBuffer, FuzzyPicker, PickerOutcome, VaultFilePickerSource},
};

mod view;

/// Top-level state for the Notes tab. Each variant owns the data the
/// corresponding view needs — no shared mutable scratch.
pub enum NotesState {
    /// Default landing surface. Shows the keymap-style help panel; `o`
    /// opens the file picker, `m` enters the section-move flow.
    Idle,
    /// File / heading picker open for the "open in editor / Obsidian"
    /// flow. `Enter` → editor at line 1, `Ctrl+O` → Obsidian URL, `Esc`
    /// → back to idle.
    OpenPicking {
        picker: FuzzyPicker<VaultFilePickerSource>,
    },
    /// Section-move flow (sessions 4 + 5). See [`SectionMoveState`].
    MoveSection(SectionMoveState),
}

/// State machine for the section-move flow. Variants line up 1:1 with
/// the four steps documented in the plan.
pub enum SectionMoveState {
    /// Step 1/4 — pick the source note (or a heading inside one — we
    /// only use the file part of the hit).
    SourcePicking {
        picker: FuzzyPicker<VaultFilePickerSource>,
    },
    /// Step 2/4 — choose which sections to move. `selected` carries the
    /// **explicit** picks by 1-indexed source line number; descendants
    /// are computed on the fly so deselecting a parent restores the
    /// children's idle state without bookkeeping.
    HeadingMultiSelect {
        source_rel: PathBuf,
        source_abs: PathBuf,
        source_content: String,
        headings: Vec<Heading>,
        selected: BTreeSet<usize>,
        focus: usize,
    },
    /// Step 3/4 — pick the target note. The picker's same-file pick is
    /// rejected inline (`error` is shown in the popup footer) and the
    /// state stays put. `headings`/`selected`/`focus` are carried so
    /// `Esc` can rebuild the multi-select with the user's prior choices.
    /// `clipboard` is the extracted-section payload that will feed the
    /// compose view.
    TargetPicking {
        source_rel: PathBuf,
        source_abs: PathBuf,
        source_content: String,
        headings: Vec<Heading>,
        selected: BTreeSet<usize>,
        focus: usize,
        clipboard: Vec<ClipboardItem>,
        picker: FuzzyPicker<VaultFilePickerSource>,
        error: Option<String>,
    },
    /// Step 4/4 — compose the move. The layout interleaves the target's
    /// own headings (as `Anchor` rows, immutable) with the clipboard's
    /// pending picks (as `Pending` rows, reorderable + level-shiftable).
    /// `Enter` commits via [`commit_move`]; `Esc` returns to step 3 with
    /// the layout intact. The step-3 state (`source_*`, `headings`,
    /// `selected`) is carried so the Esc round-trip preserves prior picks.
    Composing {
        source_rel: PathBuf,
        source_abs: PathBuf,
        source_content: String,
        headings: Vec<Heading>,
        selected: BTreeSet<usize>,
        target_rel: PathBuf,
        target_abs: PathBuf,
        target_headings: Vec<Heading>,
        clipboard: Vec<ClipboardItem>,
        layout: Vec<ComposeRow>,
        focus: usize,
        /// Transient sub-mode: while `Some`, the user is typing into a
        /// rename buffer attached to the focused Pending row. `None` is
        /// the normal compose-keymap mode.
        editing: Option<RenameBuffer>,
    },
}

/// Inline rename buffer attached to a Pending row in the compose layout.
/// Owns its own `EditBuffer` so the compose state's level/order is
/// untouched until the user commits with `Enter`.
#[derive(Debug, Clone)]
pub struct RenameBuffer {
    /// Index of the Pending row in `Composing.layout` being renamed.
    pub row_idx: usize,
    /// Single-line text input. Pre-filled with the row's current
    /// effective text on open; commits to `ComposeRow::Pending.rename`
    /// on `Enter`, discards on `Esc`.
    pub buf: EditBuffer,
}

/// One row in the compose layout. Anchor rows are the target's pre-existing
/// headings (read-only, shown for context); Pending rows are the picks
/// awaiting commit (movable, level-shiftable).
#[derive(Debug, Clone)]
pub enum ComposeRow {
    Anchor {
        line: usize,
        level: u8,
        text: String,
    },
    Pending {
        clip_idx: usize,
        level: u8,
        /// `Some(s)` overrides the source heading text at commit time
        /// (threaded into `SectionPick.new_text`). `None` keeps the
        /// source text. Independent of `level` — both can change.
        rename: Option<String>,
    },
}

/// One section pending insertion into the target. Built at the
/// step-2 → step-3 transition from the in-memory source content. The
/// `body` is post-extraction (heading line included, body trimmed at
/// the next equal-or-higher heading) and is used only for the compose
/// preview — the commit re-extracts from a fresh disk read so the body
/// shown isn't the body written.
#[derive(Debug, Clone)]
pub struct ClipboardItem {
    /// 1-indexed source-file line of the heading. The freshness check
    /// at commit time looks the heading up by line number.
    pub source_line: usize,
    /// Original heading text. The freshness check rejects the commit if
    /// the line still has a heading but the text or level changed.
    pub source_text: String,
    /// Original heading level (1-6). The compose layout's `Pending`
    /// rows start at this level; `←/→` shift moves them per row.
    pub level: u8,
    /// The extracted section body — heading line included, trimmed at
    /// the next equal-or-higher heading. Used by the compose preview;
    /// not used by the commit path.
    #[allow(dead_code)]
    pub body: String,
}

pub struct NotesTab {
    state: NotesState,
    /// Whether the tab-local help overlay is showing. Toggled by `?` while
    /// idle; the overlay shadows the help-panel body until dismissed.
    show_help: bool,
}

impl NotesTab {
    pub fn new() -> Self {
        Self {
            state: NotesState::Idle,
            show_help: false,
        }
    }

    fn new_vault_picker(ctx: &TabCtx) -> FuzzyPicker<VaultFilePickerSource> {
        FuzzyPicker::new(VaultFilePickerSource::new(
            Arc::clone(ctx.vault),
            Arc::clone(ctx.recents),
        ))
    }

    fn handle_idle_key(&mut self, k: KeyEvent, ctx: &TabCtx) -> EventOutcome {
        if self.show_help {
            return match k.code {
                KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => {
                    self.show_help = false;
                    EventOutcome::Consumed
                }
                _ => EventOutcome::Consumed,
            };
        }
        match (k.code, k.modifiers) {
            (KeyCode::Char('?'), _) => {
                self.show_help = true;
                EventOutcome::Consumed
            }
            (KeyCode::Char('o'), KeyModifiers::NONE) => {
                self.state = NotesState::OpenPicking {
                    picker: Self::new_vault_picker(ctx),
                };
                EventOutcome::Consumed
            }
            (KeyCode::Char('m'), KeyModifiers::NONE) => {
                self.state = NotesState::MoveSection(SectionMoveState::SourcePicking {
                    picker: Self::new_vault_picker(ctx),
                });
                EventOutcome::Consumed
            }
            _ => EventOutcome::NotHandled,
        }
    }

    fn handle_open_picker_key(&mut self, k: KeyEvent, ctx: &TabCtx) -> EventOutcome {
        let NotesState::OpenPicking { picker } = &mut self.state else {
            return EventOutcome::NotHandled;
        };
        // `Ctrl+O` is our own binding; intercept before handing to picker.
        if k.code == KeyCode::Char('o') && k.modifiers.contains(KeyModifiers::CONTROL) {
            if let Some(item) = picker.selected_item() {
                let hit = item.data.clone();
                request_open_in_obsidian(ctx, &hit);
                self.state = NotesState::Idle;
            }
            return EventOutcome::Consumed;
        }
        match picker.handle_key(k) {
            PickerOutcome::Selected(hit) => {
                request_open_in_editor(ctx, &hit);
                self.state = NotesState::Idle;
                EventOutcome::Consumed
            }
            PickerOutcome::Cancelled => {
                self.state = NotesState::Idle;
                EventOutcome::Consumed
            }
            PickerOutcome::StillOpen => EventOutcome::Consumed,
            PickerOutcome::NotHandled => EventOutcome::NotHandled,
        }
    }

    fn handle_move_key(&mut self, k: KeyEvent, ctx: &TabCtx) -> EventOutcome {
        let NotesState::MoveSection(ms) = &mut self.state else {
            return EventOutcome::NotHandled;
        };
        let next = match ms {
            SectionMoveState::SourcePicking { picker } => handle_source_picker_key(k, picker, ctx),
            SectionMoveState::HeadingMultiSelect {
                source_rel,
                source_abs,
                source_content,
                headings,
                selected,
                focus,
            } => handle_multiselect_key(
                k,
                source_rel,
                source_abs,
                source_content,
                headings,
                selected,
                focus,
                ctx,
            ),
            SectionMoveState::TargetPicking {
                source_rel,
                source_abs,
                source_content,
                headings,
                selected,
                focus,
                clipboard,
                picker,
                error,
            } => handle_target_picker_key(
                k,
                source_rel,
                source_abs,
                source_content,
                headings,
                selected,
                focus,
                clipboard,
                picker,
                error,
                ctx,
            ),
            SectionMoveState::Composing {
                source_rel,
                source_abs,
                source_content,
                headings,
                selected,
                target_rel,
                target_abs,
                target_headings,
                clipboard,
                layout,
                focus,
                editing,
            } => handle_compose_key(
                k,
                source_rel,
                source_abs,
                source_content,
                headings,
                selected,
                target_rel,
                target_abs,
                target_headings,
                clipboard,
                layout,
                focus,
                editing,
                ctx,
            ),
        };
        match next {
            MoveAction::Stay => EventOutcome::Consumed,
            MoveAction::NotHandled => EventOutcome::NotHandled,
            MoveAction::Set(next) => {
                self.state = *next;
                EventOutcome::Consumed
            }
        }
    }
}

impl Tab for NotesTab {
    fn title(&self) -> &str {
        "Notes"
    }

    fn handle_event(&mut self, ev: Event, ctx: &mut TabCtx) -> Result<EventOutcome> {
        let Event::Key(k) = ev else {
            return Ok(EventOutcome::NotHandled);
        };
        let outcome = match &self.state {
            NotesState::Idle => self.handle_idle_key(k, ctx),
            NotesState::OpenPicking { .. } => self.handle_open_picker_key(k, ctx),
            NotesState::MoveSection(_) => self.handle_move_key(k, ctx),
        };
        Ok(outcome)
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &TabCtx) {
        view::render(frame, area, ctx, &mut self.state, self.show_help);
    }
}

/// Outcome of a step-handler: either keep the current state, replace
/// it, or pass on the keypress. Lets the handlers run with `&mut` on
/// individual fields without re-borrowing `self.state`.
enum MoveAction {
    Stay,
    NotHandled,
    Set(Box<NotesState>),
}

fn handle_source_picker_key(
    k: KeyEvent,
    picker: &mut FuzzyPicker<VaultFilePickerSource>,
    ctx: &TabCtx,
) -> MoveAction {
    match picker.handle_key(k) {
        PickerOutcome::Selected(hit) => MoveAction::Set(Box::new(advance_to_multiselect(ctx, hit))),
        PickerOutcome::Cancelled => MoveAction::Set(Box::new(NotesState::Idle)),
        PickerOutcome::StillOpen => MoveAction::Stay,
        PickerOutcome::NotHandled => MoveAction::NotHandled,
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_multiselect_key(
    k: KeyEvent,
    source_rel: &mut PathBuf,
    source_abs: &mut PathBuf,
    source_content: &mut String,
    headings: &mut Vec<Heading>,
    selected: &mut BTreeSet<usize>,
    focus: &mut usize,
    ctx: &TabCtx,
) -> MoveAction {
    match (k.code, k.modifiers) {
        (KeyCode::Esc, _) => MoveAction::Set(Box::new(NotesState::MoveSection(
            SectionMoveState::SourcePicking {
                picker: NotesTab::new_vault_picker(ctx),
            },
        ))),
        (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::NONE) => {
            if *focus > 0 {
                *focus -= 1;
            } else {
                *focus = headings.len().saturating_sub(1);
            }
            MoveAction::Stay
        }
        (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::NONE) => {
            if headings.is_empty() {
                return MoveAction::Stay;
            }
            *focus = (*focus + 1) % headings.len();
            MoveAction::Stay
        }
        (KeyCode::Char(' '), _) => {
            toggle_selection(headings, selected, *focus);
            MoveAction::Stay
        }
        (KeyCode::Enter, _) => {
            if selected.is_empty() {
                queue_toast(ctx, "select at least one heading", ToastStyle::Error);
                return MoveAction::Stay;
            }
            let clipboard = build_clipboard(source_content, headings, selected);
            if clipboard.is_empty() {
                queue_toast(ctx, "no sections extracted", ToastStyle::Error);
                return MoveAction::Stay;
            }
            MoveAction::Set(Box::new(NotesState::MoveSection(
                SectionMoveState::TargetPicking {
                    source_rel: std::mem::take(source_rel),
                    source_abs: std::mem::take(source_abs),
                    source_content: std::mem::take(source_content),
                    headings: std::mem::take(headings),
                    selected: std::mem::take(selected),
                    focus: *focus,
                    clipboard,
                    picker: NotesTab::new_vault_picker(ctx),
                    error: None,
                },
            )))
        }
        _ => MoveAction::NotHandled,
    }
}

#[allow(clippy::too_many_arguments, clippy::ptr_arg)]
fn handle_target_picker_key(
    k: KeyEvent,
    source_rel: &mut PathBuf,
    source_abs: &mut PathBuf,
    source_content: &mut String,
    headings: &mut Vec<Heading>,
    selected: &mut BTreeSet<usize>,
    focus: &mut usize,
    clipboard: &mut Vec<ClipboardItem>,
    picker: &mut FuzzyPicker<VaultFilePickerSource>,
    error: &mut Option<String>,
    ctx: &TabCtx,
) -> MoveAction {
    match picker.handle_key(k) {
        PickerOutcome::Selected(hit) => {
            if hit.path == *source_rel {
                *error = Some("same-file move is out of scope — pick a different target".into());
                MoveAction::Stay
            } else {
                MoveAction::Set(Box::new(advance_to_composing(
                    ctx,
                    std::mem::take(source_rel),
                    std::mem::take(source_abs),
                    std::mem::take(source_content),
                    std::mem::take(headings),
                    std::mem::take(selected),
                    std::mem::take(clipboard),
                    hit,
                )))
            }
        }
        PickerOutcome::Cancelled => MoveAction::Set(Box::new(NotesState::MoveSection(
            SectionMoveState::HeadingMultiSelect {
                source_rel: std::mem::take(source_rel),
                source_abs: std::mem::take(source_abs),
                source_content: std::mem::take(source_content),
                headings: std::mem::take(headings),
                selected: std::mem::take(selected),
                focus: *focus,
            },
        ))),
        PickerOutcome::StillOpen => {
            // Any text-edit / nav keystroke clears a stale "same file" error.
            if error.is_some() {
                *error = None;
            }
            MoveAction::Stay
        }
        PickerOutcome::NotHandled => MoveAction::NotHandled,
    }
}

fn advance_to_multiselect(ctx: &TabCtx, hit: Hit) -> NotesState {
    let abs = ctx.vault.path.join(&hit.path);
    let content = match std::fs::read_to_string(&abs) {
        Ok(s) => s,
        Err(e) => {
            queue_toast(
                ctx,
                &format!("could not read source: {e}"),
                ToastStyle::Error,
            );
            return NotesState::Idle;
        }
    };
    let headings = extract_headings(&content);
    if headings.is_empty() {
        queue_toast(ctx, "source has no headings to move", ToastStyle::Error);
        return NotesState::Idle;
    }
    NotesState::MoveSection(SectionMoveState::HeadingMultiSelect {
        source_rel: hit.path,
        source_abs: abs,
        source_content: content,
        headings,
        selected: BTreeSet::new(),
        focus: 0,
    })
}

/// Build the step-4 (compose) state from the step-3 transition. Reads
/// the target file, extracts its headings, and seeds the layout with
/// the target's anchors followed by the clipboard's pending picks at
/// their original level. On IO error (target unreadable) drops a toast
/// and snaps the user back to idle.
#[allow(clippy::too_many_arguments)]
fn advance_to_composing(
    ctx: &TabCtx,
    source_rel: PathBuf,
    source_abs: PathBuf,
    source_content: String,
    headings: Vec<Heading>,
    selected: BTreeSet<usize>,
    clipboard: Vec<ClipboardItem>,
    target_hit: Hit,
) -> NotesState {
    let target_abs = ctx.vault.path.join(&target_hit.path);
    let target_content = match std::fs::read_to_string(&target_abs) {
        Ok(s) => s,
        Err(e) => {
            queue_toast(
                ctx,
                &format!("could not read target: {e}"),
                ToastStyle::Error,
            );
            return NotesState::Idle;
        }
    };
    let target_headings = extract_headings(&target_content);
    let mut layout: Vec<ComposeRow> = target_headings
        .iter()
        .map(|h| ComposeRow::Anchor {
            line: h.line,
            level: h.level,
            text: h.text.clone(),
        })
        .collect();
    for (idx, item) in clipboard.iter().enumerate() {
        layout.push(ComposeRow::Pending {
            clip_idx: idx,
            level: item.level,
            rename: None,
        });
    }
    // Focus the first pending row so the user lands on something
    // movable. With no anchors the first row is already pending; with
    // some anchors it sits at `target_headings.len()`.
    let focus = target_headings.len().min(layout.len().saturating_sub(1));
    NotesState::MoveSection(SectionMoveState::Composing {
        source_rel,
        source_abs,
        source_content,
        headings,
        selected,
        target_rel: target_hit.path,
        target_abs,
        target_headings,
        clipboard,
        layout,
        focus,
        editing: None,
    })
}

/// Step-4 key dispatcher. Up/Down move focus across the whole layout
/// (Anchor rows are read-only context but still focusable so the user
/// can see where they're inserting); Shift+Up/Down reorders the focused
/// Pending row (Anchor rows can't move); Left/Right shifts the focused
/// Pending row's heading level with cascade-overflow blocking; Enter
/// commits via [`commit_move`]; Esc returns to TargetPicking with the
/// step-3 state carried forward.
#[allow(clippy::too_many_arguments, clippy::ptr_arg)]
fn handle_compose_key(
    k: KeyEvent,
    source_rel: &mut PathBuf,
    source_abs: &mut PathBuf,
    source_content: &mut String,
    headings: &mut Vec<Heading>,
    selected: &mut BTreeSet<usize>,
    target_rel: &mut PathBuf,
    target_abs: &mut PathBuf,
    _target_headings: &mut Vec<Heading>,
    clipboard: &mut Vec<ClipboardItem>,
    layout: &mut Vec<ComposeRow>,
    focus: &mut usize,
    editing: &mut Option<RenameBuffer>,
    ctx: &TabCtx,
) -> MoveAction {
    // Rename buffer is a sub-mode of Composing: when open it consumes
    // every compose key so `r`/`Shift+↑`/`←` etc. don't fire under it.
    if editing.is_some() {
        return handle_rename_buffer_key(k, layout, editing, ctx);
    }
    let shift = k.modifiers.contains(KeyModifiers::SHIFT);
    match (k.code, shift) {
        (KeyCode::Esc, _) => MoveAction::Set(Box::new(NotesState::MoveSection(
            SectionMoveState::TargetPicking {
                source_rel: std::mem::take(source_rel),
                source_abs: std::mem::take(source_abs),
                source_content: std::mem::take(source_content),
                headings: std::mem::take(headings),
                selected: std::mem::take(selected),
                focus: 0,
                clipboard: std::mem::take(clipboard),
                picker: NotesTab::new_vault_picker(ctx),
                error: None,
            },
        ))),
        (KeyCode::Up, false) | (KeyCode::Char('k'), false) => {
            if *focus > 0 {
                *focus -= 1;
            } else {
                *focus = layout.len().saturating_sub(1);
            }
            MoveAction::Stay
        }
        (KeyCode::Down, false) | (KeyCode::Char('j'), false) => {
            if !layout.is_empty() {
                *focus = (*focus + 1) % layout.len();
            }
            MoveAction::Stay
        }
        (KeyCode::Up, true) | (KeyCode::Char('K'), _) => {
            if reorder_pending(layout, *focus, -1) {
                *focus -= 1;
            }
            MoveAction::Stay
        }
        (KeyCode::Down, true) | (KeyCode::Char('J'), _) => {
            if reorder_pending(layout, *focus, 1) {
                *focus += 1;
            }
            MoveAction::Stay
        }
        (KeyCode::Left, _) | (KeyCode::Char('h'), false) => {
            shift_focused_level(layout, clipboard, *focus, -1, ctx);
            MoveAction::Stay
        }
        (KeyCode::Right, _) | (KeyCode::Char('l'), false) => {
            shift_focused_level(layout, clipboard, *focus, 1, ctx);
            MoveAction::Stay
        }
        (KeyCode::Char('r'), false) => {
            open_rename_buffer(layout, clipboard, *focus, editing);
            MoveAction::Stay
        }
        (KeyCode::Enter, _) => {
            commit_move(
                ctx, source_rel, source_abs, target_rel, target_abs, clipboard, layout,
            );
            MoveAction::Set(Box::new(NotesState::Idle))
        }
        _ => MoveAction::NotHandled,
    }
}

/// Open the inline rename buffer on the focused Pending row, pre-filled
/// with that row's current effective text (override if set, otherwise
/// the source heading). No-op on Anchor rows.
fn open_rename_buffer(
    layout: &[ComposeRow],
    clipboard: &[ClipboardItem],
    focus: usize,
    editing: &mut Option<RenameBuffer>,
) {
    let Some(row) = layout.get(focus) else {
        return;
    };
    let ComposeRow::Pending {
        clip_idx, rename, ..
    } = row
    else {
        return;
    };
    let initial = rename
        .as_deref()
        .unwrap_or_else(|| clipboard[*clip_idx].source_text.as_str());
    *editing = Some(RenameBuffer {
        row_idx: focus,
        buf: EditBuffer::from(initial),
    });
}

/// Handle a single key while the rename buffer is open. Printable chars
/// go into the buffer; `Enter` validates + commits into the row;
/// `Esc` discards; everything else is consumed without effect (so
/// `r`/`Shift+↑`/`←` etc. don't leak through to compose-level handlers).
fn handle_rename_buffer_key(
    k: KeyEvent,
    layout: &mut [ComposeRow],
    editing: &mut Option<RenameBuffer>,
    ctx: &TabCtx,
) -> MoveAction {
    let Some(rb) = editing.as_mut() else {
        return MoveAction::NotHandled;
    };
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    match (k.code, ctrl) {
        (KeyCode::Esc, _) => {
            *editing = None;
            MoveAction::Stay
        }
        (KeyCode::Enter, _) => {
            let trimmed = rb.buf.text.trim();
            if trimmed.is_empty() {
                queue_toast(ctx, "rename cannot be empty", ToastStyle::Error);
                return MoveAction::Stay;
            }
            if trimmed.contains('\n') || trimmed.contains('\r') {
                queue_toast(ctx, "rename cannot contain newlines", ToastStyle::Error);
                return MoveAction::Stay;
            }
            let new_text = trimmed.to_string();
            let row_idx = rb.row_idx;
            if let Some(ComposeRow::Pending { rename, .. }) = layout.get_mut(row_idx) {
                *rename = Some(new_text);
            }
            *editing = None;
            MoveAction::Stay
        }
        (KeyCode::Char('w'), true) => {
            rb.buf.delete_word_backward();
            MoveAction::Stay
        }
        (KeyCode::Char(c), false) => {
            rb.buf.insert(c);
            MoveAction::Stay
        }
        (KeyCode::Backspace, _) => {
            rb.buf.backspace();
            MoveAction::Stay
        }
        (KeyCode::Delete, _) => {
            rb.buf.delete();
            MoveAction::Stay
        }
        (KeyCode::Left, _) => {
            rb.buf.left();
            MoveAction::Stay
        }
        (KeyCode::Right, _) => {
            rb.buf.right();
            MoveAction::Stay
        }
        (KeyCode::Home, _) => {
            rb.buf.home();
            MoveAction::Stay
        }
        (KeyCode::End, _) => {
            rb.buf.end();
            MoveAction::Stay
        }
        // Swallow everything else so compose-level keys (`r`, `Shift+↑`,
        // navigation, Enter-modifiers) can't fire under the buffer.
        _ => MoveAction::Stay,
    }
}

/// Move the row at `focus` by `delta` (-1 or +1) within `layout`,
/// constrained to swaps with other `Pending` rows or `Anchor` rows in
/// either direction. Returns true iff a swap happened. Anchor rows at
/// `focus` are immutable (returns false).
fn reorder_pending(layout: &mut [ComposeRow], focus: usize, delta: i32) -> bool {
    if focus >= layout.len() {
        return false;
    }
    if !matches!(layout[focus], ComposeRow::Pending { .. }) {
        return false;
    }
    let target = focus as i32 + delta;
    if target < 0 || target as usize >= layout.len() {
        return false;
    }
    layout.swap(focus, target as usize);
    true
}

/// Shift the focused row's level by `delta`. Anchor rows ignore the
/// keystroke (with a toast); Pending rows clamp at level 1 and bail
/// with an error toast if the cascade would push any nested heading
/// past level 6.
fn shift_focused_level(
    layout: &mut [ComposeRow],
    clipboard: &[ClipboardItem],
    focus: usize,
    delta: i32,
    ctx: &TabCtx,
) {
    let Some(row) = layout.get_mut(focus) else {
        return;
    };
    let (clip_idx, cur_level) = match row {
        ComposeRow::Pending {
            clip_idx, level, ..
        } => (*clip_idx, *level),
        ComposeRow::Anchor { .. } => {
            return;
        }
    };
    let next = cur_level as i32 + delta;
    if next < 1 {
        return;
    }
    if next > 6 {
        queue_toast(ctx, "heading level 6 is the max", ToastStyle::Error);
        return;
    }
    let new_level = next as u8;
    // Dry-run the cascade against the cached body so the user finds out
    // *before* commit that an inner heading would overflow. We rebuild
    // a Section from the clipboard's cached body — its line numbers are
    // garbage but `shift_section_level` only walks levels.
    let item = &clipboard[clip_idx];
    let probe_section = Section {
        heading: Heading {
            line: 1,
            level: item.level,
            text: item.source_text.clone(),
        },
        body: item.body.clone(),
    };
    if let Err(e) = shift_section_level(&probe_section, new_level) {
        queue_toast(
            ctx,
            &format!("cascade would overflow: {e}"),
            ToastStyle::Error,
        );
        return;
    }
    if let ComposeRow::Pending { level, .. } = row {
        *level = new_level;
    }
}

/// Commit the move: re-read source, freshness-check every pick, build
/// `picks` + `plan` from the current layout, call `move_sections` +
/// `write_pair`. Emits a success toast on the happy path, an error
/// toast on any failure. Returning to Idle is the caller's job.
#[allow(clippy::too_many_arguments)]
fn commit_move(
    ctx: &TabCtx,
    source_rel: &Path,
    source_abs: &Path,
    target_rel: &Path,
    target_abs: &Path,
    clipboard: &[ClipboardItem],
    layout: &[ComposeRow],
) {
    let fresh_source = match std::fs::read_to_string(source_abs) {
        Ok(s) => s,
        Err(e) => {
            queue_toast(
                ctx,
                &format!("could not re-read source: {e}"),
                ToastStyle::Error,
            );
            return;
        }
    };
    let fresh_headings = extract_headings(&fresh_source);
    for item in clipboard {
        let still_matches = fresh_headings.iter().any(|h| {
            h.line == item.source_line && h.level == item.level && h.text == item.source_text
        });
        if !still_matches {
            queue_toast(ctx, "source changed on disk — aborted", ToastStyle::Error);
            return;
        }
    }

    let fresh_target = match std::fs::read_to_string(target_abs) {
        Ok(s) => s,
        Err(e) => {
            queue_toast(
                ctx,
                &format!("could not re-read target: {e}"),
                ToastStyle::Error,
            );
            return;
        }
    };

    let (picks, plan) = build_picks_and_plan(layout, clipboard);
    if picks.is_empty() {
        queue_toast(ctx, "no sections to move", ToastStyle::Error);
        return;
    }
    let (new_source, new_target) = match move_sections(&fresh_source, &picks, &fresh_target, &plan)
    {
        Ok(pair) => pair,
        Err(e) => {
            queue_toast(ctx, &format!("move failed: {e}"), ToastStyle::Error);
            return;
        }
    };
    if let Err(e) = write_pair(target_abs, &new_target, source_abs, &new_source) {
        queue_toast(ctx, &format!("write failed: {e}"), ToastStyle::Error);
        return;
    }
    queue_toast(
        ctx,
        &format!(
            "Moved {} section(s): {} → {}",
            picks.len(),
            source_rel.display(),
            target_rel.display(),
        ),
        ToastStyle::Success,
    );
}

/// Walk the layout in order, emitting one `SectionPick` per `Pending`
/// row (in layout order — that's what the user sees) and one
/// `Placement` pointing at the most recently passed `Anchor` (`None`
/// before any anchors, i.e. top of the target). The picks vector is
/// indexed positionally so `plan[i].pick_idx == i`.
fn build_picks_and_plan(
    layout: &[ComposeRow],
    clipboard: &[ClipboardItem],
) -> (Vec<SectionPick>, Vec<Placement>) {
    let mut picks: Vec<SectionPick> = Vec::new();
    let mut plan: Vec<Placement> = Vec::new();
    let mut after_line: Option<usize> = None;
    for row in layout {
        match row {
            ComposeRow::Anchor { line, .. } => {
                after_line = Some(*line);
            }
            ComposeRow::Pending {
                clip_idx,
                level,
                rename,
            } => {
                let item = &clipboard[*clip_idx];
                let pick_idx = picks.len();
                picks.push(SectionPick {
                    source_line: item.source_line,
                    new_level: *level,
                    new_text: rename.clone(),
                });
                plan.push(Placement {
                    pick_idx,
                    after_line,
                });
            }
        }
    }
    (picks, plan)
}

fn queue_toast(ctx: &TabCtx, text: &str, style: ToastStyle) {
    *ctx.pending_request.borrow_mut() = Some(AppRequest::Toast {
        text: text.to_string(),
        style,
    });
}

/// Toggle the explicit selection state of `headings[focus]`. Implicit
/// (ancestor-selected) targets are left alone — the rule the plan
/// spells out is "descendants can't be toggled while the parent is
/// selected". When the user newly selects a parent that has explicit
/// children, those children are demoted to implicit (so the eventual
/// pick list stays disjoint and `validate_disjoint` is happy).
fn toggle_selection(headings: &[Heading], selected: &mut BTreeSet<usize>, focus: usize) {
    if focus >= headings.len() {
        return;
    }
    let line = headings[focus].line;
    if is_implicitly_selected(headings, focus, selected) {
        return;
    }
    if selected.contains(&line) {
        selected.remove(&line);
        return;
    }
    // Newly selecting: clear any explicit descendants — they'll be
    // implicit from now on.
    let descendants = descendant_lines(headings, focus);
    for d in descendants {
        selected.remove(&d);
    }
    selected.insert(line);
}

/// True if any ancestor of `headings[i]` is in `selected`. Walks back
/// up the implicit tree by tracking the smallest level we've yet to
/// pierce — when a heading's level drops below `cur_level`, it's our
/// next ancestor.
pub(crate) fn is_implicitly_selected(
    headings: &[Heading],
    i: usize,
    selected: &BTreeSet<usize>,
) -> bool {
    if i >= headings.len() {
        return false;
    }
    let mut cur_level = headings[i].level;
    for h in headings[..i].iter().rev() {
        if h.level < cur_level {
            if selected.contains(&h.line) {
                return true;
            }
            cur_level = h.level;
            if cur_level == 1 {
                break;
            }
        }
    }
    false
}

/// 1-indexed source-file line numbers for every descendant of
/// `headings[i]`. Used when newly selecting a parent so explicit
/// children get demoted to implicit.
fn descendant_lines(headings: &[Heading], i: usize) -> Vec<usize> {
    if i >= headings.len() {
        return Vec::new();
    }
    let level = headings[i].level;
    let mut out = Vec::new();
    for h in headings[i + 1..].iter() {
        if h.level <= level {
            break;
        }
        out.push(h.line);
    }
    out
}

/// Pull the picked sections out of `source_content`, returning a
/// clipboard entry per explicit pick (in document order). Uses
/// `extract_sections` so the body bounds match what `move_sections`
/// will compute at commit time.
fn build_clipboard(
    source_content: &str,
    headings: &[Heading],
    selected: &BTreeSet<usize>,
) -> Vec<ClipboardItem> {
    let sections = extract_sections(source_content);
    let mut items: Vec<ClipboardItem> = headings
        .iter()
        .filter(|h| selected.contains(&h.line))
        .filter_map(|h| {
            sections
                .iter()
                .find(|s| s.heading.line == h.line)
                .map(|s| ClipboardItem {
                    source_line: h.line,
                    source_text: h.text.clone(),
                    level: h.level,
                    body: s.body.clone(),
                })
        })
        .collect();
    items.sort_by_key(|c| c.source_line);
    items
}

fn request_open_in_editor(ctx: &TabCtx, hit: &Hit) {
    let abs = ctx.vault.path.join(&hit.path);
    let line = hit.heading.as_ref().map(|h| h.line).unwrap_or(1);
    // Record the open before raising the AppRequest so a subsequent
    // picker invocation surfaces this note at the top of recents.
    // `record_open` is best-effort and never errors.
    ctx.recents.record_open(&hit.path);
    *ctx.pending_request.borrow_mut() = Some(AppRequest::OpenInEditor { path: abs, line });
}

fn request_open_in_obsidian(ctx: &TabCtx, hit: &Hit) {
    let vault_name = ctx
        .vault
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "vault".to_string());
    let url = ft_core::notes::obsidian_url(&vault_name, &hit.path, hit.heading.as_ref());
    ctx.recents.record_open(&hit.path);
    *ctx.pending_request.borrow_mut() = Some(AppRequest::OpenInObsidian { url });
}
