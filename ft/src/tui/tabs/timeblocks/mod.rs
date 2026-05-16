//! Timeblocks tab — read-only today + tomorrow view (plan 015 session 4).
//!
//! Layout: sidebar (24 cols) + main split horizontally between **Today**
//! and **Tomorrow** panes (50/50). The sidebar shows a live clock, today's
//! date, and per-top-level-tag totals for today's blocks. Each pane has
//! its own selection cursor; `h`/`l` (or `←`/`→`) toggles pane focus,
//! `j`/`k` / `↓`/`↑` move within the focused pane, `g`/`G` jump
//! first/last, `r` re-reads both days. (Tab and Shift+Tab are reserved
//! for the App's global tab-cycle, so we deliberately don't shadow them
//! here — see plan 015 session 4 outcome for the rationale.)
//!
//! Mutations land in session 5 — this session is read-only, so the tab
//! never writes to disk.

use std::path::PathBuf;

use anyhow::Result;
use chrono::{DateTime, Local, NaiveTime, Timelike};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ft_core::timeblock::{
    self,
    doc::Document,
    ops::{self, AddOptions, EditMutation, Selector, TimeChange},
    Timeblock,
};
use ratatui::{layout::Rect, Frame};

use crate::tui::{
    event::Event,
    tab::{AppRequest, EventOutcome, Tab, TabCtx, ToastStyle},
    widgets::EditBuffer,
};

mod view;

/// Function pointer for "what time is it now?". Production uses
/// [`Local::now`]; tests inject a fixed value for deterministic snapshots.
pub type ClockFn = fn() -> DateTime<Local>;

fn local_now() -> DateTime<Local> {
    Local::now()
}

/// Sidebar width matches the Tasks tab so the column stays aligned when
/// the user switches tabs mid-session.
pub(crate) const SIDEBAR_WIDTH: u16 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pane {
    Today,
    Tomorrow,
}

/// Per-pane state. `path` is the resolved daily-note path (None when
/// `[periodic_notes.daily]` isn't configured — both panes share the
/// same not-configured state then). `present` distinguishes "file
/// missing on disk" (renders the placeholder) from "file exists but
/// section is empty" (renders an empty list).
pub(crate) struct PaneState {
    pub date: chrono::NaiveDate,
    /// Resolved daily-note path. Read in session 5 by the `c` chord
    /// (create-tomorrow via the daily-note template).
    #[allow(dead_code)]
    pub path: Option<PathBuf>,
    pub present: bool,
    pub blocks: Vec<Timeblock>,
    pub selection: usize,
}

impl PaneState {
    fn empty(date: chrono::NaiveDate) -> Self {
        Self {
            date,
            path: None,
            present: false,
            blocks: Vec::new(),
            selection: 0,
        }
    }
}

/// Editing mode the tab is currently in. `Idle` is the default; the
/// other variants own the buffers / focus targets the corresponding
/// keymaps need. `DeleteConfirm` is a two-stroke chord: first `d`
/// transitions Idle → DeleteConfirm, second `d` commits and returns to
/// Idle.
pub(crate) enum Mode {
    Idle,
    /// First `d` of the `d d` delete chord. Holds the pane + selected
    /// block index captured at chord start so the commit isn't shifted
    /// by an intervening selection move.
    DeleteConfirm {
        pane: Pane,
        block_idx: usize,
    },
    /// `a` quickline open. Buffer captures a blockstring to parse.
    Quickline(EditBuffer),
    /// `e` inline description edit. The pane + block index identify which
    /// block is being edited; the buffer holds the new desc.
    EditDesc {
        pane: Pane,
        block_idx: usize,
        buf: EditBuffer,
    },
    /// `A` modal form for entering a block via three rows.
    Form(FormState),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FormField {
    Start,
    End,
    Desc,
}

pub(crate) struct FormState {
    pub start: EditBuffer,
    pub end: EditBuffer,
    pub desc: EditBuffer,
    pub focus: FormField,
}

pub struct TimeblocksTab {
    pub(crate) clock: ClockFn,
    pub(crate) today: PaneState,
    pub(crate) tomorrow: PaneState,
    pub(crate) focus: Pane,
    pub(crate) mode: Mode,
    /// Heading the panes were last loaded under. Refresh is cheap so we
    /// could read this from `ctx.vault.config` every render, but caching
    /// it removes an allocation on the hot path.
    pub(crate) heading: String,
    /// Most recent load error (e.g. malformed file). Surfaced in the
    /// status-bar via a Toast in session 5; for now we expose it via the
    /// test API.
    #[allow(dead_code)]
    pub(crate) last_error: Option<String>,
}

impl TimeblocksTab {
    pub fn new() -> Self {
        Self::with_clock(local_now)
    }

