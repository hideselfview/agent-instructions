---
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
may shape how that result's rows display. A long list keeps one window, a live
query that core moves and sizes around the rows on screen.

Two kinds of value reach a screen outside its query's rows:

- A value that changes many times a second travels on a keyed latest-value
  stream, as a channel would carry it inside Rust
  (`native-views-draw-fast-changing-values.md`).
  - The value lives in Rust's memory (playback position, transfer bytes, rates,
    levels), or a Rust watcher computes it from such a value and a live query's
    values ("A fast value computed with a live query's values").
  - Core never writes such a value to a table for a query to read: each such
    write makes coven rerun screen queries many times a second to deliver one
    number.
  - Core never puts a plain query result, with no fast-changing part, on a fast
    stream. Such a value reaches the screen through the screen's live query,
    however often it changes. When that query reruns too often, the fix is in
    the query or in coven.
- State that exists only on screen (hover, focus, the selected rows, an open
  popover or menu, a drag in progress, the text typed in a search or input
  field) stays in the view that controls it. The view owns the state: no table
  stores it, and no Rust code holds it. A selection lives in one place: if a
  screen ever needs a selection the view can't hold, such as select all over a
  list longer than the view loads, that screen keeps its whole selection in a
  table, never part of it in the view and part elsewhere. A query that depends
  on it takes it as a parameter in its request, such as a search's text in a
  WHERE filter, and an action on the selected rows gets their ids in the
  request.
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

SQL decides which rows a read returns and computes each value across a list from
stored rows. Each kind of work a read needs has one place:

- Filtering, sorting, joining, grouping, counting and ranking a list's rows:
  WHERE, ORDER BY, JOIN, GROUP BY, COUNT, a window function.
- A slow query: a better query or an index.
- A write computes each rule a read would otherwise compute (a decision, a
  resolution, a merge, a grouping or an order) once, with the rule's one copy,
  and stores the result in the same transaction.
  - Every write that changes one of the rule's inputs rewrites the results that
    input feeds.
  - A read selects, joins, filters, sorts and counts stored rows and never
    computes a rule again.

Start from the plain query over the domain's own tables. When that query is too
slow, add an index; when it would compute a rule, store the rule's result in the
write instead.

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
- Count a list's rows. SQL does, in the rows' query or a live query of its own.
- Keep state between runs, such as memory or a cache.
  - An incremental or lazy computation framework keeps results between runs to
    recompute only what changed. That is memory between reads, so bae never uses
    one.

### A fast value computed with a live query's values

A fast stream may carry a value that Rust computes from a value in its memory
that changes many times a second, such as transfer progress or playback
position, together with a live query's values. An upload's bar is one: the
release's stored bytes, which a live query reads, plus, for each file being
sent, its size times the share of its bytes that bae has sent so far, which
coven's transfer watch reports.

A Rust watcher in the domain that owns the value computes it:

