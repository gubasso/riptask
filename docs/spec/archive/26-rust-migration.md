# Rust Migration

Status: archived

> Source of truth has moved to code and tests. This document is retained as historical record.

This spec records the migration of `tsk` from Bash to Rust: rationale, library choices, and the final runtime model now that the migration is complete.

### Migration rationale

The Bash implementation proved the product model, but the codebase grew to ~4,500 lines spread across many modules with increasingly complex state transitions:

- Sync pull/push orchestration
- Conflict detection and `.REMOTE.md` preservation
- LOCAL → remote ID renames and cross-reference updates
- Recurring task scheduling and instantiation
- Multi-host session workflows

The Rust migration exists to:

- Eliminate runtime dependencies on `yq`, `jq`, and `python3`
- Replace shell-typed frontmatter and cache manipulation with compile-time checked structs
- Replace remote CLI JSON parsing with native HTTP API clients where mature crates exist
- Preserve the plaintext and git-native UX while shipping a single binary

Wrapped dependencies remain where the external tool's exact behavior is user-visible or currently the lowest-risk compatibility path: `git`, `fzf`, and `claude` / `llm`.

### Library selection

| Area | Library / Tool | Version | Strategy | Notes |
|---|---|---|---|---|
| CLI | `clap` | 4.5.x | USE | Primary parser for commands, flags, help, and exit surface |
| Completions | `clap_complete` | 4.5.x | USE | Generate shell completions during build/install |
| Serialization | `serde` | 1.0.228 | USE | Shared serialization layer for config, issues, and caches |
| YAML | `serde-yaml-ng` | 0.10.0 | USE | Native YAML parsing/writing for config and frontmatter |
| JSON | `serde_json` | 1.0.149 | USE | Cache files and remote payload handling |
| Error handling | `thiserror` | 2.0.18 | USE | Typed domain and command errors |
| Error context | `anyhow` | 1.0.102 | USE | Command boundary context and top-level propagation |
| Date/time | `jiff` | 0.2.23 | USE | UTC timestamps, recurrence math, ISO8601 parsing |
| XDG paths | `xdg` | 3.0.0 | USE | Exact XDG Base Directory handling |
| Filesystem traversal | `walkdir` | 2.5.0 | USE | Issue and view directory traversal |
| UTF-8 paths | `camino` | 1.2.2 | USE | Safer path handling in app code |
| Temp files | `tempfile` | 3.27.0 | USE | Atomic writes and test helpers |
| Terminal styling | `console` | latest | USE | Terminal colors, term width, emoji handling (mitsuhiko ecosystem) |
| Tree rendering | `termtree` | 1.0.0 | USE | Native board tree output without external `tree` |
| Interactive prompts | `dialoguer` | latest | USE | Confirm/select/multi-select/input prompts (mitsuhiko ecosystem) |
| Progress/spinners | `indicatif` | latest | USE | Progress bars, spinners, status messages (mitsuhiko ecosystem) |
| Table output | `comfy-table` | latest | USE | Formatted table output (e.g., `tsk ls`) |
| Async runtime | `tokio` | 1.x (latest) | USE | Runtime for native HTTP adapters |
| GitHub API | `octocrab` | 0.49.x | USE | Native GitHub issue/label/PR operations |
| GitLab API | `gitlab` | latest | USE | Native GitLab issue/label/MR operations; pin to tested GitLab API version |
| Git operations | `git` CLI | external | WRAP | Preserve exact init/add/commit/pull/push/branch behavior |
| Fuzzy picker | `fzf` CLI | external | WRAP | Preserve current preview, multi-select, and picker UX |
| AI backend | `claude` / `llm` CLI | external | WRAP | No official Rust Anthropic SDK; keep current opt-in backend |
| Subprocess calls | `std::process::Command` | stdlib | STDLIB | All external subprocess calls (`git`, `fzf`, `claude`/`llm`) |
| Frontmatter delimiter handling | internal splitter/joiner | n/a | IMPLEMENT | Markdown frontmatter/body boundaries remain app-defined |
| Config write-back | internal config save layer | n/a | IMPLEMENT | Needed for `tsk config set` and recurring `last_run` updates |

