#!/usr/bin/env bash
# Tests publish-build.sh and fetch-build.sh against a LOCAL store on 127.0.0.1 (no k3s, no device, nothing outside a temporary folder):
# publish, fetch, resume (upload and download), tamper detection, wrong key / token, retention 1 and N, "latest" during and after a publish.
#   dist/release/test-build-share.sh        (needs the debug binaries: cargo build -p iyakki-store-server)
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; REPO="$(cd "$HERE/../.." && pwd)"
TARGET="${CARGO_TARGET_DIR:-$REPO/target}"
SERVER="${IYAKKI_STORE_SERVER:-$TARGET/debug/iyakki-store-server}"
export IYAKKI_BUILDSHARE="${IYAKKI_BUILDSHARE:-$TARGET/debug/iyakki-buildshare}"
[ -x "$SERVER" ] && [ -x "$IYAKKI_BUILDSHARE" ] || { echo "build first: cargo build -p iyakki-store-server" >&2; exit 2; }

T="$(mktemp -d)"; SPID=""
cleanup() { [ -z "$SPID" ] || kill "$SPID" 2>/dev/null || true; rm -rf "${T:?}"; }
trap cleanup EXIT
PASS=0
ok() { PASS=$((PASS + 1)); echo "ok   $*"; }
bad() { echo "FAIL $*" >&2; exit 1; }
expect_fail() { local what="$1"; shift; if "$@" >"$T/ef.out" 2>&1; then bad "$what: it succeeded but must be refused"; fi; ok "$what (refused: $(tail -n1 "$T/ef.out" | cut -c1-90))"; }

umask 077
S="$T/secrets"; mkdir -p "$S"
"$IYAKKI_BUILDSHARE" keygen --out "$S/update.key" > "$S/update.pub"
"$IYAKKI_BUILDSHARE" keygen --out "$S/other.key" > "$S/other.pub"
echo "publish-token-for-the-local-test-0123456789" > "$S/publish.token"
echo "read-token-for-the-local-test-0123456789ab" > "$S/read.token"
echo "wrong-token-for-the-local-test-0123456789a" > "$S/wrong.token"
echo "catalog-seed-for-the-local-test-0123456789" > "$S/seed"
PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')"
URL="http://127.0.0.1:$PORT"; DATA="$T/data"; mkdir -p "$DATA"
"$SERVER" --data "$DATA" --listen "127.0.0.1:$PORT" --publish-token-file "$S/publish.token" --read-token-file "$S/read.token" \
  --catalog-seed-file "$S/seed" --builds-key "$(cat "$S/update.pub")" --max-component-mib 256 > "$T/server.log" 2>&1 &
SPID=$!
for _ in $(seq 50); do curl -sf "$URL/healthz" >/dev/null 2>&1 && break; sleep 0.1; done
curl -sf "$URL/healthz" >/dev/null || bad "the local store did not start"

# a small pack that passes the device's own checks (update.json with sizes and hashes)
mkpack() { # dir root-seed kernel-seed
  local d="$1"; mkdir -p "$d/boot/GRUB"
  head -c $((40 * 1024 * 1024)) /dev/urandom > "$d/boot/ROOT.SFS"
  head -c 4000000 /dev/zero | tr '\0' 'k' > "$d/boot/bzImage"
  printf 'kernel-%s\n' "$3" >> "$d/boot/bzImage"
  printf 'menu %s\n' "$2" > "$d/boot/GRUB/GRUB.CFG"; head -c 100000 /dev/urandom > "$d/boot/INITRD"
  python3 - "$d" <<'PY'
import hashlib, json, os, sys
d = sys.argv[1]; files = []
for root, _, names in os.walk(d):
    for n in sorted(names):
        p = os.path.join(root, n); rel = os.path.relpath(p, d)
        if rel == "update.json": continue
        b = open(p, "rb").read(); files.append({"path": rel, "size": len(b), "sha256": hashlib.sha256(b).hexdigest()})
files.sort(key=lambda f: f["path"])
json.dump({"format": 1, "version": "0.1.0-test", "variant": "pc", "built": "", "source": "test", "secure_boot": False, "files": files}, open(os.path.join(d, "update.json"), "w"), indent=2)
PY
}
mkpack "$T/packA" a K1
mkpack "$T/packB" b K1      # same kernel text as A: its blob is shared, the root image differs
mkpack "$T/packC" c K3
cp "$T/packA/boot/bzImage" "$T/kA"; cmp -s "$T/packA/boot/bzImage" "$T/packB/boot/bzImage" || bad "test setup: A and B should share the kernel"

