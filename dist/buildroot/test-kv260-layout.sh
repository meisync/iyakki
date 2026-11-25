#!/usr/bin/env bash
# F-KV260-01..03: checks of the KV260 SD-card image recipe that need no kernel build and no board: the kernel fragment, the pinned
# vendor tree, the extlinux menu, the genimage layout, the SD writer's refusals and its write + read-back + stretch on an ordinary file
# made to the same layout. Nothing is booted (dist/buildroot/qemu-test-kv260.sh boots the userspace; the board itself is a Manual step).
# Run: dist/buildroot/test-kv260-layout.sh
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
B="$HERE/board/iyakki-kv260"
T="$(mktemp -d)"; trap 'rm -rf "${T:?}"' EXIT
PASS=0
ok() { PASS=$((PASS + 1)); echo "ok   $*"; }
bad() { echo "FAIL $*" >&2; exit 1; }

python3 - "$B" "$HERE/configs/iyakki_kv260.fragment" "$ROOT/dist/kv260/PINS" <<'PY' || bad "the kernel recipe is not what the plan says"
import re, sys
b, frag, pins = sys.argv[1:4]
f = open(b + "/linux.fragment").read()
on = set(re.findall(r'^CONFIG_(\w+)=[ym]$', f, re.M))
need = ["ARCH_ZYNQMP", "ZYNQMP_FIRMWARE", "ZYNQMP_PM_DOMAINS", "COMMON_CLK_ZYNQMP", "RESET_ZYNQMP", "SERIAL_XILINX_PS_UART", "SERIAL_XILINX_PS_UART_CONSOLE",
        "MMC_SDHCI_OF_ARASAN", "MACB", "USB_DWC3", "USB_DWC3_XILINX", "USB_XHCI_HCD", "DRM_ZYNQMP_DPSUB", "XILINX_ZYNQMP_DPDMA", "PHY_XILINX_ZYNQMP"]
missing = [n for n in need if n not in on]
assert not missing, "missing " + ", ".join(missing)
off = set(re.findall(r'^# CONFIG_(\w+) is not set$', f, re.M))
for n in ("MTD_SPI_NOR", "SPI_ZYNQMP_GQSPI", "MTD_BLOCK"):
    assert n in off and n not in on, n + " must be off: IYAKKI never writes the boot flash"
d = open(frag).read()
pin = dict(re.findall(r'^(\w+)=(\S+)$', open(pins).read(), re.M))
commit = pin["LINUX_XLNX_COMMIT"]
assert re.fullmatch(r'[0-9a-f]{40}', commit), "the kernel commit is pinned in full"
assert ('BR2_LINUX_KERNEL_CUSTOM_REPO_VERSION="%s"' % commit) in d, "the defconfig fragment and PINS name the same commit"
assert 'BR2_LINUX_KERNEL_DEFCONFIG="xilinx"' in d and "github.com/Xilinx/linux-xlnx" in d
assert "LINUX_FIRMWARE" not in d and "BR2_TARGET_UBOOT" not in d and "BR2_TARGET_ARM_TRUSTED" not in d, "no vendor blob and no boot loader is built into the image"
assert "BR2_TARGET_GRUB2=n" in d
dts = re.search(r'INTREE_DTS_NAME="([^"]*)"', d).group(1).split()
assert dts == ["xilinx/zynqmp-smk-k26-revA-sck-kv-g-revA", "xilinx/zynqmp-smk-k26-revA-sck-kv-g-revB"], dts
PY
ok "kernel recipe: ZynqMP drivers on, the QSPI flash drivers off, the vendor tree pinned by full commit (same in PINS), no blob, no GRUB, the two KV260 device trees"

python3 - "$B/extlinux.conf" "$B/genimage.cfg" <<'PY' || bad "extlinux.conf or genimage.cfg is wrong"
import re, sys
e = open(sys.argv[1]).read()
labels = re.split(r'^label ', e, flags=re.M)[1:]
assert re.search(r'^default (\S+)', e, re.M).group(1) == "iyakki-dtb" and len(labels) >= 4
dtbs = []
for l in labels:
    assert re.search(r'^\s+linux /Image$', l, re.M) and re.search(r'^\s+initrd /rootfs\.cpio\.gz$', l, re.M), l.split()[0]
    app = re.search(r'^\s+append (.*)$', l, re.M).group(1)
    cons = re.findall(r'console=(\S+)', app)
    assert cons[-1].startswith("ttyPS1,115200"), "the serial port (PS UART1) is the last console= word: " + l.split()[0]
    assert "root=" not in app, "initramfs boot: no root= word"
    m = re.search(r'^\s+fdt (\S+)$', l, re.M)
    if m: dtbs.append(m.group(1))
