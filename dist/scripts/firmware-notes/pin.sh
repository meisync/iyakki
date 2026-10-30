#!/usr/bin/env bash
# Writes the pinned manifest of the "Hardware firmware" Setup item:  dist/firmware/manifest.txt
#   dist/scripts/firmware-notes/pin.sh TAG COMMIT [TARBALL]        (needs curl, tar, awk, sha256sum; ~1 GB download unless TARBALL is given)
# Every value comes from the real snapshot published by kernel.org (never typed by hand):
#   snapshot  size + sha256 of https://cdn.kernel.org/pub/linux/kernel/firmware/linux-firmware-TAG.tar.gz
#   file      size, sha256, licence tokens (from WHENCE) and path of every file WHENCE names that is in the tree
#   link      every "Link: a -> b" of WHENCE and every symbolic link of the tree (path, target)
#   doc       size, sha256, path of every licence text (LICENSES/*, LICENSE, WHENCE)
# crates/iyakki-agent/src/fwextra.rs reads the result with include_str!; scripts/firmware-notes/fetch.sh reads it too (same lines).
# Nothing here is part of an image. Move the pin by running this with another TAG and COMMIT and reviewing the diff.
set -euo pipefail
TAG="${1:?usage: pin.sh TAG COMMIT [TARBALL]}"
COMMIT="${2:?usage: pin.sh TAG COMMIT [TARBALL]}"
case "$TAG" in *[!0-9]*|"") echo "pin: the tag is a date like 20261019" >&2; exit 2 ;; esac
case "$COMMIT" in *[!0-9a-f]*|"") echo "pin: the commit must be 40 hex digits" >&2; exit 2 ;; esac
[ "${#COMMIT}" = 40 ] || { echo "pin: the commit must be 40 hex digits" >&2; exit 2; }
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
OUT="$ROOT/dist/firmware/manifest.txt"
URL="https://cdn.kernel.org/pub/linux/kernel/firmware/linux-firmware-$TAG.tar.gz"
BASE="https://git.kernel.org/pub/scm/linux/kernel/git/firmware/linux-firmware.git/plain"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
TARBALL="${3:-$WORK/snapshot.tar.gz}"
[ -f "$TARBALL" ] || curl -fsS --retry 3 -o "$TARBALL" "$URL"
SNAP_SIZE="$(stat -c %s "$TARBALL")"
SNAP_SHA="$(sha256sum "$TARBALL" | cut -d' ' -f1)"
mkdir "$WORK/tree"
tar -xzf "$TARBALL" -C "$WORK/tree" --no-same-owner
T="$WORK/tree/linux-firmware-$TAG"
[ -f "$T/WHENCE" ] || { echo "pin: no WHENCE in the snapshot" >&2; exit 1; }

# WHENCE blocks: a Driver line, File/RawFile and Link lines, and one or more Licence lines (+ an SPDX line). A Licence line applies to the
# files and links listed since the previous Licence line of the block; files after the last Licence line of the block take that one.
awk '
  function tok(s,   t, out) {
    out = ""
    while (match(s, /(LICEN[CS]E|NOTICE|GPL)[A-Za-z0-9_.-]*/)) {
      t = substr(s, RSTART, RLENGTH); sub(/\.+$/, "", t)
      if (t != "LICENCE" && t != "LICENSE" && index("+" out "+", "+" t "+") == 0) out = (out == "" ? t : out "+" t)
      s = substr(s, RSTART + RLENGTH)
    }
    return out
  }
  function assign(t,   i) {
    if (t == "") t = "unspecified"
    for (i = 1; i <= nf; i++) print "F\t" t "\t" f[i]
    for (i = 1; i <= nl; i++) print "L\t" l[i]
    nf = 0; nl = 0; last = t
  }
  function settle() { if (waiting) { assign(""); waiting = 0 } }
  function end_block() { settle(); if (nf + nl > 0) assign(last) ; last = "" }
  /^Driver: / { end_block(); next }
  /^(File|RawFile): / { settle(); f[++nf] = $2; next }
  /^Link: / { settle(); s = $0; sub(/^Link: */, "", s); l[++nl] = s; next }
  /^Licen[cs]e: / { settle(); t = tok($0); if (t != "") assign(t); else waiting = 1; next }
  /^SPDX-License-Identifier: / { if (waiting) { sp = $0; sub(/^SPDX-License-Identifier: */, "", sp); sub(/^LicenseRef-/, "", sp); gsub(/ /, "_", sp); assign(sp); waiting = 0 } next }
  END { end_block() }
' "$T/WHENCE" > "$WORK/whence.tsv"

cd "$T"
{
  echo "# Pinned linux-firmware snapshot for the Hardware firmware Setup item (made by dist/scripts/firmware-notes/pin.sh; do not edit by hand)."
  echo "# Lines: tag | commit | base | snapshot <size> <sha256> <url> | doc <size> <sha256> <path> | file <size> <sha256> <licence-tokens> <path> | link <path> <target>"
  echo "tag $TAG"
  echo "commit $COMMIT"
  echo "base $BASE"
  echo "snapshot $SNAP_SIZE $SNAP_SHA $URL"
  { echo WHENCE; echo LICENSE; find LICENSES -type f; } | LC_ALL=C sort | while IFS= read -r p; do
    [ -f "$p" ] && printf 'doc %s %s %s\n' "$(stat -c %s "$p")" "$(sha256sum "$p" | cut -d' ' -f1)" "$p"
  done
  awk -F'\t' '$1=="F"{print $2 "\t" $3}' "$WORK/whence.tsv" | LC_ALL=C sort -s -t$'\t' -k2,2 -u | while IFS=$'\t' read -r tokens p; do
    if [ -f "$p" ] && [ ! -L "$p" ]; then
      case "$p" in *[[:space:]]*) echo "pin: a name with a space: $p" >&2; exit 1 ;; esac
      printf 'file %s %s %s %s\n' "$(stat -c %s "$p")" "$(sha256sum "$p" | cut -d' ' -f1)" "$tokens" "$p"
    fi
  done
  {
    # links of WHENCE: "a -> b", b relative to the folder of a
    awk -F'\t' '$1=="L"{print $2}' "$WORK/whence.tsv" | sed -n 's/^\(.*[^ ]\) *-> *\(.*\)$/\1 \2/p'
    # links in the tree
    find . -type l -printf '%P %l\n'
  } | awk 'NF==2' | LC_ALL=C sort -u | while read -r a b; do printf 'link %s %s\n' "$a" "$b"; done
} > "$OUT.tmp"
n=$(grep -c '^file ' "$OUT.tmp")
[ "$n" -ge 1000 ] || { echo "pin: only $n files; the WHENCE format may have changed" >&2; exit 1; }
mv "$OUT.tmp" "$OUT"
echo "wrote $OUT: $n files, $(grep -c '^link ' "$OUT") links, $(grep -c '^doc ' "$OUT") licence texts; snapshot sha256 $SNAP_SHA ($SNAP_SIZE bytes)"