    pub fn with_clock(clock: ClockFn) -> Self {
        let now = (clock)().date_naive();
        Self {
            clock,
            today: PaneState::empty(now),
            tomorrow: PaneState::empty(now + chrono::Duration::days(1)),
            focus: Pane::Today,
            mode: Mode::Idle,
            heading: "Time Blocks".into(),
            last_error: None,
        }
    }

    /// Re-read both days from disk. Called from `on_focus` and `r`.
    ///
    /// Preserves each pane's selection *index*. That's the right policy
    /// for the read-only refresh path (`r` on a file that didn't change
    /// keeps the cursor where it was, on a file that did change the
    /// clamp falls back to the last block). Mutation chords that move
    /// a block's start time (which can re-sort the list) re-anchor by
    /// start time via [`Self::select_by_start`] after this call.
    fn reload(&mut self, ctx: &mut TabCtx) {
        self.heading = ctx.vault.config.config.timeblocks_heading().to_string();
        let today = ctx.today;
        let tomorrow = today + chrono::Duration::days(1);
        let prev_today_sel = self.today.selection;
        let prev_tomorrow_sel = self.tomorrow.selection;
        self.today = self.load_pane(ctx, today);
        self.tomorrow = self.load_pane(ctx, tomorrow);
        self.today.selection = prev_today_sel;
        self.tomorrow.selection = prev_tomorrow_sel;
        self.clamp_selection();
    }

    fn load_pane(&mut self, ctx: &TabCtx, date: chrono::NaiveDate) -> PaneState {
        let path = match ctx.vault.resolve_target(date, None) {
            Ok(p) => Some(p),
            Err(e) => {
                self.last_error = Some(format!("{e}"));
                return PaneState::empty(date);
            }
        };
        let exists = path.as_ref().map(|p| p.exists()).unwrap_or(false);
        if !exists {
            return PaneState {
                date,
                path,
                present: false,
                blocks: Vec::new(),
                selection: 0,
            };
        }
        let p = path.as_ref().unwrap();
        match Document::read(p, &self.heading) {
            Ok(doc) => PaneState {
                date,
                path: path.clone(),
                present: true,
                blocks: doc.blocks,
                selection: 0,
            },
            Err(e) => {
                self.last_error = Some(format!("{e}"));
                PaneState {
                    date,
                    path,
                    present: true,
                    blocks: Vec::new(),
                    selection: 0,
                }
            }
        }
    }

    /// After a mutation that changes a block's start time, the
    /// post-sort index of "the block I just edited" may differ from
    /// the pre-mutation index. This helper finds the block whose start
    /// matches `start` and sets the focused-pane selection to its
    /// index, so the cursor follows the user's intent through the sort.
    fn select_by_start(&mut self, pane: Pane, start: NaiveTime) {
        let p = self.pane_mut(pane);
        if let Some(idx) = p.blocks.iter().position(|b| b.start == start) {
            p.selection = idx;
        }
    }

    fn clamp_selection(&mut self) {
        for pane in [&mut self.today, &mut self.tomorrow] {
            if pane.blocks.is_empty() {
                pane.selection = 0;
            } else if pane.selection >= pane.blocks.len() {
                pane.selection = pane.blocks.len() - 1;
            }
        }
    }

