#!/usr/bin/env bash
# Regenerates the appendix of KNOWN-ISSUES.md (every FEATURES.md row that is not `done`) between its two markers.
#   dist/release/known-issues-appendix.sh [KNOWN-ISSUES.md]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
F="${1:-$ROOT/KNOWN-ISSUES.md}"
START='<!-- appendix:start -->'; END='<!-- appendix:end -->'
grep -q "$START" "$F" && grep -q "$END" "$F" || { echo "$F has no appendix markers" >&2; exit 1; }
TABLE="$(awk -F'|' '
  function trim(s) { gsub(/^ +| +$/, "", s); return s }
  function clean(s, n) { gsub(/\|/, "/", s); gsub(/`/, "", s); s = trim(s); if (length(s) > n) s = substr(s, 1, n) "..."; return s }
  /^\| F-/ { st = trim($5); if (st != "done") print "| " trim($2) " | " st " | " clean($3, 90) " | " clean($9, 220) " |" }
' "$ROOT/FEATURES.md")"
N="$(printf '%s\n' "$TABLE" | grep -c '^| F-' || true)"
{
  sed -n "1,/$START/p" "$F"
  echo
  echo "_Generated from FEATURES.md by dist/release/known-issues-appendix.sh: $N rows that are partial, planned, blocked or deprecated. The Notes column is the row's own note, shortened._"
  echo
  echo "| ID | Status | Feature | Note |"
  echo "|---|---|---|---|"
  printf '%s\n' "$TABLE"
  echo
  sed -n "/$END/,\$p" "$F"
} > "$F.new"
mv "$F.new" "$F"
echo "appendix: $N rows"
