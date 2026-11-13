# Implementation briefs: the planner's instructions

Given to an orchestrator-model planner, followed by its items and output file.

You write implementation briefs for queued work so that Sonnet (as of 2026-11:
capable, follows a clear brief well) can carry each one out with no design
decision left to make. Change no code.

Read first: ~/dev/bae/plans/todo.md (§1 decision stack and §2 work queue; your
items are named below), ~/dev/bae/plans/decisions.md, the agent workflow and
model rule in ~/dev/agent-instructions/instructions-agent.md ("Plan with Opus,
execute with Sonnet"), and every rule file the matching-rules script lists for
the files your items touch:
`~/.codex/agent-instructions/scripts/rules_review/matching_rules.py --project bae <path>...`
(for coven, read ~/dev/coven-worktrees/side-1/AGENTS.md or CLAUDE.md and its
rules the same way).

Read code through git only, on current main:
`git -C ~/dev/bae-worktrees/side-7 fetch -q` then
`git -C ~/dev/bae-worktrees/side-7 grep <pattern> origin/main -- <paths>` /
`git -C ~/dev/bae-worktrees/side-7 show origin/main:<path>`; for coven use
~/dev/coven-worktrees/side-1 the same way. Never checkout, edit or build; never
touch ~/dev/bae or ~/dev/coven. Read every file an item cites completely, and
its callers; line numbers in the items may have moved, so find code by name.

For each item write one brief:

- **Title** and the §2 item it implements (quote its first words).
- **Now:** what the code does, with path:line evidence. If the item is already
  done or no longer applies, say so with evidence and write nothing else for it.
- **Change:** the shape: what changes and what must hold, with names where a
  name is a decision. Settle every design choice from the code and rules, and
  say which rule or fact settles it. Don't write the change file by file: the
  executor reads the code, and a brief that deep is the implementation twice.
- **Test first:** the failing test (file, name, what it asserts) for a bug; for
  a refactor, which existing tests prove it.
- **Checks:** the exact `scripts/cargo.sh` commands, the hook, and an app build
  in bae side-1 under the app lock if Swift/Kotlin changes (never UI test
  suites, iOS builds or the Android emulator).
- **Spec:** the spec sections (bae spec/bae.md, coven spec/\*.md) to edit in the
  same commit, or "none" with why.
- **Locales:** new user-visible strings go in every locale; say which.
- **Model:** Sonnet executes. Only a design too large for the coordinator to
  hold goes to Opus, which writes a plan for Sonnet; say why.
- **Order:** what it must come after (other items, running agents' work named in
  §2 Running, the domain moves).
- **Open question** (only if the code and rules can't settle it): the facts and
  the alternatives that add no special-case UI. Such an item goes no further
  until the person answers.

Group the briefs in commit order. Write them to the file named below. No size or
effort words (no "simple", "small", "quick", "just", time estimates). Plain
words, no jargon.

Report to me in a few lines: the path, how many briefs, which items are
done/stale, and the open questions (one line each).
