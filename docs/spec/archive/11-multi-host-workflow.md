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
> | Multi-host sessions | `lib/tsk/sync.sh, tests/integration/session.bats` |
>
> ---

# Multi-Host Workflow

### Two distinct sync layers

| Category | Source of Truth | Cross-host mechanism |
|---|---|---|
| GitHub-synced issues | GitHub | `tsk sync` via `gh` — not $RIPTASK_REPO git |
| GitLab-synced issues | GitLab | `tsk sync` via `glab` — not $RIPTASK_REPO git |
| Local project issues (non-gh/glab remote or no remote) | `$RIPTASK_REPO/issues/*.md` | `$RIPTASK_REPO` git push/pull |
| Local-only issues (not tied to any project) | `$RIPTASK_REPO/issues/*.md` | `$RIPTASK_REPO` git push/pull |
| Config | `$RIPTASK_REPO/config.yaml` | `$RIPTASK_REPO` git push/pull |

**`$RIPTASK_REPO` git is a backup and transport layer, not the sync mechanism for GitHub/GitLab-synced issues.** Remote-synced issue files do exist in `$RIPTASK_REPO` and are committed/pushed via git, but this is for backup and host migration — not for synchronization. Each host must independently run `tsk sync pull` against the remote to receive updates. Never rely on `$RIPTASK_REPO` git pull to get the latest GitHub/GitLab-synced issue state.

**For local projects** (non-GitHub/GitLab remotes like codeberg, gitolite, etc., or repos with no remote), `$RIPTASK_REPO` git **is** the sync mechanism — it is the only way to share these issues across hosts.

### Work partitioning strategies

Working on multiple hosts simultaneously is safe as long as hosts don't edit the same issues concurrently. Two practical strategies, from least to most restrictive:

1. **Task-per-host** — each host works on different tasks. No two hosts touch the same issue file, so sync push never races and `$RIPTASK_REPO` git merges are always clean (different files on each side).
2. **Project-per-host** — each host works on a different project entirely. The issue namespace is disjoint by definition, making even accidental overlap impossible.

Either strategy eliminates the conflict scenario described below. The conflict section remains relevant only if the same issue is edited on multiple hosts between syncs.

### What sessions provide

`tsk session start/end` is useful regardless of how many hosts are in play:

| Scenario | `session start` | `session end` |
|---|---|---|
| **Single host** | Pulls latest remote state (issues updated via GitLab/GitHub web UI, by teammates, or by CI) | Pushes local changes to remotes; commits and backs up `$RIPTASK_REPO` |
| **Multiple hosts** | All of the above, plus: receives local-only issues and config changes made on other hosts | All of the above, plus: shares local-only issues and config with other hosts via `$RIPTASK_REPO` git |

Even on a single host, sessions keep the local view fresh against remote changes made outside `tsk` and ensure work is backed up.

### Session discipline

The ordering on session end is strict:

```
tsk sync push    →    git push $RIPTASK_REPO
```

Never reversed. If you `git push $RIPTASK_REPO` before `tsk sync push`, host B will pull local edits that haven't been pushed to the remote yet. When host B runs `tsk sync pull`, the remote may have a newer version — creating a divergence.

### Standard multi-host flow

```bash
# start of session (any host)
tsk session start
# internally:
#   git -C $RIPTASK_REPO pull           → receive local-only issues from other host
#   tsk sync pull                  → receive remote-synced issues from gh/glab

# ... work normally ...

# end of session
tsk session end
# internally:
#   tsk sync push                  → push dirty remote-synced issues
#   git -C $RIPTASK_REPO add -A
#   git -C $RIPTASK_REPO commit -m "riptask: session end $(hostname) $(date +%Y-%m-%d)"
#   git -C $RIPTASK_REPO push          → share local-only issues + config to other host
```

`session end` intentionally stages the entire `$RIPTASK_REPO` with `git add -A`. That is broader than lifecycle auto-commit from [15 — Version Control & Backup](15-version-control-backup.md), but safe here because cache lives outside the repo and `$RIPTASK_REPO` only contains issue data, templates, config, and the comment-only `.gitignore`.

