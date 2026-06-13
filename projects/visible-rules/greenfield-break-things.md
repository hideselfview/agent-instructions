---
digest: Pre-1.0; rm -rf ~/.visible is the migration strategy. Change a canonical shape and update every caller in one PR — no migration shims or dual-shape compatibility.
paths:
  - '**/*'
blocking: false
---

## Greenfield — break things and move on

Pre-1.0. `rm -rf ~/.visible` is the migration strategy. When the canonical shape
of anything changes — the `nodes` schema, bridge types, on-disk image layout —
edit the definition and update every caller in the same PR. Flag as a violation:
migration shims, dual-shape compatibility flags, `#[serde(default)]` added only
to absorb a rename, fallback decoders for old data, or code that branches to
keep reading a shape the change is replacing. A stale fixture is regenerated,
not kept. Forward compatibility with old on-disk data is a non-goal until 1.0.
