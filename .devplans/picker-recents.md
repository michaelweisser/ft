---
id: 008
name: picker-recents
title: Fuzzy picker: show recent notes on empty input
status: finished
created: 2026-05-13
updated: 2026-05-13
---

# Fuzzy picker: show recent notes on empty input

## Goal
Show up to **25 recent notes** in every `FuzzyPicker<VaultFilePickerSource>`
*before* the user has typed anything. Recency is the union of two signals:
(a) files **opened from `ft`** (TUI `o`, both move-section pickers, the
tasks-tab target picker, and the `ft notes open` CLI), and (b) files
**recently modified on disk** (covering edits made in Obsidian or any
other tool). Opens win over mtime when both are present, and mtime fills
the tail so a freshly-created note shows up even if `ft` has never
touched it.

Once the user types a single character, behavior is identical to today —
this is strictly an empty-input enhancement.

## Motivation and Context
The picker is the spine of three Notes flows (`o`, move-section source,
move-section target) plus the new-task target field. In a real vault
with hundreds of notes the most common interaction pattern is
"I just touched this note, open it again" — and today the user still
has to fuzzy-match. A pre-populated recent list reduces that to a single
`Enter` press.

We want both signals because each one covers a gap:
- **Opens** capture intent — "I work on these notes a lot" — but miss
  notes the user created in Obsidian and never opened via `ft`.
- **Mtime** captures everything written to disk, including external
  edits and brand-new files, but treats a one-off auto-save the same as
  a note the user is actively working on.

Merging them (opens first, mtime fills the tail) is the cheapest way to
get the "show me the note I just made or touched" behavior the user
asked for.

The trait surface (`PickerSource`) already abstracts "what does this
source produce"; extending it with an explicit `initial_items` hook
keeps the contract honest and avoids overloading `query("")`. Render
gating moves from "empty input ⇒ hint" to "empty input ⇒ initial items
(falling back to hint only if there are none)".

## Acceptance Criteria

### Core library — `ft_core::recents`

- [x] New module `ft_core::recents` with a `RecentsLog` type:
      - Backed by a JSONL file at
        `$XDG_STATE_HOME/ft/<vault-hash>/recents.jsonl`
        (falling back to `~/.local/state/ft/<vault-hash>/` when
        `XDG_STATE_HOME` is unset; macOS follows the same XDG fallback
        for consistency with how the project already handles paths).
        `<vault-hash>` is a short stable digest of the vault's canonical
        path (e.g. 16-char blake3 / sha256 hex prefix) so multiple
        vaults stay isolated.
      - Each line is `{"path": "<vault-relative path>", "opened_at":
        "<RFC3339 UTC>"}`. Vault-relative — not absolute — so the log
        survives a vault directory rename.
      - `RecentsLog::record_open(&self, path: &Path)` appends one line,
        creating parent dirs as needed. Best-effort: a write error logs
        a warning and is **never** propagated to the caller (recents
        must never break an open).
      - `RecentsLog::load_recent(&self, limit: usize) -> Vec<PathBuf>`
        reads the tail, dedupes by path keeping most-recent-wins,
        returns vault-relative paths in recency order (newest first).
        Trims the file in-place when it grows beyond a cap (default
        **200 entries** post-dedupe) to keep reads bounded.
      - Both methods take `&self` and are internally I/O — no in-memory
        cache that could drift across processes (the TUI and the CLI
        share the same file).
      - Public constructor `RecentsLog::for_vault(&Vault) -> Self`
        resolves the per-vault directory and creates it lazily on
        first write.
- [x] Unit tests in `ft_core::recents`:
      - `record_open` then `load_recent` returns the path.
      - Two `record_open` calls for the same path return one entry,
        newest timestamp.
      - Records survive dedupe ordering across N writes.
      - Trim kicks in past the cap and preserves the newest N.
      - Missing log file yields empty `load_recent` (no error).
      - Malformed lines are skipped (forward-compat).
      - `record_open` on a read-only directory logs and returns without
        panicking.

