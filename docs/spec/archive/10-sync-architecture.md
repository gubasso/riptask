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
> | Sync pull/push/conflict | `lib/tsk/sync.sh, lib/tsk/remote_*.sh, tests/sync/*.bats` |
>
> ---

# Sync Architecture

### Principles

- Sync is **always explicit** — never runs in the background, never auto-triggered
- `tsk sync` requires `glab` (for GitLab) and/or `gh` (for GitHub) installed and authenticated
- `tsk` does not manage authentication — delegated entirely to `glab`/`gh`
- The default sync scope when inside a project repo is **that project only**
- `--all` flag overrides scope to all configured remotes
- **Local projects** (`type: local` — non-GitHub/GitLab remotes or no remote) are **skipped by `tsk sync`**. They have no remote issue tracker to sync against. Cross-host sharing for local projects is handled entirely via `$TSK_REPO` git push/pull

### Pull algorithm

```
for each remote in scope:
    issues = glab issue list --repo <repo> --all
             OR gh issue list --repo <repo> --state all

    for each remote_issue in issues:
        key = "<type>:<repo>:<issue_id>"
        local_file = find issues/ where gitlab.issue_id == remote_issue.iid
                                      OR github.issue_id == remote_issue.number
        last_synced = remote_state.json[key].updated_at   # baseline from last sync

        if local_file not found:
            # new issue from remote
            id = "<PREFIX>-<remote_issue.iid>"     # GitLab
                 OR "<PREFIX>-<remote_issue.number>"  # GitHub
            create issues/<id>.md with:
              - frontmatter from remote data
              - body from remote description
              - TSK:REMOTE section from comments + MRs

        else if local_file.frontmatter.conflict is not null:
            # already in conflict — do not stack
            warn "<ID>: conflict unresolved, skipping"

        else if last_synced is not null
                AND remote_issue.updated_at > last_synced
                AND local_file.frontmatter.local_updated_at > last_synced:
            # CONFLICT: both sides changed since last sync
            write issues/<ID>.REMOTE.md with:
              - conflict_role: remote
              - conflict_parent: <ID>
              - full remote frontmatter and body
            add conflict: object to local_file frontmatter:
              - detected_at: now
              - remote_file: <ID>.REMOTE.md
              - remote_updated_at: remote_issue.updated_at
              - local_updated_at: local_file.frontmatter.local_updated_at
              - last_synced_at: last_synced
            warn "<ID>: CONFLICT — both local and remote changed"
            set exit_code = 3

        else if remote_issue.updated_at > last_synced
                OR (last_synced is null
                    AND remote_issue.updated_at > local_file.frontmatter.local_updated_at):
            # clean pull: remote changed, local did not (or no baseline — fallback to last-write-wins)
            patch frontmatter: title, state, labels, assignee, milestone
            replace body from remote description
            replace TSK:REMOTE section
            do NOT update local_updated_at (preserve local timestamp)

        else:
            # no remote change, or local is newer: skip, push will handle
            pass

    for each local_file with this remote's repo where no matching remote issue:
        if issue was previously synced (has remote issue_id):
            # issue deleted or transferred on remote
            set remote_deleted: true in frontmatter
            # do NOT delete local file — data is preserved

update $XDG_CACHE_HOME/tsk/remote_state.json
regenerate views/
```

When `--force` is passed, the conflict detection branch is skipped entirely and the pull uses last-write-wins (remote overwrites local if remote is newer). Existing unresolved conflicts are still skipped.

After writing `.REMOTE.md` and adding `conflict:` metadata, `tsk sync pull` commits these files to `$TSK_REPO` with `TSK_HOOK_ALLOW_REMOTE=1` (see [25 — Hooks](25-tsk-repo-hooks.md)).

### Push algorithm

```
dirty_issues = issues/ where local_updated_at > gitlab.updated_at
                            OR github.updated_at
               AND remote_deleted != true
               AND conflict is null          # never push conflicted issues

for each issue in dirty_issues:
    if local_id is set (never pushed):
        # first push: create on remote
        result = glab issue create \
            --repo <repo> \
            --title "<title>" \
            --description "<body>" \
            --label "status::<state>,<labels...>" \
            --assignee "<assignee>"
        remote_id = result.iid       # GitLab
                    OR result.number # GitHub
        new_filename = "<PREFIX>-<remote_id>.md"
        rename: issues/LOCAL-xxxx.md → issues/<new_filename>
        update frontmatter: id, local_id=null, gitlab.issue_id OR github.issue_id, remote URL, remote updated_at
        update $XDG_CACHE_HOME/tsk/id_map.json: LOCAL-xxxx → <PREFIX>-<remote_id>
        update cross-references in other issue files

    else:
        # subsequent push: update on remote
        glab issue update <issue_id> \
            --repo <repo> \
            --title "<title>"
        glab issue label <issue_id> --repo <repo> \
            --add "status::<state>" \
            --remove "status::*" (all other status labels)
        if state == done:
            glab issue close <issue_id> --repo <repo>
        if state != done AND remote is closed:
            glab issue reopen <issue_id> --repo <repo>
        update frontmatter: gitlab.updated_at from response

update $XDG_CACHE_HOME/tsk/remote_state.json
regenerate views/
```

### Remote state cache

`~/.cache/tsk/remote_state.json` — the sync baseline for conflict detection and the diff baseline for `tsk sync status`.

```json
{
  "gitlab:chrono/wormhole-router:42": {
    "title": "Fix wormhole stabilizer",
    "state": "opened",
    "labels": ["status::in-progress", "bug"],
    "assignee": "ppuffin",
    "updated_at": "2026-03-13T10:22:00Z"
  },
  "github:Penguin-Chrono-Labs/fish-from-the-future:17": {
    "title": "Investigate temporal image drift",
    "state": "open",
    "labels": ["enhancement"],
    "assignee": "ppuffin",
    "updated_at": "2026-03-10T08:00:00Z"
  }
}
```

### ID map cache

`~/.cache/tsk/id_map.json` — tracks local-to-remote ID assignments for the file rename flow.

```json
{
  "LOCAL-3f2a": "WHL-043",
  "LOCAL-a81c": "GH-018"
}
```

### Cross-reference integrity on rename

When a local issue (`LOCAL-3f2a`) is assigned a real ID (`WHL-043`) after first push, `tsk` scans all `issues/*.md` for references to `LOCAL-3f2a` in frontmatter fields (e.g. `blocks:`, `related:`) and updates them to `WHL-043`. This scan happens at rename time before the git commit.

### Label safety rule

On push, `tsk` only manages `status::*` scoped labels. It:
- Removes the old `status::*` label
- Adds the new `status::*` label
- Never touches any other label

Labels set via GitLab/GitHub directly (e.g. `bug`, `priority::high`) are preserved and reflected in the local `labels:` frontmatter array on next pull, but `tsk` never removes them on push.
