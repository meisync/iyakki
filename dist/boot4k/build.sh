#!/bin/sh
# Rebuild boot4k.bin from boot4k.S with LLVM's assembler and linker (permissive licences), or with --check say whether the checked-in
# file is what the source gives. Needs clang, ld.lld and llvm-objcopy.
set -eu
cd "$(dirname "$0")"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
clang -target i386-none-elf -x assembler -c boot4k.S -o "$tmp/b.o"
ld.lld -m elf_i386 --image-base=0 -Ttext=0x7C00 --oformat binary -o "$tmp/b.bin" "$tmp/b.o"
if [ "${1:-}" = "--check" ]; then
  cmp "$tmp/b.bin" boot4k.bin && echo "boot4k.bin is what boot4k.S gives"
else
  cp "$tmp/b.bin" boot4k.bin
fi
