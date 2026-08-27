# Inventory style

State what exists and what it does. Nothing else.

Invoked by name: "write this in inventory style," "inventory style, please."
Applies to docs, notes, READMEs, and to prose in chat.

## Rules

- Present tense, declarative. Every line is a fact about the thing.
- Flat bullets and short paragraphs. The shortest sentence that carries the
  fact.
- No framing. No thesis, no argument, no "the core idea is," no "what this
  unlocks."
- No "X is not Y." Say what X is. The contrast is padding, and it is usually
  also wrong.
- No verdicts, recommendations, tensions, options, tradeoffs, or open questions
  — unless explicitly asked for. Those are a different document.
- No gaps, holes, risks, or next steps. Not asked for, not included.
- No marketing adjectives, no pitch vocabulary, no size or effort framing.
- Sections are categories of fact ("The model", "What it does"), never editorial
  ("Why it matters", "Where it leans").

## Example

```markdown
# visible

A tree of your stuff, with photos, shared with your household.

## The model

- One node type. House, room, shelf, box, thing — all the same row, placed by parent.
- A node: parent, optional name, optional photo, sibling position.
- Attributes on a node: quantity, notes, value, acquired date, serial, barcode, tags.

## What it does

- Capture is photo-first: the node exists untitled the moment you take the picture.
- Browse the tree as a photo grid.
- Rename, move, delete, re-photo, edit attributes, tag.
- Search: substring against name.
```

## The failure mode

The reflex is to explain, position, and evaluate — to tell the reader what the
thing means or what to do about it. Inventory style has no room for any of it.
If a line would survive being deleted without losing a fact, it was framing.
