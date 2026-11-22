#!/usr/bin/env python3
"""PreToolUse hook on Bash: refuse a command that waits by polling or on a timer.

`until ! pgrep -f "<pattern>"; do sleep 5; done` never ends: the loop's own
command line holds the pattern, so pgrep always finds it. With other agents
building, a pattern like "cargo" always matches someone else's process too.
Such loops ran for 15 hours before anyone killed them. The same goes for a
wait on a pid or a time without a loop (`caffeinate -w <pid>`, `caffeinate -t
170`), which `builds health` stopped as junk several times an hour. Work that
takes a while runs in the background, and the harness reports when it exits.
The same goes for one `sleep` followed by a read of the build's output.
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
    "no-polling-loops refuses commands that wait by polling or on a timer. Examples include while or "
    "until loops that check a process (pgrep, lsof, kill -0, ps -p) or sleep between checks, `tail -f | grep`, "
    "`tail -f --pid`, `tail -f /dev/null` (including under `timeout`), "
    "`caffeinate -t` or `-w`, `lsof -r`, and python's `time.sleep`. Such a wait can "
    "keep running for hours after its condition came true, or forever when the condition never "
    "comes true. `wait <pid>` returns at once when another command started the process. A command "
    "that starts with `sleep` and goes on to read a log (`sleep 240; tail -1 build.log`) polls the "
    "same way. Run the work in the background (run_in_background) and wait for its completion "
    "notification."
)

LOOP = re.compile(r"\b(while|until)\b(?P<cond>.*?)\bdo\b", re.S)
# A loop condition that asks whether a process still runs.
PROCESS_CHECK = re.compile(r"\b(pgrep|pidof|lsof)\b|\bkill\s+-0\b|\bps\b[^;|&]*\s-p\b")
FOLLOW = re.compile(r"-[a-zA-Z]*[fF][a-zA-Z]*|--follow\S*")
TAIL_SHORT_OPTIONS = re.compile(r"-(?P<flags>[fFqrv]*)(?:(?P<value_option>[bcns])(?P<value>.*))?")
TAIL_LEGACY_OPTIONS = re.compile(r"[+-]\d+(?P<flags>[bclfr]*)")
TAIL_LONG_VALUE_OPTIONS = {
    "--blocks", "--bytes", "--lines", "--max-unchanged-stats", "--pid", "--sleep-interval",
}
ASSIGNMENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*=")
# Programs that run the command after them, with their options that take a
# value and the number of arguments of their own: `timeout 590 caffeinate -w <pid>`.
TIMEOUT_VALUE_OPTIONS = {"-s", "--signal", "-k", "--kill-after"}
WRAPPERS = {
    "nohup": (set(), 0), "exec": ({"-a"}, 0), "command": (set(), 0), "time": (set(), 0),
    "nice": ({"-n"}, 0), "env": ({"-u", "-P", "-S"}, 0),
    "timeout": (TIMEOUT_VALUE_OPTIONS, 1), "gtimeout": (TIMEOUT_VALUE_OPTIONS, 1),
}


def follows_into_pipe(command: str) -> bool:
    """`tail -f`/`-F` (or --follow) piped into a reader: tail follows the file
    forever, and only notices its reader has exited on its next write, so
    `tail -F log | grep -m1 done` outlives its match for as long as the log is
    quiet (one ran 2 hours after its line came). Read as shell words, so the
    same words inside a quoted string (a commit message) are text."""
    return any(
        separator == "|" and words[:1] == ["tail"] and any(FOLLOW.fullmatch(w) for w in words[1:])
        for words, separator in shell_words.commands(command)
    )


def tail_waits(rest: list[str]) -> bool:
    """Following only /dev/null waits forever or until its wrapper times out.
    GNU tail with --pid waits for a separate process to exit."""
    follows = False
    has_pid = False
    files = []
    args = iter(rest)
    for word in args:
        if word == "--":
            files.extend(args)
            break
        if not files and (legacy := TAIL_LEGACY_OPTIONS.fullmatch(word)):
            follows |= "f" in legacy["flags"]
        elif word == "-" or not word.startswith("-"):
            files.append(word)
        elif word.startswith("--"):
            option, equals, _ = word.partition("=")
            follows |= option == "--follow"
            has_pid |= option == "--pid"
            if option in TAIL_LONG_VALUE_OPTIONS and not equals:
                next(args, None)
        elif short := TAIL_SHORT_OPTIONS.fullmatch(word):
            follows |= any(flag in "fF" for flag in short["flags"])
            if short["value_option"] and not short["value"]:
                next(args, None)
        else:
            return False
    return follows and (has_pid or set(files) == {"/dev/null"})


def program(words: list[str]) -> list[str]:
    """The words of the program a command runs, from its name on: without the
    variable assignments before it, the wrappers that run it (`nohup`,
    `timeout 590`, `env X=1`) and its redirections."""
    i = 0
    while i < len(words) and ASSIGNMENT.match(words[i]):
        i += 1
    while i < len(words) and (wrapper := Path(words[i]).name) in WRAPPERS:
        value_options, arguments = WRAPPERS[wrapper]
        i += 1
        while i < len(words) and (words[i].startswith("-") or ASSIGNMENT.match(words[i])):
            i += 2 if words[i] in value_options else 1
        i += arguments
    return shell_words.without_redirections(words[i:])


def caffeinate_waits(words: list[str]) -> bool:
    """`caffeinate -t <seconds>` or `-w <pid>` with no program to run: it waits
    for the time to pass or the process to exit. With a program caffeinate
    ignores both and runs the program (`caffeinate -i cargo build`)."""
    if not words or Path(words[0]).name != "caffeinate":
        return False
    waits, i = False, 1
    while i < len(words) and words[i].startswith("-") and words[i] != "--":
        flags = words[i][1:]
        for n, flag in enumerate(flags):
            if flag in "tw":
                waits = True
                # The value is the rest of the word (`-w4242`), else the next word.
                i += n == len(flags) - 1
                break
        i += 1
    if words[i:i + 1] == ["--"]:
        i += 1
    return waits and not words[i:]


def waits_on_another_pid(words: list[str]) -> bool:
    """`wait <pid>` for a process this shell did not start: the shell refuses
    it at once ("not a child of this shell"), so what comes next runs before
    the process exits. A literal pid, or one from `$(pgrep ...)` (which the
    shell words split as `$`) or backquotes, names such a process; `wait`,
    `wait $!` and `wait %1` wait on this shell's own jobs."""
    return words[:1] == ["wait"] and any(re.fullmatch(r"\d+|\$|`.*", w) for w in words[1:])


