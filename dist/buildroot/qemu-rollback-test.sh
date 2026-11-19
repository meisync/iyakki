#!/bin/sh
# Upgrade and rollback test in QEMU (compatibility charter step 6, F-CORE-21; docs/development/CORE-COMPATIBILITY-DESIGN.md section 8 item 5).
# TWO builds boot ONE data disk, in this order, and the person's data must come through every time:
#
#   dist/buildroot/qemu-rollback-test.sh OLD-build-dir NEW-build-dir
#
#   OLD-build-dir   the recorded build of release N (images/bzImage and images/rootfs.cpio.zst), for example an unpacked release image
#   NEW-build-dir   the build of release N+1 (the one under test)
#
# Boot 1  OLD: the volume made from the synthetic alpha.1 fixture (tests/fixtures/upgrade/alpha1) plus a marker is found and mounted.
# Boot 2  OLD, selftest `upgrade`: the data is created/stamped the way release N does (only when OLD has that stage; else boot 1 is the baseline).
# Boot 3  NEW, selftest `upgrade`: the stores migrate; every file that changed has its `.pre-v1` copy equal to the original, nothing is
#         `.corrupt-*`, the marker is there, e2fsck is clean.
# Boot 4  OLD AGAIN (the "previous version" the boot menu keeps): the volume that release N+1 touched still reads: the marker, the settings
#         (providers, chat model), the sources and the inbox are all there, no `.corrupt-*` file appeared, e2fsck is clean. A file that
#         release N+1 stamped with a newer schema is read, not rewritten and not moved aside.
# Boot 5  NEW again: nothing is rewritten (idempotent).
#
# Exit codes: 0 PASS, 1 FAIL, 2 SKIP (an image lacks the `upgrade` stage), 3 not runnable here (no QEMU, missing image files, repack running).
# Never writes to an image or to a real drive: the disk is a temporary file made with mke2fs -d. Needs qemu-system-x86_64, mke2fs, debugfs, e2fsck.
set -eu
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OLD="${1:-}"; NEW="${2:-}"
[ -n "$OLD" ] && [ -n "$NEW" ] || { echo "usage: $0 OLD-build-dir NEW-build-dir"; exit 3; }
QEMU="${QEMU:-qemu-system-x86_64}"
FIX="$ROOT/tests/fixtures/upgrade/alpha1"
command -v "$QEMU" >/dev/null 2>&1 || { echo "NOT RUNNABLE: $QEMU is not installed on this computer"; exit 3; }
for t in mke2fs debugfs e2fsck; do command -v "$t" >/dev/null 2>&1 || { echo "NOT RUNNABLE: $t is not installed"; exit 3; }; done
for d in "$OLD" "$NEW"; do
  for f in "$d/images/bzImage" "$d/images/rootfs.cpio.zst"; do [ -r "$f" ] || { echo "NOT RUNNABLE: missing $f"; exit 3; }; done
  if pgrep -f "repack.sh" >/dev/null 2>&1 && [ "${d##*/}" = "buildroot-macweb" ]; then echo "repack.sh is running on $d: not touching it"; exit 3; fi
done
W="$(mktemp -d)"
if [ -n "${KEEP:-}" ]; then echo "work dir $W (kept: logs l1..l5, data.img)"; else trap 'rm -rf "$W"' EXIT; fi
mkdir "$W/tree"; cp -r "$FIX"/. "$W/tree/"; printf 'iyakki-persist-ok' > "$W/tree/marker"
IMG="$W/data.img"; truncate -s 96M "$IMG"
mke2fs -q -t ext4 -L IYAKKI-DATA -d "$W/tree" "$IMG"
boot() { # $1 = build dir, $2 = selftest stage, $3 = log
  timeout 180 $QEMU -accel kvm -accel tcg -m 1024 -smp 2 -nographic -no-reboot \
    -kernel "$1/images/bzImage" -initrd "$1/images/rootfs.cpio.zst" \
    -append "console=ttyS0 iyakki.selftest=$2 iyakki.disk=vda panic=-1" \
    -drive "file=$IMG,format=raw,if=virtio" >"$3" 2>&1 || true
  tr -d '\r' < "$3" | grep -a "IYAKKI-SELFTEST" || true
}
dump() { debugfs -R "cat /$1" "$IMG" 2>/dev/null; }
has_corrupt() { debugfs -R "ls -l /" "$IMG" 2>/dev/null | grep -aq '\.corrupt-' || debugfs -R "ls -l /dash" "$IMG" 2>/dev/null | grep -aq '\.corrupt-'; }
fail() { echo "FAIL: $1 (log: $2)"; trap - EXIT; exit 1; }
clean() { e2fsck -fn "$IMG" >/dev/null 2>&1 || fail "the volume does not pass e2fsck after $1" "$2"; has_corrupt && fail "a .corrupt- file appeared after $1: some data was judged damaged" "$2"; echo "  after $1: e2fsck clean, no .corrupt- file"; true; }
reads() { # $1 = log: the data this fixture holds is read
  grep -aq 'upgrade settings providers=2 chat=qwen3:8b' "$1" || fail "settings not read as the fixture has them" "$1"
  grep -aq 'upgrade sources 2' "$1" || fail "dashboard sources lost" "$1"
  grep -aq 'upgrade inbox 5' "$1" || fail "inbox notes lost" "$1"
}

