# agent-instructions

Personal coding standards and guidelines for AI coding agents, written by
[@dminkovsky](https://github.com/dminkovsky). Used by both local agents
(Claude Code via `~/.claude/` symlinks) and CI agents (PR-review workflows
that check out this repo).

## Layout

- `CLAUDE.md` — main standards. Identity (`# You`), working style (anti-bias
  rules), engineering discipline, design discipline, communication style.
- `rules/*.md` — path-scoped rules. Each file has YAML frontmatter declaring
  which file patterns it applies to; the rule only loads into agent context
  when matching files are read.
- `projects/<name>.md` — project-specific guidance too narrow for user-level
  rules (e.g. a project's pre-1.0 development stance). Linked into a
  project's working tree via `link-project.sh`.

## Local setup (Claude Code)

```bash
git clone git@github.com:hideselfview/agent-instructions.git ~/dev/agent-instructions
~/dev/agent-instructions/install.sh
```

`install.sh` symlinks `CLAUDE.md` and every file under `rules/` into
`~/.claude/`. It's idempotent — re-run after pulling new rules.

## Per-project setup

For a project that has its own guidance file in `projects/`, link it into
the project's working tree:

```bash
~/dev/agent-instructions/link-project.sh <project-name> <target-dir>
# e.g.: link-project.sh forage ~/dev/forage
# creates <target-dir>/CLAUDE.md -> ~/dev/agent-instructions/projects/<project-name>.md
```

Re-run for each new checkout or worktree. The `CLAUDE.md` symlink should be
gitignored in the target project so cloners don't inherit the link.

## CI consumption (PR review)

Check this repo out alongside your project, then point the Claude Code
GitHub Action at `agent-instructions/CLAUDE.md` in the prompt:

```yaml
- uses: actions/checkout@v4
- uses: actions/checkout@v4
  with:
    repository: hideselfview/agent-instructions
    path: agent-instructions
- uses: anthropics/claude-code-action@v1
  with:
    anthropic_api_key: ${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }}
    prompt: |
      Review the PR diff against agent-instructions/CLAUDE.md (and any
      rule files in agent-instructions/rules/). Read those first, then
      review the diff. Only post comments for substantive violations.
```