### Core library — `ft_core::vault`

- [x] `Vault::markdown_files_with_mtime() -> Vec<(PathBuf, SystemTime)>`
      (new) walks `.md` files and attaches each file's mtime via
      `std::fs::metadata`. The existing `markdown_files()` keeps its
      current shape — `_with_mtime` is a new fn so nothing else has to
      change.
- [x] Errors from individual `metadata()` calls are demoted to
      `SystemTime::UNIX_EPOCH` (so the file is included but ranks last
      by recency) rather than dropped.

### Core library — `ft_core::search` (recents merge)

- [x] New free fn
      `recent_hits(vault: &Vault, recents: &RecentsLog, limit: usize)
      -> Vec<Hit>` that:
      - Loads opens from `recents.load_recent(limit * 2)` (oversample
        so the dedupe leaves room).
      - Loads `vault.markdown_files_with_mtime()`.
      - Filters opens against the current file set (drops paths whose
        files no longer exist).
      - Composes the final list as: **opens first, in recency order**,
        then **mtime-ordered files** not already in the opens slice,
        until `limit` entries are reached.
      - Returns `Hit` values with `path` set to the **vault-relative**
        path (consistent with `fuzzy_find` — the plan originally said
        "absolute", but the rest of `Hit` is vault-relative, so keeping
        that convention avoids a special case at every render site),
        `heading: None`, all scores `0`. Picker renders these verbatim
        with no match highlighting.
- [x] Unit tests for `recent_hits`:
      - Opens-only path (mtime tail is empty).
      - Mtime-only path (no recents log).
      - Merge keeps opens first; mtime fills the tail.
      - Deleted files in the opens log are dropped.
      - Cap honored (length ≤ limit).
      - Deduped: a file in both opens and mtime appears once, in its
        opens position.

### TUI — picker widget (`tui::widgets::picker`)

- [x] `PickerSource` trait grows
      `fn initial_items(&mut self, limit: usize) -> Vec<PickerItem<Self::Item>> { Vec::new() }`
      — default empty for back-compat. Existing test sources don't
      have to opt in.
- [x] `FuzzyPicker::refresh` calls `source.initial_items(limit)` when
      `input.text.is_empty()`, otherwise stays on the current
      `source.query(...)` path. Cache key flips between "empty" and
      the last non-empty query so we don't recompute initial items
      every keystroke when the user clears the field. (`FuzzyPicker::new`
      also calls `refresh` at construction so the first render is warm
      without a separate `prime` call.)
- [x] `FuzzyPicker::render_list` empty-state logic:
      - Items present (recents) → render them with no match
        highlighting; first row is selected by default; list block
        title flips to `" recent · type to search "`.
      - Items empty (cold-start vault, no recents) → keep the existing
        `type to search…` hint.

### TUI — `VaultFilePickerSource`

- [x] Constructor takes the `RecentsLog` alongside `Arc<Vault>`:
      `VaultFilePickerSource::new(vault: Arc<Vault>, recents:
      Arc<RecentsLog>)`. `Arc` on the log so the four picker sites
      share one instance per `App`.
- [x] `initial_items` impl calls `recent_hits(&self.vault,
      &self.recents, limit.min(25))` and maps each `Hit` to a
      `PickerItem` with vault-relative display path and empty
      match-indices.
- [x] `query` is unchanged. Empty-string behavior at the source level
      still returns `Vec::new()` — the widget routes empties through
      `initial_items` now.

### TUI — call sites and the open chokepoint

- [x] `App` owns `recents: Arc<RecentsLog>` constructed in
      `App::new` via `RecentsLog::for_vault(&vault)`. Test helpers
      (`for_test`, `for_test_with_clock`) route the log under
      `vault.path/.ft-state/` so test runs never write to the user's
      real `$XDG_STATE_HOME`. New `for_test_with_recents` /
      `for_test_with_clock_and_recents` helpers let tests inject a
      pre-seeded log they hold an `Arc` to.
