use anyhow::Result;
use chrono::{Duration, Local, NaiveDate};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ft_core::{
    query::{
        dsl::{self, Query},
        sort::{sort_by_keys, SortKey, SortOrder},
    },
    task::{
        ops::{self, CompleteOptions},
        Priority, Task,
    },
};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::tui::{
    event::Event,
    tab::{AppRequest, EventOutcome, TabCtx},
    tabs::tasks::view::View,
};

/// Search view: lazy task scan, editable DSL query bar, and a paginated list
/// split into "overdue" and "upcoming" buckets. Quick mutations and editor
/// handoff land in sessions 4–5 — this session lays the foundation.
pub struct SearchView {
    /// Loaded tasks. Empty until the first focus triggers a scan.
    tasks: Vec<Task>,
    /// Whether `tasks` reflects a real scan (vs. the initial empty state).
    loaded: bool,
    /// Indices into `tasks` (sorted) that match the active query and pass
    /// the today-cutoff. Recomputed on load, on query apply, and on `R`.
    matches: Vec<usize>,
    /// Number of leading entries in `matches` that are overdue (due < today).
    /// The remainder are upcoming.
    overdue_count: usize,
    /// Index into `matches` for the highlighted row. Saturates at boundaries
    /// when wrapping is disabled, otherwise wraps via `↑` past 0 / `↓` past N.
    selected: usize,
    /// Top-of-viewport row offset within the visible row sequence (including
    /// dividers). Updated to keep `selected` on screen.
    scroll: u16,

    /// Currently active query string (the one driving `matches`).
    query_text: String,
    /// Most recent parse outcome for `query_text`. `Ok(None)` = empty query
    /// (matches all). `Err(msg)` shows the message in place of the list.
    parse_state: ParseState,

    /// Whether the query bar is focused for editing. While editing, all key
    /// events go to the buffer (not the list).
    edit_state: Option<EditBuffer>,

    /// Open edit-popup state, if any. Set by `e`; cleared by Esc / Ctrl+S.
    /// While the popup is open, all keys go to it.
    popup: Option<EditPopup>,
}

/// Modal form opened with `e` for the selected task. Six text fields plus
/// focus tracking and a parse-error slot. Submit (Ctrl+S) parses dates via
/// `ft_core::dates::parse` so users can type natural-language input.
#[derive(Debug, Clone)]
struct EditPopup {
    description: EditBuffer,
    due: EditBuffer,
    scheduled: EditBuffer,
    priority: EditBuffer,
    tags: EditBuffer,
    recurrence: EditBuffer,
    focus: EditField,
    error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditField {
    Description,
    Due,
    Scheduled,
    Priority,
    Tags,
    Recurrence,
}

impl EditField {
    fn label(self) -> &'static str {
        match self {
            EditField::Description => "description",
            EditField::Due => "due",
            EditField::Scheduled => "scheduled",
            EditField::Priority => "priority",
            EditField::Tags => "tags",
            EditField::Recurrence => "recurrence",
        }
    }

    fn next(self) -> Self {
        match self {
            EditField::Description => EditField::Due,
            EditField::Due => EditField::Scheduled,
            EditField::Scheduled => EditField::Priority,
            EditField::Priority => EditField::Tags,
            EditField::Tags => EditField::Recurrence,
            EditField::Recurrence => EditField::Description,
        }
    }

    fn prev(self) -> Self {
        match self {
            EditField::Description => EditField::Recurrence,
            EditField::Due => EditField::Description,
            EditField::Scheduled => EditField::Due,
            EditField::Priority => EditField::Scheduled,
            EditField::Tags => EditField::Priority,
            EditField::Recurrence => EditField::Tags,
        }
    }
}

impl EditPopup {
    /// Pre-populate from the selected task so the popup opens with the
    /// current state and the user can edit-in-place.
    fn from_task(task: &Task) -> Self {
        Self {
            description: EditBuffer::from(&task.description),
            due: EditBuffer::from(&fmt_date(task.due)),
            scheduled: EditBuffer::from(&fmt_date(task.scheduled)),
            priority: EditBuffer::from(priority_text(task.priority)),
            tags: EditBuffer::from(&task.tags.join(" ")),
            recurrence: EditBuffer::from(task.recurrence.as_deref().unwrap_or("")),
            focus: EditField::Description,
            error: None,
        }
    }

    fn focused_buffer_mut(&mut self) -> &mut EditBuffer {
        match self.focus {
            EditField::Description => &mut self.description,
            EditField::Due => &mut self.due,
            EditField::Scheduled => &mut self.scheduled,
            EditField::Priority => &mut self.priority,
            EditField::Tags => &mut self.tags,
            EditField::Recurrence => &mut self.recurrence,
        }
    }

    fn buffer_for(&self, field: EditField) -> &EditBuffer {
        match field {
            EditField::Description => &self.description,
            EditField::Due => &self.due,
            EditField::Scheduled => &self.scheduled,
            EditField::Priority => &self.priority,
            EditField::Tags => &self.tags,
            EditField::Recurrence => &self.recurrence,
        }
    }
}

