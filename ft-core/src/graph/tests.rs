//! End-to-end tests for [`Graph::build`] and [`Graph::refresh_note`]
//! against the dedicated `tests/fixtures/links/` vault.
//!
//! Parser-internal and resolver-internal tests live next to the code
//! they cover, in `parser.rs::parser_tests` and
//! `resolve.rs::resolve_tests`. The tests here exercise the full
//! parse → resolve → graph pipeline and the per-file refresh + ghost
//! cleanup paths.

use std::path::{Path, PathBuf};

use crate::graph::{EdgeKind, Graph, LinkForm, NodeKind, NoteId};
use crate::vault::Vault;

fn fixture_vault() -> Vault {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tests/fixtures/links");
    Vault::discover(Some(path)).expect("links fixture vault must exist")
}

fn note(graph: &Graph, rel: &str) -> NoteId {
    graph
        .note_by_path(Path::new(rel))
        .unwrap_or_else(|| panic!("no note for {rel}"))
}

fn outgoing_targets(graph: &Graph, src: NoteId) -> Vec<String> {
    graph
        .outgoing(src)
        .map(|(dst, edge)| {
            let kind_label = match graph.node(dst) {
                NodeKind::Note(n) => format!("note:{}", n.path.display()),
                NodeKind::Ghost(g) => format!("ghost:{}", g.raw),
            };
            let edge_kind = match edge {
                EdgeKind::Link(_) => "link",
                EdgeKind::Embed(_) => "embed",
            };
            let l = edge.link();
            format!(
                "{kind_label}|{edge_kind}|{:?}|target={}",
                l.form, l.target_text
            )
        })
        .collect()
}

#[test]
fn build_creates_one_node_per_markdown_file() {
    let v = fixture_vault();
    let g = Graph::build(&v).unwrap();
    let note_count = g
        .nodes()
        .filter(|(_, k)| matches!(k, NodeKind::Note(_)))
        .count();
    // hub, alpha, beta, gamma, sub/inner, sub/My Inner, Index,
    // archive/Index, collision-linker → 9 notes
    assert_eq!(note_count, 9, "expected 9 note nodes");
}

#[test]
fn hub_outgoing_covers_every_link_shape() {
    let v = fixture_vault();
    let g = Graph::build(&v).unwrap();
    let hub = note(&g, "notes/hub.md");
    let edges: Vec<&EdgeKind> = g.outgoing(hub).map(|(_, e)| e).collect();

    // Sanity: at least the wikilink + md + embed shapes from the
    // fixture all show up. Exact count below.
    let wiki = edges
        .iter()
        .filter(|e| e.link().form == LinkForm::WikiLink && matches!(e, EdgeKind::Link(_)))
        .count();
    let md = edges
        .iter()
        .filter(|e| e.link().form == LinkForm::MdLink && matches!(e, EdgeKind::Link(_)))
        .count();
    let wiki_embed = edges
        .iter()
        .filter(|e| e.link().form == LinkForm::WikiLink && matches!(e, EdgeKind::Embed(_)))
        .count();
    let md_embed = edges
        .iter()
        .filter(|e| e.link().form == LinkForm::MdLink && matches!(e, EdgeKind::Embed(_)))
        .count();

    // 8 wikilinks: alpha, beta|alias, gamma#anchor, gamma#anchor|alias,
    //              sub/inner, Phantom, alpha (repeat 1), alpha (repeat 2)
    assert_eq!(wiki, 8, "wikilinks");
    // 4 md links: alpha.md, beta (extless), sub/My Inner.md, missing.md
    assert_eq!(md, 4, "md links");
    // 2 wiki embeds: ![[alpha]], ![[diagram.png]]
    assert_eq!(wiki_embed, 2, "wiki embeds");
    // 1 md embed: ![alt](sub/inner.md)
    assert_eq!(md_embed, 1, "md embeds");
}

