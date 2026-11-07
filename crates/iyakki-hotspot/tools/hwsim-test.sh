#!/bin/bash
# Virtual-radio test of the Wi-Fi hotspot (plan: docs/development/HOTSPOT-PLAN.md, phase P1).
#
# Boots a QEMU microVM with a small test kernel and a small initramfs. Inside it the mac80211_hwsim module makes two virtual
# radios; our access point (crates/iyakki-agent/examples/hotspot_hwsim.rs, real nl80211 + AF_PACKET code) runs on radio 0 and the
# host's wpa_supplicant joins it from radio 1 (in its own network namespace). Two boots: right passphrase and wrong passphrase.
#
# Safe for the development PC: no module is loaded on the host, the guest has NO network device, 1 GB RAM, a hard time-out, and
# everything is written under $WORK (default ~/iyakki-work/hwsim). Needs: qemu-system-x86_64 with /dev/kvm, busybox (static),
# zstd, cpio, wpa_supplicant, iw and a bootable kernel image with hwsim (see KERNEL below).
#
# Usage: hwsim-test.sh            build what is needed, run both boots, print PASS/FAIL per check
#        hwsim-test.sh --check   only evaluate the logs of the last run
#        hwsim-test.sh --unit     only run the socket-framing unit tests of the transport (no VM)
set -u
REPO=$(cd "$(dirname "$0")/../../.." && pwd)
WORK=${WORK:-$HOME/iyakki-work/hwsim}
KVER=$(uname -r)
# The distribution kernel in /boot is root-only on most systems (it was here), so the default is a small kernel with cfg80211, mac80211
# and mac80211_hwsim built in (kernel.frag + kbuild.sh in $WORK, built once from the 6.12 source tree). KERNEL=... overrides.
KERNEL=${KERNEL:-$WORK/kbuild/arch/x86/boot/bzImage}
BIN=$WORK/target/debug/hotspot_hwsim
LOGS=$HOME/iyakki-work/logs
mkdir -p "$WORK" "$LOGS"

cargo_run() { # one cargo at a time, capped, as the project rules say
  flock "$HOME/.cache/iyakki-build.lock" env CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2 CARGO_TARGET_DIR="$WORK/target" \
    systemd-run --user --scope -q -p MemoryMax=9G -p MemorySwapMax=6G cargo "$@"
}

# The test binary and the transport tests are built from the repository's own source files in a tiny stand-alone crate, so the
# VM test does not depend on the rest of the agent compiling at that moment. The crate name is the agent's, so the example is unchanged.
make_crate() {
  local c=$WORK/bincrate
  mkdir -p "$c/src"
  cat > "$c/Cargo.toml" <<EOT
[package]
name = "iyakki-agent"
version = "0.0.0"
edition = "2021"
[lib]
path = "src/lib.rs"
[[bin]]
name = "hotspot_hwsim"
path = "$REPO/crates/iyakki-agent/examples/hotspot_hwsim.rs"
[[bin]]
name = "hotspot_probe"
path = "$REPO/crates/iyakki-agent/examples/hotspot_probe.rs"
[dependencies]
iyakki-hotspot = { path = "$REPO/crates/iyakki-hotspot" }
libc = "0.2"
[workspace]
EOT
  cat > "$c/src/lib.rs" <<EOT
#[path = "$REPO/crates/iyakki-agent/src/nl80211_os.rs"]
pub mod nl80211_os;
#[path = "$REPO/crates/iyakki-agent/src/hotspot_radio.rs"]
pub mod hotspot_radio;
EOT
  [ -f "$REPO/Cargo.lock" ] && [ ! -f "$c/Cargo.lock" ] && cp "$REPO/Cargo.lock" "$c/Cargo.lock"
}

make_crate
if [ "${1:-}" = "--check" ]; then CHECK_ONLY=1; else CHECK_ONLY=0; fi
if [ "${1:-}" = "--unit" ]; then
  cargo_run test --manifest-path "$WORK/bincrate/Cargo.toml" --lib 2>&1 | tail -25
  exit 0
fi

if [ "$CHECK_ONLY" = 0 ]; then
echo "== building the test binary"
cargo_run build --manifest-path "$WORK/bincrate/Cargo.toml" --bins > "$LOGS/hwsim-bin-build.log" 2>&1 || { echo "build failed, see $LOGS/hwsim-bin-build.log"; tail -20 "$LOGS/hwsim-bin-build.log"; exit 1; }
BIN=$WORK/target/debug/hotspot_hwsim

