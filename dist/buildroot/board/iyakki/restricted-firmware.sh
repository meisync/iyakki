#!/bin/sh
# Restricted firmware in the image recipes (docs/RESTRICTED-FIRMWARE.md). Policy: firmware whose licence IYAKKI is not sure it may redistribute
# is NOT in the images it publishes; a person can build an image that includes it for their own use (not for redistribution by the project).
#
#   restricted-firmware.sh excludes          print '!path' lines (prune-firmware.sh keep-list format) for every file of the table
#   restricted-firmware.sh strip FW_DIR      delete the table's files from FW_DIR (an image's /lib/firmware); post-build.sh calls it for every variant
#   restricted-firmware.sh add TARGET_DIR    ONLY when IYAKKI_INCLUDE_RESTRICTED_FIRMWARE=1: put the files and their licence texts into the image
#                                            tree, each fetched from the pinned linux-firmware commit and checked for size and sha256, and stamp the
#                                            image ("includes third-party firmware under its own licences: not for redistribution by the IYAKKI project")
#   restricted-firmware.sh fetch DIR [PACK]  download and verify the files (all packs, or one: qualcomm-bt realtek-net broadcom-net atheros-carl9170)
#                                            into DIR, in the repository layout: the folder a USB stick needs for Store -> Setup -> "Install from a folder"
#   restricted-firmware.sh check             verify the table is well formed (used by the tests)
#
# Table: restricted-firmware.table (written by dist/release/pin-firmware.sh; the Setup tiles read the same file, crates/iyakki-agent/src/fwpack.rs).
# IYAKKI_RESTRICTED_FIRMWARE_DIR=DIR uses files already downloaded into DIR (repository layout) instead of the network: for offline builds and tests.
# Needs curl (unless that folder is given), awk, sha256sum, wc, install.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
TABLE="${IYAKKI_RESTRICTED_FIRMWARE_TABLE:-$HERE/restricted-firmware.table}"
[ -f "$TABLE" ] || { echo "restricted-firmware: no table $TABLE (run dist/release/pin-firmware.sh)" >&2; exit 1; }

