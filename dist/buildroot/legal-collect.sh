#!/usr/bin/env bash
# Collect Buildroot's legal-info (licence texts, source archives, manifest.csv) for the builds the images are made from, INCREMENTALLY, and
# add the licence texts Buildroot cannot find.   docs/development/RELEASE-IMAGE-PROFILE.md, LICENSING.md ("The licence ... gate")
#
#   legal-collect.sh [--dry-run] [--force] [--check] [TREE...]
#
#   TREE      a Buildroot output folder or its name under target/ (web, mac, ...). Default: web mac. The layered images (macweb, pc, and every
#             release image) are repacked from these two builds, so the gate reads the legal-info of both (crates/iyakki-relcheck/src/gate.rs).
#   --dry-run  run `make -n legal-info` per tree (no file is written) and say how many packages it would collect and whether any source
#              archive is missing from Buildroot's download folder; it also prints whether anything would be rebuilt (it must not).
#   --force    collect again although the stamp says the tree's legal-info is current.
#   --check    when done, run the licence gate's legal-info part (iyakki-relcheck licence-check --no-images).
#
# What it does per tree:
#   1. skips the tree when legal-info/.iyakki-collect.stamp matches the hash of its .config, its build/packages-file-list.txt and
#      legal/extra-licences (the incremental part: an unchanged build is not collected twice);
#   2. keeps the old legal-info/ as legal-info.prev, runs Buildroot's `make legal-info` (it only prepares and copies: it depends on
#      <package>-source and -patch, never on the build; nothing is compiled), and puts the old folder back when that fails (Buildroot
#      removes legal-info/ first);
#   3. copies legal/extra-licences/<package>/* into legal-info/licenses/<package>-<version>/ for a package whose folder there is empty
#      (apple-bce: the upstream repository has no licence file; legal/extra-licences/apple-bce/LICENSE-NOTE.txt says what was done);
#   4. writes the stamp.
# Needs the Buildroot source (target/buildroot-src), network only when a source archive is missing from its dl/ folder.
# Buildroot refuses GNU-incompatible `install` (the uutils one of newer Ubuntu: "affected by uutils/coreutils#12166"): when /usr/bin/install
# is not GNU's and /usr/bin/gnuinstall exists, a temporary PATH entry makes `install` GNU's for this run only; nothing in the system changes.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="${IYAKKI_ROOT:-$(cd "$HERE/../.." && pwd)}"
BR_VERSION="${BR_VERSION:-2026.02.3}"
BR_DIR="${BR_DIR:-$ROOT/target/buildroot-src/buildroot-$BR_VERSION}"
DRY=0; FORCE=0; CHECK=0; TREES=""
for a in "$@"; do
  case "$a" in
    --dry-run) DRY=1 ;; --force) FORCE=1 ;; --check) CHECK=1 ;;
    -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
    -*) echo "legal-collect.sh: unknown option $a" >&2; exit 2 ;;
    *) TREES="$TREES $a" ;;
  esac
done
[ -n "$TREES" ] || TREES="web mac"
[ -d "$BR_DIR" ] || { echo "legal-collect.sh: no Buildroot source at $BR_DIR" >&2; exit 1; }

# GNU install for this run only
SHIM=""
if ! install --version 2>/dev/null | grep -q 'GNU coreutils'; then
  if [ -x /usr/bin/gnuinstall ]; then
    SHIM="$(mktemp -d)"; ln -s /usr/bin/gnuinstall "$SHIM/install"; export PATH="$SHIM:$PATH"
    echo "legal-collect: /usr/bin/install is not GNU coreutils; using /usr/bin/gnuinstall for this run"
  else
    echo "legal-collect: WARNING: install is not GNU coreutils and /usr/bin/gnuinstall does not exist; Buildroot will refuse to run" >&2
  fi
fi
trap '[ -z "$SHIM" ] || rm -rf "$SHIM"' EXIT

stamp_of() { # tree -> hash of what decides the content of legal-info
  { cat "$1/.config" "$1/build/packages-file-list.txt" 2>/dev/null; find "$ROOT/legal/extra-licences" -type f 2>/dev/null | LC_ALL=C sort | xargs -r sha256sum; } | sha256sum | cut -c1-32
}

