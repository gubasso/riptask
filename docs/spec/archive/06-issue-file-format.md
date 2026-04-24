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
> | Issue frontmatter schema | `lib/tsk/frontmatter.sh, lib/tsk/hooks/pre-commit, tests/unit/frontmatter.bats` |
>
> ---

# Issue File Format

Every issue is a single Markdown file. The frontmatter is the machine-readable contract and the single source of truth for all issue metadata. The body is human-readable content. The remote-readonly section is managed exclusively by `tsk sync pull`.

```markdown
---
id: WHL-042
local_id: ~                        # null once synced; populated before first push
title: Fix wormhole stabilizer retry logic
state: in-progress                 # backlog | todo | in-progress | review | done
board: penguin-chrono-labs              # which kanban board this belongs to
project: wormhole-router    # logical project grouping
org: penguin-chrono-labs                # org/team grouping
priority: high                     # low | medium | high | urgent
labels:
  - bug
  - temporal-drift
assignee: ppuffin
milestone: ~
cycle: 2026-Q1                     # optional cycle/sprint membership
order: 1                           # position within the kanban lane (integer, 1-based)
gitlab:
  repo: chrono/wormhole-router
  issue_id: 42
  url: https://gitlab.penguin-labs.io/chrono/wormhole-router/-/issues/42
  updated_at: 2026-03-13T10:22:00Z # remote's last updated_at (conflict resolution)
github: ~
local_updated_at: 2026-03-13T11:05:00Z   # updated on every local write
due: 2026-03-20
recurring: ~                       # null | ref to recur ID if this is a recur instance
---

## Description

Fix the temporal drift in the wormhole stabilizer's retry logic for era-hopping calls.
Confirmed reproducible on IceOS 4.2 under heavy penguin traffic.

## Tasks

- [x] Reproduce the issue
- [ ] Write regression test
- [ ] Fix and submit MR

---
<!-- TSK:REMOTE:START — managed by tsk sync pull, do not edit below this line -->

## Comments

**ppuffin** · 2026-03-12T09:14:00Z
Confirmed reproducible. Happens consistently under heavy traffic on IceOS 4.2.

**reviewer** · 2026-03-13T08:00:00Z
Looks like the chronon calculation is wrong in the stabilizer formula.

## Linked MRs

- !88 · Fix wormhole stabilizer · merged

<!-- TSK:REMOTE:END -->
```

### Frontmatter field reference

| Field | Type | Description |
|---|---|---|
| `id` | string | Final issue ID (`WHL-042`, `GH-017`). Set after first sync. |
| `local_id` | string\|null | Provisional ID before first push. Null after sync. |
| `title` | string | Issue title. Used in `tsk ls`, commit messages, sync, and fzf display. |
| `state` | enum | `backlog` \| `todo` \| `in-progress` \| `review` \| `done` |
| `board` | string | Kanban board name. Maps to `views/kanban/<board>/`. |
| `project` | string | Project grouping. Maps to `views/projects/<project>/`. |
| `org` | string\|null | Org/team grouping. Maps to `views/orgs/<org>/`. |
| `priority` | enum | `low` \| `medium` \| `high` \| `urgent` |
| `labels` | list | Freeform labels. Synced to remote labels. |
| `assignee` | string\|null | Username. |
| `milestone` | string\|null | Milestone name. |
| `cycle` | string\|null | Cycle/sprint name. Maps to `views/cycles/<cycle>/`. |
| `order` | int | Sort order within the kanban lane. 1-based. |
| `gitlab` | object\|null | GitLab-specific sync metadata. Null for non-GitLab projects. |
| `github` | object\|null | GitHub-specific sync metadata. Null for non-GitHub projects. |
| `local_updated_at` | datetime | ISO8601. Updated on every local write. Conflict resolution baseline. |
| `due` | date\|null | Due date. |
| `recurring` | string\|null | Recurrence ID if this issue was auto-generated from a recurring definition. |
| `remote_deleted` | bool\|null | Set to `true` when the synced remote issue has been deleted or transferred away. Excluded from kanban views, but still visible in `tsk ls --all`. |
| `conflict` | object\|null | Present only during an unresolved sync conflict. See below. |

### Conflict metadata

When `tsk sync pull` detects that both local and remote changed since the last sync, a `conflict:` object is added to the local file's frontmatter:

```yaml
conflict:
  detected_at: 2026-03-16T14:30:00Z
  remote_file: WHL-042.REMOTE.md
  remote_updated_at: 2026-03-16T12:00:00Z
  local_updated_at: 2026-03-15T09:00:00Z
  last_synced_at: 2026-03-14T08:00:00Z
```

| Field | Description |
|---|---|
| `detected_at` | When the conflict was detected |
| `remote_file` | Filename of the `.REMOTE.md` file containing the remote version |
| `remote_updated_at` | The remote's `updated_at` at conflict detection time |
| `local_updated_at` | The local `local_updated_at` at conflict detection time |
| `last_synced_at` | The `remote_state.json` baseline timestamp |

The `conflict:` field is removed on resolution via `tsk resolve`.

### `.REMOTE.md` convention

On conflict, the incoming remote version is written to `issues/<ID>.REMOTE.md`. This file has conflict metadata in frontmatter plus the full remote issue content (frontmatter fields and body):

```markdown
---
conflict_role: remote
conflict_parent: WHL-042
id: WHL-042
title: Fix wormhole stabilizer retry logic
state: review
board: penguin-chrono-labs
project: wormhole-router
org: penguin-chrono-labs
priority: high
labels:
  - bug
  - temporal-drift
assignee: ppuffin
milestone: ~
cycle: 2026-Q1
order: 1
gitlab:
  repo: chrono/wormhole-router
  issue_id: 42
  url: https://gitlab.penguin-labs.io/chrono/wormhole-router/-/issues/42
  updated_at: 2026-03-16T12:00:00Z
github: ~
local_updated_at: 2026-03-16T12:00:00Z
due: 2026-03-20
recurring: ~
remote_deleted: false
---

## Description

Remote version of the issue body, preserved verbatim for manual conflict resolution.
```

`.REMOTE.md` files are:
- Excluded from `tsk ls` output and all views
- Committed to `$RIPTASK_REPO` so conflict state survives host switches
- Deleted on conflict resolution via `tsk resolve`

### Remote metadata subobject

```yaml
gitlab:
  repo: chrono/wormhole-router    # namespace/project slug
  issue_id: 42                         # project-local numeric issue ID on remote (GitLab iid / GitHub number)
  url: https://gitlab.penguin-labs.io/.../42
  updated_at: 2026-03-13T10:22:00Z     # remote's last updated_at

github:
  repo: Penguin-Chrono-Labs/fish-from-the-future
  issue_id: 17
  url: https://github.com/.../17
  updated_at: 2026-03-10T08:00:00Z
```

### State → GitLab label mapping

`tsk sync push` manages only `status::*` scoped labels. It never does a full label replace — non-status labels managed outside `tsk` are preserved.

| `state` | GitLab label |
|---|---|
| `backlog` | `status::backlog` |
| `todo` | `status::todo` |
| `in-progress` | `status::in-progress` |
| `review` | `status::review` |
| `done` | issue closed + `status::done` |

### Remote-readonly section

The `<!-- TSK:REMOTE:START -->` ... `<!-- TSK:REMOTE:END -->` block is:
- Written and updated exclusively by `tsk sync pull`
- Never edited by hand (edits are silently overwritten on next pull)
- Contains: comments, linked MRs, CI status, milestone details — anything that doesn't map cleanly to frontmatter
- Safe to ignore entirely if you don't care about it
