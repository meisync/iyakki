#!/bin/sh
# Real-kernel integration tests on a Linux host (the ignored tests that need nf_tables, WireGuard, namespaces and root), run in throw-away network namespaces.
#   dist/linux-host/kernel-tests.sh [--keep]       see docs/development/LINUX-GATE-HOST.md ("Kernel tests")
# What runs, each in a fresh network namespace made by `sudo unshare -n` (the host's own network, routes and firewall are never touched; nothing is installed):
#   iyakki-fw   tests/kernel.rs, counters_sets_kernel.rs, tablegen_kernel.rs   our nf_tables encoder against the real kernel, `nft list` decodes it, a second
#               namespace is let in or dropped, blocked attempts are counted and logged, the VPN guard and DNS-redirect tables load and elements come and go
#   iyakki-fw   tests/direct_kernel.rs   the direct-link zone and a page rule on 48 real packets over two veth cables: the kernel and the packet model agree
#   iyakki-fw   tests/engine_kernel.rs   the firewall engine on a real kernel with a fake clock: a change that cuts off the remote desktop port is applied and really cuts it, is taken back by itself after 60 s, a kept change
#               stays and is saved, the emergency switch removes the table; the audit lines are checked
#   iyakki-fw   tests/gate_kernel.rs   the domain listener's gate rules (`gaterule`) on real connections from a private and a public address: shut, local network only, everybody, one port closed, an expired opening, shut again
#   iyakki-agent tests/vpn_kernel.rs   WireGuard between two namespaces through IYAKKI's own system layer (generic netlink, `ip`), handshake, traffic, teardown; the VPN kill switch,
#               the fail-closed guard (with a network prefix) and the DNS-redirect table on real traffic
#   iyakki-agent lib test sharedfolder_tests::a_real_second_network_stack...   a share on every address, used from two other network stacks with the real curl: a private
#               address fetches, lists, resumes and uploads, a public one is kept out
#   iyakki-agent tests/directlink_kernel.rs   the direct-link manager (`directlink::real`) on a veth cable called thunderbolt0 between two namespaces: the real ioctls, raw ARP probe with a real conflict, no forwarding, beacons both ways
#   iyakki-agent lib test tests_realfat   the system update engine on a 512 MB loop disk (200 MB FAT32 boot partition) with a real FAT32 boot partition and an ext4 data volume: the real mount of the boot partition (`SysBoot`),
#               a signed pack installed and rolled back, the volumes compared and checked with fsck.fat and e2fsck
#   iyakki-agent tests/sharing_kernel.rs   internet sharing (`sharing::manager` with the real `RealOs`): a real dhcpcd gets a lease and the resolver, dig asks the forwarder, a device is NATed to a public
#               upstream, kept off this computer and off the uplink's private range (until "allow the local network"), everything restored at the end (needs dhcpcd, dnsmasq, dig)
#   iyakki-agent tests/disks_crypt_kernel.rs   the DISKS tile's helper runner against real cryptsetup and dm-crypt: LUKS1 and LUKS2 on a loop device, right key, wrong key, read-only, close
#   iyakki-netscan lib test real_avahi_tests   our mDNS announcer and resolver beside a real avahi-daemon (no D-Bus, private /run) over a veth cable: avahi's name resolved, our probes without
#               conflict, a real `dig` reading our host and service records, a name conflict in both directions
# (`ONLY=text dist/linux-host/kernel-tests.sh` runs only the programs whose title contains text.)
# Rules for every program here (amd-mini and other shared machines): the root part runs only inside `unshare -nm` with a private tmpfs over /run (set up by this script, which also sets
# IYAKKI_IN_SANDBOX=1; every test refuses to run as root without it), each daemon (avahi-daemon, dnsmasq, dhcpcd) gets scratch paths for its pid, config and lease files, nothing is written
# under /dev, /etc, /var or /usr of the machine, and /dev/null, /dev/zero and /dev/urandom must look the same after a run as before (this script checks that at the end).
# (`unshare -nm`: a private mount namespace too, so that /sys shows this namespace's interfaces; the host's mounts are not changed.)
# Needs: passwordless sudo, unshare, nsenter, ip, ping, curl, openssl, base64, nft, python3, avahi-daemon, dig, cryptsetup, losetup, mke2fs, a kernel with nf_tables and wireguard. A tool or module that is missing is
# reported by the test itself ("skipped: ...", which this script counts as NOT RUN, never as passed). The source is copied to a world-readable scratch folder
# first (a root process in a user namespace cannot read a private home folder), built there as the normal user and run as root.
# Exit 0 when every test ran and passed, 1 when one failed, 2 when something could not run (missing sudo or tool).
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"; ROOT="$(cd "$HERE/../.." && pwd)"
KEEP=0; [ "${1:-}" = "--keep" ] && KEEP=1
for t in sudo unshare nsenter ip ping openssl base64 nft python3 cargo tar; do command -v "$t" >/dev/null 2>&1 || [ -x "/usr/sbin/$t" ] || { echo "kernel-tests: needs $t" >&2; exit 2; }; done
sudo -n true 2>/dev/null || { echo "kernel-tests: needs passwordless sudo (the tests run as root inside a throw-away network namespace)" >&2; exit 2; }
W="$(mktemp -d /tmp/iyakki-kernel-tests.XXXXXX)"; chmod 755 "$W"
trap '[ "$KEEP" = 1 ] && echo "kept $W" || { rm -rf "$W" 2>/dev/null; [ -e "$W" ] && sudo -n rm -rf "$W"; }' EXIT
DEVS_BEFORE="$(ls -l /dev/null /dev/zero /dev/urandom 2>&1)"
echo "== copying the source to $W/src"
mkdir "$W/src"; (cd "$ROOT" && tar --exclude=./target --exclude=./.git --exclude=./iyakki-boot -cf - .) | tar -xf - -C "$W/src"
chmod -R o+rX "$W/src"
echo "== building the test programs (scratch target folder)"
( cd "$W/src" && CARGO_TARGET_DIR="$W/tgt" cargo build --locked -q -p iyakki-fw --examples && \
  CARGO_TARGET_DIR="$W/tgt" cargo test --locked -q -p iyakki-fw --no-run && CARGO_TARGET_DIR="$W/tgt" cargo test --locked -q -p iyakki-agent --test vpn_kernel --test disks_crypt_kernel --test directlink_kernel --test sharing_kernel --no-run && CARGO_TARGET_DIR="$W/tgt" cargo test --locked -q -p iyakki-agent --lib --no-run && CARGO_TARGET_DIR="$W/tgt" cargo test --locked -q -p iyakki-netscan --lib --no-run ) 2>&1 | tail -3