rc=0
for t in $TREES; do
  case "$t" in /*) T="$t" ;; */*) T="$ROOT/$t" ;; *) T="$ROOT/target/buildroot-$t" ;; esac
  name="$(basename "$T")"
  if [ ! -f "$T/.config" ] || [ ! -d "$T/build" ]; then echo "legal-collect: $name: no finished Buildroot build here (.config and build/ missing); skipped" >&2; rc=1; continue; fi
  MK=(make -C "$BR_DIR" O="$T" BR2_EXTERNAL="$HERE")
  if [ "$DRY" = 1 ]; then
    plan="$(mktemp)"
    if "${MK[@]}" -n legal-info > "$plan" 2>&1; then
      pk="$(grep -c '^echo ">>> .* Collecting legal info' "$plan" || true)"
      miss=0; for f in $(grep -o 'hardlink-or-copy [^ ]*/dl/[^ ]*' "$plan" | awk '{print $2}' | sort -u); do [ -e "$f" ] || { echo "  missing source archive: $f"; miss=$((miss + 1)); }; done
      build="$(grep -c -E '(touch|rm) [^|]*\.stamp_|^[[:space:]]*(\./configure|cmake|ninja|meson|gcc|g\+\+|cc) |/(configure|cmake|ninja|meson) ' "$plan" || true)"
      echo "legal-collect: $name: would collect $pk packages (target and host); $miss source archive(s) missing from dl/; $build build-looking command(s) in the plan (must be 0); stamp $( [ -f "$T/legal-info/.iyakki-collect.stamp" ] && [ "$(cat "$T/legal-info/.iyakki-collect.stamp")" = "$(stamp_of "$T")" ] && echo current || echo stale )"
      [ "$miss" = 0 ] && [ "$build" = 0 ] || rc=1
    else
      echo "legal-collect: $name: the dry run failed:" >&2; tail -n 8 "$plan" >&2; rc=1
    fi
    rm -f "$plan"; continue
  fi
  want="$(stamp_of "$T")"
  if [ "$FORCE" = 0 ] && [ -f "$T/legal-info/manifest.csv" ] && [ -f "$T/legal-info/.iyakki-collect.stamp" ] && [ "$(cat "$T/legal-info/.iyakki-collect.stamp")" = "$want" ]; then
    echo "legal-collect: $name: legal-info is current ($(($(wc -l < "$T/legal-info/manifest.csv") - 1)) packages); nothing to do"; continue
  fi
  echo "legal-collect: $name: make legal-info (copies licence texts and source archives; no compile)"
  rm -rf "$T/legal-info.prev"; [ ! -d "$T/legal-info" ] || mv "$T/legal-info" "$T/legal-info.prev"
  if ! "${MK[@]}" legal-info > "$T/legal-info.log" 2>&1; then
    echo "legal-collect: $name: make legal-info FAILED (log: $T/legal-info.log); the previous legal-info is put back" >&2; tail -n 8 "$T/legal-info.log" >&2
    rm -rf "$T/legal-info"; [ ! -d "$T/legal-info.prev" ] || mv "$T/legal-info.prev" "$T/legal-info"; rc=1; continue
  fi
  # licence texts Buildroot could not find: legal/extra-licences/<package>/ -> licenses/<package>-<version>/ (only into an EMPTY folder)
  added=0
  for d in "$ROOT"/legal/extra-licences/*/; do
    [ -d "$d" ] || continue
    p="$(basename "$d")"
    for lic in "$T"/legal-info/licenses/"$p"-*/; do
      [ -d "$lic" ] || continue
      if [ -z "$(ls -A "$lic" 2>/dev/null)" ]; then cp -a "$d". "$lic"; added=$((added + 1)); echo "legal-collect: $name: licence texts for $(basename "$lic") taken from legal/extra-licences/$p"; fi
    done
    # a package that has a manifest row but no licenses/ folder at all (Buildroot makes none when it finds no file)
    ver="$(awk -F'","' -v p="\"$p" '$1 == p { gsub(/"/, "", $2); print $2; exit }' "$T/legal-info/manifest.csv" 2>/dev/null || true)"
    if [ -n "$ver" ] && [ ! -d "$T/legal-info/licenses/$p-$ver" ]; then mkdir -p "$T/legal-info/licenses/$p-$ver"; cp -a "$d". "$T/legal-info/licenses/$p-$ver/"; added=$((added + 1)); echo "legal-collect: $name: licence texts for $p-$ver taken from legal/extra-licences/$p"; fi
  done
  "$HERE/legal-extras.sh" "$T" || rc=1
  echo "$want" > "$T/legal-info/.iyakki-collect.stamp"
  rm -rf "$T/legal-info.prev"
  echo "legal-collect: $name: done, $(($(wc -l < "$T/legal-info/manifest.csv") - 1)) packages in legal-info/manifest.csv ($added from legal/extra-licences)"
done
if [ "$CHECK" = 1 ] && [ "$DRY" = 0 ]; then
  (cd "$ROOT" && cargo run -q -p iyakki-relcheck -- licence-check . --no-images) || rc=1
fi
exit "$rc"
