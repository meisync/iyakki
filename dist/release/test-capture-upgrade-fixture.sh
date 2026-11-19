#!/bin/sh
# Test of capture-upgrade-fixture.sh on a made-up folder (no image, no device). Run: dist/release/test-capture-upgrade-fixture.sh
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
W="$(mktemp -d "${TMPDIR:-/tmp}/iyakki-capture-test.XXXXXX")"; trap 'rm -rf "$W"' EXIT
fail=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi; }
mkdir -p "$W/data/dash" "$W/data/inbox" "$W/data/vault/users" "$W/out"
echo '{"schema":2,"tool_log_days":5}' > "$W/data/settings.json"
echo '{}' > "$W/data/dash/sources-amy.json"
echo '[]' > "$W/data/inbox/amy.json"
printf 'sealed' > "$W/data/vault/users/amy.afv"
echo 'ignored' > "$W/data/not-a-store.txt"
C="$HERE/capture-upgrade-fixture.sh"
check "refuses without --synthetic" "! sh $C beta1 $W/data --dest $W/out 2>/dev/null"
check "refuses the name 'current'" "! sh $C current $W/data --synthetic --dest $W/out 2>/dev/null"
check "copies the known stores" "sh $C beta1 $W/data --synthetic --dest $W/out >/dev/null && [ -f $W/out/beta1/settings.json ] && [ -f $W/out/beta1/dash/sources-amy.json ] && [ -f $W/out/beta1/vault/users/amy.afv ]"
check "leaves unknown files out" "[ ! -e $W/out/beta1/not-a-store.txt ]"
check "never overwrites a fixture" "! sh $C beta1 $W/data --synthetic --dest $W/out 2>/dev/null"
echo '{"path":"/home/someone/x"}' > "$W/data/dash/sources-amy.json"
check "stops on a home folder path" "! sh $C beta2 $W/data --synthetic --dest $W/out 2>/dev/null && [ ! -e $W/out/beta2 ]"
echo '{"host":"192.0.2.7"}' > "$W/data/dash/sources-amy.json"
check "stops on an address" "! sh $C beta3 $W/data --synthetic --dest $W/out 2>/dev/null"
exit $fail
