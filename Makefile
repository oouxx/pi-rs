.PHONY: build run run-tui test test-all fmt lint check clean help version release-patch release-minor release-major

APP_NAME := pi
VERSION := $(shell grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
VERSION_FILE := Cargo.toml

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

# ─── Release ──────────────────────────────────────────

release-patch: _require-clean _bump-patch _tag-release
release-minor: _require-clean _bump-minor _tag-release
release-major: _require-clean _bump-major _tag-release

_require-clean:
	@git diff --quiet --exit-code || { echo "❌ 工作区有未提交的修改，请先 commit"; exit 1; }

_bump-patch:
	@v=$$(grep '^version' $(VERSION_FILE) | head -1 | sed 's/.*"\(.*\)"/\1/'); \
	IFS='.' read -r major minor patch <<< "$$v"; \
	new="$${major}.$${minor}.$$((patch + 1))"; \
	echo "⬆  $$v → $$new (patch)"; \
	sed -i '' 's/^version = ".*"/version = "'"$$new"'"/' $(VERSION_FILE)

_bump-minor:
	@v=$$(grep '^version' $(VERSION_FILE) | head -1 | sed 's/.*"\(.*\)"/\1/'); \
	IFS='.' read -r major minor patch <<< "$$v"; \
	new="$${major}.$$((minor + 1)).0"; \
	echo "⬆  $$v → $$new (minor)"; \
	sed -i '' 's/^version = ".*"/version = "'"$$new"'"/' $(VERSION_FILE)

_bump-major:
	@v=$$(grep '^version' $(VERSION_FILE) | head -1 | sed 's/.*"\(.*\)"/\1/'); \
	IFS='.' read -r major minor patch <<< "$$v"; \
	new="$$((major + 1)).0.0"; \
	echo "⬆  $$v → $$new (major)"; \
	sed -i '' 's/^version = ".*"/version = "'"$$new"'"/' $(VERSION_FILE)

_tag-release:
	@v=$$(grep '^version' $(VERSION_FILE) | head -1 | sed 's/.*"\(.*\)"/\1/'); \
	tag="v$$v"; \
	git tag -l "$$tag" | grep -q "$$tag" && { echo "❌ Tag $$tag 已存在"; exit 1; }; \
	echo "🚀 发版 $$tag"; \
	cargo generate-lockfile && \
	git add -A && git commit -m "chore: bump to version $$tag"; \
	git tag "$$tag"; \
	git push origin main --tags; \
	echo "✅ 已推送 $$tag"

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
	@echo "Release:"
	@echo "  release-patch   bump patch (1.79.7 → 1.79.8)"
	@echo "  release-minor   bump minor (1.79.7 → 1.80.0)"
	@echo "  release-major   bump major (1.79.7 → 2.0.0)"
	@echo ""
	@echo "Other:"
	@echo "  clean           cargo clean"
	@echo "  version         show current version ($(VERSION))"
