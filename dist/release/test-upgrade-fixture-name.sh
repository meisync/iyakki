#!/bin/sh
# The upgrade boot test takes the fixture folder from FIXTURE: a bad or missing name stops it before anything is booted, and the default is
# the alpha.1 folder (F-UPG-07). No QEMU, no image.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
S="$HERE/../buildroot/qemu-upgrade-test.sh"
fail() { echo "FAIL: $1" >&2; exit 1; }
for bad in "../alpha1" "a b" "current" "x;y" "/etc"; do
  out="$(FIXTURE="$bad" sh "$S" /nonexistent 2>&1)" && fail "FIXTURE='$bad' was accepted"
  echo "$out" | grep -q "FIXTURE must be" || fail "FIXTURE='$bad' gave another error: $out"
done
out="$(FIXTURE=beta sh "$S" /nonexistent 2>&1)" && fail "a fixture folder that does not exist was accepted"
echo "$out" | grep -q "no fixture folder tests/fixtures/upgrade/beta" || fail "missing folder not named: $out"
# the default is alpha1: it gets past the name checks and stops at the missing image (exit 3), not at the fixture
set +e; out="$(sh "$S" /nonexistent 2>&1)"; rc=$?; set -e
[ "$rc" = 3 ] || fail "the default fixture did not reach the image check (rc $rc): $out"
echo "ok: the upgrade boot test names its fixture and refuses a bad one"
