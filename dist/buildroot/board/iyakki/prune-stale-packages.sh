#!/bin/sh
# Removes from a root file system tree the files of Buildroot packages that are NOT selected in the current .config any more.
#
#   prune-stale-packages.sh [--dry-run] FILELIST CONFIG BR_PACKAGE_DIR ROOT
#
#   FILELIST        the build's `build/packages-file-list.txt` (lines `package,./path`)
#   CONFIG          the build's `.config`
#   BR_PACKAGE_DIR  the Buildroot source's `package/` folder (to know which packages have a selectable symbol)
#   ROOT            the unpacked root file system to clean
#
# Why: Buildroot's incremental builds do not delete the files a deselected package installed earlier, and `packages-file-list.txt` lists
# every package build folder still on disk, so a tree built once with cage, cog, wlroots, libinput or eudev and rebuilt without them keeps
# their files in target/ and in the image, with no legal-info row (they are not in the current manifest).
# A package is "stale" only when (1) its recipe has `config` or `menuconfig BR2_PACKAGE_<NAME>` (seatd uses menuconfig) in BR_PACKAGE_DIR, and (2) that symbol is not `=y` in
# CONFIG. Packages without a symbol of their own (skeletons, toolchain, busybox helpers, host-*) are never touched. A path is removed only
# when every package that lists it is stale; a path that a selected package also lists stays. Prints one summary line.
set -eu
DRY=0; [ "${1:-}" != --dry-run ] || { DRY=1; shift; }
[ $# -eq 4 ] || { echo "usage: prune-stale-packages.sh [--dry-run] FILELIST CONFIG BR_PACKAGE_DIR ROOT" >&2; exit 2; }
FL="$1"; CFG="$2"; PKGDIR="$3"; ROOT="$4"
[ -f "$FL" ] && [ -f "$CFG" ] && [ -d "$PKGDIR" ] && [ -d "$ROOT" ] || { echo "prune-stale-packages: missing input" >&2; exit 2; }
W="$(mktemp -d)"; trap 'rm -rf "$W"' EXIT
cut -d, -f1 "$FL" | LC_ALL=C sort -u | grep -v '^host-' > "$W/pkgs"
: > "$W/stale"
while read -r p; do
  sym="BR2_PACKAGE_$(printf '%s' "$p" | tr 'a-z-' 'A-Z_')"
  # a symbol of its own in the recipe folder (the folder name may differ from the package name for sub-packages: search the Config.in files)
  if grep -qs -E "^(menu)?config $sym\$" "$PKGDIR/$p/Config.in" 2>/dev/null || grep -rqs -E "^(menu)?config $sym\$" "$PKGDIR/$p" 2>/dev/null; then
    grep -q -x "$sym=y" "$CFG" || echo "$p" >> "$W/stale"
  fi
done < "$W/pkgs"
# paths owned only by stale packages
awk -F, -v stale="$W/stale" 'BEGIN{while((getline l<stale)>0) s[l]=1} {p=$1; path=substr($0,length($1)+2); if(p in s) st[path]=1; else live[path]=1} END{for(x in st) if(!(x in live)) print x}' "$FL" | LC_ALL=C sort > "$W/paths"
n=0
while IFS= read -r path; do
  f="$ROOT/${path#./}"
  if [ -e "$f" ] || [ -L "$f" ]; then
    n=$((n+1))
    [ "$DRY" = 1 ] || { [ -d "$f" ] && [ ! -L "$f" ] || rm -f "$f"; }
  fi
done < "$W/paths"
# empty directories the removal left behind
[ "$DRY" = 1 ] || { while IFS= read -r path; do d="$(dirname "$ROOT/${path#./}")"; rmdir -p "$d" 2>/dev/null || true; done < "$W/paths"; }
# plugin options of a SELECTED package that are switched off: gst-plugins-bad's WPE plugin (-Dwpe) is a sub-option, not a package, and an
# earlier build that had it on leaves two files that no package list owns (the licence scan reports them as unattributed)
if ! grep -q -x 'BR2_PACKAGE_GST1_PLUGINS_BAD_PLUGIN_WPE=y' "$CFG"; then
  for f in usr/lib/gstreamer-1.0/libgstwpe.so usr/lib/gst-plugins-bad/wpe-extension/libgstwpeextension.so; do
    if [ -e "$ROOT/$f" ] || [ -L "$ROOT/$f" ]; then n=$((n+1)); [ "$DRY" = 1 ] || { rm -f "$ROOT/$f"; rmdir -p "$(dirname "$ROOT/$f")" 2>/dev/null || true; }; fi
  done
fi
echo "prune-stale-packages: $(wc -l < "$W/stale" | tr -d ' ') deselected package(s) ($(tr '\n' ' ' < "$W/stale" | sed 's/ $//')), $n file(s) $( [ "$DRY" = 1 ] && echo 'would be removed' || echo removed )"
