#!/bin/sh
# Takes the builder's home folder out of a root file system tree: a Buildroot package compiled from the build tree records paths like
# /home/NAME/.../build/libpsl-0.21.5/... (assert and __FILE__ strings, configure results in text files such as icu's Makefile.inc), and a person's user
# name must not ship in an image (found 2026-11-13 by the image content gate: 8 files of the web image, libWPEWebKit, libpsl, sqlite3, icu, libxslt, ...).
#   scrub-paths.sh TARGET_DIR      (called at the end of post-build.sh and web-post-build.sh; running it twice is harmless)
# Every occurrence of $HOME in a regular file is overwritten, byte for byte, by the same number of slashes: "/home/NAME/x" becomes "//////////x", which is
# still a valid (if odd) path, so no string changes length and no binary needs relinking. This only touches strings that name the build machine; nothing that
# runs on the device reads them. Skipped when HOME is empty, "/" or shorter than 5 characters. IYAKKI_SCRUB_HOME=PATH scrubs that folder instead (tests).
set -eu
T="${1:?usage: scrub-paths.sh TARGET_DIR}"
H="${IYAKKI_SCRUB_HOME:-${HOME:-}}"
[ -d "$T" ] || { echo "scrub-paths: $T is not a folder" >&2; exit 2; }
[ "${#H}" -ge 5 ] || { echo "scrub-paths: HOME is too short to scrub safely; skipped" >&2; exit 0; }
list="$(mktemp)"
# regular files only (find -type f skips links); the list first, then the rewrite of just those files
find "$T" -type f -print0 | xargs -0 grep -lZaF -- "$H" > "$list" 2>/dev/null || true
n="$(tr -cd '\0' < "$list" | wc -c)"
if [ "$n" -gt 0 ]; then
  xargs -0 chmod u+w < "$list" 2>/dev/null || true
  H="$H" xargs -0 perl -pi -e 'BEGIN { $h = $ENV{H}; $r = "/" x length($h); } s/\Q$h\E/$r/g' < "$list"
fi
rm -f "$list"
echo "scrub-paths: $n file(s) had the build machine's home folder; replaced by slashes"