# KI-1031: the default is the working display tree (carrier rev B, from the SD card); the board's own tree stays as a separate, named entry
by_name = {l.split()[0]: l for l in labels}
dflt = re.search(r'^default (\S+)', e, re.M).group(1)
assert re.search(r'^\s+fdt /dtb/zynqmp-smk-k26-revA-sck-kv-g-revB\.dtb$', by_name[dflt], re.M), "the default entry names the rev B device tree"
assert "fdt " not in by_name["iyakki"] and "board's own device tree" in by_name["iyakki"], "the board's own tree is the separate entry 'iyakki', clearly named"
assert dtbs and all(d.startswith("/dtb/zynqmp-smk-k26-revA-sck-kv-g-rev") and d.endswith(".dtb") for d in dtbs)
g = open(sys.argv[2]).read()
assert 'partition-table-type = "mbr"' in g and 'label = "IYAKKI-BOOT"' in g and "-F 32" in g
assert re.search(r'partition-type = 0xC\s+offset = 1M\s+bootable = true', g), "partition 1: FAT32 LBA, bootable (U-Boot scans bootable partitions)"
assert "partition-type = 0x83" in g and "data-marker.img" in g
for need in ("extlinux/extlinux.conf", "Image", "rootfs.cpio.gz", "kv260-dtb", "board-kv260"):
    assert need in g, need
assert "EFI" not in g and "grub" not in g.lower(), "no GRUB and no EFI on this card"
PY
ok "extlinux.conf: default is the rev B entry, the board's own tree is a named entry, ttyPS1 console last; genimage.cfg: MBR, bootable FAT32 IYAKKI-BOOT with extlinux/extlinux.conf, marked data partition, no GRUB"

# the post-image hook on a scratch directory
BIN="$T/bin"; mkdir -p "$BIN"; touch "$BIN/zynqmp-smk-k26-revA-sck-kv-g-revA.dtb" "$BIN/zynqmp-smk-k26-revA-sck-kv-g-revB.dtb" "$BIN/zynqmp-sm-k26-revA.dtb"
BINARIES_DIR="$BIN" sh "$B/post-image.sh"
[ "$(cat "$BIN/board-kv260")" = kv260 ] && [ -f "$BIN/extlinux.conf" ] && [ "$(ls "$BIN/kv260-dtb" | wc -l)" = 2 ] && cmp -n 20 "$BIN/data-marker.img" <(printf 'IYAKKI-FORMAT-ME-v1\0') && [ "$(stat -c %s "$BIN/data-marker.img")" = 1048576 ] || bad "post-image.sh staged the wrong files"
ok "post-image.sh stages extlinux.conf, the board file, only the two KV260 device trees and the 1 MiB data marker"
# KI-1032: the template's data partition is a fresh empty volume: the marker and nothing else (no files, no filesystem, no test state)
python3 - "$BIN/data-marker.img" <<'PY' || bad "the template's data partition holds more than the marker (test state would reach every card)"
import sys
d = open(sys.argv[1], "rb").read()
assert len(d) == 1048576, "1 MiB"
assert d.startswith(b"IYAKKI-FORMAT-ME-v1\0"), "the marker"
assert d[20:] == bytes(len(d) - 20), "everything after the marker is zero: no files, no filesystem"
PY
ok "the template's data partition is the empty marker volume: nothing but the marker and zeros (no files, so no lost+found either)"
# a second run over a marker file that holds test data must not keep that data
printf 'TLS-KEY-FROM-TEST' > "$BIN/data-marker.img"; BINARIES_DIR="$BIN" sh "$B/post-image.sh"
cmp -s <(tr -d '\0' < "$BIN/data-marker.img") <(printf 'IYAKKI-FORMAT-ME-v1') && [ "$(stat -c %s "$BIN/data-marker.img")" = 1048576 ] || bad "post-image.sh kept old data in the data partition"
ok "a rebuild over an old data marker replaces it: no state from an earlier run survives"

