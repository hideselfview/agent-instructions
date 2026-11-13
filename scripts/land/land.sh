#!/bin/bash
# The landing worktree must be claimed (builds claim): the claim names who is landing, and the store records that name.
# land.sh <worktree> [--done N]... [--part N]... [--design N] <sha>...: put the commits on origin/main (fast-forward when they sit on it, else cherry-pick), retrying the whole step on a refused push.
# A cherry-picked result is a tree no hook has seen, so before pushing it the repo's whole-tree checks run: the commands in the repo's `.land-checks` (one per line, blank lines and lines starting with # skipped, each run with `sh -c` from the repo root). A failing check is named with its output, nothing is pushed, and the worktree goes back to origin/main (exit 1, no retry). Then the repo's `.land-compile`, if present, runs (`bash .land-compile` from the repo root, reading what the commits changed from `git diff origin/main HEAD`): the compile check of the result, which the static checks can't do, since two commits that each compile can break each other. It fails the same way. A fast-forward runs both too: a branch rebased after its hook ran is a tree no hook has seen.
# Two landings in one worktree move each other's HEAD: one pushed the other's and called it landed. Each waits for the worktree to itself.
if [ -z "$LAND_LOCKED" ]; then
  git_dir=$(git -C "$1" rev-parse --absolute-git-dir) || exit 1
  LAND_LOCKED=1 exec lockf "$git_dir/land.lock" "$0" "$@"
fi
cd "$1" || exit 1; shift
# An agent's worktree sits on its branch; the landing worktree is detached. Landing there would move the agent off its branch.
if b=$(git symbolic-ref -q --short HEAD); then echo "refusing: $PWD is on branch $b; land from the detached landing worktree"; exit 1; fi
# In a repo that belongs to a project of the lists (`builds lists project-of`),
# a landing names the work items it finishes (--done N) or advances (--part N)
# and, when the commits change design places, the design change (--design N),
# which the person must have approved (`builds design check` says so).
# Each is checked before anything is fetched or picked, and recorded after the
# push. `builds design places` prints one line per changed design path
# (`place`, `source`, `spec` or `shared`, then the path), nothing when none; any
# line, shared-only included, requires --design. When every commit's paths are design
# files (`builds design only`: boards, the design system, spec lines), --design N
# alone satisfies the --done/--part requirement: the design change holds the
# app work. A repo in no project takes none of these flags and lands as before.
done_args=(); design=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --done|--part) [ "$#" -ge 2 ] || { echo "refusing: $1 needs an item number"; exit 1; }; done_args+=("$1" "$2"); shift 2 ;;
    --design) [ "$#" -ge 2 ] || { echo "refusing: --design needs a design change number"; exit 1; }
      [ -z "$design" ] || { echo "refusing: --design given twice"; exit 1; }; design=$2; shift 2 ;;
    --) shift; break ;;
    --*) echo "refusing: unknown option $1"; exit 1 ;;
    *) break ;;
  esac
done
[ "$#" -gt 0 ] || { echo "usage: land.sh <worktree> [--done N]... [--part N]... [--design N] <sha>... (the landing worktree must be claimed; --design is required exactly when the commits change a design place or shared design file, and alone it lands commits that are all design files)"; exit 1; }
project=$(builds lists project-of "$PWD" 2>&1) || { echo "refusing: builds lists project-of failed: $project"; exit 1; }
if [ -z "$project" ]; then
  if [ "${#done_args[@]}" != 0 ] || [ -n "$design" ]; then
    echo "refusing: $PWD is in no project of the lists, so --done, --part and --design don't apply"; exit 1
  fi
else
  places=$(builds design places "$PWD" "$@" 2>&1) || { echo "refusing: builds design places failed: $places"; exit 1; }
  if [ "${#done_args[@]}" = 0 ]; then
    if [ -z "$design" ]; then
      echo "refusing: $PWD is in the project $project; name the work item this lands with --done N or --part N (commits that are all design files take --design N alone)"; exit 1
    fi
    if [ -z "$places" ]; then
      echo "refusing: --design $design given, but the commits change no design place; name the work item with --done N or --part N"; exit 1
    fi
    if ! out=$(builds design only "$PWD" "$@" 2>&1); then
      echo "LANDING REFUSED, nothing fetched or pushed: --design alone lands only commits that are all design files; name the work item with --done N or --part N"
      echo "$out"; exit 1
    fi
  elif ! out=$(builds item check-landing "$PWD" "${done_args[@]}" "$@" 2>&1); then
    echo "LANDING REFUSED, nothing fetched or pushed:"; echo "$out"; exit 1
  fi
  if [ -n "$places" ] && [ -z "$design" ]; then
    echo "LANDING REFUSED, nothing fetched or pushed: the commits change design places; name the design change with --design N"
    echo "$places"; exit 1
  fi
  if [ -z "$places" ] && [ -n "$design" ]; then
    echo "refusing: --design $design given, but the commits change no design place"; exit 1
  fi
  if [ -n "$design" ] && ! out=$(builds design check "$design" "$PWD" "$@" 2>&1); then
    echo "LANDING REFUSED, nothing fetched or pushed:"; echo "$out"; exit 1
  fi