publish() { "$HERE/publish-build.sh" "$@" --store "$URL" --token-file "$S/publish.token" --key "$S/update.key"; }
fetch() { "$HERE/fetch-build.sh" --store "$URL" --token-file "$S/read.token" --pubkey "$S/update.pub" "$@"; }
nblobs() { find "$DATA/blobs" -type f | wc -l; }
blob_of() { sha256sum "$1" | cut -d' ' -f1; }

# ---- upload resume: the store already holds the first 5 MiB of A's root image
RA="$(blob_of "$T/packA/boot/ROOT.SFS")"; SZ=$(stat -c %s "$T/packA/boot/ROOT.SFS")
code="$(head -c $((5 * 1024 * 1024)) "$T/packA/boot/ROOT.SFS" | curl -s -o /dev/null -w '%{http_code}' -X PUT -H "Authorization: Bearer $(cat "$S/publish.token")" \
  -H "Content-Range: bytes 0-$((5 * 1024 * 1024 - 1))/$SZ" --data-binary @- "$URL/v1/blobs/$RA?component=build")"
[ "$code" = 202 ] || [ "$code" = 200 ] || [ "$code" = 201 ] || bad "could not start a partial upload ($code)"
publish "$T/packA" --variant pc-x86_64 --channel dev --build-id a1 --dry-run > "$T/dry.out"
[ "$(nblobs)" = 0 ] && ok "dry run uploads nothing and keeps nothing" || bad "dry run left blobs"
publish "$T/packA" --variant pc-x86_64 --channel dev --build-id a1 > "$T/pubA.out"
grep -q "send  boot/ROOT.SFS .*from 5242880" "$T/pubA.out" && ok "an upload cut at 5 MiB resumed from 5 MiB" || { cat "$T/pubA.out"; bad "upload did not resume"; }
[ "$(nblobs)" = 5 ] && ok "A is in the store: 4 files + update.json = 5 blobs" || bad "blobs after A: $(nblobs)"

# ---- publishing the same files again skips them
publish "$T/packA" --variant pc-x86_64 --channel dev --build-id a1 > "$T/pubA2.out"
[ "$(grep -c '   have ' "$T/pubA2.out")" = 5 ] && ok "a second publish of the same pack sends nothing (5 blobs already there)" || { cat "$T/pubA2.out"; bad "skip"; }

# ---- fetch: the files come back byte for byte
fetch --variant pc-x86_64 --channel dev --out "$T/outA" > "$T/fetchA.out"
diff -r "$T/packA" "$T/outA" > /dev/null && ok "fetch latest = the published pack, byte for byte" || bad "fetched pack differs"
grep -q "iyakki dev update $T/outA --device NAME" "$T/fetchA.out" && ok "fetch prints the dev update command and touches no device" || bad "no install hint"
fetch --list > "$T/l0.out"; grep -q "pc-x86_64/dev a1 " "$T/l0.out" && ok "--list shows the build" || bad "--list"

# ---- download resume: a half-downloaded file continues
mkdir -p "$T/outR/boot"; head -c 7000000 "$T/packA/boot/ROOT.SFS" > "$T/outR/boot/ROOT.SFS.part"
fetch --variant pc-x86_64 --channel dev --out "$T/outR" > /dev/null
diff -r "$T/packA" "$T/outR" > /dev/null && ok "a half-downloaded file resumes and verifies" || bad "resume"
# a finished folder is not downloaded again
fetch --variant pc-x86_64 --channel dev --out "$T/outR" > "$T/again.out"
grep -q "   have  boot/ROOT.SFS" "$T/again.out" && ok "a finished folder is not fetched twice" || bad "have"