command -v sfdisk >/dev/null && command -v mkfs.vfat >/dev/null && command -v mcopy >/dev/null || { echo "skip the disk emulation: sfdisk, mkfs.vfat and mtools are not installed"; echo "all $PASS checks passed (disk emulation skipped)"; exit 0; }
# an image to the same layout, made with the usual tools
IMG="$T/iyakki-kv260.img"; truncate -s 300M "$IMG"
sfdisk -q "$IMG" >/dev/null <<SF
label: dos
start=2048, size=262144, type=c, bootable
start=264192, type=83
SF
OFF=$((2048 * 512)); BOOT="$T/boot.part"
mkfs.vfat -F 32 -n IYAKKI-BOOT --offset 2048 "$IMG" >/dev/null
mmd -i "$IMG@@$OFF" ::extlinux
mcopy -i "$IMG@@$OFF" "$B/extlinux.conf" ::extlinux/extlinux.conf
printf 'k' | mcopy -i "$IMG@@$OFF" - ::Image
mtype -i "$IMG@@$OFF" ::extlinux/extlinux.conf | grep -q '^default iyakki' && ok "the boot partition reads back with extlinux/extlinux.conf at the place the board's U-Boot looks"
printf 'IYAKKI-FORMAT-ME-v1\0' | dd of="$IMG" bs=512 seek=264192 conv=notrunc 2>/dev/null

W="$HERE/kv260-write-sd.sh"
# refusals, none of which needs a card
for d in /dev/loop0 /dev/nvme0n1 /dev/mtd0 /dev/mtdblock0 /dev/sda1 /dev/mmcblk0p1 /dev/dm-0 /dev/zram0; do
  if out="$(sh "$W" "$IMG" "$d" 2>&1)"; then bad "$d was accepted"; fi
  echo "$out" | grep -q "kv260-write-sd:" || bad "$d: no reason given: $out"
done
ok "the writer refuses loop, nvme, mtd, mtdblock, dm, zram devices and partitions (sda1, mmcblk0p1) before touching anything"
printf 'not an image' > "$T/junk.img"; truncate -s 300M "$T/junk.img"
: > "$T/card"; truncate -s 400M "$T/card"
wr() { KV260_WRITE_TEST=1 KV260_CONFIRM="$1" sh "$W" "$2" "$3" 2>&1 || true; }
case "$(wr card "$T/junk.img" "$T/card")" in *"no MBR signature"*) ok "an image without our partition table is refused" ;; *) bad "junk image accepted" ;; esac
truncate -s 100M "$T/small"
case "$(wr small "$IMG" "$T/small")" in *"smaller than the image"*) ok "a card smaller than the image is refused" ;; *) bad "small card accepted" ;; esac
case "$(wr wrong "$IMG" "$T/card")" in *"not confirmed"*) cmp -s -n 1000000 /dev/zero "$T/card" && ok "a wrong confirmation writes nothing" || bad "wrong confirmation wrote" ;; *) bad "wrong confirmation accepted" ;; esac
KV260_WRITE_TEST=1 KV260_CONFIRM=card sh "$W" "$IMG" "$T/card" | tee "$T/w.log" >/dev/null
grep -q "read-back: the first" "$T/w.log" && cmp -i 512 -n "$(( $(stat -c %s "$IMG") - 512 ))" "$IMG" "$T/card" && ok "write + read-back: the card holds the image byte for byte (the partition table differs only by the stretched data partition)"
grep -q "stretched" "$T/w.log" || bad "the writer did not report stretching"
{
  END="$(sfdisk -d "$T/card" | sed -n 's/.*start= *264192, size= *\([0-9]*\),.*/\1/p')"
  [ "$((264192 + END))" -ge $(( 400 * 2048 - 2048 )) ] && ok "the data partition now reaches the end of the card" || bad "the data partition was not stretched ($END sectors)"
}
# the board-table generator of the loader repository (iyakki-boot/crates/board-cfg) holds the golden copy of this file and its own test
# proves it writes those bytes: the recipe's file and the generator's golden file must stay the same bytes until the build switches over
G="$ROOT/iyakki-boot/crates/board-cfg/tests/golden/baseline-kv260/extlinux.conf"
if [ -f "$G" ]; then cmp -s "$B/extlinux.conf" "$G" && ok "extlinux.conf is the board-table generator's golden file, byte for byte" || bad "extlinux.conf differs from the generator's golden file (iyakki-boot/crates/board-cfg)"; fi
echo "all $PASS checks passed"