fi
workdir=$PWD
# Records what landed. Both calls are idempotent, so a failure prints the rerun.
record_landing() {
  [ -n "$project" ] || return 0
  local failed=0 out cmd
  if [ "${#done_args[@]}" != 0 ]; then
    cmd=(builds item landed "$workdir" "${done_args[@]}" "$@")
    if ! out=$("${cmd[@]}" 2>&1); then
      failed=1; echo "LANDED, NOT RECORDED: $out"; printf 'rerun: '; printf '%q ' "${cmd[@]}"; echo
    fi
  fi
  if [ -n "$design" ]; then
    cmd=(builds design landed "$design" "$workdir" "$@")
    if ! out=$("${cmd[@]}" 2>&1); then
      failed=1; echo "LANDED, NOT RECORDED: $out"; printf 'rerun: '; printf '%q ' "${cmd[@]}"; echo
    fi
  fi
  return "$failed"
}
last="${@: -1}"
for attempt in 1 2 3 4 5; do
  git fetch -q origin || { echo "fetch failed"; continue; }
  git checkout -q --detach origin/main
  before=$(git rev-parse origin/main)
  # A fast-forward lands every commit between origin/main and the last one, so
  # it is taken only when those are exactly the commits named: an unnamed
  # commit below them (an earlier one on the agent's branch) is never carried in.
  all_in=1; for c in "$@"; do git merge-base --is-ancestor "$c" "$last" || all_in=0; done
  [ "$(git rev-list --count origin/main.."$last")" = "$#" ] || all_in=0
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
      on_main=(); for c in "$@"; do
        h=$(git log --format=%H -1 --grep="^$(git log -1 --format=%s "$c" | sed 's/[][\.*^$]/\\&/g')$" origin/main)
        [ -z "$h" ] || git log --oneline -1 "$h"
        on_main+=("$h")
      done
      # An earlier push that reported failure may have landed without its record.
      if [ -n "$project" ]; then
        for h in "${on_main[@]}"; do [ -n "$h" ] || { echo "LANDED, NOT RECORDED: a commit's copy on origin/main was not found by its subject; record by hand with builds item landed"; exit 1; }; done
      fi
      record_landing "${on_main[@]}" || exit 1
      exit 0
    fi
    if ! git cherry-pick "${todo[@]}" >/dev/null 2>&1; then git cherry-pick --abort; echo CONFLICT; exit 1; fi
  fi
  landed=$(git rev-parse HEAD)
  # A fast-forward runs them too: an agent that rebased after its hook ran
  # hands over a tree no hook has seen.
  if [ -f .land-checks ]; then
    while IFS= read -r check || [ -n "$check" ]; do
      case "$check" in ''|'#'*) continue ;; esac
      if ! out=$(sh -c "$check" </dev/null 2>&1); then
        git reset -q --hard origin/main
        echo "CHECK FAILED on the result, nothing pushed: $check"
        echo "$out"
        exit 1
      fi
    done < .land-checks
  fi
  if [ -f .land-compile ]; then
    if ! out=$(bash .land-compile </dev/null 2>&1); then
      git reset -q --hard origin/main
      echo "COMPILE CHECK FAILED on the result, nothing pushed: .land-compile"
      echo "$out"
      exit 1
    fi
  fi
  # Something else that moved this worktree's HEAD during the checks (a
  # checkout run by hand) leaves nothing of the landing's to push, and the
  # push of origin/main to itself would report success. HEAD is compared with
  # the commit the landing made, not by patch: a cherry-pick onto changed
  # surrounding lines has another patch id though it holds the change.
  if [ "$(git rev-parse HEAD)" != "$landed" ]; then
    echo "HEAD moved during the landing: it is $(git log --oneline -1 HEAD), not $(git log --oneline -1 "$landed"); nothing pushed"
    exit 1
  fi
  if out=$(git push -q origin HEAD:main 2>&1); then
    git fetch -q origin; git log --oneline -"$#" origin/main
    on_main=(); while IFS= read -r h; do on_main+=("$h"); done < <(git rev-list --reverse "$before"..HEAD)
    record_landing "${on_main[@]}" || exit 1
    exit 0
  fi
  echo "push failed: $(echo "$out" | grep -v '^hint' | tail -1)"
done
exit 1
