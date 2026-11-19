#!/usr/bin/env bash
# F-INST-28: checks of the aarch64 install template that need no boot and no genimage: the layout the configuration describes, the
# data marker the post-image hook writes (byte for byte the same as the x86 one), and a disk made with sgdisk to that layout whose
# table and marker are read back. Nothing is booted. Run: dist/buildroot/test-aarch64-install-layout.sh
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CFG="$HERE/board/iyakki-aarch64/genimage.cfg"
T="$(mktemp -d)"; trap 'rm -rf "${T:?}"' EXIT
PASS=0
ok() { PASS=$((PASS + 1)); echo "ok   $*"; }
bad() { echo "FAIL $*" >&2; exit 1; }

python3 - "$CFG" <<'PY' || bad "the genimage configuration does not describe the install template"
import re, sys
t = open(sys.argv[1]).read()
def block(kind, name):
    m = re.search(r'image\s+' + re.escape(name) + r'\s*\{', t)
    assert m, f"no image {name}"
    i = m.end(); d = 1
    while d:
        d += {'{': 1, '}': -1}.get(t[i], 0); i += 1
    return t[m.end():i - 1]
vfat = block('image', 'iyakki-boot-aarch64.vfat')
assert 'label = "IYAKKI-BOOT"' in vfat and '-F 32' in vfat, "the boot volume is FAT32 named IYAKKI-BOOT"
for f in ('EFI', 'Image', 'rootfs.cpio.gz'):
    assert re.search(r'file\s+' + re.escape(f) + r'\s*\{', vfat), f"the boot volume lacks {f}"
img = block('image', 'iyakki-install-aarch64.img')
assert 'partition-table-type = "gpt"' in img, "GPT"
parts = re.findall(r'partition\s+(\w+)\s*\{(.*?)\}', img, re.S)
assert [p[0] for p in parts] == ['boot', 'data'], "partition 1 boot, partition 2 data"
assert 'partition-type-uuid = U' in parts[0][1] and 'bootable = true' in parts[0][1] and 'iyakki-boot-aarch64.vfat' in parts[0][1]
assert 'partition-type-uuid = L' in parts[1][1] and 'data-marker.img' in parts[1][1]
# the existing live image is untouched
assert re.search(r'image\s+iyakki-aarch64\.img\s*\{', t) and re.search(r'image\s+efi-part\.vfat\s*\{', t)
PY
ok "genimage.cfg: GPT, FAT32 IYAKKI-BOOT (EFI, Image, rootfs) as partition 1, the marked data partition as partition 2; the live image is still there"

# the post-image hook's marker lines, run on their own
sed -n '/^# the install template.s data partition/,$p' "$HERE/board/iyakki-aarch64/post-image.sh" | grep -v '^#' > "$T/marker.sh"
BINARIES_DIR="$T" sh "$T/marker.sh"
printf 'IYAKKI-FORMAT-ME-v1\0' > "$T/want"
cmp -n 20 "$T/data-marker.img" "$T/want" && [ "$(stat -c %s "$T/data-marker.img")" = 1048576 ] || bad "the marker file is wrong"
BX="$T/x86"; mkdir -p "$BX"; sed -n '/^# partition 2 starts with our marker/,$p' "$HERE/board/iyakki/post-image.sh" | grep -v '^#' > "$T/x86marker.sh"
BINARIES_DIR="$BX" sh "$T/x86marker.sh"
cmp "$T/data-marker.img" "$BX/data-marker.img" && ok "the data marker is byte for byte the x86 template's (1 MiB, starts with the format marker)"

# a disk made to the same layout with the usual tools, read back
command -v sgdisk >/dev/null && command -v mkfs.vfat >/dev/null || { echo "skip the disk emulation: sgdisk and mkfs.vfat are not installed"; echo "all $PASS checks passed (disk emulation skipped)"; exit 0; }
D="$T/disk.img"; truncate -s 600M "$D"
sgdisk -o -n 1:2048:+256M -t 1:C12A7328-F81F-11D2-BA4B-00A0C93EC93B -A 1:set:2 -n 2:0:0 -t 2:0FC63DAF-8483-4772-8E79-3D69D8477DE4 "$D" >/dev/null
sgdisk -p "$D" | grep -q "EF00" && sgdisk -p "$D" | grep -q "8300" && ok "the table has an EFI System partition then a Linux data partition"
START2=$(sgdisk -i 2 "$D" | sed -n 's/^First sector: \([0-9]*\).*/\1/p')
dd if="$T/data-marker.img" of="$D" bs=512 seek="$START2" conv=notrunc 2>/dev/null
[ "$(dd if="$D" bs=512 skip="$START2" count=1 2>/dev/null | head -c 19)" = "IYAKKI-FORMAT-ME-v1" ] && ok "the marker sits at the start of partition 2"
echo "all $PASS checks passed"
