default:
  @just --list

build:
  cargo build --release

test:
  cargo nextest run

test-clean:
  cargo clean -p riptask
  cargo nextest run

lint:
  cargo fmt --check
  cargo clippy --all-targets --all-features -- -D warnings

install:
  cargo install --path . --force

uninstall:
  cargo uninstall riptask

clean:
  rm -rf "${HOME}/.local/share/riptask"

check: lint test