chmod -R o+rX "$W/tgt"
fail=0; skipped=0; ran=0
run() { # name pattern [filter]
  [ -z "${ONLY:-}" ] || echo "$1" | grep -q "$ONLY" || return 0   # ONLY=text runs just the programs whose name contains it
  f="$(ls "$W"/tgt/debug/build/*/*/out/"$2"-* 2>/dev/null | grep -v '\.d$' | head -1)"
  [ -n "$f" ] || { echo "kernel-tests: no test program for $1"; fail=1; return; }
  echo "== $1"
  out="$(sudo -n timeout 600 unshare -nm sh -c "export IYAKKI_IN_SANDBOX=1; mount --make-rprivate / 2>/dev/null; mount -t tmpfs tmpfs /run; mount -t sysfs sysfs /sys; ip link set lo up; cd $W/src && $f --ignored --test-threads=1 --nocapture ${3:-}" 2>&1)"; rc=$?
  echo "$out" | grep -E "^test |test result|skipped" | cut -c1-200
  [ "$rc" = 0 ] || { fail=1; echo "$out" | tail -25 | cut -c1-200; }
  echo "$out" | grep -q "skipped:" && skipped=$((skipped + 1))
  ran=$((ran + 1))
}
run "iyakki-fw kernel.rs" kernel
run "iyakki-fw counters_sets_kernel.rs" counters_sets_kernel
run "iyakki-fw tablegen_kernel.rs" tablegen_kernel
run "iyakki-fw direct_kernel.rs (direct-link zone and page rules on real packets, two cables)" direct_kernel
run "iyakki-fw engine_kernel.rs (the engine: apply, 60-second Keep/Revert of a change that cuts off the remote desktop, keep and save, emergency off, on a real kernel)" engine_kernel
run "iyakki-fw gate_kernel.rs (the domain listener gate: local network, everybody, closed, expired, on real connections)" gate_kernel
run "iyakki-agent vpn_kernel.rs (WireGuard, kill switch, DNS capture, policy routing, manager over the helper socket)" vpn_kernel
run "iyakki-agent directlink_kernel.rs (the direct-link manager on a real cable: ioctls, ARP probes, forwarding, beacons)" directlink_kernel
run "iyakki-agent tests_realfat (the update engine on a real FAT32 boot partition and ext4 data volume, install and rollback)" iyakki_agent the_update_engine_installs_and_rolls_back_on_a_real_fat32
run "iyakki-agent sharing_kernel.rs (internet sharing: real DHCP client, DNS, NAT and the firewall table on a real cable)" sharing_kernel
run "iyakki-agent disks_crypt_kernel.rs (cryptsetup, LUKS1 and LUKS2)" disks_crypt_kernel
run "iyakki-agent sharedfolder_tests (a real second network stack with curl)" iyakki_agent a_real_second_network_stack
run "iyakki-netscan real_avahi_tests (our mDNS beside a real avahi-daemon and a real dig)" iyakki_netscan real_avahi
[ "$(ls -l /dev/null /dev/zero /dev/urandom 2>&1)" = "$DEVS_BEFORE" ] || { echo "kernel-tests: /dev/null, /dev/zero or /dev/urandom CHANGED during the run" >&2; fail=1; }
echo "kernel-tests: $ran program(s) run, $skipped reported a skip, failed=$fail"
[ "$fail" = 0 ] || exit 1
[ "$skipped" = 0 ] || { echo "kernel-tests: a test skipped itself: that is NOT a pass" >&2; exit 2; }
echo "kernel-tests: PASS"
