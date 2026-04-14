# CLAUDE.md

## Versioning

This project is pre-1.0.0. There is NO backward compatibility requirement. Breaking changes are allowed and expected — do not add migration paths, compatibility shims, or legacy support for old formats.

## Pre-commit configs

- `.pre-commit-config.yaml` — active repository config

## Linting & Testing

The justfile wraps the primary local checks directly:

```bash
just lint   # cargo fmt --check + cargo clippy --all-targets --all-features -- -D warnings
just test   # cargo nextest run
just check  # runs both lint and test
```

- `.pre-commit-config.yaml` still defines commit-time and pre-push hooks for formatting, linting, and security checks.

## Backend Implementation Priority

When implementing backend (GitHub/GitLab) functionality, prefer in this order:
1. octocrab/gitlab native Rust crate methods
2. Direct REST API or GraphQL calls
3. gh/glab CLI commands
4. Other approaches
