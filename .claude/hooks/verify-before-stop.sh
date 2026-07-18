#!/usr/bin/env bash
# Stop hook: runs before Claude Code ends a turn.
# Only fires verification if this turn touched .rs files (checked via git
# diff against the last commit). Blocks the turn from ending (exit 2) until
# `cargo clippy` and `cargo test` pass, so a turn can't be declared "done"
# with a red build. Claude Code force-ends the turn after 8 consecutive
# blocks, so this can't loop forever even if something is stuck.

set -euo pipefail

cd "$(git rev-parse --show-toplevel 2>/dev/null || echo .)"

CHANGED_RS=$(git diff --name-only HEAD -- '*.rs' 2>/dev/null || true)
CHANGED_RS_STAGED=$(git diff --name-only --cached HEAD -- '*.rs' 2>/dev/null || true)

if [ -z "$CHANGED_RS" ] && [ -z "$CHANGED_RS_STAGED" ]; then
  exit 0
fi

echo "检测到 .rs 文件改动，运行 clippy + test 门禁检查..." >&2

if ! cargo clippy --all-targets --quiet -- -D warnings 2>&1 | tee /tmp/clippy_out.log; then
  echo "阻止：cargo clippy 未通过，见上方输出。修完再结束本轮。" >&2
  exit 2
fi

if ! cargo test --workspace --quiet 2>&1 | tee /tmp/test_out.log; then
  echo "阻止：cargo test --workspace 未通过，见上方输出。修完再结束本轮。" >&2
  exit 2
fi

exit 0