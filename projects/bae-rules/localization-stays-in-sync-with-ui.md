---
digest: A change that adds, removes, or rewords user-facing text must update the string catalogs (key + translations) in the same change.
paths:
  - bae-macos/**/*.swift
  - bae-ios/**/*.swift
  - bae-android/**/*.kt
  - bae-windows/**/*.cs
  - bae-windows/**/*.xaml
  - bae-bridge/src/**/*.rs
  - bae-bridge/loc/catalog.toml
blocking: false
---

## Localization stays in sync with the UI

bae ships in 14 locales; every user-facing string lives in a catalog, not inline
in the code. A change that touches user-facing text must update the strings in
the **same** change — otherwise the new text is English-only, the stale text
misleads translators, and dead keys accumulate. Three cases, each with an
obligation:

- **Added text** — a new `Text(...)` / label / menu item / alert, or a new
  `core.*` message produced in bae-bridge — adds its catalog key. A `core.*` key
  goes in `bae-bridge/loc/catalog.toml` (plus the `bridge_*_key` producer if it
  is enum-mapped — the `loc_key_coverage` test fails otherwise); UI chrome goes
  in that platform's catalog (`Localizable.xcstrings` / `strings.xml` /
  `Resources.resw`). Translate it, or leave the non-source locales at state
  `new` with translation queued — but the key must exist.

- **Removed text** — deleting a screen, button, or message deletes its key
  across the catalog and all 13 locale slots. A key no code references is dead
  (see `yagni`); leave none behind — `no_orphan_core_keys` catches `core.*`
  orphans, Android `UnusedResources` catches chrome orphans.

- **Reworded text** — changing the English source updates the string **and**
  re-flags its locale slots for re-translation. A changed source with stale
  translations is worse than an untranslated one: it ships a confident
  mistranslation.

Applies to all four UIs and to bae-bridge message producers. The locale never
crosses the bridge (see `ui-and-bridge-thinness-letter-of-the-law`); this rule
keeps the catalogs the bridge keys into complete and current.
