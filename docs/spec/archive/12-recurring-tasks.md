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
> | Recurring task instantiation | `lib/tsk/recur.sh, tests/integration/recur.bats` |
>
> ---

# Recurring Tasks

### Definition

Recurring tasks are definitions in `tsk.yaml` that auto-generate new issues on a schedule. They are not issues themselves — they are templates + scheduling metadata.

### `tsk.yaml` recurring definition

```yaml
recurring:
  - id: recur-weekly-review
    template: templates/weekly-review.md
    title_pattern: "Weekly review — {YYYY-MM-DD}"
    board: personal
    project: personal
    org: ~
    state: todo
    priority: medium
    assignee: ppuffin
    labels: [recurring, review]
    frequency: weekly              # daily | weekly | monthly | yearly
    day_of_week: monday            # for weekly frequency
    day_of_month: ~                # for monthly frequency (1-28)
    start: 2026-03-17
    end: ~                         # null = indefinite
    last_run: ~                    # updated by tsk recur run
```

### Instantiation (`tsk recur run`)

For each recurring definition where `(last_run + frequency) <= today`:

1. Copy template file to `issues/<ID>.md`, where ID is allocated using the same logic as `tsk new`: `LOCAL-<hash>` for GitHub/GitLab projects (provisional until `tsk sync push`), or `<PREFIX>-<sequence>` for local projects (permanent)
2. Substitute `{YYYY-MM-DD}`, `{date}`, `{week}` in title and body
3. Set all frontmatter fields from definition
4. Set `recurring: <recur-id>` in frontmatter
5. Update `last_run: <today>` in `tsk.yaml`
6. Regenerate views

`tsk recur run` is idempotent — safe to run multiple times per day. It checks `last_run` and only creates instances that are actually due.

### Multi-host dedup

**Race condition**: If both hosts run `tsk recur run` before syncing `tsk.yaml` via `$TSK_REPO` git, both see `last_run: null` (or same old date) and instantiate the same recurring task — creating duplicates.

**Dedup strategy**: Before creating an instance, check if `issues/` already contains a file with `recurring: <recur-id>` and a title matching the current period's `title_pattern` expansion. If a match exists, update `last_run` in `tsk.yaml` without creating a duplicate. This makes `tsk recur run` idempotent across hosts, not just within a single host.

**Git merge of `last_run`**: When `tsk.yaml` is merged across hosts, conflicting `last_run` values should be resolved by keeping the later date (both hosts created the same instance — the later `last_run` is correct).

### Cron setup

```bash
# ~/.config/systemd/user/tsk-recur.service
[Unit]
Description=tsk recurring task instantiation

[Service]
Type=oneshot
ExecStart=%h/.local/bin/tsk recur run

# ~/.config/systemd/user/tsk-recur.timer
[Timer]
OnCalendar=Mon..Fri 09:00
Persistent=true

[Install]
WantedBy=timers.target
```

Or a simple crontab entry:

```
0 9 * * 1-5 tsk recur run
```
