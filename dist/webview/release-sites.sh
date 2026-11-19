#!/bin/sh
# The per-release check of web apps in the full web engine (F-BROW-33): for each site in release-sites.tsv, open it in the engine as a
# signed-out visitor, wait for the sign-in screen, take a screenshot and say pass or fail. Run it on a device (or the QEMU web image)
# that has the full engine, with the network on, signed in to IYAKKI as a person who may run `iyakki browser-test`:
#
#   sh dist/webview/release-sites.sh                    every site, 1 run each
#   sh dist/webview/release-sites.sh --only Gmail       one site
#   sh dist/webview/release-sites.sh --out DIR          where the screenshots and specs go (default ./release-sites-out)
#   sh dist/webview/release-sites.sh --dry-run          print the specs and run nothing (needs only sh and python3)
#   sh dist/webview/release-sites.sh --list FILE        another list in the same format
#
# A pass means: the page loaded, the sign-in text showed within the time limit and the engine stayed up. It does NOT mean the app is usable
# (that needs an account and a person: the Manual column of F-BROW-33). Specs are the format of docs/BROWSER-AGENT.md; no password is
# in them. Exit status: 0 all passed, 1 some failed, 2 the list or the engine could not be used.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
LIST="$HERE/release-sites.tsv"
OUT=./release-sites-out
ONLY=""
DRY=0
TIMEOUT_MS=${IYAKKI_SITE_TIMEOUT_MS:-60000}
while [ $# -gt 0 ]; do
  case "$1" in
    --only) ONLY=$2; shift 2;;
    --out) OUT=$2; shift 2;;
    --list) LIST=$2; shift 2;;
    --dry-run) DRY=1; shift;;
    *) echo "unknown option $1" >&2; exit 2;;
  esac
done
[ -r "$LIST" ] || { echo "cannot read $LIST" >&2; exit 2; }
mkdir -p "$OUT" || exit 2
if [ "$DRY" = 0 ] && ! command -v iyakki >/dev/null 2>&1; then
  echo "iyakki is not on this machine: run this on a device (or use --dry-run)" >&2
  exit 2
fi
pass=0; fail=0
printf '%-14s %-6s %s\n' SITE RESULT DETAIL
# a JSON string from shell text: the only characters a list may carry that need care are the quote and the backslash
json_str() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'; }
while IFS="$(printf '\t')" read -r name url domains marker notes; do
  case "$name" in ''|'#'*) continue;; esac
  [ -n "$ONLY" ] && [ "$ONLY" != "$name" ] && continue
  doms=$(printf '%s' "$domains" | awk -F, '{for(i=1;i<=NF;i++){printf "%s\"%s\"", (i>1?",":""), $i}}')
  slug=$(printf '%s' "$name" | tr 'A-Z ' 'a-z-' | tr -cd 'a-z0-9-')
  spec="$OUT/$slug.json"
  cat > "$spec" <<JSON
{"name":"$(json_str "$name")","allow_domains":[$doms],"timeout_ms":$TIMEOUT_MS,"steps":[
 {"goto":"$(json_str "$url")"},
 {"wait_for":{"text":"$(json_str "$marker")"}},
 {"expect":{"no_text":"This browser or app may not be secure"}},
 {"screenshot":"$slug"}]}
JSON
  if [ "$DRY" = 1 ]; then
    python3 -c 'import json,sys; json.load(open(sys.argv[1])); print("spec ok:", sys.argv[1])' "$spec" || { fail=$((fail+1)); }
    continue
  fi
  start=$(date +%s)
  iyakki browser-test "$spec" --out "$OUT/$slug" --json > "$OUT/$slug.result.json" 2> "$OUT/$slug.err"
  rc=$?
  secs=$(( $(date +%s) - start ))
  if [ "$rc" = 0 ]; then
    printf '%-14s %-6s %s\n' "$name" pass "${secs}s"
    pass=$((pass+1))
  else
    why=$(head -c 160 "$OUT/$slug.err" | tr '\n' ' ')
    printf '%-14s %-6s %s\n' "$name" "FAIL($rc)" "${secs}s $why (see $OUT/$slug.*)"
    fail=$((fail+1))
  fi
done < "$LIST"
echo "passed $pass, failed $fail"
[ "$fail" = 0 ] || exit 1