    fn pane_mut(&mut self, p: Pane) -> &mut PaneState {
        match p {
            Pane::Today => &mut self.today,
            Pane::Tomorrow => &mut self.tomorrow,
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let pane = self.pane_mut(self.focus);
        let len = pane.blocks.len();
        if len == 0 {
            return;
        }
        let cur = pane.selection as isize;
        let new = (cur + delta).clamp(0, (len as isize) - 1);
        pane.selection = new as usize;
    }

    fn jump_selection(&mut self, to_end: bool) {
        let pane = self.pane_mut(self.focus);
        if pane.blocks.is_empty() {
            return;
        }
        pane.selection = if to_end { pane.blocks.len() - 1 } else { 0 };
    }

    fn toggle_focus(&mut self, forward: bool) {
        self.focus = match (self.focus, forward) {
            (Pane::Today, _) => Pane::Tomorrow,
            (Pane::Tomorrow, _) => Pane::Today,
        };
    }

    fn handle_key(&mut self, key: KeyEvent) -> EventOutcome {
        // Tab / Shift+Tab are deliberately NOT consumed here — they belong
        // to the App's global tab-cycle. `h`/`l` (or `←`/`→`) toggle pane
        // focus instead.
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_selection(1);
                EventOutcome::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_selection(-1);
                EventOutcome::Consumed
            }
            KeyCode::Char('g') => {
                self.jump_selection(false);
                EventOutcome::Consumed
            }
            KeyCode::Char('G') => {
                self.jump_selection(true);
                EventOutcome::Consumed
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.toggle_focus(false);
                EventOutcome::Consumed
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.toggle_focus(true);
                EventOutcome::Consumed
            }
            KeyCode::Char('r') => {
                // Refresh happens in handle_event so the ctx is available.
                EventOutcome::NotHandled
            }
            _ => EventOutcome::NotHandled,
        }
    }

    // ── mutation chord handlers ────────────────────────────────────────

    fn selected_block_idx(&self, pane: Pane) -> Option<usize> {
        let p = match pane {
            Pane::Today => &self.today,
            Pane::Tomorrow => &self.tomorrow,
        };
        if p.blocks.is_empty() {
            None
        } else {
            Some(p.selection)
        }
    }

    fn pane_path(&self, pane: Pane) -> Option<PathBuf> {
        match pane {
            Pane::Today => self.today.path.clone(),
            Pane::Tomorrow => self.tomorrow.path.clone(),
        }
    }

    /// Run a time-shift edit on the focused pane's selected block.
    /// `which == 'start'` → shifts start; otherwise shifts end. Negative
    /// values move earlier. Library clamps at 00:00 / 23:59 and enforces
    /// `end > start`.
    fn shift_block_time(&mut self, ctx: &mut TabCtx, shift_minutes: i32, on_end: bool) {
        let pane = self.focus;
        let Some(idx) = self.selected_block_idx(pane) else {
            return;
        };
        let Some(path) = self.pane_path(pane) else {
            queue_toast(ctx, "no daily-note path resolved", ToastStyle::Error);
            return;
        };
        let p = match pane {
            Pane::Today => &self.today,
            Pane::Tomorrow => &self.tomorrow,
        };
        let block = &p.blocks[idx];
        let old_start = block.start;
        let mutation = if on_end {
            EditMutation {
                end: Some(TimeChange::ShiftMinutes(shift_minutes)),
                ..Default::default()
            }
        } else {
            EditMutation {
                start: Some(TimeChange::ShiftMinutes(shift_minutes)),
                ..Default::default()
            }
        };
        let selector = Selector::Time(old_start);
        match ops::edit_block(&path, &self.heading, &selector, mutation) {
            Ok(_) => {
                self.reload(ctx);
                // End-shift leaves the start unchanged, so the (now
                // preserved) selection index already points at the same
                // block. A start-shift can move the block in the sorted
                // list — re-anchor by the new start time so the cursor
                // tracks the user's intent across the re-sort.
                let new_start = if on_end {
                    old_start
                } else {
                    shift_clamped(old_start, shift_minutes)
                };
                self.select_by_start(pane, new_start);
            }
            Err(e) => queue_toast(ctx, &format!("{e}"), ToastStyle::Error),
        }
    }

    fn shift_end(&mut self, ctx: &mut TabCtx, m: i32) {
        self.shift_block_time(ctx, m, true);
    }

    fn shift_start(&mut self, ctx: &mut TabCtx, m: i32) {
        self.shift_block_time(ctx, m, false);
    }

