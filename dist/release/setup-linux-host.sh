#!/usr/bin/env bash
# Make an Ubuntu / Debian computer a gate runner: the workspace tests (dist/release/test-with-rerun.sh), the loader and QEMU tests, the image scripts.
#   dist/release/setup-linux-host.sh            install what is missing (needs sudo for apt), then check everything and print one line per check
#   dist/release/setup-linux-host.sh --check    only check, install nothing (exit 1 when something needed is missing)
# Settings (environment): IYAKKI_RUST (the Rust toolchain: default stable; the dev PC may pin one, e.g. 1.99.0), IYAKKI_EXTRA_TARGETS (more rustup targets).
# Safe to run again: apt only installs what is missing, rustup only adds what is missing. No secrets are read or written, no network address is used
# other than the package archives and rustup.rs. See docs/development/LINUX-GATE-HOST.md for what each item is for and the differences found on a fresh host.
set -uo pipefail
CHECK=0; [ "${1:-}" = "--check" ] && CHECK=1
RUST="${IYAKKI_RUST:-stable}"
TARGETS="aarch64-unknown-linux-gnu x86_64-unknown-linux-musl wasm32-unknown-unknown x86_64-unknown-uefi ${IYAKKI_EXTRA_TARGETS:-}"
# what the tests and scripts start: build tools, Node (browser-compat and JavaScript tests), python3 + Pillow (image tests), the QEMU tests (qemu, OVMF,
# GRUB tools, mtools, dosfstools, gdisk for sgdisk), the image and ISO scripts (squashfs-tools, xorriso, fakeroot, cpio, zstd), signing (sbsigntool, openssl),
# terminal and network tests (openssh-client, iproute2, iw, alsa-utils, procps, util-linux for `script`), jq.
PKGS="build-essential musl-tools cryptsetup tesseract-ocr pkg-config git curl ca-certificates nodejs python3 python3-pil qemu-system-x86 qemu-utils ovmf mtools dosfstools gdisk squashfs-tools xorriso
grub-common grub-pc-bin grub-efi-amd64-bin sbsigntool fakeroot cpio zstd xz-utils openssl openssh-client iproute2 iw alsa-utils procps util-linux e2fsprogs jq"
fail=0
say() { printf '%-28s %s\n' "$1" "$2"; }
bad() { say "$1" "MISSING: $2"; fail=1; }

if [ "$CHECK" = 0 ]; then
  if command -v apt-get >/dev/null 2>&1; then
    missing=""
    for p in $PKGS; do dpkg -s "$p" >/dev/null 2>&1 || missing="$missing $p"; done
    if [ -n "$missing" ]; then
      echo "setup-linux-host: installing:$missing"
      SUDO=""; [ "$(id -u)" = 0 ] || SUDO="sudo"
      $SUDO env DEBIAN_FRONTEND=noninteractive apt-get update -qq && $SUDO env DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends $missing \
        || echo "setup-linux-host: apt could not install everything (see above); the checks below say what is still missing" >&2
    fi
  else
    echo "setup-linux-host: no apt-get here: install the tools named in this script by hand" >&2
  fi
  if ! command -v rustup >/dev/null 2>&1 && [ ! -x "$HOME/.cargo/bin/rustup" ]; then
    echo "setup-linux-host: installing rustup (from rustup.rs)"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain "$RUST" || echo "setup-linux-host: rustup install failed" >&2
  fi