# rows: KIND SIZE SHA256 PATH PACK (only 'file' and 'doc' lines), after checking the table
rows() {
  awk '
    /^[[:space:]]*#/ || /^[[:space:]]*$/ { next }
    $1 == "commit" { c = $2; next }
    $1 == "base" { next }
    $1 == "pack" { p = $2; next }
    $1 == "file" || $1 == "doc" { if (p == "" || $2 !~ /^[0-9]+$/ || length($3) != 64 || $3 !~ /^[0-9a-f]+$/ || $4 ~ /(^|\/)\.\.(\/|$)/ || $4 ~ /^\//) { print "bad table line: " $0 > "/dev/stderr"; bad = 1; exit 1 } print $1, $2, $3, $4, p; next }
    { print "unknown table line: " $0 > "/dev/stderr"; bad = 1; exit 1 }
    END { if (c !~ /^[0-9a-f]+$/ || length(c) != 40) { print "table has no 40-digit commit" > "/dev/stderr"; exit 1 } }
  ' "$TABLE"
}
commit() { awk '$1 == "commit" { print $2 }' "$TABLE"; }
base() { awk '$1 == "base" { print $2 }' "$TABLE"; }

# fetch_one PATH SIZE SHA256 DEST: copy or download, then check size and sha256; a wrong file is deleted and the build stops
fetch_one() {
  p="$1"; size="$2"; sha="$3"; dest="$4"
  mkdir -p "$(dirname "$dest")"
  if [ -n "${IYAKKI_RESTRICTED_FIRMWARE_DIR:-}" ]; then
    [ -f "$IYAKKI_RESTRICTED_FIRMWARE_DIR/$p" ] || { echo "restricted-firmware: $p is not in $IYAKKI_RESTRICTED_FIRMWARE_DIR" >&2; return 1; }
    cp "$IYAKKI_RESTRICTED_FIRMWARE_DIR/$p" "$dest"
  else
    command -v curl >/dev/null 2>&1 || { echo "restricted-firmware: curl is needed to download the files" >&2; return 1; }
    curl -fsS --retry 3 --max-time 120 -o "$dest" "$(base)/$p?id=$(commit)" || { rm -f "$dest"; echo "restricted-firmware: download of $p failed" >&2; return 1; }
  fi
  got_size="$(wc -c < "$dest" | tr -d ' ')"
  got_sha="$(sha256sum "$dest" | cut -d' ' -f1)"
  if [ "$got_size" != "$size" ] || [ "$got_sha" != "$sha" ]; then
    rm -f "$dest"
    echo "restricted-firmware: $p has $got_size bytes and sha256 $got_sha; the pinned table says $size and $sha. Deleted; nothing was added." >&2
    return 1
  fi
}

cmd="${1:-}"; [ $# -eq 0 ] || shift
case "$cmd" in
  check)
    n="$(rows | wc -l | tr -d ' ')"
    [ "$n" -ge 50 ] || { echo "restricted-firmware: the table has only $n rows" >&2; exit 1; }
    echo "restricted-firmware: table ok ($(rows | awk '$1 == "file"' | wc -l | tr -d ' ') files, $(rows | awk '$1 == "doc"' | wc -l | tr -d ' ') licence texts, commit $(commit))"
    ;;
  excludes)
    rows | awk '$1 == "file" { print "!" $4 }'
    ;;
  strip)
    FW="${1:?usage: restricted-firmware.sh strip FW_DIR}"
    [ -d "$FW" ] || exit 0
    if [ "${IYAKKI_INCLUDE_RESTRICTED_FIRMWARE:-0}" = 1 ]; then echo "restricted-firmware: IYAKKI_INCLUDE_RESTRICTED_FIRMWARE=1: nothing is stripped (this image is for the builder's own use)"; exit 0; fi
    removed=0
    for p in $(rows | awk '$1 == "file" { print $4 }'); do
      if [ -e "$FW/$p" ] || [ -L "$FW/$p" ]; then rm -f "$FW/$p"; removed=$((removed + 1)); fi
    done
    find "$FW" -depth -type d -empty -exec rmdir {} \; 2>/dev/null || true
    echo "restricted-firmware: $removed restricted file(s) removed from $FW"
    ;;
  fetch)
    DIR="${1:?usage: restricted-firmware.sh fetch DIR [PACK]}"; ONLY="${2:-}"
    [ -z "$ONLY" ] || rows | awk -v p="$ONLY" '$5 == p { f = 1 } END { exit f ? 0 : 1 }' || { echo "restricted-firmware: no pack called $ONLY" >&2; exit 2; }
    mkdir -p "$DIR"
    rows | while read -r kind size sha path pack; do
      [ -z "$ONLY" ] || [ "$pack" = "$ONLY" ] || continue
      fetch_one "$path" "$size" "$sha" "$DIR/$path"
    done
    echo "restricted-firmware: the verified files are in $DIR ($( [ -z "$ONLY" ] && echo all packs || echo "$ONLY" ))"
    ;;
  add)
    TARGET="${1:?usage: restricted-firmware.sh add TARGET_DIR}"
    [ "${IYAKKI_INCLUDE_RESTRICTED_FIRMWARE:-0}" = 1 ] || { echo "restricted-firmware: IYAKKI_INCLUDE_RESTRICTED_FIRMWARE is not 1: nothing is added (the default: the published images never include these files)"; exit 0; }
    WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
    n=0
    rows > "$WORK/rows"
    while read -r kind size sha path pack; do
      fetch_one "$path" "$size" "$sha" "$WORK/dl/$path"
      if [ "$kind" = file ]; then
        install -D -m 0644 "$WORK/dl/$path" "$TARGET/lib/firmware/$path"; n=$((n + 1))
      else
        install -D -m 0644 "$WORK/dl/$path" "$TARGET/usr/share/doc/linux-firmware-restricted/$pack/$(echo "$path" | tr '/' '-')"
      fi
    done < "$WORK/rows"
    # the licence texts also sit with the image's other firmware licences (firmware-licences.sh runs next and fills the rest)
    DOC="$TARGET/usr/share/doc/linux-firmware-restricted"
    cat > "$DOC/NOT-FOR-REDISTRIBUTION.txt" <<EOF
This image includes third-party firmware under its own licences ($n files from linux-firmware commit $(commit)):
Qualcomm Bluetooth, Realtek PCIe Ethernet, Broadcom Ethernet and Atheros AR9170 firmware. It was built by the person who has it, with
IYAKKI_INCLUDE_RESTRICTED_FIRMWARE=1, for their own use. It is NOT for redistribution by the IYAKKI project: the project does not publish
or distribute an image that includes these files. Whoever passes such an image on is responsible for the licences in this folder.
EOF
    # the stamp: /etc/iyakki-release (the About page reads it) and /etc/os-release
    if [ -f "$TARGET/etc/iyakki-release" ]; then
      grep -v '^RESTRICTED_FIRMWARE=' "$TARGET/etc/iyakki-release" > "$WORK/rel" || true
      echo "RESTRICTED_FIRMWARE=included" >> "$WORK/rel"
      cat "$WORK/rel" > "$TARGET/etc/iyakki-release"
    fi
    if [ -f "$TARGET/etc/os-release" ]; then
      grep -v '^IYAKKI_RESTRICTED_FIRMWARE=' "$TARGET/etc/os-release" > "$WORK/os" || true
      echo 'IYAKKI_RESTRICTED_FIRMWARE="includes third-party firmware under its own licences: not for redistribution by the IYAKKI project"' >> "$WORK/os"
      cat "$WORK/os" > "$TARGET/etc/os-release"
    fi
    echo "restricted-firmware: $n restricted file(s) and their licence texts added to $TARGET (image stamped: not for redistribution)"
    ;;
  *)
    echo "usage: restricted-firmware.sh excludes | strip FW_DIR | add TARGET_DIR | fetch DIR [PACK] | check" >&2
    exit 2
    ;;
esac
