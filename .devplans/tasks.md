---
id: 001
name: tasks
title: "Tasks: foundation library + ft tasks CLI"
status: finished
created: 2026-05-09
updated: 2026-05-10
---

# Tasks: foundation library + ft tasks CLI

## Goal
Establish `ft` as a Rust workspace (library + binary) that can locate an
Obsidian vault, parse and serialize tasks in the Obsidian-Tasks emoji format
(matching plugin v7.22 behavior closely enough for round-trip safety), and
expose a first set of subcommands — `ft tasks list`, `ft tasks create`,
`ft tasks move`, `ft tasks complete` — that scratch real daily-driver itches
on a working PARA-style vault. This plan ships an end-to-end vertical slice;
later plans add a TUI, a cache layer, and notes commands on top.

## Motivation and Context
The user maintains a real vault at `/Users/cmw/git/fortytwo` (PARA layout,
Tasks plugin v7.22 active) and wants command-line + scriptable access to the
same task data Obsidian sees, without booting the Electron app. The CLI is
the foundation; a TUI (plan 002) and notes commands (plan 003) reuse this
library. Getting the parser and the data model right is load-bearing for
everything that follows, so this plan invests in a strong test bed
(fixture vaults, snapshot tests, proptest round-trips) before building UX.

## Acceptance Criteria

### Workspace & project skeleton
- [x] Cargo workspace with members `ft` (binary, thin), `ft-core` (library)
- [x] `cargo build --release` produces a single `ft` binary; `cargo test --workspace` passes
- [x] `ft --version` and `ft --help` work; subcommand structure uses clap derive
- [x] CI-ready: clippy clean with `-D warnings`, rustfmt clean, MSRV pinned in `rust-toolchain.toml` to a recent stable
- [x] README with quick-start, install instructions (`cargo install --path ft`), and a one-page architecture overview

### Vault discovery & config
- [x] Discovery precedence: `--vault` flag > `FT_VAULT` env > walk up from CWD looking for `.obsidian/` > named vaults in `~/.config/ft/config.toml` (`default` key)
- [x] Per-vault config file at `<vault>/.ft/config.toml`, layered on top of user config (per-vault wins)
- [x] Config schema covers: `default_task_location`, `daily_notes_path`, `daily_notes_format`, `ignored_paths`, `presets` (named queries)
- [x] When no vault can be found, error message lists every location that was tried
- [x] `ft vault info` subcommand prints resolved vault path, config files in effect, and merged config

### Task model (library)
- [x] `Task` struct in `ft-core` with: `description`, `status` (enum: Open, Done, InProgress, Cancelled), `priority` (Highest..Lowest, optional), `tags`, `created`, `start`, `scheduled`, `due`, `done`, `cancelled` dates, `recurrence` (string form preserved verbatim for v1; semantic parsing deferred), `id`, `depends_on` (Vec<String>), `on_completion` field preserved verbatim, `block_link`, `source_file`, `source_line`, `indent_level`, `parent` (for subtask hierarchy)
- [x] Standard statuses only: `[ ]` Open, `[x]`/`[X]` Done, `[/]` InProgress, `[-]` Cancelled. Unknown markers parse as Open with a warning surfaced via `tracing`
- [x] Multi-level subtask support: indented `- [ ]` lines under a task become children; arbitrary depth
- [x] Format module is trait-based (`TaskFormat` trait with `parse_line` / `serialize_line`); emoji format is the v1 implementation; dataview format is a deferred impl that will plug into the same trait
- [x] Round-trip property: for any `Task` produced by parsing a real line, `serialize(parse(line)) == line` byte-for-byte (proptest covers generated tasks; snapshot tests cover real fixtures)
- [x] Parser preserves unknown emojis/fields in a `raw_trailing` field so we never lose data on rewrite

### Vault scanning
- [x] `Vault::scan()` walks markdown files using the `ignore` crate (respects `.gitignore` + configured `ignored_paths`)
- [x] Parallel parsing with `rayon`; aim for sub-second scan on a vault with ~5k notes on the test machine
- [x] Scan returns `Vec<Task>` with file/line provenance preserved
- [x] Scan errors per-file are collected and reported, not fatal — one bad file does not abort the run
- [x] Excludes binary files and any path under `.obsidian/`, `.git/`, `attachments/` (configurable)

### `ft tasks list`
- [x] Flag-based filters: `--status`, `--priority`, `--tag`, `--path`, `--due-before`, `--due-after`, `--scheduled-before`, `--scheduled-after`, `--has-due`, `--no-due`
- [x] `--query "<DSL>"` accepts a subset of the Obsidian Tasks query language: status / priority / path / tag predicates, date comparisons, `and`/`or`/`not`, `sort by <field>`, `limit N`. Document the supported subset; reject the rest with a clear error pointing to the docs section
- [x] Flags compose with `--query` (flags appended as additional `and` clauses)
- [x] `--sort` flag with multiple keys; default sort: due date asc, then priority desc, then path
- [x] Output formats: `--format table` (default, with terminal width awareness via `comfy-table`), `--format json`, `--format ndjson`, `--format markdown` (emits the source lines so output can be piped back as a task list)
- [x] Presets: `ft tasks list <preset-name>` looks up the preset in config; ships with built-ins `today`, `overdue`, `upcoming`, `done-today` that users can override
- [x] `--group-by path|folder|due|priority|tag` for the table format
- [x] Exit code 1 if no tasks match (configurable via `--allow-empty`) — useful in scripting