### Conflict scenario (same issue on multiple hosts)

```
host-a: tsk sync pull → gets WHL-041 at state X
         remote_state.json[WHL-041].updated_at = T0
host-b: tsk sync pull → gets WHL-041 at state X
         remote_state.json[WHL-041].updated_at = T0

host-a: tsk move WHL-041 in-progress → local_updated_at = T2 (T2 > T0)
host-b: tsk move WHL-041 review, tsk sync push → remote.updated_at = T3 (T3 > T0)

host-a: tsk sync pull
→ remote.updated_at (T3) > last_synced (T0): remote changed
→ local.local_updated_at (T2) > last_synced (T0): local changed
→ CONFLICT detected
→ issues/WHL-041.REMOTE.md created with remote version
→ issues/WHL-041.md gets conflict: metadata in frontmatter
→ exit code 3
```

Resolution flow:

```bash
# option 1: review both versions and manually edit local file
cat issues/WHL-041.REMOTE.md    # see what the remote version looks like
tsk edit WHL-041                # incorporate changes into local file
tsk resolve WHL-041             # clean up conflict state

# option 2: accept remote version wholesale
tsk resolve WHL-041 --take-remote

# option 3: keep local version, discard remote
tsk resolve WHL-041 --take-local
```

Prevention: **push before switching hosts**. `tsk session end` enforces this as a habit. Alternatively, adopt a task-per-host or project-per-host partitioning strategy (see above) to avoid this scenario entirely.

### Failure modes and recovery

**Git merge conflict in `$RIPTASK_REPO`.** When two hosts both modify `config.yaml` or create local-only issues with overlapping filenames, `git pull` in `tsk session start` may hit a merge conflict. Recovery: standard git merge resolution (`git mergetool` or manual edit), then re-run `tsk session start`. Since `config.yaml` is structured YAML, conflicts are usually in `recurring[].last_run` or newly added `remotes[]` entries — resolvable by keeping the later value.

**Forgotten `session end`.** If you switch hosts without running `session end`, local-only issues and config changes stay on host A. Remote-synced changes are also unpushed. Recovery: run `tsk session end` on host A when you return, or manually `tsk sync push && git -C $RIPTASK_REPO add -A && git commit && git push`. If host B has since made changes, the git pull on next `session start` will merge them (may conflict — see above). Note: auto-commit on lifecycle events (see [spec 15](15-version-control-backup.md)) reduces the blast radius for local issues — lifecycle changes (`new`, `close`, `move`, `reopen`, `rm`) are already committed, so only deferred changes (edits, comments, tags) and the git push are lost.

**Partial `tsk sync push` failure.** If `tsk sync push` fails mid-way (network error, auth expired), some issues are pushed and some are not. Recovery: re-run `tsk sync push` — it is idempotent (only pushes issues where `local_updated_at > remote.updated_at`). Already-pushed issues will be skipped.

**`git push $RIPTASK_REPO` before `tsk sync push`.** Violates session discipline. Host B receives local edits not yet pushed to the remote. When host B runs `tsk sync pull`, remote may overwrite those edits (or detect a conflict if conflict detection is enabled). Recovery: on host A, run `tsk sync push` to push remaining changes to the remote.

### $RIPTASK_REPO git remote options

In order of recommended privacy:

1. **Self-hosted gitolite** — minimal footprint, SSH-only, no web UI needed, full control
2. **Self-hosted Forgejo** — more setup, adds web browsing, ~200MB RAM
3. **Private GitLab repo** — on `gitlab.penguin-labs.io` (already within Penguin Labs infra)
4. **git-crypt + GitHub private** — GitHub stores encrypted blobs, GPG key required to decrypt
5. **Local only + restic** — no remote git, offsite backup via existing restic pipeline

Recommendation for this use case: **gitolite on an existing VPS or home server**, or **private repo on `gitlab.penguin-labs.io`** (already within trusted infra).

Note: `$RIPTASK_REPO/issues/*.md` will contain internal hostnames, project names, and work context. Do not push to a public repo or any untrusted third-party host without encryption.
