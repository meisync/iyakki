#!/usr/bin/env bash
# ./run.sh release [version] [--with-web] [--with-mac] [--with-macweb] [--no-window] [--allow-stale] [--dry-run]
# Collects the built images and the window-mode tarball in target/release-<version>/, adds the release documents, sums and
# signs everything (SHA256SUMS + .sig, release.json + .sig, index.html) with the release key, and checks the result.
# Nothing is uploaded or published; this only prepares the folder.
#   --with-web      also include the optional web image (target/buildroot-web) as iyakki-web-*; it is big (about 2 GB)
#   --with-mac      also include the optional Mac image (target/buildroot-mac) as iyakki-mac-*
#   --with-macweb   also include the optional Mac + web image (target/buildroot-macweb) as iyakki-macweb-*; big (about 2 GB)
#   --no-window     skip the Linux x86_64 window-mode tarball (it needs a release build of crates/iyakki-gui)
#   --no-store-server  skip the static private store servers iyakki-store-server-x86_64 / -aarch64 (dist/store-server/build-static.sh;
#                   a build that cannot run here is skipped with a warning, it never stops the release)
#   --allow-stale   accept images older than the last change of Cargo.toml (they then carry the old version in /etc/iyakki-release)
#   --dry-run       show what would be collected; write nothing, sign nothing
# The signing key lives in ~/.config/iyakki/release.key. It is made here the first time; the public half is printed and the
# secret half is never printed. BACK THE KEY UP: a release cannot be re-signed without it.
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/env.sh"
cd "$ROOT"
VER=""; WEB=0; MAC=0; MACWEB=0; WIN=1; STALE=0; DRY=0; SRV=1
for a in "$@"; do case "$a" in
  --with-web) WEB=1 ;; --with-mac) MAC=1 ;; --with-macweb) MACWEB=1 ;; --no-window) WIN=0 ;; --no-store-server) SRV=0 ;; --allow-stale) STALE=1 ;; --dry-run) DRY=1 ;;
  -h|--help) sed -n 2,17p "$0"; exit 0 ;;
  -*) echo "unknown option $a" >&2; exit 2 ;;
  *) VER="$a" ;; esac; done
CARGO_VER="$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n1)"
VER="${VER:-$CARGO_VER}"
[ "$VER" = "$CARGO_VER" ] || { echo "release: version $VER differs from Cargo.toml ($CARGO_VER). Bump the workspace first (docs/RELEASING.md)." >&2; exit 1; }
DEST="$ROOT/target/release-$VER"
KEY="$HOME/.config/iyakki/release.key"
trap iyakki_tmp_clean EXIT

# name in the release <- built file
declare -a MAP=(
  "iyakki-x86_64.iso|$ROOT/target/buildroot-libre/images/iyakki-x86_64.iso"
  "iyakki-install-x86_64.img|$ROOT/target/buildroot-libre/images/iyakki-install-x86_64.img"
  "iyakki-aarch64.img|$ROOT/target/buildroot-libre-aarch64/images/iyakki-aarch64.img"
)
[ "$WEB" = 1 ] && MAP+=("iyakki-web-x86_64.iso|$ROOT/target/buildroot-web/images/iyakki-x86_64.iso" "iyakki-web-install-x86_64.img|$ROOT/target/buildroot-web/images/iyakki-install-x86_64.img")
[ "$MAC" = 1 ] && MAP+=("iyakki-mac-x86_64.iso|$ROOT/target/buildroot-mac/images/iyakki-x86_64.iso" "iyakki-mac-install-x86_64.img|$ROOT/target/buildroot-mac/images/iyakki-install-x86_64.img")

[ "$MACWEB" = 1 ] && MAP+=("iyakki-macweb-x86_64.iso|$ROOT/target/buildroot-macweb/images/iyakki-x86_64.iso" "iyakki-macweb-install-x86_64.img|$ROOT/target/buildroot-macweb/images/iyakki-install-x86_64.img")

found=0
echo "Release $VER -> $DEST"
for m in "${MAP[@]}"; do
  name="${m%%|*}"; src="${m#*|}"
  if [ -f "$src" ]; then
    note=""; if [ "$src" -ot Cargo.toml ]; then note="  (OLDER than Cargo.toml: built before the last version change)"; [ "$STALE" = 1 ] || { echo "  STALE  $name$note" >&2; echo "release: rebuild it (./run.sh build) or pass --allow-stale." >&2; exit 1; }; fi
    echo "  image  $name  $(du -h "$src" | cut -f1)$note"; found=$((found+1))
  else
    echo "  absent $name  (not built: $src)"
  fi
done
[ "$WIN" = 1 ] && echo "  window iyakki-$VER-linux-x86_64-window.tar.gz  (built here from crates/iyakki-gui)"
[ "$SRV" = 1 ] && echo "  server iyakki-store-server-x86_64 and -aarch64  (static musl, built here with dist/store-server/build-static.sh)"
echo "  macOS and Windows window builds: not in this alpha"
[ "$found" -gt 0 ] || { echo "release: no images to release. Build them first: ./run.sh build  (and ./run.sh build-arm; ./run.sh build-web for the web image, ./run.sh build-macweb for Intel Macs that need the full web engine)" >&2; exit 1; }
[ "$DRY" = 1 ] && { echo "(dry run: nothing written)"; exit 0; }

