#!/bin/sh
# Upgrade test in QEMU (v1.0.0 gate item 5, docs/development/DATA-LAYOUT-AND-UPGRADE.md): a data disk that holds what an alpha.1
# install left (tests/fixtures/upgrade/alpha1, synthetic) is booted by the CURRENT image, and the data must come through.
#
#   dist/buildroot/qemu-upgrade-test.sh [build-output-dir]        (default: target/buildroot-libre, the image the persist test uses, opened read-only)
#
# Boot 1  selftest persist2: the volume is found and mounted, the marker is there, and the boot itself changed no file of the volume
#         (a boot is not an upgrade: nothing is migrated before the shell opens its stores).
# Boot 2  selftest upgrade: the stores are opened the way the system opens them (settings, dashboard sources, inbox); the settings
#         are the fixture's (2 providers, chat model qwen3:8b), nothing was lost, `settings.json.pre-v1` is the old file byte for byte.
# Boot 3  the same again: nothing is rewritten (idempotent), the backup is still the old file.
#
# Needs an image built with the `upgrade` selftest stage (iyakki-core main.rs, added with this script). An older image prints SKIP for
# boots 2 and 3 and exits 2 after boot 1 passed. Never writes to the image or to a real drive: the disk is a temporary file made with
# mke2fs -d. Never run it against target/buildroot-macweb while repack.sh runs.
set -eu
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${1:-$ROOT/target/buildroot-libre}"
QEMU="${QEMU:-qemu-system-x86_64}"
FIX="$ROOT/tests/fixtures/upgrade/alpha1"
if pgrep -f "repack.sh" >/dev/null 2>&1 && [ "${OUT##*/}" = "buildroot-macweb" ]; then echo "repack.sh is running on $OUT: not touching it"; exit 3; fi
for f in "$OUT/images/bzImage" "$OUT/images/rootfs.cpio.zst"; do [ -r "$f" ] || { echo "missing $f"; exit 3; }; done
W="$(mktemp -d)"; [ -n "${KEEP:-}" ] && echo "work dir $W"; trap 'rm -rf "$W"' EXIT
mkdir "$W/tree"; cp -r "$FIX"/. "$W/tree/"; printf 'iyakki-persist-ok' > "$W/tree/marker"
IMG="$W/data.img"; truncate -s 96M "$IMG"
mke2fs -q -t ext4 -L IYAKKI-DATA -d "$W/tree" "$IMG"
boot() { # $1 = selftest stage, $2 = log
  timeout 180 $QEMU -accel kvm -accel tcg -m 1024 -smp 2 -nographic -no-reboot \
    -kernel "$OUT/images/bzImage" -initrd "$OUT/images/rootfs.cpio.zst" \
    -append "console=ttyS0 iyakki.selftest=$1 iyakki.disk=vda panic=-1" \
    -drive "file=$IMG,format=raw,if=virtio" >"$2" 2>&1 || true
  tr -d '\r' < "$2" | grep -a "IYAKKI-SELFTEST" || true
}
dump() { debugfs -R "cat /$1" "$IMG" 2>/dev/null; } # a file of the volume, read without mounting
fail() { echo "FAIL: $1 (log: $2)"; trap - EXIT; exit 1; }

echo "--- boot 1: the volume is found, the boot changes nothing"
boot persist2 "$W/l1"
grep -aq 'IYAKKI-SELFTEST persist2 location Disk' "$W/l1" || fail "the data volume was not found" "$W/l1"
grep -aq 'IYAKKI-SELFTEST persist2 marker "iyakki-persist-ok"' "$W/l1" || fail "marker lost" "$W/l1"
for f in settings.json inbox/amy.json dash/sources-amy.json k3s-dev/state.json audit.log; do
  dump "$f" | cmp -s - "$FIX/$f" || fail "$f changed during a plain boot" "$W/l1"
done
e2fsck -fn "$IMG" >/dev/null 2>&1 || fail "the volume does not pass e2fsck after the boot" "$W/l1"
echo "boot 1 ok"

echo "--- boot 2: the stores are opened by this release"
boot upgrade "$W/l2"
grep -aq 'IYAKKI-SELFTEST upgrade end' "$W/l2" || { echo "SKIP: this image has no 'upgrade' selftest stage (built before it was added); boots 2 and 3 not run (log: $W/l2)"; trap - EXIT; exit 2; }
grep -aq 'upgrade settings providers=2 chat=qwen3:8b tool_log_days=5 read_only=false' "$W/l2" || fail "settings were not read as the fixture has them" "$W/l2"
grep -aq 'upgrade sources 2' "$W/l2" || fail "dashboard sources lost" "$W/l2"
grep -aq 'upgrade inbox 5' "$W/l2" || fail "inbox notes lost" "$W/l2"
dump settings.json.pre-v1 | cmp -s - "$FIX/settings.json" || fail "settings.json.pre-v1 is not the old file" "$W/l2"
dump dash/sources-amy.json.pre-v1 | cmp -s - "$FIX/dash/sources-amy.json" || fail "the sources backup is not the old file" "$W/l2"
dump settings.json | grep -aq '"schema": 1' || fail "settings.json was not stamped" "$W/l2"
dump settings.json > "$W/settings.after"
echo "boot 2 ok"

echo "--- boot 3: again, nothing is rewritten"
boot upgrade "$W/l3"
grep -aq 'upgrade settings providers=2 chat=qwen3:8b' "$W/l3" || fail "settings lost on the second run" "$W/l3"
dump settings.json | cmp -s - "$W/settings.after" || fail "settings.json was rewritten by the second run" "$W/l3"
dump settings.json.pre-v1 | cmp -s - "$FIX/settings.json" || fail "the backup changed" "$W/l3"
e2fsck -fn "$IMG" >/dev/null 2>&1 || fail "the volume does not pass e2fsck" "$W/l3"
echo PASS
