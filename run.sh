#!/usr/bin/env bash
# IYAKKI launcher: try it, test it, boot it.     ./run.sh help
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
IMG="$ROOT/target/buildroot-libre/images"

usage() {
cat <<'TXT'
IYAKKI launcher

  ./run.sh window     open the IYAKKI shell in a desktop window (fastest way to try it); it shows virtual
                      disks only, never your real drives (--real-disks to see them)
  ./run.sh disk       add 8G | list | rm vda: the window's virtual disks
                      first run: create the administrator account on the setup screen
  ./run.sh vm         boot the real ISO in QEMU (BIOS + GRUB + kernel + iyakki as PID 1)
  ./run.sh vm-direct  same, skipping GRUB (faster boot); add --ssh to reach it with: ssh -i ~/.ssh/iyakki_diag -p 2222 root@127.0.0.1
  ./run.sh vm-disk    boot an installed disk like the USB stick (GRUB, data partition that keeps your account between runs)
                      add --uefi for UEFI instead of BIOS;  ./run.sh vm-reset  forgets that disk
  ./run.sh vm-arm     boot the ARM64 image the way an Apple Silicon VM does (UEFI; slow without KVM, but real);  build-arm builds it
  ./run.sh screens    render every screen to PNG files in /tmp/iyakki-screens
  ./run.sh test       run the whole automated test suite (first the warnings gate)
  ./run.sh warnings   the warnings gate alone: cargo check of every target must print no warning
  ./run.sh vmtest     boot in QEMU, type into the login screen, save screenshots
  ./run.sh build      build the ISO (first time: ~30-40 min, downloads ~1 GB)
  ./run.sh build-mac  OPTIONAL image for Intel Macs (Mac mini 2014 and similar): adds applesmc, hid_apple, Cirrus audio and the Wi-Fi
                      firmware files; see docs/MAC-HARDWARE.md. Not part of the default build.
  ./run.sh build-web  OPTIONAL big image with the full web engine (WPE WebKit in the shell, Mesa, GStreamer): YouTube and modern
                      sites; hours to build, needs ~30 GB disk and 16+ GB RAM; add --h264 for H.264/AAC; see docs/FULL-BROWSER.md
  ./run.sh build-macweb  OPTIONAL: build-mac and build-web in one image (Intel Macs such as the Mac mini 2014 that should run YouTube and modern
                      sites); output target/buildroot-macweb; --h264 for H.264/AAC, --crocus for Mesa's Haswell GPU driver (hours more); docs/YOUTUBE-ON-IYAKKI.md
  ./run.sh build-pc-layered  OPTIONAL: the PC image (macweb recipe, no compile): the whole GPU firmware set (NVIDIA GSP, all AMD, all Intel) and common PC
                      Wi-Fi/Ethernet firmware, for an ordinary x86-64 PC; output target/buildroot-pc; docs/GPU-K3S.md
                      Restricted firmware (Qualcomm Bluetooth, Realtek and Broadcom Ethernet, Atheros AR9170) is NOT in these images; for an image
                      of your OWN use that includes it: IYAKKI_INCLUDE_RESTRICTED_FIRMWARE=1 ./run.sh build-pc-layered (also build-mac, build-macweb).
                      Not for redistribution; docs/RESTRICTED-FIRMWARE.md
  ./run.sh licence-check [--strict] [--write] [--no-images] [--verbose]  release gate point 7: Cargo licences, assets, downloads, notices/SBOM, legal-info, content scan of every target/buildroot-*/images (exit 1 on FAIL; docs: LICENSING.md)
  ./run.sh dlmanifest keygen|sign|verify|check|gen  the signed downloads manifest (versions and checksums of the download items; docs/development/DOWNLOADS-MANIFEST.md)
  ./run.sh image-programs  fails when production code starts a program the images do not ship (list: dist/buildroot/image-programs.txt)
  ./run.sh alpha-check  the gate before an alpha: build, tests, licences, docs links, versions, secrets, QEMU smoke tests (--dry: fast checks only)
  ./run.sh check-macos  cargo check of the window crates for the macOS targets (compiles only; skips when the Rust target is not installed)
  ./run.sh release [version]  collect the images, sign them, write release.json and the download page (docs/RELEASING.md)
  ./run.sh cli ARGS   run the iyakki command line, e.g.  ./run.sh cli run tests/fixtures/hello.wat

Reset the desktop demo:  rm -rf ~/.local/share/iyakki-dev
TXT
}