    /// `c` chord — when the focused pane's daily note doesn't yet exist,
    /// create it via `create_or_get_periodic_path` and re-read. Otherwise
    /// toast "already exists".
    fn handle_create_daily(&mut self, ctx: &mut TabCtx) {
        let pane = self.focus;
        let date = match pane {
            Pane::Today => self.today.date,
            Pane::Tomorrow => self.tomorrow.date,
        };
        let already_present = match pane {
            Pane::Today => self.today.present,
            Pane::Tomorrow => self.tomorrow.present,
        };
        if already_present {
            queue_toast(ctx, "daily note already exists", ToastStyle::Info);
            return;
        }
        let Some(daily_cfg) = ctx.vault.config.config.periodic_notes.daily.as_ref() else {
            queue_toast(
                ctx,
                "no `[periodic_notes.daily]` configured",
                ToastStyle::Error,
            );
            return;
        };
        let (today_n, now_n) = today_now_for_template(ctx, self.clock);
        match ft_core::periodic::create_or_get_periodic_path(
            &ctx.vault.path,
            &ctx.vault.templates_dir(),
            daily_cfg,
            date,
            today_n,
            now_n,
        ) {
            Ok((_path, _created)) => {
                queue_toast(
                    ctx,
                    &format!("created daily note for {date}"),
                    ToastStyle::Success,
                );
                self.reload(ctx);
            }
            Err(e) => queue_toast(ctx, &format!("{e}"), ToastStyle::Error),
        }
    }

    /// First `d` of the `d d` chord. Captures the focused selection so
    /// subsequent navigation doesn't shift the delete target. Toasts
    /// the inter-stroke hint.
    fn start_delete_confirm(&mut self, ctx: &mut TabCtx) {
        let pane = self.focus;
        let Some(idx) = self.selected_block_idx(pane) else {
            queue_toast(ctx, "nothing to delete", ToastStyle::Info);
            return;
        };
        self.mode = Mode::DeleteConfirm {
            pane,
            block_idx: idx,
        };
        queue_toast(
            ctx,
            "press `d` again to delete, Esc to cancel",
            ToastStyle::Info,
        );
    }

    fn handle_delete_confirm(&mut self, k: KeyEvent, ctx: &mut TabCtx) -> EventOutcome {
        let Mode::DeleteConfirm { pane, block_idx } = self.mode else {
            return EventOutcome::NotHandled;
        };
        match k.code {
            KeyCode::Char('d') if k.modifiers == KeyModifiers::NONE => {
                self.mode = Mode::Idle;
                self.commit_delete(ctx, pane, block_idx);
                EventOutcome::Consumed
            }
            KeyCode::Esc => {
                self.mode = Mode::Idle;
                queue_toast(ctx, "delete cancelled", ToastStyle::Info);
                EventOutcome::Consumed
            }
            _ => {
                // Any other key cancels the chord — matches the
                // tasks-tab convention so an accidental `j`/`k` doesn't
                // silently arm the deletion.
                self.mode = Mode::Idle;
                EventOutcome::Consumed
            }
        }
    }

    fn commit_delete(&mut self, ctx: &mut TabCtx, pane: Pane, block_idx: usize) {
        let Some(path) = self.pane_path(pane) else {
            queue_toast(ctx, "no daily-note path resolved", ToastStyle::Error);
            return;
        };
        let p = match pane {
            Pane::Today => &self.today,
            Pane::Tomorrow => &self.tomorrow,
        };
        if block_idx >= p.blocks.len() {
            queue_toast(ctx, "block no longer exists", ToastStyle::Error);
            return;
        }
        let target = p.blocks[block_idx].clone();
        let selector = Selector::Time(target.start);
        match ops::delete_block(&path, &self.heading, &selector) {
            Ok(_) => {
                queue_toast(
                    ctx,
                    &format!(
                        "deleted {} - {} {}",
                        fmt_hhmm(target.start),
                        fmt_hhmm(target.end),
                        target.desc
                    ),
                    ToastStyle::Success,
                );
                // Aim the cursor at the block that took the deleted
                // one's slot. `reload` preserves the index and
                // `clamp_selection` brings it down when we removed the
                // tail — but pinning it here also handles the case
                // where `block_idx` happens to be 0 (no clamp needed
                // but we'd otherwise stay at 0 anyway).
                self.pane_mut(pane).selection = block_idx;
                self.reload(ctx);
            }
            Err(e) => queue_toast(ctx, &format!("{e}"), ToastStyle::Error),
        }
    }

    // ── quickline (`a`) ────────────────────────────────────────────────

