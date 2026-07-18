#!/usr/bin/env bash
# PostToolUse hook: runs after Edit/Write on .rs files.
# Blocks (exit 2) if .unwrap()/.expect() appears outside #[cfg(test)] blocks
# or files under a tests/ directory. Enforces the CLAUDE.md rule mechanically
# instead of relying on the model remembering it.

set -euo pipefail

FILE_PATH="${CLAUDE_TOOL_INPUT_FILE_PATH:-}"
[ -z "$FILE_PATH" ] && exit 0
[[ "$FILE_PATH" != *.rs ]] && exit 0

# Skip files that are entirely test files by path convention.
case "$FILE_PATH" in
  */tests/*|*_test.rs|*/test_*.rs) exit 0 ;;
esac

# Find .unwrap()/.expect() calls, excluding lines inside #[cfg(test)] mod
# blocks (crude but effective: strip everything from the first `#[cfg(test)]`
# to end of file before grepping).
VIOLATIONS=$(awk '/#\[cfg\(test\)\]/{exit} {print}' "$FILE_PATH" \
  | grep -nE '\.unwrap\(\)|\.expect\(' || true)

if [ -n "$VIOLATIONS" ]; then
  echo "阻止：$FILE_PATH 中检测到非测试代码使用 .unwrap()/.expect()，违反 CLAUDE.md 代码规范。" >&2
  echo "$VIOLATIONS" >&2
  echo "请改用 Result<T, E> 显式传播错误（见 CLAUDE.md「代码规范」）。" >&2
  exit 2
fi

exit 0