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
import shlex
import sys

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


def segments(command: str):
    # Split on shell separators; a quoted separator only costs a missed check.
    for part in re.split(r"&&|\|\||[;|\n]|\$\(|`", command):
        try:
            tokens = shlex.split(part, comments=True)
        except ValueError:
            tokens = part.split()
        yield without_redirections(tokens)


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
        if not arg.startswith("-"):
            return False
    skip = False
    for arg in after:
        if skip:
            skip = False
            continue
        if arg in LIBTEST_VALUE_OPTIONS:
            skip = True
            continue
        if not arg.startswith("-"):
            return False
    return True


# Words that run the command after them: `bash scripts/check.sh`, `env X=1 ./check.sh`.
RUNNERS = {"bash", "sh", "zsh", "env", "exec", "time", "nice", "nohup", "caffeinate"}
# Runners whose first non-option argument is theirs, not the command:
# `lockf -k <file> <command>`, `timeout <duration> <command>`.
RUNNERS_WITH_ARGUMENT = {"lockf", "timeout"}


def runs_check_script(tokens) -> bool:
    """True when check.sh is the command a segment runs, not an argument to one
    (`git show HEAD:scripts/check.sh`, `grep deny scripts/check.sh` only read it)."""
    skip_argument = False
    for token in tokens:
        if token == "check.sh" or token.endswith("/check.sh"):
            return not skip_argument
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


def blocked(command: str) -> bool:
    for tokens in segments(command):
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
