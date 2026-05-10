//! Generic fuzzy picker widget: a single-line input + scrollable result
//! list. Drives any [`PickerSource`] — the [`VaultFilePickerSource`]
//! shipping in this module is the file/heading source used by plan 004's
//! target field, but the same widget can serve future surfaces (command
//! palette, status picker, link inserter) without any rework.
//!
//! The widget is generic over the source's `Item` type so callers get
//! their concrete payload back (e.g. a [`ft_core::search::Hit`]) without
//! reaching for `Any` downcasts.
//!
//! Plan-004 hasn't wired the picker into the create flow yet, so several
//! items below show up as "unused" until that session lands. We keep the
//! API surface live (and unit-tested) so the next session can consume it
//! without reshaping; `#[allow(dead_code)]` annotations on the unused
//! handles are intentional and called out where they appear.

#![allow(dead_code)]

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ft_core::search::{fuzzy_find, Hit, Query, SearchOptions};
use ft_core::vault::Vault;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::tui::widgets::EditBuffer;

// ── public surface ───────────────────────────────────────────────────────

/// One row in the picker's result list.
#[derive(Debug, Clone)]
pub struct PickerItem<T> {
    /// Text shown in the row. The picker renders this verbatim, with any
    /// chars listed in `match_indices` highlighted.
    pub label: String,
    /// Char positions (not byte offsets) within `label` that should be
    /// rendered bold/highlighted to show why the row matched. May be empty.
    pub match_indices: Vec<u32>,
    /// Caller-defined payload returned on `Enter`.
    pub data: T,
}

/// A source of picker rows. Implementors compute `PickerItem`s for a given
/// query string. The picker is generic over the source's `Item` type so
/// callers don't pay for type erasure.
pub trait PickerSource {
    type Item;
    fn query(&mut self, q: &str, limit: usize) -> Vec<PickerItem<Self::Item>>;
}

/// What the caller should do after dispatching a key.
#[derive(Debug)]
pub enum PickerOutcome<T> {
    /// User pressed `Enter`; here's the highlighted row's data.
    Selected(T),
    /// User pressed `Esc`.
    Cancelled,
    /// Key handled internally (text edit, navigation, etc.); keep the
    /// picker open and redraw.
    StillOpen,
    /// Key not recognized by the picker; the caller's event chain can
    /// continue (used for keys like F1, mouse, etc.).
    NotHandled,
}

/// Single-input fuzzy picker. Hold one per modal that wants the UX.
///
/// The widget does not draw its own border or surrounding chrome — the
/// caller picks the [`Rect`] and any framing. This keeps it reusable as
/// either an inline panel or a floating popup body.
pub struct FuzzyPicker<S: PickerSource> {
    source: S,
    input: EditBuffer,
    items: Vec<PickerItem<S::Item>>,
    selected: usize,
    scroll: u16,
    /// Cache key for the last source query so we don't re-run on every
    /// non-input keystroke.
    last_query: Option<String>,
    /// Maximum rows to ask the source for.
    limit: usize,
}

impl<S: PickerSource> FuzzyPicker<S> {
    pub fn new(source: S) -> Self {
        Self {
            source,
            input: EditBuffer::default(),
            items: Vec::new(),
            selected: 0,
            scroll: 0,
            last_query: None,
            limit: 50,
        }
    }

    /// Override the default per-query result cap.
    #[allow(dead_code)]
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    pub fn input_text(&self) -> &str {
        &self.input.text
    }

    pub fn selected_item(&self) -> Option<&PickerItem<S::Item>> {
        self.items.get(self.selected)
    }

