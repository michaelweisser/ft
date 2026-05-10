# Architecture

`ft` is split into a thin binary (`ft/`) and a library crate
(`ft-core/`). Everything reusable lives in the library; the binary owns
clap parsing, terminal/TTY concerns, the editor handoff, and the
interactive picker.

## Workspace layout

```
ft/
├── Cargo.toml                  # workspace manifest
├── rust-toolchain.toml         # MSRV pin
├── ft/                         # binary crate (thin)
│   ├── Cargo.toml
│   ├── src/main.rs             # clap dispatch only
│   ├── src/cmd/                # one file per subcommand
│   │   ├── tasks.rs            # list / create / complete / move
│   │   ├── vault.rs            # vault info
│   │   ├── completions.rs      # `ft completions <shell>`
│   │   └── man.rs              # `ft man [--out DIR]`
│   ├── src/output/             # table.rs, json.rs, markdown.rs, ndjson.rs
│   └── tests/                  # integration tests with assert_cmd
└── ft-core/                    # library crate (the brain)
    ├── Cargo.toml
    └── src/
        ├── lib.rs
        ├── vault.rs            # discovery + scan
        ├── config.rs           # layered config (user + vault)
        ├── daily.rs            # daily-notes resolution
        ├── dates.rs            # ISO / keyword / relative / NL parsing
        ├── fs.rs               # write_atomic
        ├── selector.rs         # id / file:line / fuzzy
        ├── error.rs
        ├── task/
        │   ├── mod.rs          # Task struct, Status / Priority enums
        │   ├── format.rs       # TaskFormat trait
        │   ├── emoji.rs        # emoji format impl
        │   ├── hierarchy.rs    # parent-pointer resolution
        │   ├── ops.rs          # create_task / complete_task / plan_move
        │   └── recurrence.rs   # rule parser + next-instance engine
        └── query/
            ├── mod.rs
            ├── filter.rs       # programmatic typed filters
            ├── expr.rs         # AST: Expr / Atom
            ├── dsl.rs          # tokenizer + recursive-descent parser
            ├── preset.rs       # built-in named queries
            └── sort.rs         # sort_by_keys + SortKey / SortOrder
```

## Key traits and seams

### `TaskFormat`

`ft-core::task::format::TaskFormat` is the seam between the in-memory
`Task` model and a wire format. Implementors provide:

```rust
fn parse_line(&self, line: &str, ctx: ParseContext) -> Option<Task>;
fn serialize_line(&self, task: &Task) -> String;
```

The v1 implementation is `task::emoji::EmojiFormat`, matching the
Obsidian Tasks plugin v7.22 canonical output. A dataview implementation
is a future plug-in here — every consumer (scanner, ops layer, query
engine) holds a `Task`, not a format-specific representation, so a new
format only needs to plug into this trait.

### Operations API (`task::ops`)

Mutation primitives. Each one reads a file, computes the new content,
and writes via `crate::fs::write_atomic`:

- `create_task(path, input, opts)` — insert a new task at append /
  under-heading / at-line position; refuses duplicates unless `--force`
- `complete_task(path, line, opts)` — mark a task done; if recurring,
  insert the next instance above the now-completed line
- `plan_move(sources, target)` — pure: produce a `MovePlan` of per-file
  before/after edits without writing
- `apply_move_plan(plan)` — write each non-no-op edit atomically

The CLI binary calls these directly. The TUI (plan 002) will compose
them at finer granularity.

### Query language (`query::dsl`)

`dsl::parse(src, today)` returns a `Query { expr, sort_keys, limit }`.
The expression AST (`query::expr`) is `Expr` (And/Or/Not over `Atom`s)
where atoms are predicates like `Status(Open)`, `DueBefore(date)`, etc.
`Expr::matches(&Task)` evaluates against a task. The CLI composes the
DSL with flag filters by AND-ing the parsed expression with a typed
`Filter`. See `docs/query-dsl.md` for the supported subset.

## Adding things

### A new subcommand

1. Create `ft/src/cmd/<name>.rs` with an `Args` struct and `run` fn.
2. Add `pub mod <name>;` to `ft/src/cmd/mod.rs`.
3. Add the variant to `Commands` in `ft/src/main.rs` and dispatch it.
4. If it needs vault data, call `Vault::discover(vault_flag)?` and
   `vault.scan()` — same pattern as the existing subcommands.

### A new task format (e.g. dataview)

1. Create `ft-core/src/task/<name>.rs` implementing `TaskFormat`.
2. Add format detection in `vault::parse_file` (try formats in priority
   order configured by `.ft/config.toml`).
3. Round-trip property tests: `serialize(parse(line)) == line` and
   `parse(serialize(task)) == task` (proptest, snapshot, real-vault).

### A new output format

1. Add a module to `ft/src/output/`.
2. Add a variant to `output::Format`.
3. Wire it into the match in `ft/src/cmd/tasks.rs::run_list`.

## Testing strategy

- **Unit tests** live with the modules (`#[cfg(test)] mod tests`)
- **Integration tests** under `ft/tests/` use `assert_cmd` + `assert_fs`
  against fixture vaults built per-test in temp directories
- **Fixture vaults** under `tests/fixtures/`: `tiny/` (a few tasks),
  `realistic/` (~25 tasks across PARA + journal + inbox), `pathological/`
  (deep subtasks, every emoji, weird unicode, malformed lines)
- **Snapshot tests** with `insta` for stable output formats
- **Proptest** round-trips for the parser
- **Real-vault tests** (`ft-core/tests/real_vault.rs` and
  `ft/tests/real_vault_cli.rs`) gated on `FT_REAL_VAULT_TESTS=1` so CI
  never depends on a local vault

## Build invariants

- `cargo build --release` produces a single `ft` binary
- `cargo test --workspace` runs everything
- `cargo clippy --workspace --tests -- -D warnings` is clean
- `cargo fmt --check` is clean
