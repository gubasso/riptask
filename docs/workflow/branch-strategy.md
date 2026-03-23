# Branch Strategy

Branch model using `develop` as integration and `master` as release.

---

## Branch Roles

```text
master              ← production / release, protected, PR-only
  └── develop       ← integration branch, receives feature PRs
       ├── feat/*   ← new functionality
       ├── fix/*    ← bug fixes
       └── chore/*  ← refactors, CI, docs, dependencies
```

- **`master`** — only receives release PRs from `develop`. Every commit is a tagged release. Protected by branch rules and pre-commit hooks.
- **`develop`** — default branch. Never commit directly — all changes arrive via merged PRs from implementation branches.
- **Implementation branches** — short-lived, branched from `develop`, always rebased before merge. Named by the forge (e.g., `42-add-auth-module`).

## Flow

```text
feature/* → (rebase) → develop → (release PR) → master → (auto-merge back) → develop
```

## Rules

- No direct pushes to `master` or `develop`.
- Implementation branches always rebase onto `develop`. See [Rebase Guide](rebase-guide.md).
- Release PRs are the only path to `master`.
- `master` always merges back to `develop` after release (automated by CI).
- Every commit on `develop` should compile and pass tests.

## pre-commit Enforcement

The `no-commit-to-branch` hook in `.pre-commit-config.yaml` blocks direct commits to both `develop` and `master`:

```yaml
- id: no-commit-to-branch
  args: [--branch, develop, --branch, master]
```

## Branch Protection (GitHub)

`master` is protected with:

- Require a pull request before merging (0 approvals for solo dev)
- Require status checks to pass (`check` job from CI)
- Optionally require branches to be up to date before merging
