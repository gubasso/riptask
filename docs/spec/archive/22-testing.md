# Testing

Status: archived

> Source of truth has moved to code and tests. This document is retained as historical record.

### Philosophy

The repository now uses Rust-native tests exclusively.

Code and tests are the source of truth for migrated behavior. Specs document intent and current migration status.

### Current test layout

```
tests/
├── cmd_foundation.rs        # assert_cmd smoke tests for init/help/foundation
├── cmd_lifecycle.rs         # assert_cmd lifecycle smoke tests
├── cmd_views.rs             # assert_cmd view generation smoke tests
├── format_contracts.rs      # insta snapshots for config/frontmatter/cache fixtures
├── snapshots/               # insta snapshot outputs
└── fixtures/                # YAML/Markdown/JSON fixtures for Rust tests
```

### Rust test strategy

Rust tests use these crates:

| Crate | Purpose |
|---|---|
| `assert_cmd` | End-to-end CLI invocation and exit assertions |
| `predicates` | Output matching for command assertions |
| `insta` | Snapshot contracts for fixtures and serialized formats |
| `wiremock` | Planned remote-provider and sync HTTP mocking |

Current Rust coverage is concentrated on:

- foundational CLI behavior (`init`, `help`, `version`)
- local lifecycle smoke tests (`new`)
- view generation smoke tests (`view`, scoped `board`)
- serialized format contracts for fixture parity
- targeted command regressions such as AI fallback behavior and auto-registration on `new`

### Pre-commit and CI reality

`.pre-commit-config.yaml` currently enforces:

- `cargo fmt --all`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo audit` on `pre-push`
- `cargo deny check advisories bans sources` on `pre-push`

Current gap:

- there is no `cargo test` or `cargo nextest` hook in `.pre-commit-config.yaml`

Pre-commit is therefore a lint/security gate, not the complete test runner.

### Execution targets

- `make test` runs the Rust suite with `cargo nextest run`
- `cargo test` remains useful for local iteration and contract checks

### Migration policy

- New Rust features should land with Rust tests whenever the subsystem is already migrated.
- Keep fixture-based format tests for issue/config/cache files even though `26-rust-migration.md` allows format changes; they still provide explicit review points for intentional drift.
