#!/bin/sh
# Size-budget gate (F-PKG-05): check every finished image folder against dist/release/image-budgets.json with check-budgets.py.
#   dist/release/budgets-gate.sh [IMAGES_DIR...]     default: target/buildroot-*/images (a path with "libre" in it is checked as the libre image, one with buildroot-web as the web image, any other as the full one)
# Last line: "budgets-gate: PASS ...", "budgets-gate: FAIL ..." (a budget is over) or "budgets-gate: SKIPPED ..." (no folder: the images are built elsewhere).
# Exit 1 on FAIL, 2 on a usage error, else 0. Boot time and memory come from `measure-budgets.sh` and are not checked here.
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT" || exit 2
if [ "$#" -eq 0 ]; then
  set --
  for d in target/buildroot-*/images; do [ -d "$d" ] && set -- "$@" "$d"; done
fi
if [ "$#" -eq 0 ]; then echo "budgets-gate: SKIPPED, no images folder found (the check runs where the images are built)"; exit 0; fi
bad=0; n=0
for d in "$@"; do
  [ -d "$d" ] || { echo "budgets-gate: $d is not a folder" >&2; exit 2; }
  case "$d" in *libre*) kind=libre ;; *buildroot-web*) kind=web ;; *) kind=full ;; esac
  echo "== $d ($kind)"
  python3 -I dist/release/check-budgets.py --image "$kind" --images-dir "$d" || { [ "$?" -eq 1 ] && bad=1 || exit 2; }
  n=$((n+1))
done
if [ "$bad" = 0 ]; then echo "budgets-gate: PASS, $n folder(s) within their size budgets"; exit 0; fi
echo "budgets-gate: FAIL, a size budget is over (see above)"; exit 1
