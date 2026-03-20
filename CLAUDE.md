# CLAUDE.md

## Versioning

This project is pre-1.0.0. There is NO backward compatibility requirement. Breaking changes are allowed and expected — do not add migration paths, compatibility shims, or legacy support for old formats.

## Pre-commit configs

- `.pre-commit-config.yaml` — active repository config

## Linting & Testing

The Makefile wraps the primary local checks directly:

```bash
make lint   # cargo fmt --check + cargo clippy --all-targets --all-features -- -D warnings
make test   # cargo nextest run
make check  # runs both lint and test
```

- `.pre-commit-config.yaml` still defines commit-time and pre-push hooks for formatting, linting, and security checks.
