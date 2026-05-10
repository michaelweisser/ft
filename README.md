# ft

A command-line interface to your Obsidian vault, focused on the
[Tasks plugin](https://publish.obsidian.md/tasks/) emoji format. Read,
create, complete, and move tasks across thousands of notes without booting
the Electron app.

```
$ ft tasks list overdue --format markdown
- [ ] Pay rent ⏫ 🔁 every month on the 1st 📅 2026-04-01
- [ ] Finish PR review 🔼 📅 2026-05-08

$ ft tasks create "Call dentist" --due tomorrow --priority high
Created task at journal/2026-05-10.md:42
  - [ ] Call dentist ⏫ 📅 2026-05-11

$ ft tasks complete dentist --on today
Completed journal/2026-05-10.md:42
  - [x] Call dentist ⏫ 📅 2026-05-11 ✅ 2026-05-10
```

## Install

```sh
cargo install --path ft
```

This drops a single `ft` binary in `~/.cargo/bin/` (or your configured
target). MSRV is pinned in `rust-toolchain.toml`.

After install, generate shell completions:

```sh
ft completions bash > ~/.local/share/bash-completion/completions/ft
ft completions zsh  > "${fpath[1]}/_ft"
ft completions fish > ~/.config/fish/completions/ft.fish
```

…and man pages:

```sh
ft man --out ~/.local/share/man/man1
```

## Quick start

ft auto-discovers your vault by walking up from the current directory
looking for a `.obsidian/` folder. You can override that with `--vault DIR`,
the `FT_VAULT` env var, or by setting `default_vault` in
`~/.config/ft/config.toml`. Run `ft vault info` to see the resolved path
and the merged configuration.

```sh
# Find every open task across the vault
ft tasks list --status open

# Use a built-in preset
ft tasks list today
ft tasks list overdue
ft tasks list upcoming

# Filter with the query DSL (subset of the plugin's own language)
ft tasks list --query 'priority is high and not done'

# Group / sort
ft tasks list today --group-by priority --sort due

# Add a task to today's daily note
ft tasks create "Send invoice" --due +3d --priority medium --tag work

# Mark something complete (selector: id, file:line, or fuzzy)
ft tasks complete invoice
ft tasks complete journal/2026-05-10.md:7
ft tasks complete xyz123 --on 2026-05-09

# Move tasks (single, or in bulk by query) — preview first with --dry-run
ft tasks move stale-id --to inbox/triage.md
ft tasks move --query 'tag is legacy' --to inbox/triage.md#Triage --dry-run
```

## Output formats

`ft tasks list --format <fmt>` accepts:

- `table` (default) — terminal-aware, color when stdout is a TTY
- `markdown` — emits the source task lines, pipeable back into another
  vault tool
- `json` — single JSON array of full Task objects
- `ndjson` — one JSON Task per line (script-friendly)

Color is auto-suppressed when `NO_COLOR` is set, when `--no-color` is
passed, or when stdout is not a TTY.

## Scripting

For pipelines, pass `--json-errors` at the top level to get errors as a
JSON object on stderr (`{"error": ..., "chain": [...]}`). Combined with
`--allow-empty` on `tasks list` (so empty results aren't an error), `ft`
fits cleanly into shell loops and `xargs`.

```sh
ft --json-errors tasks list overdue --format ndjson \
  | jq -r '.description' \
  | head -5
```

## Documentation

- [docs/architecture.md](docs/architecture.md) — workspace layout, key
  traits, where to add a new subcommand or task format
- [docs/task-format.md](docs/task-format.md) — exactly which Tasks-plugin
  emoji fields are supported, with examples and the deferred list
- [docs/query-dsl.md](docs/query-dsl.md) — supported subset of the plugin's
  query language with grammar, examples, and an error catalog

## Status

`ft` is the foundation. A TUI (plan 002) and notes commands (plan 003)
build on top of `ft-core`.
