#!/usr/bin/env python3
"""Count a todo file's open items and log what opened and closed since the last run.

An item is a list line ("- " or "N. ", any indent) with no list lines nested
under it; a line with nested items is a group heading, not an item. Each item
carries a stable id, " [#N]" at the end of its first line (a "[#N]" elsewhere
in the text is a reference, not an id): this script appends
one to every item that has none, so an item keeps its id while its text,
position or number changes. Items are counted per "## 1." and "## 2." section.

State (the open ids and the totals) is kept in todo-tally.json beside the todo
file; each run that finds a change appends a line to todo-tally.log there.

    tally.py ~/dev/bae/plans/todo.md
"""

import datetime
import json
import os
import re
import sys

LIST = re.compile(r"^(\s*)(?:- |\d+\. )")
# Only the tag at the end of an item's first line is its id; a "[#12]" inside
# the text is a reference to another item.
ID = re.compile(r" \[#(\d+)\]\s*$")


def items(lines):
    """Yield (line index, section) of each item: a list line nothing nests under."""
    section = None
    for i, line in enumerate(lines):
        if line.startswith("## "):
            section = {"## 1.": "1", "## 2.": "2"}.get(line[:5])
            continue
        m = LIST.match(line)
        if not m or section is None:
            continue
        indent = len(m.group(1))
        leaf = True
        for later in lines[i + 1 :]:
            if later.startswith("#"):
                break
            n = LIST.match(later)
            if n:
                leaf = len(n.group(1)) <= indent
                break
        if leaf:
            yield i, section


def title(line):
    text = ID.sub("", LIST.sub("", line, count=1)).replace("**", "").strip()
    text = re.sub(r"^Running: [^.]*\. ", "", text)
    return text[:70]


def main(todo):
    folder = os.path.dirname(os.path.abspath(todo))
    state_path = os.path.join(folder, "todo-tally.json")
    log_path = os.path.join(folder, "todo-tally.log")
    try:
        with open(state_path) as f:
            state = json.load(f)
    except FileNotFoundError:
        state = None

    with open(todo) as f:
        text = f.read()
    lines = text.split("\n")
    next_id = state["next_id"] if state else 1
    for i, _ in items(lines):
        if not ID.search(lines[i]):
            lines[i] = f"{lines[i].rstrip()} [#{next_id}]"
            next_id += 1

    now = {}
    for i, section in items(lines):
        ident = ID.search(lines[i]).group(1)
        if ident in now:
            sys.exit(f"tally: id #{ident} is on two items; give one of them a new id")
        now[ident] = {"section": section, "title": title(lines[i])}

    new_text = "\n".join(lines)
    if new_text != text:
        tmp = todo + ".tally-tmp"
        with open(tmp, "w") as f:
            f.write(new_text)
        os.replace(tmp, todo)

    stamp = datetime.datetime.now().strftime("%Y-%m-%d %H:%M")
    counts = {s: sum(1 for v in now.values() if v["section"] == s) for s in "12"}
    if state is None:
        entry = f"{stamp}  baseline: §1 {counts['1']} open, §2 {counts['2']} open"
        state = {"opened": 0, "closed": 0}
        lines_out = [entry]
    else:
        before = state["open"]
        opened = [k for k in now if k not in before]
        closed = [k for k in before if k not in now]
        moved = [k for k in now if k in before and before[k]["section"] != now[k]["section"]]
        state_opened = state["opened"] + len(opened)
        state_closed = state["closed"] + len(closed)
        lines_out = []
        if opened or closed or moved:
            lines_out.append(
                f"{stamp}  §1 {counts['1']} open, §2 {counts['2']} open;"
                f" +{len(opened)} opened, -{len(closed)} closed;"
                f" since baseline {state_opened} opened, {state_closed} closed"
            )
            for k in opened:
                lines_out.append(f"    + #{k} §{now[k]['section']} {now[k]['title']}")
            for k in closed:
                lines_out.append(f"    - #{k} §{before[k]['section']} {before[k]['title']}")
            for k in moved:
                lines_out.append(f"    > #{k} §{before[k]['section']}→§{now[k]['section']} {now[k]['title']}")
        state = {"opened": state_opened, "closed": state_closed}

    state.update(next_id=next_id, open=now)
    tmp = state_path + ".tmp"
    with open(tmp, "w") as f:
        json.dump(state, f, indent=1)
    os.replace(tmp, state_path)
    if lines_out:
        with open(log_path, "a") as f:
            f.write("\n".join(lines_out) + "\n")
    print(
        f"§1 {counts['1']} open, §2 {counts['2']} open;"
        f" since baseline {state['opened']} opened, {state['closed']} closed"
    )


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
