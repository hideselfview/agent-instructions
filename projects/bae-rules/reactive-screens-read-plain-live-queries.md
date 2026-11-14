---
digest: Everything a bae screen shows comes from SQL, with two exceptions, values that change many times a second (keyed latest-value streams), and state that lives only on screen, such as hover, focus, an open popover or menu, or a drag in progress (held by the view, never in SQL). A read that must follow changes is a plain coven live query, a one-off read a plain SQL read, never a live query read once; nothing is kept in Rust or app memory between reads, no side pass, no field or write count kept to force a rerun, no size cutoff between read paths, Rust and the apps never filter, sort, join (a second statement keyed by the first one's rows is a join), group, count or derive what a screen shows, and nothing answers from the rows a screen has loaded.
paths:
  - bae-store/src/**/*.rs
  - bae-views/src/**/*.rs
  - bae-import/src/**/*.rs
  - bae-library/src/**/*.rs
  - bae-bridge/src/**/*.rs
  - bae-transfers/src/**/*.rs
  - bae-playback/src/**/*.rs
  - bae-people/src/**/*.rs
  - bae-records/src/**/*.rs
  - bae-automation/src/**/*.rs
  - bae-mcp/src/**/*.rs
  - '**/*.sql'
  - BaeKit/**/*.swift
  - bae-macos/**/*.swift
  - bae-android/**/*.kt
blocking: true
---

## Reactive screens read plain live queries

Everything a screen shows comes from SQL, with two exceptions:

- a value that changes many times a second (playback position, transfer bytes,
  rates, levels) travels on a keyed latest-value stream
  (`high-frequency-values-skip-observed-ui-state.md`);
- state that lives only on screen, such as hover, focus, an open popover or
  menu, or a drag in progress, is held by the view itself and never goes into
  SQL.

A read that must follow changes, such as a reactive screen or a bridge,
automation or MCP subscriber, is one SQL query subscribed through coven
(`subscribe`), which reruns it when a write changes what it read. Everything a
screen shows beside its rows comes from SQL as well: counts, totals, groups and
section headers, "N of M", sums, facets, whatever accompanies the list. It is
part of the same query, or, when it costs more than the rows, its own live
query, and the screen draws each as it arrives. A read that need not follow
changes (a one-off lookup, a command's check, an export) is a plain SQL read,
not a live query. That is the whole read path.

A long list's window, its count and its section headers are separate live
queries. Each window the screen shows is its own live query whose request is
that window, so coven reruns only the window whose rows changed. The count is
never in the same query as the rows: a count changes with every insert, so a
query holding both rereads and resends every window on every write.

```text
-- Wrong: one query, count and windows together. A scan writes a row,
-- the count moves, every window is reread and resent.
subscribe(request = [window 0-99, window 100-199]) -> (count, rows, rows)

-- Right: each its own live query, each window bounded by a key at both ends.
subscribe(request = {from: null,      until: <row 100>}) -- rows 0-99
subscribe(request = {from: <row 100>, until: <row 200>}) -- rows 100-199
subscribe(SELECT COUNT(*) FROM ...)                       -- the count
-- A scan write reruns the count, and only the window the new row lands in.
```

Coven narrows which writes rerun a live query by the query's WHERE comparisons,
never by its LIMIT or OFFSET. So a window is a range of the order bounded by a
key at each end, `WHERE (sort) >= (:from) AND (sort) < (:until) ORDER BY sort`,
and reruns only when a row inside it changes. `WHERE (sort) > (:edge) LIMIT 100`
reruns for any change below it, and `LIMIT 100 OFFSET 200` for any change at
all. The screen holds its windows as a chain sharing bounds, so every row is in
exactly one; a jump reads the key at a position once and starts a chain there. A
screen shows two or three windows, so that is two or three small queries.

A window pages by cursor: the next or previous window starts from the sort
values of the row at the screen's edge, which the screen sends in the request,
and seeks through the order's index. A jump to a position (a scrollbar drag, an
end key) reads by offset, and gets an index of its own only when it measures
slow enough to warrant one. The cursor comes from the screen, never from rows
Rust kept from an earlier read.

Never, in either kind of read, in Rust or in the apps:

- **Memory kept between reads.** No places, counts, windows, last results or
  standings held in Rust or the app (behind a `Mutex`, a `OnceLock`, an atomic,
  a field, a cache across deliveries) for a later read to reuse. A read starts
  from the database every time.
- **A side pass.** No second walk over the rows beside the live query to count
  them, note where blocks start, or warm anything.
- **A forced rerun.** No field written into a request, a table or a version only
  so a live query reads again. A query reruns because what it read changed. The
  same goes for a count of writes kept in a table (`additions = additions + 1`)
  so a reader can tell that a write happened: the write's own answer says what
  it did.
- **A cutoff between read paths.** No threshold (rows, matches, size) that sends
  a read down one of two paths. One query, one path.
- **A live query read once.** No subscribing to take the first value and end the
  subscription: a read that need not follow changes is a plain SQL read.
- **Rust or the app doing SQL's work.** No filtering, sorting, joining,
  grouping, counting (a length or an emptiness test is a count), summing or
  deriving anything from rows a query returned. If a screen shows it, SQL
  produced it. A second statement keyed by a value the first one returned, or by
  keys picked out of returned rows, is a join done in Rust: one statement reads
  both.
- **Answers from the rows a screen has loaded.** No test of whether an id is
  among the loaded rows, whether they are empty, or where a row sits among them,
  even when the answer only decides state the view owns (a drop's hold, a
  removal animation, a menu's targets). The loaded windows are the part of the
  list the screen happens to hold, so the answer changes with scrolling. SQL
  answers the question, or the view keeps the ids it acted on.

A row in the hook's `frozen` lists records a violation not yet fixed; it is
never permission. A change that touches a frozen site and keeps the shape still
breaks this rule.

SQLite is local: write the plain query first, the GROUP BY, COUNT, JOIN or
window function that says what the screen shows, read straight from the domain's
own tables. A table of pre-added counts or copied columns kept by triggers is
never the starting shape.

When a plain live query measures too slow, the fix is in SQL: the query, an
index, or a device-local table that triggers keep in the same write
(notes/screens-as- queries.md), and it is measured. A value with no table that
changes many times a second travels on a keyed latest-value stream, never in a
list query.

A plan or brief that proposes any of the forbidden shapes is wrong; raise it
instead of building it. Example: a search over a 500,000-row import list that
shows its top rows first does it with two live queries (the window, and the
count), not with a counting pass beside the live query that bumps a request
field to force a reread and keeps block places in memory.

bae's pre-commit hook refuses the shapes a static check can see in Rust. The
ownership check's `read-shapes` rule refuses a sort, filter, group, sum, count
or pick of rows in read code, a query run once for each row, memory kept between
reads, a column or request field counted up to rerun a query, and a comparison
against a fixed limit that picks a read path. Today's violations sit in
`read_shapes.frozen` in `owner-construction-check.toml`, keyed and counted
exactly; a check (`scripts/check-frozen-shrinks.py`) fails a new row or a higher
count, so the list only shrinks as the code is fixed. A use over values that are
not a read's rows (a constant table, one name's characters, the audio engine's
state) goes in `allowed` with what the values are. No check reads the apps'
Swift and Kotlin: a static check cannot tell a screen's rows from the app's own
state (windows, menus, a view's selection) or from code in an action or event
handler, and it pushed code into worse shapes. The landing review reads the
apps' filters, searches, counts, sorts and picks over a screen's rows, and
passes the same work over the app's own state and in handlers. The Rust check
does not see work after a value leaves read code (the bridge, services, watches
over delivered values) or calls through receivers of unknown type; work without
the named methods (`.len()` as a count, a loop that filters with `continue`);
several statements combined in Rust without a map; memory outside a live-query
holder; a counter Rust writes as a bound value; or a threshold with no constant.
Review still covers those.
