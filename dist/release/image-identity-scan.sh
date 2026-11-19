#!/usr/bin/env bash
# Scans a kernel image (and the NVIDIA modules if given) for a builder's account name in the version banner:
#   dist/release/image-identity-scan.sh FILE...        e.g. target/buildroot-pc-push/images/bzImage
# Every "(user@host)" banner found must be iyakki@iyakki-build; anything else fails (a person's name must not ship in an image).
set -u
rc=0; seen=0
for f in "$@"; do
  [ -f "$f" ] || { echo "image-identity-scan: $f is not a file" >&2; rc=2; continue; }
  while IFS= read -r l; do
    seen=1
    case "$l" in
      *"(iyakki@iyakki-build)"*) echo "image-identity-scan: ok   $f";;
      *) echo "image-identity-scan: FAIL $f: $l" >&2; rc=1;;
    esac
  done < <(LC_ALL=C strings -a "$f" | grep -E '\([A-Za-z0-9._-]+@[A-Za-z0-9._-]+\)' | head -5)
done
[ "$seen" = 1 ] || echo "image-identity-scan: no banner found in the files given" >&2
exit $rc