fn fmt_date(d: Option<NaiveDate>) -> String {
    d.map(|x| x.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

fn priority_text(p: Option<Priority>) -> &'static str {
    match p {
        None => "",
        Some(Priority::Lowest) => "lowest",
        Some(Priority::Low) => "low",
        Some(Priority::Medium) => "medium",
        Some(Priority::High) => "high",
        Some(Priority::Highest) => "highest",
    }
}

fn parse_priority(s: &str) -> Result<Option<Priority>, String> {
    let trimmed = s.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "lowest" => Ok(Some(Priority::Lowest)),
        "low" => Ok(Some(Priority::Low)),
        "medium" | "med" => Ok(Some(Priority::Medium)),
        "high" => Ok(Some(Priority::High)),
        "highest" => Ok(Some(Priority::Highest)),
        other => Err(format!(
            "priority `{other}` not recognized (try none / low / medium / high)"
        )),
    }
}

fn parse_tags_field(s: &str) -> Vec<String> {
    s.split(|c: char| c.is_whitespace() || c == ',')
        .map(|t| t.trim_start_matches('#').trim())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

/// Rewrite `description` so its inline `#tag` words exactly match `tags`.
///
/// `Task.tags` is a *derived* index extracted from the description on parse —
/// the serializer never writes tags as a separate field. To persist tag edits
/// from the popup we have to strip the old inline tags from the description
/// and re-append the ones the user wants.
fn merge_tags_into_description(description: &str, tags: &[String]) -> String {
    let mut kept: Vec<&str> = Vec::new();
    for word in description.split_whitespace() {
        if !is_tag_word(word) {
            kept.push(word);
        }
    }
    let mut out = kept.join(" ");
    for tag in tags {
        let bare = tag.trim_start_matches('#');
        if bare.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push('#');
        out.push_str(bare);
    }
    out
}

fn is_tag_word(w: &str) -> bool {
    let Some(rest) = w.strip_prefix('#') else {
        return false;
    };
    !rest.is_empty()
        && rest
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/'))
}

fn parse_optional_date(s: &str, today: NaiveDate) -> Result<Option<NaiveDate>, String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    ft_core::dates::parse(trimmed, today)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// Result of compiling the active `query_text` against the current `today`.
#[derive(Debug, Clone)]
enum ParseState {
    Ok(Option<Query>),
    Err(String),
}

#[derive(Debug, Clone, Default)]
struct EditBuffer {
    text: String,
    /// Cursor position as a character offset (not byte offset).
    cursor: usize,
}

impl EditBuffer {
    fn from(text: &str) -> Self {
        let cursor = text.chars().count();
        Self {
            text: text.to_string(),
            cursor,
        }
    }

    fn insert(&mut self, c: char) {
        let byte_idx = self
            .text
            .char_indices()
            .nth(self.cursor)
            .map(|(b, _)| b)
            .unwrap_or(self.text.len());
        self.text.insert(byte_idx, c);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev_char = self
            .text
            .char_indices()
            .nth(self.cursor - 1)
            .map(|(b, c)| (b, c.len_utf8()));
        if let Some((b, len)) = prev_char {
            self.text.replace_range(b..b + len, "");
            self.cursor -= 1;
        }
    }

    fn delete(&mut self) {
        let target = self
            .text
            .char_indices()
            .nth(self.cursor)
            .map(|(b, c)| (b, c.len_utf8()));
        if let Some((b, len)) = target {
            self.text.replace_range(b..b + len, "");
        }
    }

    fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn right(&mut self) {
        let max = self.text.chars().count();
        if self.cursor < max {
            self.cursor += 1;
        }
    }

    fn home(&mut self) {
        self.cursor = 0;
    }

    fn end(&mut self) {
        self.cursor = self.text.chars().count();
    }

    /// Delete from the cursor leftward to the start of the previous word.
    /// Matches the bash/readline `unix-word-rubout` behavior: skip trailing
    /// whitespace, then skip non-whitespace, then erase the span.
    fn delete_word_backward(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = self.cursor;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        let start_byte: usize = chars[..i].iter().map(|c| c.len_utf8()).sum();
        let end_byte: usize = chars[..self.cursor].iter().map(|c| c.len_utf8()).sum();
        self.text.replace_range(start_byte..end_byte, "");
        self.cursor = i;
    }
}

impl SearchView {
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            loaded: false,
            matches: Vec::new(),
            overdue_count: 0,
            selected: 0,
            scroll: 0,
            query_text: String::new(),
            parse_state: ParseState::Ok(None),
            edit_state: None,
            popup: None,
        }
    }

    /// Default DSL: tasks that are still actionable, due before `today + 8`,
    /// sorted due ascending then priority descending. The literal date keeps
    /// the bar copy-pastable and round-trippable through the parser.
    fn default_query(today: NaiveDate) -> String {
        let upper = today + Duration::days(8);
        format!(
            "not done and due before {} sort by due, priority reverse",
            upper.format("%Y-%m-%d")
        )
    }

    fn ensure_loaded(&mut self, ctx: &mut TabCtx) -> Result<()> {
        if self.loaded {
            return Ok(());
        }
        self.reload(ctx)
    }

    fn reload(&mut self, ctx: &mut TabCtx) -> Result<()> {
        let scan = ctx.vault.scan();
        self.tasks = scan.tasks;
        self.loaded = true;
        if self.query_text.is_empty() {
            self.query_text = Self::default_query(ctx.today);
        }
        self.recompile(ctx.today);
        self.recompute_matches(ctx.today);
        ctx.last_refresh.set(Some(Local::now()));
        Ok(())
    }

    fn recompile(&mut self, today: NaiveDate) {
        let trimmed = self.query_text.trim();
        if trimmed.is_empty() {
            self.parse_state = ParseState::Ok(None);
            return;
        }
        match dsl::parse(trimmed, today) {
            Ok(q) => self.parse_state = ParseState::Ok(Some(q)),
            Err(e) => self.parse_state = ParseState::Err(e.to_string()),
        }
    }

    fn recompute_matches(&mut self, today: NaiveDate) {
        self.matches.clear();
        self.overdue_count = 0;
        self.selected = 0;
        self.scroll = 0;

        let query = match &self.parse_state {
            ParseState::Ok(q) => q.clone(),
            ParseState::Err(_) => return,
        };

        // Filter
        let active_expr = query.as_ref().and_then(|q| q.expr.as_ref());
        let mut keep: Vec<&Task> = self
            .tasks
            .iter()
            .filter(|t| active_expr.is_none_or(|expr| expr.matches(t)))
            .collect();

        // Sort
        let sort_keys: Vec<(SortKey, SortOrder)> = query
            .as_ref()
            .map(|q| q.sort_keys.clone())
            .unwrap_or_default();
        sort_by_keys(&mut keep, &sort_keys);

        // Apply DSL limit if present.
        let limit = query.as_ref().and_then(|q| q.limit);
        if let Some(n) = limit {
            keep.truncate(n);
        }

        // Reverse-map back to indices into self.tasks. Tasks are uniquely
        // identified by (path, line); we look each one up.
        for t in &keep {
            if let Some(idx) = self
                .tasks
                .iter()
                .position(|s| s.source_file == t.source_file && s.source_line == t.source_line)
            {
                self.matches.push(idx);
            }
        }

        // Bucket: count leading overdue entries. After sort by due asc, all
        // overdue rows precede upcoming ones.
        self.overdue_count = self
            .matches
            .iter()
            .take_while(|&&i| self.tasks[i].due.map(|d| d < today).unwrap_or(false))
            .count();
    }

    // --- selection ---------------------------------------------------------

    fn select_prev(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        if self.selected == 0 {
            self.selected = self.matches.len() - 1;
        } else {
            self.selected -= 1;
        }
    }

    fn select_next(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.matches.len();
    }

    // --- query editing -----------------------------------------------------

    fn enter_edit_mode(&mut self) {
        self.edit_state = Some(EditBuffer::from(&self.query_text));
    }

    fn cancel_edit(&mut self) {
        self.edit_state = None;
    }

    fn apply_edit(&mut self, ctx: &mut TabCtx) {
        if let Some(buf) = self.edit_state.take() {
            self.query_text = buf.text;
            self.recompile(ctx.today);
            self.recompute_matches(ctx.today);
        }
    }

    // --- rendering helpers -------------------------------------------------

    fn render_query_bar(&self, frame: &mut Frame, area: Rect) {
        let editing = self.edit_state.is_some();
        let title = if editing {
            " query (editing) "
        } else {
            " query "
        };
        let border_style = if editing {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        // Inner content width inside borders. We scroll horizontally so the
        // edit cursor stays visible — long queries would otherwise drop the
        // caret off the right edge.
        let inner_width = area.width.saturating_sub(2) as usize;

        let line: Line = if let Some(buf) = &self.edit_state {
            let chars: Vec<char> = buf.text.chars().collect();
            let cursor = buf.cursor.min(chars.len());
            let scroll = horizontal_scroll(cursor, chars.len(), inner_width);

            let visible_end = (scroll + inner_width.saturating_sub(1)).min(chars.len());
            let visible: String = chars[scroll..visible_end].iter().collect();
            let visible_cursor = cursor.saturating_sub(scroll);
            let split = visible_cursor.min(visible.chars().count());
            let mut iter = visible.chars();
            let left: String = iter.by_ref().take(split).collect();
            let right: String = iter.collect();
            Line::from(vec![
                Span::raw(left),
                Span::styled(
                    "│",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(right),
            ])
        } else {
            let display = if self.query_text.is_empty() {
                "(no filter — press / to edit)".to_string()
            } else {
                self.query_text.clone()
            };
            Line::from(Span::styled(display, Style::default().fg(Color::White)))
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(title);
        let para = Paragraph::new(line).block(block);
        frame.render_widget(para, area);
    }

    fn render_list(&self, frame: &mut Frame, area: Rect, today: NaiveDate) {
        // Parse error short-circuits the list.
        if let ParseState::Err(msg) = &self.parse_state {
            let body = Paragraph::new(vec![
                Line::from(""),
                Line::from(Span::styled(
                    "query parse error",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(msg, Style::default().fg(Color::Red))),
                Line::from(""),
                Line::from(Span::styled(
                    "press / to edit the query",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                )),
            ])
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title(" tasks "));
            frame.render_widget(body, area);
            return;
        }

        if !self.loaded {
            let body = Paragraph::new(Line::from(Span::styled(
                "loading…",
                Style::default().fg(Color::DarkGray),
            )))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title(" tasks "));
            frame.render_widget(body, area);
            return;
        }

        if self.matches.is_empty() {
            let body = Paragraph::new(Line::from(Span::styled(
                "no matching tasks",
                Style::default().fg(Color::DarkGray),
            )))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title(" tasks "));
            frame.render_widget(body, area);
            return;
        }

        // Inner width inside the borders. The fixed cells are: cursor (2)
        // + priority label (4) + due block (14) + scheduled block (14) = 34.
        // The description column flexes to fill what's left, with a small
        // floor so very narrow terminals still render something readable.
        let inner_width = area.width.saturating_sub(2);
        let desc_width = inner_width.saturating_sub(34).max(20) as usize;

        let lines = self.build_lines(today, desc_width);
        let scroll = self.scroll;
        let list = Paragraph::new(lines)
            .scroll((scroll, 0))
            .block(Block::default().borders(Borders::ALL).title(" tasks "));
        frame.render_widget(list, area);
    }

    fn build_lines(&self, today: NaiveDate, desc_width: usize) -> Vec<Line<'static>> {
        let mut lines: Vec<Line> = Vec::with_capacity(self.matches.len() + 4);
        let upcoming_start = self.overdue_count;

        if upcoming_start > 0 {
            lines.push(divider_line(&format!("── overdue ({}) ──", upcoming_start)));
            for (i, &task_idx) in self.matches[..upcoming_start].iter().enumerate() {
                let selected = i == self.selected;
                lines.push(task_line(
                    &self.tasks[task_idx],
                    today,
                    selected,
                    desc_width,
                ));
            }
        }
        if upcoming_start < self.matches.len() {
            let upcoming_n = self.matches.len() - upcoming_start;
            lines.push(divider_line(&format!("── upcoming ({}) ──", upcoming_n)));
            for (i, &task_idx) in self.matches[upcoming_start..].iter().enumerate() {
                let selected = (i + upcoming_start) == self.selected;
                lines.push(task_line(
                    &self.tasks[task_idx],
                    today,
                    selected,
                    desc_width,
                ));
            }
        }
        lines
    }

    /// Compute the row index of `selected` within the rendered line sequence
    /// (which includes section dividers). Returns 0 when nothing is selected.
    fn selected_row(&self) -> u16 {
        if self.matches.is_empty() {
            return 0;
        }
        let upcoming_start = self.overdue_count;
        // Each non-empty section adds 1 divider row before its tasks.
        let mut row: usize = 0;
        if upcoming_start > 0 {
            row += 1; // overdue divider
        }
        if self.selected < upcoming_start {
            row += self.selected;
        } else {
            row += upcoming_start; // skip overdue rows
            row += 1; // upcoming divider
            row += self.selected - upcoming_start;
        }
        u16::try_from(row).unwrap_or(u16::MAX)
    }

    fn adjust_scroll(&mut self, body_height: u16) {
        // Body has a 1-row top border + 1-row bottom border ⇒ 2 reserved rows.
        let visible = body_height.saturating_sub(2).max(1);
        let row = self.selected_row();
        if row < self.scroll {
            self.scroll = row;
        } else if row >= self.scroll + visible {
            self.scroll = row + 1 - visible;
        }
    }
}

