#!/usr/bin/env bash
# Publish an update pack to the shared build store (docs/development/BUILD-SHARING.md).
#   publish-build.sh PACKDIR --variant pc-x86_64 --channel dev --build-id ID [--store URL] [--keep N] [--commit SHA]
#                    [--key FILE] [--token-file FILE] [--dry-run] [--no-purge]
# In this order, and why:
#   1 verifies the pack (update.json exists, every listed file has the right size and SHA-256)
#   2 reads the store's current signed list (404 = nothing published yet)
#   3 adds the build, applies "keep the newest N per machine type and channel" (default 1), signs the list HERE with the update key
#   4 uploads the files as blobs (resumable, skipping blobs the store already has)
#   5 sends the new list (the store refuses it unless every blob is there; the old build stays fetchable until step 6)
#   6 reads the list and every blob back; only then asks the store to remove the blobs only dropped builds used
# The update key never leaves this machine; tokens are read from files and never put on a command line.
set -euo pipefail

VARIANT=""; CHANNEL=""; BUILD_ID=""; STORE="${IYAKKI_BUILD_STORE:-}"; KEEP=1; COMMIT=""
KEY="${IYAKKI_UPDATE_KEY:-$HOME/.config/iyakki/update-signing.key}"
TOKEN_FILE="${IYAKKI_BUILD_PUBLISH_TOKEN_FILE:-$HOME/.config/iyakki/build-store-publish.token}"
DRY=0; PURGE=1; PIECE=$((32 * 1024 * 1024))
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; REPO="$(cd "$HERE/../.." && pwd)"

die() { echo "publish-build: $*" >&2; exit 1; }
usage() { sed -n '2,13p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit "${1:-0}"; }

[ $# -ge 1 ] || usage 1
case "$1" in -h|--help) usage 0;; esac
PACK="$1"; shift
while [ $# -gt 0 ]; do
  case "$1" in
    --variant) VARIANT="$2"; shift 2;; --channel) CHANNEL="$2"; shift 2;; --build-id) BUILD_ID="$2"; shift 2;;
    --store) STORE="$2"; shift 2;; --keep) KEEP="$2"; shift 2;; --commit) COMMIT="$2"; shift 2;;
    --key) KEY="$2"; shift 2;; --token-file) TOKEN_FILE="$2"; shift 2;; --dry-run) DRY=1; shift;; --no-purge) PURGE=0; shift;;
    -h|--help) usage 0;; *) die "unknown option $1";;
  esac
done
[ -d "$PACK" ] || die "$PACK is not a folder"
[ -n "$VARIANT" ] && [ -n "$CHANNEL" ] && [ -n "$BUILD_ID" ] || die "--variant, --channel and --build-id are required"
STORE="${STORE%/}"
[ -n "$STORE" ] || die "give the store address with --store http://HOST:PORT (or IYAKKI_BUILD_STORE)"
[ -f "$KEY" ] || die "no update key at $KEY (make one: iyakki-buildshare keygen --out $KEY, then commit the printed public key as dist/release/update-signing.pub)"
[ -f "$TOKEN_FILE" ] || die "no publish token file at $TOKEN_FILE"
[ -n "$COMMIT" ] || COMMIT="$(git -C "$REPO" rev-parse --short=12 HEAD 2>/dev/null || true)"

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
PIN="$("$TOOL" pubkey --key "$KEY")"
cu() { curl -sS -K "$W/auth.cfg" "$@"; }

echo "1/6 verifying the pack"
"$TOOL" verify-pack --pack "$PACK"

echo "2/6 reading the store's list"
code="$(cu -o "$W/old.json" -w '%{http_code}' "$STORE/v1/builds.json" || true)"
case "$code" in 200) OLD="$W/old.json";; 404) OLD="-";; *) die "the store answered $code for the list (is the token right and the store reachable?)";; esac

