#!/bin/sh
# Hardware-free test of the installer package generator and validator, with fake inputs. Run:  dist/asahi/test-installer-package.sh
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
T="$(mktemp -d "${TMPDIR:-/tmp}/iyakki-asahi-test.XXXXXX")"; trap 'rm -rf "$T"' EXIT
P="python3 -I $HERE/asahi_package.py"
mkdir -p "$T/esp/m1n1" "$T/esp/EFI/BOOT"
echo fake > "$T/esp/m1n1/boot.bin"; echo fake > "$T/esp/EFI/BOOT/BOOTAA64.EFI"; echo fake > "$T/esp/Image"
head -c 8192 /dev/zero > "$T/data.img"
sh "$HERE/make-installer-package.sh" "$T/esp" "$T/data.img" "$T/out" 1.2.3 >/dev/null
$P validate --zip "$T/out/os/iyakki-apple-1.2.3.zip" --entry "$T/out/installer_data.json" >/dev/null || { echo "FAIL good package rejected"; exit 1; }
python3 -I - "$T/out/installer_data.json" <<'PY'
import json,sys
e=json.load(open(sys.argv[1]))["os_list"][0]
assert e["package"]=="iyakki-apple-1.2.3.zip" and e["next_object"]=="m1n1/boot.bin"
assert [p["type"] for p in e["partitions"]]==["EFI","Linux"] and e["partitions"][1]["expand"]
PY
bad() { # name, then a python edit of the entry `e`
  python3 -I - "$T/out/installer_data.json" "$T/bad.json" "$2" <<'PY'
import json,sys
d=json.load(open(sys.argv[1])); e=d["os_list"][0]; exec(sys.argv[3]); json.dump(d,open(sys.argv[2],"w"))
PY
  if $P validate --zip "$T/out/os/iyakki-apple-1.2.3.zip" --entry "$T/bad.json" >/dev/null; then echo "FAIL accepted: $1"; exit 1; fi
}
bad "wrong boot_object" 'e["boot_object"]="x.bin"'
bad "no expand" 'e["partitions"][1]["expand"]=False'
bad "missing image" 'e["partitions"][1]["image"]="nope.img"'
bad "bad size" 'e["partitions"][0]["size"]="500MB"'
bad "no macOS version" 'e["supported_fw"]=[]'
bad "unpinned" 'e["name"]="UNPINNED"'
# a package without the m1n1 payload is refused at make time
rm "$T/esp/m1n1/boot.bin"
if sh "$HERE/make-installer-package.sh" "$T/esp" "$T/data.img" "$T/out2" >/dev/null 2>&1; then echo "FAIL: ESP without m1n1 accepted"; exit 1; fi
# Apple firmware inside the package is refused
echo fake > "$T/esp/m1n1/boot.bin"; mkdir "$T/esp/vendorfw"; echo x > "$T/esp/vendorfw/firmware.cpio"
if sh "$HERE/make-installer-package.sh" "$T/esp" "$T/data.img" "$T/out3" >/dev/null 2>&1; then echo "FAIL: firmware in package accepted"; exit 1; fi
# an image that is not a multiple of 4 KiB is refused
head -c 5000 /dev/zero > "$T/odd.img"; rm -rf "$T/esp/vendorfw"
if sh "$HERE/make-installer-package.sh" "$T/esp" "$T/odd.img" "$T/out4" >/dev/null 2>&1; then echo "FAIL: odd image accepted"; exit 1; fi
echo "OK: installer package generator and validator"
