//! Output formatter for link rows (backlinks / outgoing).
//!
//! Same shape across all four formats (`table` / `json` / `ndjson` /
//! `markdown`) so a single [`LinkRow`] type drives them all. The
//! flat shape also keeps the JSON / NDJSON wire format stable for
//! scripting consumers.

use std::path::PathBuf;

use comfy_table::presets::UTF8_FULL;
use comfy_table::{ContentArrangement, Table};
use ft_core::graph::{EdgeKind, Graph, LinkForm, NodeKind, NoteId};
use serde::Serialize;

/// One row in a backlinks / forward-links result.
#[derive(Debug, Clone, Serialize)]
pub struct LinkRow {
    /// Note doing the linking. For `backlinks`: each linker. For `links`:
    /// the queried note (same on every row).
    pub src: PathBuf,
    /// 1-indexed line in `src` where the link occurs.
    pub src_line: usize,
    /// Where the link points. For `backlinks` this is always the
    /// queried note (Resolved). For `links` it may be Resolved or
    /// Unresolved (ghost).
    pub dst: LinkRowTarget,
    /// `"wiki"` or `"md"`.
    pub form: &'static str,
    /// `true` for `![[...]]` and `![alt](...)`.
    pub embed: bool,
    /// Wiki alias or markdown link text.
    pub display: Option<String>,
    /// Heading anchor (post-`#`).
    pub anchor: Option<String>,
    /// Verbatim source token: `"[[Foo|alias]]"`, `"[Foo](foo.md)"`, etc.
    pub raw: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum LinkRowTarget {
    Resolved { path: PathBuf },
    Unresolved { raw: String },
}

impl LinkRow {
    /// Build a row from one outgoing edge: `src` is the queried note,
    /// `dst` is the edge destination (which may be Note or Ghost).
    pub fn from_outgoing(
        graph: &Graph,
        src_path: &std::path::Path,
        dst: NoteId,
        edge: &EdgeKind,
    ) -> Self {
        let link = edge.link();
        let dst_target = match graph.node(dst) {
            NodeKind::Note(n) => LinkRowTarget::Resolved {
                path: n.path.clone(),
            },
            NodeKind::Ghost(g) => LinkRowTarget::Unresolved { raw: g.raw.clone() },
        };
        Self {
            src: src_path.to_path_buf(),
            src_line: link.line,
            dst: dst_target,
            form: form_str(link.form),
            embed: matches!(edge, EdgeKind::Embed(_)),
            display: link.display.clone(),
            anchor: link.anchor.clone(),
            raw: link.raw_text.clone(),
        }
    }

    /// Build a row from one incoming edge: `src` is the linker (yielded
    /// by `Graph::incoming`), `dst` is the queried note (always
    /// Resolved for backlinks since the queried note exists by
    /// construction in this code path).
    pub fn from_incoming(
        graph: &Graph,
        src: NoteId,
        dst_path: &std::path::Path,
        edge: &EdgeKind,
    ) -> Self {
        let link = edge.link();
        let src_path = match graph.node(src) {
            NodeKind::Note(n) => n.path.clone(),
            NodeKind::Ghost(_) => {
                // Ghosts have no outgoing edges (they never link to
                // anything), so this branch shouldn't be reachable. Fall
                // back to a placeholder rather than panic.
                PathBuf::from("<ghost>")
            }
        };
        Self {
            src: src_path,
            src_line: link.line,
            dst: LinkRowTarget::Resolved {
                path: dst_path.to_path_buf(),
            },
            form: form_str(link.form),
            embed: matches!(edge, EdgeKind::Embed(_)),
            display: link.display.clone(),
            anchor: link.anchor.clone(),
            raw: link.raw_text.clone(),
        }
    }
}

fn form_str(f: LinkForm) -> &'static str {
    match f {
        LinkForm::WikiLink => "wiki",
        LinkForm::MdLink => "md",
    }
}

/// Which side of the row is "the queried note" — controls which column
/// the table renderer hides (the queried note is always identical on
/// every row, so showing it is noise).
#[derive(Debug, Clone, Copy)]
pub enum Direction {
    /// `backlinks`: queried note is on the right (`dst`); `src` varies.
    Backlinks,
    /// `links`: queried note is on the left (`src`); `dst` varies.
    Forward,
}

pub struct TableOpts {
    pub use_color: bool,
    pub direction: Direction,
}

pub fn render_table(rows: &[LinkRow], opts: TableOpts) -> String {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic);
    let _ = opts.use_color; // reserved for future per-row coloring

    match opts.direction {
        Direction::Backlinks => {
            table.set_header(vec!["Src", "Line", "Form", "Display", "Raw"]);
            for r in rows {
                table.add_row(vec![
                    r.src.display().to_string(),
                    r.src_line.to_string(),
                    form_label(r),
                    r.display.clone().unwrap_or_default(),
                    r.raw.clone(),
                ]);
            }
        }
        Direction::Forward => {
            table.set_header(vec!["Dst", "Line", "Form", "Display", "Raw"]);
            for r in rows {
                let dst_label = match &r.dst {
                    LinkRowTarget::Resolved { path } => path.display().to_string(),
                    LinkRowTarget::Unresolved { raw } => format!("? {raw}"),
                };
                table.add_row(vec![
                    dst_label,
                    r.src_line.to_string(),
                    form_label(r),
                    r.display.clone().unwrap_or_default(),
                    r.raw.clone(),
                ]);
            }
        }
    }
    table.to_string()
}

fn form_label(r: &LinkRow) -> String {
    if r.embed {
        format!("{}!", r.form)
    } else {
        r.form.to_string()
    }
}

pub fn render_json(rows: &[LinkRow]) -> anyhow::Result<()> {
    let stdout = std::io::stdout().lock();
    serde_json::to_writer_pretty(stdout, rows)?;
    println!();
    Ok(())
}

pub fn render_ndjson(rows: &[LinkRow]) -> anyhow::Result<()> {
    use std::io::Write as _;
    let mut out = std::io::stdout().lock();
    for r in rows {
        serde_json::to_writer(&mut out, r)?;
        writeln!(out)?;
    }
    Ok(())
}

/// Markdown bullet per row, pipeable back into another tool. Shape:
/// `- src.md:42 — [[Target|alias]]` (one per row, one row per link
/// occurrence). The shown link form is the verbatim `raw` token, so the
/// output round-trips visually.
pub fn render_markdown(rows: &[LinkRow]) -> String {
    let mut out = String::new();
    for r in rows {
        out.push_str(&format!(
            "- {}:{} — {}\n",
            r.src.display(),
            r.src_line,
            r.raw
        ));
    }
    out
}
