#!/bin/sh
# Keep only the firmware a keep list names:   prune-firmware.sh KEEP_LIST FIRMWARE_DIR
# Every file or symbolic link under FIRMWARE_DIR that no pattern of KEEP_LIST matches is deleted; a kept link also keeps the file
# it points to; empty directories go. Used by post-build.sh for the Mac image (board/iyakki/mac-firmware.keep, the format is
# described there). Prints one line: how many files and bytes were kept and removed.
set -eu
LIST="$1"
FW="$2"
[ -f "$LIST" ] || { echo "prune-firmware: no keep list $LIST" >&2; exit 1; }
[ -d "$FW" ] || { echo "prune-firmware: no firmware directory $FW (nothing to prune)" >&2; exit 0; }
FW="$(cd "$FW" && pwd -P)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
grep -v '^[[:space:]]*#' "$LIST" | grep -v '^[[:space:]]*$' > "$WORK/patterns" || true
[ -s "$WORK/patterns" ] || { echo "prune-firmware: $LIST has no patterns; refusing to delete all firmware" >&2; exit 1; }
(cd "$FW" && find . \( -type f -o -type l \) | sed 's|^\./||' | LC_ALL=C sort) > "$WORK/all"

# 1. what the patterns match (no pathname expansion of the patterns themselves: they are only used by `case`)
set -f
while IFS= read -r f; do
  while IFS= read -r p; do
    # shellcheck disable=SC2254
    case "$f" in $p) printf '%s\n' "$f"; break ;; esac
  done < "$WORK/patterns"
done < "$WORK/all" > "$WORK/keep"
set +f

# 2. the files kept links point to (linux-firmware's links are relative; a chain of links is followed to its end)
while IFS= read -r f; do
  if [ -L "$FW/$f" ]; then
    t="$(readlink -f "$FW/$f" || true)"
    case "$t" in "$FW"/*) printf '%s\n' "${t#"$FW"/}" ;; esac
    # intermediate links of a chain
    l="$FW/$f"
    while [ -L "$l" ]; do
      n="$(readlink "$l")"
      case "$n" in /*) l="$n" ;; *) l="$(dirname "$l")/$n" ;; esac
      r="$(cd "$(dirname "$l")" 2>/dev/null && pwd -P)/$(basename "$l")" || break
      case "$r" in "$FW"/*) printf '%s\n' "${r#"$FW"/}" ;; *) break ;; esac
      l="$r"
    done
  fi
done < "$WORK/keep" >> "$WORK/keep"
LC_ALL=C sort -u "$WORK/keep" -o "$WORK/keep"

# 3. delete the rest
LC_ALL=C comm -23 "$WORK/all" "$WORK/keep" > "$WORK/drop"
bytes() { s=0; while IFS= read -r f; do [ -L "$FW/$f" ] || s=$((s + $(wc -c < "$FW/$f"))); done < "$1"; echo "$s"; }
kept_b="$(bytes "$WORK/keep")"
drop_b="$(bytes "$WORK/drop")"
while IFS= read -r f; do rm -f "$FW/$f"; done < "$WORK/drop"
find "$FW" -depth -type d -empty -exec rmdir {} \; 2>/dev/null || true
echo "prune-firmware: kept $(wc -l < "$WORK/keep") files ($((kept_b / 1048576)) MiB), removed $(wc -l < "$WORK/drop") ($((drop_b / 1048576)) MiB) per $(basename "$LIST")"
