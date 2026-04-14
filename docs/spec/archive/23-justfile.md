# Justfile

Status: archived

> Source of truth has moved to code and tests. This document is retained as historical record.

The `justfile` is a thin wrapper around Cargo commands for building, testing, linting, and installing the Rust implementation of `riptsk`.

---

### Recipes

| Recipe | Description |
|---|---|
| `default` | Print available recipes via `just --list` |
| `build` | Build the release binary |
| `test` | Run the Rust test suite with `cargo nextest` |
| `lint` | Run `cargo fmt --check` and `cargo clippy --all-targets --all-features -- -D warnings` |
| `install` | Install `riptsk` from the current source tree |
| `uninstall` | Remove the installed `riptsk` cargo package |
| `clean` | Remove the local riptsk data directory |
| `check` | Run lint and test |

---

### `build` recipe

Builds the release binary:

```bash
cargo build --release
```

---

### `test` recipe

Runs the Rust test suite using `cargo nextest`:

```bash
cargo nextest run
```

---

### `lint` recipe

Runs Rust formatting and lint checks:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
```

---

### `install` recipe

Installs `riptsk` from the current path:

```bash
cargo install --path . --force
```

---

### `check` recipe

Runs lint and test in sequence via recipe dependencies:

```bash
just lint
just test
```

---

### Full justfile

```just
default:
  @just --list

build:
  cargo build --release

test:
  cargo nextest run

lint:
  cargo fmt --check
  cargo clippy --all-targets --all-features -- -D warnings

install:
  cargo install --path . --force

uninstall:
  cargo uninstall riptsk

clean:
  rm -rf "${HOME}/.local/share/riptsk"

check: lint test
```

---

### Notes

- The `justfile` is intentionally a thin wrapper; Cargo remains the source of truth for build behavior.
- `default` delegates to `just --list` instead of maintaining a manual help block.
- `check` uses native just dependencies instead of recursive self-invocation.
- `cargo nextest` is the default test runner for faster and more reliable Rust test execution.
- Linting is Rust-native: formatting is enforced with `cargo fmt`, and warnings are treated as errors via `cargo clippy --all-targets --all-features -- -D warnings`.
- Installation produces a single binary rather than copying a Bash runtime tree.
