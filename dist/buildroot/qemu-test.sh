#!/bin/sh
# Boot an IYAKKI build in QEMU with the self-test flag and check the result.
#   ./qemu-test.sh [build-output-dir]     (QEMU may need root if io_uring/memlock limits bite: set QEMU="sudo qemu-system-x86_64")
set -eu
OUT="${1:-$(cd "$(dirname "$0")/../.." && pwd)/target/buildroot-libre}"
QEMU="${QEMU:-qemu-system-x86_64}"
LOG="$(mktemp)"
timeout 120 $QEMU -accel kvm -accel tcg -m 1024 -smp 2 -nographic -no-reboot \
  -kernel "$OUT/images/bzImage" -initrd "$OUT/images/rootfs.cpio.zst" \
  -append "console=ttyS0 iyakki.selftest=1 panic=-1" \
  -netdev user,id=n0 -device virtio-net-pci,netdev=n0 \
  -netdev user,id=n1 -device e1000e,netdev=n1 \
  -netdev user,id=n2 -device rtl8139,netdev=n2 \
  -audiodev none,id=snd0 -device intel-hda -device hda-duplex,audiodev=snd0 >"$LOG" 2>&1 || true
tr -d '\r' < "$LOG" | grep -a "IYAKKI-SELFTEST\|IYAKKI-HWREPORT" || true
for want in "IYAKKI-HWREPORT   architecture: x86_64" "IYAKKI-HWREPORT   \[keyboard\]" "IYAKKI-SELFTEST tool arecord ok" "IYAKKI-SELFTEST tool aplay ok" "IYAKKI-SELFTEST tool ssh ok" "IYAKKI-SELFTEST tool scp ok" "IYAKKI-SELFTEST tool dropbearkey ok" "IYAKKI-SELFTEST ssh Dropbear v" "IYAKKI-SELFTEST audio-capture card" "IYAKKI-SELFTEST begin" "IYAKKI-SELFTEST wasm guest-ok" "IYAKKI-SELFTEST end" "iface eth2 Ethernet" "addr eth2 .* 10\\.0\\.2\\."; do
  tr -d '\r' < "$LOG" | grep -aq "$want" || { echo "FAIL: missing '$want' (log: $LOG)"; exit 1; }
done
# N1: the smallest call of every runtime path that used external programs, against a loopback listener the self-test starts itself
# (crates/iyakki-agent/src/pathcheck.rs). A missing program (git, ip, ...) or CA bundle shows up here as a FAILED line.
tr -d '\r' < "$LOG" | grep -a "IYAKKI-SELFTEST path .* FAILED" && { echo "FAIL: a runtime path failed on this image (log: $LOG)"; exit 1; }
for want in "IYAKKI-SELFTEST path ca-bundle ok" "IYAKKI-SELFTEST path programs ok" "IYAKKI-SELFTEST path platform-connect ok" "IYAKKI-SELFTEST path registry-probe ok" "IYAKKI-SELFTEST path registry-setup ok" "IYAKKI-SELFTEST path cluster-tools ok" "IYAKKI-SELFTEST path git-client ok" "IYAKKI-SELFTEST path doctor-probes ok" "IYAKKI-SELFTEST path mp3-decoder ok" "IYAKKI-SELFTEST path web-recorder ok"; do
  tr -d '\r' < "$LOG" | grep -aq "$want" || { echo "FAIL: missing '$want' (log: $LOG)"; exit 1; }
done
echo "PASS"