    fn handle_quickline(&mut self, k: KeyEvent, ctx: &mut TabCtx) -> EventOutcome {
        let Mode::Quickline(buf) = &mut self.mode else {
            return EventOutcome::NotHandled;
        };
        match k.code {
            KeyCode::Esc => {
                self.mode = Mode::Idle;
                EventOutcome::Consumed
            }
            KeyCode::Enter => {
                let input = buf.text.clone();
                self.commit_quickline(ctx, &input);
                EventOutcome::Consumed
            }
            KeyCode::Backspace => {
                buf.backspace();
                EventOutcome::Consumed
            }
            KeyCode::Delete => {
                buf.delete();
                EventOutcome::Consumed
            }
            KeyCode::Left => {
                buf.left();
                EventOutcome::Consumed
            }
            KeyCode::Right => {
                buf.right();
                EventOutcome::Consumed
            }
            KeyCode::Home => {
                buf.home();
                EventOutcome::Consumed
            }
            KeyCode::End => {
                buf.end();
                EventOutcome::Consumed
            }
            KeyCode::Char(c) => {
                buf.insert(c);
                EventOutcome::Consumed
            }
            _ => EventOutcome::Consumed,
        }
    }

    fn commit_quickline(&mut self, ctx: &mut TabCtx, input: &str) {
        let pane = self.focus;
        let Some(path) = self.pane_path(pane) else {
            queue_toast(ctx, "no daily-note path resolved", ToastStyle::Error);
            return;
        };
        let block = match timeblock::parse_line(input) {
            Ok(b) => b,
            Err(e) => {
                // Keep the buffer populated so the user can fix the input.
                queue_toast(ctx, &format!("parse: {e}"), ToastStyle::Error);
                return;
            }
        };
        let summary = format!(
            "+ {} - {} {}",
            fmt_hhmm(block.start),
            fmt_hhmm(block.end),
            block.desc.trim()
        );
        // The daily note might be missing on disk — same behavior as
        // CLI `ft timeblocks add`: render the template first.
        if let Err(e) = self.ensure_pane_file(ctx, pane) {
            queue_toast(ctx, &format!("{e}"), ToastStyle::Error);
            return;
        }
        let new_start = block.start;
        match ops::add_block(&path, &self.heading, block, AddOptions::default()) {
            Ok(_) => {
                self.mode = Mode::Idle;
                queue_toast(ctx, &summary, ToastStyle::Success);
                self.reload(ctx);
                // Pin the cursor to the freshly added block so the next
                // chord (`]`, `[`, `e`, …) operates on what the user
                // just typed, rather than wherever they last navigated.
                self.select_by_start(pane, new_start);
            }
            Err(e) => queue_toast(ctx, &format!("{e}"), ToastStyle::Error),
        }
    }

