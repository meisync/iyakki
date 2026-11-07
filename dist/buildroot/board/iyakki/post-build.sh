#!/bin/sh
# Buildroot post-build hook: $1 = target rootfs dir. Installs the cargo-built static binary as PID 1.
set -eu
TARGET_DIR="$1"
BIN="${IYAKKI_BIN:?IYAKKI_BIN must point to the static musl iyakki binary (build.sh sets it)}"
install -D -m 0755 "$BIN" "$TARGET_DIR/usr/bin/iyakki"
ln -sf ../usr/bin/iyakki "$TARGET_DIR/sbin/init"

# The private store server (one small static file for this image's CPU): the Private store wizard finds it here (docs/STORE.md, section 13.1).
# Not built, not shipped: skipped with a warning, the image build is not stopped.
if [ -n "${IYAKKI_STORE_SERVER_BIN:-}" ] && [ -f "$IYAKKI_STORE_SERVER_BIN" ]; then
  install -D -m 0755 "$IYAKKI_STORE_SERVER_BIN" "$TARGET_DIR/usr/share/iyakki/store-server/${IYAKKI_ARCH:-$(uname -m)}/iyakki-store-server"
else
  echo "post-build: WARNING: no iyakki-store-server to ship (IYAKKI_STORE_SERVER_BIN unset or missing); the Private store wizard will build or ask for one" >&2
fi
mkdir -p "$TARGET_DIR"/proc "$TARGET_DIR"/sys "$TARGET_DIR"/dev "$TARGET_DIR"/run "$TARGET_DIR"/tmp \
         "$TARGET_DIR"/etc/iyakki "$TARGET_DIR"/var/lib/iyakki

# What this image is (SYSTEM -> About, `iyakki --version`): version, build date, git describe, variant, kernel. build.sh exports the
# values; a missing one is written as "unknown" so the file always has all five keys.
KVER="$(sed -n 's/^BR2_LINUX_KERNEL_CUSTOM_VERSION_VALUE="\(.*\)"/\1/p' "${BR2_CONFIG:-/nonexistent}" 2>/dev/null | head -n1)"
[ -n "$KVER" ] || KVER="$(sed -n 's/^BR2_LINUX_KERNEL_VERSION="\(.*\)"/\1/p' "${BR2_CONFIG:-/nonexistent}" 2>/dev/null | head -n1)"
cat > "$TARGET_DIR/etc/iyakki-release" <<EOF
VERSION=${IYAKKI_VERSION:-unknown}
BUILD_DATE=${IYAKKI_BUILD_DATE:-$(date -u +%Y-%m-%d)}
GIT_DESCRIBE=${IYAKKI_GIT_DESCRIBE:-unknown}
VARIANT=${IYAKKI_VARIANT:-unknown}
KERNEL=${KVER:-${IYAKKI_KERNEL:-unknown}}
EOF
chmod 0644 "$TARGET_DIR/etc/iyakki-release"

# The SSH client of the CLI tile (docs/CLI-SSH.md) is Dropbear's: ssh (a link to dbclient), scp and dropbearkey must be in every image.
for p in ssh scp dbclient dropbearkey; do
  [ -e "$TARGET_DIR/usr/bin/$p" ] || { echo "post-build: /usr/bin/$p is missing; BR2_PACKAGE_DROPBEAR_CLIENT must stay on" >&2; exit 1; }
done

# Remote diagnostics (boot entry "iyakki.ssh=1"): only these public keys may log in. Nothing is installed when unset.
if [ -n "${IYAKKI_SSH_AUTHORIZED_KEYS:-}" ] && [ -f "$IYAKKI_SSH_AUTHORIZED_KEYS" ]; then
  install -D -m 0644 "$IYAKKI_SSH_AUTHORIZED_KEYS" "$TARGET_DIR/etc/iyakki/authorized_keys"
fi

# Sample media (CC0, made by dist/samples/make-samples.py): read-only for MEDIA in every image, listed by MEDIA as "IYAKKI samples".
# dist/samples/ is the single copy in the repository; this step puts it in the root file system of the default and the web image.
SAMPLES_SRC="${BR2_EXTERNAL_IYAKKI_PATH}/../samples"
SAMPLES_DST="$TARGET_DIR/usr/share/iyakki/samples"
for f in "Music/IYAKKI Sample Melody.mp3" "Music/IYAKKI Sample Beats.mp3" "Videos/IYAKKI Sample Video.webm"; do
  [ -f "$SAMPLES_SRC/$f" ] || { echo "post-build: missing sample $SAMPLES_SRC/$f" >&2; exit 1; }
  install -D -m 0644 "$SAMPLES_SRC/$f" "$SAMPLES_DST/$f"
done
install -m 0644 "$SAMPLES_SRC/README.md" "$SAMPLES_SRC/LICENSE" "$SAMPLES_DST/"
# (kept writable by mode so that Buildroot can clean its copy as a normal user; "read-only" is what MEDIA does with the folder)

# Attribution: our license, notices, and the licenses of the statically linked Rust crates.
DOC="$TARGET_DIR/usr/share/doc/iyakki"
REPO="${BR2_EXTERNAL_IYAKKI_PATH}/../.."
mkdir -p "$DOC"
for f in LICENSE NOTICE LICENSING.md THIRD_PARTY_LICENSES.txt; do
  install -m 0644 "$REPO/$f" "$DOC/$f"
done

