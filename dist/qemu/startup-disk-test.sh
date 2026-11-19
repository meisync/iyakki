#!/bin/sh
# The startup-disk commands (`iyakki startup-disk status | register | first | revert`, F-NET-17, F-INST-08's boot entry) on a REAL UEFI firmware variable store: OVMF's, through efivarfs.
#   dist/qemu/startup-disk-test.sh [build-output-dir]
# Needs: the built image (images/bzImage, rootfs.cpio.zst, iyakki-install-x86_64.img; the boot stage `startupdisk` is in images built after 2026-11-14, otherwise IYAKKI_BIN=path of a newer
# `iyakki` program (musl) is laid over the RAM disk as a second zstd cpio), qemu-system-x86_64 (KVM recommended), OVMF (/usr/share/OVMF), `iyakki-usb` (IYAKKI_USB, default target/debug/iyakki-usb),
# zstd, cpio. The guest is started through OVMF with the kernel handed over as an EFI stub (`-kernel`), so the kernel has real EFI runtime services and efivarfs; the firmware variables are a
# fresh copy of OVMF_VARS in a scratch folder. The data disk is an MBR install made by `iyakki-usb --target-file` (boot volume 1, data volume 2). One guest boot (the first start formats the data volume by itself): `iyakki.selftest=startupdisk` runs status, register, first, status, revert, status, each with its exit code.
# PASS = status reads the real BootOrder (exit 0); register makes IYAKKI's own entry (a Boot#### variable appears, the others untouched); first puts it first in BootOrder; revert restores the
# old order exactly and the status says so. Not covered: Apple's firmware and its own startup variables, a real motherboard, Secure Boot, a boot that follows the new entry.
set -eu
. "$(dirname "$0")/lib.sh"
check_images; need python3; need zstd; need cpio
OVMF="${OVMF:-/usr/share/OVMF/OVMF_CODE_4M.fd}"; VARS_SRC="${OVMF_VARS:-/usr/share/OVMF/OVMF_VARS_4M.fd}"
[ -f "$OVMF" ] && [ -f "$VARS_SRC" ] || { echo "needs OVMF ($OVMF)" >&2; exit 2; }
USB="${IYAKKI_USB:-$ROOT/target/debug/iyakki-usb}"
[ -x "$USB" ] && [ -f "$OUT/images/iyakki-install-x86_64.img" ] || { echo "needs iyakki-usb ($USB) and $OUT/images/iyakki-install-x86_64.img" >&2; exit 2; }
INITRD="$OUT/images/rootfs.cpio.zst"
if [ -n "${IYAKKI_BIN:-}" ]; then
  mkdir -p "$WORK/ov/usr/bin"; cp "$IYAKKI_BIN" "$WORK/ov/usr/bin/iyakki"
  ( cd "$WORK/ov" && find . | cpio -o -H newc 2>/dev/null | zstd -q -19 ) > "$WORK/overlay.zst"
  cat "$OUT/images/rootfs.cpio.zst" "$WORK/overlay.zst" > "$WORK/initrd.zst"; INITRD="$WORK/initrd.zst"
fi
DISK="$WORK/disk.img"; truncate -s 2G "$DISK"
"$USB" "$OUT/images/iyakki-install-x86_64.img" --target-file "$DISK" --erase testfile --table mbr >"$WORK/usb.log" 2>&1 || { cat "$WORK/usb.log"; fail "iyakki-usb could not write the test disk"; }
cp "$VARS_SRC" "$WORK/vars.fd"
fwboot() { # log args
  log="$1"; args="$2"
  timeout "${TIMEOUT:-240}" $QEMU -accel kvm -accel tcg -m 2048 -smp 2 -nographic -no-reboot -machine q35 \
    -drive "if=pflash,format=raw,readonly=on,file=$OVMF" -drive "if=pflash,format=raw,file=$WORK/vars.fd" \
    -kernel "$OUT/images/bzImage" -initrd "$INITRD" -append "console=ttyS0 panic=-1 $args" -drive "file=$DISK,format=raw,if=virtio" >"$log" 2>&1 || true
}
echo "=== the startup-disk commands against OVMF's variables"
fwboot "$WORK/s.log" "iyakki.selftest=startupdisk iyakki.disk=vda"
tr -d '\r' < "$WORK/s.log" | sed -n '/startupdisk step status/,/startupdisk end/p' | sed 's/^/  /' | cut -c1-220
want "$WORK/s.log" "startupdisk end" "the stage ran to its end"
python3 - "$WORK/s.log" <<'PY' || fail "the startup-disk steps did not behave as expected (see the lines above)"
import re, sys
t = open(sys.argv[1], errors="replace").read().replace("\r", "")
steps = re.findall(r"IYAKKI-SELFTEST startupdisk step (\w+)\n(.*?)IYAKKI-SELFTEST startupdisk rc \w+ (\d+)", t, re.S)
assert len(steps) == 6, "expected six steps, got %d" % len(steps)
def order(text):
    m = re.search(r"BootOrder: (.*)", text)
    return re.findall(r"(Boot[0-9A-F]{4}) \"", m.group(1)) if m else None
(s1, o1, r1), (rg, org, rr), (f, of, rf), (s2, o2, r2), (rv, orv, rrv), (s3, o3, r3) = steps
assert (r1, rr, rf, r2, rrv, r3) == ("0",) * 6, "exit codes %s" % [r1, rr, rf, r2, rrv, r3]
before, after_first, after_revert = order(o1), order(o2), order(o3)
assert before and len(before) >= 2, "no BootOrder read from the firmware: %s" % o1
assert "Boot" in org and "IYAKKI" in org, "register: %s" % org
own = re.search(r"IYAKKI's own entry: (Boot[0-9A-F]{4})", o2).group(1)
assert after_first[0] == own, "the own entry is first after `first`: %s" % after_first
assert sorted(after_first) == sorted(before), "no entry was added or lost: %s vs %s" % (before, after_first)
assert [x for x in after_first if x != own] == [x for x in before if x != own], "the others keep their order"
assert after_revert == before, "revert puts the old order back exactly: %s vs %s" % (after_revert, before)
assert "earlier boot order is back" in orv
print("ok: BootOrder read from OVMF %s; register made/replaced %s; first put it first %s; revert restored %s" % (before, own, after_first, after_revert))
PY
echo "PASS (startup-disk)"
