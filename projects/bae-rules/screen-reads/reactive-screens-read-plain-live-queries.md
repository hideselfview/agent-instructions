---
digest: >-
  A bae screen shows the rows of its SQL query, with no cache, enrichment or
  other step between the query and the screen, except values that change many
  times a second (keyed latest-value streams, which carry a value in Rust's
  memory or one a Rust watcher computes from it and a live query's latest
  result, and never a plain query result), state that exists only on screen
  (kept by the view), and a pure Rust function of one live query result that
  only shapes how its rows display (coven's `process`). A read that follows
  changes is one coven live query; any other read is a plain SQL read. Each
  window of a long list is its own live query bounded by a key at both ends. SQL
  decides which rows a screen gets, reads every row a value needs in one
  statement, and counts across windows. Rust and the apps never keep memory
  between reads (incremental or lazy computation included), run a side pass,
  force a rerun, pick a read path by size, or filter, sort or join rows outside
  SQL, and nothing answers from the rows a screen has loaded. The apps draw the
  rows Rust hands them and never filter, sort, group or count them.
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
between the query and the screen. In Rust, a pure function of one query result
may shape how that result's rows display. A long list shows several windows at a
time, and each window is its own live query.

Two kinds of value reach a screen outside its query's rows:

- A value that changes many times a second travels on a keyed latest-value
  stream, as a channel would carry it inside Rust
  (`native-views-draw-fast-changing-values.md`).
  - The value lives in Rust's memory (playback position, transfer bytes, rates,
    levels), or a Rust watcher computes it from such a value and a live query's
    values ("A fast value computed with a live query's values").
  - Core never writes such a value to a table for a query to read.
  - Core never puts a plain query result, with no fast-changing part, on a fast
    stream. Such a value reaches the screen through the screen's live query,
    however often it changes.
- State that exists only on screen (hover, focus, the selected rows, an open
  popover or menu, a drag in progress, the text typed in a search or input
  field) stays in the view that controls it and never goes into SQL. A query
  that depends on it, such as a search, gets it in the request, and an action on
  the selected rows gets their ids in the request.
  - A drag or a click that writes stays in progress until its write returns. The
    view keeps drawing the moved row or the new value until then, and then draws
    the query's rows, so the screen never jumps back to the old order or value.
    When the write fails, the view draws the query's rows, which show what is
    stored. The view keeps nothing past the write's return, and never waits for
    a later query result to match what it drew.
  - When a query's parameters change, such as a search's text as the person
    types, the view keeps drawing the last rows until the query returns its
    first rows for the new parameters, and then draws those. The screen never
    empties between one set of parameters and the next.

### Why nothing goes between the query and the screen

The database is bae's own: SQLite on the device's disk, with no server or
network in between. SQLite reads and writes fast, and coven reruns a query
whenever a write changes what it read, so a screen that shows a query's result
stays current with no other code.

Data that Rust copies into memory and works on there (a structure built from
rows, a filter, a sort, a count, a cache) stops following the database. Some
other code then has to notice each change and redo the work, and until it does,
the screen shows an old copy. That code does what SQLite and coven already do,
worse. So nothing goes between the query and the screen.

A pure function of one query result doesn't stop following the database: it runs
again each time the query reruns and keeps nothing between runs. So Rust may run
one ("Shaping one result in Rust").

### Two kinds of read

A read that must follow changes (a screen, or a subscriber in the bridge,
automation or MCP) is one SQL query subscribed through coven, which reruns it
when a write changes what it read. Any other read (a lookup, a command's check,
an export) is a plain SQL read.

A count or total goes in the same query as the rows. A count changes with every
insert, so when it makes the rows' query rerun too often, move the count into a
live query of its own.

SQL decides which rows a read returns and computes every value across a list,
and every fix is in SQL:

- Filtering, sorting, joining, grouping, counting and ranking a list's rows:
  WHERE, ORDER BY, JOIN, GROUP BY, COUNT, a window function.
- A slow query: a better query or an index.
- A value too slow to compute on each read, or computed across tables that
  SQLite can't index together: a device-local table that triggers write in the
  same write that changes any table the value comes from, with an index on it.
  The album grid sorts by `album_listing`, and the import list reads its tabs,
  states, groups, dates and shown text from `import_list_placement`
  (`bae-import/src/tables/import_list/placement.sql`).

Start from the plain query over the domain's own tables, and add an index or a
trigger table when the plain query can't serve the screen.

### Shaping one result in Rust

Rust may run a pure, stateless function over one live query result when the
function only shapes how that result's rows display. These functions qualify:

- One that merges label rows that share a catalog number into display lines.
- One that summarizes a release's audio files into one format line.
- One that numbers an album's releases by their place in the result.

Rust passes such a function to the live query's `process` method
(`LiveQuery::process`, `ReconfigurableLiveQuery::process`), and coven runs it:

- coven runs the function on each result, on a worker, after it releases the
  database connection.
- coven compares the function's output, not the rows: when a write reruns the
  query and the output equals the last one, the subscriber gets no new value.

A `process` function never does any of these:

- Decide which rows a screen gets, such as with a filter or sort over a long
  list, or a window. SQL does.
- Use rows outside the one result.
  - A second statement keyed by values the first one returned is a join: one
    statement reads both.
- Count across windows. SQL does, in the rows' query or a live query of its own.
- Keep state between runs, such as memory or a cache.
  - An incremental or lazy computation framework keeps results between runs to
    recompute only what changed. That is memory between reads, so bae never uses
    one.

### A fast value computed with a live query's values

A fast stream may carry a value that Rust computes from a value in its memory
that changes many times a second, such as transfer progress or playback
position, together with a live query's values. An upload's bar is one: the
release's stored bytes, which a live query reads, plus, for each file being
sent, its size times the share of it that coven's transfer watch says has moved.
Its speed is measured from how fast those shares move.

A Rust watcher in the domain that owns the value computes it:

- The watcher waits on the live query and the in-memory source (for the upload
  bar, coven's transfer watch) together.
- Each time the query reruns or the source changes, the watcher computes the
  value from the query's latest result and the source's latest value, and sends
  it on the fast stream.
- A speed is a measurement of the source's changes over time, so the watcher
  keeps those changes for as long as the measurement needs them.

The watcher keeps no other memory between reads:

- No value kept from an earlier read to cover the time between a transfer ending
  and the query reading its file as stored.
- No copy of the values it last sent, kept so it sends only the ones that
  changed.

### The apps draw only the rows Rust hands them

The apps (Swift and Kotlin) draw each row as Rust hands it over. They never
filter, sort, group, count or sum those rows, and never answer from them
("Answers from the loaded rows" below).

- A length or emptiness test is a count.
- When a screen needs a value from the rows, SQL or Rust's `process` function
  computes it and Rust hands it over in the bridge type.

### Long lists

Each window the screen shows is its own live query, bounded by a sort key at
both ends:

```sql
WHERE (sort) >= (:from) AND (sort) < (:until) ORDER BY sort
```

coven decides which writes rerun a query from its WHERE comparisons, not from
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

### Shapes that Rust and the apps never use

- **Memory between reads.** No counts, windows, positions or last results kept
  (in a `Mutex`, `OnceLock`, atomic, field or cache) for a later read to use.
  Every read starts from the database.
  - An incremental or lazy computation framework is memory between reads.
  - A fast value's watcher holds its query's latest result until the query
    reruns, and nothing else from it ("A fast value computed with a live query's
    values").
- **A side pass.** No second walk over the rows beside the live query to count
  them or note where groups start.
  - A walk that shapes one result for display is that query's `process`
    function, not a side pass.
- **A forced rerun.** No field written into a request, a table or a version only
  so a query reads again, and no write count kept in a table
  (`additions = additions + 1`) so a reader can tell a write happened. A query
  reruns because what it read changed.
- **A size cutoff.** No threshold (rows, matches, bytes) that sends a read down
  one of two paths.
- **A live query read once.** A read that takes the first value and stops is a
  plain SQL read.
- **Choosing or joining rows outside SQL.** No filtering, sorting or windowing
  that decides which rows a screen gets, no count across windows, and no second
  statement keyed by values the first one returned.
  - Rust may shape one result's rows for display in `process`.
  - The apps may not shape rows at all.
- **Answers from the loaded rows.** No test of whether an id is among the rows
  the screen holds, whether they are empty, or where a row sits among them, even
  for state the view owns (a drop target, a removal animation, a menu's
  targets). The loaded rows change with scrolling. SQL answers, or the view
  keeps the ids it acted on.

A plan or brief that proposes one of these shapes is wrong: raise it instead of
building it.

### What the pre-commit hook checks

bae's pre-commit hook refuses the shapes it can see in Rust read code (the
ownership check's `read-shapes` rule). The violations in the code today are
listed in `read_shapes.frozen` in `owner-construction-check.toml`, one row per
function and shape, with how many times it occurs there. The hook lets those
through, and fails a commit that adds a row or raises a count, so the list can
only shrink as the code is fixed. A frozen row is a violation not yet fixed,
never permission: a change to a frozen site that keeps the shape still breaks
this rule. Uses over values that are not a read's rows (a constant table, one
name's characters) go in `allowed` with what the values are.

The check counts a closure passed to a live query's `process`, and every
function it calls, as read code. So it also flags row work (such as grouping,
counting or `position`) in a function this rule allows there.

- Before you list such a finding, check the function against "Shaping one result
  in Rust": it takes only that one result, keeps nothing between runs, and only
  shapes how the rows display.
- List it in `allowed` with what the function shapes for display and which
  query's result it takes.