    /// Render the daily-note template for the focused pane's date when
    /// the file is missing. No-op when the file already exists or when
    /// `[periodic_notes.daily]` isn't configured (the subsequent write
    /// will create the file with just the section heading — same as the
    /// pre-session-5 behavior, surfaced via the existing remedy hint).
    fn ensure_pane_file(&self, ctx: &mut TabCtx, pane: Pane) -> Result<()> {
        let (date, present, path) = match pane {
            Pane::Today => (self.today.date, self.today.present, self.today.path.clone()),
            Pane::Tomorrow => (
                self.tomorrow.date,
                self.tomorrow.present,
                self.tomorrow.path.clone(),
            ),
        };
        if present {
            return Ok(());
        }
        let Some(_path) = path else { return Ok(()) };
        let Some(daily_cfg) = ctx.vault.config.config.periodic_notes.daily.as_ref() else {
            return Ok(());
        };
        let (today_n, now_n) = today_now_for_template(ctx, self.clock);
        ft_core::periodic::create_or_get_periodic_path(
            &ctx.vault.path,
            &ctx.vault.templates_dir(),
            daily_cfg,
            date,
            today_n,
            now_n,
        )
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("{e}"))
    }

    // ── edit description (`e`) ─────────────────────────────────────────

    fn start_edit_desc(&mut self, ctx: &mut TabCtx) {
        let pane = self.focus;
        let Some(idx) = self.selected_block_idx(pane) else {
            queue_toast(ctx, "nothing to edit", ToastStyle::Info);
            return;
        };
        let block = match pane {
            Pane::Today => &self.today.blocks[idx],
            Pane::Tomorrow => &self.tomorrow.blocks[idx],
        };
        self.mode = Mode::EditDesc {
            pane,
            block_idx: idx,
            buf: EditBuffer::from(&block.desc),
        };
    }

    fn handle_edit_desc(&mut self, k: KeyEvent, ctx: &mut TabCtx) -> EventOutcome {
        let Mode::EditDesc {
            pane,
            block_idx,
            buf,
        } = &mut self.mode
        else {
            return EventOutcome::NotHandled;
        };
        match k.code {
            KeyCode::Esc => {
                self.mode = Mode::Idle;
                EventOutcome::Consumed
            }
            KeyCode::Enter => {
                let new_desc = buf.text.clone();
                let pane = *pane;
                let block_idx = *block_idx;
                self.commit_edit_desc(ctx, pane, block_idx, new_desc);
                EventOutcome::Consumed
            }
            KeyCode::Backspace => {
                buf.backspace();
                EventOutcome::Consumed
            }
            KeyCode::Delete => {
                buf.delete();
                EventOutcome::Consumed
            }
            KeyCode::Left => {
                buf.left();
                EventOutcome::Consumed
            }
            KeyCode::Right => {
                buf.right();
                EventOutcome::Consumed
            }
            KeyCode::Home => {
                buf.home();
                EventOutcome::Consumed
            }
            KeyCode::End => {
                buf.end();
                EventOutcome::Consumed
            }
            KeyCode::Char(c) => {
                buf.insert(c);
                EventOutcome::Consumed
            }
            _ => EventOutcome::Consumed,
        }
    }

    fn commit_edit_desc(
        &mut self,
        ctx: &mut TabCtx,
        pane: Pane,
        block_idx: usize,
        new_desc: String,
    ) {
        let Some(path) = self.pane_path(pane) else {
            queue_toast(ctx, "no daily-note path resolved", ToastStyle::Error);
            return;
        };
        let p = match pane {
            Pane::Today => &self.today,
            Pane::Tomorrow => &self.tomorrow,
        };
        if block_idx >= p.blocks.len() {
            queue_toast(ctx, "block no longer exists", ToastStyle::Error);
            return;
        }
        let target = p.blocks[block_idx].clone();
        let selector = Selector::Time(target.start);
        let mutation = EditMutation {
            desc: Some(new_desc),
            ..Default::default()
        };
        match ops::edit_block(&path, &self.heading, &selector, mutation) {
            Ok(_) => {
                self.mode = Mode::Idle;
                self.reload(ctx);
                // Desc edits don't touch the block's start time, but
                // anchor by start anyway to make the invariant — "the
                // block you just edited stays selected" — uniform across
                // every mutation chord.
                self.select_by_start(pane, target.start);
            }
            Err(e) => queue_toast(ctx, &format!("{e}"), ToastStyle::Error),
        }
    }

    // ── form (`A`) ─────────────────────────────────────────────────────

    fn default_form(&self) -> FormState {
        let now = (self.clock)();
        // Snap clock time to the nearest 5-minute boundary.
        let total = now.hour() * 60 + now.minute();
        let snapped = (total / 5) * 5;
        let start = NaiveTime::from_hms_opt(snapped / 60, snapped % 60, 0).unwrap();
        let end = start + chrono::Duration::minutes(30);
        FormState {
            start: EditBuffer::from(&fmt_hhmm(start)),
            end: EditBuffer::from(&fmt_hhmm(end)),
            desc: EditBuffer::default(),
            focus: FormField::Start,
        }
    }

    fn handle_form(&mut self, k: KeyEvent, ctx: &mut TabCtx) -> EventOutcome {
        let Mode::Form(state) = &mut self.mode else {
            return EventOutcome::NotHandled;
        };
        match k.code {
            KeyCode::Esc => {
                self.mode = Mode::Idle;
                EventOutcome::Consumed
            }
            KeyCode::Tab | KeyCode::Down => {
                state.focus = next_field(state.focus);
                EventOutcome::Consumed
            }
            KeyCode::BackTab | KeyCode::Up => {
                state.focus = prev_field(state.focus);
                EventOutcome::Consumed
            }
            KeyCode::Enter => {
                if state.focus == FormField::Desc {
                    let start_text = state.start.text.clone();
                    let end_text = state.end.text.clone();
                    let desc = state.desc.text.clone();
                    self.commit_form(ctx, &start_text, &end_text, &desc);
                } else {
                    state.focus = next_field(state.focus);
                }
                EventOutcome::Consumed
            }
            KeyCode::Backspace => {
                form_buf_mut(state).backspace();
                EventOutcome::Consumed
            }
            KeyCode::Delete => {
                form_buf_mut(state).delete();
                EventOutcome::Consumed
            }
            KeyCode::Left => {
                form_buf_mut(state).left();
                EventOutcome::Consumed
            }
            KeyCode::Right => {
                form_buf_mut(state).right();
                EventOutcome::Consumed
            }
            KeyCode::Home => {
                form_buf_mut(state).home();
                EventOutcome::Consumed
            }
            KeyCode::End => {
                form_buf_mut(state).end();
                EventOutcome::Consumed
            }
            KeyCode::Char(c) => {
                form_buf_mut(state).insert(c);
                EventOutcome::Consumed
            }
            _ => EventOutcome::Consumed,
        }
    }

    fn commit_form(&mut self, ctx: &mut TabCtx, start: &str, end: &str, desc: &str) {
        // Build a blockstring and reuse the quickline parser so the
        // grammar and error messages stay in one place.
        let blockstring = if desc.trim().is_empty() {
            format!("{} - {}", start, end)
        } else {
            format!("{} - {} {}", start, end, desc)
        };
        self.commit_quickline(ctx, &blockstring);
    }
}