#[test]
fn fenced_and_indented_and_inline_code_are_skipped() {
    let v = fixture_vault();
    let g = Graph::build(&v).unwrap();
    let hub = note(&g, "notes/hub.md");
    // The hub has fenced and indented code blocks containing fake links;
    // those should not contribute outgoing edges. Total checked above.
    // Spot-check: the inline-code `[[alpha]]` doesn't add a 9th wikilink.
    let wiki_count = g
        .outgoing(hub)
        .filter(|(_, e)| matches!(e, EdgeKind::Link(_)) && e.link().form == LinkForm::WikiLink)
        .count();
    assert_eq!(wiki_count, 8);
}

#[test]
fn frontmatter_links_are_skipped() {
    let v = fixture_vault();
    let g = Graph::build(&v).unwrap();
    let alpha = note(&g, "notes/alpha.md");
    // alpha.md has a `[[Phantom]]` inside its frontmatter and a real
    // `[[hub]]` in the body. Only `hub` should appear.
    let targets: Vec<&str> = g
        .outgoing(alpha)
        .map(|(_, e)| e.link().target_text.as_str())
        .collect();
    assert_eq!(targets, vec!["hub"]);
}

#[test]
fn ghost_node_is_shared_across_linkers() {
    // hub.md and (we'll add via mutation) another note both point at
    // [[Phantom]]; the ghost is shared.
    let v = fixture_vault();
    let g = Graph::build(&v).unwrap();
    let phantom = g
        .ghost_by_raw("Phantom")
        .expect("Phantom ghost should exist");
    // Only hub.md links to Phantom in the fixture; one incoming edge.
    let incoming: Vec<_> = g.incoming(phantom).collect();
    assert_eq!(incoming.len(), 1);
}

#[test]
fn shortest_path_tiebreak_resolves_collision_linker_to_top_level_index() {
    let v = fixture_vault();
    let g = Graph::build(&v).unwrap();
    let linker = note(&g, "notes/collision-linker.md");
    let mut targets: Vec<PathBuf> = g
        .outgoing(linker)
        .filter_map(|(dst, _)| match g.node(dst) {
            NodeKind::Note(n) => Some(n.path.clone()),
            _ => None,
        })
        .collect();
    targets.sort();
    assert_eq!(targets, vec![PathBuf::from("Index.md")]);
}

#[test]
fn url_encoded_md_link_resolves() {
    let v = fixture_vault();
    let g = Graph::build(&v).unwrap();
    let hub = note(&g, "notes/hub.md");
    // Look for the edge whose raw_text is the URL-encoded form.
    let resolved = g
        .outgoing(hub)
        .filter(|(_, e)| e.link().raw_text.contains("%20"))
        .find_map(|(dst, _)| match g.node(dst) {
            NodeKind::Note(n) => Some(n.path.clone()),
            _ => None,
        });
    assert_eq!(
        resolved,
        Some(PathBuf::from("notes/sub/My Inner.md")),
        "URL-encoded path should resolve to the spaced filename"
    );
}

#[test]
fn external_urls_do_not_become_edges() {
    let v = fixture_vault();
    let g = Graph::build(&v).unwrap();
    let hub = note(&g, "notes/hub.md");
    for (_, e) in g.outgoing(hub) {
        let raw = &e.link().raw_text;
        assert!(
            !raw.contains("https://") && !raw.contains("mailto:"),
            "external URL leaked as an edge: {raw}"
        );
    }
}

#[test]
fn byte_ranges_round_trip_against_source_files() {
    let v = fixture_vault();
    let g = Graph::build(&v).unwrap();
    let hub_id = note(&g, "notes/hub.md");
    let abs = v.path.join("notes/hub.md");
    let content = std::fs::read_to_string(&abs).unwrap();
    for (_, edge) in g.outgoing(hub_id) {
        let l = edge.link();
        assert_eq!(
            &content[l.byte_range.clone()],
            l.raw_text,
            "byte_range did not round-trip for {:?}",
            l.raw_text
        );
    }
}

