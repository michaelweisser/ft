//! Notes tab renderer. The idle body is a keymap-style panel; an opt-in
//! help overlay floats above it on `?`; the open-flow picker and the
//! section-move flow each render their own centered popup over the body.

use std::collections::BTreeSet;

use ft_core::markdown::Heading;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::tui::tab::TabCtx;
use crate::tui::tabs::notes::{is_implicitly_selected, NotesState, SectionMoveState};

/// Idle-panel keymap. Each row is `(keys, description)`. Kept identical to
/// the `?` help overlay so users see one canonical list.
const IDLE_KEYS: &[(&str, &str)] = &[
    ("o", "open file / heading"),
    ("m", "move section(s) to another file"),
    ("?", "show this help"),
    ("Esc", "close overlay"),
];

/// Open-flow picker keymap shown along the bottom while the open-flow
/// picker is on screen. Mirrors the bindings in `mod.rs`.
const OPEN_PICKER_KEYS: &[(&str, &str)] = &[
    ("Enter", "open in $EDITOR"),
    ("Ctrl+O", "open in Obsidian"),
    ("Esc", "back to idle"),
];

/// Footer keymap for step 1/4 of the section-move flow.
const MOVE_STEP_1_KEYS: &[(&str, &str)] = &[("Enter", "use source"), ("Esc", "cancel move")];

/// Footer keymap for step 2/4 (heading multi-select).
const MOVE_STEP_2_KEYS: &[(&str, &str)] = &[
    ("↑/↓", "focus"),
    ("Space", "toggle"),
    ("Enter", "next"),
    ("Esc", "back"),
];

/// Footer keymap for step 3/4 (target picker).
const MOVE_STEP_3_KEYS: &[(&str, &str)] = &[("Enter", "use target"), ("Esc", "back to selection")];

pub(super) fn render(
    frame: &mut Frame,
    area: Rect,
    _ctx: &TabCtx,
    state: &mut NotesState,
    show_help: bool,
) {
    render_idle_body(frame, area);

    match state {
        NotesState::Idle => {
            if show_help {
                render_help_overlay(frame, area);
            }
        }
        NotesState::OpenPicking { picker } => {
            render_picker_popup(
                frame,
                area,
                " open · pick file / heading ",
                picker,
                OPEN_PICKER_KEYS,
                None,
            );
        }
        NotesState::MoveSection(ms) => render_move_overlay(frame, area, ms),
    }
}

fn render_move_overlay(frame: &mut Frame, area: Rect, ms: &mut SectionMoveState) {
    match ms {
        SectionMoveState::SourcePicking { picker } => {
            render_picker_popup(
                frame,
                area,
                " move · 1/4 source ",
                picker,
                MOVE_STEP_1_KEYS,
                None,
            );
        }
        SectionMoveState::HeadingMultiSelect {
            source_rel,
            headings,
            selected,
            focus,
            ..
        } => {
            render_multiselect_popup(
                frame,
                area,
                source_rel.display().to_string(),
                headings,
                selected,
                *focus,
            );
        }
        SectionMoveState::TargetPicking {
            source_rel,
            clipboard,
            picker,
            error,
            ..
        } => {
            let title = format!(
                " move · 3/4 target · {} from {} ",
                clipboard.len(),
                source_rel.display()
            );
            render_picker_popup(
                frame,
                area,
                &title,
                picker,
                MOVE_STEP_3_KEYS,
                error.as_deref(),
            );
        }
    }
}

