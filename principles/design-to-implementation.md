# Design to implementation

When a plan implements a design file:

- The plan carries the design's link and file name, and its block outline
  transcribed from the markup: nesting, order, sizes. Never a paraphrase; the
  outline proves the plan's author read the markup.
- The plan lists every change to the existing layout by name, so the implementer
  doesn't keep what the design replaced.
- The implementer and the reviewer open the design file itself before the plan's
  prose, and build and check against the markup.
- Done means a screenshot of the built view compared against the design's
  render, before review and before merge.

When drawing a design:

- Start from what the code does today, then draw the screen the way the product
  should work. Anything the code can't do yet, already queued or not, is drawn
  anyway.
- A note beside the design names each thing the code needs for it, with the file
  and line of what it does today.
