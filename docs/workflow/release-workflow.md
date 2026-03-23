# Release Workflow

Automated release pipeline using GitHub Actions and cargo-release.

---

## Overview

```text
develop → (release PR) → master → (auto-publish + tag + merge back) → develop
```

| Workflow | Trigger | Purpose |
|----------|---------|---------|
| CI | Push/PR to `develop` and `master` | Run pre-commit hooks in CI |
| Create Release PR | `workflow_dispatch` | Bump version, open PR to `master` |
| Publish & Sync | Push to `master` | Publish to crates.io, tag, GitHub release, merge back |

---

## Step by Step

### 1. Trigger the release

From any local branch (the workflow checks out `develop` on GitHub's side):

```bash
gh workflow run "Create Release PR" -f bump=patch
```

Bump options: `patch`, `minor`, `major`, or an exact version string.

Watch progress:

```bash
gh run watch
```

### 2. What the workflow does

1. Checks out `develop`
2. Runs `cargo release version <bump>` to determine the next version
3. Creates a `release/v<version>` branch
4. Bumps the version in `Cargo.toml` and `Cargo.lock`
5. Commits: `chore(release): bump version to <version>`
6. Pushes the branch and opens a PR targeting `master`

### 3. CI runs on the PR

The CI workflow automatically runs the `check` job (pre-commit with all hooks). Branch protection requires this to pass before merging.

### 4. Merge the release PR

Merge via GitHub UI or CLI:

```bash
gh pr merge <pr-number> --merge
```

Use `--merge` (merge commit), not `--squash` or `--rebase`.

### 5. What happens after merge

The Publish & Sync workflow triggers automatically on push to `master`:

1. Runs `make check` (lint + tests)
2. Publishes the crate to crates.io
3. Creates a git tag `v<version>`
4. Creates a GitHub Release with auto-generated notes
5. Merges `master` back into `develop`

### 6. Pull locally

After the workflow completes:

```bash
git checkout develop
git pull
```

---

## Configuration Files

| File | Purpose |
|------|---------|
| `release.toml` | cargo-release config (commit message template, tag format) |
| `.github/workflows/ci.yml` | CI — pre-commit on push/PR |
| `.github/workflows/release-pr.yml` | Release PR creation |
| `.github/workflows/publish.yml` | Publish, tag, release, merge-back |

---

## Secrets

| Secret | Source | Notes |
|--------|--------|-------|
| `CARGO_REGISTRY_TOKEN` | crates.io/settings/tokens | Scope: `publish-update` |
| `GITHUB_TOKEN` | Built-in | No setup needed |

---

## Quick Reference

```text
RELEASE:
  gh workflow run "Create Release PR" -f bump=patch   # trigger
  gh run watch                                         # monitor
  # wait for CI to pass on the PR
  gh pr merge <number> --merge                         # merge to master
  # Publish & Sync runs automatically
  git checkout develop && git pull                     # sync local

BUMP OPTIONS:
  patch   0.1.0 → 0.1.1
  minor   0.1.0 → 0.2.0
  major   0.1.0 → 1.0.0
```
