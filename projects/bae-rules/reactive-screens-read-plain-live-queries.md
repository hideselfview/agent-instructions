---
digest: Everything a bae screen shows comes from SQL, with two exceptions, values that change many times a second (keyed latest-value streams), and state that lives only on screen, such as hover, focus, an open popover or menu, or a drag in progress (held by the view, never in SQL). A read that must follow changes is a plain coven live query, a one-off read a plain SQL read; nothing is kept in Rust memory between reads, no side pass, no field written to force a rerun, no size cutoff between read paths, and Rust never filters, sorts, joins, groups, counts or derives what a screen shows.
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

Never, in either kind of read:

- **Memory kept between reads.** No places, counts, windows, last results or
  standings held in Rust (behind a `Mutex`, a `OnceLock`, an atomic, a field)
  for a later read to reuse. A read starts from the database every time.
- **A side pass.** No second walk over the rows beside the live query to count
  them, note where blocks start, or warm anything.
- **A forced rerun.** No field written into a request, a table or a version only
  so a live query reads again. A query reruns because what it read changed.
- **A cutoff between read paths.** No threshold (rows, matches, size) that sends
  a read down one of two paths. One query, one path.
- **Rust doing SQL's work.** No filtering, sorting, joining, grouping, counting,
  summing or deriving anything from rows a query returned. If a screen shows it,
  SQL produced it.

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
