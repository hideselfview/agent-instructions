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
- `generate.py` writes two gitignored files from `instructions-agent.md`:
  - `claude/CLAUDE.md` holds `instructions-agent.md` alone.
  - `codex/AGENTS.md` adds an index of the rules, built from each rule's
    `digest`.
  - `CLAUDE.md` and `AGENTS.md` at the repo root link to them, so Claude Code or
    Codex opened in this repo loads them.
- `rules/<group>/*.md` holds one code rule per file, in the folder of its review
  group (see below). Each file's frontmatter has these fields:
  - `digest`: the one line that goes into the Codex index.
  - `paths`: globs for the files the rule covers. Claude Code loads the rule's
    body when it reads a matching file.
  - `blocking`: whether the reviewer's prompt and the report mark the rule's
    findings blocking. Every finding, blocking or not, must be settled before
    `builds land` takes the commits.
  - `review: false` (optional): marks a rule a reviewer can't check, which
    `builds review` leaves out.
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
  - They refuse full local test suites, polling loops, and moves in an app
    checkout without its lock.
- `scripts/rules_review/matching_rules.py` lists the rule files whose `paths`
  match given repo paths.
- `.githooks/` holds this repo's git hooks.
  - `pre-commit` runs `builds checks run`, which runs the checks `.checks` lists
    on the staged files.
  - `post-commit` and `post-merge` run `install.sh` again in the main checkout
    when a change touches what it installs.

## Where a rule goes

Claude Code loads the rules itself. A file with `paths` frontmatter loads only
when Claude Code reads a matching file; a file with no `paths` loads at launch,
on every turn, like `CLAUDE.md`. So:

- Every `rules/` file has `paths`.
  - A language rule gets narrow globs (`**/*.rs`).
  - A design rule for all code gets the code globs (`**/*.rs`, `**/*.swift`,
    `**/*.kt`, `**/*.ts`, `**/*.tsx`), so it loads when an agent reads code but
    not during chat.
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
- runs `generate.py` and links `claude/CLAUDE.md` into `~/.claude/CLAUDE.md` and
  `codex/AGENTS.md` into `~/.codex/AGENTS.md`
- links `claude/settings.json` into `~/.claude/` and `codex/config.toml` into
  `~/.codex/`
  - It first moves an existing `~/.codex/config.toml` that is not a link aside.
- links each skill folder of `claude/skills/` into `~/.claude/skills/` and of
  `codex/skills/` into `~/.codex/skills/`
- links every rule file under `rules/` into `~/.claude/rules/`, directly and
  without its group folder (Claude Code has open bugs loading path-scoped rules
  from subfolders), and this repo into `~/.codex/agent-instructions`
- links each project's files, as the next section describes
- builds `builds` from the committed HEAD and runs `builds install`

It always links the main checkout, whichever worktree runs it.

## Per-project setup

`install.sh` links two things for each project whose folder (`~/dev/<name>`)
exists on this machine, and skips the rest:

- `projects/<name>.md` → `~/dev/<name>/CLAUDE.md` and `~/dev/<name>/AGENTS.md`
- `projects/<name>-rules/<group>/*.md` → `.claude/rules/` in `~/dev/<name>` and
  in each of its worktrees, without the group folder
  - A project's own post-checkout hook links them into a worktree it creates.
  - Claude Code loads these rules the same way as the ones in `rules/`, and
    `builds review` reviews against them too.

Gitignore `CLAUDE.md`, `AGENTS.md` and `.claude/rules/` in the project, because
they are links to this machine's files.

## Rules in Codex

Codex reads `AGENTS.md` but doesn't load rules by path. The generated
`codex/AGENTS.md` holds the rule index and tells Codex to list the matching rule
files before it edits:

```bash
python3 ~/.codex/agent-instructions/scripts/rules_review/matching_rules.py \
  --project <project> <repo-relative-path>...
```

The script prints the rule files whose `paths` match, including the project's
rules under `projects/<project>-rules/`. Codex reads each of them before it
edits.

## Code review

Every branch is reviewed against the rules before it lands, with one reviewer
per group of rules, one folder of `rules/` each. No diff is reviewed more than
once. Each change gets one review; the coordinator reads the fixes for its
confirmed findings. `builds land` refuses a commit that is neither read by a
finished review nor named as the fix of one of its confirmed findings. The
`code-rules-review` skill says how to run the review, settle its findings and
name their fixes, and `builds help` describes each review command.
