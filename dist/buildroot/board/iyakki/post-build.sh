#!/bin/sh
# Buildroot post-build hook: $1 = target rootfs dir. Installs the cargo-built static binary as PID 1.
set -eu
TARGET_DIR="$1"
BIN="${IYAKKI_BIN:?IYAKKI_BIN must point to the static musl iyakki binary (build.sh sets it)}"
install -D -m 0755 "$BIN" "$TARGET_DIR/usr/bin/iyakki"
ln -sf ../usr/bin/iyakki "$TARGET_DIR/sbin/init"
mkdir -p "$TARGET_DIR"/proc "$TARGET_DIR"/sys "$TARGET_DIR"/dev "$TARGET_DIR"/run "$TARGET_DIR"/tmp \
         "$TARGET_DIR"/etc/iyakki "$TARGET_DIR"/var/lib/iyakki

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
