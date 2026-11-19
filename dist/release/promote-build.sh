#!/usr/bin/env bash
# Promote a shared build from one channel to another, for example dev to beta (docs/development/BUILD-SHARING.md).
#   promote-build.sh --variant pc-x86_64 --from dev --to beta [--build ID|latest] [--store URL] [--keep N]
#                    [--key FILE] [--token-file FILE] [--dry-run]
# Nothing is uploaded: the store holds each file once, and the promoted build names the same files. The list is read, the build copied
# into the target channel, the list signed HERE with the update key and sent (the store refuses it unless every blob is there).
set -euo pipefail

VARIANT=""; FROM=""; TO=""; BUILD="latest"; STORE="${IYAKKI_BUILD_STORE:-}"; KEEP=1; DRY=0
KEY="${IYAKKI_UPDATE_KEY:-$HOME/.config/iyakki/update-signing.key}"
TOKEN_FILE="${IYAKKI_BUILD_PUBLISH_TOKEN_FILE:-$HOME/.config/iyakki/build-store-publish.token}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; REPO="$(cd "$HERE/../.." && pwd)"

die() { echo "promote-build: $*" >&2; exit 1; }
usage() { sed -n '2,6p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit "${1:-0}"; }
[ $# -ge 1 ] || usage 1
while [ $# -gt 0 ]; do
  case "$1" in
    --variant) VARIANT="$2"; shift 2;; --from) FROM="$2"; shift 2;; --to) TO="$2"; shift 2;; --build) BUILD="$2"; shift 2;;
    --store) STORE="$2"; shift 2;; --keep) KEEP="$2"; shift 2;; --key) KEY="$2"; shift 2;;
    --token-file) TOKEN_FILE="$2"; shift 2;; --dry-run) DRY=1; shift;; -h|--help) usage 0;; *) die "unknown option $1";;
  esac
done
[ -n "$VARIANT" ] && [ -n "$FROM" ] && [ -n "$TO" ] || die "--variant, --from and --to are required"
STORE="${STORE%/}"
[ -n "$STORE" ] || die "give the store address with --store http://HOST:PORT (or IYAKKI_BUILD_STORE)"
[ -f "$KEY" ] || die "no update key at $KEY"
[ -f "$TOKEN_FILE" ] || die "no publish token file at $TOKEN_FILE"
TOOL=""
for c in "${IYAKKI_BUILDSHARE:-}" "$(command -v iyakki-buildshare 2>/dev/null || true)" \
  "${CARGO_TARGET_DIR:-$REPO/target}/x86_64-unknown-linux-musl/release/iyakki-buildshare" \
  "${CARGO_TARGET_DIR:-$REPO/target}/release/iyakki-buildshare" "${CARGO_TARGET_DIR:-$REPO/target}/debug/iyakki-buildshare"; do
  if [ -n "$c" ] && [ -x "$c" ]; then TOOL="$c"; break; fi
done
[ -n "$TOOL" ] || die "iyakki-buildshare not found (cargo build -p iyakki-store-server --bin iyakki-buildshare, or set IYAKKI_BUILDSHARE)"
command -v curl >/dev/null || die "curl is needed"

umask 077
W="$(mktemp -d)"; trap 'rm -rf "${W:?}"' EXIT
printf 'header = "Authorization: Bearer %s"\n' "$(tr -d '[:space:]' < "$TOKEN_FILE")" > "$W/auth.cfg"
cu() { curl -sS -K "$W/auth.cfg" "$@"; }

code="$(cu -o "$W/old.json" -w '%{http_code}' "$STORE/v1/builds.json" || true)"
[ "$code" = 200 ] || die "the store answered $code for the list (nothing published yet, a wrong token, or the store is not reachable)"
"$TOOL" promote --key "$KEY" --list "$W/old.json" --variant "$VARIANT" --from "$FROM" --to "$TO" --build "$BUILD" --keep "$KEEP" --out "$W/new.json" | tee "$W/plan.txt"
if [ "$DRY" = 1 ]; then echo "dry run: nothing sent"; exit 0; fi
code="$(cu -o "$W/put.out" -w '%{http_code}' -X PUT -H 'Content-Type: application/json' --data-binary @"$W/new.json" "$STORE/v1/builds.json" || true)"
[ "$code" = 200 ] || die "the store refused the list ($code): $(head -c 300 "$W/put.out")"
cu -o "$W/back.json" "$STORE/v1/builds.json" || die "cannot read the list back"
cmp -s "$W/back.json" "$W/new.json" || die "the store holds a different list than the one sent"
cu -X POST "$STORE/v1/builds/purge" | sed 's/^/   store: /'
echo "promoted $VARIANT $FROM -> $TO ($BUILD)"
