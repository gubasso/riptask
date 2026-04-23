# Philosophy & Design Principles

Status: active (permanent)

### 2.1 Plaintext is the interface

Every issue is a Markdown file. No proprietary format, no database, no binary blobs. Every issue is readable and editable with `cat`, `grep`, `nvim`, or any text editor on any machine without any tooling installed beyond a shell.

### 2.2 Git is the database

Version history, audit trail, and backup are handled entirely by git. No additional persistence layer. `git log issues/WHL-041.md` is the audit trail for a single issue. `git diff` before a sync shows exactly what changed remotely.

### 2.3 Views are derived, never stored

Kanban boards, RepoProject views, org views, cycle views — all are generated at runtime by reading issue frontmatter. They are never the source of truth. The `views/` directory lives in the cache (`$XDG_CACHE_HOME/riptsk/views/`), outside the repo, and is always regeneratable from scratch.

### 2.4 One source of truth per concern

No concern has two sources of truth. This invariant is non-negotiable and is the foundation the entire design protects.

### 2.5 The CLI is load-bearing

Direct file manipulation is always possible, but the CLI is the primary write interface. It keeps frontmatter consistent, regenerates views, and manages the sync state. Without it, the system degrades to a collection of markdown files — which is still readable, just not manageable.

### 2.6 Remote IDs win (when a remote exists)

For GitHub and GitLab RepoProjects, the remote is the source of truth for issue identity. Local IDs are provisional until a first push assigns a real remote ID. After that, the remote ID is permanent.

For RepoProjects with non-GitHub/GitLab remotes or no remote at all, there is no remote source of truth. IDs are assigned locally using the RepoProject key prefix (e.g. `ICE-001`) and are permanent from creation — they are never provisional.

### 2.7 Mirror glab/gh behavior

`tsk` is context-aware: commands run from inside a RepoProject repo automatically scope to that RepoProject, exactly as `glab` and `gh` do. You never need to specify which RepoProject you're operating on — your current directory provides that context.

### 2.8 Opt-in complexity

Start with the simplest thing that works. Every advanced feature (AI, recurring tasks, cycles, orgs) is additive and optional. The core — create issue, move it, sync it — must always work with zero config beyond RepoProject registration.