# ---- retention 1: publish B (shares the kernel with A, new root image)
RB="$(blob_of "$T/packB/boot/ROOT.SFS")"; KB="$(blob_of "$T/packB/boot/bzImage")"
publish "$T/packB" --variant pc-x86_64 --channel dev --build-id b1 --no-purge > "$T/pubB.out"
# DURING: list says B is latest, A's own blobs are still in the store (a fetch that started on A can finish)
[ -f "$DATA/blobs/$RA" ] && ok "before the purge the old build's blobs are still there" || bad "A removed too early"
fetch --variant pc-x86_64 --channel dev --out "$T/outB" > /dev/null && diff -r "$T/packB" "$T/outB" > /dev/null && ok "latest is B at once, and fetches while A's blobs wait" || bad "fetch latest after list update"
curl -s -X POST -H "Authorization: Bearer $(cat "$S/publish.token")" "$URL/v1/builds/purge" > "$T/purge.out"; grep -q '"removed_blobs":' "$T/purge.out" && ok "purge answered" || bad "purge"
[ ! -f "$DATA/blobs/$RA" ] && [ -f "$DATA/blobs/$RB" ] && [ -f "$DATA/blobs/$KB" ] && ok "after the purge: A's root image is gone, B's root image and the shared kernel stay" || bad "purge result"
[ "$(nblobs)" = 5 ] && ok "store holds exactly B's 5 blobs (A's root image, GRUB.CFG and update.json went)" || bad "blobs after B: $(nblobs)"
expect_fail "the old build id is no longer there" fetch --variant pc-x86_64 --channel dev --build a1 --out "$T/outOld"
fetch --variant pc-x86_64 --channel dev --out "$T/outB2" > /dev/null && diff -r "$T/packB" "$T/outB2" >/dev/null && ok "latest still fetches after the purge" || bad "latest after purge"
# the normal path (no --no-purge) purges by itself
publish "$T/packC" --variant pc-x86_64 --channel dev --build-id c1 > "$T/pubC.out"
grep -q '"removed_blobs":' "$T/pubC.out" && [ ! -f "$DATA/blobs/$RB" ] && ok "publish C purged B's unique blobs by itself" || bad "auto purge"
[ "$(nblobs)" = 5 ] && ok "still one build in the store" || bad "blobs after C: $(nblobs)"

# ---- other machine types and channels keep their own latest; --keep 2
publish "$T/packA" --variant pc-aarch64 --channel dev --build-id x1 > /dev/null
publish "$T/packB" --variant pc-x86_64 --channel beta --build-id b1 --keep 2 > /dev/null
publish "$T/packA" --variant pc-x86_64 --channel beta --build-id b2 --keep 2 > /dev/null
fetch --list > "$T/list.out"
[ "$(grep -c 'pc-x86_64/beta' "$T/list.out")" = 2 ] && grep -q "pc-aarch64/dev x1" "$T/list.out" && grep -q "pc-x86_64/dev c1" "$T/list.out" && ok "retention is per machine type and channel (keep 2 holds two)" || { cat "$T/list.out"; bad "retention per type"; }
publish "$T/packC" --variant pc-x86_64 --channel beta --build-id b3 --keep 2 > /dev/null
fetch --list > "$T/l2.out"
if grep 'pc-x86_64/beta' "$T/l2.out" | grep -q " b1 "; then bad "keep 2 should have dropped b1"; else ok "keep 2 dropped the oldest beta build"; fi

