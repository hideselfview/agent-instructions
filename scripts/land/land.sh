#!/bin/bash
# land.sh <worktree> <sha>...: put the commits on origin/main (fast-forward when they sit on it, else cherry-pick), retrying the whole step on a refused push.
# A cherry-picked result is a tree no hook has seen, so before pushing it the repo's whole-tree checks run: the commands in the repo's `.land-checks` (one per line, blank lines and lines starting with # skipped, each run with `sh -c` from the repo root). A failing check is named with its output, nothing is pushed, and the worktree goes back to origin/main (exit 1, no retry). Then the repo's `.land-compile`, if present, runs (`bash .land-compile` from the repo root, reading what the commits changed from `git diff origin/main HEAD`): the compile check of the result, which the static checks can't do, since two commits that each compile can break each other. It fails the same way. A fast-forward is the tree the hook already ran on, so it skips both.
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
    # A push that reported failure may still have landed, and someone else may
    # have landed the same change: pick only the commits whose change isn't on
    # origin/main yet, so a retry doesn't call an applied commit a conflict.
    todo=(); for c in "$@"; do
      [ "$(git cherry origin/main "$c" "$c^" | cut -c1)" = + ] && todo+=("$c")
    done
    if [ "${#todo[@]}" = 0 ]; then
      echo "already on origin/main:"
      for c in "$@"; do git log --oneline -1 --grep="^$(git log -1 --format=%s "$c" | sed 's/[][\.*^$]/\\&/g')$" origin/main; done
      exit 0
    fi
    if ! git cherry-pick "${todo[@]}" >/dev/null 2>&1; then git cherry-pick --abort; echo CONFLICT; exit 1; fi
    if [ -f .land-checks ]; then
      while IFS= read -r check || [ -n "$check" ]; do
        case "$check" in ''|'#'*) continue ;; esac
        if ! out=$(sh -c "$check" </dev/null 2>&1); then
          git reset -q --hard origin/main
          echo "CHECK FAILED on the cherry-picked result, nothing pushed: $check"
          echo "$out"
          exit 1
        fi
      done < .land-checks
    fi
    if [ -f .land-compile ]; then
      if ! out=$(bash .land-compile </dev/null 2>&1); then
        git reset -q --hard origin/main
        echo "COMPILE CHECK FAILED on the cherry-picked result, nothing pushed: .land-compile"
        echo "$out"
        exit 1
      fi
    fi
  fi
  if out=$(git push -q origin HEAD:main 2>&1); then git fetch -q origin; git log --oneline -"$#" origin/main; exit 0; fi
  echo "push failed: $(echo "$out" | grep -v '^hint' | tail -1)"
done
exit 1