impl View for SearchView {
    fn title(&self) -> &str {
        "Search"
    }

    fn on_focus(&mut self, ctx: &mut TabCtx) -> Result<()> {
        self.ensure_loaded(ctx)
    }

    fn handle_event(&mut self, ev: Event, ctx: &mut TabCtx) -> Result<EventOutcome> {
        let Event::Key(k) = ev else {
            return Ok(EventOutcome::NotHandled);
        };

        // Modal popup swallows everything until Esc / Ctrl+S.
        if self.popup.is_some() {
            return self.handle_popup_key(k, ctx);
        }

        // Editing the query bar swallows everything except Apply/Cancel.
        if self.edit_state.is_some() {
            return Ok(self.handle_edit_key(k, ctx));
        }

        match (k.code, k.modifiers) {
            // Plan lists `/` and `q` as edit-mode triggers, but `q` is the
            // global quit keybinding. `/` alone (vi/less convention) avoids
            // the conflict; `q` remains quit.
            (KeyCode::Char('/'), _) => {
                self.enter_edit_mode();
                Ok(EventOutcome::Consumed)
            }
            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                self.select_prev();
                Ok(EventOutcome::Consumed)
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                self.select_next();
                Ok(EventOutcome::Consumed)
            }
            (KeyCode::Char('R'), _) => {
                self.reload(ctx)?;
                Ok(EventOutcome::Consumed)
            }
            // Quick mutations on the selected task. Each writes atomically
            // through ft-core, then re-scans so the row reflects the new
            // state and overdue/upcoming bucketing stays correct.
            (KeyCode::Char(']'), _) => self.nudge_field(ctx, Field::Due, 1),
            (KeyCode::Char('['), _) => self.nudge_field(ctx, Field::Due, -1),
            (KeyCode::Char('}'), _) => self.nudge_field(ctx, Field::Scheduled, 1),
            (KeyCode::Char('{'), _) => self.nudge_field(ctx, Field::Scheduled, -1),
            (KeyCode::Char('p'), KeyModifiers::NONE) => self.cycle_priority(ctx, 1),
            (KeyCode::Char('P'), _) => self.cycle_priority(ctx, -1),
            (KeyCode::Char('x'), KeyModifiers::NONE) => self.complete_selected(ctx),
            (KeyCode::Char('X'), _) => self.cancel_selected(ctx),
            (KeyCode::Char('t'), KeyModifiers::NONE) => self.set_due_today(ctx),
            (KeyCode::Char('e'), KeyModifiers::NONE) => {
                self.open_edit_popup();
                Ok(EventOutcome::Consumed)
            }
            (KeyCode::Enter, _) => {
                self.request_editor_open(ctx);
                Ok(EventOutcome::Consumed)
            }
            _ => Ok(EventOutcome::NotHandled),
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &TabCtx) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(1)])
            .split(area);

        self.render_query_bar(frame, chunks[0]);
        // Scroll adjustment depends on the body area height; calculate before
        // the render call so the snapshot reflects the post-adjustment state.
        self.adjust_scroll(chunks[1].height);
        self.render_list(frame, chunks[1], ctx.today);

        // Popup is drawn last so it floats above the list. Use the full body
        // area as the anchor — the helper centers the popup within it.
        if let Some(popup) = &self.popup {
            render_edit_popup(frame, area, popup);
        }
    }

    fn refresh(&mut self, ctx: &mut TabCtx) -> Result<()> {
        self.reload(ctx)
    }
}

