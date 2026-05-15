//! Link target resolution.
//!
//! Maps a parsed link's `target_text` (plus the linker's path, for
//! markdown links) to a [`Resolution`]. Mirrors Obsidian's defaults:
//!
//! - **Wikilink with `/`** → vault-relative path lookup. Hit ⇒
//!   `Resolved`; miss ⇒ `Unresolved(target_text)`.
//! - **Wikilink without `/`** → title (filename stem) lookup. Zero
//!   matches ⇒ `Unresolved`. One match ⇒ `Resolved`. Multiple matches ⇒
//!   pick the **shortest path**, breaking ties **alphabetically by
//!   full vault-relative path** — Obsidian's "shortest path when
//!   possible" default.
//! - **Markdown link** → URL-decoded path interpreted relative to the
//!   linker's directory; `.md` suffix added if missing for the lookup,
//!   then path-index lookup. Miss ⇒ `Unresolved` keyed by the
//!   normalized vault-relative path string.
//!
//! External URLs (`http://`, `https://`, `mailto:`, `obsidian://`,
//! ...) are filtered out at the parser level — they never reach this
//! module.

use std::path::{Path, PathBuf};

use crate::graph::{normalize_path, Graph, NoteId};

#[derive(Debug)]
pub enum Resolution {
    Resolved(NoteId),
    /// Unresolved with the canonical key used to intern the ghost node.
    /// For wikilinks, this is the verbatim target text; for markdown
    /// links, the normalized vault-relative path string.
    Unresolved(String),
    /// The link doesn't refer to a vault note (e.g. external URL).
    /// In practice the parser already filters these — kept as a return
    /// option for defense-in-depth.
    NotALink,
}

/// Resolve a wikilink target string.
pub fn resolve_wiki(target: &str, graph: &Graph) -> Resolution {
    let target = target.trim();
    if target.is_empty() {
        return Resolution::NotALink;
    }

    if target.contains('/') {
        // Path form — try with and without `.md`.
        if let Some(id) = lookup_path_with_md_fallback(target, graph) {
            return Resolution::Resolved(id);
        }
        return Resolution::Unresolved(target.to_string());
    }

    // Title form — multi candidate, shortest-path tiebreak.
    let candidates = graph.note_by_title(target);
    match candidates.len() {
        0 => Resolution::Unresolved(target.to_string()),
        1 => Resolution::Resolved(candidates[0]),
        _ => Resolution::Resolved(pick_shortest_path(candidates, graph)),
    }
}

/// Resolve a markdown-link href against the linker's directory.
pub fn resolve_md(href: &str, from_rel: &Path, graph: &Graph) -> Resolution {
    let href = href.trim();
    if href.is_empty() {
        return Resolution::NotALink;
    }

    let base_dir = from_rel.parent().unwrap_or(Path::new(""));
    let joined = base_dir.join(href);
    let normalized = normalize_relative(&joined);

    if let Some(id) = lookup_path_with_md_fallback(&normalized.to_string_lossy(), graph) {
        return Resolution::Resolved(id);
    }
    // Use the normalized form as the ghost key so two linkers writing
    // `../foo.md` from sibling files share one ghost.
    Resolution::Unresolved(normalized.to_string_lossy().into_owned())
}

/// Try `path_index[s]` first, then `path_index[s + ".md"]`. Obsidian
/// accepts links without the `.md` suffix (`[[Foo]]` → `Foo.md`).
fn lookup_path_with_md_fallback(s: &str, graph: &Graph) -> Option<NoteId> {
    let p = PathBuf::from(s);
    if let Some(id) = graph.note_by_path(&p) {
        return Some(id);
    }
    if p.extension().is_none() {
        let with_md = PathBuf::from(format!("{s}.md"));
        if let Some(id) = graph.note_by_path(&with_md) {
            return Some(id);
        }
    }
    None
}

/// Pick the candidate with the fewest path components; break ties
/// alphabetically by the full vault-relative path (lexicographic on
/// the platform-string form, which is stable enough for this purpose).
fn pick_shortest_path(candidates: &[NoteId], graph: &Graph) -> NoteId {
    debug_assert!(!candidates.is_empty());
    let mut best = candidates[0];
    let (mut best_depth, mut best_path) = depth_and_path(graph, best);
    for &id in &candidates[1..] {
        let (depth, path) = depth_and_path(graph, id);
        if depth < best_depth || (depth == best_depth && path < best_path) {
            best = id;
            best_depth = depth;
            best_path = path;
        }
    }
    best
}

fn depth_and_path(graph: &Graph, id: NoteId) -> (usize, String) {
    use crate::graph::NodeKind;
    if let NodeKind::Note(data) = graph.node(id) {
        let depth = data.path.components().count();
        (depth, data.path.to_string_lossy().into_owned())
    } else {
        // title_index only ever contains Note nodes, but be defensive.
        (usize::MAX, String::new())
    }
}