- [x] `TabCtx` exposes `recents: &'a Arc<RecentsLog>` so tabs can
      clone the `Arc` for picker construction.
- [x] `new_vault_picker(ctx)` in `tabs/notes/mod.rs` clones
      `ctx.vault` and `ctx.recents` into the source.
- [x] `tabs/tasks/search.rs` target picker constructed the same way.
- [x] Recording done at the **caller** sites rather than
      `App::handle_request` (the plan permits this for `OpenInObsidian`
      to avoid URL-parsing the path back out; for symmetry both
      variants record at the construction site).
      `request_open_in_editor` and `request_open_in_obsidian` in
      `tabs/notes/mod.rs` call `ctx.recents.record_open(&hit.path)`
      before raising the `AppRequest`. Best-effort — log failures
      don't propagate.
- [x] CLI `ft notes open` in `cmd/notes.rs` records the open via
      `RecentsLog::for_vault(&vault).record_open(&hit.path)`.
- [x] Help-overlay copy unchanged.

### Testing

- [x] Unit tests as listed under each library section (session 1).
- [x] `tui::tests` recents coverage:
      - `notes_open_picker_shows_logged_open_first` — opened file
        beats mtime-newer files.
      - `notes_open_picker_empty_log_falls_back_to_mtime` — newest
        mtime first when no opens.
      - `notes_open_picker_cold_start_shows_type_to_search_hint` —
        zero `.md` files keeps the legacy hint.
      - `notes_open_picker_typing_transitions_from_recents_to_results`
        — title flips on first keystroke.
      - `notes_open_picker_backspace_returns_to_recents` — clearing
        the input restores the recents view.
      - `notes_open_picker_enter_on_recent_records_and_reopens_at_top`
        — end-to-end: open a file, re-open the picker, the just-opened
        file leads.
      - `cli_record_open_through_recents_log` — `RecentsLog::for_vault`
        with `XDG_STATE_HOME` override round-trips a recorded open.
- [x] Snapshot test: `notes_open_picker_recents_80x24.snap` —
      empty-input picker with one opened note + two mtime-tail files.

## Technical Notes

- **Why opens-first beats a weighted score.** A weighted blend
  (`opens * w1 + mtime_rank * w2`) sounds tidy but produces surprising
  ordering — a note edited five minutes ago in Obsidian could
  out-rank a note the user just opened twice in `ft`. The user wants
  "the note I just touched is on top," and the simplest reading of
  "touched" is: `ft`-open trumps disk-modify, because opens are
  intent and writes can be background noise (auto-save, sync). Opens
  first → mtime tail keeps the rule legible.
- **Vault-relative paths in the log.** Storing absolute paths bakes in
  `~` and machine-specific prefixes; a vault sync (e.g. iCloud,
  Syncthing) across machines would break recents. Vault-relative is
  the right unit of address.
- **`<vault-hash>` for state dir isolation.** Two vaults on the same
  machine must not see each other's recents. Hashing the canonical
  vault path is cheaper and more deterministic than e.g. a slugified
  name (which collides on `Notes` vs `Notes (copy)`). Use a short
  prefix of a fast hash (blake3 if it's already in the tree;
  otherwise sha256). Verify against `Cargo.toml` before settling.
- **Cap at 200 entries.** Bounded read latency, bounded disk usage,
  comfortably above the 25-item display limit so the dedupe window
  is wide enough that infrequent paths don't get evicted by churn.
- **Trim on write, not on read.** Trimming on every read costs us
  rewrites for read-only workloads. Trim opportunistically inside
  `record_open` whenever the file exceeds the cap by more than a
  small slack (e.g. 50 extra lines) — at most one rewrite per ~50
  opens.
- **No mtime sorting cache.** The mtime walk is fast enough on
  realistic vaults (sub-millisecond per file for `metadata()`); the
  picker only calls it on empty input, and the result is cached
  inside `FuzzyPicker` for the duration of an empty-input session.
  Premature caching here would only add invalidation bugs.
