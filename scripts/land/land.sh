#!/bin/bash
# land.sh <worktree> <sha>...: put the commits on origin/main (fast-forward when they sit on it, else cherry-pick), retrying the whole step on a refused push.
cd "$1" || exit 1; shift
# An agent's worktree sits on its branch; the landing worktree is detached. Landing there would move the agent off its branch.
if b=$(git symbolic-ref -q --short HEAD); then echo "refusing: $PWD is on branch $b; land from the detached landing worktree"; exit 1; fi
last="${@: -1}"
for attempt in 1 2 3 4 5; do
  git fetch -q origin || { echo "fetch failed"; continue; }
  git checkout -q --detach origin/main
  all_in=1; for c in "$@"; do git merge-base --is-ancestor "$c" "$last" || all_in=0; done
  if [ "$all_in" = 0 ] || ! git merge -q --ff-only "$last" 2>/dev/null; then
    git checkout -q --detach origin/main
    if ! git cherry-pick "$@" >/dev/null 2>&1; then git cherry-pick --abort; echo CONFLICT; exit 1; fi
  fi
  if out=$(git push -q origin HEAD:main 2>&1); then git fetch -q origin; git log --oneline -"$#" origin/main; exit 0; fi
  echo "push failed: $(echo "$out" | grep -v '^hint' | tail -1)"
done
exit 1