# Mac images (mac, macweb): keep only the firmware Macs and the documented USB Wi-Fi adapters need (board/iyakki/mac-firmware.keep).
# GRUB reads the initramfs through the Mac firmware's slow USB driver, so this cuts the silent wait at the logo by about half
# (docs/MAC-HARDWARE.md, "Boot time"). The hw image keeps the whole set.
# RELEASE image profile (IYAKKI_IMAGE_PROFILE=release, repack.sh / build.sh set it; docs/development/RELEASE-IMAGE-PROFILE.md): the base
# image keeps no vendor firmware at all, so the mac and pc keep lists below are NOT used; release-firmware.keep is (default: only the
# wireless-regdb files). The person installs what the computer asks for with the "Hardware firmware" item.
REL=0; [ "${IYAKKI_IMAGE_PROFILE:-}" = release ] && REL=1
if [ "$REL" = 0 ]; then
case "${IYAKKI_VARIANT:-}" in
  mac-*|macweb-*)
    sh "$(dirname "$0")/prune-firmware.sh" "$(dirname "$0")/mac-firmware.keep" "$TARGET_DIR/lib/firmware"
    ;;
esac
fi

# The PC image (variant "pc", made by repack.sh from the macweb recipe, docs/GPU-K3S.md): repack.sh has put the whole linux-firmware set in
# /lib/firmware; keep the GPUs (NVIDIA GSP, all AMD, all Intel), the Wi-Fi, Bluetooth and Ethernet chips of ordinary PCs
# (board/iyakki/pc-firmware.keep) and drop the rest. The Apple-only drivers of the Mac kernel stay as modules: they bind to nothing on a PC.
if [ "$REL" = 0 ]; then
case "${IYAKKI_VARIANT:-}" in
  pc-*)
    # the Intel Wi-Fi files are chosen by what this kernel's iwlwifi asks for (iwlwifi-keep.sh), the rest by the static list
    PCKEEP="$(mktemp)"
    cat "$(dirname "$0")/pc-firmware.keep" > "$PCKEEP"
    sh "$(dirname "$0")/iwlwifi-keep.sh" "$TARGET_DIR/lib/modules" "$TARGET_DIR/lib/firmware" >> "$PCKEEP"
    sh "$(dirname "$0")/prune-firmware.sh" "$PCKEEP" "$TARGET_DIR/lib/firmware"
    rm -f "$PCKEEP"
    ;;
esac
fi
if [ "$REL" = 1 ]; then
  sh "$(dirname "$0")/release-profile.sh" apply "$TARGET_DIR"
fi

# Restricted firmware (docs/RESTRICTED-FIRMWARE.md, docs/FIRMWARE-LICENCES.md): files whose licence IYAKKI is not sure it may redistribute are removed from
# EVERY variant (hw, mac, macweb, pc: the keep lists also exclude them, this catches what a package or a repack brought in). The person installs them from
# Store -> Setup -> Hardware. With IYAKKI_INCLUDE_RESTRICTED_FIRMWARE=1 (a build for the builder's own use) nothing is stripped, the pinned files are added
# after a sha256 check, and the image says "not for redistribution by the IYAKKI project" (restricted-firmware.sh add).
sh "$(dirname "$0")/restricted-firmware.sh" strip "$TARGET_DIR/lib/firmware"
sh "$(dirname "$0")/restricted-firmware.sh" add "$TARGET_DIR"

# Firmware licence texts: every image that carries linux-firmware files also carries the licences that allow redistributing them
# (board/iyakki/firmware-licences.sh, LICENSING.md item 7). The source tree is Buildroot's build dir, or the donor's when repack.sh assembled the image.
sh "$(dirname "$0")/firmware-licences.sh" "$TARGET_DIR" ${FWFROM:+"$FWFROM"} ${BUILD_DIR:+"$BUILD_DIR"/linux-firmware-*} ${KFROM:+"$KFROM"/build/linux-firmware-*}

# Releasable flag (machine-readable, shown in About): a release image was stamped IMAGE_PROFILE=release / FIRMWARE=none by release-profile.sh
# above; every other image is a development/test image and says so, with FIRMWARE=bundled when vendor files are inside /lib/firmware.
if [ "$REL" = 0 ]; then
  REL_FILE="$TARGET_DIR/etc/iyakki-release"
  grep -v -e '^IMAGE_PROFILE=' -e '^FIRMWARE=' "$REL_FILE" > "$REL_FILE.new" || true
  echo "IMAGE_PROFILE=dev" >> "$REL_FILE.new"
  if [ -d "$TARGET_DIR/lib/firmware" ] && find "$TARGET_DIR/lib/firmware" \( -type f -o -type l \) 2>/dev/null | grep -v -e '/regulatory\.db$' -e '/regulatory\.db\.p7s$' | grep -q .; then
    echo "FIRMWARE=bundled" >> "$REL_FILE.new"
  else
    echo "FIRMWARE=none" >> "$REL_FILE.new"
  fi
  cat "$REL_FILE.new" > "$REL_FILE"; rm -f "$REL_FILE.new"; chmod 0644 "$REL_FILE"
fi

# Secure Boot (docs/SECURE-BOOT.md): with a signing key every kernel module in the image, the out-of-tree ones included (apple-bce in
# updates/), gets the owner's signature. The kernel build itself signs nothing (MODULE_SIG_ALL is off), so a build without a key
# leaves them unsigned and Secure Boot must be off. With Secure Boot on the kernel only loads modules signed by the enrolled key.
if [ -n "${IYAKKI_SB_KEY:-}" ] && [ -n "${IYAKKI_SB_CERT:-}" ]; then
  sh "$(dirname "$0")/../../secureboot.sh" sign-modules "$TARGET_DIR/lib/modules" || { echo "post-build: module signing failed" >&2; exit 1; }
  echo "SECURE_BOOT_CERT_SHA1=$(openssl x509 -in "$IYAKKI_SB_CERT" -noout -fingerprint -sha1 | sed 's/.*=//')" >> "$TARGET_DIR/etc/iyakki-release"
fi
