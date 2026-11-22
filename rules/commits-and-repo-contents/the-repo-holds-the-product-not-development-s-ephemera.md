---
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

## The repo holds the product, not development's ephemera

Before you add a file or a hook step, ask whether it is the product or a
by-product of making it.

- The product is what the software is and needs on every build or run: its
  source, its assets, the tests of its behavior, the tools a build runs each
  time.
- A by-product helped make it once:
  - the script that cut a drawing into parts;
  - the probe that measured something;
  - the generator of a file that is now edited by hand;
  - a check that the generator could still make that file.

Keep what the by-product made, where the product uses it, and drop the
by-product. Never add a hook step to keep a by-product working
(`a-commit-hook-formats-and-statically-checks-what-changed`).
