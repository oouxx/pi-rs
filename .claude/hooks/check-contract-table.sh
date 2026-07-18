#!/usr/bin/env bash
# PreToolUse hook: runs before every Bash tool call, only acts on `git commit`.
#
# Enforces CLAUDE.md phase 三 "契约级：接口行为对齐": whenever staged .rs
# changes add/modify a `pub fn`/`pub struct`/`pub enum`/`pub trait` in a
# crate, that crate's CONTRACT_ALIGNMENT.md must be staged in the same
# commit. This doesn't check the *content* of the table (that needs
# judgment — see check-contract-consistency.sh and the human/adversarial
# review step), it only guarantees the table can't be silently skipped when
# public surface area changes.
#
# Assumes a `crates/<name>/...` workspace layout. Adjust the grep pattern
# below if your layout differs.

set -euo pipefail

CMD="${CLAUDE_TOOL_INPUT_COMMAND:-}"
[ -z "$CMD" ] && exit 0
echo "$CMD" | grep -qE '\bgit[[:space:]]+commit\b' || exit 0

REPO_ROOT=$(git rev-parse --show-toplevel 2>/dev/null || echo .)
cd "$REPO_ROOT"

STAGED_RS=$(git diff --cached --name-only -- '*.rs' || true)
[ -z "$STAGED_RS" ] && exit 0

MATCHED_CRATES_FILE=$(mktemp)
trap 'rm -f "$MATCHED_CRATES_FILE"' EXIT

while IFS= read -r f; do
  [ -z "$f" ] && continue
  crate_dir=$(echo "$f" | grep -oE '^crates/[^/]+' || true)
  [ -z "$crate_dir" ] && continue
  if git diff --cached -- "$f" | grep -E '^\+.*\bpub (fn|struct|enum|trait) ' > /dev/null; then
    echo "$crate_dir" >> "$MATCHED_CRATES_FILE"
  fi
done <<< "$STAGED_RS"

sort -u "$MATCHED_CRATES_FILE" -o "$MATCHED_CRATES_FILE" 2>/dev/null || true

MISSING=()
while IFS= read -r crate_dir; do
  [ -z "$crate_dir" ] && continue
  table="$crate_dir/CONTRACT_ALIGNMENT.md"
  if ! git diff --cached --name-only | grep -qx "$table"; then
    MISSING+=("$table")
  fi
done < "$MATCHED_CRATES_FILE"

if [ "${#MISSING[@]}" -gt 0 ]; then
  echo "阻止：以下 crate 的公开接口（pub fn/struct/enum/trait）有改动，但对应的" >&2
  echo "CONTRACT_ALIGNMENT.md 没有在本次提交中一起更新：" >&2
  printf '  %s\n' "${MISSING[@]}" >&2
  echo "按 CLAUDE.md 阶段三「契约级：接口行为对齐」，公开接口变化必须同步补充" >&2
  echo "对照表条目（TS 行为 / Rust 行为 / 是否一致 / 差异原因）。" >&2
  exit 2
fi

exit 0