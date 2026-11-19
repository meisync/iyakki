#!/bin/sh
# The C header of the iyakki-core shared library (F-CORE-27), made from the list of exported C functions that the c-abi gate keeps
# (docs/development/c-abi-symbols.txt). Prints to stdout; `make-core-header.sh --check FILE` says whether FILE is what would be printed.
# The header holds only what the gate lists, so it cannot promise a function the library does not export.
set -eu
here=$(cd "$(dirname "$0")/../.." && pwd)
list="$here/docs/development/c-abi-symbols.txt"
render() {
  echo '/* iyakki_core.h: the C interface of the iyakki-core shared library. Made by dist/release/make-core-header.sh; do not edit. */'
  echo '#ifndef IYAKKI_CORE_H'
  echo '#define IYAKKI_CORE_H'
  echo '#include <stddef.h>'
  echo '#include <stdint.h>'
  echo '#ifdef __cplusplus'
  echo 'extern "C" {'
  echo '#endif'
  awk -F'\t' '$1=="iyakki-core-lib" && $2!="" {print $3}' "$list" | sort | while IFS= read -r sig; do
    # pub unsafe extern "C" fn name(a: *mut u32, ...) -> i32   becomes   int32_t name(uint32_t *a, ...);
    echo "$sig" | sed -e 's/^pub //' -e 's/unsafe //' -e 's/extern "C" //' | awk '
      function ctype(t) {
        gsub(/^ +| +$/, "", t)
        if (t ~ /^\*mut /) { sub(/^\*mut /, "", t); return ctype(t) " *" }
        if (t ~ /^\*const /) { sub(/^\*const /, "", t); return "const " ctype(t) " *" }
        if (t == "u8") return "uint8_t"; if (t == "u16") return "uint16_t"; if (t == "u32") return "uint32_t"; if (t == "u64") return "uint64_t"
        if (t == "i8") return "int8_t"; if (t == "i16") return "int16_t"; if (t == "i32") return "int32_t"; if (t == "i64") return "int64_t"
        if (t == "usize") return "size_t"; if (t == "f64") return "double"; if (t == "f32") return "float"
        return t
      }
      {
        s = $0; sub(/^fn /, "", s)
        name = s; sub(/\(.*/, "", name)
        args = s; sub(/^[^(]*\(/, "", args); sub(/\).*$/, "", args)
        ret = "void"; if (s ~ /->/) { ret = s; sub(/^.*-> */, "", ret) }
        out = ""; n = split(args, a, /, */)
        for (i = 1; i <= n; i++) { if (a[i] == "") continue; split(a[i], p, /: */); t = ctype(p[2]); sep = (t ~ /\*$/) ? "" : " "; out = out (out == "" ? "" : ", ") t sep p[1] }
        if (out == "") out = "void"
        printf "%s %s(%s);\n", ctype(ret), name, out
      }'
  done
  echo '#ifdef __cplusplus'
  echo '}'
  echo '#endif'
  echo '#endif'
}
if [ "${1:-}" = "--check" ]; then
  render | diff -u "$2" - >/dev/null
else
  render
fi
