# forage

The Forage scraping DSL + runtime. A `.forage` recipe declaratively describes
how to fetch structured records from a site or API; the runtime executes recipes
against an HTTP engine or a real browser engine (wry-backed WebView). Ships with
a CLI, an LSP, and Forage Studio for interactive authoring.

## Greenfield — no migrations, no compat shims

Pre-1.0 — break things and move on. When you change the canonical definition of
anything (schema, AST, IR, binary format, capture/replay fixtures, hub-api KV
entries, LSP wire messages), edit the definition and update every caller in one
PR. No migration shims, no dual-shape compatibility flags, no
`#[serde(default)]` to silently absorb renamed fields. If a fixture is stale,
regenerate it.
