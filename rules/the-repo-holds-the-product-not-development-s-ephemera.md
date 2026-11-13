---
digest: The repo holds the product; anything development produces that could be recreated in an instant (a script, a check, a generated copy, scaffolding, a probe) stays out of it and its hooks, and a hook never runs one.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
  - '**/*.js'
  - '**/*.py'
  - '**/*.sh'
  - '**/hooks/**'
  - '**/*.toml'
  - '**/*.yml'
blocking: false
---

**The repo holds the product, not development's ephemera.** Anything development
produces that could be recreated in an instant (a script, a check, a generated
copy, scaffolding, a probe) stays out of the repo and its hooks. Before adding a
file or a hook step, ask whether it is the product or a by-product of making it.

The product is what the software is and needs on every build or run: its source,
its assets, the tests of its behaviour, the tools a build runs each time. A
by-product is what helped make it once: the script that cut a drawing into
parts, the probe that measured something, the generator of a file that is now
edited by hand, a check that the generator could still regenerate it. Keep what
the by-product made, where the product uses it; drop the by-product.

**What a hook may run** is its own rule: formatting and static analysis of what
changed, within 45 seconds
(`a-commit-hook-formats-and-statically-checks-what-changed.md`). Never add a
hook step to keep a by-product working.

Example (bae, 2026-11-13): the animated sheep's parts were cut from the logo by
`design/system/split-sheep.py`, and a hook step checked on every commit that the
script still reproduced them. The parts are drawings in the design system,
edited as drawings; the script and its check were by-products, and both went.
The person: "we need to do a better job of figuring out what is ephemera --
products of development that we can in an instant recreate -- and what is NOT,
what is the product."
