---
digest: A bug fix's diff is the change for the confirmed cause and nothing else — delete investigation logs and any mechanism built for a hypothesis the evidence later disproved.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
blocking: false
---

**Ship the fix, not the investigation.** A bug fix's diff contains exactly one
thing: the change that addresses the *confirmed* root cause. Debugging
accumulates code that is not that, and all of it comes back out before you
commit.

Two kinds of residue:

- **Instrumentation** — logs, `dbg!`/print, counters, timing, scratch asserts, a
  temporary `if id == "the-one" { … }` — added to *observe*. Delete it. A
  genuinely useful log can stay, but through the project's structured logger at
  the right level, not as a leftover probe (see
  `use-the-project-s-logger-for-real-logs`).
- **Speculative fixes** — a mechanism, type, guard, cache, or flag you built to
  fix a cause you *hypothesized*. When the evidence later confirms a *different*
  cause, that mechanism fixed nothing. Delete it too. This is the missed one: it
  compiles, it has tests, it looks like real work, so it rides along into the
  commit next to the change that actually fixed the bug.

**The test for every hunk:** name the evidence that makes it necessary. If a
hunk only addresses a hypothesis you disproved — or one you never confirmed — it
is investigation residue, not fix. A mechanism built for a problem that turned
out not to exist drives no decision that matters
(`a-mechanism-must-be-load-bearing`) and is dead on arrival (`yagni`).

**"But it's still correct / more robust / harmless to keep" is the trap.**
Correctness is not the bar; *necessity for the confirmed cause* is. A correct
mechanism for a non-existent problem is still speculative code that every future
reader must carry and explain. If it is a real, separate improvement, it earns
its own change with its own justification — never a silent passenger on the fix
that disproved its premise.

The receipt of a finished fix: the diff reads as the smallest change that makes
the confirmed failure stop, and you can point to the evidence behind each line.
This is `question-necessity-before-any-change` run once more at commit time,
against your own debugging detritus.
