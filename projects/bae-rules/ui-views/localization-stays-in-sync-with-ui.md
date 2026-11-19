---
digest: A change that adds, removes or rewords user-facing text updates the string catalogs (key and every locale's translation) in the same change. Core formats no text for a locale, and the one locale value core holds is the language the app sets for ordering names. A translation says what the English says the way that language's apps say it, never in the English sentence's structure.
paths:
  - bae-macos/**/*.swift
  - bae-ios/**/*.swift
  - BaeKit/**/*.swift
  - bae-android/**/*.kt
  - bae-bridge/src/**/*.rs
  - bae-bridge/loc/catalog.toml
  - '**/*.xcstrings'
  - bae-android/**/strings.xml
blocking: false
---

## Localization stays in sync with the UI

bae ships the locales in bae-loc's `TARGET_LOCALES` (`loc-gen locales` prints
them), and every user-facing string is in a catalog, never inline in the code. A
change that adds, removes or rewords user-facing text updates the catalogs in
the same change. A change without its catalog update is incomplete, and a
finding about it is fixed, never deferred.

Core formats no text for a locale. bae-bridge sends the apps raw data (numbers,
typed enums and stable message keys), never prose or text formatted for a
locale. Each app formats numbers, byte counts, durations and dates with its
platform's locale formatters, and resolves each message key through its string
catalogs. The one locale value core holds is the language the app sets for
ordering names (bae's spec, §2.1): the app passes it to core, and SQL orders
names by that language's collation keys.

Each kind of change has its own catalog work:

- **Added text** adds its catalog key.
  - Added text is a new `Text(...)`, label, menu item or alert, or a new
    `core.*` message that bae-bridge produces.
  - A `core.*` key goes in `bae-bridge/loc/catalog.toml`. When an enum maps to
    it, its `bridge_*_key` producer maps it too, or the `loc_key_coverage` test
    fails.
  - UI text goes in the catalog of the module that shows it: an `.xcstrings`
    file on macOS (the app's or a BaeKit module's), `strings.xml` on Android.
  - Translate it into every locale in the same change. Never copy the English
    into another locale, write placeholder text, or leave a string marked new,
    untranslated or fuzzy for later.
- **Removed text** removes its key from the catalog and every locale.
  - A key no code uses is dead
    (`rules/necessary-code/write-only-what-is-needed-today.md`).
  - `no_orphan_core_keys` catches an orphaned `core.*` key, and the hook's
    `loc-orphans` check (`scripts/loc-chrome-orphans.py`) catches an orphaned
    key in a macOS or Android catalog.
- **Reworded text** changes the English and retranslates every locale.
  - A changed English string with old translations is worse than an untranslated
    one: the app shows a wrong translation with no sign that it is wrong.

### Each translation follows its own language

A translation says what the English says, the way that language's apps normally
say it, never in the English sentence's structure. The plain-writing rule's
grammar forms, such as a named subject or a button that starts with its verb,
are English's. Every language keeps plain words, no figures of speech, no
marketing words and no emoji, and translates each of bae's own terms the same
way every time. Review never flags a translation for not following the English
sentence's structure.