### Selection notes

- `console` + `dialoguer` + `indicatif` chosen as cohesive mitsuhiko ecosystem, replacing `owo-colors` and `inquire`.
- `inquire` was considered but `dialoguer` preferred for ecosystem cohesion with `console`/`indicatif`.
- `std::process::Command` chosen over `duct`/`xshell` for better LLM code-generation reliability with stdlib APIs.
- `gix` was considered for native git operations, but `tsk` currently relies on user-visible `git` CLI behavior for branch creation, push/pull, commit hooks, and session flows. The compatibility-first decision is to wrap `git`.
- `git2` was considered and rejected for the initial migration because it still would not guarantee CLI-parity workflows and adds a libgit2 dependency surface.
- `gray_matter` was considered for frontmatter parsing. It is useful for extraction, but not strong enough as the sole read/write contract for `tsk`'s Markdown files. The Rust implementation must own the exact frontmatter split/join semantics.
- `config-rs` was considered and rejected as the primary config layer because its own contract is read-focused and explicitly not a config write-back solution.

### Async architecture

Remote HTTP providers are async. The Rust implementation uses `tokio` internally, with `block_on` only at command boundaries:

- `main.rs` parses the CLI and enters the selected command
- Command handlers remain the boundary between synchronous CLI flow and async remote providers
- `RemoteProvider` methods are async because GitHub and GitLab operations are network-bound
- `GitBackend`, `Picker`, and `AiBackend` remain synchronous traits because they wrap synchronous subprocess behavior

This preserves simple command-handler signatures while allowing native async HTTP clients for remotes.

### File-format compatibility

This is a full rewrite. No backward compatibility with the Bash implementation is required. On-disk formats, field names, cache shapes, and exit codes are all free to change as needed.

### Core traits

The migration introduces four key traits to isolate volatile boundaries:

- `RemoteProvider` (async): list, create, update, close, reopen issues, labels, and PR/MR creation
- `GitBackend` (sync): `git` init, add, commit, pull, push, checkout, and repo-state queries
- `Picker` (sync): fuzzy and non-fuzzy interactive selection
- `AiBackend` (sync): summarize, ask, and body-generation subprocess bridge

### Testing

See [[22-testing]] for the full testing strategy. In summary:

- Pre-commit handles linting (cargo fmt, clippy, taplo, typos) and security checks (cargo audit, cargo deny).
- `make test` runs `cargo nextest run` directly — not through pre-commit.
- `make lint` runs `cargo fmt --check` and `cargo clippy` directly.
- The legacy BATS suite and Bash pre-commit config have been removed.

### Build and distribution changes

See [[23-makefile]] for the full target list. Key targets:

- `make build` — `cargo build --release`
- `make install` — `cargo install --path .` (installs to `~/.cargo/bin/`)
- `make uninstall` — `cargo uninstall tsk`
- `make test` — `cargo nextest run`
- `make lint` — `cargo fmt --check` + `cargo clippy`
- `make check` — lint + test

**Dev workflow** (idiomatic Rust):

- `cargo run -- <args>` — build + run in one step, the standard dev loop
- `cargo install --path .` — installs release binary to `~/.cargo/bin/tsk` for system-wide testing
- No symlinks or copies to `~/.local/bin` — `~/.cargo/bin` is already in PATH
- Quick iteration: `cargo build && ./target/debug/tsk <args>`

Default templates and hook script are embedded with `include_str!`. Completions are generated from `clap_complete`.

---

## Current migration state

As of 2026-03-19, the Bash implementation has been fully removed. The Rust binary is the only supported runtime entry point and is installed with `cargo install --path .` to `~/.cargo/bin/tsk`.

### Runtime model