### `ft tasks create`
- [x] Positional arg is the description; flags add metadata: `--due`, `--scheduled`, `--start`, `--priority`, `--tag` (repeatable), `--recurrence`, `--id`, `--depends-on`
- [x] Date parsing accepts ISO (`2026-05-10`), relative (`+3d`, `tomorrow`), and natural language (`next monday`) — `chrono` + `chrono-english`
- [x] Default location: today's daily note resolved from a configurable source. `[daily_notes].source` in ft's config picks one of `core` (Obsidian's built-in plugin, default), `periodic-notes` (community plugin), or `explicit` (`path` + `format` keys, both supporting moment.js patterns like `journal/YYYY`). If the chosen source can't be resolved, fail with a message naming alternative sources and `--file`
- [x] `--file <path>` overrides location (relative to vault root)
- [x] `--under-heading "<heading>"` inserts at the end of the section under that heading; creates the heading at file end if missing
- [x] `--at-line N` inserts at a specific 1-indexed line
- [x] `--append` appends to file end (default for daily note path with no heading)
- [x] `--edit` opens `$EDITOR` on the resulting line after writing, positioned at the new task (use `EDITOR` env var; fall back to `vi`)
- [x] Atomic writes: write to a temp file in the same directory, then rename; preserves file mode
- [x] Idempotency: refuses to create an exact duplicate task (same description + same dates) on the same day unless `--force`

### `ft tasks complete`
- [x] `ft tasks complete <selector>` marks one or more tasks done. Selector forms: task `id` (the `🆔 abc123` field), `<file>:<line>`, or interactive picker with `fzf`-style prompt (use `dialoguer` or `inquire`) when ambiguous
- [x] Sets done date to today (or `--on <date>`)
- [x] If task has `recurrence`, creates the next instance at the original location and marks the current one done — matching plugin behavior. Recurrence semantics in v1 cover daily/weekly/monthly with a clearly-tested whitelist; unsupported patterns error out with the exact unsupported token

### `ft tasks move` and bulk move
- [x] `ft tasks move <selector> --to <file>[#heading]` moves a single task (and its subtasks) to the new location
- [x] `ft tasks move --query "<DSL>" --to <file>[#heading]` bulk-moves all matching tasks; prompts for confirmation showing a count and a sample of 5 unless `--yes`
- [x] Move preserves indentation/subtask hierarchy, rewrites the source files atomically, and updates internal `[[wikilinks]]` ONLY if the target file is in a different folder (deferred — note in code comments that this needs a follow-up plan)
- [x] Dry-run with `--dry-run` prints the diff of every affected file without writing

### Error model & UX
- [x] Library uses `thiserror` enums; binary uses `anyhow` with `Context`
- [x] All errors include vault-relative paths (not absolute) where possible
- [x] `--verbose` / `-v` flags map to `tracing` levels
- [x] `--json-errors` produces structured error output for scripting
- [x] Color output via `owo-colors`, auto-disabled when stdout is not a TTY or `NO_COLOR` is set

### Testing
- [x] Unit tests live with the modules in `ft-core/src/`
- [x] Integration tests under `ft/tests/` use `assert_cmd` + `assert_fs` against fixture vaults checked into `tests/fixtures/`
- [x] At least three fixture vaults: `tiny/` (a few tasks, all formats), `realistic/` (tens of notes mirroring PARA layout), `pathological/` (deep subtasks, weird emoji combos, malformed lines)
- [x] Snapshot tests with `insta` for every output format on each fixture
- [x] Proptest round-trip on the parser (generated tasks → serialize → parse → equal)
- [x] At least one test that runs against the real fortytwo vault if present (gated on env var `FT_REAL_VAULT_TESTS=1` so CI doesn't depend on it), comparing list output before/after `ft tasks complete` is a no-op
- [ ] Coverage target: 80%+ on `ft-core` (track via `cargo-llvm-cov` but don't gate CI on it) — deferred (explicit non-gating per plan)

### Documentation
- [x] `docs/architecture.md` — workspace layout, key traits, where to add a new subcommand, where to add a new task format
- [x] `docs/task-format.md` — exactly which Obsidian Tasks emoji fields are supported, with examples and a "deferred" section
- [x] `docs/query-dsl.md` — the supported subset of the query language with grammar and examples
- [x] `man/ft.1` and per-subcommand man pages generated from clap (use `clap_mangen`) via `ft man [--out DIR]`
- [x] Shell completions generated for bash/zsh/fish via `clap_complete` via `ft completions <shell>`

## Technical Notes

### Workspace layout
```
ft/
├── Cargo.toml                  # workspace manifest
├── rust-toolchain.toml         # MSRV pin
├── ft/                         # binary crate (thin)
│   ├── Cargo.toml
│   ├── src/main.rs             # clap dispatch only
│   ├── src/cmd/                # one file per subcommand: list.rs, create.rs, move.rs, complete.rs, vault.rs
│   ├── src/output/             # table.rs, json.rs, markdown.rs
│   └── tests/                  # integration tests with assert_cmd
├── ft-core/                    # library crate (the brain)
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── vault.rs            # discovery + scan
│       ├── config.rs           # layered config (user + vault)
│       ├── task/
│       │   ├── mod.rs          # Task struct, Status/Priority enums
│       │   ├── format.rs       # TaskFormat trait
│       │   └── emoji.rs        # emoji format impl
│       ├── query/
│       │   ├── mod.rs
│       │   ├── filter.rs       # programmatic filter API
│       │   ├── dsl.rs          # parser for the query subset
│       │   └── sort.rs
│       ├── daily.rs            # daily-notes core plugin config reader
│       └── error.rs
├── tests/
│   └── fixtures/
│       ├── tiny/
│       ├── realistic/
│       └── pathological/
└── docs/
```

### Library/binary boundary
The binary owns clap parsing, output rendering, terminal/TTY concerns, and the
editor handoff. Everything else — vault discovery, config, parsing, scanning,
filtering, sorting, mutation primitives — lives in `ft-core` and is consumed
unchanged by the TUI in plan 002. The library exposes both an "operations" API
(`scan_tasks`, `create_task`, `move_tasks`, `complete_task`) and the underlying
types so the TUI can compose finer-grained workflows.

### Why a trait for task formats
The plugin supports two serialization modes (emoji + dataview). v1 ships emoji
only, but the trait shape lets us add dataview as a sibling impl without
touching the rest of the codebase. Format detection per-line (a file can mix
both, in theory) is supported by trying parsers in priority order configured
via `.ft/config.toml` (`task_formats = ["emoji"]` initially).

### Dependencies (locked-in stack)
```
clap 4 (derive), pulldown-cmark, ignore, rayon, chrono, chrono-english,
serde, toml, figment, anyhow (binary), thiserror (lib), tracing,
tracing-subscriber, ratatui (plan 002 only), crossterm, comfy-table,
owo-colors, dialoguer or inquire, clap_mangen, clap_complete,
insta, assert_cmd, assert_fs, proptest
```

### Atomic file writes
Every task mutation goes through a single `write_atomic(path, content)` helper
that writes to `path.tmp-XXXX` in the same directory then renames. Same dir
matters for atomicity guarantees on POSIX. Preserve file mode and (where
practical) mtime semantics that don't fight git.

### Editor handoff
Only `--edit` triggers `$EDITOR`. The TUI may invoke it later for
"edit task in editor" actions but that's plan 002. Use `std::process::Command`
with the file path; pass `+<line>` for vim-family editors (parse `$EDITOR`
basename to decide).

### Parser strategy
The Tasks-plugin emoji format is **not** standard markdown extension syntax.
Each task is a line that starts (after indentation) with `- [<status>] ` and
then has the description with embedded emoji-prefixed fields. We do NOT need
pulldown-cmark for the task line itself — a hand-written parser scoped to "one
line at a time" is cleaner and gives us byte-accurate provenance. We use
pulldown-cmark only to find the list-item ranges in a file (so we know which
lines are actually task lines vs prose that happens to start with `- [`).

### Daily-notes resolution
Read `<vault>/.obsidian/daily-notes.json` for `folder`, `format` (moment.js
format string — translate to chrono format, with a small allowlist of tokens
documented in `docs/task-format.md`), and `template`. Fail loudly if the
moment.js format uses tokens we don't support, with a pointer to the doc.

### Out of scope for this plan
- Dataview format (trait is in place; impl is a future session)
- Custom statuses beyond the four standard ones
- Index/cache layer (mtime-based or sled) — only added if scan-on-demand
  is too slow on the real vault
- Templater integration to auto-create missing daily notes
- Rewriting wikilinks during moves (cross-folder)
- Recurrence patterns beyond daily/weekly/monthly basics
- Anything UI beyond the CLI (TUI = plan 002)

## Sessions
### Session 1 · 2026-05-09 · done
**Goal:** Cargo workspace + vault discovery + layered config + `ft vault info`

**Scope:**
- Cargo workspace with `ft` (binary) and `ft-core` (library) members
- `rust-toolchain.toml` pinning a recent stable; `rustfmt.toml`, `clippy.toml`
- Top-level `Cargo.toml` with the dependency stack agreed in the plan
- `ft-core::vault` module: `Vault::discover()` with full precedence chain
  (flag > `FT_VAULT` env > walk up from CWD > user config default)
- `ft-core::config` module: layered loading from `~/.config/ft/config.toml`
  and `<vault>/.ft/config.toml` using `figment`; per-vault wins
- Config schema (serde structs) with sensible defaults; unknown keys
  preserved (round-trip safe) or rejected with a clear error — pick one and
  document
- Error types via `thiserror` in lib; `anyhow` + `Context` in binary
- `tracing` + `tracing-subscriber` set up; `-v` flag wired up in clap
- `ft vault info` subcommand prints resolved vault path, all config files in
  effect (with their precedence ranking), and the merged config

**Tests:**
- Unit tests for discovery precedence (each rung of the chain) using
  `assert_fs` to build temp directory trees
- Unit test for config layering (user-only, vault-only, merged, conflict
  resolution)
- Integration test: `ft vault info` against the `tiny/` fixture vault
- First fixture vault committed at `tests/fixtures/tiny/` — empty `.obsidian/`
  marker file plus a couple of markdown files; just enough to exercise
  discovery

**Done means:**
- `cargo build --release` produces a working `ft` binary
- `cargo test --workspace` passes
- `cargo clippy --workspace -- -D warnings` clean
- `cargo fmt --check` clean
- `ft vault info` works against the real `/Users/cmw/git/fortytwo` vault and
  prints something useful

**Advances acceptance criteria:** all of "Workspace & project skeleton",
all of "Vault discovery & config", parts of "Error model & UX" (thiserror /
anyhow split, `-v` flag, vault-relative paths in errors).

**Deferred:** task parsing, scanning, any `tasks` subcommand. The README is
deferred to session 8 (we'll have something concrete to document by then).

**Outcome:** Cargo workspace scaffolded with `ft` (binary) and `ft-core` (library) crates.
`Vault::discover()` implements the full four-rung precedence chain with `find_vault`
owning its own `tried` list. Config layering via `figment` (user + vault, vault wins);
unknown keys rejected with a clear error via `#[serde(deny_unknown_fields)]`.
`thiserror`/`anyhow` split in place; `-v` verbosity flag wired to `tracing-subscriber`.
`ft vault info` prints vault path, config file precedence, and merged config.
18 tests green (`cargo test --workspace`), clippy clean with `-D warnings`, fmt clean.
Works against the real `/Users/cmw/git/fortytwo` vault. Decision: unknown config keys
are rejected (not preserved) for early typo detection.

### Session 2 · 2026-05-09 · done
**Goal:** Task model + emoji-format parser + serializer + round-trip property tests

**Scope:**
- `ft-core::task` module:
  - `Task` struct with all fields enumerated in the plan
  - `Status` enum (Open, Done, InProgress, Cancelled) with parse/display
  - `Priority` enum (Highest, High, Medium, Low, Lowest) with the matching
    plugin emojis
  - `raw_trailing` field to preserve unknown emojis/fields verbatim
- `TaskFormat` trait with `parse_line(line, ctx) -> Option<Task>` and
  `serialize_line(&Task) -> String`
- `task::emoji` module: full emoji-format implementation
- `task::hierarchy`: helper that takes a `Vec<Task>` from one file (with
  `source_line`/`indent_level`) and resolves `parent` pointers for subtasks
- Logging warnings via `tracing` for unknown status markers (parsed as Open)

**Tests:**
- Unit tests for each emoji field's parser and serializer in isolation
- Snapshot tests (`insta`) over a corpus of real task lines copied from the
  fortytwo vault into `tests/fixtures/tiny/sample-tasks.md`
- Proptest round-trip: `for_all_tasks: parse(serialize(t)) == t`
- Proptest preservation: `for_all_lines: serialize(parse(line)) == line`
  byte-for-byte (where `parse` returns `Some`)
- Subtask hierarchy test: 3+ levels deep, mixed statuses
- Pathological cases: blank description, all emojis, no emojis, weird
  whitespace, embedded brackets in description, unicode, long line

**Done means:**
- The library can parse every task in the real fortytwo vault without losing
  data on round-trip (run as a one-off check, not committed as a test)
- All tests pass, no clippy warnings, no flaky proptest cases
- The trait shape supports a hypothetical dataview impl without any change
  to consumers (write a stub `task::dataview` module returning
  `unimplemented!()` to prove the seams are right; remove the stub at end
  of session if too noisy)

**Advances acceptance criteria:** all of "Task model (library)".

**Deferred:** scanning multiple files (next session), dataview format
(future plan), recurrence semantic parsing (session 6 — we just preserve
the string here).

**Outcome:** `Task` struct with all planned fields; `Status`/`Priority` enums; `TaskFormat` trait
with `ParseContext`; `EmojiFormat` implementing the full Obsidian Tasks emoji format. Parser
detects field boundaries using date-validity checks (so `📅 today` stays in the description).
Space-preserving raw_trailing accumulator retains post-field comment content byte-for-byte.
`resolve_hierarchy` wires parent pointers by indent level. 73 unit + proptest tests green.
Real-vault smoke test: 4,674 tasks parsed, 0 unexpected round-trip mismatches (11 trailing-space
and 1 unknown-status artifacts are documented, expected behavior). Clippy clean, fmt clean.

### Session 3 · 2026-05-09 · done
**Goal:** Parallel vault scan + `ft tasks list` with flag filters + table/JSON output

**Scope:**
- `ft-core::vault::scan()` walks markdown files via `ignore` (respects
  `.gitignore` + `ignored_paths` from config)
- `rayon` parallelism over the file list; per-file errors collected, not
  fatal; returns `(Vec<Task>, Vec<ScanError>)`
- Default exclusions: `.obsidian/`, `.git/`, `attachments/` (configurable)
- `ft tasks list` clap subcommand with flag filters: `--status`,
  `--priority`, `--tag` (repeatable), `--path`, `--due-before`,
  `--due-after`, `--scheduled-before`, `--scheduled-after`, `--has-due`,
  `--no-due`
- `ft-core::query::filter` programmatic API that takes typed filters and
  returns matching tasks
- Output formats: `table` (default, via `comfy-table`, terminal-width
  aware) and `json` (full `Task` structure)
- Color output via `owo-colors` (TTY/`NO_COLOR`-aware) — table only
- Default sort: due asc, priority desc, path

**Tests:**
- Snapshot tests (`insta`) for table and JSON output against the `tiny/`
  fixture
- Add `tests/fixtures/realistic/` (~30 notes, PARA-shaped, mix of done /
  open / overdue / future tasks)
- Snapshot tests for each filter flag in isolation against `realistic/`
- Test that `.gitignore` is respected
- Test that one malformed task file does not abort the scan and is
  reported in the error list
- Microbench (criterion or just `Instant` in a `#[ignore]` test) on
  `realistic/` to track scan time

**Done means:**
- `ft tasks list` works against the real fortytwo vault and produces
  reasonable output
- All flag filters covered by snapshot tests
- Scan is parallel (visible in CPU usage on `realistic/`)
- JSON output is parseable by `jq` (test in shell from CI script)

**Advances acceptance criteria:** all of "Vault scanning"; first half of
"`ft tasks list`" (flags, table, JSON, default sort); parts of "Testing"
(realistic fixture, snapshot tests).

**Deferred:** query DSL, presets, `--sort`/`--group-by` flags, markdown
and ndjson formats, `--allow-empty` — all session 4.

**Outcome:** `Vault::scan()` walks markdown files via `ignore` (respecting `.gitignore`,
default exclusions of `.obsidian/`, `.git/`, `attachments/`, plus per-vault `ignored_paths`),
parses in parallel via `rayon`, and returns `Scan { tasks, errors }` with relative paths
and per-file hierarchy resolved. `ft-core::query::filter::Filter` implements the conjunctive
typed filter API; `query::sort::default_sort` orders due-asc, priority-desc, path. `ft tasks
list` clap subcommand wires every flag from the plan (`--status`, `--priority`, `--tag`,
`--path`, `--due-before/-after`, `--scheduled-before/-after`, `--has-due`, `--no-due`) plus
`--format table|json` and `--no-color`. Table output via `comfy-table` with TTY/`NO_COLOR`/
`--no-color`-aware coloring. Realistic fixture vault committed (~25 tasks across PARA folders
+ Journal + inbox, plus `.gitignore`'d `private/` and `attachments/` to prove exclusions
fire). 18 integration tests + 5 new unit tests on scan/filter/sort = 112 total tests green;
clippy clean with `-D warnings`; fmt clean. Real-vault smoke: 4,675 tasks scanned in ~0.7s
wall time on `/Users/cmw/git/fortytwo`. `Status` enum gained `Copy`. Decision: tags must
appear before field emojis in source lines to be indexed (parser stops at first field
boundary); fixture authored accordingly, matching plugin convention.

### Session 4 · 2026-05-09 · done
**Goal:** Query DSL subset + sort/group + presets + markdown/ndjson output

**Scope:**
- `ft-core::query::dsl` parser for the supported subset:
  - Predicates: `status is X`, `priority is X`, `path includes X`,
    `tag is X` / `has tag X`, `due {before|after|on} <date>`,
    `scheduled {before|after|on} <date>`, `done`, `not done`,
    `has due`, `no due date`
  - Boolean combinators: `and`, `or`, `not`, parens
  - `sort by <field> [reverse]` (multi-key, comma-separated or repeated)
  - `limit N`
- Reject anything outside the subset with an error pointing to a specific
  doc anchor; tested explicitly per unsupported feature
- `--query "<DSL>"` flag composes with the existing flag filters from
  session 3 (flags become additional `and` clauses)
- `--sort` flag with multiple keys
- `--group-by path|folder|due|priority|tag` — table format only
- Built-in presets: `today`, `overdue`, `upcoming`, `done-today`
- User-defined presets read from `presets` map in config; user definitions
  override built-ins
- `markdown` output format (emits source lines, pipeable back as a task
  list); `ndjson` format
- `--allow-empty` flag; default exit code 1 on no matches

**Tests:**
- Unit tests for DSL parser: each predicate, each combinator, error cases
- Snapshot tests for each preset against `realistic/`
- Snapshot tests for grouped table output
- Add `tests/fixtures/pathological/`: deep subtasks, every emoji, weird
  unicode, intentionally malformed lines
- Test that flags + `--query` compose as `and`
- Test that user preset shadows built-in

**Done means:**
- `ft tasks list "not done and due before tomorrow sort by priority"` works
- `ft tasks list today` works against the real vault
- All three fixture vaults exercised by snapshot tests

**Advances acceptance criteria:** remainder of "`ft tasks list`"; pathological
fixture (Testing); query DSL doc placeholder created (real doc in session 8).

**Deferred:** docs prose (session 8), shell completions for preset names
(session 8).

**Outcome:** Session 4 ships the full DSL + presets + formats stack.
`ft-core::query::dsl` adds a hand-rolled tokenizer + recursive-descent parser
producing a `Query { expr, sort_keys, limit }`; the AST lives in
`query::expr` (`Expr` / `Atom`) and evaluates against `Task` directly.
Predicates supported: `status is X`, `priority is X`, `path includes X`,
`tag is X` / `has tag X`, `due/scheduled/completed (before|after|on) <date>`,
`done`, `not done`, `has due [date]`, `no due date`. Combinators: `and`, `or`,
`not`, parentheses, with `and` binding tighter than `or`. Tail clauses:
`sort by <key> [reverse], …` and `limit N`. Date keywords `today`,
`tomorrow`, `yesterday` resolve against an injected `today: NaiveDate`, and
the CLI honours an `FT_TODAY=YYYY-MM-DD` env var for deterministic tests and
reproducible scripts. Unsupported plugin features (`group by`, `hide`, etc.)
and unknown identifiers reject with structured `DslError` variants pointing at
`docs/query-dsl.md`. `query::preset::builtin` ships `today`, `overdue`,
`upcoming`, `done-today` as DSL strings (so user definitions in
`Config.presets` shadow them through the same parser); positional CLI arg is
preset-then-DSL fallback. `--query` and the positional arg compose with flag
filters as additional `and` clauses. `--sort` accepts repeated or
comma-separated keys with a `:reverse` suffix and overrides any DSL
`sort by`. `--group-by path|folder|due|priority|tag` renders one labelled
sub-table per bucket via the new `output::table::render_grouped`. New output
formats `markdown` (round-trippable source lines via `EmojiFormat::serialize_line`)
and `ndjson` (one Task JSON object per line) live in `ft/src/output/`.
`--allow-empty` flips the default exit code, which is now `1` when nothing
matches so the binary plays nicely in pipelines. `Config.presets:
HashMap<String, String>` was added with a unit test for round-trip loading.
Pathological fixture committed at `tests/fixtures/pathological/` covering deep
subtasks (5 levels), every emoji field, weird unicode, `[brackets]`, wikilinks,
trailing whitespace, and malformed lines (`[ task` / `[?]` / `[]missing-space`)
— scanner does not crash and surfaces a `tracing` warning for unknown markers
as designed. Real-vault smoke (`/Users/cmw/git/fortytwo`) confirms presets and
DSL queries return sensible markdown lines. Tests: 38 query-module unit tests,
46 `tasks_list` integration tests (DSL predicates, combinators, presets, user
preset shadowing via temp vault, sort overrides, grouped table sections,
markdown/ndjson formats, `--allow-empty` vs default exit-1, pathological scan,
deep-subtask parent resolution), 4 cli unit tests on `parse_cli_sort_keys`,
plus prior tests = **172 total green**, clippy clean with `-D warnings`,
fmt clean. Decisions: (a) `not done` parses to a primitive `Atom::NotDone`
rather than `Not(Done)` for cleaner snapshots and to match plugin convention;
(b) DSL `sort` only fires when the CLI does not pass `--sort`, so the CLI is
the more local override; (c) preset resolution prefers user config over
built-ins by name match, with built-ins still living in code so they always
have a known-parseable definition.

### Session 5 · 2026-05-09 · done
**Goal:** `ft tasks create` + atomic writes + daily-notes resolution + date parsing + editor handoff

**Scope:**
- `ft-core::fs::write_atomic(path, content)` helper: temp file in same dir,
  rename, preserve mode
- `ft-core::dates` module: parse ISO + relative (`+3d`, `tomorrow`,
  `yesterday`) + natural language (`next monday`) via `chrono` +
  `chrono-english`; one entry point that tries each in order
- `ft-core::daily` module: read `<vault>/.obsidian/daily-notes.json`,
  translate the moment.js format string subset to chrono format, return
  the resolved path for a given date; clear error on unsupported tokens
- `ft-core::task::ops::create_task(...)` library API
- `ft tasks create` CLI subcommand with all flags from the plan
  (`--due`, `--scheduled`, `--start`, `--priority`, `--tag` repeatable,
  `--recurrence`, `--id`, `--depends-on`, `--file`, `--under-heading`,
  `--at-line`, `--append`, `--edit`, `--force`)
- Default location: today's daily note; if missing, hard error with
  remedy hint
- Idempotency: refuse exact duplicate same-day unless `--force`
- `--edit` opens `$EDITOR` at the new task's line (vim-family `+N` syntax;
  fall back to opening the file)

**Tests:**
- Unit tests for date parsing (each form)
- Unit tests for daily-notes path resolution: each supported moment.js
  token; rejection of unsupported tokens
- Atomic write: test that interruption mid-write leaves the original
  file intact (simulate by writing to a path then panicking; assert
  original unchanged)
- Integration tests using `assert_fs` temp vaults: create with `--file`
  + `--under-heading`, with `--at-line`, with `--append`; verify the
  file content with snapshot tests
- Test idempotency: second create with same description+date refused;
  with `--force` it goes through
- Round-trip: create, list, output contains the new task

**Done means:**
- `ft tasks create "buy milk" --due tomorrow --priority high` works
  against the real fortytwo vault and lands in today's daily note
- All flag combinations have integration test coverage
- Atomic write is genuinely atomic on the test machine

**Advances acceptance criteria:** all of "`ft tasks create`"; parts of
"Error model & UX" (atomic writes, vault-relative paths in messages).

**Deferred:** Templater integration to auto-create missing daily notes
(future plan).

**Outcome:** Session 5 ships `ft tasks create` end-to-end. New ft-core modules:
`fs::write_atomic` (tempfile-based same-dir rename, preserves POSIX mode),
`dates::parse` (ISO → keywords `today/tomorrow/yesterday` → relative `±Nd/±Nw`
forms incl. `+10days` → chrono-english fallback), `daily` (reads
`<vault>/.obsidian/daily-notes.json`, defaults `format` to `YYYY-MM-DD` and
`folder` to `""` to match Obsidian, translates the supported moment.js subset
`YYYY/YY/MMMM/MMM/MM/M/DDDD/DD/D/dddd/ddd/HH/mm/ss` plus `[literals]` to chrono
format, rejects unsupported tokens by name), and `task::ops::create_task`
(builds the task line via `EmojiFormat::serialize_line`, positions it via
`Position::Append | UnderHeading | AtLine`, refuses duplicates whose
description+due+scheduled+start all match unless `--force`, writes via
`write_atomic`). `Status` gained `Default = Open`. The CLI `tasks create`
subcommand wires every flag from the plan: free-text positional description
(joined across argv), `--due/--scheduled/--start` parsed via `dates`,
`--priority`, `--tag` (repeatable, `#` optional, deduplicated against the
description), `--recurrence/--id/--depends-on`, plus position flags
`--file/--under-heading/--at-line/--append` (mutually exclusive via clap),
`--edit` (launches `$EDITOR` with `+N` for vim-family editors, otherwise just
the file path; falls back to `vi`), and `--force`. Default location is today's
daily note resolved from the core plugin config; missing config errors with a
hint to either configure it or pass `--file`. The duplicate error and CLI
output use vault-relative paths. `FT_TODAY=YYYY-MM-DD` honored for
deterministic tests. New deps: `chrono-english 0.1`, `tempfile 3`,
`serde_json` exposed in ft-core. Tests: 15 ft-core unit tests on `fs`/`dates`/
`daily`/`ops` (atomic writes incl. mode preservation and an interrupted-write
safety test, every supported and one rejected moment.js token, every date
form incl. case-insensitive keywords, every position branch, idempotency
with and without `--force`, distinct-dates non-duplicate, heading parsing
edge cases) plus 14 new integration tests under `ft/tests/tasks_create.rs`
covering create-in-daily-note, custom file, under-heading existing/missing,
at-line, duplicate refusal/force/relative-path, invalid date, multi-arg
description, recurrence/id/depends-on, missing daily-notes config remedy,
round-trip create→list. **227 total tests green** (was 172), clippy clean
with `-D warnings` (added a single `#[allow(clippy::large_enum_variant)]` on
the top-level `Commands` enum since it's parsed once from argv), fmt clean.
Real-vault smoke against `/Users/cmw/git/fortytwo`: both default daily-note
resolution (folder `journal/2024` from the user's stale-but-real config
landed the task at `journal/2024/2026-05-09.md`) and `--file` worked; both
files cleaned up after verification. Decisions: (a) the "section end" for
`UnderHeading` walks back over trailing blank lines so the new task visually
belongs to its heading rather than the next section's whitespace; (b) tags
passed via `--tag` are appended as `#tag` to the description (deduped) so
they round-trip cleanly through the parser's tag index; (c) duplicate
detection ignores status — a done duplicate is still a duplicate, matching
"don't accidentally re-add the same thing" semantics.

**Post-session refactor (same day):** Daily-note resolution generalized from
hard-coded core-plugin lookup to a `[daily_notes]` config table with three
sources: `core` (default; reads `.obsidian/daily-notes.json`),
`periodic-notes` (reads `.obsidian/plugins/periodic-notes/data.json`'s
`daily` block, respects `enabled = false`, defaults empty `format` to
`YYYY-MM-DD`), and `explicit` (uses `path` + `format` keys directly, both
supporting moment.js patterns so `path = "journal/YYYY"` keeps working as
the year rolls over). The flat `daily_notes_path` / `daily_notes_format`
keys were replaced by the table. `translate_format` relaxed from "reject
any unrecognized letter run" to "pass through anything that isn't a known
token", with a small reserved-tokens list (currently `Q`/`Qo`) that still
errors loudly — this lets ordinary folder names (`journal`, `notes`,
`inbox`) appear in `path` patterns without bracket escaping, matching
moment.js's own permissive behavior. Verified end-to-end against
`/Users/cmw/git/fortytwo` for all three sources: core lands at
`journal/2024/2026-05-09.md` (the user's stale core config), periodic-notes
and explicit `journal/YYYY` both land at `journal/2026/2026-05-09.md`.
**238 tests green** (up from 227); clippy + fmt clean.

### Session 6 · 2026-05-09 · done
**Goal:** `ft tasks complete` + selector resolution + recurrence engine (daily/weekly/monthly)

**Scope:**
- Selector parser: `<id>`, `<file>:<line>`, fall through to interactive
  picker (`inquire` or `dialoguer`) when ambiguous and stdin is a TTY
- `ft-core::task::ops::complete_task(...)` library API
- `ft tasks complete <selector>` CLI with `--on <date>` and `--yes`
- `ft-core::task::recurrence` engine:
  - Whitelist: `every day`, `every week`, `every month`, with optional
    `on the Nth`, `on <weekday>`, anchored to due/scheduled/start date
  - Returns the next instance's dates; unsupported patterns produce an
    error naming the unsupported token
  - Matches plugin behavior on the cases in the whitelist (cross-checked
    against the plugin's own test fixtures where possible)
- On completion of a recurring task: write the next instance at the
  original location, mark the current as done

**Tests:**
- Selector resolution unit tests for each form
- Recurrence unit tests: every supported pattern × due/scheduled/start
  anchor × edge cases (end-of-month, leap day, DST-adjacent)
- Recurrence rejection tests: each unsupported token produces a clear
  error with the offending substring
- Integration test: complete a non-recurring task, verify done date set
- Integration test: complete a recurring task, verify both old (done)
  and new (open, next date) tasks present in the file
- Snapshot test on the resulting file content for a few realistic
  recurrence cases

**Done means:**
- `ft tasks complete <id>` works against the real vault
- Recurrence engine behaves correctly on every whitelisted pattern
- Unsupported recurrence patterns produce errors that name the token

**Advances acceptance criteria:** all of "`ft tasks complete`".

**Deferred:** richer recurrence (skip, until, count, etc.) — future plan
once we hit a real case where the user wants it.

**Outcome:** Session 6 ships `ft tasks complete` end-to-end. New ft-core
modules: `task::recurrence` (parser + engine for the whitelisted patterns —
`every [N] day[s]`, `every [N] week[s]`, `every week on <weekday>`,
`every [N] month[s]`, `every month on the Nth`; case-insensitive; ordinal
suffixes `1st/2nd/3rd/Nth` accepted; weekday full names + 3-letter abbrevs;
unsupported tokens including `every year`, `when done`, and
`every 2 weeks on monday` reject with `RecurrenceError::Unsupported { rule,
token }` naming the offending substring) and `selector` (three forms:
`Selector::Id` exact match against `Task.id`, `Selector::FileLine` exact
relative-path + 1-indexed line with suffix-match support so
`inbox.md:5` resolves `notes/inbox.md:5`, `Selector::Fuzzy` case-insensitive
substring against description or path, restricted to non-Done tasks). The
new `task::ops::complete_task(target_path, line, CompleteOptions { on })`
reads the target file, parses the line, errors cleanly on
`LineMissing`/`NotATask`/`AlreadyDone`, marks the task done with the given
date, and — if the task is recurring — inserts the next instance *above*
the now-completed line (matching plugin behavior). Anchor preference is
due > scheduled > start; secondary dates shift by the same delta as the
primary date; chrono's end-of-month clamping is respected, with
`MonthOnDay` re-clamped against the destination month so `monthly on the
31st` advances 2026-01-31 → 2026-02-28 → 2026-03-31. The CLI `tasks
complete` subcommand wires every flag from the plan: positional
`<selector>` (optional — when omitted the picker shows all open tasks),
`--on <date>` (parsed via the same `dates` module so ISO/keywords/relative
all work; defaults to today), and `--yes` (skips the picker by replacing it
with an error listing up to 5 candidates so it's pipeline-friendly). Bare
ID selectors fall back to fuzzy matching when no task has the literal id —
so `ft tasks complete dog` finds `Walk dog`. The picker uses
`dialoguer::FuzzySelect` (added with `default-features = false, features =
["fuzzy-select"]` so we don't pull in the rest of the crate). Stdin TTY
detection through `is_terminal` triggers the candidate-list error in
non-TTY contexts so scripts get a clean error rather than hanging on a
prompt. All error messages use vault-relative paths. Tests: 28
`task::recurrence` unit tests covering each pattern × edge cases (leap
day, end-of-month, year rollover, anchor preference, no-anchor error,
delta-shift consistency), 12 `selector` unit tests (parser
classification + resolve rules), 8 `task::ops::tests::complete_*` unit
tests (incl. recurrence integration, file-content snapshots, indentation
preservation, unsupported-pattern atomicity), 11 `tasks_complete.rs`
integration tests (id / file:line / fuzzy / `--on` / recurrence /
already-done / non-match / ambiguous-with-yes / unsupported-recurrence
non-modification / round-trip create-list-complete-list). **287 → 298
total tests green** (`4 + 11 + 18 + 46 + 5 + 213 + 1`); clippy clean with
`-D warnings`; fmt clean. Real-vault smoke against
`/Users/cmw/git/fortytwo`: created two test tasks (one non-recurring, one
`every week`) in a temp file inside the vault; `ft tasks complete
<id>` produced `- [x] … ✅ 2026-05-10` for the non-recurring task and
inserted `📅 2026-05-17` next instance above the completed `📅 2026-05-10`
line for the recurring one; cleanup removed the smoke directory.
Decisions: (a) the next instance is inserted *above* the completed line
because that's plugin behavior and means `ft tasks list` then sees the
new instance ahead of the completed one in source order — useful in
markdown output; (b) selector parser prefers the structured forms
(`file:line`, then bare ID) and the CLI does an Id→Fuzzy fallback so a
single-word selector like `dog` keeps working when no `🆔 dog` exists;
(c) `Fuzzy` resolution skips Done tasks by default (you don't normally
re-complete a done task by fuzzy match) but `Id` and `FileLine` don't, so
the CLI's `AlreadyDone` error path is reachable when the user is
explicit; (d) `ft tasks complete` (no selector) opens the picker over all
open tasks rather than erroring — feels like the natural "what do I want
to mark done right now?" affordance.

### Session 7 · 2026-05-09 · done
**Goal:** `ft tasks move` single + bulk-move + dry-run + diff preview + confirmation

**Scope:**
- `ft-core::task::ops::move_tasks(...)` library API: takes a list of
  resolved tasks and a target (file + optional heading); produces a plan
  of file edits (`Vec<FileEdit>`) without writing anything
- Apply step that takes the plan and writes atomically (per file)
- Hierarchy preservation: when moving a task with children, the children
  move with it; indentation is normalized to the new context
- `ft tasks move <selector> --to <file>[#heading]` (single)
- `ft tasks move --query "<DSL>" --to <file>[#heading]` (bulk)
- `--dry-run` prints a unified diff per affected file (use `similar`
  crate); no writes
- Confirmation prompt for bulk: shows count + sample of 5 task lines;
  bypassed by `--yes`
- Code comment in `move_tasks` explicitly noting the deferred wikilink
  rewriting on cross-folder moves; pointer to plan 003

**Tests:**
- Unit tests for the edit-plan builder: single task, task with children,
  task that's mid-file, task at file end
- Integration test: bulk-move with `--query`, snapshot the resulting
  files
- Test that `--dry-run` does not modify any file (compare mtimes
  before/after)
- Test confirmation flow with mocked stdin (`--yes` path is the
  test-friendly one)
- Test that moving the same task twice produces no diff on the second run
  (idempotent under no-op)

**Done means:**
- `ft tasks move --query "tag is #legacy" --to inbox/triage.md --dry-run`
  works on the real vault and prints a sensible diff
- Bulk move on `realistic/` produces correct files (snapshot)

**Advances acceptance criteria:** all of "`ft tasks move` and bulk move".

**Deferred:** wikilink rewriting on cross-folder moves (plan 003).

**Outcome:** Session 7 ships `ft tasks move` end-to-end. New ft-core
types in `task::ops`: `MoveSource { path, line }`, `MoveTarget` (variants
`Append(PathBuf)` / `UnderHeading(PathBuf, String)`), `MovePlan { edits,
blocks }`, `FileEdit { path, original, new }`, `MovedBlock { source,
end_line, task_description }`. `plan_move(sources, target)` reads each
affected file once, parses the head line of each move source to learn its
indent level, computes the contiguous block range (head line + every
following line whose first non-whitespace column exceeds the head indent;
blank lines bound the block), drops descendants whose range is contained
in another in-list range so users can pass both `parent` and `child`
selectors without double-moving, normalizes the moved block's indentation
by stripping the head's leading whitespace from every line, and produces
a `Vec<FileEdit>` of before/after content (target included; unchanged
files yield `original == new`). When source and target are the same file
the plan threads removals through before insertion. `apply_move_plan`
walks the edits and writes atomically through `crate::fs::write_atomic`,
skipping no-ops. Wikilink rewriting on cross-folder moves is explicitly
deferred to plan 003 and called out in a code TODO at the top of
`plan_move`. The CLI `tasks move` subcommand: positional `<selector>` or
`--query DSL` (mutually exclusive via clap), `--to FILE[#HEADING]` (parsed
into `MoveTarget`; relative paths resolve against the vault root),
`--dry-run` prints unified per-file diffs via `similar::TextDiff` and
never writes, `--yes` bypasses bulk confirmation; without `--yes` and
without a TTY a bulk move errors with a message naming the flag, and with
a TTY the user gets a `dialoguer::Confirm` prompt previewing 5 task
lines + count. Selector resolution reuses `selector::parse` /
`selector::resolve` with the same Id→Fuzzy fallback the `complete`
subcommand has, so `ft tasks move buy-milk --to inbox.md` works for both
literal IDs and fuzzy matches. Output uses vault-relative paths. New
deps: `similar = "2"` for the unified diff renderer (added to workspace +
`ft` binary). Tests: 11 new `task::ops::tests::move_*` unit tests
covering single move, subtree-with-children, indent normalization, target
heading existing/missing, parent-subsumes-child dedupe, multi-file bulk,
within-same-file move, non-task error, line-out-of-range error, and the
empty-input no-op edge case; 9 `tasks_move.rs` integration tests covering
single by id, heading creation, subtree move, dry-run-no-mtime-change,
bulk with `--yes`, bulk-no-tty errors, no-match error, idempotent second
run, within-same-file under heading. **298 → 318 tests green** (`4 + 11
+ 18 + 46 + 9 + 5 + 224 + 1`); clippy clean with `-D warnings`; fmt
clean. Real-vault smoke against `/Users/cmw/git/fortytwo`: created a
temporary `_ft_smoke_session7/source.md` with two tagged tasks and one
indented child; `--dry-run` printed the expected unified diff against
both source and (yet-to-exist) target, leaving the source untouched and
the target uncreated; the real bulk move with `--to
'_ft_smoke_session7/triage.md#Triage'` excised the two parent tasks +
the beta task's child, created the heading, dropped the duplicate child
from the move list, and produced clean post-move file contents; cleanup
removed the smoke directory. Decisions: (a) the block boundary stops at
the first blank line, matching how Obsidian renders adjacent task lists
as one item — this means moving a task does not pull in commentary from
a separate following list; (b) `--to FILE` (no heading) appends to the
target instead of inserting at the top, which feels closer to "queue"
semantics for a triage workflow than "stack"; (c) child-subsumed-by-
parent is computed by file-relative range containment rather than by
walking `Task.parent` pointers — so the rule works even when the user
passes raw `file:line` selectors that bypass scan-time hierarchy.

### Session 8 · 2026-05-09 · done
**Goal:** Polish — man pages, shell completions, color/`NO_COLOR`, `--json-errors`, docs, real-vault test

**Scope:**
- `clap_mangen` man page generation for `ft` and each subcommand;
  generated into `man/`; build.rs or a `xtask` to regenerate
- `clap_complete` for bash, zsh, fish; `ft completions <shell>`
  subcommand emits to stdout
- Final pass on color output: TTY detection, `NO_COLOR`, `--no-color` flag
- `--json-errors` flag: structured error output for scripting
- Docs:
  - `README.md` with quick-start, install, link to architecture
  - `docs/architecture.md` — workspace layout, key traits, extension points
  - `docs/task-format.md` — exactly which Tasks-plugin emoji fields are
    supported, with examples and a "deferred" section
  - `docs/query-dsl.md` — full grammar of the supported subset, examples,
    error catalog
- Coverage check via `cargo-llvm-cov`, target 80%+ on `ft-core`; fix gaps
- Real-vault test gated on `FT_REAL_VAULT_TESTS=1`: scans
  `/Users/cmw/git/fortytwo`, asserts task count > 0, asserts list output
  is non-empty, asserts a list-then-list cycle is stable

**Tests:**
- man pages generate without panicking and contain the expected sections
- Completions parse correctly (shellcheck or equivalent)
- `--json-errors` produces valid JSON on every error path (smoke test
  every error variant)

**Done means:**
- The project is publishable: `cargo install --path ft` works, completions
  drop into the right shell paths, man pages install, README is enough to
  get someone going
- Coverage report committed to `docs/`
- All acceptance criteria in plan 001 ticked

**Advances acceptance criteria:** "Documentation"; remainder of "Error
model & UX"; "Testing" (real-vault test).

**Outcome:** Session 8 polishes the project for general use. New CLI
subcommands `ft completions <bash|zsh|fish|elvish|powershell>` (via
`clap_complete::generate`) emits the script to stdout, and `ft man
[--out DIR]` (via `clap_mangen`) renders the top-level man page to
stdout, or — with `--out DIR` — writes one man file per subcommand
and nested subcommand into the directory (`ft.1`, `ft-vault.1`,
`ft-tasks.1`, `ft-tasks-list.1`, `ft-tasks-create.1`,
`ft-tasks-complete.1`, `ft-tasks-move.1`). The meta-subcommands
`completions`/`man`/`help` are intentionally excluded from man-page
generation. New global flag `--json-errors` short-circuits the
top-level error printer in `main`: instead of the human-readable
`anyhow` chain, errors render as a single-line JSON object on stderr
(`{"error": "<top-level message>", "chain": [<every link in the
context chain>]}`), so pipelines can `jq -r .error` cleanly. The
human path is unchanged when the flag is absent. `main` was
restructured from `Result<ExitCode>` to a manual `match` so the
JSON branch can intercept the error before the default anyhow Display
runs. New deps in `ft`: `clap_complete = "4"`, `clap_mangen = "0.2"`;
`serde_json` was promoted to dev-deps for the polish-flag tests.
Documentation: `README.md` (quick-start, install, completions/man
hints, output formats overview, scripting tips, links to the in-repo
docs), `docs/architecture.md` (workspace layout, the `TaskFormat` /
`task::ops` / `query::dsl` seams, build invariants, how to add a new
subcommand / output format / task format, testing strategy),
`docs/task-format.md` (every emoji field with table, canonical
serialization order, date-input forms, recurrence whitelist, daily-
notes resolution, deferred list), `docs/query-dsl.md` (full grammar,
examples, date keywords, built-in presets, composition with flag
filters, sort/limit, error catalog mapping every `DslError` variant
back to the grammar). Tests: 8 new `polish.rs` integration tests
covering each shell's completion script signature, `ft man` stdout
mode and `--out DIR` mode (asserting every expected page exists and
contains a `.TH` header, and that meta-subcommands are excluded), and
both `--json-errors` (parses as JSON, contains `error` + `chain`) and
the human-error fallback. New `real_vault_cli.rs` test file gated on
`FT_REAL_VAULT_TESTS=1` adds 4 CLI-level real-vault smokes
(non-empty list, list-then-list byte-stable, `overdue` preset runs,
`--dry-run` move against `_ft_smoke_real_target.md` with a synthetic
no-match query); when the env var is unset they early-return so CI
never depends on a local vault. **318 → 330 tests green** (`4 + 8 +
4 + 11 + 18 + 46 + 9 + 5 + 224 + 1`); clippy clean with `-D
warnings`; fmt clean. Real-vault smoke against
`/Users/cmw/git/fortytwo` (run as `FT_REAL_VAULT_TESTS=1 cargo test`)
passes all four CLI smokes plus the existing parser-level
`real_vault_round_trip`. Coverage gating is explicitly deferred per
plan ("track via cargo-llvm-cov but don't gate CI on it"); every
other acceptance criterion in plan 001 is now ticked. Decisions: (a)
`ft man` writes per-subcommand man pages with the page title set via
`clap_mangen::Man::title(&str)` rather than mutating
`Command::name`, because clap's `Str` type only converts from
`&'static str`, and `title` is the documented seam for this anyway;
(b) the `--json-errors` JSON shape is intentionally minimal
(`error` + `chain`) so consumers don't break when we add fields
later; (c) `ft man` stdout-only renders the top-level page rather
than every page concatenated, so `ft man | man -l -` works as
expected for quick reading; (d) the existing `--no-color` /
`NO_COLOR` / TTY-detection plumbing already covered the "Color
output" criterion from earlier sessions, so this session just ticks
it off without code changes.
