#!/usr/bin/env bash
# Format (or check) only the crates owned by this repository.
#
# Vendored crates under crates/vendor are kept byte-identical to upstream
# (apart from the documented adaptations), so running rustfmt over them would
# turn every re-sync into a formatting diff. Extra args are passed through to
# `cargo fmt` (e.g. `scripts/fmt.sh -- --check`).
set -euo pipefail
cd "$(dirname "$0")/.."

status=0
for manifest in crates/*/Cargo.toml; do
    case "$manifest" in
        crates/vendor/*) continue ;;
    esac
    cargo fmt --manifest-path "$manifest" "$@" || status=$?
done
exit "$status"