# QEMU needs more locked memory than some desktops allow a user (io_uring). Raise it for this one
# process with sudo; QEMU itself still runs as you, so the window opens on your own display.
qemu_run() {
  local probe
  probe="$(qemu-system-x86_64 -machine none -display none -S -monitor none 2>&1 </dev/null & sleep 1; kill %1 2>/dev/null || true)" || true
  if echo "$probe" | grep -q io_uring; then
    echo "(raising the locked-memory limit for QEMU; sudo may ask for your password)" >&2
    sudo prlimit --memlock=unlimited:unlimited --pid $$
  fi
  exec qemu-system-x86_64 "$@"
}

vm() {
  local mode="$1"
  [ -f "$IMG/rootfs.iso9660" ] || { echo "No image yet. Run:  ./run.sh build" >&2; exit 1; }
  command -v qemu-system-x86_64 >/dev/null || { echo "Install QEMU:  sudo apt install qemu-system-x86" >&2; exit 1; }
  local boot=(-cdrom "$IMG/rootfs.iso9660" -boot d)
  local cmdline="console=ttyS0 panic=-1" fwd=""
  if [ "${2:-}" = "--ssh" ]; then cmdline="$cmdline iyakki.ssh=1"; fwd=",hostfwd=tcp:127.0.0.1:2222-:22"; fi
  [ "$mode" = direct ] && boot=(-kernel "$IMG/bzImage" -initrd "$IMG/rootfs.cpio.zst" -append "$cmdline")
  echo "Booting IYAKKI. Click inside the window to capture the mouse (Ctrl+Alt+G releases it)."
  qemu_run -accel kvm -accel tcg -m 1536 -smp 2 -name IYAKKI \
    -device virtio-vga -display "${IYAKKI_VM_DISPLAY:-gtk}" \
    -device qemu-xhci -device usb-kbd -device usb-tablet \
    -netdev "user,id=n0$fwd" -device virtio-net-pci,netdev=n0 "${boot[@]}" -serial "file:/tmp/iyakki-vm-serial.log"
}

# A disk made the way the USB writer makes a stick: boot partition with GRUB, data partition for your files.
vm_disk() {
  local disk="$ROOT/target/vm/iyakki-disk.img" fw=()
  [ -f "$IMG/iyakki-install-x86_64.img" ] || { echo "No image yet. Run:  ./run.sh build" >&2; exit 1; }
  mkdir -p "$ROOT/target/vm"
  if [ ! -f "$disk" ]; then
    cargo build -q -p iyakki-usb
    truncate -s 4G "$disk"
    "$ROOT/target/debug/iyakki-usb" "$IMG/iyakki-install-x86_64.img" --target-file "$disk" --erase testfile >/dev/null
    echo "Made a 4 GB virtual disk: $disk (the first boot formats its data partition)"
  fi
  if [ "${1:-}" = "--uefi" ]; then fw=(-drive "if=pflash,format=raw,readonly=on,file=${OVMF:-/usr/share/OVMF/OVMF_CODE_4M.fd}"); fi
  echo "In the GRUB menu pick the entry you want. Click inside the window to capture the mouse (Ctrl+Alt+G releases it)."
  qemu_run -accel kvm -accel tcg -m 2048 -smp 4 -name "IYAKKI disk" "${fw[@]}" \
    -device virtio-vga -display "${IYAKKI_VM_DISPLAY:-gtk}" \
    -device qemu-xhci -device usb-kbd -device usb-tablet \
    -netdev user,id=n0,hostfwd=tcp:127.0.0.1:2222-:22 -device virtio-net-pci,netdev=n0 \
    -drive "file=$disk,format=raw,if=none,id=d0" -device usb-storage,drive=d0 -serial "file:/tmp/iyakki-vm-serial.log"
}

