# agent-instructions

Personal coding standards and guidelines for AI coding agents, written by
[@dminkovsky](https://github.com/dminkovsky). Used by local agents (Claude Code
via `~/.claude/` symlinks and Codex via `~/.codex/` symlinks). Code review is
one manual pass by the agent holding the implementation context. Per-rule review
matrices are deprecated.

## Layout

- `instructions-agent.md` — always-on agent behavior: working style,
  communication style, writing style, agent workflow.
- `claude/CLAUDE.md` — generated, gitignored: `instructions-agent.md` only.
  Linked into `~/.claude/CLAUDE.md`.
- `codex/AGENTS.md` — generated, gitignored: `instructions-agent.md` plus a
  digest index built from `rules/*` frontmatter. Linked into
  `~/.codex/AGENTS.md`.
- `CLAUDE.md` / `AGENTS.md` — repo-level aliases to `claude/CLAUDE.md` and
  `codex/AGENTS.md`, so the repo can be opened directly by Claude Code or Codex.
- `rules/*.md` — one code rule per file. Frontmatter: `digest` (the one-liner
  stitched into `codex/AGENTS.md`), `paths` (globs scoping when the body loads
  and which files the rule applies to), `blocking` (whether a violation blocks
  the PR), and optional `review: false` for authoring-only rules excluded from
  the deprecated review runner. The body loads into agent context only when a
  matching file is read.
- `principles/*.md` — elaborated principles referenced from
  `instructions-agent.md`. Loaded on demand when an agent reaches for the
  longer-form treatment of a rule.
- `claude/settings.json` — portable Claude Code settings: env vars, permission
  default mode, enabled plugins, UI prefs. Linked into `~/.claude/`.
  Machine-specific overrides go in `~/.claude/settings.local.json` (not in this
  repo).
- `codex/config.toml` — portable Codex settings: model, permissions, TUI prefs,
  and feature flags. Linked into `~/.codex/config.toml`.
- `codex/skills/` — Codex skills linked into `~/.codex/skills/` by `install.sh`.
- `projects/<name>.md` — always-on project facts too narrow for user-level rules
  (e.g. a project's pre-1.0 development stance). Linked into a project's working
  tree via `install.sh`.
- `projects/<name>-rules/*.md` — project-specific path-scoped rules (same
  frontmatter as `rules/`), symlinked into the project's `.claude/rules/`.

## Where a rule goes

Claude Code loads `rules/*.md` natively: a file with `paths` frontmatter loads
only when a matching file is read; a file with **no** `paths` loads at launch,
always-on (same priority as `CLAUDE.md`). So:

- Every `rules/` file gets `paths`. Language rule → narrow globs (`**/*.rs`).
  Universal design rule → broad code globs (`**/*.rs`, `**/*.swift`, `**/*.kt`,
  `**/*.ts`, `**/*.tsx`) so it loads when you touch any code, but not during
  pure chat.
- Always-every-turn content (agent behavior: be terse, hold position, no
  marketing) is **not** a `rules/` file — it stays prose in
  `instructions-agent.md`, always-on by being part of each generated instruction
  file.

Rule of thumb: `rules/` = scoped rules (always have `paths`);
`instructions-agent.md` = always-on behavior; `projects/<name>-rules/` =
project-specific scoped rules.

## Local setup

```bash
git clone git@github.com:hideselfview/agent-instructions.git ~/dev/agent-instructions
~/dev/agent-instructions/install.sh
```

`install.sh` generates `claude/CLAUDE.md` and `codex/AGENTS.md`, links them into
`~/.claude/CLAUDE.md` and `~/.codex/AGENTS.md`, links `claude/settings.json`
into `~/.claude/`, and links every file under `rules/`, `principles/`, and
`claude/agents/` into `~/.claude/`. It also links `codex/config.toml` into
`~/.codex/config.toml`, this repo into `~/.codex/agent-instructions`, and
`codex/skills/` into `~/.codex/skills/`. It always links the main checkout,
whichever worktree runs it, and the repo's post-commit and post-merge hooks
re-run it in main when a commit, pull or landed branch changes what it consumes.

## Per-project setup

A project gets two things, both auto-linked by `install.sh` (projects whose
target dir doesn't exist on this machine are skipped):

- `projects/<name>.md` → `~/dev/<name>/CLAUDE.md` and `~/dev/<name>/AGENTS.md` —
  the always-on project facts.
- `projects/<name>-rules/*.md` → `~/dev/<name>/.claude/rules/` — path-scoped
  project rules. Same frontmatter as user-level `rules/` (`digest` / `paths` /
  `blocking` / optional `review`); Claude Code loads each body when a matching
  file is read, exactly like the user-level rules. They apply alongside `rules/`
  during authoring and manual review.

Gitignore both `CLAUDE.md`/`AGENTS.md` and `.claude/rules/` in the target
project so cloners don't inherit the machine-local symlinks.

## Codex Rule Loading

Codex reads `AGENTS.md`, but it does not have Claude Code's native path-scoped
rule loading. The generated `codex/AGENTS.md` carries the rule index and tells
Codex to list matching full rule bodies before editing files:

```bash
python3 ~/.codex/agent-instructions/scripts/rules_review/matching_rules.py \
  --project <project> <repo-relative-path>...
```

The script prints the full rule files whose frontmatter `paths` match the target
paths, including project rules under `projects/<project>-rules/`. Codex must
read those files before editing and apply them during its manual review. Listing
and reading rules does not launch reviewers or require model calls.

## Code review

The agent holding the implementation context reviews the diff itself in one pass
against the requirements and applicable rules. Fix confirmed findings and verify
the corrections with relevant checks. Do not dispatch one reviewer per rule or
run repeated review passes.

## Deprecated per-rule review workflows

The per-rule CI matrix, local Codex review matrix, and `code-rules-review` skill
are deprecated. They are not required before committing, opening a PR, or
merging. The former instruction to avoid reviewing rules together is withdrawn.

`scripts/rules_review/local_codex.py` and its supporting review code remain as
historical tooling, not part of the agent workflow. The skill carries a
deprecation notice instead of instructions to run the matrix.
`matching_rules.py` remains active for finding rule bodies to read; it does not
run an AI review.
