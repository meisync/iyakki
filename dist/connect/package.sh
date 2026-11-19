#!/bin/bash
# Package the window program (iyakki-connect) for Linux or macOS: a tar.gz with the binary, a SHA256SUMS file and, when a signing key
# is given, a detached OpenSSH signature of SHA256SUMS (ssh-keygen -Y sign, namespace iyakki-connect). Nothing is uploaded or installed.
#   dist/connect/package.sh BINARY OUTDIR [VERSION]        key: IYAKKI_CONNECT_SIGN_KEY=/path/to/ed25519-private-key (kept outside the repository)
set -euo pipefail
bin=${1:?path to the iyakki-connect binary}; out=${2:?output folder}; ver=${3:-0.0.0}
os=$(uname -s | tr 'A-Z' 'a-z'); arch=$(uname -m)
name="iyakki-connect-$ver-$os-$arch"
mkdir -p "$out/$name"
install -m 755 "$bin" "$out/$name/iyakki-connect"
cp "$(dirname "$0")/../../LICENSE" "$out/$name/" 2>/dev/null || true
tar -C "$out" -czf "$out/$name.tar.gz" "$name"
( cd "$out" && { sha256sum "$name.tar.gz" 2>/dev/null || shasum -a 256 "$name.tar.gz"; } > "$name.SHA256SUMS" )
if [ -n "${IYAKKI_CONNECT_SIGN_KEY:-}" ]; then
  ssh-keygen -Y sign -f "$IYAKKI_CONNECT_SIGN_KEY" -n iyakki-connect "$out/$name.SHA256SUMS" >/dev/null
  echo "signed: $out/$name.SHA256SUMS.sig"
else
  echo "not signed (set IYAKKI_CONNECT_SIGN_KEY to sign)"
fi
echo "$out/$name.tar.gz"
