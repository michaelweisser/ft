//! Notes tab — Obsidian-flavoured editing surface.
//!
//! Session 3 (plan 003) wired the tab into the App and added the open
//! flow. Session 4 adds the first three steps of the section-move flow
//! (source pick → heading multi-select → target pick); the compose view
//! lands in session 5, so for now `Enter` on a valid target queues a
//! toast and drops back to idle.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ft_core::markdown::{extract_headings, Heading};
use ft_core::notes::extract_sections;
use ft_core::search::Hit;
use ratatui::{layout::Rect, Frame};

use crate::tui::{
    event::Event,
    tab::{AppRequest, EventOutcome, Tab, TabCtx, ToastStyle},
    widgets::{FuzzyPicker, PickerOutcome, VaultFilePickerSource},
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
/// the four steps documented in the plan; session 4 ships the first
/// three and Enter on a valid target step-3 selection short-circuits
/// to a toast (Composing lands in session 5).
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
    /// compose view in session 5.
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
}

/// One section pending insertion into the target. Built at the
/// step-2 → step-3 transition from the in-memory source content. The
/// `body` is post-extraction (heading line included, body trimmed at
/// the next equal-or-higher heading); session 5 will re-shift bodies
/// at commit time, so the cached value is for rendering, not writing.
///
/// Most fields are unread until session 5 lands the compose view —
/// `source_line` is used by the toast preview today, the rest exist
/// so the step-2 → step-3 transition has somewhere to land its
/// extracted payload.
#[allow(dead_code)]
pub struct ClipboardItem {
    pub source_line: usize,
    pub source_text: String,
    pub level: u8,
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
        FuzzyPicker::new(VaultFilePickerSource::new(Arc::clone(ctx.vault)))
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

#[allow(clippy::too_many_arguments)]
fn handle_target_picker_key(
    k: KeyEvent,
    source_rel: &mut PathBuf,
    source_abs: &mut PathBuf,
    source_content: &mut String,
    headings: &mut Vec<Heading>,
    selected: &mut BTreeSet<usize>,
    focus: &mut usize,
    clipboard: &mut [ClipboardItem],
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
                let count = clipboard.len();
                let src = source_rel.display().to_string();
                let dst = hit.path.display().to_string();
                queue_toast(
                    ctx,
                    &format!(
                        "compose view lands in session 5 — would move {count} section(s): {src} → {dst}"
                    ),
                    ToastStyle::Success,
                );
                MoveAction::Set(Box::new(NotesState::Idle))
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
    *ctx.pending_request.borrow_mut() = Some(AppRequest::OpenInObsidian { url });
}
