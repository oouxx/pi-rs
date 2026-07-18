#!/usr/bin/env bash
# PreToolUse hook: runs before every Bash tool call.
# Blocks git commands that can destroy uncommitted work or silently discard
# state — relevant if you ever run multiple Claude Code sessions/worktrees
# on this repo at once (see Bun's postmortem: `git stash`/`git reset --hard`
# across parallel agents caused them to step on each other).

set -euo pipefail

CMD="${CLAUDE_TOOL_INPUT_COMMAND:-}"
[ -z "$CMD" ] && exit 0

if echo "$CMD" | grep -qE '\bgit\s+(stash|reset\s+--hard|clean\s+-[a-z]*f)'; then
  echo "阻止：检测到危险 git 命令（stash / reset --hard / clean -f）。" >&2
  echo "如果在并行 workflow 中运行，这类命令会清掉其他会话未提交的改动。" >&2
  echo "改用针对具体文件的 'git add <file> && git commit' 提交当前进度。" >&2
  exit 2
fi

exit 0