/// Which date column a `]`/`[`/`}`/`{` keypress targets.
#[derive(Debug, Clone, Copy)]
enum Field {
    Due,
    Scheduled,
}

/// Priority cycle order per plan: `p` walks None → Low → Medium → High → None;
/// `P` walks the other way. Highest/Lowest aren't on the cycle — they're
/// rarely used and the future edit popup will set them explicitly.
const PRIORITY_CYCLE: &[Option<Priority>] = &[
    None,
    Some(Priority::Low),
    Some(Priority::Medium),
    Some(Priority::High),
];

fn cycle_pos(p: Option<Priority>) -> usize {
    PRIORITY_CYCLE.iter().position(|x| *x == p).unwrap_or(0)
}

impl SearchView {
    /// Re-scan the vault and recompute matches, then restore the selection
    /// to the row whose `(path, line)` matches `anchor` if it's still in the
    /// list. Falls back to saturating at the last row.
    fn refresh_after_mutation(
        &mut self,
        ctx: &mut TabCtx,
        anchor: Option<(std::path::PathBuf, usize)>,
    ) -> Result<()> {
        self.reload(ctx)?;
        if let Some((path, line)) = anchor {
            if let Some((i, _)) = self.matches.iter().enumerate().find(|(_, &task_idx)| {
                let t = &self.tasks[task_idx];
                t.source_file == path && t.source_line == line
            }) {
                self.selected = i;
                return Ok(());
            }
        }
        if !self.matches.is_empty() && self.selected >= self.matches.len() {
            self.selected = self.matches.len() - 1;
        }
        Ok(())
    }

