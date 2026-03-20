> **[ARCHIVED]** — This spec was the design input for implementation. Code + tests are
> now the source of truth for this behavior. This document is retained as historical
> record only.
>
> - **Status:** archived
> - **Date archived:** 2026-03-18
> - **Source of truth:** code + tests
>
> | Concern | Source of Truth |
> |---------|----------------|
> | Auto-commit and backup | `lib/tsk/sync.sh, tests/integration/session.bats` |
>
> ---

# Version Control & Backup

### What to version

```
~/.local/share/tsk/  ($TSK_REPO)
├── issues/        ✅ version — SoT for all issue content
├── templates/     ✅ version — SoT for templates
└── tsk.yaml       ✅ version — config, registered remotes (no secrets)

~/.cache/tsk/      (outside repo — not versioned, not gitignored, just absent)
├── views/         generated view tree (file copies from $TSK_REPO/issues/)
├── remote_state.json
├── id_map.json
└── index.json
```

### Commit discipline

**Auto-commit on lifecycle events for local issues.** When a lifecycle command (`new`, `close`, `move`, `reopen`, `rm`) affects only local issues (belonging to a project without a GitHub/GitLab remote, or not tied to any project), `tsk` auto-commits `$TSK_REPO` immediately after the operation. The commit message follows the conventions below.

**Defer for everything else.** Synced issues (gh/glab), trivial mutations (`edit`, `comment`, `tag`), and bulk operations are not auto-committed. These are batched and committed explicitly via `tsk commit` or `tsk session end`.

Auto-commit does NOT auto-push — the same constraint as `tsk commit`. Pushing is always explicit (`git push` or `tsk session end`).

Commit message conventions:

```
tsk: new ICE-043 — investigate temporal drift
tsk: close ICE-042 — wormhole stabilizer fix
tsk: move ICE-041 → in-progress
tsk: reopen ICE-040
tsk: rm ICE-039
tsk: sync 2026-03-14 (5 pulled, 2 pushed)
tsk: session end host-a 2026-03-14
```

### Remote options

| Option | Privacy | Complexity | Notes |
|---|---|---|---|
| gitolite on own server | ✅ Full | Low | SSH-only, minimal footprint, recommended |
| Private repo on gitlab.penguin-labs.io | ✅ Within Penguin Labs infra | Zero | Already trusted infra |
| Self-hosted Forgejo | ✅ Full | Medium | Adds web UI, ~200MB RAM |
| git-crypt + GitHub private | ⚠️ Encrypted blobs | Low-Medium | GitHub stores ciphertext |
| Local only + restic | ✅ Full | Zero | No remote git, offsite via restic |

**Recommendation:** gitolite on an existing server, or a private repo on `gitlab.penguin-labs.io`.

**Warning:** `issues/*.md` will contain internal hostnames, project names, and work context from Penguin Chrono Labs infrastructure. Do not push to a public or untrusted third-party host without encryption at rest.

### git-crypt setup (if using GitHub/untrusted remote)

```bash
git-crypt init
echo "issues/** filter=git-crypt diff=git-crypt" >> .gitattributes
echo "tsk.yaml filter=git-crypt diff=git-crypt" >> .gitattributes
git-crypt add-gpg-user YOUR_GPG_KEY_ID
# from this point: normal git workflow, GitHub stores ciphertext
```
