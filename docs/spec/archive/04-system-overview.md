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
> | System architecture | `bin/tsk, lib/tsk/core.sh` |
>
> ---

# System Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                        your machines                            │
│                                                                 │
│  ~/code/project-a/    ~/code/project-b/    ~/code/project-c/   │
│       ↕ tsk                ↕ tsk                ↕ tsk          │
│                                                                 │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │             ~/.local/share/riptask/  (data repo)            │  │
│  │                                                          │  │
│  │  issues/          ← SoT for all issue content           │  │
│  │  templates/       ← issue and recurring task templates  │  │
│  │  riptask.yaml         ← config, registered remotes          │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                 │
│  ~/.cache/riptask/        ← views, sync state, id map (outside repo) │
│       ↕ git (for local-only issues + config)                   │
└──────────────────┬──────────────────────────────────────────────┘
                   │ tsk sync (glab/gh)
        ┌──────────┴──────────┐
        ▼                     ▼
   GitLab remotes       GitHub remotes
  (self-hosted or       (github.com or
   gitlab.com)           enterprise)
```

### Sync flow

```
Remote (gh/glab) ←──────── push ─────── $RIPTASK_REPO/issues/*.md
                 ────────── pull ──────► $RIPTASK_REPO/issues/*.md
```

### Multi-host flow

```
host-a/$RIPTASK_REPO ──── git push ────► gitolite/private remote
host-b/$RIPTASK_REPO ◄─── git pull ────  gitolite/private remote
```