fn next_field(f: FormField) -> FormField {
    match f {
        FormField::Start => FormField::End,
        FormField::End => FormField::Desc,
        FormField::Desc => FormField::Start,
    }
}

fn prev_field(f: FormField) -> FormField {
    match f {
        FormField::Start => FormField::Desc,
        FormField::End => FormField::Start,
        FormField::Desc => FormField::End,
    }
}

fn form_buf_mut(s: &mut FormState) -> &mut EditBuffer {
    match s.focus {
        FormField::Start => &mut s.start,
        FormField::End => &mut s.end,
        FormField::Desc => &mut s.desc,
    }
}

fn fmt_hhmm(t: NaiveTime) -> String {
    format!("{:02}:{:02}", t.hour(), t.minute())
}

/// Apply the same `±N`-minute shift the library performs on
/// [`TimeChange::ShiftMinutes`], clamping at `00:00` and `23:59` so the
/// "expected new start" computed by the TUI matches what
/// `ops::edit_block` will have written. Kept in sync with
/// `ft_core::timeblock::ops::apply_change`.
fn shift_clamped(t: NaiveTime, delta: i32) -> NaiveTime {
    let cur = (t.hour() as i32) * 60 + (t.minute() as i32);
    let new = (cur + delta).clamp(0, 23 * 60 + 59);
    NaiveTime::from_hms_opt((new / 60) as u32, (new % 60) as u32, 0).unwrap()
}

fn queue_toast(ctx: &TabCtx, text: &str, style: ToastStyle) {
    *ctx.pending_request.borrow_mut() = Some(AppRequest::Toast {
        text: text.to_string(),
        style,
    });
}

/// `(today, now)` for template rendering — honors `FT_TODAY` for tests
/// and falls back to the tab's clock for production. Mirrors the CLI
/// helper so both surfaces stay in lockstep on template variables.
fn today_now_for_template(
    ctx: &TabCtx,
    clock: ClockFn,
) -> (chrono::NaiveDate, chrono::NaiveDateTime) {
    if let Ok(s) = std::env::var("FT_TODAY") {
        if let Ok(d) = chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d") {
            return (d, d.and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap()));
        }
    }
    let now = (clock)();
    let _ = ctx; // ctx kept in signature for future use (e.g. real-clock-from-app)
    (now.date_naive(), now.naive_local())
}

impl Default for TimeblocksTab {
    fn default() -> Self {
        Self::new()
    }
}

impl Tab for TimeblocksTab {
    fn title(&self) -> &str {
        "Timeblocks"
    }

    fn on_focus(&mut self, ctx: &mut TabCtx) -> Result<()> {
        self.reload(ctx);
        Ok(())
    }

