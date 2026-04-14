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