    /// Dispatch a key event. Most calls will be `StillOpen` (input edits
    /// + navigation); `Selected` / `Cancelled` end the picker lifecycle.
    pub fn handle_key(&mut self, key: KeyEvent) -> PickerOutcome<S::Item>
    where
        S::Item: Clone,
    {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => PickerOutcome::Cancelled,
            (KeyCode::Enter, _) => {
                if let Some(item) = self.items.get(self.selected) {
                    PickerOutcome::Selected(item.data.clone())
                } else {
                    PickerOutcome::StillOpen
                }
            }
            (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::CONTROL) => {
                self.select_prev();
                PickerOutcome::StillOpen
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::CONTROL) => {
                self.select_next();
                PickerOutcome::StillOpen
            }
            // Text-edit keys go through EditBuffer; mirror the same set the
            // existing query bar and edit popup use so Ctrl+W / Ctrl+⌫
            // already work via `delete_word_backward`.
            (KeyCode::Backspace, m)
                if m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::ALT) =>
            {
                self.input.delete_word_backward();
                self.refresh();
                PickerOutcome::StillOpen
            }
            (KeyCode::Char('w'), KeyModifiers::CONTROL) => {
                self.input.delete_word_backward();
                self.refresh();
                PickerOutcome::StillOpen
            }
            (KeyCode::Backspace, _) => {
                self.input.backspace();
                self.refresh();
                PickerOutcome::StillOpen
            }
            (KeyCode::Delete, _) => {
                self.input.delete();
                self.refresh();
                PickerOutcome::StillOpen
            }
            (KeyCode::Left, _) => {
                self.input.left();
                PickerOutcome::StillOpen
            }
            (KeyCode::Right, _) => {
                self.input.right();
                PickerOutcome::StillOpen
            }
            (KeyCode::Home, _) => {
                self.input.home();
                PickerOutcome::StillOpen
            }
            (KeyCode::End, _) => {
                self.input.end();
                PickerOutcome::StillOpen
            }
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => {
                self.input.insert(c);
                self.refresh();
                PickerOutcome::StillOpen
            }
            _ => PickerOutcome::NotHandled,
        }
    }

    /// Re-run the source query if the input changed since the last refresh.
    fn refresh(&mut self) {
        let current = self.input.text.clone();
        if self.last_query.as_deref() == Some(&current) {
            return;
        }
        self.items = self.source.query(&current, self.limit);
        self.selected = 0;
        self.scroll = 0;
        self.last_query = Some(current);
    }

    fn select_prev(&mut self) {
        if self.items.is_empty() {
            return;
        }
        if self.selected == 0 {
            self.selected = self.items.len() - 1;
        } else {
            self.selected -= 1;
        }
    }

    fn select_next(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.items.len();
    }

    /// Render the picker into `area`. Layout: 3-row input panel (border +
    /// text + border) on top, the rest for the result list. The caller is
    /// responsible for clearing the underlying buffer if it's a popup.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        // Carve out top input row + bottom list area.
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(1)])
            .split(area);

        self.render_input(frame, chunks[0]);
        self.adjust_scroll(chunks[1].height);
        self.render_list(frame, chunks[1]);
    }

    fn render_input(&self, frame: &mut Frame, area: Rect) {
        let chars: Vec<char> = self.input.text.chars().collect();
        let cursor = self.input.cursor.min(chars.len());
        let line = if chars.is_empty() {
            Line::from(Span::styled(
                "type to search…",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            ))
        } else {
            let mut iter = chars.iter().copied();
            let left: String = iter.by_ref().take(cursor).collect();
            let right: String = iter.collect();
            Line::from(vec![
                Span::styled(left, Style::default().fg(Color::White)),
                Span::styled(
                    "│",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(right, Style::default().fg(Color::White)),
            ])
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title(" find ");
        frame.render_widget(Paragraph::new(line).block(block), area);
    }

    fn render_list(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default().borders(Borders::ALL).title(" results ");
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.input.text.is_empty() {
            let hint = Paragraph::new(Line::from(Span::styled(
                "type to search…",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )))
            .alignment(Alignment::Center);
            frame.render_widget(hint, inner);
            return;
        }

        if self.items.is_empty() {
            let hint = Paragraph::new(Line::from(Span::styled(
                "no matches",
                Style::default().fg(Color::DarkGray),
            )))
            .alignment(Alignment::Center);
            frame.render_widget(hint, inner);
            return;
        }

        let visible_rows = inner.height as usize;
        let start = self.scroll as usize;
        let end = (start + visible_rows).min(self.items.len());
        let label_width = inner.width.saturating_sub(2) as usize; // cursor (2)

        let mut lines: Vec<Line> = Vec::with_capacity(end.saturating_sub(start));
        for i in start..end {
            let item = &self.items[i];
            lines.push(render_row(item, i == self.selected, label_width));
        }
        let para = Paragraph::new(lines);
        frame.render_widget(para, inner);
    }

    fn adjust_scroll(&mut self, list_height: u16) {
        let visible = list_height.saturating_sub(2).max(1); // minus the borders
        let selected = self.selected as u16;
        if selected < self.scroll {
            self.scroll = selected;
        } else if selected >= self.scroll + visible {
            self.scroll = selected + 1 - visible;
        }
    }
}

/// Render one row with cursor + highlighted match chars + plain rest.
fn render_row<T>(item: &PickerItem<T>, selected: bool, label_width: usize) -> Line<'static> {
    let cursor = if selected { "▶ " } else { "  " };
    let row_style = if selected {
        Style::default()
            .bg(Color::Rgb(40, 40, 60))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let label: String = item.label.chars().take(label_width).collect();
    // Map each char position to either highlighted or plain — emit spans
    // greedily by walking runs of like-kind positions.
    let highlight_set: std::collections::HashSet<u32> =
        item.match_indices.iter().copied().collect();
    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled(cursor.to_string(), row_style));
    let mut run = String::new();
    let mut run_highlight = false;
    for (i, c) in label.chars().enumerate() {
        let hi = highlight_set.contains(&(i as u32));
        if hi != run_highlight && !run.is_empty() {
            spans.push(make_span(
                std::mem::take(&mut run),
                run_highlight,
                row_style,
            ));
        }
        run.push(c);
        run_highlight = hi;
    }
    if !run.is_empty() {
        spans.push(make_span(run, run_highlight, row_style));
    }
    Line::from(spans)
}

