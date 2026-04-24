# Cargo Release Workflow Setup — riptask

## Context

Full cargo-release workflow: Cargo.toml metadata, `release.toml`, GitHub Actions CI/CD, branch protection, pre-commit updates, and first manual publish.

Refs: `~/.dotfiles/_docs/development/languages/rust/cargo-release-setup.md` and linked docs.

---

## Checklist

Legend: **[LLM]** = Claude · **[YOU]** = manual · **[VERIFY]** = check together

### 1. Cargo.toml metadata [LLM] — DONE

- [x] Added `description`, `license`, `repository` fields

### 2. Create `release.toml` [LLM] — DONE

- [x] Created `release.toml` with commit message template, tag format, publish, push

### 3. Add `master` to `no-commit-to-branch` [LLM] — DONE

- [x] Updated `.pre-commit-config.yaml` to block commits to both `develop` and `master`

### 4. Create GitHub Actions workflows [LLM] — DONE

- [x] `.github/workflows/ci.yml` — pre-commit on push/PR to develop and master
- [x] `.github/workflows/release-pr.yml` — workflow_dispatch, bumps version, opens PR to master
- [x] `.github/workflows/publish.yml` — on push to master: publish, tag, release, merge back

### 5. Push to `develop` to trigger CI [YOU]

- [x] Push changes to `develop` so the `check` job runs at least once (required before branch protection)
  - created a pr, check job ran, all green, merged with develop, now created a new branch 6-continue-release-setup to continue this implementation

```bash
git push origin develop
```

### 6. Set up `master` branch protection [YOU]

- [x] `Settings → Branches → Add branch protection rule`
  1. Branch name pattern: `master`
  2. **Require a pull request before merging** (approvals = 0 for solo dev)
  3. **Require status checks to pass** — add the `check` job
  4. Optional: **Require branches to be up to date before merging**
  5. **Restrict who can push** (add no one → PR-only)
  6. Leave **Do not allow bypassing** unchecked (admin override)
  7. Save

### 7. Create crates.io API token [YOU]

- [x] Go to [crates.io/settings/tokens](https://crates.io/settings/tokens)
- [x] Create token named `github-actions-riptask` with scope `publish-update`

### 8. First manual publish [YOU]

- [x] `cargo login`
- [x] `cargo publish --dry-run`
- [x] `cargo publish`

First publish **cannot** be automated.

### 9. Add `CARGO_REGISTRY_TOKEN` to GitHub [YOU]

- [x] `Settings → Secrets and variables → Actions → New repository secret`
  - Name: `CARGO_REGISTRY_TOKEN`
  - Value: token from step 7

### 10. Verify end-to-end [VERIFY]

- [x] `cargo publish --dry-run` passes locally

#### 10.1 Trigger Release PR [YOU]

1. Go to GitHub → `Actions` → `Create Release PR` workflow
2. Click `Run workflow`
3. Select branch: `develop`
4. Set bump to `patch` (will bump `0.1.0` → `0.1.1`)
5. Click `Run workflow`

- [ ] Workflow completes successfully
- [ ] A `release/v0.1.1` branch is created
- [ ] A PR titled "Release v0.1.1" is opened targeting `master`

#### 10.2 Verify CI on the release PR [VERIFY]

- [ ] CI `check` job runs on the PR and passes

#### 10.3 Merge the release PR [YOU]

1. Merge the PR into `master` (use merge commit, not squash)

- [ ] Merge succeeds

#### 10.4 Verify Publish & Sync [VERIFY]

After merge, the `Publish & Sync` workflow triggers automatically. Check:

- [ ] `just check` passes in the workflow
- [ ] Crate published to crates.io (check crates.io/crates/riptask)
- [ ] Git tag `v0.1.1` created on the repo
- [ ] GitHub Release `v0.1.1` created with auto-generated notes
- [ ] `master` merged back into `develop` automatically
- [ ] `develop` Cargo.toml version is now `0.1.1`