mkdir -p "$DEST" "$HOME/.config/iyakki"
chmod 700 "$HOME/.config/iyakki"
# a fresh folder: previous files of the same release are replaced, nothing else is touched
for m in "${MAP[@]}"; do rm -f "$DEST/${m%%|*}"; done
for m in "${MAP[@]}"; do
  name="${m%%|*}"; src="${m#*|}"
  [ -f "$src" ] || continue
  ln -f "$src" "$DEST/$name" 2>/dev/null || cp "$src" "$DEST/$name"
done

# window-mode tarball (Linux x86_64)
if [ "$WIN" = 1 ]; then
  cargo build --release --locked --manifest-path crates/iyakki-gui/Cargo.toml --bin iyakki-desktop
  STAGE="$TMPDIR/iyakki-stage-$$/iyakki-$VER-linux-x86_64-window"
  rm -rf "$TMPDIR/iyakki-stage-$$"; mkdir -p "$STAGE"
  install -m 0755 crates/iyakki-gui/target/release/iyakki-desktop "$STAGE/iyakki-desktop"
  install -m 0755 dist/release/run-window.sh "$STAGE/run-window.sh"
  install -m 0644 LICENSE NOTICE THIRD_PARTY_LICENSES.txt "$STAGE/"
  install -m 0644 dist/release/README-WINDOW.md "$STAGE/README-WINDOW.md"
  tar -C "$(dirname "$STAGE")" -czf "$DEST/iyakki-$VER-linux-x86_64-window.tar.gz" "$(basename "$STAGE")"
  rm -rf "$TMPDIR/iyakki-stage-$$"
fi

# the private store server (one small static file per CPU): the wizard and the Dockerfile route use it, Buildroot ships it in the images
if [ "$SRV" = 1 ]; then
  for A in x86_64 aarch64; do
    rm -f "$DEST/iyakki-store-server-$A"
    if OUTBIN="$(dist/store-server/build-static.sh "$A" 2>"$TMPDIR/iyakki-store-server-$A.log" | tail -n1)" && [ -f "$OUTBIN" ]; then
      install -m 0755 "$OUTBIN" "$DEST/iyakki-store-server-$A"
      echo "  server iyakki-store-server-$A  $(du -h "$DEST/iyakki-store-server-$A" | cut -f1)"
    else
      echo "  WARNING: iyakki-store-server-$A was not built ($(tail -n1 "$TMPDIR/iyakki-store-server-$A.log" 2>/dev/null)); the release goes without it" >&2
    fi
  done
fi

# the documents travel with the release (they are signed with it)
for f in "RELEASE-NOTES-$VER.md" CHANGELOG.md KNOWN-ISSUES.md SECURITY.md SBOM.json LICENSE NOTICE; do
  [ -f "$f" ] && cp "$f" "$DEST/$f" || echo "  note: $f not found, not included"
done

cargo build -q --locked --release -p iyakki-core
IY="$ROOT/target/release/iyakki"
if [ ! -f "$KEY" ]; then
  "$IY" release keygen "$KEY" | grep -v '^secret key' || true
  echo
  echo "A new release signing key was made: $KEY   (mode 0600)"
  echo "  >>> BACK IT UP NOW (offline, encrypted). Without it you cannot sign the next release with the same identity."
  echo "  >>> Publish the public key above in the README and on the download page so people can compare it."
fi
KERNEL="$(sed -n 's/^BR2_LINUX_KERNEL_CUSTOM_VERSION_VALUE="\(.*\)"/\1/p' dist/buildroot/configs/iyakki_x86_64_defconfig | head -n1)"
COMMIT="$(git rev-parse HEAD 2>/dev/null || true)"; DESCRIBE="$(git describe --tags --always --dirty 2>/dev/null || true)"
[ -n "$COMMIT" ] && [ -n "$(git status --porcelain 2>/dev/null | head -n1)" ] && echo "  note: the tree has uncommitted changes; release.json records $COMMIT with describe '$DESCRIBE'"
SIGN_OUT="$("$IY" release sign "$DEST" --key "$KEY" --title "IYAKKI" --version "$VER" --git-commit "$COMMIT" --git-describe "$DESCRIBE" --kernel "$KERNEL" --date "$(date -u +%F)")"
echo "$SIGN_OUT" | sed 's/^/  /'
PUB="$(echo "$SIGN_OUT" | sed -n 's/.*public key \([0-9a-f]\{64\}\).*/\1/p' | tail -n1)"
echo
echo "Public signing key (ed25519): $PUB"
echo "Checking what was just written ..."
"$IY" release verify "$DEST" --pubkey "$PUB"
( cd "$DEST" && sha256sum -c SHA256SUMS )
echo
echo "Release folder: $DEST"
echo "Upload everything in it. People verify with:"
echo "  sha256sum -c SHA256SUMS --ignore-missing && iyakki release verify . --pubkey $PUB"
echo "  (iyakki-usb checks the image against release.json next to it before it writes a stick)"