    fn with_selected_task<F>(&mut self, ctx: &mut TabCtx, op: F) -> Result<EventOutcome>
    where
        F: FnOnce(&std::path::Path, &Task, NaiveDate) -> Result<()>,
    {
        let Some(&task_idx) = self.matches.get(self.selected) else {
            return Ok(EventOutcome::Consumed);
        };
        let task = &self.tasks[task_idx];
        // Tasks store paths relative to the vault root; ft-core mutators
        // need an absolute (or CWD-relative) path to read/write.
        let absolute = ctx.vault.path.join(&task.source_file);
        let anchor = Some((task.source_file.clone(), task.source_line));
        op(&absolute, task, ctx.today)?;
        self.refresh_after_mutation(ctx, anchor)?;
        Ok(EventOutcome::Consumed)
    }

    fn nudge_field(
        &mut self,
        ctx: &mut TabCtx,
        field: Field,
        delta_days: i64,
    ) -> Result<EventOutcome> {
        self.with_selected_task(ctx, |path, task, today| {
            let line = task.source_line;
            ops::update_task_line(path, line, move |t| {
                let current = match field {
                    Field::Due => t.due,
                    Field::Scheduled => t.scheduled,
                };
                let base = current.unwrap_or(today);
                let next = base + Duration::days(delta_days);
                match field {
                    Field::Due => t.due = Some(next),
                    Field::Scheduled => t.scheduled = Some(next),
                }
            })?;
            Ok(())
        })
    }

