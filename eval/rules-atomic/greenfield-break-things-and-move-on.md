## Greenfield — break things and move on

Pre-1.0. `rm -rf ~/.bae` is the migration strategy. When the canonical shape of
anything changes — DB schema, bae-bridge types, UiEventBus event payloads,
on-disk file layouts, sync membership chain format, encryption schemes, cloud
storage paths — edit the definition and update every caller in one PR. No
migration shims, no dual-shape compatibility flags, no `#[serde(default)]` to
silently absorb renames, no fallback decoders for old data. If a fixture is
stale, regenerate it.
