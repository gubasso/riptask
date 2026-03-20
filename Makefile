.POSIX:

.PHONY: help build test lint install uninstall clean check

help:
	@echo "targets:"
	@echo "  build       Build the release binary"
	@echo "  test        Run cargo nextest"
	@echo "  lint        Run cargo fmt and cargo clippy"
	@echo "  install     Install riptsk from the current path"
	@echo "  uninstall   Uninstall riptsk"
	@echo "  clean       Remove riptsk data directory"
	@echo "  check       Run lint and test"

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
	rm -rf $(HOME)/.local/share/riptsk

check:
	$(MAKE) lint
	$(MAKE) test
