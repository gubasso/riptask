# Makefile

Status: archived

> Source of truth has moved to code and tests. This document is retained as historical record.

The Makefile is a thin wrapper around Cargo commands for building, testing, linting, and installing the Rust implementation of `riptsk`.

---

### Targets

| Target | Description |
|---|---|
| `build` | Build the release binary |
| `test` | Run the Rust test suite with `cargo nextest` |
| `lint` | Run `cargo fmt --check` and `cargo clippy --all-targets --all-features -- -D warnings` |
| `install` | Install `riptsk` from the current source tree |
| `uninstall` | Remove the installed `riptsk` cargo package |
| `check` | Run lint and test |
| `help` | Print available targets |

---

### `build` target

Builds the release binary:

```bash
cargo build --release
```

---

### `test` target

Runs the Rust test suite using `cargo nextest`:

```bash
cargo nextest run
```

---

### `lint` target

Runs Rust formatting and lint checks:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
```

---

### `install` target

Installs `riptsk` from the current path:

```bash
cargo install --path .
```

---

### `check` target

Runs lint and test in sequence:

```bash
make lint
make test
```

---

### Full Makefile

```makefile
.POSIX:

.PHONY: help build test lint install uninstall check

help:
	@echo "targets:"
	@echo "  build       Build the release binary"
	@echo "  test        Run cargo nextest"
	@echo "  lint        Run cargo fmt and cargo clippy"
	@echo "  install     Install riptsk from the current path"
	@echo "  uninstall   Uninstall riptsk"
	@echo "  check       Run lint and test"

build:
	cargo build --release

test:
	cargo nextest run

lint:
	cargo fmt --check
	cargo clippy --all-targets --all-features -- -D warnings

install:
	cargo install --path .

uninstall:
	cargo uninstall riptsk

check:
	$(MAKE) lint
	$(MAKE) test
```

---

### Notes

- All targets are declared `.PHONY`.
- The Makefile is intentionally a thin wrapper; Cargo remains the source of truth for build behavior.
- `cargo nextest` is the default test runner for faster and more reliable Rust test execution.
- Linting is Rust-native: formatting is enforced with `cargo fmt`, and warnings are treated as errors via `cargo clippy --all-targets --all-features -- -D warnings`.
- Installation produces a single binary rather than copying a Bash runtime tree.
