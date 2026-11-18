#!/bin/sh
# F-BOOT-28: runs the boot speed kit (qemu-boot-speed.py) for real on an existing lean image and fails unless the kit saw the
# firmware hand over to GRUB, the system's first line, the data volume and the supervisor, in that order and each later than the one before.
# A control run on a file that is no disk must NOT produce the stamps (so a kit that prints stamps from nothing is caught).
#   sh dist/buildroot/test-boot-speed-kit.sh IMAGE.img        (QEMU, nothing is written to the image; about 30 s with KVM)
set -eu
img=${1:?usage: test-boot-speed-kit.sh IMAGE.img}
here=$(cd "$(dirname "$0")" && pwd)
out=$(python3 "$here/qemu-boot-speed.py" "$img" --mbps 0 --runs 1)
echo "$out"
line=$(echo "$out" | grep "unthrottled:") || { echo "FAIL: the kit printed no result line"; exit 1; }
echo "$line" | python3 -c '
import re, sys
l = sys.stdin.read()
v = {k: float(re.search(k + r"\s+([0-9.]+)", l).group(1)) for k in ("grub", "output", "data", "super")}
assert v["grub"] <= v["output"] <= v["data"] <= v["super"], v
assert v["super"] > v["grub"], v
print("stamps in order:", v)
'
junk=$(mktemp)
trap 'rm -f "$junk"' EXIT
head -c 1048576 /dev/zero > "$junk"
if python3 "$here/qemu-boot-speed.py" "$junk" --mbps 0 --runs 1 2>&1 | grep -Eq "unthrottled:.*super +[0-9]"; then
  echo "FAIL: a file that is no disk produced a supervisor stamp"; exit 1
fi
echo "PASS: boot speed kit stamps a real boot in order and none for a non-bootable file"