fi
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
if command -v rustup >/dev/null 2>&1; then
  if [ "$CHECK" = 0 ]; then
    rustup toolchain list | grep -q "^$RUST" || rustup toolchain install "$RUST" --profile minimal
    rustup default "$RUST" >/dev/null 2>&1 || true
    for t in $TARGETS; do rustup target list --installed | grep -qx "$t" || rustup target add "$t" || echo "setup-linux-host: could not add the Rust target $t (the loader target may need a newer toolchain)" >&2; done
  fi
  # the licence gate tools (alpha-check: cargo deny licenses, THIRD_PARTY_LICENSES.txt); both build from source, a few minutes the first time
  if [ "$CHECK" = 0 ]; then
    command -v cargo-deny >/dev/null 2>&1 || cargo install --locked cargo-deny || echo "setup-linux-host: could not install cargo-deny" >&2
    command -v cargo-about >/dev/null 2>&1 || cargo install --locked cargo-about --features cli || echo "setup-linux-host: could not install cargo-about" >&2   # the binary needs the cli feature
  fi
  for c in cargo-deny cargo-about; do command -v "$c" >/dev/null 2>&1 && say "$c" ok || bad "$c" "cargo install --locked $c$( [ "$c" = cargo-about ] && echo " --features cli")"; done
  say "rust toolchain" "$(rustc -V 2>/dev/null || echo none)"
  for t in $TARGETS; do rustup target list --installed | grep -qx "$t" && say "target $t" ok || bad "target $t" "rustup target add $t"; done
else
  bad "rustup" "install it from rustup.rs"
fi

# the checks: every program the tests start
for t in cc musl-gcc pkg-config git node python3 qemu-system-x86_64 qemu-img mcopy mdir mmd mtype mdel sfdisk sgdisk mkfs.vfat mksquashfs xorriso grub-mkimage sbsign fakeroot cpio zstd xz openssl \
         ssh ssh-keygen ssh-add ssh-agent sftp ip iw amixer arecord ps pgrep script jq debugfs cryptsetup losetup tesseract fsck.fat blkid nsenter unshare; do
  p="$(command -v "$t" 2>/dev/null || ls "/usr/sbin/$t" 2>/dev/null || true)"
  [ -n "$p" ] && say "$t" ok || bad "$t" "$t (apt package: see PKGS in this script)"
done
python3 -c "import PIL" 2>/dev/null && say "python3 Pillow" ok || bad "python3 Pillow" "python3-pil"
ls /usr/share/OVMF/OVMF_CODE_4M.fd >/dev/null 2>&1 && say "OVMF firmware" ok || bad "OVMF firmware" "ovmf"
# KVM makes the QEMU tests fast (and some time limits only hold with it); without it they fall back to software emulation
if [ -r /dev/kvm ] && [ -w /dev/kvm ]; then say "/dev/kvm" "ok"; else say "/dev/kvm" "not usable by $(id -un): QEMU tests will be slow (sudo usermod -aG kvm $(id -un), then log in again)"; fi
# a browser for the compat harness (optional)
command -v google-chrome >/dev/null 2>&1 || command -v chromium >/dev/null 2>&1 && say "browser (optional)" ok || say "browser (optional)" "none: the browser compat harness is skipped"
# GNU coreutils: Ubuntu 25.10+ ships uutils; the tests pass with it, the image scripts want GNU install (they put their own shim first on PATH)
inst="$(readlink -f "$(command -v install 2>/dev/null)" 2>/dev/null || true)"
case "$inst" in *uutils*|*coreutils/install*) say "install" "uutils (the release scripts use their GNU shim: fine)";; *) say "install" "ok";; esac
# disk and limits
free_gb="$(df -Pk . 2>/dev/null | awk 'NR==2 {print int($4/1048576)}')"
[ "${free_gb:-0}" -ge 40 ] && say "free disk here" "${free_gb} GB ok" || say "free disk here" "${free_gb:-?} GB: the release scripts refuse to start below 40 GB"
say "open files limit" "$(ulimit -n) (4096 or more is comfortable)"
[ -c /dev/ptmx ] && [ -d /dev/pts ] && say "pty devices" ok || bad "pty devices" "/dev/ptmx and /dev/pts (terminal tests need them)"
if [ "$fail" = 0 ]; then echo "setup-linux-host: this computer can run the gate (dist/release/test-with-rerun.sh OUT_DIR, with . dist/release/env.sh first)"; else echo "setup-linux-host: something is missing (see above)" >&2; fi
exit "$fail"
