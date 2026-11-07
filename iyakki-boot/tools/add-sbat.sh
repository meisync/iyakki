#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Add an SBAT section (Secure Boot Advanced Targeting, https://github.com/rhboot/shim/blob/main/SBAT.md) to a UEFI program.
#   add-sbat.sh INPUT.efi OUTPUT.efi SBAT.csv
# shim 15.3 and newer refuse a second stage that has no .sbat section. SBAT.csv is the distribution's own text: the first line is
# `sbat,1,SBAT Version,sbat,1,https://github.com/rhboot/shim/blob/main/SBAT.md`, then one line per component, e.g.
# `my.loader,1,My Distribution,my-loader,0.1,https://example.org`. Use only your OWN component name: shim's revocation list names the
# big projects (grub, systemd) and a file must not claim to be one of them. Run this BEFORE signing (signing covers the section).
# Needs GNU objcopy (binutils). INPUT and OUTPUT must differ; the output is written under a temporary name and moved into place.
set -eu
usage() { echo "usage: $0 INPUT.efi OUTPUT.efi SBAT.csv" >&2; exit 2; }
[ $# -eq 3 ] || usage
IN=$1; OUT=$2; CSV=$3
[ -f "$IN" ] || { echo "add-sbat: input $IN not found" >&2; exit 1; }
[ -f "$CSV" ] || { echo "add-sbat: $CSV not found" >&2; exit 1; }
[ "$IN" != "$OUT" ] || { echo "add-sbat: input and output must be different files" >&2; exit 1; }
command -v objcopy >/dev/null 2>&1 || { echo "add-sbat: objcopy is not installed (package binutils)" >&2; exit 1; }
head -n1 "$CSV" | grep -q '^sbat,1,' || { echo "add-sbat: the first line of $CSV must be the sbat,1,SBAT Version,... line" >&2; exit 1; }
[ "$(wc -l < "$CSV")" -ge 2 ] || { echo "add-sbat: $CSV names no component (need a second line)" >&2; exit 1; }
if grep -qE '^(grub|shim|systemd)[,.]' "$CSV"; then echo "add-sbat: do not claim a component name of another project (grub, shim, systemd)" >&2; exit 1; fi
if objdump -h "$IN" 2>/dev/null | grep -q ' \.sbat '; then echo "add-sbat: $IN has an .sbat section already" >&2; exit 1; fi
TMP=$(mktemp "$OUT.XXXXXX")
trap 'rm -f "$TMP"' EXIT
# objcopy gives a new PE section no address of its own: place it after the last section, on the section alignment
ALIGN=$(objdump -p "$IN" | awk '$1 == "SectionAlignment" { print $2 }')
END=0
for l in $(objdump -h "$IN" | awk 'NR > 5 && $1 ~ /^[0-9]+$/ { print $4 ":" $3 }'); do
	e=$(( 0x${l%%:*} + 0x${l##*:} ))
	[ "$e" -gt "$END" ] && END=$e
done
[ -n "$ALIGN" ] && [ -n "$END" ] || { echo "add-sbat: cannot read the section layout of $IN" >&2; exit 1; }
VMA=$(( (END + 0x$ALIGN - 1) / 0x$ALIGN * 0x$ALIGN ))
objcopy --add-section .sbat="$CSV" --set-section-flags .sbat=contents,alloc,load,readonly,data --change-section-vma .sbat=$VMA "$IN" "$TMP"
objdump -h "$TMP" | grep -q ' \.sbat ' || { echo "add-sbat: the section did not end up in the file" >&2; exit 1; }
mv "$TMP" "$OUT"
trap - EXIT
echo "sbat section added: $OUT ($(wc -c < "$OUT") bytes)"
