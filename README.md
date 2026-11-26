# agent-instructions

Personal coding standards for AI coding agents, written by
[@dminkovsky](https://github.com/dminkovsky). The repo holds the instructions,
rules, settings and skills that Claude Code and Codex load, and the `builds`
tool that agents run builds, landings and reviews through. `install.sh` links
them into `~/.claude/` and `~/.codex/`, so both tools read the same files.

## Layout

- `instructions-agent.md` holds the instructions every agent loads on every
  turn.
  - They cover how to design and judge work, how to find out what is true, how
    to talk with the user, and commits.
- Both `AGENTS.md` and `CLAUDE.md` link to `instructions-agent.md`.
- `rules/<group>/*.md` holds one code rule per file, in the folder of its review
  group. Each file's frontmatter gives `paths`, the globs of the files the rule
  covers, and the flags that `builds help LANDING AND REVIEW` explains:
  `blocking`, `review: false`, `design: true` and `capture: true`.
- `projects/<name>.md` holds a project's always-loaded instructions.
- `projects/<name>-rules/<group>/*.md` holds a project's rules, with the same
  frontmatter as `rules/`.
- `rules/<group>/group.toml` holds the settings of one review group for
  `builds review`, which `builds help LANDING AND REVIEW` explains with how each
  rule belongs to one group.
- `claude/settings.json` holds the Claude Code settings: environment variables,
  permissions, hooks and plugins.
  - Machine-specific settings go in `~/.claude/settings.local.json`, outside
    this repo.
- `claude/skills/` holds Claude Code's skills, one folder each.
- `claude/plugins/` is a local plugin marketplace that `claude/settings.json`
  registers.
  - Its rust-analyzer plugin builds into each repo's `target-ra` and checks
    nothing on save.
- `codex/config.toml` holds the Codex settings.
- `scripts/builds/` is the `builds` tool, a Rust program (`builds help`).
- `scripts/hooks/` holds the Claude Code hooks that `claude/settings.json` runs
  before each Bash command.
  - They refuse full local test suites, polling loops and other waits on a
    process or a time (such as `caffeinate -w`), and moves in an app checkout
    without its lock.
- `.githooks/` holds this repo's git hooks.
  - `pre-commit` runs `builds checks run`, which runs the checks `.checks` lists
    on the staged files.
  - `post-commit` and `post-merge` run `install.sh` again when a commit or merge
    on the main checkout's main branch touches what it installs. A commit on a
    side worktree's branch installs nothing.

## Where a rule goes

- Give each rule the `paths` of the files it covers. A Rust rule uses `**/*.rs`,
  for example.
- Agent behavior that applies on every turn (be terse, hold your position) goes
  in `instructions-agent.md`, not in `rules/`.
- A rule for one project goes in `projects/<name>-rules/`.

## Local setup

```bash
git clone git@github.com:hideselfview/agent-instructions.git ~/dev/agent-instructions
~/dev/agent-instructions/install.sh
```

`install.sh` does the following, and is safe to run again:

- installs mdformat if it is missing, and sets this repo's hooks folder to
  `.githooks`
- links `instructions-agent.md` into `~/.claude/CLAUDE.md` and
  `~/.codex/AGENTS.md`
- links `claude/settings.json` into `~/.claude/` and `codex/config.toml` into
  `~/.codex/`
  - It first moves an existing `~/.codex/config.toml` that is not a link aside.
- links each skill folder of `claude/skills/` into `~/.claude/skills/`
- links this repo into `~/.codex/agent-instructions` for Builds
- links each project's files, as the next section describes
- builds `builds` from the committed HEAD and runs `builds install`

It always links the main checkout, whichever worktree runs it.

## Making a change

Agents change this repo the way they change bae and coven: on a branch in a side
worktree, reviewed and landed through `builds`. The main checkout is the
person's, and it is the live copy that `install.sh` links, so a change goes live
once `builds land` moves the main checkout to it. The opening of `builds help`
says how, and `builds help workflow` gives the steps.

## Per-project setup

`install.sh` links `projects/<name>.md` to both `CLAUDE.md` and `AGENTS.md` in
`~/dev/<name>` when that checkout exists. Gitignore both links in the project.

## Briefs and code review

`builds help workflow DISPATCH` says what a brief carries: builds gives each
agent the coding rules and the workflow itself. Every branch is reviewed against
the rules before it lands, with one reviewer per group of rules
(`builds help workflow REVIEW`, and `builds help LANDING AND REVIEW` for each
review command).
