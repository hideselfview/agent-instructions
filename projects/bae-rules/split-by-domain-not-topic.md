---
digest: When bae code, SQL or tests are split, the cut follows a domain (what owns a set of data and the rules that keep it right), never a loose topic; a domain's tables, triggers, code and tests live together.
paths:
  - '**/*.rs'
  - '**/*.sql'
blocking: false
---

**Split by domain, not by topic.**

A *domain* owns a set of data and the rules that keep it right. Its writes go
only through it; its tables, triggers, code and tests live together, beside each
other. A *topic* is a word several domains share ("covers", "progress", "CUE",
"SQL"): grouping by it puts code from different owners in one file, and the
reader can no longer tell who keeps which data right.

When a file is split, or tests are placed, or schema moves, cut along a domain.

**A domain in bae:** the import list. It owns `import_list_placement` and the
triggers that keep each row's place, the placement contract in
`bae-import/src/tables/import_list.rs`, the list's reads, and the tests of all
of it. Its schema and trigger tests sit beside it, not in a shared schema file
(checker-policy Decision 12).

**A topic, not a domain:** "covers". A release's stored cover, the covers an
import candidate offers, and the images a catalog answer lists belong to
different domains with different rules. One `covers.rs` holding all three would
mix their owners. Each goes with its own domain.

Same idea for tables: each table has one writer domain; other domains read it.
