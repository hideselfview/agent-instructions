#!/usr/bin/env python3
"""PreToolUse hook on Bash: refuse a command that waits by polling or on a timer.

`until ! pgrep -f "<pattern>"; do sleep 5; done` never ends: the loop's own
command line holds the pattern, so pgrep always finds it. With other agents
building, a pattern like "cargo" always matches someone else's process too.
Such loops ran for 15 hours before anyone killed them. The same goes for a
wait on a pid or a time without a loop (`caffeinate -w <pid>`, `caffeinate -t
170`), which `builds health` stopped as junk several times an hour. Work that
takes a while runs in the background, and the harness reports when it exits.
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
    "no-polling-loops refuses commands that wait by polling or on a timer: a while or until loop "
    "that checks a process (pgrep, lsof, kill -0, ps -p) or sleeps between checks, `tail -f | grep`, "
    "`tail -f --pid`, `caffeinate -t` or `-w`, `lsof -r`, and python's `time.sleep`. Such a wait can "
    "keep running for hours after its condition came true, or forever when the condition never "
    "comes true. `wait <pid>` returns at once when another command started the process. Run the "
    "work in the background (run_in_background) and wait for its completion notification."
)

LOOP = re.compile(r"\b(while|until)\b(?P<cond>.*?)\bdo\b(?P<body>.*?)\bdone\b", re.S)
# A loop condition that asks whether a process still runs.
PROCESS_CHECK = re.compile(r"\b(pgrep|pidof|lsof)\b|\bkill\s+-0\b|\bps\b[^;|&]*\s-p\b")
FOLLOW = re.compile(r"-[a-zA-Z]*[fF][a-zA-Z]*|--follow\S*")
ASSIGNMENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*=")
# Programs that run the command after them, with their options that take a
# value and the number of arguments of their own: `timeout 590 caffeinate -w <pid>`.
WRAPPERS = {
    "nohup": (set(), 0), "exec": ({"-a"}, 0), "command": (set(), 0), "time": (set(), 0),
    "nice": ({"-n"}, 0), "env": ({"-u", "-P", "-S"}, 0),
    "timeout": ({"-s", "-k"}, 1), "gtimeout": ({"-s", "-k"}, 1),
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


def waits_on_pid(command: str) -> bool:
    """`tail -f /dev/null --pid=N`: GNU tail's wait for a process to exit.
    macOS's tail has no --pid, so with stderr hidden it follows /dev/null
    forever; one ran 2 hours after its process was gone."""
    return any(
        words[:1] == ["tail"] and any(FOLLOW.fullmatch(w) for w in words[1:])
        and any(w == "/dev/null" or w.startswith("--pid") for w in words[1:])
        for words, _ in shell_words.commands(command)
    )


def program(words: list[str]) -> list[str]:
    """The words of the program a command runs, from its name on: without the
    variable assignments before it, the wrappers that run it (`nohup`,
    `timeout 590`, `env X=1`) and its redirections."""
    i = 0
    while i < len(words) and ASSIGNMENT.match(words[i]):
        i += 1
    while i < len(words) and words[i] in WRAPPERS:
        value_options, arguments = WRAPPERS[words[i]]
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
    """A single command that waits on a pid or a time: caffeinate, `wait
    <pid>`, lsof repeating until the process's files close (`+r`) or forever
    (`-r`), python's `time.sleep`."""
    words = program(words)
    if not words:
        return False
    name, rest = Path(words[0]).name, words[1:]
    return (
        caffeinate_waits(words)
        or waits_on_another_pid(words)
        or (name == "lsof" and any(re.match(r"[+-]r", w) for w in rest))
        or (name.startswith("python") and any("time.sleep" in w for w in rest))
    )


def blocked(command: str) -> bool:
    # A loop that checks a process, or any loop that sleeps between checks:
    # both poll for a condition the harness already reports, and a condition
    # that never comes true (a pattern matching the loop itself, a log line
    # that is never the last one) keeps the shell running for hours.
    return (
        follows_into_pipe(command)
        or waits_on_pid(command)
        or any(simple_wait(words) for words, _ in shell_words.commands(command))
        or any(
            PROCESS_CHECK.search(m.group("cond")) or re.search(r"\bsleep\b", m.group("body"))
            for m in LOOP.finditer(command)
        )
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
