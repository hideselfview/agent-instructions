---
digest: Every sentence a person reads is plain, literal and active - who does what, in the words for what it does, starting with the verb for a title or task; no figures of speech, borrowed metaphors, coinages, headline grammar or passive voice.
paths:
  - '**/*.md'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.xml'
  - '**/*.xcstrings'
  - '**/*.strings'
  - '**/*.html'
  - '**/*.tsx'
  - '**/*.ts'
  - '**/*.axaml'
  - '**/*.cs'
  - '**/*.rs'
  - design/**
blocking: true
---

## Write plain, literal, active sentences

This is the first rule for any text a person reads: app strings, notifications,
errors, item and plan titles, commit messages, PR descriptions, docs, comments,
and replies in chat. It outranks every other writing rule.

**Active.** Name the actor and the action: "bae uploads the cover", not "the
cover is uploaded"; "Ana removed this Mac", not "this Mac was removed". A title,
task or button starts with its verb: "Rebuild the dashboard to match its board",
"Scan device…".

**Literal.** Use the words for what happens. Figures of speech make the reader
translate: "reads like", "lands", "short-circuits", "hydrate", "the flywheel".
So do metaphors borrowed from another field ("signed" for an ID that isn't
signed, "fan-out"), made-up coinages ("wire-printed") and pattern names used as
nouns ("the strategy", "the visitor"). Write what the thing does instead.

**Plain.** Pick the simplest word that does the job. Spell out any abbreviation
the reader hasn't met. A word that needs a parenthetical to explain it should be
replaced by the parenthetical.

**Whole sentences.** Keep the articles and verbs. Headline grammar ("Dashboard
reads like its new board", "Key wait names devices holding it") saves a word and
costs the reader the meaning.

**Name the subject; don't open with "It".** A sentence that starts with "It"
pointing back at the last one ("This Mac is offline. It holds your changes…")
reads as machine-written. Name the thing, or start from what the reader cares
about: "This Mac is offline. Your changes stay here until it reconnects." The
same goes for "This" and "That" standing alone as a subject.

Examples:

- ✗ "Dashboard reads like its new board" → ✓ "Rebuild the dashboard to match its
  board"
- ✗ "Your changes are kept" → ✓ "bae keeps your changes on this Mac"
- ✗ "Tier 1 signed IDs short-circuit to hydrate" → ✓ "Exact-match IDs skip the
  search and look up the release directly by ID"
- ✗ "the planner short-circuits" → ✓ "the planner stops at the first match and
  skips the rest"

Before you send or commit text, read each sentence and ask: who does what, and
would someone who doesn't know the code understand it the first time? If not,
rewrite it.

Example (2026-11-13): an item titled "Dashboard reads like its new board" left
the person asking "what does this mean"; "Rebuild the dashboard to match its
board" was "so much simpler to understand". The person: "the most important
thing about how you output human language, both to me and in strings: plain,
non-figurative language. active not passive."
