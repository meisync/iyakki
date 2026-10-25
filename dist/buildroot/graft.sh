#!/bin/sh
# Put the files of a graft list from a finished base build's target/ into an unpacked root file system (used by repack.sh --graft,
# docs/BUILD-INCREMENTAL.md). Nothing here runs Buildroot. Hard links stay hard links (git's 150 commands are one file).
#
#   graft.sh <name> <SRC_TARGET> <DEST_ROOT> [--dry-run]
#
# <name> is a file graft/<name>.list next to this script. Every file is copied with its mode; a symbolic link stays a link; files
# that begin with a perl or python shebang are skipped (the image has neither). Afterwards each program that was copied is checked:
# its shared libraries (readelf -d, when a readelf is found) must be in DEST_ROOT, or the graft fails and says which one is missing.
# The list of copied files is printed (one per line, "graft: +path") and a size line at the end.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
NAME="${1:?graft name}"; SRC="${2:?source target dir}"; DEST="${3:?destination root}"; DRY="${4:-}"
LIST="$HERE/graft/$NAME.list"
[ -f "$LIST" ] || { echo "graft: no list $LIST" >&2; exit 2; }
[ -d "$SRC" ] || { echo "graft: no source folder $SRC" >&2; exit 2; }
[ -d "$DEST" ] || { echo "graft: no destination folder $DEST" >&2; exit 2; }
W="$(mktemp -d "${TMPDIR:-/tmp}/iyakki-graft.XXXXXX")"; trap 'rm -rf "$W"' EXIT
: > "$W/files"
cd "$SRC"
while IFS= read -r line; do
  case "$line" in ''|'#'*) continue ;; esac
  case "$line" in
    -*) pat="${line#-}"
        # shellcheck disable=SC2086
        for m in $pat; do [ -e "$m" ] || [ -L "$m" ] || continue; awk -v p="$m" '$0 != p && index($0, p "/") != 1' "$W/files" > "$W/files.new" || true; mv "$W/files.new" "$W/files"; done ;;
    *)  found=0
        # shellcheck disable=SC2086
        for m in $line; do
          [ -e "$m" ] || [ -L "$m" ] || continue
          found=1
          if [ -d "$m" ] && [ ! -L "$m" ]; then find "$m" \( -type f -o -type l \) | LC_ALL=C sort >> "$W/files"; else echo "$m" >> "$W/files"; fi
        done
        [ "$found" = 1 ] || { echo "graft: $NAME: nothing matches '$line' in $SRC (was the package built? make -C <base> <package>)" >&2; exit 1; } ;;
  esac
done < "$LIST"
LC_ALL=C sort -u "$W/files" > "$W/files.u"
: > "$W/copy"
while IFS= read -r f; do
  if [ -f "$f" ] && [ ! -L "$f" ]; then
    first="$(head -c 64 "$f" | head -n1 | tr -d '\000')"
    case "$first" in '#!'*perl*|'#!'*python*) echo "graft: skipped $f (needs $(echo "$first" | sed 's/.*[ \/]//'))" >&2; continue ;; esac
  fi
  echo "$f" >> "$W/copy"
done < "$W/files.u"
n=$(wc -l < "$W/copy"); [ "$n" -gt 0 ] || { echo "graft: $NAME: the list gave no files" >&2; exit 1; }
if [ "$DRY" = "--dry-run" ]; then sed 's/^/graft: +/' "$W/copy"; echo "graft: $NAME: $n files (dry run, nothing written)"; exit 0; fi
# tar keeps modes, links and hard links; the destination is only written, never emptied
tar -cf - -T "$W/copy" | tar -xf - -C "$DEST" --no-same-owner 2>/dev/null || tar -cf - -T "$W/copy" | tar -xf - -C "$DEST"
sed 's/^/graft: +/' "$W/copy" | head -n 400
# every shared library an ELF program or library of the graft needs must exist in the destination
RE="${READELF:-}"
[ -n "$RE" ] || RE="$(ls "$SRC"/../host/bin/*-readelf 2>/dev/null | head -n1 || true)"; [ -n "$RE" ] || RE="$(command -v readelf || true)"
if [ -n "$RE" ]; then
  miss=0
  while IFS= read -r f; do
    [ -f "$DEST/$f" ] && [ ! -L "$DEST/$f" ] || continue
    head -c 4 "$DEST/$f" | grep -q 'ELF' || continue
    for lib in $("$RE" -d "$DEST/$f" 2>/dev/null | sed -n 's/.*Shared library: \[\(.*\)\]/\1/p'); do
      case "$lib" in libc.so|libc.so.*|ld-musl*) continue ;; esac
      if [ ! -e "$DEST/usr/lib/$lib" ] && [ ! -e "$DEST/lib/$lib" ] && [ ! -e "$DEST/usr/lib64/$lib" ] && [ ! -e "$DEST/lib64/$lib" ]; then
        echo "graft: $f needs $lib, which is not in the image" >&2; miss=1
      fi
    done
  done < "$W/copy"
  [ "$miss" = 0 ] || exit 1
else
  echo "graft: no readelf found: the shared libraries were not checked" >&2
fi
bytes=$(while IFS= read -r f; do [ -f "$DEST/$f" ] && [ ! -L "$DEST/$f" ] && stat -c '%i %s' "$DEST/$f"; done < "$W/copy" | LC_ALL=C sort -u -k1,1 | awk '{s+=$2} END{print s+0}')
echo "graft: $NAME: $n files, $bytes bytes (hard-linked files counted once)"
