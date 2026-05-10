use anyhow::Result;
use chrono::{Duration, Local, NaiveDate};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ft_core::{
    query::{
        dsl::{self, Query},
        sort::{sort_by_keys, SortKey, SortOrder},
    },
    task::{Priority, Task},
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
    tab::{EventOutcome, TabCtx},
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

        let lines = self.build_lines(today);
        let scroll = self.scroll;
        let list = Paragraph::new(lines)
            .scroll((scroll, 0))
            .block(Block::default().borders(Borders::ALL).title(" tasks "));
        frame.render_widget(list, area);
    }

    fn build_lines(&self, today: NaiveDate) -> Vec<Line<'static>> {
        let mut lines: Vec<Line> = Vec::with_capacity(self.matches.len() + 4);
        let upcoming_start = self.overdue_count;

        if upcoming_start > 0 {
            lines.push(divider_line(&format!("── overdue ({}) ──", upcoming_start)));
            for (i, &task_idx) in self.matches[..upcoming_start].iter().enumerate() {
                let selected = i == self.selected;
                lines.push(task_line(&self.tasks[task_idx], today, selected));
            }
        }
        if upcoming_start < self.matches.len() {
            let upcoming_n = self.matches.len() - upcoming_start;
            lines.push(divider_line(&format!("── upcoming ({}) ──", upcoming_n)));
            for (i, &task_idx) in self.matches[upcoming_start..].iter().enumerate() {
                let selected = (i + upcoming_start) == self.selected;
                lines.push(task_line(&self.tasks[task_idx], today, selected));
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
    }

    fn refresh(&mut self, ctx: &mut TabCtx) -> Result<()> {
        self.reload(ctx)
    }
}

impl SearchView {
    fn handle_edit_key(&mut self, k: KeyEvent, ctx: &mut TabCtx) -> EventOutcome {
        match (k.code, k.modifiers) {
            (KeyCode::Esc, _) => {
                self.cancel_edit();
            }
            (KeyCode::Enter, _) => {
                self.apply_edit(ctx);
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

fn task_line(task: &Task, today: NaiveDate, selected: bool) -> Line<'static> {
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

    // Description truncated to leave headroom for due (always shown when set)
    // and scheduled (shown when it fits). The viewport inner width at 80x24
    // is 54 cols; cursor(2) + pri(4) + desc(22) + " 📅 "(4) + date(10) = 42,
    // leaving room for " ⏳ "(4) + date(10) = 14 more before clipping.
    let desc = task.description.replace('\n', " ");
    let desc_trimmed = if desc.chars().count() > 22 {
        let cut: String = desc.chars().take(21).collect();
        format!("{cut}…")
    } else {
        desc
    };

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
    spans.push(Span::styled(format!("{:<22}", desc_trimmed), row_style));
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