    fn set_due_today(&mut self, ctx: &mut TabCtx) -> Result<EventOutcome> {
        self.with_selected_task(ctx, |path, task, today| {
            let line = task.source_line;
            ops::update_task_line(path, line, move |t| {
                t.due = Some(today);
            })?;
            Ok(())
        })
    }

    fn cycle_priority(&mut self, ctx: &mut TabCtx, direction: i64) -> Result<EventOutcome> {
        self.with_selected_task(ctx, |path, task, _today| {
            let line = task.source_line;
            ops::update_task_line(path, line, move |t| {
                let pos = cycle_pos(t.priority) as i64;
                let len = PRIORITY_CYCLE.len() as i64;
                let next = ((pos + direction).rem_euclid(len)) as usize;
                t.priority = PRIORITY_CYCLE[next];
            })?;
            Ok(())
        })
    }

    fn complete_selected(&mut self, ctx: &mut TabCtx) -> Result<EventOutcome> {
        self.with_selected_task(ctx, |path, task, today| {
            // Already-done tasks are a no-op rather than an error so the user
            // can hammer `x` without ceremony.
            match ops::complete_task(path, task.source_line, CompleteOptions { on: today }) {
                Ok(_) => Ok(()),
                Err(ops::CompleteError::AlreadyDone { .. }) => Ok(()),
                Err(e) => Err(anyhow::Error::from(e)),
            }
        })
    }

    fn cancel_selected(&mut self, ctx: &mut TabCtx) -> Result<EventOutcome> {
        self.with_selected_task(ctx, |path, task, today| {
            match ops::cancel_task(path, task.source_line, today) {
                Ok(_) => Ok(()),
                Err(ops::CancelError::AlreadyCancelled { .. }) => Ok(()),
                Err(e) => Err(anyhow::Error::from(e)),
            }
        })
    }

    fn open_edit_popup(&mut self) {
        let Some(&task_idx) = self.matches.get(self.selected) else {
            return;
        };
        self.popup = Some(EditPopup::from_task(&self.tasks[task_idx]));
    }

    fn request_editor_open(&self, ctx: &TabCtx) {
        let Some(&task_idx) = self.matches.get(self.selected) else {
            return;
        };
        let task = &self.tasks[task_idx];
        let absolute = ctx.vault.path.join(&task.source_file);
        *ctx.pending_request.borrow_mut() = Some(AppRequest::OpenInEditor {
            path: absolute,
            line: task.source_line,
        });
    }

