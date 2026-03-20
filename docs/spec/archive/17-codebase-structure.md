# riptsk Codebase Structure

Status: archived

> Source of truth has moved to code and tests. This document is retained as historical record.

```
riptsk/
├── Cargo.toml
├── src/
│   ├── adapters/
│   │   ├── ai.rs                   # AI backend trait + CLI wrapper
│   │   ├── git.rs                  # Git backend trait + CLI wrapper
│   │   ├── github.rs               # GitHub RemoteProvider implementation
│   │   ├── gitlab.rs               # GitLab RemoteProvider implementation
│   │   ├── mod.rs
│   │   ├── picker.rs               # fzf-backed picker
│   │   ├── prompts.rs              # PromptBackend trait + dialoguer impl
│   │   └── remote.rs               # RemoteProvider trait
│   ├── assets/
│   │   ├── hook.rs                 # Embedded pre-commit hook
│   │   ├── mod.rs
│   │   ├── pre-commit.sh           # Embedded hook payload
│   │   └── templates.rs            # Embedded default templates
│   ├── commands/
│   │   ├── ai.rs
│   │   ├── branch_pr.rs
│   │   ├── config_cmd.rs
│   │   ├── hooks.rs
│   │   ├── init.rs
│   │   ├── issues.rs
│   │   ├── mod.rs
│   │   ├── push_cmd.rs
│   │   ├── recur.rs
│   │   ├── register.rs
│   │   ├── sync_cmd.rs
│   │   ├── templates.rs
│   │   └── views.rs
│   ├── domain/
│   │   ├── id_map.rs
│   │   ├── issue.rs
│   │   ├── mod.rs
│   │   ├── remote_state.rs
│   │   └── session.rs              # Session state for start/end workflow
│   ├── services/
│   │   ├── auto_commit.rs          # Auto-commit helper for issue mutations
│   │   ├── issue_service.rs
│   │   ├── mod.rs
│   │   ├── project_detection.rs
│   │   ├── recurrence.rs
│   │   ├── remote_mapping.rs       # Remote provider factory and converters
│   │   ├── sync_engine.rs
│   │   ├── templates.rs
│   │   └── view_builder.rs
│   ├── storage/
│   │   ├── cache.rs
│   │   ├── frontmatter.rs
│   │   ├── issue_store.rs
│   │   ├── mod.rs
│   │   └── session.rs              # Session state JSON persistence
│   ├── cli.rs
│   ├── config.rs
│   ├── error.rs
│   ├── lib.rs
│   ├── main.rs
│   ├── paths.rs
│   └── scope.rs
├── tests/
│   ├── cmd_foundation.rs           # Rust CLI smoke/foundation tests
│   ├── cmd_lifecycle.rs            # Rust lifecycle smoke tests
│   ├── cmd_views.rs                # Rust view smoke tests
│   ├── format_contracts.rs         # Snapshot contracts for fixtures
│   ├── fixtures/                   # YAML/Markdown/JSON contract fixtures
│   ├── snapshots/                  # insta snapshots for Rust format tests
│   └── ...                         # Additional Rust integration tests
├── templates/                      # Source templates copied/embedded by Rust
├── docs/spec/                      # Active + archived specifications
├── Makefile                        # Thin wrapper around cargo commands
└── .pre-commit-config.yaml         # Rust-focused pre-commit configuration
```

### Module conventions

- `src/main.rs` owns CLI startup, command dispatch, and exit-code handling.
- `src/cli.rs` defines the clap command tree.
- `src/commands/*.rs` are thin command handlers that wire CLI args into services and adapters.
- `src/services/*.rs` hold workflow and business logic.
- `src/adapters/*.rs` isolate subprocess and remote-provider boundaries.
- `src/domain/*.rs` define serialized config, issue, and cache data.
- `src/storage/*.rs` own filesystem and frontmatter persistence.
- `src/assets/*.rs` embed templates and hooks with `include_str!`.

### Current dependencies

| Dependency | Required | Purpose |
|---|---|---|
| `anyhow` | Yes | Error context at command and service boundaries |
| `async-trait` | Yes | Async trait support for `RemoteProvider` |
| `camino` | Yes | UTF-8 path handling |
| `clap` | Yes | CLI parsing and help text |
| `clap_complete` | Yes | Shell completion generation support |
| `comfy-table` | Yes | Table formatting |
| `console` | Yes | Terminal styling utilities |
| `dialoguer` | Yes | Interactive prompts |
| `gitlab` | For GitLab sync | Native GitLab API client |
| `indicatif` | Yes | Progress bars and spinners |
| `jiff` | Yes | Date/time parsing and recurrence math |
| `octocrab` | For GitHub sync | Native GitHub API client |
| `serde` | Yes | Shared serialization layer |
| `serde_json` | Yes | JSON cache and payload handling |
| `serde_yaml_ng` | Yes | YAML config/frontmatter parsing |
| `tempfile` | Yes | Atomic writes and temp fixtures |
| `termtree` | Yes | Tree rendering |
| `thiserror` | Yes | Typed application errors |
| `tokio` | Yes | Async runtime at remote command boundaries |
| `walkdir` | Yes | Filesystem traversal |
| `xdg` | Yes | XDG path resolution |
| `assert_cmd` | Dev | Rust CLI integration testing |
| `insta` | Dev | Snapshot-based contract testing |
| `predicates` | Dev | Command output assertions |
| `wiremock` | Dev | Planned HTTP-mocking support for remote tests |