vm_arm() {
  local img="$ROOT/target/buildroot-libre-aarch64/images/iyakki-aarch64.img" vars="$ROOT/target/vm/aavmf-vars.fd" disk="$ROOT/target/vm/iyakki-arm.img"
  [ -f "$img" ] || { echo "No ARM image yet. Run:  ./run.sh build-arm" >&2; exit 1; }
  command -v qemu-system-aarch64 >/dev/null || { echo "Install QEMU:  sudo apt install qemu-system-arm qemu-efi-aarch64" >&2; exit 1; }
  mkdir -p "$ROOT/target/vm"
  [ -f "$vars" ] || cp /usr/share/AAVMF/AAVMF_VARS.fd "$vars"
  [ -f "$disk" ] || cp "$img" "$disk"
  echo "Booting the ARM64 image (emulated; the first boot takes a minute or two). Ctrl+Alt+G releases the mouse."
  exec qemu-system-aarch64 -M virt -cpu cortex-a72 -accel tcg -m 3072 -smp 4 -name "IYAKKI ARM" \
    -drive if=pflash,format=raw,readonly=on,file=/usr/share/AAVMF/AAVMF_CODE.fd -drive if=pflash,format=raw,file="$vars" \
    -drive file="$disk",format=raw,if=none,id=hd0 -device virtio-blk-pci,drive=hd0 \
    -device virtio-gpu-pci -display "${IYAKKI_VM_DISPLAY:-gtk}" -device qemu-xhci -device usb-kbd -device usb-tablet \
    -netdev user,id=n0 -device virtio-net-pci,netdev=n0 -serial "file:/tmp/iyakki-vm-serial-arm.log"
}

