#!/bin/sh
# Measure the size and boot-time budgets (F-PKG-05) from real build outputs on a Linux PC.
#
#   dist/release/measure-budgets.sh [images-dir] [image] [runs]
#
#   images-dir  the build's images folder (default: the newest target/buildroot-*/images)
#   image       which budget set: full (default) or libre
#   runs        boot-time runs for the median (default 3; 0 skips the boot)
#
# It checks the file sizes against dist/release/image-budgets.json and prints "suggested" ceilings from what it found (measured plus 10 percent;
# paste them into the budget file only after looking at them). With runs above 0 it also boots the image in QEMU to the sign-in picture
# (dist/buildroot/boot-time.py: needs qemu-system-x86_64 and /dev/kvm, run it with sudo or as a user that may use KVM) and checks the median
# against the login-time budget. Idle RAM at the sign-in screen is not measured by this script: read it from the System tile on the booted
# image and put it in a results file as {"memory": {"idle_ram_mib": N}} (pass it with RESULTS=file). Exit 0 within budget, 1 over, 2 usage.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
dir=${1:-$(ls -dt "$root"/target/buildroot-*/images 2>/dev/null | head -1)}
image=${2:-full}
runs=${3:-3}
[ -d "${dir:-}" ] || { echo "no images folder: pass one (the build writes target/buildroot-<variant>/images)" >&2; exit 2; }
res=${RESULTS:-}
tmp=""
if [ "$runs" -gt 0 ] 2>/dev/null; then
  if command -v qemu-system-x86_64 >/dev/null 2>&1 && { [ -w /dev/kvm ] || [ "$(id -u)" = 0 ]; }; then
    tmp=$(mktemp)
    BOOT_TIME_JSON=$tmp python3 "$root/dist/buildroot/boot-time.py" "$(dirname "$dir")" "$runs" || echo "the boot measurement failed; sizes are still checked" >&2
    [ -s "$tmp" ] && res=${res:-$tmp}
  else
    echo "no qemu-system-x86_64 or no access to /dev/kvm: the boot time is not measured" >&2
  fi
fi
status=0
if [ -n "$res" ]; then
  python3 "$here/check-budgets.py" --image "$image" --images-dir "$dir" --results "$res" --suggest || status=$?
else
  python3 "$here/check-budgets.py" --image "$image" --images-dir "$dir" --suggest || status=$?
fi
[ -n "$tmp" ] && rm -f "$tmp"
exit $status