/// Collapse `.` and (where possible) `..` components in a vault-relative
/// path. Pure path arithmetic — does not touch the filesystem. A `..`
/// that would escape the vault root is preserved verbatim (it'll fail
/// the path lookup, which is the correct outcome).
fn normalize_relative(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out: Vec<std::ffi::OsString> = Vec::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.last().map(|s| s.as_os_str()), Some(os) if os != "..") {
                    out.pop();
                } else {
                    out.push(std::ffi::OsString::from(".."));
                }
            }
            Component::Normal(s) => out.push(s.to_os_string()),
            Component::RootDir | Component::Prefix(_) => out.push(c.as_os_str().to_os_string()),
        }
    }
    let mut result = PathBuf::new();
    for s in out {
        result.push(s);
    }
    normalize_path(&result)
}

#[cfg(test)]
mod resolve_tests {
    use super::*;
    use crate::graph::Graph;
    use crate::vault::Vault;
    use assert_fs::prelude::*;
    use assert_fs::TempDir;

    fn make_vault(files: &[(&str, &str)]) -> (TempDir, Vault) {
        let dir = TempDir::new().unwrap();
        dir.child(".obsidian").create_dir_all().unwrap();
        for (rel, content) in files {
            dir.child(rel).write_str(content).unwrap();
        }
        let vault = Vault::discover(Some(dir.path().to_path_buf())).unwrap();
        (dir, vault)
    }

    #[test]
    fn wiki_title_resolves_when_unique() {
        let (_dir, v) = make_vault(&[("Foo.md", ""), ("Bar.md", "")]);
        let g = Graph::build(&v).unwrap();
        let r = resolve_wiki("Foo", &g);
        assert!(matches!(r, Resolution::Resolved(_)));
    }

    #[test]
    fn wiki_title_unresolved_when_missing() {
        let (_dir, v) = make_vault(&[("Foo.md", "")]);
        let g = Graph::build(&v).unwrap();
        match resolve_wiki("Missing", &g) {
            Resolution::Unresolved(key) => assert_eq!(key, "Missing"),
            r => panic!("expected unresolved, got {r:?}"),
        }
    }

    #[test]
    fn wiki_title_collision_picks_shortest_path() {
        let (_dir, v) = make_vault(&[
            ("Index.md", "top-level"),
            ("archive/Index.md", "archived"),
            ("deep/nested/Index.md", "very deep"),
        ]);
        let g = Graph::build(&v).unwrap();
        let r = resolve_wiki("Index", &g);
        match r {
            Resolution::Resolved(id) => match g.node(id) {
                crate::graph::NodeKind::Note(data) => {
                    assert_eq!(data.path, std::path::PathBuf::from("Index.md"));
                }
                _ => panic!("expected Note"),
            },
            _ => panic!("expected Resolved"),
        }
    }

    #[test]
    fn wiki_title_collision_alphabetical_when_depth_tied() {
        let (_dir, v) = make_vault(&[
            ("zeta/Same.md", ""),
            ("alpha/Same.md", ""),
            ("beta/Same.md", ""),
        ]);
        let g = Graph::build(&v).unwrap();
        match resolve_wiki("Same", &g) {
            Resolution::Resolved(id) => match g.node(id) {
                crate::graph::NodeKind::Note(data) => {
                    assert_eq!(data.path, std::path::PathBuf::from("alpha/Same.md"));
                }
                _ => panic!(),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn wiki_path_form_resolves() {
        let (_dir, v) = make_vault(&[("sub/Foo.md", "")]);
        let g = Graph::build(&v).unwrap();
        assert!(matches!(
            resolve_wiki("sub/Foo", &g),
            Resolution::Resolved(_)
        ));
        assert!(matches!(
            resolve_wiki("sub/Foo.md", &g),
            Resolution::Resolved(_)
        ));
    }

    #[test]
    fn md_link_resolves_relative_to_linker_dir() {
        let (_dir, v) = make_vault(&[("notes/from.md", ""), ("notes/target.md", "")]);
        let g = Graph::build(&v).unwrap();
        let r = resolve_md("target.md", std::path::Path::new("notes/from.md"), &g);
        assert!(matches!(r, Resolution::Resolved(_)));
    }

    #[test]
    fn md_link_handles_parent_dir_traversal() {
        let (_dir, v) = make_vault(&[("a/from.md", ""), ("b/target.md", "")]);
        let g = Graph::build(&v).unwrap();
        let r = resolve_md("../b/target.md", std::path::Path::new("a/from.md"), &g);
        assert!(matches!(r, Resolution::Resolved(_)));
    }

    #[test]
    fn md_link_unresolved_keys_normalized_path() {
        let (_dir, v) = make_vault(&[("from.md", "")]);
        let g = Graph::build(&v).unwrap();
        match resolve_md("./missing.md", std::path::Path::new("from.md"), &g) {
            Resolution::Unresolved(k) => assert_eq!(k, "missing.md"),
            r => panic!("expected unresolved, got {r:?}"),
        }
    }
}
