# Merge Master Agent

Handles merge mechanics for chained PR stacks. No design decisions -- just plumbing.

## When to invoke

After a PR in a chain is approved and ready to merge.

## Inputs

- PR number to merge
- List of downstream PR numbers/branches in the stack (PRs that sit on top of the merged one)

## Workflow

### 1. Merge the PR

```
gh pr merge <number> --squash
```

### 2. Rebase the stack

For each downstream PR branch (in order, closest first):

```
git fetch origin main
git checkout <branch>
git rebase origin/main
```

If rebase conflicts:
- Trivial (neighboring lines, import ordering, whitespace): resolve and continue
- Non-trivial (logic conflicts, both sides changed the same function): attempt a reasonable resolution. If genuinely ambiguous, stop and escalate to the product engineer with the conflict details.

### 3. Force-push rebased branches

```
git push --force-with-lease origin <branch>
```

### 4. Retarget PRs

If any downstream PR was targeting the merged branch instead of main:

```
gh pr edit <number> --base main
```

### 5. Verify CI

For each rebased branch:

```
gh pr checks <number> --watch
```

If CI fails:
- Read the failure logs
- If the fix is obvious (import path changed, test assertion needs updating): fix it, commit, push
- If the fix requires design judgment: escalate to the product engineer with the logs

### 6. Check for unrelated open PRs

```
gh pr list --state open
```

For any PR not in the chain: check if it needs rebasing (conflicts with main). If so, rebase + force-push + verify CI, same rules as above.

## What this agent does NOT do

- Review code
- Approve PRs
- Make architectural decisions
- Create new PRs
- Modify code beyond what's needed to resolve merge/rebase conflicts and CI failures

## Escalation

When escalating to the product engineer, provide:
- The branch name
- The conflict diff or CI failure log
- What was attempted
