#!/bin/sh
# The reader browser's style and layout speed on THIS machine, in numbers that can be put next to another machine's (F-BROX-15).
#
#   sh dist/scripts/browser-speed-report.sh [OUT_FILE]
#
# Builds the shell's tests with the `fast` profile (release-like: a debug build says nothing about speed), runs the ignored test
# `speed_report_style_and_layout_of_synthetic_pages` (three synthetic pages of 100, 1000 and 4000 blocks with a style sheet; the
# median of IYAKKI_SPEED_RUNS runs, default 5) and prints one table:
#   parse_ms         HTML text to a tree
#   style_ms         the tree to the styled document (cascade, no network)
#   first_frame_ms   the first drawing of the opened page: layout and paint at 1280 x 720
#   cached_frame_ms  a drawing of the same page again (layout kept)
#   scroll_frame_ms  a page down and a drawing
# followed by the machine's name, cores, memory and the commit, so two files can be compared. The text goes to OUT_FILE too (default
# $IYAKKI_WORK/speed-report-<host>-<date>.txt). Needs cargo; no network, no browser, no root. Close other programs first: the numbers
# are wall clock. Run it on the MacBookPro14,1 and on the build Mac, then fill the table in docs/BROWSER-SPEED.md.
set -eu
cd "$(dirname "$0")/../.."
WORK=${IYAKKI_WORK:-$HOME/iyakki-work}
HOST=$(hostname -s 2>/dev/null || echo host)
OUT=${1:-$WORK/speed-report-$HOST-$(date +%Y%m%d).txt}
CPU=$( (sysctl -n machdep.cpu.brand_string 2>/dev/null || grep -m1 'model name' /proc/cpuinfo 2>/dev/null | cut -d: -f2 || echo unknown) | sed 's/^ *//')
CORES=$( (sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 0) )
MEM=$( (sysctl -n hw.memsize 2>/dev/null | awk '{printf "%d MB", $1/1048576}') 2>/dev/null || awk '/MemTotal/ {printf "%d MB", $2/1024}' /proc/meminfo 2>/dev/null || echo unknown)
MODEL=$( (sysctl -n hw.model 2>/dev/null) || cat /sys/class/dmi/id/product_name 2>/dev/null || echo unknown)
COMMIT=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)
mkdir -p "$(dirname "$OUT")"
LOG=$(mktemp)
{
  echo "machine: $MODEL; cpu: $CPU; cores: $CORES; memory: $MEM; commit: $COMMIT; date: $(date -u +%Y-%m-%dT%H:%MZ); profile: fast"
  CARGO_INCREMENTAL=0 cargo test --profile fast -p iyakki-shell --lib speed_report_style_and_layout -- --ignored --nocapture --test-threads=1 > "$LOG" 2>&1 || true
  grep '^SPEEDREPORT' "$LOG" | sed 's/^SPEEDREPORT\t//'
} | tee "$OUT"
if ! grep -q 'small' "$OUT"; then echo 'no numbers: the run failed, its last lines:' >&2; tail -20 "$LOG" >&2; rm -f "$LOG"; exit 1; fi
rm -f "$LOG"
echo "written to $OUT" >&2
