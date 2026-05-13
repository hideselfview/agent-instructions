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

## Local setup (Claude Code)

```bash
git clone git@github.com:hideselfview/agent-instructions.git ~/agent-instructions
ln -sf ~/agent-instructions/CLAUDE.md ~/.claude/CLAUDE.md
ln -sf ~/agent-instructions/rules/rust-patterns.md ~/.claude/rules/rust-patterns.md
ln -sf ~/agent-instructions/rules/swiftui.md ~/.claude/rules/swiftui.md
```

Edits in either location reflect everywhere.

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
