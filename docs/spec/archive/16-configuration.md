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
> | riptask.yaml loading and structure | `lib/tsk/core.sh, tests/integration/config.bats` |
>
> ---

# Configuration

`riptask.yaml` lives at the root of `$RIPTASK_REPO`. It is git-versioned. It must not contain credentials or tokens — authentication is managed entirely by `glab` and `gh`.

```yaml
# $RIPTASK_REPO/riptask.yaml
version: 1

# Default values applied to new issues when not specified
defaults:
  board: personal
  state: todo
  priority: medium
  assignee: ppuffin
  template: task                       # default template for tsk new (filename stem)

# Registered remote repositories
remotes:
  - name: wormhole-router
    type: gitlab
    host: https://gitlab.penguin-labs.io       # omit for gitlab.com
    repo: chrono/wormhole-router
    project_prefix: WHL                # must be unique across all remotes
    default_board: penguin-chrono-labs
    default_org: penguin-chrono-labs

  - name: fish-from-the-future
    type: github
    repo: Penguin-Chrono-Labs/fish-from-the-future
    project_prefix: FSH
    default_board: penguin-chrono-labs
    default_org: penguin-chrono-labs

  - name: igloo-scripts
    type: github
    repo: ppuffin/igloo-scripts
    project_prefix: IGL
    default_board: personal
    default_org: ~

  - name: snowflakes
    type: local                        # non-GitHub/GitLab remote (or no remote)
    host: https://codeberg.org         # optional — absent if no remote
    repo: ppuffin/snowflakes            # optional — absent if no remote
    path: ~                            # optional for remote-backed local projects; required when repo is absent
    project_prefix: SNO
    default_board: personal
    default_org: ~

# Kanban board definitions
boards:
  - name: penguin-chrono-labs
    states: [backlog, todo, in-progress, review, done]
  - name: personal
    states: [backlog, todo, in-progress, done]

# UI preferences
ui:
  opener: "nvim -R"                    # used by tsk board --open
  tree_depth: 2                        # depth for tsk board (tree command)
  fzf_opts: "--height 40% --border"    # extra flags appended to every fzf invocation

# AI configuration
ai:
  enabled: false
  model: claude-haiku-4-5-20251001
  features:
    new_body_gen: true
    triage: true
    summarize: true
    ask: true

# Sync behavior
sync:
  conflict_detection: true         # default: true — detect and preserve conflicts
                                   # when false: uses last-write-wins (remote overwrites local)
                                   # can also be bypassed per-pull with tsk sync pull --force

# Recurring task definitions
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
    frequency: weekly
    day_of_week: monday
    start: 2026-03-17
    end: ~
    last_run: ~
```

### Validation rules

On config load, `tsk` validates `riptask.yaml` and aborts with a clear error if any rule is violated:

- **`project_prefix` uniqueness** — no two entries in `remotes[]` may share the same `project_prefix`. Prefixes are the namespace that prevents ID collisions across projects; duplicates would make issue IDs ambiguous. See [09 — Project Detection & Registration](09-project-detection-registration.md#prefix-uniqueness-validation) for enforcement details.
- **`name` uniqueness** — no two entries in `remotes[]` may share the same `name`.
- **`boards[].name` uniqueness** — no two board definitions may share the same name.
- **No-remote local lookup** — if a `type: local` entry omits `repo` (or sets it to `~`), it must set `path` to the canonical absolute repo path so `tsk` can re-detect the project from `$PWD`. See [09 — Project Detection & Registration](09-project-detection-registration.md#no-remote-project-lookup).

### Global `tsk` config (machine-local, not in repo)

```bash
# ~/.config/riptask/config.env  ($XDG_CONFIG_HOME/riptask/config.env)
RIPTASK_REPO=~/.local/share/riptask
RIPTASK_AI_MODEL=claude-haiku-4-5-20251001
```

Or set via environment:

```bash
# ~/.bashrc
export RIPTASK_REPO=~/.local/share/riptask
```

### XDG Base Directory layout

`tsk` follows the [XDG Base Directory Specification](https://specifications.freedesktop.org/basedir-spec/latest/):

| XDG variable | Default | tsk path | Contents |
|---|---|---|---|
| `$XDG_DATA_HOME` | `~/.local/share` | `~/.local/share/riptask/` | The git repo (`$RIPTASK_REPO`) — issues, templates, riptask.yaml |
| `$XDG_CONFIG_HOME` | `~/.config` | `~/.config/riptask/config.env` | Machine-local config (not in git repo) |
| `$XDG_CACHE_HOME` | `~/.cache` | `~/.cache/riptask/` | Derived data — **views/**, remote_state.json, id_map.json, index.json |

### `$RIPTASK_REPO` resolution order

1. `$RIPTASK_REPO` environment variable (if set)
2. `RIPTASK_REPO` value in `$XDG_CONFIG_HOME/riptask/config.env` (i.e. `~/.config/riptask/config.env`)
3. Fallback: `$XDG_DATA_HOME/riptask` (i.e. `~/.local/share/riptask/`)
