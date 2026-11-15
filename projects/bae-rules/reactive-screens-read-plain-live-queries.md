---
digest: A bae screen shows the rows of its SQL query, with no cache, enrichment or other step between the query and the screen, except values that change many times a second (keyed latest-value streams) and state that exists only on screen (kept by the view). A read that follows changes is one coven live query; any other read is a plain SQL read. Each window of a long list is its own live query bounded by a key at both ends. Rust and the apps never keep memory between reads, run a side pass, force a rerun, pick a read path by size, or filter, sort, join, group or count rows SQL returned, and nothing answers from the rows a screen has loaded.
paths:
  - bae-*/src/**/*.rs
  - '**/*.sql'
  - bae-macos/**/*.swift
  - BaeKit/**/*.swift
  - bae-android/**/*.kt
blocking: true
---

## Reactive screens read plain live queries

A screen shows the rows of its SQL query, and the counts, totals, section
headers, "N of M", sums and facets that come with them. Rust turns the rows into
bridge types and the app draws them, with no cache, enrichment or other step
between the query and the screen. A long list shows several windows at a time,
and each window is its own live query.

Two things a screen shows don't come from SQL:

- A value that changes many times a second (playback position, transfer bytes,
  rates, levels) travels on a keyed latest-value stream
  (`high-frequency-values-skip-observed-ui-state.md`).
- State that exists only on screen (hover, focus, an open popover or menu, a
  drag in progress) stays in the view and never goes into SQL.

### Why

The database is bae's own: SQLite on the device's disk, with no server or
network in between. SQLite reads and writes fast, and coven reruns a query
whenever a write changes what it read, so a screen that shows a query's result
stays current with no other code.

Data that Rust copies into memory and works on there (a structure built from
rows, a filter, a sort, a count, a cache) stops following the database. Some
other code then has to notice each change and redo the work, and until it does,
the screen shows an old copy. That code does what SQLite and coven already do,
worse. So nothing goes between the query and the screen.

### Two kinds of read

A read that must follow changes (a screen, or a subscriber in the bridge,
automation or MCP) is one SQL query subscribed through coven, which reruns it
when a write changes what it read. Any other read (a lookup, a command's check,
an export) is a plain SQL read.

A count or total goes in the same query as the rows. A count changes with every
insert, so when it makes the rows' query rerun too often, move the count into a
live query of its own.

Every answer is SQL, and so is every fix:

- Grouping, counting, joining, ranking: GROUP BY, COUNT, JOIN, a window
  function.
- A slow query: a better query or an index.
- A value too slow to compute on each read, or computed across tables that
  SQLite can't index together: a device-local table that triggers write in the
  same write that changes any table the value comes from, with an index on it.
  The album grid sorts by `album_listing`, and the import list reads its tabs,
  states, groups, dates and shown text from `import_list_placement`
  (`bae-import/src/tables/import_list/placement.sql`).

Start from the plain query over the domain's own tables, and add an index or a
trigger table when the plain query can't serve the screen.

### Long lists

Each window the screen shows is its own live query, bounded by a sort key at
both ends:

```sql
WHERE (sort) >= (:from) AND (sort) < (:until) ORDER BY sort
```

Coven decides which writes rerun a query from its WHERE comparisons, not from
LIMIT or OFFSET. A bounded window reruns only when a row inside it changes;
`LIMIT 100` reruns for every change after its start, and `OFFSET` for every
change at all.

The screen holds two or three windows in a chain, each starting at the key where
the last one ends, so every row is in exactly one. To add a window, or to jump
to a position (a scrollbar drag, an end key), the screen reads the key at the
new edge once with a plain read and subscribes the window from there. Every key
comes from the screen's request, never from rows Rust kept.

```text
subscribe({from: null,      until: <key of row 100>})  -- rows 0-99
subscribe({from: <row 100>, until: <key of row 200>})  -- rows 100-199
subscribe(SELECT COUNT(*) FROM ...)                    -- the count
```

### Never, in Rust or in the apps

- **Memory between reads.** No counts, windows, positions or last results kept
  (in a `Mutex`, `OnceLock`, atomic, field or cache) for a later read to use.
  Every read starts from the database.
- **A side pass.** No second walk over the rows beside the live query to count
  them or note where groups start.
- **A forced rerun.** No field written into a request, a table or a version only
  so a query reads again, and no write count kept in a table
  (`additions = additions + 1`) so a reader can tell a write happened. A query
  reruns because what it read changed.
- **A size cutoff.** No threshold (rows, matches, bytes) that sends a read down
  one of two paths.
- **A live query read once.** A read that takes the first value and stops is a
  plain SQL read.
- **SQL's work done outside SQL.** No filtering, sorting, joining, grouping,
  counting or summing of rows a query returned. A length or emptiness test is a
  count. A second statement keyed by values the first one returned is a join:
  one statement reads both.
- **Answers from the loaded rows.** No test of whether an id is among the rows
  the screen holds, whether they are empty, or where a row sits among them, even
  for state the view owns (a drop target, a removal animation, a menu's
  targets). The loaded rows change with scrolling. SQL answers, or the view
  keeps the ids it acted on.

A plan or brief that proposes one of these shapes is wrong: raise it instead of
building it.

### What checks this

bae's pre-commit hook refuses the shapes it can see in Rust read code (the
ownership check's `read-shapes` rule). The violations in the code today are
listed in `read_shapes.frozen` in `owner-construction-check.toml`, one row per
function and shape, with how many times it occurs there. The hook lets those
through, and fails a commit that adds a row or raises a count, so the list can
only shrink as the code is fixed. A frozen row is a violation not yet fixed,
never permission: a change to a frozen site that keeps the shape still breaks
this rule. Uses over values that are not a read's rows (a constant table, one
name's characters) go in `allowed` with what the values are.

The landing review covers what the hook cannot see: the apps' Swift and Kotlin,
work on rows after they leave read code (the bridge, services), loops that
filter without the named methods, statements combined in Rust without a map,
memory kept outside a live-query holder, a counter Rust writes as a bound value,
a threshold with no named constant, and calls through receivers of unknown type.
