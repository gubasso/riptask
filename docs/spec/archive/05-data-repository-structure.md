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
> | XDG layout | `lib/tsk/core.sh, tests/unit/core.bats` |
>
> ---

# Data Repository Structure

### Full XDG layout

```
~/.local/share/riptask/                    # $XDG_DATA_HOME/riptask — the git repo ($RIPTASK_REPO)
├── issues/
├── templates/
├── riptask.yaml
└── .gitignore

~/.config/riptask/config.env                  # $XDG_CONFIG_HOME/riptask — machine-local config

~/.cache/riptask/                          # $XDG_CACHE_HOME/riptask — derived/transient data
├── views/                             # generated view tree (file copies from $RIPTASK_REPO/issues/)
├── remote_state.json
├── id_map.json
└── index.json
```

### Data repository (`$RIPTASK_REPO`)

```
~/.local/share/riptask/
├── issues/                            # canonical issue store (git-versioned)
│   ├── WHL-042.md                     # synced, ID from GitLab
│   ├── FSH-017.md                     # synced, ID from GitLab
│   ├── GH-008.md                      # synced, ID from GitHub
│   └── LOCAL-3f2a.md                  # not yet synced, provisional ID
│
├── templates/                         # issue and recurring task templates (git-versioned)
│   ├── bug.md                         # users can add custom templates here
│   ├── feature.md                     # (see spec 24 — Templates for management CLI)
│   ├── task.md
│   └── weekly-review.md
│
├── riptask.yaml                           # configuration (git-versioned)
└── .gitignore
```

### Cache directory (`~/.cache/riptask/`)

```
~/.cache/riptask/
├── views/                             # generated view tree
│   ├── kanban/
│   │   ├── penguin-chrono-labs/
│   │   │   ├── backlog/
│   │   │   │   ├── 01-WHL-039.md
│   │   │   │   └── 02-WHL-040.md
│   │   │   ├── todo/
│   │   │   │   ├── 01-WHL-041.md
│   │   │   │   └── 02-FSH-017.md
│   │   │   ├── in-progress/
│   │   │   │   └── 01-WHL-042.md
│   │   │   ├── review/
│   │   │   └── done/
│   │   │       └── 01-WHL-037.md
│   │   └── personal/
│   │       ├── todo/
│   │       │   └── 01-LOCAL-3f2a.md
│   │       └── ...
│   │
│   ├── projects/
│   │   ├── wormhole-router/
│   │   │   ├── WHL-041.md
│   │   │   └── WHL-042.md
│   │   └── fish-from-the-future/
│   │       └── FSH-017.md
│   │
│   ├── orgs/
│   │   └── penguin-chrono-labs/
│   │       ├── WHL-042.md
│   │       └── FSH-017.md
│   │
│   └── cycles/
│       └── 2026-Q1/
│           ├── 01-WHL-041.md
│           └── 02-WHL-042.md
│
├── remote_state.json                  # last known remote snapshot (diff baseline)
├── id_map.json                        # LOCAL-xxxx → WHL-NNN after sync
└── index.json                         # reconstructible query index
```

The cache directory lives outside the repo entirely per XDG Base Directory Specification. It is never committed or gitignored — it simply does not exist within `$RIPTASK_REPO`. All cache contents are reconstructible from `issues/*.md` and remote state, with the exception of `id_map.json` entries for already-synced issues (see [20 — Open Questions](../20-open-questions.md)). Views are **file copies** of `$RIPTASK_REPO/issues/<ID>.md` — disposable and always regeneratable via `tsk view`. Kanban lane directories are materialized from board config, so the full lane structure is present even when lanes are empty. Cache is per-host by design — each host maintains its own views, `remote_state.json`, `id_map.json`, and `index.json`. These files are never shared across hosts. Each host independently builds its cache from `issues/*.md` and remote API responses. This is correct behavior: `remote_state.json` tracks what *this host* last saw from the remote, which may differ from what another host last saw.

### `.gitignore`

```gitignore
# Views have moved to $XDG_CACHE_HOME/riptask/views/ — nothing to gitignore.
```

`tsk init` creates this file anyway as a comment-only marker. It makes the intentional emptiness explicit: cache and derived state live outside `$RIPTASK_REPO`, so there are no ignore rules to maintain inside the repo itself.
