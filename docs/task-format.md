# Task format

`ft` parses and emits the [Obsidian Tasks plugin](https://publish.obsidian.md/tasks/)
emoji format, matching plugin v7.22 canonical output. A task is a single
markdown list-item line:

```
- [<status>] <description> <emoji-prefixed fields…> [^block-link]
```

Indented `- [<status>]` lines under a task become its children at
arbitrary depth.

## Statuses

| Marker | Status      |
|--------|-------------|
| `[ ]`  | Open        |
| `[x]`, `[X]` | Done  |
| `[/]`  | InProgress  |
| `[-]`  | Cancelled   |

Unknown markers (e.g. `[!]`) parse as Open with a `tracing` warning.
The original marker is **not** preserved on rewrite.

## Field emojis

| Emoji | Field        | Value                                       |
|-------|--------------|---------------------------------------------|
| 🔺    | priority     | Highest                                     |
| ⏫    | priority     | High                                        |
| 🔼    | priority     | Medium                                      |
| 🔽    | priority     | Low                                         |
| ⏬    | priority     | Lowest                                      |
| 🔁    | recurrence   | text until next field emoji (verbatim)      |
| ➕    | created      | `YYYY-MM-DD`                                |
| 🛫    | start        | `YYYY-MM-DD`                                |
| ⏳    | scheduled    | `YYYY-MM-DD`                                |
| 📅    | due          | `YYYY-MM-DD`                                |
| ✅    | done         | `YYYY-MM-DD`                                |
| ❌    | cancelled    | `YYYY-MM-DD`                                |
| 🆔    | id           | non-whitespace identifier                   |
| ⛔    | depends_on   | comma-separated identifiers                 |

A trailing ` ^identifier` is parsed as a markdown block-link and
preserved verbatim.

## Canonical serialization order

When `ft` rewrites a task, fields are emitted in this order:

```
description, priority, recurrence, created, start, scheduled, due,
done, cancelled, id, depends_on, [raw_trailing], [^block-link]
```

This matches the plugin's own canonical order. Round-trip property is:
`serialize(parse(line)) == line` byte-for-byte for any line already in
canonical order; lines in non-canonical order normalize on first
rewrite.

## Date parsing on input

`ft tasks create --due` and `--on` (on `complete`) accept several forms
via `ft_core::dates`:

- ISO: `2026-05-10`
- Keywords: `today`, `tomorrow`, `yesterday` (case-insensitive)
- Relative: `+3d`, `-1w`, `+10days`
- Natural language: `next monday`, `in 2 weeks` (via `chrono-english`)

`FT_TODAY=YYYY-MM-DD` overrides the system clock for deterministic
tests and reproducible scripts.

## Recurrence (whitelist)

`ft tasks complete` understands the following recurrence patterns
(case-insensitive). Anything outside this whitelist errors with the
unsupported token named.

| Rule                              | Behavior                                |
|-----------------------------------|-----------------------------------------|
| `every day`                       | next = primary + 1 day                  |
| `every N day[s]`                  | next = primary + N days                 |
| `every week`                      | next = primary + 7 days                 |
| `every N week[s]`                 | next = primary + N×7 days               |
| `every week on <weekday>`         | next = next occurrence of weekday strictly after primary |
| `every month`                     | next = primary + 1 month (chrono clamps EOM) |
| `every N month[s]`                | next = primary + N months               |
| `every month on the Nth`          | next = day N of next month (clamped to last day of that month) |

Weekdays accept full names (`monday`) or 3-letter abbreviations
(`mon`/`tue`/`tues`/`wed`/`thu`/`thur`/`thurs`/`fri`/`sat`/`sun`).
Ordinals accept bare integers or `1st`/`2nd`/`3rd`/`Nth`.

The "primary" date is the first defined of `due`, `scheduled`, `start`.
The new instance shifts every other date by the same number of days
the primary date moved.

## Daily-notes resolution

`ft tasks create` defaults to today's daily note. `[daily_notes].source`
in `.ft/config.toml` picks one of:

- `core` (default) — reads `<vault>/.obsidian/daily-notes.json`
- `periodic-notes` — reads `<vault>/.obsidian/plugins/periodic-notes/data.json`
- `explicit` — uses literal `path` and `format` keys

Both `path` and `format` accept moment.js-style patterns. Supported
tokens: `YYYY YY MMMM MMM MM M DDDD DD D dddd ddd HH mm ss` plus
`[literals]`. Tokens not in this list pass through verbatim, so folder
names like `journal/YYYY` work without bracket escaping. Reserved
moment.js tokens (`Q`, `Qo`) error explicitly.

## Deferred (out of scope for v1)

The trait shape lets these plug in later without churn:

- **Dataview format** (`task::dataview`) — sibling impl of `TaskFormat`
- **Custom statuses beyond the four standard ones** — would extend the
  `Status` enum and the marker mapping
- **Recurrence: `when done`, `skip`, `until`, count limits** — extend
  `recurrence::Rule`
- **`on_completion` field semantics** — currently preserved verbatim in
  `Task.on_completion`, but never read
- **Wikilink rewriting on cross-folder moves** — flagged with TODO in
  `task::ops::plan_move`
- **Templater integration** for auto-creating missing daily notes