fn make_span(text: String, highlight: bool, row_style: Style) -> Span<'static> {
    if highlight {
        Span::styled(
            text,
            row_style
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )
    } else {
        Span::styled(text, row_style.fg(Color::White))
    }
}

// ── concrete source: vault files + headings ─────────────────────────────

/// File / heading [`PickerSource`] backed by [`Vault::fuzzy_find`].
///
/// Holds two `Matcher`s (one path-aware for the file part, one default for
/// heading text) so the indices we compute for highlighting match the
/// scoring done inside `fuzzy_find`.
pub struct VaultFilePickerSource<'v> {
    vault: &'v Vault,
    path_matcher: Matcher,
    text_matcher: Matcher,
}

impl<'v> VaultFilePickerSource<'v> {
    pub fn new(vault: &'v Vault) -> Self {
        Self {
            vault,
            path_matcher: Matcher::new(Config::DEFAULT.match_paths()),
            text_matcher: Matcher::new(Config::DEFAULT),
        }
    }
}

impl<'v> PickerSource for VaultFilePickerSource<'v> {
    type Item = Hit;

    fn query(&mut self, q: &str, limit: usize) -> Vec<PickerItem<Hit>> {
        let parsed = Query::parse(q);
        if parsed.is_empty() {
            return Vec::new();
        }
        let hits = fuzzy_find(
            self.vault,
            &parsed,
            SearchOptions {
                limit,
                include_headings: parsed.heading_part.is_some(),
            },
        );

        let file_pat = (!parsed.file_part.is_empty()).then(|| {
            Pattern::parse(
                &parsed.file_part,
                CaseMatching::Ignore,
                Normalization::Smart,
            )
        });
        let head_pat = parsed
            .heading_part
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|p| Pattern::parse(p, CaseMatching::Ignore, Normalization::Smart));

        hits.into_iter()
            .map(|hit| {
                let (label, match_indices) = build_label_with_indices(
                    &hit,
                    file_pat.as_ref(),
                    head_pat.as_ref(),
                    &mut self.path_matcher,
                    &mut self.text_matcher,
                );
                PickerItem {
                    label,
                    match_indices,
                    data: hit,
                }
            })
            .collect()
    }
}

/// Compose a row label and char-level match indices. The label has two
/// portions: the relative path, then (optionally) ` · <heading text>`.
/// File-part highlights map to positions in the path portion; heading-part
/// highlights are offset past the path and the separator.
fn build_label_with_indices(
    hit: &Hit,
    file_pat: Option<&Pattern>,
    head_pat: Option<&Pattern>,
    path_matcher: &mut Matcher,
    text_matcher: &mut Matcher,
) -> (String, Vec<u32>) {
    let path_str = hit.path.display().to_string();
    let mut indices = Vec::new();

    if let Some(pat) = file_pat {
        let mut buf: Vec<char> = Vec::new();
        let haystack = Utf32Str::new(&path_str, &mut buf);
        let mut local = Vec::new();
        if pat.indices(haystack, path_matcher, &mut local).is_some() {
            indices.extend(local);
        }
    }

    let path_len = path_str.chars().count() as u32;

    let label = if let Some(h) = &hit.heading {
        let sep = " · ";
        let sep_len = sep.chars().count() as u32;
        let label = format!("{path_str}{sep}{}", h.text);
        if let Some(pat) = head_pat {
            let mut buf: Vec<char> = Vec::new();
            let haystack = Utf32Str::new(&h.text, &mut buf);
            let mut local = Vec::new();
            if pat.indices(haystack, text_matcher, &mut local).is_some() {
                let offset = path_len + sep_len;
                indices.extend(local.into_iter().map(|i| i + offset));
            }
        }
        label
    } else {
        path_str
    };

    indices.sort_unstable();
    indices.dedup();
    (label, indices)
}

// ── tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// Tiny in-test source used for behavioral assertions without the cost
    /// of building a real vault.
    struct StaticSource {
        rows: Vec<PickerItem<String>>,
    }
    impl PickerSource for StaticSource {
        type Item = String;
        fn query(&mut self, q: &str, _limit: usize) -> Vec<PickerItem<String>> {
            // Trivial filter: substring match, keep all matching rows.
            let lower = q.to_lowercase();
            self.rows
                .iter()
                .filter(|r| r.label.to_lowercase().contains(&lower))
                .cloned()
                .collect()
        }
    }

    fn rows_a_b_c() -> StaticSource {
        StaticSource {
            rows: vec![
                PickerItem {
                    label: "alpha".into(),
                    match_indices: vec![0, 1],
                    data: "alpha".into(),
                },
                PickerItem {
                    label: "beta".into(),
                    match_indices: vec![],
                    data: "beta".into(),
                },
                PickerItem {
                    label: "gamma".into(),
                    match_indices: vec![],
                    data: "gamma".into(),
                },
            ],
        }
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn render_str<S: PickerSource>(picker: &mut FuzzyPicker<S>, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| picker.render(f, f.area())).unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buf.area().height {
            for x in 0..buf.area().width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn empty_input_shows_hint() {
        let mut p = FuzzyPicker::new(rows_a_b_c());
        let frame = render_str(&mut p, 50, 10);
        insta::assert_snapshot!("picker_empty_50x10", frame);
    }

    #[test]
    fn populated_list_renders() {
        let mut p = FuzzyPicker::new(rows_a_b_c());
        for c in "a".chars() {
            p.handle_key(key(c));
        }
        let frame = render_str(&mut p, 50, 10);
        insta::assert_snapshot!("picker_populated_50x10", frame);
    }

    #[test]
    fn no_match_state_renders() {
        let mut p = FuzzyPicker::new(rows_a_b_c());
        for c in "zzzz".chars() {
            p.handle_key(key(c));
        }
        let frame = render_str(&mut p, 50, 10);
        insta::assert_snapshot!("picker_no_match_50x10", frame);
    }

    #[test]
    fn narrow_width_still_readable() {
        let mut p = FuzzyPicker::new(rows_a_b_c());
        for c in "a".chars() {
            p.handle_key(key(c));
        }
        let frame = render_str(&mut p, 40, 8);
        insta::assert_snapshot!("picker_narrow_40x8", frame);
    }

    #[test]
    fn enter_returns_selected_item() {
        let mut p = FuzzyPicker::new(rows_a_b_c());
        for c in "alpha".chars() {
            p.handle_key(key(c));
        }
        let out = p.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match out {
            PickerOutcome::Selected(s) => assert_eq!(s, "alpha"),
            other => panic!("expected Selected, got {other:?}"),
        }
    }

    #[test]
    fn esc_returns_cancelled() {
        let mut p = FuzzyPicker::new(rows_a_b_c());
        let out = p.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(out, PickerOutcome::Cancelled));
    }

    #[test]
    fn arrows_navigate_selection_with_wrap() {
        let mut p = FuzzyPicker::new(rows_a_b_c());
        // Empty query matches everything in the substring filter, so 3 rows.
        // Trigger a refresh by inserting then deleting a char.
        p.handle_key(key('a'));
        p.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(p.items.len(), 3);
        // Up from 0 should wrap to last.
        p.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(p.selected, 2);
        // Down should wrap back to 0.
        p.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(p.selected, 0);
    }

    #[test]
    fn ctrl_w_deletes_word_in_input() {
        let mut p = FuzzyPicker::new(rows_a_b_c());
        for c in "foo bar".chars() {
            p.handle_key(key(c));
        }
        assert_eq!(p.input_text(), "foo bar");
        p.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(p.input_text(), "foo ");
    }

    // ── VaultFilePickerSource against a real synthetic vault ────────────

    fn make_vault(files: &[(&str, &str)]) -> (TempDir, Vault) {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("vault");
        std::fs::create_dir_all(root.join(".obsidian")).unwrap();
        for (rel, body) in files {
            let path = root.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, body).unwrap();
        }
        let vault = Vault::discover(Some(root)).unwrap();
        (dir, vault)
    }

    #[test]
    fn vault_picker_returns_hit_with_match_indices() {
        let (_dir, vault) = make_vault(&[
            ("General Considerations.md", "# Intro\n### First Try\n"),
            ("unrelated.md", "# Z\n"),
        ]);
        let mut src = VaultFilePickerSource::new(&vault);
        let items = src.query("gen consid#Firs", 10);
        assert!(!items.is_empty(), "expected at least one item");
        let top = &items[0];
        assert!(
            top.label.contains("General Considerations"),
            "label should include path: {}",
            top.label
        );
        assert!(
            top.label.contains("First Try"),
            "label should include heading: {}",
            top.label
        );
        assert!(
            !top.match_indices.is_empty(),
            "match indices should be populated for a fuzzy hit"
        );
        // The heading payload survives so the picker caller can use it.
        assert!(top.data.heading.is_some());
    }

    #[test]
    fn vault_picker_empty_query_returns_empty() {
        let (_dir, vault) = make_vault(&[("a.md", "# A\n")]);
        let mut src = VaultFilePickerSource::new(&vault);
        assert!(src.query("", 10).is_empty());
    }
}
