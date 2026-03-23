# Rebase Guide

Conventions for maintaining clean, linear history using rebase.

---

## Core Rule

- **`master` and `develop`** are shared branches. Never rebase them — only merge into them.
- **Implementation branches** are personal. Rebase freely before merging.

---

## Rebase onto develop

Bring your branch up to date without merge commits:

```bash
git fetch origin
git rebase origin/develop
```

What happens:

```text
Before:
  develop:     A --- B --- C --- D --- E
                              \
  feature:                     F --- G

After:
  develop:     A --- B --- C --- D --- E
                                        \
  feature:                               F' --- G'
```

Git removes your commits, moves the branch pointer to the tip of `develop`, then replays your commits on top.

After rebasing, force-push:

```bash
git push --force-with-lease
```

Always use `--force-with-lease` (not `--force`) — it checks that nobody else pushed to your branch since your last fetch.

---

## Interactive Rebase (cleanup)

Before opening a PR, clean up messy commits:

```bash
git rebase -i HEAD~N    # N = number of commits to edit
```

Actions:

```text
pick   (p)  Keep commit as-is
reword (r)  Keep commit, edit message
squash (s)  Meld into previous commit, combine messages
fixup  (f)  Meld into previous commit, discard this message
drop   (d)  Remove the commit entirely
(reorder)   Move lines up/down to reorder commits
```

Common patterns:

```text
# Squash WIP into previous
pick   a1b2c3 setup auth module
fixup  d4e5f6 wip stuff
pick   g7h8i9 add token refresh

# Fix a bad message
reword a1b2c3 bad message here
pick   d4e5f6 add login endpoint

# Drop unwanted commit
pick   a1b2c3 setup auth module
drop   d4e5f6 debug garbage
pick   g7h8i9 add token refresh
```

---

## Conflict Resolution

When a replayed commit touches lines that changed in `develop`, Git stops:

```bash
git status

# Open the file — look for conflict markers:
# <<<<<<< HEAD
#     the code from develop
# =======
#     your code from the commit being replayed
# >>>>>>> F: setup auth module skeleton

# Fix it, then:
git add <fixed-file>
git rebase --continue
```

If you get lost:

```bash
git rebase --abort    # returns to exactly where you were before
```

---

## Multiple Rebases

Rebasing multiple times during a long-lived branch is normal:

```bash
git fetch origin
git rebase origin/develop
# resolve conflicts if any
git push --force-with-lease
```

Rebase frequently to avoid large conflict pileups.

---

## Common Mistakes

| Mistake | Why it's bad | Fix |
|---------|-------------|-----|
| Rebasing `develop` or `master` | Rewrites shared history, breaks everyone | Never rebase shared branches |
| Forgetting `git fetch` before rebase | Rebases onto stale local copy | Always `git fetch origin` first |
| Using `--force` instead of `--force-with-lease` | Blindly overwrites remote | Always use `--force-with-lease` |
| Rebasing with uncommitted changes | Rebase won't start with dirty tree | `git stash` first, then `git stash pop` after |
| Panicking during conflicts | Unnecessary stress | `git rebase --abort` is always safe |
