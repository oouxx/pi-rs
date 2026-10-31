#!/usr/bin/env bash
# Format only the Rust files staged for this commit, then re-stage them.
# Invoked by .pre-commit-config.yaml (or directly as .git/hooks/pre-commit).
set -euo pipefail

files=()
while IFS= read -r -d '' file; do
    files+=("$file")
done < <(git diff --cached --name-only --diff-filter=ACMR -z -- '*.rs')

if [ ${#files[@]} -eq 0 ]; then
    exit 0
fi

# Edition matches the workspace in Cargo.toml; rustfmt still discovers
# rustfmt.toml by walking up from each file.
rustfmt --edition 2021 "${files[@]}"
git add -- "${files[@]}"