echo "3/6 adding the build and signing the list (keep $KEEP)"
"$TOOL" sign-add --key "$KEY" --list "$OLD" --pack "$PACK" --variant "$VARIANT" --channel "$CHANNEL" --build-id "$BUILD_ID" \
  --commit "$COMMIT" --keep "$KEEP" --out "$W/new.json" > "$W/plan.txt"
grep '^FILE ' "$W/plan.txt" | awk '{print $4 "\t" $2 "\t" $3}' > "$W/files.tsv"
grep '^DROPPED ' "$W/plan.txt" | sed 's/^/   /' || true
total=$(awk -F'\t' '{s+=$3} END{print s+0}' "$W/files.tsv")
echo "   $(wc -l < "$W/files.tsv") files, $total bytes"
if [ "$DRY" = 1 ]; then echo "dry run: nothing uploaded"; exit 0; fi

# One blob. A piece is sent with Content-Range; a cut upload continues where the store's part file ends.
upload() { # path sha size
  local f="$PACK/$1" sha="$2" size="$3" have=0 end n code
  if [ "$(cu -o /dev/null -w '%{http_code} %header{content-length}' -I "$STORE/v1/blobs/$sha" 2>/dev/null || true)" = "200 $size" ]; then
    echo "   have  $1"; return
  fi
  have="$(cu "$STORE/v1/blobs/$sha/part" 2>/dev/null | sed -n 's/.*"have":\([0-9]*\).*/\1/p')"; have="${have:-0}"
  [ "$have" -lt "$size" ] || have=0
  echo "   send  $1 ($size bytes, from $have)"
  while [ "$have" -lt "$size" ]; do
    n=$PIECE; [ $((size - have)) -lt $n ] && n=$((size - have))
    end=$((have + n - 1))
    code="$(dd if="$f" bs=1M iflag=skip_bytes,count_bytes skip="$have" count="$n" status=none |
      cu -o "$W/put.out" -w '%{http_code}' -X PUT -H "Content-Range: bytes $have-$end/$size" -H 'Content-Type: application/octet-stream' \
        --data-binary @- "$STORE/v1/blobs/$sha?component=build" || true)"
    case "$code" in 2??) ;; *) die "the store refused a piece of $1 ($code): $(head -c 200 "$W/put.out")";; esac
    have=$((end + 1))
  done
}
echo "4/6 uploading blobs"
sort -u -t$'\t' -k2,2 "$W/files.tsv" > "$W/uniq.tsv"
while IFS=$'\t' read -r path sha size; do upload "$path" "$sha" "$size"; done < "$W/uniq.tsv"

echo "5/6 sending the signed list"
code="$(cu -o "$W/put.out" -w '%{http_code}' -X PUT -H 'Content-Type: application/json' --data-binary @"$W/new.json" "$STORE/v1/builds.json" || true)"
[ "$code" = 200 ] || die "the store refused the list ($code): $(head -c 300 "$W/put.out")"

echo "6/6 checking the store, then removing what only the dropped builds used"
cu -o "$W/back.json" "$STORE/v1/builds.json" || die "cannot read the list back"
cmp -s "$W/back.json" "$W/new.json" || die "the store holds a different list than the one sent"
"$TOOL" select --list "$W/back.json" --pin "$PIN" --variant "$VARIANT" --channel "$CHANNEL" --build "$BUILD_ID" > "$W/sel.txt"
while IFS=' ' read -r _ sha size _; do
  [ "$(cu -o /dev/null -w '%{http_code} %header{content-length}' -I "$STORE/v1/blobs/$sha")" = "200 $size" ] || die "blob $sha is not in the store as sent"
done < <(grep '^FILE ' "$W/sel.txt")
if [ "$PURGE" = 1 ]; then
  cu -X POST "$STORE/v1/builds/purge" | sed 's/^/   store: /'
else
  echo "   --no-purge: the old build's blobs stay until: curl -X POST -H 'Authorization: Bearer TOKEN' $STORE/v1/builds/purge"
fi
echo
echo "published $VARIANT/$CHANNEL build $BUILD_ID (commit ${COMMIT:-unknown}) to $STORE"
