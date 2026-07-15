.PHONY: build run run-tui test test-all fmt lint check clean help version

APP_NAME := pi
VERSION := $(shell grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')

# ─── Build ────────────────────────────────────────────

build:
	cargo build

release:
	cargo build --release

# ─── Run ──────────────────────────────────────────────

run:
	cargo run -p pi-cli -- $(ARGS)

run-tui:
	cargo run -p pi-tui -- $(ARGS)

# ─── Quality ──────────────────────────────────────────

test:
	cargo test -p pi-ai -p pi-agent-core -p pi-coding-agent -p pi-cli -p pi-extension-api -p pi-extensions

test-all:
	cargo test --workspace

fmt:
	cargo fmt --all

lint:
	cargo clippy --workspace -- -D warnings

check: fmt lint test
	@echo "✅ All checks passed"

# ─── Clean ────────────────────────────────────────────

clean:
	cargo clean

# ─── Version ──────────────────────────────────────────

version:
	@echo "$(VERSION)"

# ─── Help ────────────────────────────────────────────

help:
	@echo "Usage: make <target> [ARGS=...]"
	@echo ""
	@echo "Build:"
	@echo "  build           dev build"
	@echo "  release         release build"
	@echo ""
	@echo "Run:"
	@echo "  run             pi-cli (pass ARGS for flags, e.g. make run ARGS='--help')"
	@echo "  run-tui         pi-tui TUI mode"
	@echo ""
	@echo "Quality:"
	@echo "  test            cargo test (all crates except pi-tui)"
	@echo "  test-all        cargo test --workspace"
	@echo "  fmt             cargo fmt"
	@echo "  lint            cargo clippy (deny warnings)"
	@echo "  check           fmt + lint + test"
	@echo ""
	@echo "Other:"
	@echo "  clean           cargo clean"
	@echo "  version         show current version ($(VERSION))"
