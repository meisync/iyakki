#!/usr/bin/env bash
# Adds a legal-info row, the licence texts and the source archive for packages that left files in a Buildroot tree although its current .config
# does not select them (legal/extra-licences/<package>/manifest-row.csv says how). No make, nothing compiled; idempotent.
#   legal-extras.sh TREE_DIR            (called by legal-collect.sh after `make legal-info`; may be run alone on a collected tree)
# A row is added only when (1) the package has no row in legal-info/manifest.csv, (2) build/packages-file-list.txt lists files of it, and
# (3) its source archive is in the Buildroot download folder. The package's recipe folder is never touched.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="${IYAKKI_ROOT:-$(cd "$HERE/../.." && pwd)}"
BR_VERSION="${BR_VERSION:-2026.02.3}"
BR_DIR="${BR_DIR:-$ROOT/target/buildroot-src/buildroot-$BR_VERSION}"
T="${1:?usage: legal-extras.sh TREE_DIR}"
LI="$T/legal-info"
[ -f "$LI/manifest.csv" ] || { echo "legal-extras: $T has no legal-info/manifest.csv" >&2; exit 1; }
for d in "$ROOT"/legal/extra-licences/*/; do
  p="$(basename "$d")"; row="$d/manifest-row.csv"
  [ -f "$row" ] || continue
  grep -q "^\"$p\"," "$LI/manifest.csv" && continue
  grep -q "^$p," "$T/build/packages-file-list.txt" 2>/dev/null || continue
  ver="$(awk -F'","' '{ gsub(/"/, "", $2); print $2 }' "$row")"
  arc="$(awk -F'","' '{ print $5 }' "$row")"
  src="$BR_DIR/dl/$p/$arc"
  [ -f "$src" ] || { echo "legal-extras: $(basename "$T"): $p: source archive $src is missing; no row added" >&2; continue; }
  mkdir -p "$LI/licenses/$p-$ver" "$LI/sources"
  cp -a "$d". "$LI/licenses/$p-$ver/"; rm -f "$LI/licenses/$p-$ver/manifest-row.csv"
  cp -f "$src" "$LI/sources/$arc"
  cat "$row" >> "$LI/manifest.csv"
  echo "legal-extras: $(basename "$T"): added $p-$ver (row, licence texts, source archive) from legal/extra-licences/$p"
done