case "${1:-help}" in
  window)
    shift
    cargo build --release --manifest-path crates/iyakki-gui/Cargo.toml --bin iyakki-desktop
    exec crates/iyakki-gui/target/release/iyakki-desktop "$@" ;;
  disk)
    # virtual disks for the window (sparse files; the window never shows the computer's real drives):
    #   ./run.sh disk add 8G | list | rm vda
    shift
    cargo build --release --manifest-path crates/iyakki-gui/Cargo.toml --bin iyakki-desktop
    exec crates/iyakki-gui/target/release/iyakki-desktop disk "$@" ;;
  vm) vm iso ;;
  vm-direct) shift; vm direct "$@" ;;
  vm-disk) shift; vm_disk "$@" ;;
  vm-arm) vm_arm ;;
  vm-reset) rm -f "$ROOT/target/vm/iyakki-disk.img" "$ROOT/target/vm/iyakki-arm.img" "$ROOT/target/vm/aavmf-vars.fd"; echo "The virtual disks are gone; the next vm-disk / vm-arm starts fresh." ;;
  build-arm) ARCH=aarch64 exec dist/buildroot/build.sh libre ;;
  screens)
    cargo run -q --release -p iyakki-shell --example screens
    echo "Screenshots in /tmp/iyakki-screens:"; ls /tmp/iyakki-screens
    command -v xdg-open >/dev/null && xdg-open /tmp/iyakki-screens >/dev/null 2>&1 || true ;;
  warnings)
    # the warnings gate: every crate, every target (lib, bins, tests, examples) must compile with ZERO compiler warnings.
    # Same cargo cache as a normal check (no RUSTFLAGS change, so no full rebuild); fails on the first warning it finds.
    shift; exec dist/release/warnings-gate.sh "$@" ;;
  test)
    # tests leak temporary folders: keep them on the disk (not in a small or RAM-backed /tmp) and clean old ones afterwards
    . dist/release/env.sh; trap iyakki_tmp_clean EXIT
    dist/release/warnings-gate.sh
    # KI-060: the run gets its own temp folder, removed at the end; growth of iyakki-* in the system /tmp fails the step
    iyakki_run_tmp_begin; rc=0
    cargo test --workspace || rc=$?
    iyakki_run_tmp_end || rc=1
    [ "$rc" -eq 0 ] || exit "$rc"
    (cd crates/iyakki-gui && cargo build -q --bin iyakki-desktop && echo "iyakki-gui: builds") ;;
  vmtest)
    [ -f "$IMG/rootfs.cpio.zst" ] || { echo "No image yet. Run:  ./run.sh build" >&2; exit 1; }
    sudo python3 dist/buildroot/qemu-ui-test.py target/buildroot-libre /tmp/iyakki-qemu
    echo "Screenshots in /tmp/iyakki-qemu" ;;
  # The build commands go through smart-build.sh: it reuses the images when nothing changed, repacks when only the iyakki binary
  # changed, and builds only what changed otherwise (docs/BUILD-INCREMENTAL.md). --force-full, --clean, --dry-run are passed on.
  build) shift; exec dist/buildroot/smart-build.sh libre ${1+"$@"} ;;
  build-mac) shift; exec dist/buildroot/smart-build.sh mac ${1+"$@"} ;;
  build-macweb)
    shift
    SB=()
    for a in "$@"; do case "$a" in --h264) export WEB_H264=1 ;; --iris) export WEB_IRIS=1 ;; --crocus) export WEB_CROCUS=1 ;; --force-full|--clean|--dry-run) SB+=("$a") ;; *) echo "build-macweb: unknown option $a" >&2; exit 2 ;; esac; done
    exec dist/buildroot/smart-build.sh macweb ${SB[@]+"${SB[@]}"} ;;
  build-web)
    shift
    SB=(); for a in "$@"; do case "$a" in --h264) export WEB_H264=1 ;; *) SB+=("$a") ;; esac; done
    exec dist/buildroot/smart-build.sh web ${SB[@]+"${SB[@]}"} ;;
  # macweb without compiling WebKit again: the web rootfs with the mac kernel, modules and firmware (minutes, not hours)
  build-macweb-layered) shift; exec dist/buildroot/repack.sh macweb --base target/buildroot-web --kernel-from target/buildroot-mac --graft git --cargo ${1+"$@"} ;;
  # The PC image (docs/GPU-K3S.md): the macweb recipe for ordinary x86-64 PCs, with the FULL GPU firmware set (NVIDIA GSP, all AMD, all Intel) and the
  # Wi-Fi, Bluetooth and Ethernet firmware of common PCs. Output target/buildroot-pc (images/iyakki-x86_64.iso, iyakki-install-x86_64.img),
  # /etc/iyakki-release says pc-x86_64. Needs the web and mac builds like build-macweb-layered; writes no drive.
  build-pc-layered) shift; exec dist/buildroot/repack.sh pc --base target/buildroot-web --kernel-from target/buildroot-mac --graft git --cargo ${1+"$@"} ;;
  build-changed) shift; exec dist/buildroot/changed.sh ${1+"$@"} ;;
  release)
    # ./run.sh release [version] [--with-web] [--with-mac] [--with-macweb] [--no-window] [--allow-stale] [--dry-run]
    # collects images + window tarball + documents in target/release-<version>/, signs them (key: ~/.config/iyakki/release.key,
    # made once: back it up), writes release.json and the download page. Details: docs/RELEASING.md
    shift; exec dist/release/release.sh "$@" ;;
  licence-check|license-check)
    # the licence and proprietary-software gate (iyakki-relcheck): reads legal/*.txt, Cargo.lock, target/buildroot-*/{legal-info,images}; writes nothing unless --write
    shift; exec cargo run -q -p iyakki-relcheck -- licence-check . "$@" ;;
  dlmanifest)
    # the signed downloads manifest: keygen FILE | sign MANIFEST --key FILE | verify MANIFEST [--pubkey HEX|FILE] | check | gen (docs/development/DOWNLOADS-MANIFEST.md)
    shift; exec cargo run -q -p iyakki-core -- dlmanifest "$@" ;;
  image-programs)
    # the guard against a missing program on the image (the curl bug): scans production code for spawned programs; narrow test, no image needed
    exec cargo test -q -p iyakki-agent --test image_programs ;;
  alpha-check)
    # the gate before an alpha: build, test, licences, docs links, versions, secrets, QEMU smoke tests -> PASS/FAIL table
    shift; exec dist/release/alpha-check.sh "$@" ;;
  check-macos)
    # does the code still compile for macOS (cargo check of the window's crates for the installed apple-darwin targets)? Never links or runs
    shift; exec dist/release/check-macos.sh "$@" ;;
  cli) shift; exec cargo run -q -p iyakki-core -- "$@" ;;
  help|-h|--help) usage ;;
  *) usage; exit 2 ;;
esac