# ---- promote: dev's newest build becomes gamma's newest, with no upload (the blob count stays)
promote() { "$HERE/promote-build.sh" "$@" --store "$URL" --token-file "$S/publish.token" --key "$S/update.key"; }
N0=$(nblobs)
promote --variant pc-x86_64 --from dev --to gamma --dry-run > "$T/prodry.out"
fetch --list | grep -q "pc-x86_64/gamma" && bad "a dry run promoted" || ok "promote --dry-run sends nothing"
promote --variant pc-x86_64 --from dev --to gamma > "$T/promote.out"
fetch --variant pc-x86_64 --channel gamma --out "$T/outP" > /dev/null && diff -r "$T/packC" "$T/outP" > /dev/null && ok "a promoted build fetches from the new channel, byte for byte" || bad "promoted fetch"
[ "$(nblobs)" = "$N0" ] && ok "promoting uploaded and removed no blob" || bad "blobs changed by promote: $N0 -> $(nblobs)"
fetch --variant pc-x86_64 --channel dev --out "$T/outP2" > /dev/null && diff -r "$T/packC" "$T/outP2" > /dev/null && ok "the source channel is unchanged" || bad "source channel"
expect_fail "promoting from a channel with no build is refused" promote --variant pc-x86_64 --from nochannel --to gamma
expect_fail "the read token cannot promote" "$HERE/promote-build.sh" --variant pc-x86_64 --from dev --to delta --store "$URL" --token-file "$S/read.token" --key "$S/update.key"

# ---- tamper: a changed byte in a stored blob (same size) is refused and never lands as a file
KC="$(blob_of "$T/packC/boot/bzImage")"
cp "$DATA/blobs/$KC" "$T/kc.bak"; python3 -c "
import sys; p=sys.argv[1]; b=bytearray(open(p,'rb').read()); b[100]^=1; open(p,'wb').write(b)" "$DATA/blobs/$KC"
expect_fail "a changed byte in a stored file is refused" fetch --variant pc-x86_64 --channel dev --out "$T/outT"
[ ! -e "$T/outT/boot/bzImage" ] && ok "the damaged file was not kept" || bad "damaged file kept"
cp "$T/kc.bak" "$DATA/blobs/$KC"
# a changed list (any byte) is refused: the signature no longer matches
cp "$DATA/builds.json" "$T/builds.bak"; sed -i 's/0\.1\.0-test/0.9.9-evil/' "$DATA/builds.json"
expect_fail "a changed list is refused" fetch --variant pc-x86_64 --channel dev --out "$T/outL"
cp "$T/builds.bak" "$DATA/builds.json"
# a list signed by another key is refused
expect_fail "the wrong public key is refused" "$HERE/fetch-build.sh" --store "$URL" --token-file "$S/read.token" --pubkey "$S/other.pub" --variant pc-x86_64 --channel dev --out "$T/outK"
expect_fail "the store refuses a list signed by another key" "$HERE/publish-build.sh" "$T/packA" --variant pc-x86_64 --channel dev --build-id z --store "$URL" --token-file "$S/publish.token" --key "$S/other.key"
# tokens
expect_fail "a wrong read token is refused" "$HERE/fetch-build.sh" --store "$URL" --token-file "$S/wrong.token" --pubkey "$S/update.pub" --variant pc-x86_64 --channel dev --out "$T/outW"
expect_fail "the read token cannot publish" "$HERE/publish-build.sh" "$T/packA" --variant pc-x86_64 --channel dev --build-id z --store "$URL" --token-file "$S/read.token" --key "$S/update.key"
expect_fail "no token at all is refused" curl -sf "$URL/v1/builds.json"
# a damaged pack is refused before anything is sent
cp -r "$T/packA" "$T/packBad"; printf 'x' >> "$T/packBad/boot/INITRD"
expect_fail "a pack whose file does not match update.json is refused" publish "$T/packBad" --variant pc-x86_64 --channel dev --build-id bad
# a fetch is repaired when the local file is damaged
fetch --variant pc-x86_64 --channel dev --out "$T/outG" > /dev/null; printf 'x' >> "$T/outG/boot/INITRD"
fetch --variant pc-x86_64 --channel dev --out "$T/outG" > /dev/null && diff -r "$T/packC" "$T/outG" > /dev/null && ok "a damaged local file is fetched again" || bad "repair"
# a key file other users can read is refused
chmod 644 "$S/other.key"
expect_fail "a readable signing key is refused" "$HERE/publish-build.sh" "$T/packA" --variant pc-x86_64 --channel dev --build-id z --store "$URL" --token-file "$S/publish.token" --key "$S/other.key"
chmod 600 "$S/other.key"
grep -q "$(cat "$S/publish.token")" "$T/server.log" "$T"/*.out && bad "a token leaked into a log" || ok "no token appears in any output"
echo "all $PASS checks passed"
