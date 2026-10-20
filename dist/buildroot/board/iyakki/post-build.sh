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
case "${IYAKKI_VARIANT:-}" in
  mac-*|macweb-*)
    sh "$(dirname "$0")/prune-firmware.sh" "$(dirname "$0")/mac-firmware.keep" "$TARGET_DIR/lib/firmware"
    ;;
esac

# Secure Boot (docs/SECURE-BOOT.md): with a signing key every kernel module in the image, the out-of-tree ones included (apple-bce in
# updates/), gets the owner's signature. The kernel build itself signs nothing (MODULE_SIG_ALL is off), so a build without a key
# leaves them unsigned and Secure Boot must be off. With Secure Boot on the kernel only loads modules signed by the enrolled key.
if [ -n "${IYAKKI_SB_KEY:-}" ] && [ -n "${IYAKKI_SB_CERT:-}" ]; then
  sh "$(dirname "$0")/../../secureboot.sh" sign-modules "$TARGET_DIR/lib/modules" || { echo "post-build: module signing failed" >&2; exit 1; }
  echo "SECURE_BOOT_CERT_SHA1=$(openssl x509 -in "$IYAKKI_SB_CERT" -noout -fingerprint -sha1 | sed 's/.*=//')" >> "$TARGET_DIR/etc/iyakki-release"
fi
