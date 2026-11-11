#!/usr/bin/env python3
"""PreToolUse hook on Bash: refuse full local test suites.

CI runs every suite on main. Locally an agent runs the pre-commit hook and the
tests for what it changed, so a cargo test or nextest run must name what it
runs: a test-name filter, or one integration-test file with --test. A run that
only compiles (--no-run) is fine. Running scripts/check.sh, which runs every
non-Windows CI gate in a row, is refused outright; reading it is not.
"""

import json
import re
import sys
from pathlib import Path

# The hooks run under `python3 -I`, which leaves the script's own folder off
# the import path.
sys.path.insert(0, str(Path(__file__).resolve().parent))
import shell_words  # noqa: E402

REASON = (
    "Full suites run in CI only: run the pre-commit hook and the tests for what "
    "you changed (a test-name filter, or --test <file>)."
)

# cargo options that take a value, so their value is not a test filter.
CARGO_VALUE_OPTIONS = {
    "-p", "--package", "--exclude", "-F", "--features", "--target", "--profile",
    "--manifest-path", "-j", "--jobs", "--color", "--message-format", "--config",
    "-Z", "--target-dir", "--bin", "--example", "--bench", "--lockfile-path",
    "-E", "--filterset", "--partition", "--retries", "--test-threads",
    "--cargo-profile", "--archive-file", "--workspace-remap", "--run-ignored",
}
# Libtest options after `--` that take a value.
LIBTEST_VALUE_OPTIONS = {
    "--skip", "--test-threads", "--logfile", "--color", "--format", "-Z",
    "--report-time", "--shuffle-seed",
}


def without_redirections(tokens):
    # `> log`, `2>&1`, `>>out`: a redirection's target is not a test filter.
    out, skip = [], False
    for token in tokens:
        if skip:
            skip = False
            continue
        if re.fullmatch(r"\d*(>>?|<)", token):
            skip = True
            continue
        if re.match(r"^\d*(>>?|<)", token):
            continue
        out.append(token)
    return out


SUBSTITUTION = re.compile(r"\$\(([^)]*)\)?|`([^`]*)`?")


def segments(command: str):
    # Split on shell separators outside quotes, so a `|` inside a grep pattern
    # is not a pipe. A command substitution, quoted or not, runs: its inside
    # is checked as a command of its own.
    for words, _ in shell_words.commands(command):
        current = []
        for word in words:
            for inner in SUBSTITUTION.finditer(word):
                yield from segments(inner.group(1) or inner.group(2) or "")
            current.append(SUBSTITUTION.sub(" ", word).strip() or word)
        yield without_redirections(current)


def full_cargo_run(tokens):
    """True when tokens are a cargo test / nextest run that names nothing."""
    words = [t for t in tokens if not re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", t)]
    try:
        # A repo's cargo wrapper (scripts/cargo.sh) runs cargo with its arguments.
        start = next(i for i, t in enumerate(words) if t.endswith(("cargo", "cargo.sh")))
    except StopIteration:
        return False
    rest = [t for t in words[start + 1:] if not t.startswith("+")]
    if rest[:1] == ["test"]:
        args = rest[1:]
    elif rest[:2] == ["nextest", "run"]:
        args = rest[2:]
    else:
        return False
    before, after = (args[: args.index("--")], args[args.index("--") + 1:]) if "--" in args else (args, [])
    if "--no-run" in before:
        return False
    if "--test" in before or any(a.startswith("--test=") for a in before):
        return False
    if "-E" in before or "--filterset" in before:
        return False
    skip = False
    for arg in before:
        if skip:
            skip = False
            continue
        if arg in CARGO_VALUE_OPTIONS:
            skip = True
            continue
        if names_something(arg):
            return False
    skip = False
    for arg in after:
        if skip:
            skip = False
            continue
        if arg in LIBTEST_VALUE_OPTIONS:
            skip = True
            continue
        if names_something(arg):
            return False
    return True


def names_something(arg: str) -> bool:
    """A test filter that narrows the run: it has a letter or digit. `::` or
    `_` matches every test's path, so it is a full run."""
    return not arg.startswith("-") and bool(re.search(r"[A-Za-z0-9]", arg))


# Words that run the command after them: `bash scripts/check.sh`, `env X=1 ./check.sh`.
RUNNERS = {"bash", "sh", "zsh", "env", "exec", "time", "nice", "nohup", "caffeinate"}
# Runners whose first non-option argument is theirs, not the command:
# `lockf -k <file> <command>`, `timeout <duration> <command>`.
RUNNERS_WITH_ARGUMENT = {"lockf", "timeout"}


def runs_check_script(tokens) -> bool:
    """True when check.sh is the command a segment runs, not an argument to one
    (`git show HEAD:scripts/check.sh`, `grep deny scripts/check.sh` only read it)."""
    skip_argument = False
    skip_lane = False
    tokens = list(tokens)
    for i, token in enumerate(tokens):
        # `builds lock <repo> --lane <lane> <command>`: the lane's name is not the command.
        if skip_lane:
            skip_lane = False
            continue
        if token == "--lane":
            skip_lane = True
            continue
        if token == "check.sh" or token.endswith("/check.sh"):
            return not skip_argument
        # `builds lock <repo> <command>` runs the command after the repo.
        if token == "lock" and i > 0 and tokens[i - 1].split("/")[-1] == "builds":
            skip_argument = True
            continue
        if token.split("/")[-1] == "builds" and tokens[i + 1:i + 2] == ["lock"]:
            continue
        if token in RUNNERS_WITH_ARGUMENT:
            skip_argument = True
            continue
        if token in RUNNERS or token.startswith("-") or re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", token):
            continue
        if skip_argument:
            skip_argument = False
            continue
        return False
    return False


QUOTED_HEREDOC = re.compile(r"""<<-?\s*(['"])(\w+)\1[^\n]*\n(.*?\n)?[ \t]*\2[ \t]*(?:\n|$)""", re.S)
SHELLS = ("bash", "sh", "zsh")


def without_data_heredocs(command: str) -> str:
    """Drop the body of a quoted heredoc (`<<'EOF'`): the shell expands nothing
    in it, so to any command but a shell running it as a script it is text (a
    Python script that names check.sh in a string runs nothing)."""

    def replace(match):
        line_start = command.rfind("\n", 0, match.start()) + 1
        words = command[line_start:match.start()].split()
        if any(w in SHELLS or w.endswith(tuple("/" + s for s in SHELLS)) for w in words):
            return match.group(0)
        return "\n"

    return QUOTED_HEREDOC.sub(replace, command)


def blocked(command: str) -> bool:
    for tokens in segments(without_data_heredocs(command)):
        if runs_check_script(tokens):
            return True
        if full_cargo_run(tokens):
            return True
    return False


def main():
    payload = json.load(sys.stdin)
    command = payload.get("tool_input", {}).get("command", "")
    if blocked(command):
        print(json.dumps({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": REASON,
            }
        }))


if __name__ == "__main__":
    main()
