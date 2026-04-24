# Implementation Roadmap

Status: archived

> Source of truth has moved to code and tests. This document is retained as historical record.

### v0.1 — MVP

Core issue management + GitLab sync. Usable as a daily driver.

- `tsk init`
- `tsk new` (interactive + flags, project detection + registration)
- `tsk edit`, `tsk show`, `tsk move`, `tsk close`, `tsk reopen`, `tsk ls`, `tsk path`
- `tsk view` + `tsk board` (kanban + project views)
- `tsk sync pull` + `tsk sync push` (GitLab only)
- `tsk resolve`
- `tsk session start` + `tsk session end`
- `riptask.yaml` config loading
- bash completions
- `tsk hooks install` / `update` / `status` / `uninstall` + pre-commit validation hook (see [25 — RIPTASK_REPO Data Repository Hooks](25-tsk-repo-hooks.md))
- `lib/frontmatter.sh` with full yq integration
- `lib/detect.sh` — project auto-detection from `$PWD`
- `lib/fzf.sh` — fzf interactive selection for all ID-taking commands (see [21 — fzf Interactive Selection](21-fzf-interactive-selection.md))

### v0.2 — Full sync + views

- GitHub sync (`lib/remote_github.sh`)
- `tsk reorder`, `tsk reorder-up`, `tsk reorder-down`
- Org views, cycle views in `tsk view`
- `tsk sync status` (dry-run diff)
- `tsk commit` helper
- bash completions

### v0.3 — Recurring + AI

- `tsk recur` (new, list, run, skip)
- systemd user timer for `tsk recur run`
- `lib/ai.sh` — body generation, triage, summarize, ask
- `tsk new --ai`
- `tsk sync pull --triage`

### v0.4 — Editor integration

- Neovim plugin (oil.nvim + mini.files keymaps, packaged as a plugin)
- Yazi keymap config
- `tsk board --open` with configurable opener

### Rust migration

The Bash roadmap above is retained as historical context. The Rust implementation proceeds in the following phases.

### Current phase status

| Phase | Status | Notes |
|---|---|---|
| Phase 0 — Freeze behavior | Complete | Characterization fixtures and format-contract tests exist |
| Phase 1 — Foundation | Complete | Core crate structure and base commands are in place |
| Phase 2 — Local workflow | Complete | All local commands implemented |
| Phase 3 — Recurring + local git semantics | Complete | `recur new` and local auto-commit implemented |
| Phase 4 — Remote providers | Complete | GitHub (`octocrab`) and GitLab (`gitlab` crate) providers implemented |
| Phase 5 — Sync engine | Complete | Full sync pull/push/status/resolve with conflict detection |
| Phase 6 — Workflow commands | Complete | `branch`, `pr`, `push`, `session`, `commit`, and `resolve` implemented |
| Phase 7 — AI + polish | Complete | Completions via `clap_complete`; `sync pull --triage` deferred post-migration |

#### Phase 0 — Freeze behavior

- Port fixtures and outputs into Rust characterization tests
- Lock file-format compatibility for issues, config, and caches
- Preserve exit codes, help text, and command names

#### Phase 1 — Foundation

- Create Rust crate structure (`cli`, `error`, `paths`, `config`, `scope`, `domain`, `storage`)
- Implement typed config, paths, frontmatter parsing, and cache loading
- Land foundational commands: `version`, `help`, `config` (read and set), `show`, `path`, `ls`, `init`
- `config set` is a write operation but belongs here because it only modifies `riptask.yaml`, not issue files
- Bare `tsk sync` (no subcommand) dispatches to pull-then-push and is implemented in Phase 5

#### Phase 2 — Local workflow

- Implement local issue lifecycle: `new`, `edit`, `move`, `close`, `reopen`, `rm`
- Implement views and ordering: `view`, `board`, `reorder`, `reorder-up`, `reorder-down`
- Implement template management and project registration/detection

#### Phase 3 — Recurring + local git semantics

- Implement recurring task definitions and instantiation
- Preserve `local_updated_at`, branch slug generation, and local auto-commit behavior
- Keep wrapped `git` behavior for lifecycle commits

#### Phase 4 — Remote providers

- Add native GitHub and GitLab providers using `octocrab` and `gitlab`
- Implement remote-first issue creation and `tsk push`
- Implement LOCAL → PREFIX-NNN rename on first push, `id_map.json` tracking, and cross-reference updates
- Preserve remote metadata shape and cache semantics

#### Phase 5 — Sync engine

- Implement `sync pull`, `sync push`, `sync status`, and `resolve`
- Preserve conflict detection, `.REMOTE.md`, and `remote_state.json`
- Rename/id_map logic is in Phase 4; this phase reuses it for sync-triggered renames

#### Phase 6 — Workflow commands

- Implement `branch`, `pr`, `commit`, `session`, and hooks
- Keep wrapped `git` where exact CLI behavior is part of the contract
- Preserve branch, PR, and MR metadata fields in frontmatter

#### Phase 7 — AI + polish

- `summarize`, `ask`, `new --ai` implemented with `claude`/`llm` CLI wrappers
- Completions generated via `clap_complete` (`tsk completions <shell>`)
- `sync pull --triage` deferred as post-migration polish
