---
id: 013
name: graph
title: Note graph: foundation library + backlinks/forwardlinks + rename
status: finished
created: 2026-05-15
updated: 2026-05-15
---

# Note graph: foundation library + backlinks/forwardlinks + rename

## Goal

A foundational graph capability in `ft_core` that models notes as nodes
and links between notes as edges, with a public API that future features
build on top of (rename, merge, split, "find related," navigation,
refactoring, …). The graph is built fully in memory from the vault
contents, supports incremental refresh on a per-file basis, and exposes
operations as **plans** (pure data describing edits) that callers apply
separately — mirroring the `task::ops::plan_move` / `apply_move_plan`
pattern already established in the codebase.

This plan ships three things end-to-end:

1. **Library — `ft_core::graph`**: link parser (wikilinks, markdown
   links, embeds), heterogeneous-from-day-one node/edge enums,
   Obsidian-default resolution rules including ghost-node materialization
   for unresolved targets, full-vault build, single-file incremental
   refresh, `incoming` / `outgoing` queries.
2. **CLI — read side**: `ft notes backlinks <note>` and `ft notes links
   <note>` (forward) with the standard `table` / `json` / `ndjson` /
   `markdown` output formats. These exercise the read path of the graph
   end-to-end before any mutating operation ships.
3. **Library + CLI — rename**: `plan_rename(graph, NoteId, &new_path)
   -> RenamePlan` plus `apply_rename_plan(&plan)` that writes via
   `fs::write_atomic`, applying same-file edits in **descending byte
   order** so earlier ranges remain valid; freshness guard via
   `(mtime, len)` snapshot per touched file; ghost-node rename support
   so unresolved targets can be "renamed" too (rewriting every linker
   even though the target file doesn't yet exist). CLI surface
   `ft notes rename <note> <new-name-or-path> [--dry-run]` matching
   `mv` ergonomics (bare new name → same directory; path with `/` →
   full move).

## Motivation and Context

`ft_core` already has the read primitives for tasks, headings,
sections, and the vault scan, plus mutation primitives that follow a
plan/apply split (`task::ops::plan_move` returns a `MovePlan` that
`apply_move_plan` writes). What it's missing is **structural knowledge
of how notes connect to each other**. Today:

- `task::ops` has a literal TODO at `src/task/ops.rs:640` to "rewrite
  `[[wikilinks]]` when the target file is in a moved task" — a hole
  that exists precisely because there's nothing in `ft_core` that knows
  what a wikilink resolves to.
- Renaming a note means hand-editing every linker, or trusting Obsidian
  to do it on the next launch (which only works if Obsidian is the
  thing performing the rename — `ft notes rename` and `mv` are
  invisible to it).
- Future features the user has lined up — backlinks pane in the TUI,
  "merge two notes," "split a note," "find related," refactoring
  navigation — all share the same prerequisite: a queryable in-memory
  representation of the link structure.

Why ship the read side (`backlinks` / `links`) before the mutating
side (`rename`):

1. The read commands cost almost nothing on top of the library — they're
   one-page CLI files that call `Graph::incoming` / `Graph::outgoing`
   and pipe through the existing `output/` formatters.
2. They exercise the parser, resolution, and ghost-node logic against
   real vaults before `plan_rename` builds on the same primitives. Bugs
   surface earlier and against simpler call sites.
3. They're independently useful — `ft notes backlinks today.md` is a
   shippable feature on its own.

Why design the graph as heterogeneous from day one (even though v1 only
has `NodeKind::Note | NodeKind::Ghost` and `EdgeKind::Link |
EdgeKind::Embed`): the user has explicitly flagged that future
extensions — folder/file structure as nodes, frontmatter values, tasks,
tags — should slot in additively. The `enum NodeKind { … }` /
`enum EdgeKind { … }` shape costs nothing structurally (petgraph
carries arbitrary weights) and avoids the "we have to rewrite the whole
graph for tasks" plan in six months.

Why no disk cache in v1: the existing rayon parallel scan parses a
~5k-note vault in well under a second. Disk caching adds a real
correctness surface (stale cache vs. edits made outside ft, especially
post-`git sync` from plan 012) for a speed win that isn't yet a
problem. Incremental in-memory refresh handles the per-edit hot path —
re-parsing one file is cheap. A `(path, mtime, hash)`-keyed disk cache
is an opt-in v2 feature when someone proves startup time is the
bottleneck.

## Acceptance Criteria

### Library — `ft_core::graph` (Session 1)

- [x] New module `ft-core/src/graph/` (folder) with `mod.rs`,
      `parser.rs`, `resolve.rs`, and `tests.rs`. `pub mod graph;` added
      to `lib.rs`.
- [x] **Identity model**:
      - Internal: `pub struct NoteId(NodeIndex);` — newtype wrapping
        petgraph's `NodeIndex`. Cheap, stable for the graph's lifetime,
        not exposed in serialized output.
      - External: vault-relative `PathBuf` (existing convention).
      - Side-tables on `Graph`:
        - `path_index: HashMap<PathBuf, NoteId>` — exact path lookup.
          Path canonicalization uses `Path::components` joined with
          `/` so Windows `\` and case differences on case-insensitive
          filesystems are handled consistently. v1 documents
          "case-sensitive matching, follows Obsidian"; case-insensitive
          fallback is a noted future improvement, not in scope.
        - `title_index: HashMap<String, Vec<NoteId>>` — title → all
          notes whose stem (filename without `.md`) matches. Multi
          because titles aren't unique.
- [x] **Node and edge types**:
      ```rust
      pub enum NodeKind {
          Note(NoteData),
          Ghost(GhostData),     // unresolved link target
          // Future: Folder, Task, Tag, FrontmatterValue
      }
      pub struct NoteData { pub path: PathBuf, pub title: String }
      pub struct GhostData { pub raw: String }   // the unresolved string

      pub enum EdgeKind {
          Link(LinkEdge),
          Embed(LinkEdge),     // ![[...]] — same shape, distinct kind
          // Future: Contains, HasTag, HasTask, Mentions
      }

      pub enum LinkForm { WikiLink, MdLink }
      pub enum LinkTarget {
          Resolved(NoteId),
          Unresolved(NoteId),  // points at a Ghost node — uniform traversal
      }
      pub struct LinkEdge {
          pub form: LinkForm,
          pub byte_range: std::ops::Range<usize>,  // in source-file content at parse time
          pub raw_text: String,                    // verbatim source token, e.g. "[[Foo|alias]]"
          pub target_text: String,                 // pre-pipe, pre-anchor: "Foo"
          pub anchor: Option<String>,              // post-#, e.g. "Heading"
          pub display: Option<String>,             // post-pipe alias OR md link text
      }
      ```
      Both `Link` and `Embed` carry a `LinkEdge` so the rewrite logic
      is shared; the distinction lives in the variant for callers that
      care (e.g. "find embeds of this image-style note").
- [x] **Parser** in `parser.rs`:
      - Pure: `pub fn extract_links(content: &str) -> Vec<RawLink>`,
        where `RawLink` is the unresolved per-occurrence record (form,
        byte_range, raw_text, target_text, anchor, display, is_embed).
      - Wikilinks: matches `[[target]]`, `[[target|display]]`,
        `[[target#anchor]]`, `[[target#anchor|display]]`, and all four
        with a leading `!` for embeds. Inner `]]` and `|` inside
        `[[…]]` are not nested (Obsidian doesn't support them either) —
        first `]]` closes.
      - Markdown links: matches `[text](url)` where `url` is a
        relative path ending in `.md` (or with no extension — Obsidian
        accepts `[x](foo)`); URL-decoded for path lookup. External URLs
        (`http://`, `https://`, `mailto:`) are **not** edges. Image
        links `![text](url)` are embeds when `url` resolves to a vault
        file. Reference-style links (`[text][ref]`) are out of scope
        for v1 — uncommon in Obsidian; documented as a known gap.
      - Frontmatter and code-fence skipping: reuse the same scanner
        state as `markdown.rs::ScanState` (frontmatter detection on
        first `---`, fenced blocks for ```` ``` ```` / `~~~`,
        4-space indented code blocks). Inline code spans (single /
        double / triple backtick runs) skip too — `[[Foo]]` inside
        `` `[[Foo]]` `` is not a link. Factor the shared scanner state
        out of `markdown.rs` into a small `markdown::scan` helper that
        both modules consume, rather than duplicating.
      - Returns links in document order. Byte ranges are start-of-`[[`
        (or start-of-`![[` for embeds, start-of-`[` for md links) to
        end-of-`]]` (or end-of-`)` for md links). Round-trip:
        `&content[r.byte_range]` exactly equals `r.raw_text`.
- [x] **Resolution** in `resolve.rs`:
      - `pub fn resolve_wiki(target: &str, all: &Graph, from: NoteId)
        -> LinkTarget` — implements Obsidian's "shortest path when
        possible" rule:
        1. If `target` contains a `/`, treat it as a vault-relative
           path; `path_index` lookup decides Resolved vs Unresolved.
        2. Otherwise, look up `title_index[target]`. Zero matches →
           Unresolved. One match → Resolved. Multiple matches → pick
           the one with the **shortest path** (fewest path components),
           breaking ties **alphabetically by full path**. Documented
           with a citation to the Obsidian setting in the module docs.
      - `pub fn resolve_md(href: &str, from: NoteId, all: &Graph)
        -> LinkTarget` — interprets the href relative to the directory
        of `from`'s note path; URL-decoded; `.md` suffix optional;
        `path_index` lookup decides. External URLs (`http://`,
        `https://`, `mailto:`, `obsidian://`) bail out before this
        function is called — they're not edges at all.
      - **Ghost materialization**: when resolution fails, the graph
        creates (or reuses, if one already exists for the same `raw`
        string) a `NodeKind::Ghost { raw: target.to_string() }` node
        and the edge points at it via `LinkTarget::Unresolved(ghost_id)`.
        Ghost nodes are keyed in a separate
        `ghost_index: HashMap<String, NoteId>` so two unresolved links
        to `[[Foo]]` from different notes share one ghost node. This
        unifies traversal — a ghost has incoming edges just like a
        real note does, which is exactly what makes "rename a
        not-yet-created note" work.
- [x] **`Graph` API surface**:
      ```rust
      pub struct Graph { /* graph + side-tables */ }

      impl Graph {
          /// Build the graph by scanning every markdown file in the
          /// vault in parallel (rayon), parsing links, and resolving
          /// targets. Honors the same ignore rules as `Vault::scan`.
          pub fn build(vault: &Vault) -> Result<Graph>;

          /// Re-parse one file. Removes its existing outgoing edges,
          /// re-extracts and re-resolves links from the new content,
          /// inserts the new edges. Incoming edges to this note are
          /// untouched (they belong to other notes' outgoing sets).
          /// Ghost cleanup: if removing this note's outgoing edges
          /// orphans a ghost (no remaining incoming), remove the
          /// ghost too.
          pub fn refresh_note(&mut self, path: &Path) -> Result<()>;

          pub fn note_by_path(&self, p: &Path) -> Option<NoteId>;
          pub fn note_by_title(&self, t: &str) -> &[NoteId];
          pub fn ghost_by_raw(&self, raw: &str) -> Option<NoteId>;
          pub fn node(&self, id: NoteId) -> &NodeKind;
          pub fn nodes(&self) -> impl Iterator<Item = (NoteId, &NodeKind)>;

          /// Edges where `id` is the source. Includes edges to ghosts.
          pub fn outgoing(&self, id: NoteId)
              -> impl Iterator<Item = (NoteId /* dst */, &EdgeKind)>;
          /// Edges where `id` is the destination (incoming = backlinks).
          pub fn incoming(&self, id: NoteId)
              -> impl Iterator<Item = (NoteId /* src */, &EdgeKind)>;
      }
      ```
- [x] **Test fixtures**: dedicated `tests/fixtures/links/` vault
      (separate from realistic/pathological so existing task tests
      stay untouched) covering:
      - Plain `[[Note]]` resolved to existing note
      - `[[Note|Display]]` alias preserved on edge
      - `[[Note#Heading]]` anchor preserved on edge
      - `[[Note#Heading|Display]]`
      - `[[Subdir/Note]]` path-form
      - `[[Missing]]` → ghost node
      - Title collision: two notes both named `Index.md` in different
        subdirs — one at top level and one under `archive/`; verify
        shortest-path tiebreak picks the top-level one
      - `![[image.png]]` and `![[Note]]` embeds
      - `[text](path/to/note.md)` markdown link
      - `[text](note)` extension-less markdown link
      - Links inside fenced code (```` ``` ````), indented code, and
        inline code spans → all skipped
      - Links inside frontmatter → skipped
      - URL-encoded paths in markdown links: `[x](My%20Note.md)` →
        resolves to `My Note.md`
      - External URLs (`[text](https://…)`, `[x](mailto:y)`) → not
        edges
      - Same target linked twice from one note → two edges
- [x] **Unit tests** (in `graph/tests.rs`) for the build path:
      - Empty vault → empty graph
      - Realistic fixture builds with the expected node/edge counts
        (snapshot the structure, not the exact byte ranges, so heading
        edits don't break the test)
      - `refresh_note` round-trip: read content, mutate (add a link),
        write, refresh — outgoing edges reflect the change; incoming
        edges to the changed note from other files are unchanged
      - Ghost cleanup on refresh: a note that links to `[[Phantom]]`
        is the only linker; refresh after removing the link → ghost
        node is gone
      - Two notes share a ghost: removing one linker keeps the ghost;
        removing both removes it
- [x] No new dependencies on the workspace if avoidable.
      `petgraph` is the obvious choice for the graph backbone; check
      whether anything in the workspace already pulls it in
      transitively before adding it as a direct dep. If we do add it,
      pin to a specific minor version and prefer `StableGraph` (so
      `NodeIndex` doesn't reshuffle on removal — important for
      `refresh_note`).
- [x] `cargo test --workspace`, `cargo clippy --workspace --all-targets
      -- -D warnings`, and `cargo fmt --check` clean.

### CLI — `ft notes backlinks` and `ft notes links` (Session 2)

- [x] Two new subcommands under `ft notes`:
      ```
      ft notes backlinks <note> [--format <fmt>]
      ft notes links     <note> [--format <fmt>]
      ```
      `<note>` accepts the same shapes as the existing notes commands:
      vault-relative path, fuzzy match, or bare title. Resolves via
      the existing `selector` module (extend it minimally if needed —
      `selector::resolve_note(query, vault)` may already do this; if
      not, factor it from the existing `notes open` flow).
- [x] Output: a uniform `LinkRow` shape across both commands and all
      formats, so a single `ft/src/output/links.rs` formatter handles
      table / json / ndjson / markdown:
      ```
      LinkRow {
          src: PathBuf,           // note doing the linking
          src_line: usize,        // 1-indexed line in src
          dst: LinkRowTarget,     // resolved path or unresolved raw
          form: "wiki" | "md",
          embed: bool,            // ![[ ]] or ![]( )
          display: Option<String>,
          anchor: Option<String>,
          raw: String,            // the verbatim source token
      }
      enum LinkRowTarget {
          Resolved { path: PathBuf },
          Unresolved { raw: String },
      }
      ```
      For `backlinks`: `src` is each linker; `dst` is the target note
      (always Resolved — it's the queried note). For `links`: `src` is
      the queried note; `dst` may be Resolved or Unresolved.
- [x] Format details:
      - `table` (default, TTY-aware color): columns `Src`, `Line`,
        `Form`, `Display`, `Raw` for backlinks; `Dst`, `Line`, `Form`,
        `Display`, `Raw` for links. Empty result → "no backlinks" /
        "no outgoing links" with exit 0 (or exit 1 unless
        `--allow-empty` per the existing convention; reuse the
        `--allow-empty` global plumbing from `tasks list`).
      - `markdown` — emits the original source line bullet:
        `- src.md:42 — [[Target|alias]]` for backlinks. Pipeable.
      - `json` — single array of `LinkRow` objects.
      - `ndjson` — one `LinkRow` per line.
- [x] Wiring:
      - New `ft/src/cmd/notes.rs` subcommands or extend the existing
        notes module — match whichever pattern the existing notes
        commands use.
      - Build the graph once per invocation: `Graph::build(&vault)?`.
        For two read commands this is fine; we don't yet need a
        long-lived graph cache. The TUI (future plan) will hold the
        graph in `App` state.
- [x] Integration tests under `ft/tests/notes_links.rs`:
      - `ft notes backlinks foo.md --format json` returns the expected
        `LinkRow`s sorted by `(src, src_line)`.
      - `ft notes links foo.md --format ndjson` includes both
        Resolved and Unresolved targets when the note has both kinds.
      - `--format markdown` is pipeable: parse the output back, count
        rows, assert match.
      - Empty result with `--allow-empty` exits 0; without, exits 1.
      - Unknown note → clear error, exit non-zero.
- [x] Docs: short subsections under `## Notes` in `README.md` (or
      wherever the existing notes commands are documented) for the two
      new commands. `docs/architecture.md` gains `graph/` under
      `ft-core/src/`.

### Library + CLI — rename (Session 3)

- [x] **`RenamePlan` and `apply_rename_plan`** in
      `ft-core/src/graph/rename.rs`:
      ```rust
      pub struct FileRename { pub from: PathBuf, pub to: PathBuf }

      pub struct FileEdit {
          pub path: PathBuf,
          pub byte_range: std::ops::Range<usize>,
          pub replacement: String,
      }

      pub struct FileSnapshot {
          pub path: PathBuf,
          pub mtime: std::time::SystemTime,
          pub len: u64,
      }

      pub struct RenamePlan {
          /// `None` when renaming a ghost node — no file to move,
          /// only linker rewrites.
          pub rename: Option<FileRename>,
          pub edits: Vec<FileEdit>,
          pub snapshots: Vec<FileSnapshot>,  // freshness guard
      }

      pub fn plan_rename(
          graph: &Graph,
          src: NoteId,
          new_path: &Path,         // vault-relative
      ) -> Result<RenamePlan>;

      pub fn apply_rename_plan(
          vault_root: &Path,
          plan: &RenamePlan,
      ) -> Result<()>;
      ```
- [x] **Planner** logic (`plan_rename`):
      1. Resolve `src` → `NodeKind::Note { path, title }` or
         `NodeKind::Ghost { raw }`. For Note: `rename = Some(FileRename
         { from: path, to: new_path })`. For Ghost: `rename = None`.
      2. Compute the new title from `new_path.file_stem()` (Note
         case). Ghost case: new title is `new_path.file_stem()` too —
         the linkers will be rewritten to point at `new_path`'s
         title/path even though no file is created.
      3. Walk `graph.incoming(src)`. For each incoming `LinkEdge`:
         - Read the source note's path.
         - Compute `replacement` from the link's `form`, `display`,
           `anchor`, the new target title/path:
           - `WikiLink`, no `display`, no anchor → `[[<new_title>]]`
           - `WikiLink`, with `display` → `[[<new_title>|<display>]]`
           - `WikiLink`, with `anchor` → `[[<new_title>#<anchor>]]`
             (anchor preserved verbatim — heading rename is a separate
             problem, not this plan)
           - `WikiLink`, both → `[[<new_title>#<anchor>|<display>]]`
           - `MdLink`: `[<display>](<new_relative_url>)` where
             `new_relative_url` is `new_path` made relative to the
             linker's directory and URL-encoded the same way the
             original was (preserve the original `display` text
             verbatim)
         - Wikilinks always use the new **title** (Obsidian default
           for non-path forms). If the original `raw_text` used the
           path form (`[[Subdir/Note]]`), preserve the path form with
           the new path. Decision rule: if `link.target_text` contains
           `/`, rewrite as path; otherwise rewrite as title.
         - Push a `FileEdit { path: linker_path, byte_range,
           replacement }`.
      4. Snapshot every touched file (`from`, `to`'s parent if it
         exists, and every linker) into `snapshots`.
- [x] **Applier** logic (`apply_rename_plan`):
      1. Re-stat each `FileSnapshot`; if `(mtime, len)` differs from
         the snapshot, return `Error::Notes("file changed since plan
         was made: <path> — re-plan and try again")`. Cheap and
         catches the common "user edited a file in another tool
         between plan and apply" case. (Hash-based check is more
         robust but `(mtime, len)` is what `task::ops` already uses
         and is good enough — document the limitation.)
      2. Group `edits` by `path`.
      3. For each file: validate non-overlap (sort by
         `byte_range.start`, check that each range's `end <=` next
         range's `start`); if any pair overlaps → bug, return
         `Error::Notes("planner produced overlapping edits for
         <path>")`. Apply edits **sorted by `byte_range.start`
         descending** so each edit only shifts already-processed
         bytes. Write the result via `fs::write_atomic`.
      4. Last (after all edits succeed): `std::fs::rename(from, to)`
         if `plan.rename.is_some()`. Create parent dirs of `to` if
         needed (`fs::create_dir_all`). Order matters: editing first
         keeps the planner's original-content byte ranges valid for
         the file being renamed itself (in case it links to itself,
         rare but possible), then the rename moves the now-edited
         file to its new location.
      5. Cross-file atomicity: not guaranteed (POSIX limitation,
         documented). Per-file atomicity via `write_atomic` ensures
         no half-written file. Document the partial-state-is-
         recoverable behavior under "Technical Notes."
- [x] **CLI — `ft notes rename`**:
      ```
      ft notes rename <note> <new-name-or-path> [--dry-run]
      ```
      `<note>`: same selector shapes as `links` / `backlinks`.
      `<new-name-or-path>`:
      - bare name (no `/`) → same directory, swap the file stem.
        Preserves `.md` suffix automatically; passing `.md` is
        accepted and stripped before stem-swap.
      - path with `/` → vault-relative full target path. `.md`
        suffix added if missing.
- [x] **CLI behavior**:
      - Resolve graph once (`Graph::build(&vault)?`).
      - Resolve `<note>` → `NoteId`. If not found in path or title
        index, check `ghost_index` — `ft notes rename "[[Phantom]]"
        Real.md` is a valid call (rewrite every linker; no file
        rename). Ghost selector form: leading `[[` — explicit so it
        doesn't collide with title/path lookups.
      - `plan_rename(graph, id, &new_path)`.
      - `--dry-run`: print a human-readable summary of the plan to
        stdout (number of edits, list of touched files with edit
        counts, the file rename if any) and exit 0. No writes.
      - Default: run `apply_rename_plan` and print a one-line summary
        on success (`renamed foo.md → bar.md, updated N link(s) in M
        file(s)`). Exit 0 on success, 1 on error.
      - `--json-errors` plumbed through the existing global flag.
- [x] **Integration tests** under `ft/tests/notes_rename.rs`:
      - Single linker, one wikilink: `ft notes rename foo.md bar.md`
        renames the file and updates the linker's `[[foo]]` → `[[bar]]`.
        Verify file moved; verify linker file's content updated;
        verify no stray edits to other files.
      - Multi-link in same file: a linker contains `[[foo]]` three
        times; rename updates all three; verify byte-range collisions
        don't corrupt the result (this is the descending-order
        invariant).
      - Wikilink with `display`: `[[foo|My Foo]]` → `[[bar|My Foo]]`.
      - Wikilink with `anchor`: `[[foo#H1]]` → `[[bar#H1]]`.
      - Wikilink with both: `[[foo#H1|My Foo]]` → `[[bar#H1|My Foo]]`.
      - Markdown link: `[label](foo.md)` → `[label](bar.md)`.
      - Path-form wikilink: `[[notes/foo]]` → `[[notes/bar]]` (path
        rewrite, not title rewrite).
      - Embed: `![[foo]]` → `![[bar]]`.
      - Self-link: a note that links to itself; rename updates the
        in-self link AND moves the file.
      - Ghost rename: `ft notes rename "[[Phantom]]" Real.md` rewrites
        every linker's `[[Phantom]]` → `[[Real]]`; no file
        created/renamed (`Real.md` does not appear on disk; that's
        the user's job afterward, and `ft notes create` from plan
        009 is the natural pairing).
      - `--dry-run` writes nothing and reports the plan.
      - Freshness guard: edit a touched file out-of-band between
        plan and apply (in test, simulate by running plan, sleeping,
        touching the file, running apply via two CLI invocations or
        a library-level test); applier exits 1 with the documented
        error message.
      - Rename to an existing path → exits 1 with `target already
        exists: <path>` before any writes happen (planner check).
- [x] **Docs**:
      - `README.md` gains a Notes-rename example under the existing
        Notes section (or a new `## Notes graph` section if cleaner).
      - `docs/architecture.md` documents `ft-core/src/graph/` (parser,
        resolve, rename), the plan-vs-apply pattern, and the
        descending-byte-order invariant for safe in-file edits.

## Technical Notes

- **Why node=document, not node=link-location.** A link is intrinsically
  a relationship, not a thing. Modeling links as nodes means every
  rename has to track node identity changes for ephemeral artifacts
  (link sites) that the user never thinks of as "things." Modeling
  links as **rich edges** with location attributes (byte_range,
  raw_text, anchor, display, form) gives us per-occurrence data
  without inflating the node space, and makes rename a graph mutation
  on note nodes that derives edit lists from the edges hanging off
  them. Multi-edges (same source linking same target N times) fall out
  for free.

- **Why ghost nodes for unresolved targets, not just an enum on the
  edge.** Two reasons. (1) Uniform traversal: `incoming(ghost_id)`
  works exactly like `incoming(note_id)`, so backlinks queries don't
  need a special case. (2) The "rename a not-yet-created note"
  feature: `ft notes rename "[[Phantom]] Real.md"` rewrites every
  linker through the same `plan_rename` code path the resolved case
  uses, with `rename = None` to skip the file move. Sharing a single
  ghost across multiple linkers (via `ghost_index: HashMap<String,
  NoteId>`) keeps the graph compact and makes "all linkers to this
  unresolved name" a one-hop query.

- **Why descending byte order for in-file edits.** The standard LSP
  refactoring trick: each `replace_range(byte_range, replacement)`
  shifts every byte to its right by `replacement.len() -
  byte_range.len()`. Processing edits sorted by `byte_range.start`
  descending means the only bytes that move are ones we've already
  processed, so earlier edits' byte ranges stay valid against the
  partially-mutated content. The invariant is that no two edits
  overlap; for our case they can't (each link is a distinct
  contiguous span in the source), but we still validate non-overlap
  in `apply_rename_plan` and fail loudly on a planner bug rather
  than silently corrupt the file.

- **Why `(mtime, len)` for the freshness check, not a content hash.**
  Symmetry with `task::ops`, which uses the same shape of guard. The
  failure mode is "user edited the file in another tool between plan
  and apply" — `(mtime, len)` catches that in every realistic case.
  A pathological "edit that exactly preserves length and mtime
  resolution" case can fool it; documented as a known gap. Hash-based
  freshness is a small follow-up if it ever bites anyone.

- **Why edit-then-rename ordering.** A note that links to itself is
  rare but possible. If we rename first and then edit, the planner's
  byte ranges (computed against the file at its old path) need to
  follow the file to its new location — workable but fragile. Editing
  the file at its old path keeps byte ranges valid by construction,
  then a single `std::fs::rename` moves the now-correct file. Same
  reasoning for "linker file is the same file we're renaming": the
  edit and the rename target the same path; do the edit first.

- **Why no cross-file atomicity guarantee.** POSIX has no transactional
  multi-file write, and we don't want to invent one (FUSE overlays,
  WAL files, etc. are all way too much complexity for a vault tool).
  Per-file atomicity via `write_atomic` ensures no half-written file;
  if we crash between files, partial state is *recoverable* (some
  linkers updated, others not — re-running `ft notes rename` is
  idempotent because the second run finds no remaining old-name
  links to update) rather than *lost* (which is what in-place edits
  would risk). Same constraint already documented for
  `notes::write_pair` in plan 003.

- **Why `StableGraph`, not `Graph`, from petgraph.** `Graph` reshuffles
  `NodeIndex` values when nodes are removed; `StableGraph` doesn't.
  `refresh_note` removes outgoing edges (and possibly orphaned
  ghosts) routinely, and we want the `path_index` / `title_index`
  side-tables to keep pointing at the right nodes without rebuild.
  The cost is a small per-node memory overhead; for a 5k-note vault,
  negligible.

- **Why share the markdown scanner state with `markdown.rs`.**
  `markdown.rs::ScanState` already has the right logic for skipping
  frontmatter, fenced code, and indented code. The link parser needs
  the same skipping plus inline-code-span skipping. Factor the shared
  bits into `markdown::scan::Scanner` (same module — one file edit)
  and have both `extract_headings` and `extract_links` consume it.
  Avoids duplicate state machines that drift apart over time.

- **Why no TUI integration in this plan.** The TUI work (backlinks
  pane, link navigation, "rename this note" affordance) is a
  meaningful surface unto itself and benefits from the library being
  fully shipped first. A future plan layers `App::graph: Graph` with
  appropriate refresh hooks on file writes from within the TUI, plus
  the UI affordances. Keeping this plan to library + CLI keeps the
  acceptance surface tight and reviewable.

- **Why the read commands ship before rename.** They're cheap on top
  of the library, independently useful, and exercise the parser /
  resolver / ghost logic against real vaults before the mutation
  path builds on the same primitives. Bug-for-the-buck this is the
  best ordering.

- **Case-sensitivity.** v1 documents "case-sensitive matching, follows
  Obsidian's vault-relative-path resolution." macOS APFS and Windows
  NTFS are case-insensitive by default, which means a `[[Foo]]` link
  to `foo.md` *would* resolve in Obsidian on those filesystems but
  *won't* resolve in our title/path lookup. Documented as a known
  gap; case-insensitive fallback is a follow-up plan when someone
  hits it.

- **Reference-style markdown links** (`[text][ref]` + `[ref]: url`)
  are out of scope for v1 — uncommon in Obsidian vaults, and parsing
  them correctly requires a full reference-definition pass. Out-of-
  scope, documented gap.

## Future (explicitly out of scope for this plan)

- **TUI integration.** Backlinks pane on the Notes tab, "rename this
  note" chord, link navigation with `gd` / `Ctrl+]`. Own plan.
- **`plan_merge` and `plan_split`.** Merge two notes into one (concat
  + redirect linkers); split a note's section into a new note (move
  section via `notes::move_sections` + redirect linkers that pointed
  at the moved section's anchor). Same plan-vs-apply shape as rename.
- **Folder / Task / Tag / Frontmatter nodes.** Heterogeneous
  `NodeKind` is already in the type — adding new variants is
  additive. Each new kind ships its own plan with its own resolver.
- **Heading-rename propagation.** When a heading text changes,
  rewrite every `[[Note#OldHeading]]` → `[[Note#NewHeading]]`. Needs
  a heading-anchor resolver layered on top of the link graph.
- **Disk caching.** `(path, mtime, hash)`-keyed cache of parsed
  outgoing-edge sets per file. Opt-in `[graph] cache = true` config.
  Worth shipping when someone proves startup time is the bottleneck.
- **"Find related" / similarity queries.** Graph-walk-based
  suggestions for "notes related to this one" — same-cluster, common
  neighbors, etc. Pure read-side feature on top of the library.
- **Case-insensitive link resolution** for case-insensitive
  filesystems.
- **Reference-style markdown links** (`[text][ref]` + `[ref]: url`).
- **Rename-on-ghost-create.** `ft notes create Real.md` could
  optionally check the ghost index and offer "you have N linkers to
  `[[Phantom]]` — rename them to `[[Real]]` as part of creation?"
  Composes naturally with this plan's ghost rename, but is its own
  UX surface.

## Sessions

### Session 1 · 2026-05-15 · done
**Goal:** Library foundation. New `ft-core/src/graph/` module
(`mod.rs`, `parser.rs`, `resolve.rs`, `tests.rs`); shared markdown
scanner factored out of `markdown.rs`. Node and edge types
(`NodeKind::Note | Ghost`, `EdgeKind::Link | Embed`, `LinkEdge`,
`LinkTarget::Resolved | Unresolved`, `LinkForm::WikiLink | MdLink`).
Wikilink + markdown-link + embed parser respecting frontmatter,
fenced/indented/inline code. Resolver implementing Obsidian's
shortest-path tiebreak. Ghost-node materialization with shared
`ghost_index`. `Graph::{build, refresh_note, note_by_path,
note_by_title, ghost_by_raw, node, nodes, outgoing, incoming}`.
Petgraph as a new dep (`StableGraph`). Fixture additions for the
full link matrix (titles, paths, anchors, displays, embeds, code
skipping, ghosts, collisions). Full unit-test coverage for build +
refresh + ghost cleanup. `cargo test --workspace` + clippy + fmt
clean.
**Outcome:** New `ft-core/src/graph/` module — `mod.rs` (~340 lines),
`parser.rs` (~640 lines incl. tests), `resolve.rs` (~270 lines incl.
tests), `tests.rs` (~270 lines). `pub mod graph;` added to `lib.rs`.
Public surface matches the plan: `Graph::{build, refresh_note,
note_by_path, note_by_title, ghost_by_raw, node, nodes, outgoing,
incoming}`, plus `NoteId`, `NodeKind::{Note, Ghost}`,
`EdgeKind::{Link, Embed}`, `LinkEdge`, `LinkTarget::{Resolved,
Unresolved}`, `LinkForm::{WikiLink, MdLink}`, `NoteData`, `GhostData`.

`Graph` is backed by `petgraph::stable_graph::StableDiGraph<NodeKind,
EdgeKind>` with three side-tables: `path_index: HashMap<PathBuf,
NoteId>`, `title_index: HashMap<String, Vec<NoteId>>`, and
`ghost_index: HashMap<String, NoteId>` (shared across linkers so
removing one linker keeps the ghost as long as any other points at
the same raw target).

Build pipeline: `Vault::markdown_files()` → parallel rayon parse
phase yielding `(rel_path, content, Vec<RawLink>)` → main-thread
node insertion (so side-tables stay consistent) → main-thread edge
resolution + insertion. Resolution failure materializes a ghost via
the shared `intern_ghost` helper. `refresh_note` canonicalizes both
the vault root and the absolute path (handles macOS `/tmp` →
`/private/tmp` symlink), strips to vault-relative, removes outgoing
edges + cleans orphaned ghosts, re-parses, and re-inserts.

`extract_links` (parser): single-pass per file with a per-line outer
loop that consults the shared `markdown::LineSkipState` to skip
frontmatter / fenced / indented code blocks, then a byte-level inner
loop on each content line that handles inline code spans
(single/double/triple backticks) and matches `[[...]]`, `![[...]]`,
`[text](href)`, `![text](href)`. Each `RawLink` carries a file-byte
range that round-trips against the source content. External URLs
(`http`, `https`, `mailto`, `obsidian`, `ftp`, `ssh`, `file`)
filtered at the parser level. Reference-style links explicitly
deferred. Angle-bracket form `[F](<foo bar.md>)` handled (URL with
spaces).

Resolver: wikilink path-form (target contains `/`) → `path_index`
lookup with `.md` fallback. Wikilink title-form → `title_index`
lookup; multi-candidate tiebreak picks fewest path components,
breaking ties alphabetically by full vault-relative path (Obsidian
default). Markdown link href interpreted relative to linker's
directory, URL-decoded, normalized via a `..` / `.` collapse, then
path-index lookup with `.md` fallback. Unresolved keys: verbatim
target text for wikilinks, normalized vault-relative path string for
markdown links — so two linkers writing `../foo.md` from sibling
files share one ghost.

Shared scanner refactor: extracted `markdown::LineSkipState` (was
private `ScanState`) with `new()` and `skip_line(&str) -> bool`.
`extract_headings` rewritten to consume it; `leading_fence` exposed
`pub(crate)` so the link parser can use it. The 10 existing markdown
tests stay green — refactor is behavior-preserving.

Dedicated fixture vault at `tests/fixtures/links/` (separate from
the realistic / pathological vaults so existing task tests are
untouched): 9 markdown files exercising plain wikilinks, alias,
anchor, anchor+alias, path-form, ghost target, repeated target,
URL-encoded md link, extension-less md link, embed forms, frontmatter
skipping, fenced/indented/inline code skipping, external URL
filtering, and the title-collision tiebreak (top-level `Index.md`
vs `archive/Index.md`).

Test counts: parser_tests +25, resolve_tests +8, graph::tests +13 →
+45 graph tests. Workspace test total: 852 (was 807). `cargo clippy
--workspace --all-targets -- -D warnings` clean. `cargo fmt --check`
clean after one autoformat pass.

New dependency: `petgraph = "0.6"` with `default-features = false`
and `features = ["stable_graph"]` only — minimizes the transitive
surface (no `serde_derive_state` etc.). `fixedbitset 0.4.2` pulled
in transitively. No other workspace deps changed.

Docs: `docs/architecture.md` ft-core file-tree block grew the
`graph/` entry (mod, parser, resolve) and a `markdown.rs` row
describing its dual role (heading extractor + shared scanner).

### Session 2 · 2026-05-15 · done
**Goal:** Read-side CLI. `ft notes backlinks <note>` and `ft notes
links <note>` subcommands, both selector-resolved (path / title /
fuzzy). Uniform `LinkRow` shape over `table` / `json` / `ndjson` /
`markdown` formatters in a new `ft/src/output/links.rs`. Graph
built once per invocation. `--allow-empty` plumbed via the existing
global. Integration tests under `ft/tests/notes_links.rs` covering
each format, both commands, both Resolved and Unresolved targets,
empty-result behavior. Doc updates in `README.md` and
`docs/architecture.md`.
**Outcome:** New `ft/src/output/links.rs` (~210 lines): `LinkRow`
with `serde::Serialize` derive (flat shape so the JSON / NDJSON wire
format is stable for scripting), `LinkRowTarget::{Resolved, Unresolved}`
serialized via `#[serde(tag = "kind", rename_all = "lowercase")]` so
consumers branch on `dst.kind`. `Direction::{Backlinks, Forward}`
selects the table layout (`Src/Line/Form/Display/Raw` vs
`Dst/Line/Form/Display/Raw`); unresolved targets render in the table
as `? <raw>`; embed edges show `wiki!` / `md!` in the Form column.
`LinkRow::{from_outgoing, from_incoming}` are the only constructors —
both take `&Graph` and dereference the destination node to discriminate
Note vs Ghost.

`render_table` uses comfy-table's `UTF8_FULL` preset with dynamic
content arrangement (matches `output::table` for tasks). `render_json`
emits a pretty array; `render_ndjson` emits one row per line;
`render_markdown` emits `- src.md:LINE — RAW` bullets that are
pipeable into another note (the verbatim `raw_text` token preserves
the original wikilink/md-link form).

Two new variants on `NotesCommand` in `ft/src/cmd/notes.rs`:
`Backlinks(LinksArgs)` and `Links(LinksArgs)` — sharing one args type
since they take the same flags. `LinksArgs { note: Vec<String>,
format, no_color, allow_empty }`. Both dispatch to a single
`run_links(args, vault_flag, dir)` helper that builds the graph
once, resolves the note query, and maps `incoming`/`outgoing` to
`LinkRow`s.

Note resolution (`resolve_note_query`) tries three things in order:
exact vault-relative path (with `.md` auto-appended), title (filename
stem) with shortest-path/alphabetical tiebreak, and `fuzzy_find` top
hit. Matches the ergonomics of `ft notes open` while preferring the
unambiguous path/title path when those work. Ghost selection from
the CLI is deferred to session 3 — the CLI here only resolves real
notes (the planner in session 3 will accept `[[Phantom]]` syntax for
ghost rename).

Empty-result handling mirrors `ft tasks list`: exit 1 by default with
a "no backlinks" / "no outgoing links" stdout line; `--allow-empty`
flips to exit 0. Stable result ordering: backlinks sort by
`(src_path, src_line)`, forward links sort by `(src_line, raw)`.

Added `serde = { workspace = true }` to `ft/Cargo.toml` (was already
in workspace deps but the binary crate hadn't pulled it in directly
— needed for `#[derive(Serialize)]` on `LinkRow`). No new workspace
deps.

11 new integration tests in `ft/tests/notes_links.rs`, all green:
- `backlinks_alpha_returns_three_rows_from_hub` — JSON parse +
  every row's `dst.kind == "resolved"` + path equality.
- `backlinks_with_no_incoming_edges_exits_one_by_default` and
  `…_and_allow_empty_exits_zero` — exit-code matrix on the empty
  case.
- `backlinks_unknown_note_errors` — resolution failure surfaces
  the documented "no note found" message on stderr.
- `backlinks_markdown_format_is_pipeable` — markdown bullet count
  matches the expected edge count (2 from hub: wiki-with-alias +
  md link).
- `links_hub_includes_resolved_and_unresolved_targets` — NDJSON
  parse + counts of `dst.kind` for both branches + Phantom
  presence by name.
- `links_table_format_shows_question_mark_for_unresolved` — the
  `? Phantom` rendering is in the default-format output.
- `links_with_no_outgoing_exits_one_by_default` — beta.md (no
  outgoing edges in the fixture) exits 1.
- `links_path_form_query_resolves_directly` — passing the full
  vault-relative path bypasses the title/fuzzy branches.
- `links_records_anchor_and_display_on_appropriate_rows` — finds
  `[[gamma#Heading One]]` and `[[gamma#Heading One|G1]]` rows and
  asserts on the parsed `anchor` and `display` fields.
- `links_marks_embeds_with_form_suffix_in_table` — `wiki!` marker
  in the default-format output for `![[alpha]]`.

Workspace state: `cargo test --workspace` → 863 tests green (was
852: +11 from the new integration suite). `cargo clippy --workspace
--all-targets -- -D warnings` clean. `cargo fmt --check` clean
after one autoformat pass.

Docs: `README.md` gains a `## Note links` section between Tasks and
Git sync with three example invocations and a one-paragraph
explanation of the resolution rules; `docs/architecture.md` adds
`links.rs` to the `ft/src/output/` row.

### Session 3 · 2026-05-15 · done
**Goal:** Rename. New `ft-core/src/graph/rename.rs` with `FileRename`,
`FileEdit`, `FileSnapshot`, `RenamePlan`, `plan_rename`,
`apply_rename_plan`. Planner walks `incoming(src)`, derives the
right rewrite per `LinkForm` × `display` × `anchor` × title-vs-path
target form. Applier validates non-overlap, applies same-file edits
in **descending byte order**, freshness-checks via `(mtime, len)`,
edit-then-rename ordering, per-file atomicity via `fs::write_atomic`.
Ghost-rename support (no file move, only rewrites). CLI subcommand
`ft notes rename <note> <new-name-or-path> [--dry-run]` with
`mv`-style ergonomics. Integration tests for: single + multi link,
all wikilink shape combinations (display, anchor, both, path-form),
markdown links, embeds, self-link, ghost rename, dry-run, freshness
guard, target-already-exists. Doc updates.
**Outcome:** New `ft-core/src/graph/rename.rs` (~520 lines incl. 21
unit tests). Public surface matches the plan: `FileRename { from, to }`,
`FileEdit { path, byte_range, replacement }`, `FileSnapshot { path,
mtime, len }`, `RenamePlan { rename: Option<FileRename>, edits, snapshots }`,
`plan_rename(graph, vault_root, src, new_path) -> Result<RenamePlan>`,
`apply_rename_plan(vault_root, plan) -> Result<()>`. Plus a
`RenamePlan::touched_files()` helper for CLI summary output.

Planner walks `graph.incoming(src)` and for each edge calls
`build_replacement` which switches on `LinkForm × is_embed × display
× anchor × target_form`:
- `WikiLink` with `target_text.contains('/')` → keep path form;
  preserve `.md` suffix iff the original had one. Otherwise use the
  bare new title (filename stem of `new_path`).
- `WikiLink` body assembly: `<target>[#anchor][|display]`, then
  `[[…]]`, prefixed with `!` for embeds.
- `MdLink` → compute href via `relative_url_from(linker_path,
  new_path)` (common-prefix path arithmetic + URL-encoding of each
  `Component::Normal`); strip `.md` when the original was extension-
  less; preserve `#anchor` and `display` text; prefix `!` for embeds.

`relative_url_from` is a small inline path-relativization helper —
finds the common prefix between `linker.parent()` and `target_rel`,
emits `..` for each remaining linker-side component, then descends
into the target with each component URL-encoded via
`urlencoding::encode`. No new dep. Verified by the
`rename_md_link_relative_to_linker_dir` test (`notes/from.md` →
`../foo.md`, rename to `baz.md`, expect `../baz.md`) and
`rename_md_link_to_path_with_spaces_url_encodes_in_href` (`My Note.md`
→ `My%20Note.md`).

Pre-checks in the planner:
- New path's filename stem must be non-empty (rejects `.md` and
  similar).
- Target-already-exists → `Error::Notes("target already exists: …
  refusing to overwrite")` before snapshotting or any I/O.
- Same-path no-op short-circuits to `RenamePlan { rename: None,
  edits: [], snapshots: [src] }` so users can sanity-check without
  any writes.

Snapshots: one `FileSnapshot` per touched linker plus the source file
(if real — ghost renames skip the source snapshot). `(mtime, len)`
captured at plan time via `std::fs::metadata`.

Applier order:
1. Re-stat each snapshot; mismatch → `Error::Notes("file changed
   since plan was made: <path> — re-plan and try again")`. The
   `rename_freshness_guard_trips_…` test verifies the source file
   stays in place when the guard fires (no partial writes).
2. Group edits by path, sort each group descending by
   `byte_range.start`, validate non-overlap (loud planner-bug
   error), apply via `String::replace_range`, write via
   `fs::write_atomic`.
3. `std::fs::rename(from, to)` last (creating parent dirs via
   `fs::create_dir_all` first). The self-link test
   (`rename_self_link_edits_then_renames`) confirms editing the
   source file at its old path before the rename works correctly:
   `foo.md` containing `[[foo]]` becomes `bar.md` containing
   `[[bar]]`.

CLI: new `RenameArgs { note, new, dry_run }` and `Rename(RenameArgs)`
variant on `NotesCommand`. `run_rename`:
- Resolves `<note>` via `resolve_rename_source` — same path/title/
  fuzzy logic as the read-side commands, plus a leading `[[`
  trigger that selects a ghost via `graph.ghost_by_raw`. The
  unambiguous brackets prevent collisions with title or path
  lookups.
- Translates `<new>` via `parse_new_path`: `mv` ergonomics — bare
  name (no `/`) inherits the source note's directory; path with
  `/` is vault-relative; `.md` always auto-appended when missing.
  Ghost source + bare name → vault root.
- Calls `plan_rename` then either prints a plan summary
  (`--dry-run`) or runs `apply_rename_plan` and prints a one-line
  success (`renamed foo.md → bar.md, updated N link(s) in M
  file(s)`). Ghost renames print
  `rewrote N ghost link(s) in M file(s) — pass `ft notes create
  <new>` to create the new file`.

15 new integration tests in `ft/tests/notes_rename.rs`, all green:
- `rename_simple_wikilink_renames_file_and_updates_linker`
- `rename_multi_link_in_one_file_handles_descending_order` (the
  byte-shift safety test — three `[[foo]]` → three
  `[[a-much-longer-name]]` in one file)
- `rename_preserves_alias_anchor_and_both` (all three wikilink
  shape combinations in one shot)
- `rename_path_form_wikilink_keeps_path_form`
- `rename_md_link_updates_url`
- `rename_embed_keeps_bang_prefix`
- `rename_self_link_edits_then_renames`
- `rename_bare_name_keeps_source_directory` (mv ergonomics:
  `notes/foo.md` + bare `bar` → `notes/bar.md`)
- `rename_full_path_moves_file_across_directories` (linker text
  unchanged when the title doesn't change but the path does)
- `rename_appends_md_extension_automatically`
- `rename_ghost_rewrites_linkers_without_creating_a_file`
- `rename_unknown_ghost_errors`
- `rename_dry_run_writes_nothing_and_prints_plan`
- `rename_to_existing_path_errors_before_any_writes`
- `rename_unknown_note_errors`

21 library unit tests in `graph::rename::rename_tests` covering the
same matrix at the library boundary, plus the freshness-guard test
that exercises the `(mtime, len)` re-stat path with an out-of-band
edit between plan and apply.

Workspace state: `cargo test --workspace` → 899 tests green (was
863: +21 library, +15 integration; total +36). `cargo clippy
--workspace --all-targets -- -D warnings` clean. `cargo fmt
--check` clean after one autoformat pass. No new dependencies.

Docs: `README.md` gained a paragraph + four examples under the
existing `## Note links` section explaining `ft notes rename`,
including the `mv` ergonomics, the `[[Phantom]]` ghost-rename
syntax, and the `--dry-run` flag, plus a paragraph noting the
freshness guard and descending-byte-order edit application.
`docs/architecture.md` adds `rename.rs` to the `ft-core/src/graph/`
file-tree block.

The plan is now complete — `ft notes rename` works on the CLI with
`--dry-run` and ghost-rename support; `plan_rename` /
`apply_rename_plan` are usable from the library directly (a future
TUI plan can wire them into a "rename this note" chord); the
graph + rename together close the wikilink-rewrite TODO that was
sitting in `task::ops:640` since plan 001.