- Runtime entry point: installed Rust binary `tsk`
- Repository-local Bash dispatcher: removed
- Embedded assets: default templates and the managed pre-commit hook live under `src/assets/`
- Shell completions: generate on demand with `tsk completions bash`, `tsk completions zsh`, or `tsk completions fish`

### What is implemented in Rust

| Command | Status | Notes |
|---|---|---|
| `init` | Done | Creates repo layout, default config, templates |
| `config` / `config set` | Done | Read and write `tsk.yaml` |
| `new` | Done | With auto-register-on-create and `--ai` fallback |
| `edit` | Done | Opens `$EDITOR` |
| `show` | Done | Prints issue frontmatter + body |
| `move` | Done | Changes state field |
| `close` | Done | Sets state to done |
| `reopen` | Done | Sets state to todo |
| `rm` | Done | Deletes issue file |
| `ls` | Done | Lists issues with filters |
| `path` | Done | Prints issue file path |
| `view` | Done | Regenerates all view trees |
| `board` | Done | Kanban tree with multi-project scope |
| `reorder` / `reorder-up` / `reorder-down` | Done | Reorders issues within a lane |
| `template list` / `show` / `new` / `edit` / `rm` / `validate` | Done | Full template lifecycle |
| `register` / `register --list` | Done | Interactive registration and listing |
| `hooks install/update/status/uninstall` | Done | Full lifecycle with embedded hook script |
| `recur list` / `new` / `run` / `skip` | Done | Scheduling, creation, dedup, instantiation |
| `summarize` / `ask` | Done | AI backend with graceful degradation |
| `push` | Done | Promotes local issues to remote-backed IDs |
| `sync pull` / `push` / `status` / `resolve` | Done | Remote sync, status, and conflict resolution |
| `branch` | Done | Creates and pushes an issue branch |
| `pr` | Done | Opens PR/MR from the current branch |
| `session start/end` | Done | Session lifecycle around git workflows |
| `commit` | Done | Manual issue repository commit helper |

### Migration result

- The Bash runtime tree has been deleted.
- The Rust binary is the sole supported entry point.
- Make targets are Rust-only (`build`, `lint`, `test`, `install`, `uninstall`, `check`).
- Shell completions are generated dynamically instead of shipping a static Bash file.

### Bash script migration status

All legacy Bash scripts and support files have been removed from the repository.

| Script | Status | Rust equivalent |
|---|---|---|
| `core.sh` | REMOVED | `paths.rs`, `config.rs`, `scope.rs` |
| `frontmatter.sh` | REMOVED | `storage/frontmatter.rs` |
| `id.sh` | REMOVED | `services/issue_service.rs` |
| `fzf.sh` | REMOVED | `adapters/picker.rs` |
| `hooks.sh` | REMOVED | `commands/hooks.rs` |
| `ai.sh` | REMOVED | `commands/ai.rs` |
| `detect.sh` | REMOVED | `services/project_detection.rs` |
| `template.sh` | REMOVED | `commands/templates.rs` |
| `issue.sh` | REMOVED | `commands/issues.rs` |
| `view.sh` | REMOVED | `commands/views.rs` |
| `recur.sh` | REMOVED | `commands/recur.rs` |
| `sync.sh` | REMOVED | `services/sync_engine.rs`, `commands/sync_cmd.rs` |
| `push.sh` | REMOVED | `commands/push_cmd.rs` |
| `remote_github.sh` | REMOVED | `adapters/github.rs` |
| `remote_gitlab.sh` | REMOVED | `adapters/gitlab.rs` |
| `hooks/pre-commit` | REMOVED | `src/assets/pre-commit.sh`, `commands/hooks.rs` |

---

## Migration complete

The Bash migration work is complete:

- The repository no longer ships `bin/tsk`, `lib/tsk/`, BATS suites, or a static completion script.
- Installation is via `cargo install --path .`.
- Completions are generated from the live clap command tree with `tsk completions <shell>`.
- The remaining source of truth is the Rust codebase and its Rust-native tests.
