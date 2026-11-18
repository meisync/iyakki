#!/usr/bin/env bash
# QEMU test of a layered root whose newer layers are on the DATA volume (what an applied v2 update leaves), docs/development/LAYERED-ROOT-DESIGN.md.
#
#   layers-data-qemu-test.sh OLD_LEAN_DIR NEW_LEAN_DIR
#
#   OLD_LEAN_DIR / NEW_LEAN_DIR  two outputs of `lean-image.sh ... --layers` (iyakki-lean-x86_64.img in OLD, layers/ in both).
# Makes a private copy of OLD's disk image whose data volume holds NEW's changed layers and iyakki/layers/current.json (unchanged layers stay
# on the boot volume, as an update leaves them) and previous.json (OLD's set), boots it through the real chain (qemu-lean-test.py) and checks
# the first stage's lines. Second run: the newest layer file is cut short (a half-written update), the first stage must skip that set, say so, and
# start the previous one. Nothing real is written; the copies live in a temp folder that is removed.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
OLD="${1:?OLD_LEAN_DIR}"; NEW="${2:?NEW_LEAN_DIR}"
for f in "$OLD/iyakki-lean-x86_64.img" "$OLD/layers/layers.json" "$NEW/layers/layers.json"; do [ -s "$f" ] || { echo "layers-data-qemu-test: no $f" >&2; exit 2; }; done
W="$(mktemp -d "${TMPDIR:-$HOME/.cache}/layers-data-test.XXXXXX")"; trap 'rm -rf "$W"' EXIT
MKE2FS="$(command -v mke2fs || echo /usr/sbin/mke2fs)"
python3 -I - "$OLD" "$NEW" "$W" <<'PY'
import json, os, shutil, sys
old, new, w = sys.argv[1:4]
o = json.load(open(f"{old}/layers/layers.json")); n = json.load(open(f"{new}/layers/layers.json"))
oh = {l["name"]: l["sha256"] for l in o["layers"]}
os.makedirs(f"{w}/data/iyakki/layers")
for l in n["layers"]:
    if oh.get(l["name"]) != l["sha256"]:
        fn = os.path.basename(l["path"])
        shutil.copy(f"{new}/layers/{fn}", f"{w}/data/iyakki/layers/{fn}")
        l["volume"] = "data"; l["path"] = f"iyakki/layers/{fn}"
    else:
        # unchanged: stays where the old set has it (the boot volume)
        ol = next(x for x in o["layers"] if x["name"] == l["name"]); l["volume"] = ol["volume"]; l["path"] = ol["path"]
json.dump(n, open(f"{w}/data/iyakki/layers/current.json", "w"), indent=2)
json.dump(o, open(f"{w}/data/iyakki/layers/previous.json", "w"), indent=2)
PY
[ -s "$W/data/iyakki/layers/current.json" ] || { echo "layers-data-qemu-test: no current.json" >&2; exit 1; }
KB=$(du -sk "$W/data" | cut -f1); MB=$(( KB / 1024 + 40 ))
"$MKE2FS" -q -t ext4 -L IYAKKI-DATA -m 0 -d "$W/data" "$W/data.ext4" "${MB}M"
mkdisk() {  # mkdisk OUT : the old image with the data partition replaced by data.ext4
  cp "$OLD/iyakki-lean-x86_64.img" "$1"
  local start; start=$(sfdisk -d "$1" | awk '/start=/ {gsub(",","",$4); s=$4} END {print s}')
  local dbytes; dbytes=$(stat -c %s "$W/data.ext4")
  truncate -s $(( start * 512 + dbytes + 1048576 )) "$1"
  dd if="$W/data.ext4" of="$1" bs=1M seek=$(( start * 512 / 1048576 )) conv=notrunc 2>/dev/null
  local fstart fsize; fstart=$(sfdisk -d "$1" | awk '/start=/ {gsub(",","",$4); print $4; exit}'); fsize=$(sfdisk -d "$1" | awk '/start=/ {gsub(",","",$6); print $6; exit}')
  sfdisk -q "$1" >/dev/null <<EOT
label: dos
label-id: 0x41465300
unit: sectors
start=$fstart, size=$fsize, type=ef, bootable
start=$start, size=$(( dbytes / 512 )), type=83
EOT
}
mkdisk "$W/disk1.img"
echo "--- boot 1: the newer layers on the data volume ---"
python3 "$HERE/qemu-lean-test.py" "$W/disk1.img" --entry 1 --serial-only --wait 70 --layers > "$W/t1.log" 2>&1 && rc1=0 || rc1=$?
grep -E "stage1: (data volume|root layers|root:|switching|skipping)|boot test" "$W/t1.log" || true
grep -q "(current layers)" "$W/t1.log" || { cat "$W/t1.log" >&2; echo "layers-data-qemu-test: FAIL: the current set was not the one that started" >&2; rc1=1; }
# boot 2: cut the first data layer short (an unfinished update)
python3 -I - "$W" <<'PY'
import glob, os, sys
f = sorted(glob.glob(sys.argv[1] + "/data/iyakki/layers/*.sfs"))[0]
print("cutting", os.path.basename(f))
open(f, "r+b").truncate(os.path.getsize(f) - 4096)
PY
KB=$(du -sk "$W/data" | cut -f1); "$MKE2FS" -q -t ext4 -L IYAKKI-DATA -m 0 -d "$W/data" "$W/data2.ext4" "${MB}M"; cp "$W/data2.ext4" "$W/data.ext4"
mkdisk "$W/disk2.img"
echo "--- boot 2: the newest set is incomplete, the previous set must start ---"
python3 "$HERE/qemu-lean-test.py" "$W/disk2.img" --entry 1 --serial-only --wait 70 --layers > "$W/t2.log" 2>&1 && rc2=0 || rc2=$?
grep -E "stage1: (root layers|root:|switching|skipping)|boot test" "$W/t2.log" || true
grep -q "skipping current layers" "$W/t2.log" && grep -q "(previous layers)" "$W/t2.log" || { cat "$W/t2.log" >&2; echo "layers-data-qemu-test: FAIL: the incomplete set was not skipped in favour of the previous set" >&2; rc2=1; }
[ "$rc1" = 0 ] && [ "$rc2" = 0 ] && echo "layers-data-qemu-test: PASS" || { echo "layers-data-qemu-test: FAILED" >&2; exit 1; }