    fn handle_popup_key(&mut self, k: KeyEvent, ctx: &mut TabCtx) -> Result<EventOutcome> {
        let Some(popup) = self.popup.as_mut() else {
            return Ok(EventOutcome::Consumed);
        };

        // Ctrl+S submits regardless of focused field.
        if k.code == KeyCode::Char('s') && k.modifiers.contains(KeyModifiers::CONTROL) {
            return self.submit_popup(ctx);
        }

        match (k.code, k.modifiers) {
            (KeyCode::Esc, _) => {
                self.popup = None;
            }
            (KeyCode::Tab, _) => popup.focus = popup.focus.next(),
            (KeyCode::BackTab, _) => popup.focus = popup.focus.prev(),
            (KeyCode::Down, _) => popup.focus = popup.focus.next(),
            (KeyCode::Up, _) => popup.focus = popup.focus.prev(),
            (KeyCode::Backspace, m)
                if m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::ALT) =>
            {
                popup.focused_buffer_mut().delete_word_backward();
            }
            (KeyCode::Char('w'), KeyModifiers::CONTROL) => {
                popup.focused_buffer_mut().delete_word_backward();
            }
            (KeyCode::Backspace, _) => popup.focused_buffer_mut().backspace(),
            (KeyCode::Delete, _) => popup.focused_buffer_mut().delete(),
            (KeyCode::Left, _) => popup.focused_buffer_mut().left(),
            (KeyCode::Right, _) => popup.focused_buffer_mut().right(),
            (KeyCode::Home, _) => popup.focused_buffer_mut().home(),
            (KeyCode::End, _) => popup.focused_buffer_mut().end(),
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => {
                popup.focused_buffer_mut().insert(c);
            }
            _ => {}
        }
        Ok(EventOutcome::Consumed)
    }

    fn submit_popup(&mut self, ctx: &mut TabCtx) -> Result<EventOutcome> {
        // Validate everything *before* mutating disk so a bad input keeps the
        // popup open with a clear message. Borrow popup immutably through the
        // validation phase, then drop the borrow before calling the mutator.
        let validated = {
            let Some(popup) = self.popup.as_ref() else {
                return Ok(EventOutcome::Consumed);
            };
            let due = match parse_optional_date(&popup.due.text, ctx.today) {
                Ok(v) => v,
                Err(e) => {
                    self.popup.as_mut().unwrap().error = Some(format!("due: {e}"));
                    self.popup.as_mut().unwrap().focus = EditField::Due;
                    return Ok(EventOutcome::Consumed);
                }
            };
            let scheduled = match parse_optional_date(&popup.scheduled.text, ctx.today) {
                Ok(v) => v,
                Err(e) => {
                    self.popup.as_mut().unwrap().error = Some(format!("scheduled: {e}"));
                    self.popup.as_mut().unwrap().focus = EditField::Scheduled;
                    return Ok(EventOutcome::Consumed);
                }
            };
            let priority = match parse_priority(&popup.priority.text) {
                Ok(v) => v,
                Err(e) => {
                    self.popup.as_mut().unwrap().error = Some(e);
                    self.popup.as_mut().unwrap().focus = EditField::Priority;
                    return Ok(EventOutcome::Consumed);
                }
            };
            let recurrence = popup.recurrence.text.trim();
            let recurrence = (!recurrence.is_empty()).then(|| recurrence.to_string());
            let raw_description = popup.description.text.trim().to_string();
            let tags = parse_tags_field(&popup.tags.text);
            // Description carries inline `#tag` words; rewrite it so the
            // popup's tag field is the source of truth on save. Without this
            // `t.tags = ...` is a no-op (tags are re-derived from the
            // description on parse).
            let description = merge_tags_into_description(&raw_description, &tags);
            (description, due, scheduled, priority, tags, recurrence)
        };

        let outcome = self.with_selected_task(ctx, |path, task, _today| {
            let (description, due, scheduled, priority, tags, recurrence) = validated;
            ops::update_task_line(path, task.source_line, move |t| {
                t.description = description;
                t.due = due;
                t.scheduled = scheduled;
                t.priority = priority;
                t.tags = tags;
                t.recurrence = recurrence;
            })?;
            Ok(())
        })?;
        self.popup = None;
        Ok(outcome)
    }

    fn handle_edit_key(&mut self, k: KeyEvent, ctx: &mut TabCtx) -> EventOutcome {
        match (k.code, k.modifiers) {
            (KeyCode::Esc, _) => {
                self.cancel_edit();
            }
            (KeyCode::Enter, _) => {
                self.apply_edit(ctx);
            }
            (KeyCode::Backspace, m)
                if m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::ALT) =>
            {
                if let Some(b) = self.edit_state.as_mut() {
                    b.delete_word_backward();
                }
            }
            (KeyCode::Char('w'), KeyModifiers::CONTROL) => {
                if let Some(b) = self.edit_state.as_mut() {
                    b.delete_word_backward();
                }
            }
            (KeyCode::Backspace, _) => {
                if let Some(b) = self.edit_state.as_mut() {
                    b.backspace();
                }
            }
            (KeyCode::Delete, _) => {
                if let Some(b) = self.edit_state.as_mut() {
                    b.delete();
                }
            }
            (KeyCode::Left, _) => {
                if let Some(b) = self.edit_state.as_mut() {
                    b.left();
                }
            }
            (KeyCode::Right, _) => {
                if let Some(b) = self.edit_state.as_mut() {
                    b.right();
                }
            }
            (KeyCode::Home, _) => {
                if let Some(b) = self.edit_state.as_mut() {
                    b.home();
                }
            }
            (KeyCode::End, _) => {
                if let Some(b) = self.edit_state.as_mut() {
                    b.end();
                }
            }
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => {
                if let Some(b) = self.edit_state.as_mut() {
                    b.insert(c);
                }
            }
            _ => {}
        }
        EventOutcome::Consumed
    }
}

