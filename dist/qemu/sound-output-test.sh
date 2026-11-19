#!/bin/sh
# The sound output list and the automatic choice (F-SND-02) on a REAL kernel's /proc/asound: emulated sound cards in a QEMU guest.
#   dist/qemu/sound-output-test.sh [build-output-dir]
# Needs: the built image (images/bzImage, rootfs.cpio.zst; the boot stage `sound` is in images built after 2026-11-14, otherwise IYAKKI_BIN=path of a newer `iyakki` program (musl) is laid over
# the RAM disk as a second zstd cpio), qemu-system-x86_64 (KVM recommended), zstd, cpio. Two guest boots, no sound is heard (the host's audio is `none`):
#   1. an Intel HDA controller with one duplex codec: the kernel's `snd-hda-intel` makes a card; `scan` (with the real `amixer` jack controls) must list a playback output of the kind
#      speakers/headphones with a `plughw:N,M` device, and the automatic choice must pick it.
#   2. the same plus a USB audio device (`usb-audio`): `scan` must list the USB output too (kind USB), and the automatic choice must still prefer the analog output (the documented order).
# The stage waits for the card, prints `/proc/asound/cards`, every output the selector found and the choice with its reason. Not covered: HDMI outputs with monitor names (needs the ELD of a real
# GPU), the jack state of a real laptop, `amixer` volume controls, a Mac's cards, hearing anything.
set -eu
. "$(dirname "$0")/lib.sh"
check_images
INITRD="$OUT/images/rootfs.cpio.zst"
if [ -n "${IYAKKI_BIN:-}" ]; then
  need zstd; need cpio
  mkdir -p "$WORK/ov/usr/bin"; cp "$IYAKKI_BIN" "$WORK/ov/usr/bin/iyakki"
  ( cd "$WORK/ov" && find . | cpio -o -H newc 2>/dev/null | zstd -q -19 ) > "$WORK/overlay.zst"
  cat "$OUT/images/rootfs.cpio.zst" "$WORK/overlay.zst" > "$WORK/initrd.zst"; INITRD="$WORK/initrd.zst"
fi
run() { # log extra-qemu-args...
  log="$1"; shift
  timeout "${TIMEOUT:-240}" $QEMU -accel kvm -accel tcg -m 2048 -smp 2 -nographic -no-reboot -audiodev none,id=snd0 \
    -kernel "$OUT/images/bzImage" -initrd "$INITRD" -append "console=ttyS0 panic=-1 iyakki.selftest=sound" "$@" >"$log" 2>&1 || true
}
show() { tr -d '\r' < "$1" | grep -a "IYAKKI-SELFTEST sound" | sed 's/^/  /' | cut -c1-240; }
echo "=== 1. one HDA card"
run "$WORK/hda.log" -device intel-hda,id=hda0 -device hda-duplex,audiodev=snd0
show "$WORK/hda.log"
want "$WORK/hda.log" "sound end" "the stage ran to its end"
want "$WORK/hda.log" "IYAKKI-SELFTEST sound card .*HDA" "the kernel found the HDA card"
want "$WORK/hda.log" "sound output key=.*kind=speakers/headphones alsa=plughw:0," "the selector lists a speakers/headphones output with its ALSA device"
want "$WORK/hda.log" "sound chosen Some(" "and picks one by itself"
echo "=== 2. the HDA card and a USB audio device"
run "$WORK/usb.log" -device intel-hda,id=hda0 -device hda-duplex,audiodev=snd0 -device qemu-xhci,id=xhci -device usb-audio,audiodev=snd0,bus=xhci.0
show "$WORK/usb.log"
want "$WORK/usb.log" "sound end" "the stage ran to its end"
want "$WORK/usb.log" "sound output key=.*kind=USB " "the USB audio output is listed"
want "$WORK/usb.log" "sound output key=.*kind=speakers/headphones" "and the analog one still is"
python3 - "$WORK/usb.log" <<'PY' || fail "the automatic choice did not prefer the analog output"
import re, sys
t = open(sys.argv[1], errors="replace").read().replace("\r", "")
chosen = re.search(r"sound chosen Some\(\"([^\"]+)\"\)", t)
assert chosen, "nothing chosen"
kind = re.search(r"sound output key=" + re.escape(chosen.group(1)) + r" kind=(\S+)", t)
assert kind and kind.group(1) == "speakers/headphones", "chose %s of kind %s" % (chosen.group(1), kind and kind.group(1))
print("ok: with an analog and a USB output the automatic choice is", chosen.group(1), "(speakers/headphones)")
PY
echo "PASS (sound-output)"
