#!/bin/sh
# Time page loads in the full web engine (the one YouTube and Google use) on a running IYAKKI device or a desktop with the helper.
#   sh page-timing.sh [runs] URL...        default: 3 runs of google.com, youtube.com, wikipedia, bbc.com/news
# Each run is `iyakki browser-test` with a one-step spec (goto, which returns when the page has loaded); the time is wall clock
# from /proc/uptime (hundredths of a second), so run 1 is cold and runs 2 and 3 show what the engine's own disk cache and
# connection reuse give. Prints each run and the median. Needs the signed-in user's browser-test permission; no files are kept.
RUNS=3
case "$1" in [0-9]*) RUNS=$1; shift;; esac
[ $# -eq 0 ] && set -- https://www.google.com/search?q=rust https://www.youtube.com/ "https://en.wikipedia.org/wiki/Rust_(programming_language)" https://www.bbc.com/news
T=${TMPDIR:-/tmp}/page-timing.$$
mkdir -p "$T" || exit 1
now() { awk '{printf "%d", $1*100}' /proc/uptime; }
for u in "$@"; do
  host=$(echo "$u" | sed -E 's|^[a-z]+://([^/]+).*|\1|')
  printf '{"name":"timing","allow_domains":["%s"],"timeout_ms":60000,"steps":[{"goto":"%s"}]}\n' "$host" "$u" > "$T/spec.json"
  times=""
  i=1
  while [ $i -le "$RUNS" ]; do
    a=$(now); iyakki browser-test "$T/spec.json" >/dev/null 2>&1; rc=$?; b=$(now)
    ms=$(( (b - a) * 10 ))
    echo "$u run $i: $ms ms (exit $rc)"
    times="$times $ms"
    i=$((i + 1))
  done
  med=$(echo $times | tr ' ' '\n' | sort -n | awk '{a[NR]=$1} END {print a[int((NR+1)/2)]}')
  echo "MEDIAN $u $med ms"
done
rm -rf "$T"
