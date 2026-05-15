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

## Note links

`ft notes backlinks <note>` lists every other note that links *to* the
target; `ft notes links <note>` lists every link going *out* of the
target (including `[[Unresolved]]` ghost targets). `<note>` accepts a
vault-relative path, a bare title, or a fuzzy query (in that order),
matching the ergonomics of `ft notes open`.

```sh
ft notes backlinks finance              # who links to Areas/finance.md?
ft notes backlinks Areas/finance.md     # explicit path also works
ft notes links Journal/2026-05-15.md    # what does today's note link to?
ft notes links hub --format ndjson      # script-friendly output
```

The link graph (`ft_core::graph`) is built from a parallel scan of
every markdown file in the vault, recognising wikilinks (`[[Foo]]`,
`[[Foo|alias]]`, `[[Foo#anchor]]`), markdown links (`[Foo](foo.md)`),
and embeds (`![[Foo]]`, `![alt](image.png)`). Resolution follows
Obsidian's defaults — for collisions, the shortest path wins, with
alphabetical tiebreak. Unresolved targets become "ghost" nodes that
backlinks queries can still find.

The four `--format` values (`table` / `json` / `ndjson` / `markdown`)
are the same as `ft tasks list`. `--allow-empty` is honored — pass it
in scripts that don't want a 1 exit on a no-link query.

`ft notes rename <note> <new-name-or-path>` moves a note and rewrites
every link in the vault to point at the new name. Bare new name keeps
the same directory; a path with `/` is vault-relative. `.md` is
appended automatically. Wikilink display aliases (`[[foo|My Foo]]`)
and heading anchors (`[[foo#H]]`) survive the rewrite verbatim;
markdown links re-render with the URL-encoded path relative to each
linker's directory; embeds keep their `!` prefix.

```sh
ft notes rename foo bar                 # foo.md → bar.md, link rewrites
ft notes rename notes/foo notes/bar     # explicit vault-relative path
ft notes rename foo archive/foo         # move across directories
ft notes rename "[[Phantom]]" Real      # rewrite linkers; no file created
ft notes rename foo bar --dry-run       # print plan, write nothing
```

A freshness guard (`(mtime, len)` per touched file at plan time)
catches the "user edited a file in another tool between plan and
apply" case and aborts before any write. The applier sorts same-file
edits by descending byte offset so multi-link rewrites in one file
are byte-safe; the file rename happens last so a self-linking note
stays correct.

## Git sync

`ft git sync` commits any working-tree changes in the vault repo,
pulls the configured upstream, and pushes — one shot. The repo is
discovered by walking up from the vault root; the feature is
unavailable if no `.git/` exists anywhere up the tree. The same
operation is available in the TUI via the `g s` chord on the Notes
and Tasks tabs.

```sh
ft git sync                     # commit, pull, push
ft git sync -m "msg override"   # override the auto-generated message
ft git sync --dry-run           # print the plan, write nothing
```

Conflicts (merge or rebase) leave markers in the files and exit `2`
with the conflicted-file list on stderr — resolve manually. The
pull strategy (`merge` default, `rebase` opt-in) is configured under
`[git]` in [docs/config.md](docs/config.md).

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