/// Render the modal edit popup centered over `area`. Compact one-row-per-field
/// layout (label : value) so all six fields fit within an 80x24 viewport.
/// The focused field is bold and underlined; everyone else stays plain.
fn render_edit_popup(frame: &mut Frame, area: Rect, popup: &EditPopup) {
    use ratatui::widgets::Clear;

    let popup_area = centered_rect(72, 60, area);
    frame.render_widget(Clear, popup_area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .title(" edit task ")
        .style(Style::default().bg(Color::Black));
    let inner = outer.inner(popup_area);
    frame.render_widget(outer, popup_area);

    const FIELDS: &[EditField] = &[
        EditField::Description,
        EditField::Due,
        EditField::Scheduled,
        EditField::Priority,
        EditField::Tags,
        EditField::Recurrence,
    ];

    let label_width = FIELDS.iter().map(|f| f.label().len()).max().unwrap_or(0);
    let mut lines: Vec<Line> = Vec::with_capacity(FIELDS.len() + 3);
    lines.push(Line::from("")); // top padding

    let inner_width = inner.width.saturating_sub(2) as usize; // 1-col gutter each side
    let value_width = inner_width.saturating_sub(label_width + 4); // "  " + ": "

    for field in FIELDS {
        let focused = popup.focus == *field;
        let buf = popup.buffer_for(*field);
        let label_style = if focused {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let value_spans: Vec<Span<'static>> = if focused {
            let chars: Vec<char> = buf.text.chars().collect();
            let cursor = buf.cursor.min(chars.len());
            let scroll = horizontal_scroll(cursor, chars.len(), value_width);
            let visible_end = (scroll + value_width.saturating_sub(1)).min(chars.len());
            let visible: String = chars[scroll..visible_end].iter().collect();
            let visible_cursor = cursor.saturating_sub(scroll);
            let split = visible_cursor.min(visible.chars().count());
            let mut iter = visible.chars();
            let left: String = iter.by_ref().take(split).collect();
            let right: String = iter.collect();
            vec![
                Span::styled(left, Style::default().fg(Color::White)),
                Span::styled(
                    "│",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(right, Style::default().fg(Color::White)),
            ]
        } else {
            let display: String = buf.text.chars().take(value_width).collect();
            vec![Span::styled(display, Style::default().fg(Color::White))]
        };

        let mut spans: Vec<Span<'static>> = Vec::with_capacity(value_spans.len() + 2);
        spans.push(Span::styled(
            format!("  {:>width$} : ", field.label(), width = label_width),
            label_style,
        ));
        spans.extend(value_spans);
        lines.push(Line::from(spans));
    }

    lines.push(Line::from("")); // separator before footer
    let footer = if let Some(msg) = &popup.error {
        Line::from(vec![
            Span::styled("  ⚠ ", Style::default().fg(Color::Red)),
            Span::styled(msg.clone(), Style::default().fg(Color::Red)),
        ])
    } else {
        Line::from(Span::styled(
            "  Tab/Shift+Tab next · Ctrl+S save · Esc cancel",
            Style::default().fg(Color::DarkGray),
        ))
    };
    lines.push(footer);

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Centered rect helper duplicated from `ui.rs` so this file stays
/// self-contained for popup rendering.
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// Pick a horizontal scroll offset (in chars) so `cursor` is visible within
/// `width` cols of viewport. Reserves one column for the caret itself.
fn horizontal_scroll(cursor: usize, total: usize, width: usize) -> usize {
    if width == 0 {
        return 0;
    }
    if cursor < width {
        return 0;
    }
    let max_scroll = total.saturating_sub(width.saturating_sub(1));
    cursor
        .saturating_sub(width.saturating_sub(1))
        .min(max_scroll)
}

// --- row formatting ----------------------------------------------------------

fn divider_line(label: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!(" {label}"),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    ))
}

fn task_line(task: &Task, today: NaiveDate, selected: bool, desc_width: usize) -> Line<'static> {
    let pri_label = priority_label(task.priority);
    let pri_color = priority_color(task.priority);

    let due_str = task.due.map(|d| d.format("%Y-%m-%d").to_string());
    let due_color = task
        .due
        .map(|d| if d < today { Color::Red } else { Color::White })
        .unwrap_or(Color::DarkGray);

    let scheduled_str = task.scheduled.map(|d| d.format("%Y-%m-%d").to_string());

    let cursor = if selected { "▶ " } else { "  " };
    let pri_text = if pri_label.is_empty() {
        "    ".to_string()
    } else {
        format!("{:<3} ", pri_label)
    };

    // Truncate the description only when it exceeds the budget the caller
    // computed from the actual viewport width; otherwise pad to keep the
    // due / scheduled columns aligned across rows.
    let desc = task.description.replace('\n', " ");
    let desc_count = desc.chars().count();
    let desc_trimmed = if desc_count > desc_width {
        let cut: String = desc.chars().take(desc_width.saturating_sub(1)).collect();
        format!("{cut}…")
    } else {
        desc
    };
    let desc_padded = format!("{:<width$}", desc_trimmed, width = desc_width);

    let mut spans: Vec<Span<'static>> = Vec::with_capacity(8);
    let row_style = if selected {
        Style::default()
            .bg(Color::Rgb(40, 40, 60))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    spans.push(Span::styled(cursor.to_string(), row_style));
    spans.push(Span::styled(pri_text, row_style.fg(pri_color)));
    spans.push(Span::styled(desc_padded, row_style));
    if let Some(due) = due_str {
        spans.push(Span::styled(" 📅 ", row_style.fg(Color::DarkGray)));
        spans.push(Span::styled(due, row_style.fg(due_color)));
    } else {
        spans.push(Span::styled("              ", row_style));
    }
    if let Some(sch) = scheduled_str {
        spans.push(Span::styled(" ⏳ ", row_style.fg(Color::DarkGray)));
        spans.push(Span::styled(sch, row_style.fg(Color::Cyan)));
    }
    Line::from(spans)
}

fn priority_label(p: Option<Priority>) -> &'static str {
    match p {
        Some(Priority::Highest) => "!!!",
        Some(Priority::High) => "!!",
        Some(Priority::Medium) => "!",
        Some(Priority::Low) => "v",
        Some(Priority::Lowest) => "vv",
        None => "",
    }
}

fn priority_color(p: Option<Priority>) -> Color {
    match p {
        Some(Priority::Highest | Priority::High) => Color::Red,
        Some(Priority::Medium) => Color::Yellow,
        Some(Priority::Low | Priority::Lowest) => Color::Blue,
        None => Color::DarkGray,
    }
}
