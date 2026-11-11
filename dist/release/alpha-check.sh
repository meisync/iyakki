#!/usr/bin/env bash
# ./run.sh alpha-check [--dry] [--no-qemu] [--no-tests] [--keep-licenses]
# The gate before an alpha is cut. Prints a PASS / FAIL / WARN / SKIP table and exits non-zero when anything FAILs.
#   --dry           only the fast checks that do not build or write anything (version, FEATURES totals, links, secrets, files, sizes)
#   --no-qemu       skip the QEMU smoke tests even when images exist
#   --no-tests      skip `cargo test --workspace` (the build, licence and doc checks still run)
#   --keep-licenses do not regenerate THIRD_PARTY_LICENSES.txt
# Cargo runs with TMPDIR on the disk (target/tmp) and old iyakki-* leftovers there are removed at the end.
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/env.sh"
cd "$ROOT"
DRY=0; QEMU=1; TESTS=1; LIC=1
for a in "$@"; do case "$a" in
  --dry) DRY=1 ;; --no-qemu) QEMU=0 ;; --no-tests) TESTS=0 ;; --keep-licenses) LIC=0 ;;
  -h|--help) sed -n 2,9p "$0"; exit 0 ;; *) echo "unknown option $a" >&2; exit 2 ;; esac; done
OUT="$ROOT/target/alpha-check"; mkdir -p "$OUT"
trap iyakki_tmp_clean EXIT
NAMES=(); RES=(); NOTE=()
rec() { NAMES+=("$1"); RES+=("$2"); NOTE+=("${3:-}"); }
run() { # name, logfile, command...   PASS when the command succeeds
  local name="$1" log="$OUT/$2.log"; shift 2
  if "$@" >"$log" 2>&1; then rec "$name" PASS; else rec "$name" FAIL "see $log"; fi
}

# 0. environment
pct="$(inode_pct /tmp)"
if [ -n "$pct" ] && [ "$pct" -gt 70 ]; then rec "/tmp inodes" WARN "${pct}% used: tests leak temp dirs (rm -rf /tmp/iyakki-*); TMPDIR is $TMPDIR"; else rec "/tmp inodes" PASS "${pct:-?}% used; TMPDIR=$TMPDIR"; fi
command -v cargo >/dev/null || { echo "cargo not found" >&2; exit 2; }

# 1. the checker program (also unit-tested)
if cargo build -q --locked -p iyakki-relcheck >"$OUT/relcheck-build.log" 2>&1; then CHK="$ROOT/target/debug/iyakki-relcheck"; else cat "$OUT/relcheck-build.log"; rec "iyakki-relcheck builds" FAIL "$OUT/relcheck-build.log"; CHK=; fi

if [ -n "$CHK" ]; then
  run "version consistency" version "$CHK" version "$ROOT"
  run "FEATURES.md totals" features "$CHK" features "$ROOT"
  run "docs links and anchors" links "$CHK" links "$ROOT"
  if "$CHK" secrets "$ROOT" --all >"$OUT/secrets.log" 2>&1; then rec "no secrets in the tree" PASS "$(tail -n1 "$OUT/secrets.log")"; else rec "no secrets in the tree" FAIL "see $OUT/secrets.log"; fi
fi

# 2. required files
missing=""; for f in LICENSE NOTICE LICENSING.md THIRD_PARTY_LICENSES.txt README.md CHANGELOG.md KNOWN-ISSUES.md SECURITY.md SUPPORT.md ALPHA-GATE.md COMMIT-PLAN.md docs/RELEASING.md docs/LICENSES.md dist/release/release.sh dist/release/commit-alpha.sh; do [ -f "$f" ] || missing="$missing $f"; done
VER="$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n1)"
[ -f "RELEASE-NOTES-$VER.md" ] || missing="$missing RELEASE-NOTES-$VER.md"
if [ -z "$missing" ]; then rec "release files present" PASS; else rec "release files present" FAIL "missing:$missing"; fi
if grep -q "OWNER: fill in" SECURITY.md 2>/dev/null; then rec "security contact filled in" WARN "SECURITY.md still has the OWNER: fill in placeholder"; else rec "security contact filled in" PASS; fi

