# Design Decisions

Status: active (permanent)

### 3.1 No database

The tool is I/O bound. For GitHub/GitLab projects, non-trivial operations call the GitHub/GitLab APIs over the network (via native HTTP clients in Rust, or CLI wrappers during migration). For local projects (non-GitHub/GitLab remotes or no remote), all operations are purely local file I/O. Whether the wrapper uses SQLite or flat files is irrelevant to real runtime.

A database would:
- Break `grep`, `git log`, and `$EDITOR` as first-class interfaces
- Require the DB to be running for any read operation
- Add an operational dependency to an intentionally zero-dependency tool
- Complicate the "clone and go" property

The only legitimate database concern — query performance over many issues — is mitigated by the small scale: parsing 200–500 markdown frontmatters via native Rust YAML parsing is fast enough for a single engineer's task load. A JSON index cache was considered but deferred (see [20 — Open Questions](20-open-questions.md)); current query operations read issue files directly.

### 3.2 Rust, migrated from Bash

The tool started as Bash shell orchestration but migrated to Rust as it grew to ~4,500 lines with complex state management (sync, conflicts, recurring tasks). The migration eliminates runtime dependencies (`yq`, `jq`, `python3`), provides compile-time type safety for frontmatter schemas, enables native HTTP API clients instead of CLI subprocess parsing, and produces a single binary for simplified distribution.

External CLI tools (`git`, `fzf`, `claude`/`llm`) remain as wrapped dependencies where their exact behavior is user-visible or where no mature Rust alternative exists.

### 3.3 Views are transient cache copies

The `views/` directory uses file copies so that standard tools (`ls`, `tree`, `nvim`, `yazi`, `lf`) work natively without any custom renderer. The filename encodes order within a lane (`01-WHL-041.md`), enabling natural alphabetical sorting.

Views are:
- Always outside the repo — stored in `$XDG_CACHE_HOME/riptsk/views/`, never committed
- Always regeneratable — `tsk view` rebuilds from scratch
- Never written directly — only `tsk` commands modify them as a side effect
- Disposable — edits to view files are lost on next regeneration

### 3.4 Filenames encode order, not status

Moving a card = `tsk status WHL-041 in-progress`, which:
1. Patches `status:` in `issues/WHL-041.md` frontmatter
2. Updates `local_updated_at`
3. Regenerates `views/` entirely

You never touch view files directly. The view tree is a consequence of frontmatter status.

### 3.5 Conflict detection with manual resolution

On sync pull, `remote_state.json` serves as the sync baseline — it records the `updated_at` timestamp from the last successful sync. A conflict exists when **both sides changed since the last sync**:

- `remote.updated_at > remote_state.json[key].updated_at` (remote changed)
- `local.local_updated_at > remote_state.json[key].updated_at` (local changed)

When a conflict is detected, `tsk` preserves both versions instead of silently dropping one:
- The **local file** (`issues/WHL-042.md`) is kept as-is with a `conflict:` object added to frontmatter
- A **remote file** (`issues/WHL-042.REMOTE.md`) is written with the incoming remote version

This two-file representation avoids corrupting YAML frontmatter (git-style markers would break YAML parsing, views, `tsk ls`). The user resolves the conflict manually via `tsk resolve <ID>`, which supports `--take-remote`, `--take-local`, or default (user has already edited the local file).

Issues with an unresolved `conflict:` field are **skipped on push** — half-resolved state is never sent to the remote. `.REMOTE.md` files are excluded from views and `tsk ls`.

If `remote_state.json` is missing (first sync, cache cleared), `tsk` falls back to last-write-wins for graceful degradation.

See [10 — Sync Architecture](archive/10-sync-architecture.md) for the full pull/push algorithm.

### 3.6 Local IDs are provisional (GitHub/GitLab projects only)

For GitHub/GitLab projects, issues created locally before sync get a provisional ID: `LOCAL-<short-hash>`. After `tsk sync push`, the remote assigns a real numeric ID. `tsk` renames the file, updates the frontmatter, and records the mapping in `~/.cache/riptsk/id_map.json`. All cross-references using the old local ID are updated at rename time.

For local projects (non-GitHub/GitLab remotes or no remote), IDs are assigned immediately using the project prefix and a local sequence number (e.g. `ICE-001`). These IDs are permanent — there is no remote to defer to.

### 3.7 $RIPTSK_REPO is passive — commands run from project repos

The tasks data repo (`$RIPTSK_REPO`, defaulting to `~/.local/share/riptsk/`) is never the working directory for `tsk` commands. It is a passive store that `tsk` reads and writes to. Commands are run from inside project repos, mirroring `glab`/`gh` behavior. The `RIPTSK_REPO` environment variable (or XDG default path) points to the data repo.

### 3.8 Two distinct sync layers, never conflated

Issues that exist on GitHub/GitLab use those platforms as the shared state across hosts. `$RIPTSK_REPO` git is for local projects (non-GitHub/GitLab remotes or no remote), local-only issues, and config versioning. Never use `$RIPTSK_REPO` git push to share GitHub/GitLab-synced issue state across hosts — that's what `tsk sync` is for.

Projects with non-GitHub/GitLab remotes (codeberg, gitolite, etc.) are fully managed through `$RIPTSK_REPO` git — the same layer as local-only issues. Their remote exists for code, not for issue tracking.

### 3.9 Auto-commit on lifecycle events for local issues

Lifecycle commands (`new`, `close`, `status`, `reopen`, `rm`) auto-commit `$RIPTSK_REPO` when all affected issues are local (belonging to a project without a GitHub/GitLab remote, or not tied to any project).

**Why auto-commit local lifecycle events:** For local issues, `$RIPTSK_REPO` git is the sole source of truth and the only sync mechanism across hosts. A lifecycle event is a meaningful status change — a `riptsk: new ICE-043` commit is signal, not noise. Deferring these commits risks data loss (forgotten `session end`, crash) with no offsetting benefit.

**Why not for synced issues:** Their source of truth is GitHub/GitLab. The `$RIPTSK_REPO` copy is a backup. Auto-committing every backup-copy mutation adds noise to `$RIPTSK_REPO` git history without improving data safety — the remote already has the canonical state.

**Why not for trivial mutations:** Field edits (`tsk edit`), comments, and tag changes are low-signal individually. Batching them via `tsk commit` or `tsk session end` produces a cleaner history. The data loss risk is lower — these are incremental refinements, not status transitions.

**Commit message format:** `riptsk: <verb> <ID> — <title>` (see [15 — Version Control & Backup](archive/15-version-control-backup.md) for full conventions).

Auto-commit does not auto-push. It goes through normal `git commit`, so the pre-commit hook ([25 — RIPTSK_REPO Hooks](archive/25-tsk-repo-hooks.md)) runs and validates the commit automatically.