    fn handle_event(&mut self, ev: Event, ctx: &mut TabCtx) -> Result<EventOutcome> {
        let Event::Key(k) = ev else {
            return Ok(EventOutcome::NotHandled);
        };

        // Modal input (quickline / edit-desc / form) eats everything
        // except its own commit / cancel keys. The two-stroke `d d`
        // chord is also handled at the top so the inter-stroke window
        // can short-circuit before the navigation keymap runs.
        match &mut self.mode {
            Mode::Idle => {}
            Mode::DeleteConfirm { .. } => {
                return Ok(self.handle_delete_confirm(k, ctx));
            }
            Mode::Quickline(_) => {
                return Ok(self.handle_quickline(k, ctx));
            }
            Mode::EditDesc { .. } => {
                return Ok(self.handle_edit_desc(k, ctx));
            }
            Mode::Form(_) => {
                return Ok(self.handle_form(k, ctx));
            }
        }

        // Idle keymap. `r` and the mutation chords need ctx for I/O
        // so they're handled here before delegating to the
        // navigation-only keymap.
        if k.modifiers == KeyModifiers::NONE || k.modifiers == KeyModifiers::SHIFT {
            match k.code {
                KeyCode::Char('r') => {
                    self.reload(ctx);
                    return Ok(EventOutcome::Consumed);
                }
                KeyCode::Char('c') => {
                    self.handle_create_daily(ctx);
                    return Ok(EventOutcome::Consumed);
                }
                KeyCode::Char('a') => {
                    self.mode = Mode::Quickline(EditBuffer::default());
                    return Ok(EventOutcome::Consumed);
                }
                KeyCode::Char('A') => {
                    self.mode = Mode::Form(self.default_form());
                    return Ok(EventOutcome::Consumed);
                }
                KeyCode::Char('e') => {
                    self.start_edit_desc(ctx);
                    return Ok(EventOutcome::Consumed);
                }
                KeyCode::Char('d') => {
                    self.start_delete_confirm(ctx);
                    return Ok(EventOutcome::Consumed);
                }
                KeyCode::Char(']') => {
                    self.shift_end(ctx, 5);
                    return Ok(EventOutcome::Consumed);
                }
                KeyCode::Char('[') => {
                    self.shift_end(ctx, -5);
                    return Ok(EventOutcome::Consumed);
                }
                KeyCode::Char('}') => {
                    self.shift_start(ctx, 5);
                    return Ok(EventOutcome::Consumed);
                }
                KeyCode::Char('{') => {
                    self.shift_start(ctx, -5);
                    return Ok(EventOutcome::Consumed);
                }
                _ => {}
            }
        }

        Ok(self.handle_key(k))
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &TabCtx) {
        view::render(self, frame, area, ctx);
    }

    fn refresh(&mut self, ctx: &mut TabCtx) -> Result<()> {
        self.reload(ctx);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, TimeZone};

    fn clock() -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 5, 16, 9, 30, 0)
            .single()
            .unwrap()
    }

    #[test]
    fn new_with_clock_seeds_today_and_tomorrow_dates() {
        let tab = TimeblocksTab::with_clock(clock);
        assert_eq!(
            tab.today.date,
            NaiveDate::from_ymd_opt(2026, 5, 16).unwrap()
        );
        assert_eq!(
            tab.tomorrow.date,
            NaiveDate::from_ymd_opt(2026, 5, 17).unwrap()
        );
        assert_eq!(tab.focus, Pane::Today);
    }

    #[test]
    fn toggle_focus_round_trips() {
        let mut tab = TimeblocksTab::with_clock(clock);
        assert_eq!(tab.focus, Pane::Today);
        tab.toggle_focus(true);
        assert_eq!(tab.focus, Pane::Tomorrow);
        tab.toggle_focus(true);
        assert_eq!(tab.focus, Pane::Today);
    }

    #[test]
    fn move_selection_clamps_to_block_count() {
        let mut tab = TimeblocksTab::with_clock(clock);
        tab.today.blocks = vec![mk(9, 0, 10, 0, "a"), mk(10, 0, 11, 0, "b")];
        tab.move_selection(1);
        assert_eq!(tab.today.selection, 1);
        tab.move_selection(5);
        assert_eq!(tab.today.selection, 1, "should clamp at last index");
        tab.move_selection(-99);
        assert_eq!(tab.today.selection, 0, "should clamp at zero");
    }

    #[test]
    fn jump_selection_handles_empty_pane() {
        let mut tab = TimeblocksTab::with_clock(clock);
        tab.jump_selection(true);
        assert_eq!(tab.today.selection, 0);
    }

    #[test]
    fn move_selection_does_nothing_on_empty_pane() {
        let mut tab = TimeblocksTab::with_clock(clock);
        tab.move_selection(1);
        assert_eq!(tab.today.selection, 0);
    }

    fn mk(sh: u32, sm: u32, eh: u32, em: u32, desc: &str) -> Timeblock {
        use chrono::NaiveTime;
        let start = NaiveTime::from_hms_opt(sh, sm, 0).unwrap();
        let end = NaiveTime::from_hms_opt(eh, em, 0).unwrap();
        Timeblock {
            start,
            end,
            end_explicit: true,
            desc: desc.into(),
            tags: ft_core::timeblock::parse_tags(desc),
            source_line: 1,
        }
    }
}
