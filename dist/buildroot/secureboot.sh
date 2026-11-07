#!/bin/sh
# Secure Boot for IYAKKI images: shim + a Machine Owner Key (MOK). docs/SECURE-BOOT.md explains the design and the owner's steps.
#
#   secureboot.sh genkey DIR [COMMON-NAME]     make a signing key + certificate (DIR/iyakki-sb.key, .crt, .der). Run once, keep it safe.
#   secureboot.sh check                        say whether the tools and the key for a signed build are there (exit 0 = yes)
#   secureboot.sh fetch-shim OUTDIR            download the pinned, Microsoft-signed shim + MokManager from Debian, verify, unpack
#   secureboot.sh sign-pe IN OUT               sign an EFI program (GRUB, or the kernel's EFI stub) with the key
#   secureboot.sh sign-modules DIR [SIGN-FILE] sign every *.ko under DIR with the key (the kernel's scripts/sign-file)
#   secureboot.sh grub-image OUT               build the monolithic EFI GRUB with an SBAT section (Buildroot's own settings)
#   secureboot.sh loader-image IN OUT         the boot loader (iyakki-boot) for shim's second stage: add its SBAT section, then sign it with the key
#   secureboot.sh stage-esp ESPDIR GRUB-EFI    lay out ESPDIR/EFI/BOOT (shim as BOOTX64.EFI, signed GRUB, MokManager) and the
#                                              certificate in ESPDIR/EFI/IYAKKI/
#   secureboot.sh make-fat IMG ESPDIR          a FAT image holding ESPDIR (the ISO's EFI partition)
#   secureboot.sh image TREE ROOT              all of the above for an ISO tree and an install template (post-image.sh)
#   secureboot.sh verify ESPDIR [KERNEL]       check the signatures of what stage-esp wrote (and of KERNEL)
#
# The key never lives in the repository. The build reads it from two files:
#   IYAKKI_SB_KEY   private key, PEM        IYAKKI_SB_CERT   its certificate, PEM
# Without both, post-image.sh builds the plain, unsigned image exactly as before and says so. Tools used at build time only (never
# shipped): sbsign/sbverify (sbsigntool, GPL-3.0), openssl, the kernel's sign-file (GPL-2.0), grub-mkimage (GPL-3.0+), curl, dpkg-deb.
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
BOARD="${BOARD:-$HERE/board/iyakki}"

# ---- the pinned download ------------------------------------------------------------------------------------------------
# Debian 13 ("trixie") packages: shim 16.1, signed by Microsoft (UEFI CA 2011, and 2023 as the second signature) in shim-signed;
# MokManager (mmx64.efi) is signed with Debian's key, which that shim carries inside it, in shim-helpers-amd64-signed.
# The SHA-256 values are the ones in Debian's own Packages index (dists/trixie/main/binary-amd64/Packages.xz) on 2026-10-18, and
# the unpacked files are pinned again, so a changed package is noticed even if the download address stays the same.
DEB_MIRROR="${IYAKKI_SB_MIRROR:-https://deb.debian.org/debian}"
SHIM_DEB="pool/main/s/shim-signed/shim-signed_1.51~1+deb13u1+16.1-2~deb13u1_amd64.deb"
SHIM_DEB_SHA256="3c802fa303c0e6bf126adee74028d1042e3120360d159c2c05181dc9b2f61005"
HELP_DEB="pool/main/s/shim-helpers-amd64-signed/shim-helpers-amd64-signed_1+16.1+2~deb13u1_amd64.deb"
HELP_DEB_SHA256="7e950d75d5a40ecc0a99f727452ce35780b47a04d9a2cc3b1ff7c2bb316c033e"
SHIM_EFI_SHA256="e103c5d02657879f141b086ed703584d28bb9e2721b423333be085ee1b4d397b"
MM_EFI_SHA256="30ea13138310f0819c1d875fcf3a6cdce8a1607cfd7578053a2df894a41630ae"
CACHE="${IYAKKI_SB_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/iyakki-secureboot}"

