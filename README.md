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
  group (see below). Each file's frontmatter has these fields:
  - `paths`: globs for the files the rule covers. Builds uses them when it
    prepares briefs and runs reviews.
  - `blocking`: whether the reviewer's prompt and the report mark the rule's
    findings blocking. Every finding, blocking or not, must be settled before
    `builds land` takes the commits.
  - `review: false` (optional): marks a rule a reviewer can't check, which
    `builds review` leaves out.
  - `design: true` (optional): marks a rule about how a screen, board or
    component looks. `builds review` checks a project's design files (its
    boards, design system and canvas.json) only against these rules, and against
    no other rule, whatever its `paths` match.
  - `capture: true` (optional): marks a rule about screenshot capture files.
    `builds review` checks a repo's capture files (the files below each
    `shots-declared` folder of its `.app-build`) only against these rules, and
    against no other rule, whatever its `paths` match.
- `projects/<name>.md` holds a project's always-loaded instructions.
- `projects/<name>-rules/<group>/*.md` holds a project's rules, with the same
  frontmatter as `rules/`.
- `rules/<group>/group.toml` holds the settings of one review group for
  `builds review`. Every rule, general or project, is in one group: its group is
  the folder it sits in, and a project-rules folder must have the name of a
  group folder under `rules/`. `builds help` (LANDING AND REVIEW) explains the
  fields.
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
- `codex/skills/` holds Codex's skills.
  - Its `code-rules-review` is a symlink to the Claude Code copy.
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
- links each skill folder of `claude/skills/` into `~/.claude/skills/` and of
  `codex/skills/` into `~/.codex/skills/`
- links this repo into `~/.codex/agent-instructions` for Builds
- links each project's files, as the next section describes
- builds `builds` from the committed HEAD and runs `builds install`

It always links the main checkout, whichever worktree runs it.

## Making a change

Agents change this repo the way they change bae and coven
(`builds help workflow`): each change is a branch in a side worktree that
`builds new ~/dev/agent-instructions --for <who> --project builds` hands out
(`--project bae` for bae's rules and project file), reviewed with
`builds review` and landed with `builds land`. No agent commits in the main
checkout, which is the person's.

The main checkout is also the live copy that `install.sh` links, so nothing goes
live until it is on main there. After each landing, `builds land` moves the main
checkout's main to the landed commits with
`git -C ~/dev/agent-instructions merge --ff-only origin/main`, as it does for
every repo's main checkout, and the checkout's post-merge hook runs
`install.sh`, which installs the new `builds` and links the shared instructions
and skills. When the main checkout is not on main, or the person's commits or
uncommitted edits there stop the fast-forward, `builds land` prints a
`MAIN CHECKOUT BEHIND` line with the command that the person runs after they
check out main or commit, push or discard those changes.

## Per-project setup

`install.sh` links `projects/<name>.md` to both `CLAUDE.md` and `AGENTS.md` in
`~/dev/<name>` when that checkout exists. Gitignore both links in the project.

## Rules and workflow stay out of briefs

Send the worker the brief's path. builds gives the worker each coding rule when
it first edits a file the rule covers, and the workflow when it runs
`builds ack`. `builds item take` refuses a brief that pastes either.
`builds brief [--project P] PATH...` prints the rules for repo-relative paths.

## Code review

Every branch is reviewed against the rules before it lands, with one reviewer
per group of rules, one folder of `rules/` each. No diff is reviewed more than
once. Each change gets one review, whose later runs read the fixes. The branch's
agent runs the review and lands the branch, and a judge session settles the
findings. `builds land` refuses a branch until its review has read the branch as
it is and no confirmed finding is open. The `code-rules-review` skill says how
to run the review, wait for the settles and land, and `builds help` describes
each review command.
