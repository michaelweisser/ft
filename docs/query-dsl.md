# Query DSL

`ft` ships a deliberately small subset of the [Obsidian Tasks plugin
query language](https://publish.obsidian.md/tasks/Queries/About+Queries).
Anything outside the grammar below errors with the exact unsupported
token named, so you always know what to change.

Pass a query with `--query "<DSL>"` or as the positional argument to
`ft tasks list`. CLI flag filters (`--status`, `--priority`, etc.) are
appended as additional `and` clauses.

## Grammar

```text
query     = or_expr [ "sort" "by" sort_keys ] [ "limit" integer ]
or_expr   = and_expr ( "or" and_expr )*
and_expr  = unary    ( "and" unary    )*
unary     = "not" unary | atom
atom      = "(" or_expr ")" | predicate

predicate = "status"    "is"  status_val
          | "priority"  "is"  prio_val
          | "path"      "includes" string
          | ( "tag" "is" | "has" "tag" ) tag_val
          | "due"       ("before"|"after"|"on") date_val
          | "scheduled" ("before"|"after"|"on") date_val
          | "completed" ("before"|"after"|"on") date_val
          | "done" | "not" "done"
          | "open" | "in-progress" | "cancelled"
          | "has" "due" [ "date" ]
          | "no"  "due" "date"

sort_keys = sort_key ( "," sort_key )*
sort_key  = ("due"|"scheduled"|"priority"|"path"|"description"|"status")
            [ "reverse" ]

status_val = "open" | "done" | "in-progress" | "cancelled"
prio_val   = "highest" | "high" | "medium" | "low" | "lowest"
date_val   = YYYY-MM-DD | "today" | "tomorrow" | "yesterday"
```

`and` binds tighter than `or`. Use parentheses to override.

## Status predicates

| Predicate         | Matches                                  |
|-------------------|------------------------------------------|
| `done`            | Status == Done                           |
| `not done`        | Status is Open or InProgress (excludes Done **and** Cancelled) |
| `open` / `todo`   | Status == Open                           |
| `in-progress` / `in_progress` / `doing` | Status == InProgress       |
| `cancelled` / `canceled` | Status == Cancelled               |
| `status is X`     | Long form; `X` is `open`/`done`/`in-progress`/`cancelled` |

`not done` matches plugin convention: a cancelled task isn't on your
plate, so it shouldn't show up when you ask for what's "not done." If
you need the literal `Status != Done` (cancelled included), use the
parenthesized form `not (done)` — the outer `not` then wraps the
literal `done` predicate instead of the special-cased "still
actionable" atom.

## Examples

```sh
# Open tasks with no due date
ft tasks list --query 'not done and no due date'

# Anything high or higher priority due by end of next week
ft tasks list --query 'priority is high and due before 2026-05-18'

# Bare-status predicates — easier to type than `status is open`
ft tasks list --query 'open and priority is high'
ft tasks list --query 'in-progress'
ft tasks list --query 'cancelled and completed before 2026-01-01'

# Tasks tagged work or personal that are not done, sorted by due asc then priority desc
ft tasks list --query '(tag is work or tag is personal) and not done sort by due, priority reverse'

# First five overdue items
ft tasks list --query 'not done and due before today sort by due limit 5'
```

## Date keywords

`today`, `tomorrow`, and `yesterday` resolve against the current day,
which can be pinned with `FT_TODAY=YYYY-MM-DD`. Useful for tests and
for reproducible scripts:

```sh
FT_TODAY=2026-05-10 ft tasks list today
```

## Built-in presets

| Name          | Definition                                              |
|---------------|---------------------------------------------------------|
| `today`       | `not done and (due on today or scheduled on today)`     |
| `overdue`     | `not done and due before today`                         |
| `upcoming`    | `not done and due after today`                          |
| `done-today`  | `done and completed on today`                           |

User-defined presets in `[presets]` of `~/.config/ft/config.toml` or
`<vault>/.ft/config.toml` shadow built-ins by name:

```toml
[presets]
backlog = "not done and no due date and tag is project sort by priority reverse"
```

```sh
ft tasks list backlog
```

## Composing with flag filters

Flag filters (`--status`, `--priority`, `--tag`, `--path`,
`--due-before/-after`, `--scheduled-before/-after`, `--has-due`,
`--no-due`) compose with `--query` as additional `and` clauses. So this:

```sh
ft tasks list --query 'priority is high' --status open --tag work
```

…is equivalent to:

```sh
ft tasks list --query 'priority is high and status is open and tag is work'
```

`--sort` overrides any `sort by` in the DSL when both are present (the
CLI is the more local override). `--group-by` only affects the `table`
format.

## Sorting and limits

```text
sort by <key>[,<key>...] [reverse]   — multi-key, comma-separated
limit N                               — keep only the first N rows
```

`--sort` from the CLI accepts comma-separated or repeated values, with
optional `:reverse` / `:asc` / `:desc` suffix per key:

```sh
ft tasks list --sort priority,due:reverse
ft tasks list --sort priority --sort due:desc
```

Default sort (no `--sort` and no DSL `sort by`): due asc, priority
desc, path asc.

## Error catalog

The DSL parser produces `DslError`. The variants you'll see at the CLI:

- `UnexpectedToken { found, expected }` — a known token in the wrong
  place. Example: `due 2026-05-10` (missing `before`/`after`/`on`).
- `UnknownIdentifier(name)` — the parser doesn't recognize this word.
  Example: `priority is urgent` (use `highest`/`high`/`medium`/etc.).
- `InvalidDate(s)` — `YYYY-MM-DD` expected.
- `InvalidNumber(s)` — `limit` argument must be a positive integer.
- `UnterminatedString` — unmatched quote in a `"path includes"` value.
- `UnsupportedFeature(name)` — features the plugin supports but `ft`
  doesn't yet (`group by`, `hide`, `show`, `description includes`).
- `EmptyInput` — the query string was empty after trimming.
- `TrailingTokens(rest)` — extra tokens after a complete query (or
  before `sort`/`limit` keywords; check ordering).

Errors point at `docs/query-dsl.md` (this file) so you can map a
message back to the grammar.