#[test]
fn refresh_note_replaces_outgoing_edges_and_preserves_incoming() {
    use std::io::Write as _;

    let tmp = assert_fs::TempDir::new().unwrap();
    use assert_fs::prelude::*;
    tmp.child(".obsidian").create_dir_all().unwrap();
    tmp.child("a.md").write_str("[[b]] [[c]]\n").unwrap();
    tmp.child("b.md").write_str("# b\n").unwrap();
    tmp.child("c.md").write_str("[[a]]\n").unwrap();

    let v = Vault::discover(Some(tmp.path().to_path_buf())).unwrap();
    let mut g = Graph::build(&v).unwrap();

    let a = note(&g, "a.md");
    let b = note(&g, "b.md");
    let c = note(&g, "c.md");
    assert_eq!(g.outgoing(a).count(), 2, "a starts with two outgoing");
    assert_eq!(g.incoming(a).count(), 1, "c links to a");

    // Mutate a.md: remove the [[b]] link, leave the [[c]] link.
    let mut f = std::fs::File::create(tmp.path().join("a.md")).unwrap();
    writeln!(f, "[[c]]").unwrap();
    drop(f);

    g.refresh_note(&v.path, &tmp.path().join("a.md")).unwrap();

    // Outgoing changed: only c remains.
    let outgoing: Vec<_> = g
        .outgoing(a)
        .filter_map(|(dst, _)| match g.node(dst) {
            NodeKind::Note(n) => Some(n.path.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(outgoing, vec![PathBuf::from("c.md")]);

    // Incoming to a is untouched (c.md still links to a).
    assert_eq!(g.incoming(a).count(), 1);
    // b lost its incoming edge from a.
    assert_eq!(g.incoming(b).count(), 0);
    let _ = c;
}

#[test]
fn refresh_note_garbage_collects_orphaned_ghost() {
    use assert_fs::prelude::*;
    use std::io::Write as _;

    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child(".obsidian").create_dir_all().unwrap();
    tmp.child("a.md").write_str("[[Phantom]]\n").unwrap();

    let v = Vault::discover(Some(tmp.path().to_path_buf())).unwrap();
    let mut g = Graph::build(&v).unwrap();
    assert!(g.ghost_by_raw("Phantom").is_some());

    // Remove the link from a.md.
    let mut f = std::fs::File::create(tmp.path().join("a.md")).unwrap();
    writeln!(f, "no links here").unwrap();
    drop(f);

    g.refresh_note(&v.path, &tmp.path().join("a.md")).unwrap();
    assert!(
        g.ghost_by_raw("Phantom").is_none(),
        "orphaned ghost should be removed"
    );
}

#[test]
fn refresh_note_keeps_ghost_when_other_linkers_remain() {
    use assert_fs::prelude::*;
    use std::io::Write as _;

    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child(".obsidian").create_dir_all().unwrap();
    tmp.child("a.md").write_str("[[Phantom]]\n").unwrap();
    tmp.child("b.md").write_str("[[Phantom]]\n").unwrap();

    let v = Vault::discover(Some(tmp.path().to_path_buf())).unwrap();
    let mut g = Graph::build(&v).unwrap();
    let phantom = g.ghost_by_raw("Phantom").unwrap();
    assert_eq!(g.incoming(phantom).count(), 2);

    // Remove the link from a.md only.
    let mut f = std::fs::File::create(tmp.path().join("a.md")).unwrap();
    writeln!(f, "nothing").unwrap();
    drop(f);

    g.refresh_note(&v.path, &tmp.path().join("a.md")).unwrap();
    let phantom = g
        .ghost_by_raw("Phantom")
        .expect("ghost should still exist (b still links)");
    assert_eq!(g.incoming(phantom).count(), 1);
}

#[test]
fn empty_vault_builds_empty_graph() {
    let tmp = assert_fs::TempDir::new().unwrap();
    use assert_fs::prelude::*;
    tmp.child(".obsidian").create_dir_all().unwrap();
    let v = Vault::discover(Some(tmp.path().to_path_buf())).unwrap();
    let g = Graph::build(&v).unwrap();
    assert_eq!(g.nodes().count(), 0);
}

#[test]
fn outgoing_visible_via_str_helper_for_debugging() {
    // Sanity that the debug helper this file uses doesn't blow up on
    // any node kind. (Exercised through fixture_vault.)
    let v = fixture_vault();
    let g = Graph::build(&v).unwrap();
    let hub = note(&g, "notes/hub.md");
    let dump = outgoing_targets(&g, hub);
    assert!(!dump.is_empty());
}
