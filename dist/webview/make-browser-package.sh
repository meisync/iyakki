#!/bin/sh
# Make the optional browser package (F-BROW-40) from the engine helper of a web image build:
#
#   sh dist/webview/make-browser-package.sh --version 2026.11-1 --key-file PATH --out DIR [--helper FILE] [--arch x86_64] [--iyakki PATH]
#
# The helper defaults to target/buildroot-web/images/browser-package/webview-helper (written by a build with the package option
# BR2_PACKAGE_IYAKKI_WEBVIEW_HELPER_PACKAGE_ONLY=y). The result is DIR/browser/<arch>/webview-helper and manifest.json, signed with the
# store key in PATH (64 hexadecimal characters; the file is read by `iyakki browser-package` and never printed). Nothing is uploaded: put
# DIR on the store yourself. The line it prints ends with the public key a person pins in Settings, Store.
set -eu
VERSION=""; KEY=""; OUT=""; HELPER=""; ARCH=$(uname -m); IYAKKI=iyakki
HERE=$(cd "$(dirname "$0")" && pwd)
while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION=$2; shift 2;;
    --key-file) KEY=$2; shift 2;;
    --out) OUT=$2; shift 2;;
    --helper) HELPER=$2; shift 2;;
    --arch) ARCH=$2; shift 2;;
    --iyakki) IYAKKI=$2; shift 2;;
    *) echo "unknown option $1" >&2; exit 2;;
  esac
done
[ -n "$VERSION" ] && [ -n "$KEY" ] && [ -n "$OUT" ] || { echo "usage: make-browser-package.sh --version V --key-file PATH --out DIR [--helper FILE] [--arch A] [--iyakki PATH]" >&2; exit 2; }
[ -n "$HELPER" ] || HELPER="$HERE/../../target/buildroot-web/images/browser-package/webview-helper"
[ -f "$HELPER" ] || { echo "no helper at $HELPER: build the web image with BR2_PACKAGE_IYAKKI_WEBVIEW_HELPER_PACKAGE_ONLY=y first (or pass --helper)" >&2; exit 2; }
exec "$IYAKKI" browser-package build --helper "$HELPER" --version "$VERSION" --key-file "$KEY" --out "$OUT" --arch "$ARCH"