def simple_wait(words: list[str]) -> bool:
    """A single command that waits on a pid or a time, after unwrapping it."""
    words = program(words)
    if not words:
        return False
    name, rest = Path(words[0]).name, words[1:]
    return (
        caffeinate_waits(words)
        or waits_on_another_pid(words)
        or (name == "tail" and tail_waits(rest))
        or (name == "lsof" and any(re.match(r"[+-]r", w) for w in rest))
        or (name.startswith("python") and any("time.sleep" in w for w in rest))
    )


# Words that start a command's line in a compound statement without being
# the command: `do sleep 5`, `then sleep 1`, `! sleep 1`.
STATEMENT_START = {"do", "then", "else", "elif", "{", "!", "time"}
LOOP_HEADS = {"while", "until"}
OTHER_LOOP_HEADS = {"for", "select"}


def sleeps_in_loop(command: str) -> bool:
    """A `sleep` command inside a while or until loop, at any depth: in its
    condition or body, after a nested loop's `done`, in any `if` or `&&`.
    Read as shell words, so a `sleep` inside a quoted string is text. Each
    `done` closes the innermost loop, so a for loop's `done` does not end the
    while loop around it."""
    open_loops = []
    for words, _ in shell_words.commands(command):
        while words and words[0] in STATEMENT_START:
            words = words[1:]
        if not words:
            continue
        if words[0] in LOOP_HEADS | OTHER_LOOP_HEADS:
            open_loops.append(words[0])
        elif words[0] == "done" and open_loops:
            open_loops.pop()
        if any(loop in LOOP_HEADS for loop in open_loops):
            run = program(words[1:] if words[0] in LOOP_HEADS else words)
            if run and Path(run[0]).name == "sleep":
                return True
    return False


# Programs that only change the shell's own state: a step of one of these
# (or a bare assignment) before a `sleep` does not make the sleep a later
# step of the command.
SETUP = {"cd", "pushd", "popd", "export", "set", "unset"}


def sleeps_then_runs(command: str) -> bool:
    """A command whose first step after its setup steps (`cd w`, `export
    A=1`, `A=1`, `set -e`) is `sleep` (or a wrapper around one, such as
    `timeout 300 sleep 240`) and that has any later step: `cd w && sleep 240;
    tail -1 build.log`. One wait followed by a read polls a background task by
    hand, and the harness reports when the task ends. A `sleep` alone, a
    `sleep` after any other step, and the word inside a quoted string are not
    refused."""
    steps = [words for words, _ in shell_words.commands(command) if words]
    i = 0
    while i < len(steps) and (not program(steps[i]) or program(steps[i])[0] in SETUP):
        i += 1
    if i + 1 >= len(steps):
        return False
    return Path(program(steps[i])[0]).name == "sleep"


def blocked(command: str) -> bool:
    # A loop that checks a process, or any loop that sleeps between checks:
    # both poll for a condition the harness already reports, and a condition
    # that never comes true (a pattern matching the loop itself, a log line
    # that is never the last one) keeps the shell running for hours.
    return (
        follows_into_pipe(command)
        or any(simple_wait(words) for words, _ in shell_words.commands(command))
        or sleeps_in_loop(command)
        or sleeps_then_runs(command)
        or any(PROCESS_CHECK.search(m.group("cond")) for m in LOOP.finditer(command))
    )


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
