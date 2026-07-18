#!/usr/bin/env bash
# Stop hook: runs before Claude Code ends a turn.
#
# Scans every CONTRACT_ALIGNMENT.md in the repo for table rows where the
# "是否一致" column is 否 (inconsistent) but the row doesn't reference
# DEVIATIONS.md. Such a row means: a known behavioral mismatch that was
# never actually resolved — either fix the mismatch (change 否 to 是) or
# formally register it as an accepted deviation in DEVIATIONS.md and note
# that in the row. A bare 否 left hanging is what this hook exists to catch.
#
# This is a heuristic line-based markdown table scan, not a real parser —
# it assumes one row per line with `|`-delimited columns, matching the
# table format defined in CLAUDE.md. It will miss multi-line cells.

set -euo pipefail

REPO_ROOT=$(git rev-parse --show-toplevel 2>/dev/null || echo .)
cd "$REPO_ROOT"

TABLES=$(find . -name 'CONTRACT_ALIGNMENT.md' -not -path '*/target/*' 2>/dev/null || true)
[ -z "$TABLES" ] && exit 0

BAD_ROWS=""
while IFS= read -r table; do
  [ -z "$table" ] && continue
  while IFS= read -r line; do
    # match a table row whose "是否一致" cell is exactly 否 (allow surrounding spaces)
    if echo "$line" | grep -qE '\|[[:space:]]*否[[:space:]]*\|'; then
      if ! echo "$line" | grep -q 'DEVIATIONS'; then
        BAD_ROWS="${BAD_ROWS}${table}: ${line}
"
      fi
    fi
  done < "$table"
done <<< "$TABLES"

if [ -n "$BAD_ROWS" ]; then
  echo "阻止：以下 CONTRACT_ALIGNMENT.md 行标记「是否一致 = 否」，但未引用" >&2
  echo "DEVIATIONS.md，说明这是一个还没处理完的行为不一致：" >&2
  printf '%s' "$BAD_ROWS" >&2
  echo "要么修复差异把该行改成「是」，要么在 DEVIATIONS.md 登记为已确认偏差，" >&2
  echo "并在这一行的「差异原因」列注明引用（如「见 DEVIATIONS.md #3」）。" >&2
  exit 2
fi

exit 0