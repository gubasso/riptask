# Cargo Release Workflow Setup — riptsk

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

- [ ] Push changes to `develop` so the `check` job runs at least once (required before branch protection)

```bash
git push origin develop
```

### 6. Set up `master` branch protection [YOU]

- [ ] `Settings → Branches → Add branch protection rule`
  1. Branch name pattern: `master`
  2. **Require a pull request before merging** (approvals = 0 for solo dev)
  3. **Require status checks to pass** — add the `check` job
  4. Optional: **Require branches to be up to date before merging**
  5. **Restrict who can push** (add no one → PR-only)
  6. Leave **Do not allow bypassing** unchecked (admin override)
  7. Save

### 7. Create crates.io API token [YOU]

- [ ] Go to [crates.io/settings/tokens](https://crates.io/settings/tokens)
- [ ] Create token named `github-actions-riptsk` with scope `publish-update`

### 8. First manual publish [YOU]

- [ ] `cargo login`
- [ ] `cargo publish --dry-run`
- [ ] `cargo publish`

First publish **cannot** be automated.

### 9. Add `CARGO_REGISTRY_TOKEN` to GitHub [YOU]

- [ ] `Settings → Secrets and variables → Actions → New repository secret`
  - Name: `CARGO_REGISTRY_TOKEN`
  - Value: token from step 7

### 10. Verify end-to-end [VERIFY]

- [ ] `cargo publish --dry-run` passes locally
- [ ] CI workflow runs on push to `develop`
- [ ] Direct push to `master` is rejected
- [ ] Trigger "Create Release PR" workflow via `workflow_dispatch`
- [ ] Merge the release PR → crate publishes, tag + release created, master merges back to develop