fn render_picker_popup(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    picker: &mut crate::tui::widgets::FuzzyPicker<crate::tui::widgets::VaultFilePickerSource>,
    keys: &[(&str, &str)],
    error: Option<&str>,
) {
    let popup = centered_rect(60, 70, area);
    frame.render_widget(Clear, popup);
    let outer = Block::default()
        .borders(Borders::ALL)
        .title(title.to_string())
        .border_style(Style::default().fg(Color::Cyan))
        .style(Style::default().bg(Color::Black));
    let inner = outer.inner(popup);
    frame.render_widget(outer, popup);

    let footer_height = if error.is_some() { 3 } else { 2 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(footer_height)])
        .split(inner);

    picker.render(frame, chunks[0]);

    let mut footer_lines: Vec<Line> = Vec::with_capacity(2);
    if let Some(msg) = error {
        footer_lines.push(Line::from(Span::styled(
            msg.to_string(),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
    }
    footer_lines.push(keymap_line(keys));
    frame.render_widget(
        Paragraph::new(footer_lines).alignment(Alignment::Center),
        chunks[1],
    );
}

fn render_multiselect_popup(
    frame: &mut Frame,
    area: Rect,
    source_label: String,
    headings: &[Heading],
    selected: &BTreeSet<usize>,
    focus: usize,
) {
    let popup = centered_rect(70, 70, area);
    frame.render_widget(Clear, popup);
    let title = format!(" move · 2/4 select · {source_label} ");
    let outer = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(Color::Cyan))
        .style(Style::default().bg(Color::Black));
    let inner = outer.inner(popup);
    frame.render_widget(outer, popup);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(inner);

    let body_area = chunks[0];
    let visible = body_area.height as usize;
    let total = headings.len();
    let scroll = compute_scroll(focus, visible, total);
    let end = (scroll + visible).min(total);

    let mut lines: Vec<Line> = Vec::with_capacity(end.saturating_sub(scroll));
    for i in scroll..end {
        lines.push(render_multiselect_row(headings, selected, focus, i));
    }
    frame.render_widget(Paragraph::new(lines), body_area);

    let footer = keymap_line(MOVE_STEP_2_KEYS);
    frame.render_widget(
        Paragraph::new(vec![Line::from(""), footer]).alignment(Alignment::Center),
        chunks[1],
    );
}

fn render_multiselect_row(
    headings: &[Heading],
    selected: &BTreeSet<usize>,
    focus: usize,
    i: usize,
) -> Line<'static> {
    let h = &headings[i];
    let explicit = selected.contains(&h.line);
    let implicit = !explicit && is_implicitly_selected(headings, i, selected);
    let marker = if explicit {
        "■"
    } else if implicit {
        "▣"
    } else {
        "□"
    };
    let cursor = if i == focus { "▶ " } else { "  " };
    let indent = "  ".repeat((h.level as usize).saturating_sub(1));
    let level_tag = format!("H{}  ", h.level);
    let row_style = if i == focus {
        Style::default()
            .bg(Color::Rgb(40, 40, 60))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let marker_style = if explicit {
        row_style.fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else if implicit {
        row_style.fg(Color::DarkGray)
    } else {
        row_style.fg(Color::White)
    };
    let text_style = if implicit {
        row_style.fg(Color::DarkGray)
    } else {
        row_style.fg(Color::White)
    };
    Line::from(vec![
        Span::styled(cursor, row_style),
        Span::styled(format!("{marker} "), marker_style),
        Span::styled(indent, row_style),
        Span::styled(level_tag, row_style.fg(Color::DarkGray)),
        Span::styled(h.text.clone(), text_style),
    ])
}

fn compute_scroll(focus: usize, visible: usize, total: usize) -> usize {
    if total == 0 || visible == 0 || focus < visible {
        return 0;
    }
    if focus >= total {
        return total.saturating_sub(visible);
    }
    focus + 1 - visible
}

fn keymap_line(keys: &[(&str, &str)]) -> Line<'static> {
    Line::from(
        keys.iter()
            .flat_map(|(k, d)| {
                vec![
                    Span::styled(
                        format!(" {k} "),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("{d}  "), Style::default().fg(Color::Gray)),
                ]
            })
            .collect::<Vec<_>>(),
    )
}

fn render_idle_body(frame: &mut Frame, area: Rect) {
    let outer = Block::default()
        .borders(Borders::ALL)
        .title(" notes ")
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let mut lines: Vec<Line> = Vec::with_capacity(IDLE_KEYS.len() + 3);
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Notes — Obsidian-flavoured editing",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
    for (key, desc) in IDLE_KEYS {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {key:<6}"),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(*desc, Style::default().fg(Color::White)),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_help_overlay(frame: &mut Frame, area: Rect) {
    let popup = centered_rect(50, 50, area);
    frame.render_widget(Clear, popup);

    let mut lines: Vec<Line> = Vec::with_capacity(IDLE_KEYS.len() + 4);
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Notes keybindings",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
    for (key, desc) in IDLE_KEYS {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {key:<8}"),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(*desc, Style::default().fg(Color::White)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  press ? or Esc to close",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" notes · help ")
        .style(Style::default().bg(Color::Black));
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

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
