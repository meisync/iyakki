#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Sign a UEFI program for Secure Boot and check the result.
#   sign.sh INPUT.efi OUTPUT.efi --key KEY.pem --cert CERT.pem [--no-verify]
# The key and the certificate are always given as paths; this script never looks for a key by itself, never prints one and never
# copies one. It needs sbsign and sbverify (package sbsigntool). INPUT and OUTPUT must differ.
set -eu

usage() { echo "usage: $0 INPUT.efi OUTPUT.efi --key KEY.pem --cert CERT.pem [--no-verify]" >&2; exit 2; }

[ $# -ge 2 ] || usage
IN=$1; OUT=$2; shift 2
KEY=""; CERT=""; VERIFY=1
while [ $# -gt 0 ]; do
	case $1 in
		--key) [ $# -ge 2 ] || usage; KEY=$2; shift 2 ;;
		--cert) [ $# -ge 2 ] || usage; CERT=$2; shift 2 ;;
		--no-verify) VERIFY=0; shift ;;
		*) usage ;;
	esac
done
[ -n "$KEY" ] && [ -n "$CERT" ] || usage
[ -f "$IN" ] || { echo "sign: input $IN not found" >&2; exit 1; }
[ -f "$KEY" ] || { echo "sign: key file not found" >&2; exit 1; }
[ -f "$CERT" ] || { echo "sign: certificate $CERT not found" >&2; exit 1; }
[ "$IN" != "$OUT" ] || { echo "sign: input and output must be different files" >&2; exit 1; }
command -v sbsign >/dev/null 2>&1 || { echo "sign: sbsign is not installed (package sbsigntool)" >&2; exit 1; }
command -v sbverify >/dev/null 2>&1 || { echo "sign: sbverify is not installed (package sbsigntool)" >&2; exit 1; }

# write next to the target and move into place, so a failed run never leaves a half-written signed file
TMP=$(mktemp "$OUT.XXXXXX")
trap 'rm -f "$TMP"' EXIT
sbsign --key "$KEY" --cert "$CERT" --output "$TMP" "$IN" >/dev/null
if [ "$VERIFY" = 1 ]; then
	sbverify --cert "$CERT" "$TMP" >/dev/null || { echo "sign: verification of the signed file failed" >&2; exit 1; }
fi
mv "$TMP" "$OUT"
trap - EXIT
echo "signed $OUT ($(wc -c < "$OUT") bytes)$([ "$VERIFY" = 1 ] && echo ', verified against the certificate')"