die() { echo "secureboot: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "$1 is needed for a signed build ($2)"; }
sha_of() { sha256sum "$1" | cut -d' ' -f1; }
have_key() { [ -n "${IYAKKI_SB_KEY:-}" ] && [ -n "${IYAKKI_SB_CERT:-}" ] && [ -f "$IYAKKI_SB_KEY" ] && [ -f "$IYAKKI_SB_CERT" ]; }
need_key() { have_key || die "set IYAKKI_SB_KEY and IYAKKI_SB_CERT to the signing key and certificate (PEM files); make them with: $0 genkey DIR"; }

cmd_genkey() {
  dir="${1:?usage: genkey DIR [COMMON-NAME]}"; cn="${2:-IYAKKI Secure Boot signing key}"
  need openssl "install openssl"
  [ ! -e "$dir/iyakki-sb.key" ] || die "$dir/iyakki-sb.key exists already; refusing to replace a key"
  mkdir -p "$dir"; chmod 700 "$dir"
  cfg="$(mktemp)"
  cat > "$cfg" <<'EOF'
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = placeholder
[ext]
basicConstraints = critical,CA:FALSE
keyUsage = digitalSignature
# Code signing ONLY. Do not add the "kernel module signing" OID 1.3.6.1.4.1.2312.16.1.2 that some distributions use for module-only
# MOKs: shim refuses a MOK that has it for boot programs (GRUB, the kernel), and this one key signs both.
extendedKeyUsage = codeSigning
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid
EOF
  # RSA 2048: what shim, the kernel and every firmware accept. 10 years: the key is enrolled once and lives as long as the images.
  (umask 077; openssl req -x509 -newkey rsa:2048 -nodes -sha256 -days 3650 -config "$cfg" -subj "/CN=$cn/" \
      -keyout "$dir/iyakki-sb.key" -out "$dir/iyakki-sb.crt" 2>/dev/null)
  rm -f "$cfg"
  openssl x509 -in "$dir/iyakki-sb.crt" -outform DER -out "$dir/iyakki-sb.der"
  chmod 600 "$dir/iyakki-sb.key"
  echo "wrote $dir/iyakki-sb.key (SECRET: keep it off shared disks and out of git), iyakki-sb.crt, iyakki-sb.der"
  echo "build with: IYAKKI_SB_KEY=$dir/iyakki-sb.key IYAKKI_SB_CERT=$dir/iyakki-sb.crt ./build.sh ..."
}

cmd_check() {
  ok=0
  for t in sbsign sbverify openssl curl dpkg-deb sha256sum; do
    if command -v "$t" >/dev/null 2>&1; then echo "ok:      $t"; else echo "MISSING: $t"; ok=1; fi
  done
  if have_key; then echo "ok:      key $IYAKKI_SB_KEY and certificate $IYAKKI_SB_CERT"; else echo "MISSING: IYAKKI_SB_KEY / IYAKKI_SB_CERT (images are built unsigned)"; ok=1; fi
  exit $ok
}

cmd_fetch_shim() {
  out="${1:?usage: fetch-shim OUTDIR}"
  need curl "download tool"; need dpkg-deb "dpkg-deb unpacks the .deb; on a build host without it, 'ar x' + 'tar' do the same"; need sha256sum "coreutils"
  mkdir -p "$CACHE" "$out"
  tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
  for pair in "$SHIM_DEB $SHIM_DEB_SHA256" "$HELP_DEB $HELP_DEB_SHA256"; do
    set -- $pair; f="$CACHE/$(basename "$1")"
    if [ -f "$f" ] && [ "$(sha_of "$f")" = "$2" ]; then :; else
      echo "fetch-shim: downloading $1"
      curl -fsSL --retry 3 -o "$f.part" "$DEB_MIRROR/$1" || die "download of $DEB_MIRROR/$1 failed"
      [ "$(sha_of "$f.part")" = "$2" ] || { rm -f "$f.part"; die "$1 does not match its pinned SHA-256 $2 (a changed or damaged package; nothing was used)"; }
      mv "$f.part" "$f"
    fi
    dpkg-deb -x "$f" "$tmp"
  done
  cp "$tmp/usr/lib/shim/shimx64.efi.signed" "$out/shimx64.efi"
  cp "$tmp/usr/lib/shim/mmx64.efi.signed" "$out/mmx64.efi"
  [ "$(sha_of "$out/shimx64.efi")" = "$SHIM_EFI_SHA256" ] || die "shimx64.efi differs from the pinned file"
  if [ -n "$MM_EFI_SHA256" ]; then [ "$(sha_of "$out/mmx64.efi")" = "$MM_EFI_SHA256" ] || die "mmx64.efi differs from the pinned file"; fi
  # provenance for the legal-info: the packages' own copyright files, the package hashes and where they came from
  cp "$tmp/usr/share/doc/shim-signed/copyright" "$out/shim-signed.copyright"
  cp "$tmp/usr/share/doc/shim-helpers-amd64-signed/copyright" "$out/shim-helpers-amd64-signed.copyright"
  {
    echo "shim 16.1 (BSD-2-Clause), binary signed by Microsoft; MokManager signed by Debian. Not built by IYAKKI."
    echo "$DEB_MIRROR/$SHIM_DEB  sha256 $SHIM_DEB_SHA256"
    echo "$DEB_MIRROR/$HELP_DEB  sha256 $HELP_DEB_SHA256"
    echo "shimx64.efi sha256 $(sha_of "$out/shimx64.efi")"
    echo "mmx64.efi   sha256 $(sha_of "$out/mmx64.efi")"
    echo "Source: https://github.com/rhboot/shim (tag 16.1), Debian source package shim 16.1-2~deb13u1 (https://tracker.debian.org/pkg/shim)"
  } > "$out/PROVENANCE.txt"
  if command -v sbverify >/dev/null 2>&1; then
    sbverify --list "$out/shimx64.efi" 2>/dev/null | grep -q "Microsoft Corporation UEFI CA" || die "shimx64.efi carries no Microsoft signature; refusing it"
  fi
  echo "fetch-shim: shim and MokManager are in $out (SHA-256 checked)"
}

cmd_sign_pe() {
  in="${1:?usage: sign-pe IN OUT}"; out="${2:?usage: sign-pe IN OUT}"
  need_key; need sbsign "package sbsigntool (Debian, Ubuntu, Fedora: sbsigntools)"
  # a file that already carries a signature (a second run, a signed kernel) is signed again from its unsigned body: sbsign adds one
  sbsign --key "$IYAKKI_SB_KEY" --cert "$IYAKKI_SB_CERT" --output "$out" "$in" >/dev/null 2>"$out.err" || { cat "$out.err" >&2; rm -f "$out.err"; die "sbsign failed for $in"; }
  rm -f "$out.err"
}

# The own boot loader as shim's second stage (docs/development/GRUB-REPLACEMENT-PLAN.md): shim 15.3 and newer refuse a file without an .sbat
# section, so the section is added first (only our own component name: board/iyakki/loader-sbat.csv), then the file is signed with the same key
# as GRUB and the kernel. The loader starts the kernel with LoadImage and wraps the firmware's check so shim's MOK list counts (boot-uefi/src/shim.rs).
cmd_loader_image() {
  in="${1:?usage: loader-image IN OUT}"; out="${2:?usage: loader-image IN OUT}"
  need_key; [ -s "$in" ] || die "$in not found (build iyakki-boot first)"
  tool="$HERE/../../iyakki-boot/tools/add-sbat.sh"; [ -f "$tool" ] || die "$tool not found"
  tmp="$(mktemp)"; trap 'rm -f "$tmp"' EXIT
  rm -f "$tmp"; sh "$tool" "$in" "$tmp" "$BOARD/loader-sbat.csv" >&2 || die "adding the SBAT section failed"
  cmd_sign_pe "$tmp" "$out"; rm -f "$tmp"; trap - EXIT
  echo "loader-image: $out (SBAT iyakki.loader,1; signed with the image key)"
}

cmd_sign_modules() {
  dir="${1:?usage: sign-modules DIR [SIGN-FILE]}"; sf="${2:-${IYAKKI_SIGN_FILE:-}}"
  need_key
  if [ -z "$sf" ]; then
    for c in "${BUILD_DIR:-/nonexistent}"/linux-[0-9]*/scripts/sign-file /usr/src/linux-headers-*/scripts/sign-file; do [ -x "$c" ] && { sf="$c"; break; }; done
  fi
  [ -n "$sf" ] && [ -x "$sf" ] || die "the kernel's scripts/sign-file was not found (build the kernel with CONFIG_MODULE_SIG=y, or set IYAKKI_SIGN_FILE)"
  n=0
  # .ko only (the image uses uncompressed modules); one failure stops the build rather than leaving a half-signed image
  for m in $(find "$dir" -name '*.ko' -type f); do
    "$sf" sha256 "$IYAKKI_SB_KEY" "$IYAKKI_SB_CERT" "$m" || die "could not sign $m"
    n=$((n + 1))
  done
  echo "sign-modules: signed $n modules under $dir"
  find "$dir" \( -name '*.ko.zst' -o -name '*.ko.xz' -o -name '*.ko.gz' \) | grep -q . && die "compressed modules found under $dir: sign before compressing"
  return 0
}

cmd_grub_image() {
  out="${1:?usage: grub-image OUT}"
  mk="${HOST_DIR:+$HOST_DIR/bin/}grub-mkimage"; command -v "$mk" >/dev/null 2>&1 || [ -x "$mk" ] || mk="$(command -v grub-mkimage || true)"
  [ -n "$mk" ] || die "grub-mkimage not found"
  core="$(ls -d "${BUILD_DIR:-/nonexistent}"/grub2-*/build-x86_64-efi/grub-core 2>/dev/null | head -n1 || true)"
  core="${GRUB_CORE_DIR:-$core}"; [ -d "$core" ] || die "no x86_64-efi GRUB modules directory (set GRUB_CORE_DIR)"
  cfg="${GRUB_EFI_CONFIG:-$BOARD/grub-efi-builtin.cfg}"
  mods="${GRUB_EFI_MODULES:-}"
  if [ -z "$mods" ] && [ -f "${BR2_CONFIG:-/nonexistent}" ]; then mods="$(sed -n 's/^BR2_TARGET_GRUB2_BUILTIN_MODULES_EFI="\(.*\)"/\1/p' "$BR2_CONFIG")"; fi
  [ -n "$mods" ] || die "no GRUB module list (set GRUB_EFI_MODULES, or run under Buildroot)"
  sbat="$(mktemp)"
  # SBAT (https://github.com/rhboot/shim/blob/main/SBAT.md). Only our own component name: shim's revocation list names `grub`
  # (generation 5 and up), and Buildroot's GRUB 2.12 does not carry the fixes of that generation, so it must not claim to be it.
  # Shim still checks this section; what protects the machine is that only the owner's key signs the file (the MOK).
  cat > "$sbat" <<'EOF'
sbat,1,SBAT Version,sbat,1,https://github.com/rhboot/shim/blob/main/SBAT.md
grub.iyakki,1,IYAKKI,grub2,2.12,https://github.com/iyakki
EOF
  # Same call as Buildroot's (prefix /EFI/BOOT, the board's built-in config, the module list from the defconfig) plus --sbat.
  # No `shim_lock` module is needed: in GRUB 2.12 the shim_lock verifier is part of the kernel image of GRUB itself.
  # Every module the menu needs is built in, because GRUB refuses to load modules from disk when Secure Boot is on.
  # shellcheck disable=SC2086
  "$mk" -d "$core" -O x86_64-efi -o "$out" -p /EFI/BOOT -c "$cfg" --sbat "$sbat" $mods
  rm -f "$sbat"
}

cmd_stage_esp() {
  esp="${1:?usage: stage-esp ESPDIR GRUB-EFI}"; grub="${2:?usage: stage-esp ESPDIR GRUB-EFI}"
  need_key; [ -f "$grub" ] || die "$grub not found"
  work="$(mktemp -d)"
  sh "$0" fetch-shim "$work/shim" >&2
  mkdir -p "$esp/EFI/BOOT" "$esp/EFI/IYAKKI"
  rm -f "$esp/EFI/BOOT/bootx64.efi" "$esp/EFI/BOOT/BOOTX64.EFI"
  # Layout (removable-media path, the one every firmware and every USB stick uses):
  #   EFI/BOOT/BOOTX64.EFI  shim (Microsoft-signed): the firmware starts it, it checks what it starts next
  #   EFI/BOOT/grubx64.efi  our monolithic GRUB, signed with the owner's key: shim finds it next to itself and checks it against
  #                         the MOK list (the key the person enrolled)
  #   EFI/BOOT/mmx64.efi    MokManager: shim starts it by itself when the next step is not trusted yet (the blue key screen)
  # No fbx64.efi and no BOOTX64.CSV on purpose: those make shim write boot entries into the firmware's NVRAM; IYAKKI does not.
  cp "$work/shim/shimx64.efi" "$esp/EFI/BOOT/BOOTX64.EFI"
  cp "$work/shim/mmx64.efi" "$esp/EFI/BOOT/mmx64.efi"
  cmd_sign_pe "$grub" "$esp/EFI/BOOT/grubx64.efi"
  # the certificate MokManager enrols: "Enroll key from disk" -> EFI/IYAKKI/iyakki-mok.cer (DER)
  openssl x509 -in "$IYAKKI_SB_CERT" -outform DER -out "$esp/EFI/IYAKKI/iyakki-mok.cer"
  cp "$work/shim/PROVENANCE.txt" "$esp/EFI/IYAKKI/SHIM-PROVENANCE.txt"
  fp="$(openssl x509 -in "$IYAKKI_SB_CERT" -noout -fingerprint -sha1 | sed 's/.*=//')"
  cat > "$esp/EFI/IYAKKI/README-SECURE-BOOT.txt" <<EOF
IYAKKI and Secure Boot (one-time key enrolment)

The key to enrol has the SHA-1 fingerprint (MokManager shows it under "View key 0"):
 $fp

This disk starts with Secure Boot ON after you enrol the IYAKKI key once:
 1. Start from this disk with Secure Boot on. A "Verification failed: (0x1A) Security Violation" message comes first: press Enter,
    then press any key within 10 seconds when "Press any key to perform MOK management" shows.
 2. Choose "Enroll key from disk", then this disk, then EFI, IYAKKI, then iyakki-mok.cer. "View key 0" shows its fingerprint.
 3. Choose "Continue", then "Yes" (no password is asked for a key from disk), then "Reboot".
 4. IYAKKI starts. SYSTEM -> About shows "Secure Boot: ON" and "Kernel lockdown: integrity".
With Secure Boot OFF nothing of this is needed. More: docs/SECURE-BOOT.md.
shim and MokManager are not made by IYAKKI: see SHIM-PROVENANCE.txt.
EOF
  rm -rf "$work"
  echo "stage-esp: $esp/EFI/BOOT/{BOOTX64.EFI,grubx64.efi,mmx64.efi} and EFI/IYAKKI/iyakki-mok.cer written"
}

# A FAT image for the ISO's EFI partition (Buildroot's own is 1 MiB, too small for shim + MokManager + GRUB).
cmd_make_fat() {
  img="${1:?usage: make-fat IMG ESPDIR}"; esp="${2:?usage: make-fat IMG ESPDIR}"
  mkfs="${HOST_DIR:+$HOST_DIR/sbin/mkfs.vfat}"; [ -x "${mkfs:-/nonexistent}" ] || mkfs="$(command -v mkfs.vfat || true)"
  mcp="${HOST_DIR:+$HOST_DIR/bin/mcopy}"; [ -x "${mcp:-/nonexistent}" ] || mcp="$(command -v mcopy || true)"
  [ -n "$mkfs" ] && [ -n "$mcp" ] || die "mkfs.vfat and mcopy (dosfstools, mtools) are needed"
  kb=$(du -sk "$esp" | cut -f1); mb=$(( (kb + 1023) / 1024 + 3 ))
  rm -f "$img"; dd if=/dev/zero of="$img" bs=1M count="$mb" 2>/dev/null
  "$mkfs" "$img" >/dev/null
  "$mcp" -p -m -i "$img" -s "$esp"/* ::/
}

# Everything post-image.sh does for a signed image, in one place so it can be tried on copies.
#   image TREE ROOT : TREE is the ISO tree (boot/bzImage, boot/fat.efi), ROOT the install template (EFI/, boot/bzImage).
#   Either may be missing ("-").
cmd_image() {
  tree="${1:?usage: image TREE ROOT}"; root="${2:?usage: image TREE ROOT}"
  need_key; w="$(mktemp -d)"; trap 'rm -rf "$w"' EXIT
  cmd_grub_image "$w/grub.efi"
  if [ "$tree" != - ]; then
    cmd_sign_pe "$tree/boot/bzImage" "$w/bzImage"; cat "$w/bzImage" > "$tree/boot/bzImage"
    mkdir -p "$w/iso-esp"; cmd_stage_esp "$w/iso-esp" "$w/grub.efi"
    cmd_make_fat "$tree/boot/fat.efi" "$w/iso-esp"
  fi
  if [ "$root" != - ]; then
    cmd_sign_pe "$root/boot/bzImage" "$w/bzImage2"; cat "$w/bzImage2" > "$root/boot/bzImage"
    cmd_stage_esp "$root" "$w/grub.efi"
    cmd_verify "$root" "$root/boot/bzImage"
  fi
  echo "image: signed boot files written (key: $IYAKKI_SB_CERT)"
}

cmd_verify() {
  esp="${1:?usage: verify ESPDIR [KERNEL]}"; need sbverify "package sbsigntool"
  b="$esp/EFI/BOOT"
  for f in BOOTX64.EFI mmx64.efi grubx64.efi; do [ -f "$b/$f" ] || die "$b/$f is missing"; done
  [ -f "$esp/EFI/IYAKKI/iyakki-mok.cer" ] || die "EFI/IYAKKI/iyakki-mok.cer is missing"
  [ ! -e "$b/fbx64.efi" ] || die "fbx64.efi must not be on the disk (it writes firmware boot entries)"
  sbverify --list "$b/BOOTX64.EFI" 2>/dev/null | grep -q "Microsoft Corporation UEFI CA" || die "shim is not signed by Microsoft"
  certpem="$(mktemp)"; openssl x509 -inform DER -in "$esp/EFI/IYAKKI/iyakki-mok.cer" -out "$certpem"
  sbverify --cert "$certpem" "$b/grubx64.efi" >/dev/null || { rm -f "$certpem"; die "grubx64.efi is not signed by the certificate on the disk"; }
  if [ -n "${2:-}" ]; then sbverify --cert "$certpem" "$2" >/dev/null || { rm -f "$certpem"; die "$2 is not signed by the certificate on the disk"; }; fi
  rm -f "$certpem"
  echo "verify: shim signed by Microsoft; GRUB${2:+ and the kernel} signed by the certificate on the disk"
}

c="${1:-}"; [ $# -eq 0 ] || shift
case "$c" in
  genkey) cmd_genkey "$@" ;;
  check) cmd_check ;;
  fetch-shim) cmd_fetch_shim "$@" ;;
  sign-pe) cmd_sign_pe "$@" ;;
  loader-image) cmd_loader_image "$@" ;;
  sign-modules) cmd_sign_modules "$@" ;;
  grub-image) cmd_grub_image "$@" ;;
  stage-esp) cmd_stage_esp "$@" ;;
  make-fat) cmd_make_fat "$@" ;;
  image) cmd_image "$@" ;;
  verify) cmd_verify "$@" ;;
  *) sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'; [ -z "$c" ] || exit 2 ;;
esac
