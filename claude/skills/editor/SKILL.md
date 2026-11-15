---
name: editor
description: Edit a document the way an editor does. Cut what doesn't belong, agree the outline with the person, order the document from general to specific under section headers, then make every sentence plain, literal and active. Use when asked to edit, rewrite, tighten, restructure or clean up a document, a rule, a note, instructions or a README.
---

# Editor

An editor cuts first, then agrees the outline with the person, then orders the
document from general to specific, then fixes each sentence. After each pass,
read the whole draft again: every pass turns up something the last one missed.

## Cut

Cutting is the editor's first job. For every paragraph, ask whether this reader
needs it here. Cut a paragraph when:

- Another document already holds the fact (a tool's help, a principle file, a
  rule). Replace it with one pointer to that document. Before you cut, read the
  target and check that it covers every fact you cut; it may also be more
  current than the text you are editing.
- Two documents state the same thing. They drift apart: keep it in one place and
  point to it from the other.
- It explains how a tool works inside, when the reader only needs to know what
  to run.
- It gives a reason that is true but isn't why the rule exists.
- It quotes someone to repeat the sentence before it ("(the person, 2026-11-13:
  '...')"). Keep a quote only when it adds something.
- It says what the reader would assume anyway ("comes from SQL" when all data is
  in SQL). Say what is specific, or cut it.

Also cut a long list that ends in "and the rest", "and so on" or "etc.": give
one or two examples ("such as X and Y") or the whole list.

Keep what this reader can't get any other way, even when it repeats another
document. A rule that loads only when an agent edits matching files never
reaches an agent writing a plan, so the plan-writer's instructions keep the
rule's principle and a pointer to it.

Moving a fact is not cutting it. Tell the person which facts you cut and where
the reader finds them now.

## Agree the outline

Before rewriting any prose, show the person the outline: the sections in order,
with one line each on what goes in. Rewrite only after they agree.

## Order from general to specific

The document opens with the general picture of its subject: what it is and how
its main parts fit together. That picture is the overview. Don't add a list of
the document's sections ("It starts with X, then Y"): that describes the
document, not the subject, and the headers already show it.

Each section opens with its general point, then the specifics. Every concept,
tool, role or file the document names must be introduced before or where it is
first used: "the checker's `database-boundary` rule" means nothing until the
document has said what the checker is and what runs it. Read the draft as a
reader who knows only what it has told them so far, and stop at each name it
hasn't explained. Define a word the project uses in its own sense ("lands") the
first time it appears.

Explain a name in one of three ways:

- Introduce it in the main text when it comes up more than once, when the reader
  has to act on it, or when later sections depend on it.
- Explain it in a footnote when the reader meets it once and needs it only to
  follow that sentence, and explaining it inline would break the sentence's
  point.
- Point to another document when that document already explains it.

Section headers give the structure. A header names what the section holds
("Crates and apps"), never a vague stand-in for it ("The parts", "Data").

## Break long bullets into sub-bullets

This applies only to text that is already a list; don't turn prose into lists.
Inside a list, don't shy away from sub-bullets: look for every chance to turn a
long bullet into a short bullet with sub-bullets.

- Keep a bullet's first line to the one statement that names its thing ("bae-ios
  is paused until macOS and Android are done"), and put the details in
  sub-bullets under it.
- When a bullet names several items, each with its own detail, give each item
  its own sub-bullet.
- Give each thing of the same kind its own bullet at the same level. Four apps
  are four bullets, not one bullet about two apps with the other two nested
  under it.
- Nest a bullet only under the thing it belongs to. A sub-bullet says something
  about its parent: the iOS app's rules go under the iOS app.
- Introduce a list with a sentence ("bae has four apps:"), never with a label
  ("The apps:").

## Fix each sentence

Every sentence is plain, literal and active
(`rules/write-plain-literal-active-sentences.md`). Check each one for these:

- Read every verb and ask whether it says literally what happens. Verbs of place
  and body (sits, lives, holds, lands, draws, frees) are the usual figures of
  speech: "bae-app sits over bae-library" becomes "bae-app uses bae-library",
  and "keep the build warm" becomes "keep it built". "Point at" and "aim at" are
  figures too: "point the agent at the file" becomes "give the agent the file's
  path".
- Cull "The X" labels hard: a header or bold label made of "the" and a noun
  ("The parts", "**The keychain.**") stands where a name or a statement belongs.
  Name what the section holds, or start the list item with its sentence. A bold
  label that only repeats the first words of its sentence ("**Sparkle.** Sparkle
  is...") goes too.
- Give each sentence one idea, but don't split by rule: one sentence with a list
  of parallel items reads better than four sentences in a row that start
  "Don't".
- Don't announce what comes next ("I'll make three points", "Here's what you
  need to know", "There are two exceptions."): say it. This is the sentence form
  of the section list the overview leaves out.
- Cut hype: dramatic stakes, grand framing, or importance the facts don't carry
  ("this changes everything", "the heart of bae", "a critical safeguard"). Say
  what the thing does and let the reader judge its weight.
- Eradicate clefts. A cleft splits a plain sentence in two to stage it: "What a
  screen shows is what its query returns", "What matters is X", "It is X that
  Y". Write the plain sentence: "A screen shows its query's rows."
- Use one term for each thing; don't rotate synonyms.
- Match the wording to the strength of the rule: "must" for a rule, never "can".
- Don't let an example suggest it is the only case. When many things work the
  same way, state the general rule and give the example as one of them.
- Keep every fact the source states, unless you cut it on purpose.

## Work with the person

Show one change at a time and wait for their answer. Don't commit until they say
to.