echo "== building the initramfs"
R=$WORK/rootfs
rm -rf "$R"; mkdir -p "$R"/{bin,sbin,proc,sys,dev,tmp,etc,lib64,usr/lib/x86_64-linux-gnu}
cp /usr/bin/busybox "$R/bin/busybox"
for a in $(/usr/bin/busybox --list); do [ "$a" = busybox ] || ln -sf busybox "$R/bin/$a"; done
copy_with_libs() { # binary, destination
  cp "$1" "$2"
  ldd "$1" | awk '/=>/ {print $3} /ld-linux/ {print $1}' | grep '^/' | while read -r l; do
    d="$R$(dirname "$l")"; mkdir -p "$d"; cp --update=none "$l" "$d/"
  done
}
copy_with_libs /usr/sbin/wpa_supplicant "$R/sbin/wpa_supplicant"
copy_with_libs /usr/sbin/iw "$R/sbin/iw"
copy_with_libs "$BIN" "$R/bin/hotspot_hwsim"
cp "$(dirname "$0")/hwsim-init.sh" "$R/init"; chmod +x "$R/init"
( cd "$R" && find . | cpio -o -H newc --quiet | zstd -q -3 -f -o "$WORK/initramfs.cpio.zst" )
ls -l "$WORK/initramfs.cpio.zst" | awk '{print "   initramfs bytes:", $5}'

run_vm() { # scenario
  local log=$WORK/hwsim-$1.log
  : > "$log"
  timeout -k 5 150 qemu-system-x86_64 -enable-kvm -cpu host -smp 2 -m 1024 -nographic -no-reboot -nic none \
    -kernel "$KERNEL" -initrd "$WORK/initramfs.cpio.zst" \
    -append "console=ttyS0 rdinit=/init scenario=$1 panic=-1 loglevel=3 mac80211_hwsim.radios=2" > "$log" 2>&1
  echo "   $1: vm exit $? ($(wc -l < "$log") log lines) -> $log"
}
echo "== boot 1: right passphrase"; run_vm good
echo "== boot 2: wrong passphrase";  run_vm bad

fi

G=$WORK/hwsim-good.log; B=$WORK/hwsim-bad.log
fails=0
chk() { # name, file, extended regex
  if grep -Eq -- "$3" "$2"; then echo "PASS  $1"; else echo "FAIL  $1"; fails=$((fails+1)); fi
}
nochk() { # name, file, regex that must NOT appear
  if grep -Eq -- "$3" "$2"; then echo "FAIL  $1"; fails=$((fails+1)); else echo "PASS  $1"; fi
}
echo "== results"
chk   "radios listed, radio 0 reports AP mode (probe)"        "$G" 'AP: radios .*true'
chk   "capability probe says yes for 2.4 GHz channel 6"       "$G" 'AP: probe verdict yes: channel 6 \(2437 MHz\) width 20 mode Software'
chk   "access point started (SET_INTERFACE, START_AP, GTK)"   "$G" 'AP: started ssid=IYAKKI-TEST'
chk   "supplicant found the beacon and authenticated"         "$G" 'wlan1: authenticated'
chk   "auth + association answered by our mgmt code"          "$G" 'AP: associated .* handshake started'
chk   "4-way handshake: pairwise key installed (NEW_KEY)"     "$G" 'AP: NEW_KEY pairwise installed'
chk   "station authorized (SET_STATION)"                      "$G" 'AP: authorized'
chk   "supplicant reports COMPLETED / CONNECTED"              "$G" 'CTRL-EVENT-CONNECTED|-> COMPLETED'
chk   "DHCP: our server acked an address"                     "$G" 'AP: dhcp reply type 5'
chk   "DHCP: client bound an address from 10.42.0.x"          "$G" 'udhcpc bound 10\.42\.0\.'
chk   "ping from client to the AP address works (CCMP data)"  "$G" 'GUEST: PING-OK'
nochk "wrong passphrase: supplicant never connects"           "$B" 'CTRL-EVENT-CONNECTED|-> COMPLETED'
chk   "wrong passphrase: AP reports the handshake failure"    "$B" 'AP: handshake failed for .*: wrong passphrase'
nochk "wrong passphrase: no pairwise key installed"           "$B" 'AP: NEW_KEY pairwise installed'
nochk "wrong passphrase: no station authorized"               "$B" 'AP: authorized'
echo "== $fails check(s) failed"
exit $((fails>0))
