# visible

Home inventory. Organize the physical things in your house as a browsable tree —
your house at the root, then rooms, containers, and individual things, each with
a photo. Rust core (`visible-core`), platform UIs via uniffi (`visible-bridge` →
`visible-android`, `visible-ios`), local-first storage on coven (the same
end-to-end-encrypted SQLite sync layer bae uses), sharing with co-householders
to come.

Project rules are path-scoped atomic files in `projects/visible-rules/`. Claude
Code receives them through `.claude/rules/` symlinks. Codex receives the rule
index in `AGENTS.md`; before editing files in this project, list matching full
rule bodies with
`~/.codex/agent-instructions/scripts/rules_review/matching_rules.py --project visible <path>...`
and read every listed file. This file holds the always-on project facts.

## Architecture

Everything the user owns is a *node* in one tree: `id` (text primary key),
`parent_id` (NULL for the root house), `name`, `position` (sibling order),
`image_id` (NULL or a local image file), `created_at`, `_updated_at`. Children
of a node are its contents. coven owns the SQLite connection
(`coven::Database::open` returns the connection handle plus the `_updated_at`
stamper), the on-disk layout (`coven::LibraryDir`), and per-library config +
device id (`coven::config`). visible-core owns the node domain and image files;
visible-bridge translates core types to Swift/Kotlin and nothing else.

## Greenfield — break things and move on

Pre-1.0. `rm -rf ~/.visible` is the migration strategy. When the canonical shape
of anything changes — DB schema, bridge types, on-disk layout — edit the
definition and update every caller in one PR. No migration shims, no dual-shape
compatibility flags, no `#[serde(default)]` to absorb renames, no fallback
decoders for old data. Regenerate stale fixtures.

## Bridge types are defined in Rust, generated per language

The `Bridge*` records/enums live in `visible-bridge/src/types.rs`
(`uniffi::Record` / `uniffi::Enum`). The Swift/Kotlin equivalents are generated
at build and gitignored (`visible-bridge/swift-bindings/`,
`visible-bridge/kotlin-bindings/`, the copies in the app trees) — absent from
the repo and from review. To check a bridge type's existence/fields, read
`visible-bridge/src/types.rs`; never conclude from the (missing) generated file.

## iOS and Android are at parity

Every user-facing capability ships on both `visible-ios` and `visible-android`,
backed by the same `visible-bridge` calls. A feature added to one platform
without the other is incomplete.

## Building the bridge

- Android: `./visible-bridge/build-android.sh` cross-compiles for the Android
  NDK targets, generates the Kotlin bindings, and stages the `.so`s into
  `visible-android/app/src/main/jniLibs/`.
- iOS: `./visible-bridge/build-ios.sh` cross-compiles for device + simulator,
  generates the Swift bindings, and packages `VisibleBridgeFFI-ios.xcframework`.
