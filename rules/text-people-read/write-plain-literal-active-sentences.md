---
digest: Every sentence a person reads, outside code comments and commit messages, is plain, literal and active: who does what, in the words for what it does, starting with the verb for a title or task; no figures of speech, borrowed metaphors, coinages, headline grammar, passive voice, dashes as punctuation or marketing adjectives (public website pages that aren't docs are exempt from the last). The grammar forms are English's; a translation follows the localization rule.
paths:
  - '**/*'
blocking: true
design: true
---

## Write plain, literal, active sentences

This is the first rule for any text a person reads: app strings, notifications,
errors, help text, item and plan titles, PR descriptions, docs, the spec,
briefs, rules, and replies in chat. This rule outranks every other writing rule.

**Comments and commit messages.** The rule does not cover code comments (doc and
line comments in a source file) or commit messages.

**Who checks it.** `builds review` judges no text on its word choice, grammar or
style: it never asks this rule or the other rules in its folder. The person
reviews copy, and may run the copy review (`builds review copy`), which asks
these rules. A review still judges whether text is accurate, under rules such as
committed-text-describes-the-current-state and
every-change-agrees-with-its-product-spec.

Short text (app strings, titles, buttons, messages) has no room to set up
context, so it follows the forms here. A document (prose a reader reads from the
top, such as a README, a rule or a brief) also follows the editor skill
(`claude/skills/editor/SKILL.md`), which asks a document to name its reader up
front, introduce the names it uses and give its sections headers. Where the two
give different forms below, each says which text it covers.

**English forms.** The grammar forms below, and the habits they forbid, are
English's: active voice with a named subject, a title or button that starts with
its verb, whole sentences with their articles, no sentence that opens with "It"
or "This", and no dash as punctuation. Other languages have their own forms. A
translation follows its project's localization rule, such as bae's
(`projects/bae-rules/text-people-read/localization-stays-in-sync-with-ui.md`).

**Active.** Name the actor and the action: "bae uploads the cover", not "the
cover is uploaded"; "Ana removed this Mac", not "this Mac was removed". A past
participle that leaves out who acted is passive too, whether the sentence tells
of an action or a state, and with or without a form of "be" before it: "the
release is matched", "tracks added by mistake" and "the key, saved on this Mac"
each hide who acted. Name the actor: "bae matched the release", "tracks you
added by mistake", "the key bae saved on this Mac". An item or plan title, a
task, a button or a menu item starts with its verb: "Rebuild the dashboard to
match its board", "Scan device…". A document's section header instead names what
the section holds ("Crates and apps").

**Literal.** Use the words for what happens. Figures of speech make the reader
translate: "reads like", "short-circuits", "hydrate", "the flywheel". So do
metaphors borrowed from another field ("signed" for an ID that isn't signed,
"fan-out"), made-up coinages ("wire-printed") and pattern names used as nouns
("the strategy", "the visitor"). Write what the thing does instead. A word the
project defines as the name of its own operation ("land" for `builds land`) is a
term, not a figure: a document introduces it once, then uses it.

**Plain.** Pick the simplest word that does the job. Spell out any abbreviation
the reader hasn't met. In short text, when a word needs a parenthetical to
explain it, use the parenthetical instead of the word. A document keeps a name
the reader meets again or acts on, and introduces it where it first appears.

**Whole sentences.** Keep the articles and verbs. Headline grammar ("Dashboard
reads like its new board", "Key wait names devices holding it") saves a word and
loses the meaning. Drop a "the" only when it points at nothing the reader knows:
"How to design and judge the work" becomes "How to design and judge work".

**Name the subject; don't open with "It".** A sentence that starts with "It"
pointing back at the last one ("This Mac is offline. It holds your changes…")
looks machine-written. Name the thing, or start from what the reader cares
about: "This Mac is offline. Your changes stay here until it reconnects." The
same goes for "This" and "That" standing alone as a subject.

**No dash as sentence punctuation.** No em dash (—), and no en dash or hyphen
with spaces around it ( – ), anywhere the rule covers: app text, help text and
docs. Write what the dash stood for: a pause or an aside is a comma, a second
thought is a new sentence, and a definition or a list that follows takes a
colon.

- ✗ "Fix the queue – it dropped held items" → ✓ "Fix the queue, which dropped
  held items"

**No marketing language.** Don't praise what you describe: no "improves user
experience", "seamless", "powerful", "elegant", "robust", "intuitive" or
"delightful". Say what it does.

- Public website pages that aren't docs are exempt: landing, pricing, feature
  and download pages, and their translations.
- Docs pages are covered, even when they are part of the website.

Examples:

- ✗ "Dashboard reads like its new board" → ✓ "Rebuild the dashboard to match its
  board"
- ✗ "Your changes are kept" → ✓ "bae keeps your changes on this Mac"
- ✗ "Tier 1 signed IDs short-circuit to hydrate" → ✓ "Exact-match IDs skip the
  search and look up the release directly by ID"
- ✗ "the planner short-circuits" → ✓ "the planner stops at the first match and
  skips the rest"

Before you send or commit text, read each sentence and ask: who does what, and
would its reader understand it the first time? If not, rewrite it. App text's
reader is the app's user, who knows nothing of the code. A document names its
reader up front. When a document's reader is an agent, that reader knows general
tools (sccache, SwiftPM) but not this project's own names.
