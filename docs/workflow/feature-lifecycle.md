# Feature Lifecycle

Every code change starts from a forge issue. Three entry points converge on a shared lifecycle.

---

## Entry Points

### From an existing issue

```bash
# 1. Let the forge create and name the branch
gh issue develop <id> --base develop

# 2. Retrieve the branch name
BRANCH=$(gh issue develop <id> --list --json headRefName --jq '.[0].headRefName')

# 3. Create work-clone
git clone --reference ~/Projects/gubasso/riptask \
    git@github.com:gubasso/riptask.git \
    ~/Projects/gubasso/riptask."$BRANCH"
cd ~/Projects/gubasso/riptask."$BRANCH"
git fetch origin "$BRANCH"
git checkout "$BRANCH"
```

### From a new issue

```bash
# 1. Create the issue
gh issue create --title "Add auth module"    # → returns ID

# 2–3. Same as above: gh issue develop, clone, checkout
```

### From uncommitted changes

```bash
# 1. Save changes in main repo
cd ~/Projects/gubasso/riptask
git stash

# 2. Create issue and branch (same as above)

# 3. Transfer changes to work-clone
git -C ~/Projects/gubasso/riptask stash show -p | git apply
git -C ~/Projects/gubasso/riptask stash drop
```

---

## Shared Lifecycle

### Sync

Push the feature branch to remote and set up tracking:

```bash
git push -u origin <branch>
```

### Work

Commit and push in the work-clone. Rebase onto `develop` as needed to stay current. See [Rebase Guide](rebase-guide.md).

```bash
git fetch origin
git rebase origin/develop
git push --force-with-lease
```

### Finish

Push the branch and open a PR:

```bash
git push -u origin <branch>
gh pr create --base develop --head <branch>
```

Add `--draft` for a draft PR.

### Cleanup

After the PR is merged:

```bash
# Pull merged changes into the main repo
cd ~/Projects/gubasso/riptask
git checkout develop
git pull

# Remove the work-clone
rm -rf ~/Projects/gubasso/riptask.<branch>

# Delete the local feature branch (remote auto-deleted by forge on merge)
git branch -d <branch>
```

### Release

See [Release Workflow](release-workflow.md).

---

## Work-Clones

A work-clone is an isolated clone created with `git clone --reference`, placed as a sibling directory:

```text
~/Projects/gubasso/riptask                    ← main repo
~/Projects/gubasso/riptask.42-add-auth        ← work-clone for issue #42
```

It shares the main repo's object store so disk usage is minimal. Do not delete the main repo while work-clones exist.

---

## Quick Reference

```text
FROM EXISTING ISSUE:
  gh issue develop <id> --base develop
  BRANCH=$(gh issue develop <id> --list ...)
  git clone --reference <main> <remote> <work-clone>
  git fetch origin $BRANCH && git checkout $BRANCH

FROM NEW ISSUE:
  gh issue create --title "..."
  (same as above)

FROM UNCOMMITTED CHANGES:
  git stash
  (create issue + branch + work-clone)
  git -C <main> stash show -p | git apply
  git -C <main> stash drop

SYNC:    git push -u origin <branch>
REBASE:  git fetch origin && git rebase origin/develop && git push --force-with-lease
FINISH:  gh pr create --base develop
CLEANUP: cd <main> && git pull && rm -rf <work-clone> && git branch -d <branch>
```
