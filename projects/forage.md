# forage

The Forage scraping DSL + runtime. A `.forage` recipe declaratively describes
how to fetch structured records from a site or API; the runtime executes recipes
against an HTTP engine or a real browser engine (wry-backed WebView). Ships with
a CLI, an LSP, and Forage Studio for interactive authoring.

## Greenfield — no migrations, no compat shims

Pre-1.0 — break things and move on. When you change the canonical definition
of anything (schema, AST, IR, binary format, capture/replay fixtures, hub-api
KV entries, LSP wire messages), edit the definition and update every caller in
one PR. No migration shims, no dual-shape compatibility flags, no
`#[serde(default)]` to silently absorb renamed fields. If a fixture is stale,
regenerate it.

## Architectural invariants

- **Workspace-rooted.** A workspace is a directory marked by `forage.toml`.
  Recipes live at `<workspace>/<slug>/recipe.forage`; shared `type` / `enum`
  declarations live in header-less `*.forage` files at the workspace root.
  The CLI, the LSP, and Studio all `forage_core::workspace::discover` on the
  recipe's parent directory and build a `TypeCatalog` per recipe.
- **In-process daemon, per workspace.** `forage-daemon` owns the
  `daemon.sqlite` (runs + scheduled runs) and the output stores under
  `<workspace>/.forage/`. Studio embeds it as `Arc<Daemon>`; an
  out-of-process binary is a future drop-in against the same crate API.
- **Path-based Tauri surface.** Studio's file commands take workspace-
  relative paths (`load_file`, `save_file`, `list_workspace_files`). There
  is no flat-recipe-list API and no `activeSlug` in the frontend store —
  the active file is a path, and the slug is derived via `slugOf(path)`.
- **One source of truth per concern.** The daemon holds the canonical
  `Workspace`; Studio reads through `Daemon::workspace()` rather than
  caching a duplicate. Cross-boundary types are defined once in Rust with
  ts-rs export; the TS side imports them — no hand-maintained mirrors.

See `notes/forage-studio.md` for the Studio architecture in detail and
`plans/workspaces.md` for the original workspaces design.
