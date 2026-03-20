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