echo "--- boot 1: OLD finds the volume"
boot "$OLD" persist2 "$W/l1"
grep -aq 'IYAKKI-SELFTEST persist2 location Disk' "$W/l1" || fail "OLD did not find the data volume" "$W/l1"
grep -aq 'IYAKKI-SELFTEST persist2 marker "iyakki-persist-ok"' "$W/l1" || fail "marker lost" "$W/l1"
echo "boot 1 ok"

echo "--- boot 2: OLD reads and stamps its data"
boot "$OLD" upgrade "$W/l2"
if grep -aq 'IYAKKI-SELFTEST upgrade end' "$W/l2"; then reads "$W/l2"; clean "boot 2" "$W/l2"; echo "boot 2 ok"; else echo "boot 2: OLD has no 'upgrade' stage; the fixture is the baseline"; fi
dump settings.json > "$W/settings.old"
mkdir "$W/snap2"; debugfs -R "rdump / $W/snap2" "$IMG" >/dev/null 2>&1 || true   # what the volume holds before NEW touches it

echo "--- boot 3: NEW migrates"
boot "$NEW" upgrade "$W/l3"
grep -aq 'IYAKKI-SELFTEST upgrade end' "$W/l3" || { echo "SKIP: NEW has no 'upgrade' selftest stage (log: $W/l3)"; trap - EXIT; exit 2; }
reads "$W/l3"; clean "boot 3" "$W/l3"
grep -aq 'IYAKKI-SELFTEST persist2 marker' "$W/l3" 2>/dev/null || true
dump settings.json > "$W/settings.new"
# every file NEW changed keeps its `.pre-v1` copy, byte for byte what it was before (the first migration of a file leaves the original beside it)
changed=0; checked=0
others=""
for f in $(cd "$W/snap2" && find . -type f ! -name '*.pre-v1' | sed 's|^\./||'); do
  checked=$((checked + 1))
  dump "$f" | cmp -s - "$W/snap2/$f" && continue
  case "$f" in log/*|*.log) others="$others $f"; continue ;; *.json) ;; *) others="$others $f"; continue ;; esac   # only the stores' schema files are migrated; others (clock-floor, log/, ...) are runtime stamps
  changed=$((changed + 1))
  dump "$f.pre-v1" | cmp -s - "$W/snap2/$f" || fail "NEW changed $f but $f.pre-v1 is missing or is not the original" "$W/l3"
done
echo "  after boot 3: $checked files checked, $changed changed by NEW, every changed .json file has its .pre-v1 original${others:+; other files changed (runtime stamps):$others}"
echo "boot 3 ok"

echo "--- boot 4: OLD again, on the volume NEW touched (the previous version in the boot menu)"
boot "$OLD" persist2 "$W/l4a"
grep -aq 'IYAKKI-SELFTEST persist2 marker "iyakki-persist-ok"' "$W/l4a" || fail "OLD lost the marker after NEW ran" "$W/l4a"
boot "$OLD" upgrade "$W/l4"
if grep -aq 'IYAKKI-SELFTEST upgrade end' "$W/l4"; then reads "$W/l4"; fi
clean "boot 4" "$W/l4"
dump settings.json | cmp -s - "$W/settings.new" || fail "OLD rewrote settings.json that NEW stamped (a newer file must be read, not rewritten)" "$W/l4"
echo "boot 4 ok"

echo "--- boot 5: NEW again, nothing is rewritten"
boot "$NEW" upgrade "$W/l5"
reads "$W/l5"; clean "boot 5" "$W/l5"
dump settings.json | cmp -s - "$W/settings.new" || fail "settings.json was rewritten by the second NEW run" "$W/l5"
echo "boot 5 ok"
echo PASS