- The watcher waits on the live query and the in-memory source (for the upload
  bar, coven's transfer watch) together.
- Each time the query reruns or the source changes, the watcher computes the
  value from the query's latest result and the source's latest value, and sends
  it on the fast stream.
- The watcher measures a speed, such as the upload bar's, from how fast the
  source's values change, and keeps those changes for as long as the measurement
  needs them.

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

A long list has one window, a live query of the rows from one row's sort values
on, at most a limit of them:

```sql
WHERE (sort) >= (:from) ORDER BY sort LIMIT :limit
```

coven reruns such a statement, once it returns as many rows as its LIMIT, only
for a write at or before its last row, when every condition holds (coven's
`spec/coven.md`, §4):

- The statement is one SELECT with an ORDER BY and a LIMIT, and has no OFFSET,
  no GROUP BY, and no aggregate or window function in its result columns.
- The statement returns each ORDER BY term as a result column before any `*`,
  spelled as the ORDER BY spells it.
- Each term reads one table, with no COLLATE and no NULLS FIRST or LAST, and is
  one of these:
  - a stored column whose collation is BINARY;
  - an expression over that table's stored BINARY columns, such as
    `year IS NULL` or `coalesce(year, 0) + 1`, that holds no subquery, no
    `rowid`, no clock and no function that isn't deterministic;
  - a generated column whose collation is BINARY and whose expression names only
    that table's stored BINARY columns.
- No table a term reads is the right side of a LEFT JOIN.

The terms can read more than one table. The ORDER BY's terms then fall into runs
of consecutive terms over one table:
`ORDER BY ar.sort_key, ar.id, al.title, al.id` has a run over `artists` and then
a run over `albums`. coven narrows the first run's table by that run's terms.
coven narrows a later run's table too only when coven reads that table after the
table of each earlier run and an equality with a column of that earlier table
narrows it, as `JOIN albums AS al ON al.artist_id = ar.id` does. When a
condition on the later table alone makes coven read that table first, only the
earlier runs' bound narrows the window.

Any other statement, such as one that orders by a result column's position, by a
NOCASE column or by an expression over two tables, or one with an OFFSET,
narrows by its WHERE alone: a write to any row the WHERE allows reruns the
statement, wherever that row sorts. So a list's window statement meets the
conditions wherever its order allows.

The app reports the first and last row on screen: when it opens the list, from
its own height and row height before any row arrives, and again each time the
person scrolls, resizes or jumps. Core sizes the window to the rows on screen
plus about one screen above and one below, and moves it when the rows on screen
come near its edge:

- The new window starts at the sort values of a row the window holds.
- A jump to a place the window holds no row of (a scrollbar drag, an end key)
  reads the sort values at that position once, with a plain read by offset.

The window's position is a live count of the rows before its first row. The app
draws each row at that position plus its place in the window, draws placeholders
outside the window, and keeps the top row on screen still when rows change above
it. The app never pages the list itself: it holds no chain of windows and reads
no key.

One read goes by offset instead of by sort values: a page that a platform asks
for by number, such as Android's media browser, which Android Auto and media
controllers use. The platform asks for page N, which starts at a fixed place in
the list, so bae reads that page with `LIMIT :size OFFSET :place`. A window that
starts at sort values cannot keep a numbered page at its place after a write
above the page moves its rows. coven reruns this read for every write its WHERE
allows, as it does any statement with an OFFSET (above). No other read of a
list's rows goes by offset, apart from the jump's one-time read of sort values
(above), and the app still never pages a list itself.

```text
subscribe_list_view(list, first: 0, last: 19)   -- the window and its position
set_visible(400, 419)                           -- core moves the window
subscribe_list_count(list)                      -- the count
```

### Shapes that Rust and the apps never use

- **Memory between reads.** No counts, windows, positions or last results kept
  (in a `Mutex`, `OnceLock`, atomic, field or cache) for a later read to use.
  Every read starts from the database.
  - An incremental or lazy computation framework is memory between reads.
  - A fast value's watcher holds its query's latest result until the query
    reruns, and nothing else from it ("A fast value computed with a live query's
    values").
  - A long list's view holds the latest result of its window's query and of its
    position's query until each reruns, and the rows on screen the app last
    reported, and nothing else ("Long lists").
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
  that decides which rows a screen gets, no count of a list's rows, and no
  second statement keyed by values the first one returned.
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

bae's pre-commit hook runs coven's typed lints in clippy. Their read-rule lints
(`clippy::read_sort`, `clippy::read_filter`, `clippy::read_query_per_row`,
`clippy::read_kept_state` and the rest, one for each shape) refuse the shapes
they can see in Rust read code. The `read_shapes` table of
`owner-construction-check.toml` tells the lints which types are reads, live
queries and kept state. Each finding bae keeps today carries an
`#[expect(clippy::read_<shape>, reason = "...")]` on the narrowest item that
takes one, a statement where it can. A mark whose reason starts with "kept
finding" is a violation not yet fixed, never permission: a change to that code
that keeps the shape still breaks this rule. A mark with any other reason says
why the values are not a read's rows. A mark that no longer covers a finding
fails the hook, so the mark goes when its code is fixed.

The lints find nothing in code that only a live query's `process` runs, over the
one result it passes. A function other crates can call is never such code, since
the compile cannot see their calls, so the lints flag row work there.

- Before you mark such a finding, check the function against "Shaping one result
  in Rust": it takes only that one result, keeps nothing between runs, and only
  shapes how the rows display.
- Mark it with what the function shapes for display and which query's result it
  takes.
