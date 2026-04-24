# Non-Goals

Status: active (permanent)

These are explicitly out of scope and will not be implemented:

- **TUI kanban** — the filesystem view (`tree`, `ls`, nvim/yazi) is sufficient
- **Background sync daemon** — sync is always explicit and user-triggered
- **Team collaboration tooling** — designed for a single engineer across multiple machines, but usable by a small team with a simple convention: each developer works on one task at a time within a RepoProject. Conflicts are detected and preserved for manual resolution (see [03 — Design Decisions §3.5](03-design-decisions.md)). What `riptask` will *not* build: shared dashboards, team notifications, access control, or real-time co-editing. Multi-host sync is supported via `tsk session` and `$RIPTASK_REPO` git (see [11 — Multi-Host Workflow](archive/11-multi-host-workflow.md))
- **Three-way merge for conflicts** — conflicts are detected and preserved for manual resolution (see [03 — Design Decisions §3.5](03-design-decisions.md)), but `riptask` does not attempt automatic three-way merging of frontmatter or body content
- **Web interface** — not needed, contradicts the philosophy
- **Database** — flat files + optional JSON cache is sufficient
- **Native mobile app** — terminal workflow only
- **Notifications / webhooks** — pull-based, not push-based
- **Time tracking** — out of scope for this tool
