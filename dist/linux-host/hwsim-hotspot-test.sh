#!/bin/sh
# The Wi-Fi hotspot against a REAL wpa_supplicant and the real mac80211 on virtual radios (F-HOTSPOT-03/04/08/09/16/17), inside a QEMU guest.
#   dist/linux-host/hwsim-hotspot-test.sh [WORK-DIR]       (default WORK-DIR: target/hwsim-work in the repository)
# What it does: builds a small test kernel with cfg80211, mac80211 and mac80211_hwsim built in (about 3 minutes, once; from a Linux 6.12 source tree: IYAKKI_LINUX_SRC=DIR, or the
# tarball IYAKKI_LINUX_TARBALL=FILE, or it is downloaded from kernel.org over HTTPS), then runs crates/iyakki-hotspot/tools/hwsim-test.sh: two QEMU boots of a microVM with no network
# device in which our access point (real nl80211 and AF_PACKET code of iyakki-agent) runs on virtual radio 0 and the host's wpa_supplicant joins it from radio 1 with the right and
# with a wrong passphrase. 15 checks: association, the 4-way handshake and key installation, COMPLETED, DHCP from our server, a ping over CCMP, the wrong passphrase refused.
# No module is loaded on this computer; everything is written under the work folder. Needs: qemu-system-x86_64 with /dev/kvm, wpa_supplicant, iw, busybox, zstd, cpio, flock, cargo, make, gcc.
# Exit 0 = every check passed; 1 = a check failed; 2 = something needed is missing.
set -eu
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WORK="${1:-$ROOT/target/hwsim-work}"; mkdir -p "$WORK"; WORK="$(cd "$WORK" && pwd)"
for t in qemu-system-x86_64 wpa_supplicant iw busybox zstd cpio flock cargo make gcc; do command -v "$t" >/dev/null 2>&1 || [ -x "/usr/sbin/$t" ] || { echo "hwsim-hotspot: needs $t" >&2; exit 2; }; done
[ -w /dev/kvm ] || { echo "hwsim-hotspot: needs write access to /dev/kvm" >&2; exit 2; }
KVER=6.12.112
if [ ! -f "$WORK/kbuild/arch/x86/boot/bzImage" ]; then
  SRC="${IYAKKI_LINUX_SRC:-}"
  if [ -z "$SRC" ]; then
    TB="${IYAKKI_LINUX_TARBALL:-$WORK/linux-$KVER.tar.xz}"
    [ -f "$TB" ] || { echo "hwsim-hotspot: downloading linux-$KVER from kernel.org"; curl -fL "https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-$KVER.tar.xz" -o "$TB"; }
    [ -d "$WORK/linux-$KVER" ] || tar -xf "$TB" -C "$WORK"
    SRC="$WORK/linux-$KVER"
  fi
  mkdir -p "$WORK/kbuild"
  SRC="$SRC" O="$WORK/kbuild" bash "$ROOT/crates/iyakki-hotspot/tools/hwsim-kbuild.sh" > "$WORK/kbuild.log" 2>&1 || { tail -20 "$WORK/kbuild.log"; echo "hwsim-hotspot: the test kernel did not build" >&2; exit 2; }
fi
mkdir -p "$HOME/iyakki-work/logs"
out="$(WORK="$WORK" bash "$ROOT/crates/iyakki-hotspot/tools/hwsim-test.sh" 2>&1)" || true
echo "$out" | grep -E "^(PASS|FAIL)|check\(s\) failed|== boot|vm exit" 
echo "$out" | grep -q "== 0 check(s) failed" && { echo "hwsim-hotspot: PASS"; exit 0; }
echo "hwsim-hotspot: FAIL (see $WORK/hwsim-good.log and hwsim-bad.log)" >&2; exit 1