# 3. big and junk files (tracked or untracked, not ignored)
big="$( (git ls-files; git ls-files -o --exclude-standard) 2>/dev/null | sort -u | while read -r f; do [ -f "$f" ] && [ "$(stat -c %s "$f")" -gt 10485760 ] && echo "$f"; done )"
if [ -z "$big" ]; then rec "no file over 10 MB in the tree" PASS; else rec "no file over 10 MB in the tree" FAIL "$(echo $big)"; fi
junk="$( (git ls-files; git ls-files -o --exclude-standard) 2>/dev/null | grep -E '\.(bak|orig|rej|swp)$|~$|(^|/)core(\.[0-9]+)?$' | sort -u )"
if [ -z "$junk" ]; then rec "no backup or junk files" PASS; else rec "no backup or junk files" WARN "$(echo $junk)"; fi

if [ "$DRY" = 1 ]; then
  for s in "workspace builds (locked)" "zero warnings" "workspace tests" "window (iyakki-gui) builds" "macOS cross-check" "cargo deny licenses" "THIRD_PARTY_LICENSES.txt regenerated" "SBOM.json" "QEMU smoke tests"; do rec "$s" SKIP "--dry"; done
else
  # 4. build, warnings, tests
  if cargo build --workspace --locked >"$OUT/build.log" 2>&1; then rec "workspace builds (locked)" PASS; else rec "workspace builds (locked)" FAIL "see $OUT/build.log"; fi
  w="$(grep -c '^warning' "$OUT/build.log" || true)"; if [ "${w:-0}" = 0 ]; then rec "zero warnings" PASS; else rec "zero warnings" FAIL "$w warning line(s) in $OUT/build.log"; fi
  if [ "$TESTS" = 1 ]; then
    # one run, then the failed tests alone (at most two runs); a test that passes only alone is listed as flaky and limited by dist/release/known-flaky.txt
    dist/release/test-with-rerun.sh "$OUT" >"$OUT/test-summary.log" 2>&1; trc=$?
    passed="$(grep -h '^test result' "$OUT/test.log" | awk '{p+=$4; i+=$8} END{print p" passed, "i" ignored"}')"
    case "$trc" in
      0) rec "workspace tests" PASS "$passed" ;;
      3) rec "workspace tests" WARN "$passed; FLAKY (passed alone, listed in dist/release/known-flaky.txt): $(tr '\n' ' ' < "$OUT/test-flaky.txt")" ;;
      *) rec "workspace tests" FAIL "see $OUT/test-summary.log and $OUT/test.log" ;;
    esac
  else rec "workspace tests" SKIP "--no-tests"; fi
  if cargo build --locked --manifest-path crates/iyakki-gui/Cargo.toml --bin iyakki-desktop >"$OUT/gui-build.log" 2>&1; then
    rec "window (iyakki-gui) builds" PASS "$(crates/iyakki-gui/target/debug/iyakki-desktop --version 2>&1 | head -n1)"
  else rec "window (iyakki-gui) builds" FAIL "see $OUT/gui-build.log"; fi
  # 4b. macOS: compiles only (cargo check for the installed apple-darwin targets). WARN-level: a skip or a failure warns, never fails the gate
  mac="$(bash dist/release/check-macos.sh 2>&1 | tee "$OUT/macos-check.log" | tail -n1)"
  case "$mac" in
    "check-macos: PASS"*) rec "macOS cross-check" PASS "${mac#check-macos: }" ;;
    "check-macos: SKIP"*) rec "macOS cross-check" WARN "${mac#check-macos: } (compiles-only check; see $OUT/macos-check.log)" ;;
    *) rec "macOS cross-check" WARN "${mac#check-macos: }: a Linux-only piece lost its cfg fence (see $OUT/macos-check.log)" ;;
  esac
  # 5. licences
  if command -v cargo-deny >/dev/null; then
    if cargo deny --manifest-path "$ROOT/Cargo.toml" check licenses >"$OUT/deny.log" 2>&1 && cargo deny --manifest-path "$ROOT/crates/iyakki-gui/Cargo.toml" check licenses >>"$OUT/deny.log" 2>&1; then rec "cargo deny licenses" PASS "both workspaces"; else rec "cargo deny licenses" FAIL "see $OUT/deny.log"; fi
  else rec "cargo deny licenses" FAIL "cargo-deny is not installed (cargo install cargo-deny)"; fi
  if [ "$LIC" = 1 ] && command -v cargo-about >/dev/null; then
    if cargo about generate about.hbs -c about.toml -m crates/iyakki-core/Cargo.toml -o "$OUT/THIRD_PARTY_LICENSES.txt" >"$OUT/about.log" 2>&1; then
      if cmp -s "$OUT/THIRD_PARTY_LICENSES.txt" THIRD_PARTY_LICENSES.txt; then rec "THIRD_PARTY_LICENSES.txt regenerated" PASS "unchanged"; else cp "$OUT/THIRD_PARTY_LICENSES.txt" THIRD_PARTY_LICENSES.txt; rec "THIRD_PARTY_LICENSES.txt regenerated" WARN "it changed and was rewritten: review git diff"; fi
    else rec "THIRD_PARTY_LICENSES.txt regenerated" FAIL "see $OUT/about.log"; fi
  else rec "THIRD_PARTY_LICENSES.txt regenerated" SKIP "$([ "$LIC" = 1 ] && echo 'cargo-about not installed' || echo '--keep-licenses')"; fi
  # 6. SBOM
  if [ -n "$CHK" ]; then run "SBOM.json" sbom "$CHK" sbom "$ROOT" --out "$ROOT/SBOM.json" --date "$(date -u +%F)"; else rec "SBOM.json" SKIP "no checker"; fi
  # 7. QEMU smoke tests: only when images exist (none are built here)
  IMGDIR="$ROOT/target/buildroot-libre/images"
  if [ "$QEMU" = 1 ] && [ -f "$IMGDIR/bzImage" ] && [ -f "$IMGDIR/rootfs.cpio.zst" ] && command -v qemu-system-x86_64 >/dev/null; then
    # an image built before the self-test last changed cannot show the lines it requires: say so, instead of a bare "missing ..."
    ST="$(git log -1 --format=%ct -- dist/buildroot/qemu-test.sh crates/iyakki-agent/src/pathcheck.rs 2>/dev/null || echo 0)"
    IM="$(stat -c %Y "$IMGDIR/rootfs.cpio.zst" 2>/dev/null || echo 0)"
    if [ "${ST:-0}" -gt "$IM" ]; then
      rec "QEMU boot self-test" FAIL "STALE IMAGE: $IMGDIR was built $(date -u -d @"$IM" +%F), the self-test changed $(date -u -d @"$ST" +%F); rebuild it with ./run.sh build, then run this gate again"
    else
      run "QEMU boot self-test" qemu-boot sh dist/buildroot/qemu-test.sh
    fi
    run "QEMU persistence (data volume survives a reboot)" qemu-persist sh dist/buildroot/qemu-persist-test.sh
    if [ -f "$IMGDIR/iyakki-install-x86_64.img" ]; then
      cargo build -q --locked -p iyakki-usb >/dev/null 2>&1
      run "QEMU install (iyakki-usb, BIOS and UEFI boots)" qemu-install env IYAKKI_USB="$ROOT/target/debug/iyakki-usb" sh dist/buildroot/qemu-install-test.sh
    else rec "QEMU install" SKIP "no install image"; fi
  else rec "QEMU smoke tests" SKIP "$([ "$QEMU" = 1 ] && echo 'no images or no qemu' || echo '--no-qemu')"; fi
fi

# the table
echo; printf '%-52s %-5s %s\n' CHECK RESULT NOTE; printf '%-52s %-5s %s\n' ---- ------ ----
fail=0; for i in "${!NAMES[@]}"; do printf '%-52s %-6s %s\n' "${NAMES[$i]}" "${RES[$i]}" "${NOTE[$i]}"; [ "${RES[$i]}" = FAIL ] && fail=$((fail+1)); done
echo; if [ "$fail" = 0 ]; then echo "alpha-check: no FAIL ($(printf '%s\n' "${RES[@]}" | grep -c WARN) warning(s), $(printf '%s\n' "${RES[@]}" | grep -c SKIP) skipped). Logs: $OUT"; else echo "alpha-check: $fail FAIL. Logs: $OUT"; fi
echo "Still by hand (see ALPHA-GATE.md): window smoke in a real session, Mac mini, bare-metal items, signing key backup."
[ "$fail" = 0 ]