- **No heading rows in recents.** Recents are file-level only. Once
  the user types, the existing heading-extraction path runs as
  before. Keeps the empty-state list dense and the implementation
  small.
- **Crash-safety.** The log is append-only JSONL with one record per
  line. A torn write at most loses the last record; malformed lines
  are skipped on read.
- **Threading.** `RecentsLog` is `Send + Sync`; methods take `&self`
  and synchronize internally via file locking (advisory `fs2::flock`
  or a `Mutex<()>` if a deps cost is unacceptable — implementation
  choice, no public surface impact).
- **Test isolation.** Tests must point `RecentsLog` at a `TempDir`,
  not `~/.local/state`. The `App::for_test*` helpers grow a
  `recents_dir` parameter (defaulted to a temp path inside the
  helper).

## Future (explicitly out of scope for this plan)

- Per-file open *counts* (a note opened 50 times outranking a note
  opened once 30 seconds ago). v1 is recency-ordered.
- Pinned/starred notes shown above recents.
- A `ft notes recents` CLI to list the log.
- Configurable recents limit (hardcoded at 25 for v1).
- Showing recent **headings** (the recently-jumped-to anchors). v1 is
  files only.
- Cross-vault recents (e.g. one log per machine, not per vault).

## Sessions
### Session 1 · 2026-05-13 · done
**Goal:** Core library: RecentsLog + markdown_files_with_mtime + recent_hits merge (behavior-neutral, no callers wired)
**Outcome:** Shipped. New `ft_core::recents::RecentsLog` (append-only JSONL,
vault-hash-isolated dir under `$XDG_STATE_HOME/ft/<hash>/recents.jsonl` with
`~/.local/state` fallback, `with_log_path` ctor for test injection), 12 unit
tests covering record/load/dedupe/ordering/limit/missing/malformed/trim/
unwritable-dir/abs-vs-rel paths/hash determinism. `Vault::markdown_files_with_mtime`
walks `.md` files with mtime, falling back to `UNIX_EPOCH` on metadata
errors. `ft_core::search::recent_hits(&Vault, &RecentsLog, limit)` merges
opens-first + mtime-tail with dedupe, filters deleted paths, honors limit;
7 unit tests cover all merge cases. Sync via POSIX `O_APPEND` atomicity
(no fs2 dep). `Hit.path` kept vault-relative (consistent with `fuzzy_find`;
plan updated). Full workspace: 14/14 test binaries green, clippy clean,
fmt clean.

### Session 2 · 2026-05-13 · done
**Goal:** TUI wire-up: PickerSource::initial_items trait + VaultFilePickerSource constructor + App/TabCtx Arc<RecentsLog> + record_open at open chokepoint + CLI hookup + tests
**Outcome:** Shipped. `PickerSource::initial_items` trait method (default
empty); `FuzzyPicker::refresh` routes empty input through it and
`FuzzyPicker::new` auto-primes so the first render is warm.
`render_list` flips its block title to `" recent · type to search "`
when input is empty + items populated; cold-start (no `.md` files,
no opens) keeps the legacy `"type to search…"` hint.
`VaultFilePickerSource` gained a `recents: Arc<RecentsLog>` field and
a 25-row cap on its `initial_items` impl. All four picker sites
(notes open / move-source / move-target + tasks target) and the open
chokepoints (`request_open_in_editor`, `request_open_in_obsidian`,
`ft notes open` CLI) thread `Arc<RecentsLog>` through. `App` owns
`recents: Arc<RecentsLog>`; `TabCtx` exposes it; `for_test*` helpers
route the log under `vault.path/.ft-state/` so test runs don't touch
`$XDG_STATE_HOME`; new `for_test_with_recents` /
`for_test_with_clock_and_recents` for tests that need to pre-seed.
Seven new `tui::tests` cover all behavior + an e2e open-then-reopen;
CLI round-trip test exercises the XDG path. New snapshot
`notes_open_picker_recents_80x24` accepted. Full workspace: 14/14
test binaries green (629 tests total), clippy clean (`-D warnings`),
fmt clean.
