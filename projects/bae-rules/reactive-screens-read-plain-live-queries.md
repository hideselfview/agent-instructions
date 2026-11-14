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

When a live query is too slow, the fix is in SQL: the query, an index, or a
device-local table that triggers keep in the same write (notes/screens-as-
queries.md), and it is measured. A value with no table that changes many times a
second travels on a keyed latest-value stream, never in a list query.

A plan or brief that proposes any of the forbidden shapes is wrong; raise it
instead of building it. Example: a search over a 500,000-row import list that
shows its top rows first does it with two live queries (the window, and the
count), not with a counting pass beside the live query that bumps a request
field to force a reread and keeps block places in memory.
