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
> | View generation | `lib/tsk/view.sh, tests/integration/view.bats` |
>
> ---

# Views — The Cache Layer

### Core invariant

> **The `views/` directory is always a pure function of `issues/*.md` frontmatter.**
> It is never the source of truth. It has no state of its own.

`tsk view` can be run at any time to rebuild it. If `views/` is deleted entirely, nothing is lost.

### Copy strategy

Views live in `$XDG_CACHE_HOME/riptsk/views/` (default `~/.cache/riptsk/views/`), outside `$RIPTSK_REPO`. View files are **copies** of `$RIPTSK_REPO/issues/<ID>.md`. If `$RIPTSK_REPO` moves, `tsk view` rebuilds everything.

View files are disposable copies. Edits to view files are lost on next regeneration. Always use `tsk edit` or `tsk path` to modify canonical issues.

### View types

**Kanban view** — `views/kanban/<board>/<state>/`

The primary view. Grouped by board, then by state (lane). All configured board lanes are materialized as directories, even when empty — this ensures the kanban structure is always complete. View filename: `<order_padded>-<ID>.md`. Order is zero-padded to 2 digits for correct `ls` sorting. Extend to 3 digits if lanes exceed 99 items.

```
views/kanban/penguin-chrono-labs/
├── backlog/
│   ├── 01-WHL-039.md
│   └── 02-WHL-040.md
├── todo/
│   ├── 01-WHL-041.md
│   └── 02-FSH-017.md
├── in-progress/
│   └── 01-WHL-042.md
├── review/
└── done/
    └── 01-WHL-037.md
```

**Project view** — `views/projects/<project>/`

Flat list of all issues for a project, regardless of state or board. No order prefix.

**Org view** — `views/orgs/<org>/`

All issues belonging to an org, regardless of project or state.

**Cycle view** — `views/cycles/<cycle>/`

All issues in a sprint/cycle. Order-prefixed.

### When views regenerate

Views regenerate automatically as the last step of every write operation:

- `tsk new` — after creating the file
- `tsk move` — after patching `state:`
- `tsk close` / `tsk reopen`
- `tsk edit` — after saving (via post-save hook or explicit call)
- `tsk reorder`
- `tsk sync` — after pull and push complete

Also available explicitly: `tsk view`

### Browsing examples

```bash
# Full kanban overview for a board
tree ~/.cache/riptsk/views/kanban/penguin-chrono-labs/

# What's in progress?
ls ~/.cache/riptsk/views/kanban/penguin-chrono-labs/in-progress/

# Read an issue directly (view copy has the same content)
cat ~/.cache/riptsk/views/kanban/penguin-chrono-labs/in-progress/01-WHL-042.md

# Edit the canonical issue (not the view copy)
tsk edit WHL-042

# All issues for a project
ls ~/.cache/riptsk/views/projects/wormhole-router/

# Cross-project org view
tree ~/.cache/riptsk/views/orgs/penguin-chrono-labs/
```
