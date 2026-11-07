#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Test of sign.sh with a throwaway key made here (never a real one). Usage: sign-test.sh INPUT.efi
# Works in a temp directory under ${BOOT_SCRATCH:-${XDG_CACHE_HOME:-$HOME/.cache}/boot-loader-test}. Needs openssl, sbsign, sbverify.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
IN=${1:?usage: sign-test.sh INPUT.efi}
BASE=${BOOT_SCRATCH:-${XDG_CACHE_HOME:-$HOME/.cache}/boot-loader-test}
mkdir -p "$BASE"
T=$(mktemp -d "$BASE/sign.XXXXXX")
trap 'rm -rf "$T"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }

openssl req -new -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=throwaway boot test key/" -keyout "$T/k.pem" -out "$T/c.pem" 2>/dev/null
openssl req -new -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=another throwaway key/" -keyout "$T/k2.pem" -out "$T/c2.pem" 2>/dev/null

"$HERE/sign.sh" "$IN" "$T/signed.efi" --key "$T/k.pem" --cert "$T/c.pem" >/dev/null || fail "sign.sh did not sign"
sbverify --cert "$T/c.pem" "$T/signed.efi" >/dev/null || fail "signed file does not verify against its certificate"
echo "ok: signed with a throwaway key and verified"
if sbverify --cert "$T/c2.pem" "$T/signed.efi" >/dev/null 2>&1; then fail "verified against the wrong certificate"; fi
echo "ok: a different certificate does not verify it"
if sbverify --cert "$T/c.pem" "$IN" >/dev/null 2>&1; then fail "the unsigned input verifies"; fi
echo "ok: the unsigned input does not verify"
cp "$T/signed.efi" "$T/tampered.efi"
printf 'X' | dd of="$T/tampered.efi" bs=1 seek=600 conv=notrunc 2>/dev/null
if sbverify --cert "$T/c.pem" "$T/tampered.efi" >/dev/null 2>&1; then fail "a changed file still verifies"; fi
echo "ok: a changed byte breaks the signature"
if "$HERE/sign.sh" "$IN" "$IN" --key "$T/k.pem" --cert "$T/c.pem" >/dev/null 2>&1; then fail "same input and output accepted"; fi
if "$HERE/sign.sh" "$IN" "$T/x.efi" --key "$T/missing.pem" --cert "$T/c.pem" >/dev/null 2>&1; then fail "missing key accepted"; fi
[ ! -e "$T/x.efi" ] || fail "output left behind after an error"
echo "ok: bad arguments are refused and leave nothing behind"
echo "PASS sign-test"
