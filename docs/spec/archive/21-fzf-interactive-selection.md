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
> | fzf interactive pickers | `lib/tsk/fzf.sh, tests/integration/fzf.bats` |
>
> ---

# fzf Interactive Selection

When any ID-taking command is invoked without `<ID>` and stdin is a TTY, `tsk` opens an fzf picker for interactive selection. When stdin is not a TTY (piped or scripted), a missing `<ID>` is an error (exit 1 with usage message). This preserves scriptability.

### Shared module: `lib/fzf.sh`

Two main capabilities:

- **`fzf_pick_issue`** — selects from `$RIPTASK_REPO/issues/*.md`
- **`fzf_pick_enum`** — selects from a fixed list (states, etc.)

Both functions return the selected value on stdout or exit 1 if the user cancels (Esc / Ctrl-C).

### Issue picker display format

Columnar, tab-delimited:

```
ID  PRIORITY  STATE  PROJECT  TITLE
```

- All visible fields are searchable
- Labels and assignee are included as hidden search fields (appended after a null byte so fzf indexes them but they are not displayed)
- Priority and state are colored via ANSI (`--ansi`)

**Priority colors:**

| Priority | Color |
|---|---|
| urgent | Red |
| high | Yellow |
| medium | Blue |
| low | Gray |

**State colors:**

| State | Color |
|---|---|
| in-progress | Green |
| review | Cyan |
| todo | White |
| backlog | Gray |
| done | Dim |

### Preview pane

- Shows full rendered issue via `tsk show {id}`
- Positioned right, 60% width, wrapped
- fzf flags: `--preview 'tsk show {1}' --preview-window right:60%:wrap`

### Context-aware filtering

Each command pre-filters the issue list shown in the picker:

| Command | Filter |
|---|---|
| `tsk show` | All issues |
| `tsk edit` | All issues |
| `tsk move` | Non-done issues where `remote_deleted != true` |
| `tsk close` | Only non-done issues where `remote_deleted != true` |
| `tsk reopen` | Only done issues where `remote_deleted != true` |
| `tsk rm` | All issues |
| `tsk path` | All issues |
| `tsk reorder-up` | Issues in current project's board where `remote_deleted != true` |
| `tsk reorder-down` | Issues in current project's board where `remote_deleted != true` |

Issues marked `remote_deleted: true` remain readable via `tsk show` / `tsk path`, but they are excluded from state-changing pickers and kanban reordering.

All pickers respect `$PWD` project detection — if the working directory is inside a registered project, the picker defaults to that project's issues. The `--all` flag overrides this to show all issues.

### `tsk move` — two-stage selection

`tsk move` accepts both `[<ID>]` and `[<state>]`:

- **Zero args:** pick issue via `fzf_pick_issue` (non-done filter), then pick target state via `fzf_pick_enum`
- **ID only, no state:** pick target state from the board's defined `states[]` via `fzf_pick_enum`
- **Both provided:** no picker needed

The state picker uses the board's `states[]` array from `config.yaml`. The current state is excluded from the list.

### `tsk recur skip` — recurring definition picker

When `tsk recur skip` is invoked without `<recur-id>` and stdin is a TTY, `fzf_pick_enum` is used to select from the `recurring[]` definitions in `config.yaml`. Display format:

```
ID  FREQUENCY  TITLE_PATTERN
```

### Non-TTY behavior

When stdin is not a TTY (piped input, cron, scripts), a missing required argument produces an error (exit 1) with a usage message. fzf is never spawned in non-interactive contexts.

### fzf not installed

When fzf is not installed and `<ID>` is omitted interactively, print an error with an install hint and exit 1:

```
error: <ID> required (install fzf for interactive selection)
```

fzf remains "Recommended", not "Required".

### User-configurable fzf options

Users can pass extra fzf flags globally via `ui.fzf_opts` in `config.yaml` (see [16 — Configuration](16-configuration.md)). These are appended to every fzf invocation, allowing customization of height, border, layout, etc.

### Commands affected

- `tsk show [<ID>]`
- `tsk edit [<ID>]`
- `tsk move [<ID>] [<state>]`
- `tsk close [<ID>]`
- `tsk reopen [<ID>]`
- `tsk rm [<ID>]`
- `tsk path [<ID>]`
- `tsk reorder-up [<ID>]`
- `tsk reorder-down [<ID>]`
- `tsk recur skip [<recur-id>]`
