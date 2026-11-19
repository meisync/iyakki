#!/usr/bin/env bash
# Fetch a shared build for your machine type and check it (docs/development/BUILD-SHARING.md). It never touches a device.
#   fetch-build.sh --store URL --token-file FILE --variant pc-x86_64 --channel dev [--build ID|latest] --out DIR
#                  [--pubkey FILE|HEX] [--list]
# --list prints the builds the store holds (and exits). The list is accepted only when it is signed by the update public key
# (dist/release/update-signing.pub in the repository), and every file only when its size and SHA-256 match that list.
set -euo pipefail

STORE="${IYAKKI_BUILD_STORE:-}"; TOKEN_FILE="${IYAKKI_BUILD_READ_TOKEN_FILE:-$HOME/.config/iyakki/build-store-read.token}"
VARIANT=""; CHANNEL="dev"; BUILD="latest"; OUT=""; LIST=0
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; REPO="$(cd "$HERE/../.." && pwd)"
PUBKEY="${IYAKKI_UPDATE_PUBKEY:-$REPO/dist/release/update-signing.pub}"

die() { echo "fetch-build: $*" >&2; exit 1; }
usage() { sed -n '2,6p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit "${1:-0}"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --store) STORE="$2"; shift 2;; --token-file) TOKEN_FILE="$2"; shift 2;; --variant) VARIANT="$2"; shift 2;;
    --channel) CHANNEL="$2"; shift 2;; --build) BUILD="$2"; shift 2;; --out) OUT="$2"; shift 2;;
    --pubkey) PUBKEY="$2"; shift 2;; --list) LIST=1; shift;; -h|--help) usage 0;; *) die "unknown option $1";;
  esac
done
STORE="${STORE%/}"
[ -n "$STORE" ] || die "give the store address with --store http://HOST:PORT (or IYAKKI_BUILD_STORE)"
[ -f "$TOKEN_FILE" ] || die "no read token file at $TOKEN_FILE (ask the publisher for the read token and save it there, mode 600)"
if [ "$LIST" = 0 ]; then [ -n "$VARIANT" ] && [ -n "$OUT" ] || die "--variant and --out are required"; fi

find_tool() {
  local c
  for c in "${IYAKKI_BUILDSHARE:-}" "$(command -v iyakki-buildshare 2>/dev/null || true)" \
    "${CARGO_TARGET_DIR:-$REPO/target}/x86_64-unknown-linux-musl/release/iyakki-buildshare" \
    "${CARGO_TARGET_DIR:-$REPO/target}/release/iyakki-buildshare" "${CARGO_TARGET_DIR:-$REPO/target}/debug/iyakki-buildshare"; do
    if [ -n "$c" ] && [ -x "$c" ]; then echo "$c"; return; fi
  done
  die "iyakki-buildshare not found (cargo build --release -p iyakki-store-server --bin iyakki-buildshare, or set IYAKKI_BUILDSHARE)"
}
TOOL="$(find_tool)"
command -v curl >/dev/null || die "curl is needed"
umask 077
W="$(mktemp -d)"; trap 'rm -rf "${W:?}"' EXIT
printf 'header = "Authorization: Bearer %s"\n' "$(tr -d '[:space:]' < "$TOKEN_FILE")" > "$W/auth.cfg"
cu() { curl -sS -K "$W/auth.cfg" "$@"; }

get_list() {
  local code
  code="$(cu -o "$W/list.json" -w '%{http_code}' "$STORE/v1/builds.json" || true)"
  case "$code" in 200) ;; 401) die "the store refused the read token";; 404) die "the store has no builds yet";; *) die "the store answered $code (is $STORE reachable?)";; esac
  "$TOOL" verify --list "$W/list.json" --pin "$PUBKEY" > "$W/summary.txt" || die "the build list is not signed by the update key $PUBKEY: refused"
}

get_list
if [ "$LIST" = 1 ]; then
  echo "machine-type/channel  build-id  version  commit  published(unix)  bytes"
  grep -v '^LIST ' "$W/summary.txt" || true
  exit 0
fi

fetch_once() {
  "$TOOL" select --list "$W/list.json" --pin "$PUBKEY" --variant "$VARIANT" --channel "$CHANNEL" --build "$BUILD" > "$W/sel.txt" || return 2
  ID="$(sed -n 's/^BUILD \([^ ]*\) .*/\1/p' "$W/sel.txt")"
  mkdir -p "${OUT:?}"
  local sha size path code
  while IFS=' ' read -r _ sha size path; do
    mkdir -p "$OUT/$(dirname "$path")"
    if [ -f "$OUT/$path" ] && [ "$(stat -c %s "$OUT/$path")" = "$size" ] && [ "$(sha256sum "$OUT/$path" | cut -d' ' -f1)" = "$sha" ]; then
      echo "   have  $path"; continue
    fi
    echo "   fetch $path ($size bytes)"
    # resume from the .part file; a 416 means it is already complete
    code="$(cu -C - -o "$OUT/$path.part" -w '%{http_code}' "$STORE/v1/blobs/$sha" || true)"
    case "$code" in 200|206|416) ;; 404) rm -f "${OUT:?}/${path:?}.part"; return 3;; *) return 4;; esac
    if [ "$(stat -c %s "$OUT/$path.part")" != "$size" ]; then echo "   $path: wrong size, discarded" >&2; rm -f "${OUT:?}/${path:?}.part"; return 4; fi
    if [ "$(sha256sum "$OUT/$path.part" | cut -d' ' -f1)" != "$sha" ]; then echo "   $path: SHA-256 does not match the signed list, discarded" >&2; rm -f "${OUT:?}/${path:?}.part"; return 5; fi
    mv "$OUT/$path.part" "$OUT/$path"
  done < <(grep '^FILE ' "$W/sel.txt")
}

echo "fetching $VARIANT/$CHANNEL $BUILD from $STORE"
tries=0
while :; do
  set +e; fetch_once; rc=$?; set -e
  [ $rc = 0 ] && break
  case $rc in
    2) die "no such build for $VARIANT in channel $CHANNEL (try --list)";;
    3) tries=$((tries + 1)); [ $tries -le 2 ] || die "a file vanished from the store twice (a publish replaced the build); run again"
       echo "   a file is gone: the build was replaced while fetching; reading the new list"; get_list;;
    5) die "a downloaded file does not match the signed list: refused";;
    *) die "download failed (run the same command again to resume)";;
  esac
done
"$TOOL" check-dir --list "$W/list.json" --pin "$PUBKEY" --variant "$VARIANT" --channel "$CHANNEL" --build "$ID" --dir "$OUT" || die "the folder failed the final check"
echo
echo "build $ID is in $OUT, verified (signature, sizes, SHA-256, update.json)."
echo "To install it on one of your paired devices (this script did not touch any device):"
echo "  iyakki dev update $OUT --device NAME"
echo "  iyakki dev reboot --device NAME"
echo "See docs/DEVELOPER-MODE.md section 7."
