#!/usr/bin/env bash
# The ui layer is separately shippable (docs/development/UI-BOUNDARY.md, section 6): two tiny roots that differ only in the UI program change only the
# ui layer; a root that differs only in the core program changes only the core layer. No Buildroot, no QEMU, no device. Needs mksquashfs and python3.
# Run: dist/buildroot/test-ui-layer.sh   (exit 77 when mksquashfs is missing)
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
command -v mksquashfs >/dev/null || { echo "test-ui-layer: mksquashfs missing"; exit 77; }
W="$(mktemp -d "${TMPDIR:-$HOME/.cache}/iyakki-ui-layer-test.XXXXXX")"
trap 'rm -rf "$W"' EXIT
pass=0; fail=0
ok()  { pass=$((pass+1)); echo "ok   - $1"; }
bad() { fail=$((fail+1)); echo "FAIL - $1"; }
mkroot() { # NAME UI_TEXT CORE_TEXT
  r="$W/$1"; mkdir -p "$r/usr/bin" "$r/usr/lib" "$r/usr/libexec/iyakki-ui" "$r/usr/share/fonts" "$r/etc/iyakki"
  printf '%s' "$2" > "$r/usr/bin/iyakki-ui"; printf '%s' "$3" > "$r/usr/bin/iyakki"
  echo base > "$r/usr/lib/libc.so"; echo font > "$r/usr/share/fonts/a.ttf"; echo cfg > "$r/etc/iyakki/x.conf"
  echo helper > "$r/usr/libexec/iyakki-ui/helper"
}
mkroot a ui1 core1; mkroot b ui2 core1; mkroot c ui1 core2
for n in a b c; do python3 "$HERE/layers.py" split "$W/$n" "$W/out-$n" --level 3 >/dev/null; done
h() { python3 - "$W/out-$1/layers.json" "$2" <<'P'
import json,sys
m=json.load(open(sys.argv[1]))
print(next(l for l in m["layers"] if l["name"]==sys.argv[2])["sha256"] if "sha256" in m["layers"][0] else next(l for l in m["layers"] if l["name"]==sys.argv[2])["path"])
P
}
asg="$(python3 "$HERE/layers.py" assign "$W/a")"
check() { if eval "$2"; then ok "$1"; else bad "$1"; fi; }
check "iyakki-ui is in the ui layer"            'echo "$asg" | grep -qx "ui usr/bin/iyakki-ui"'
check "usr/libexec/iyakki-ui/** is in the ui layer" 'echo "$asg" | grep -qx "ui usr/libexec/iyakki-ui/helper"'
check "iyakki (core program) stays in core"     'echo "$asg" | grep -qx "core usr/bin/iyakki"'
check "ui-only change: ui layer differs"        '[ "$(h a ui)" != "$(h b ui)" ]'
check "ui-only change: core layer identical"    '[ "$(h a core)" = "$(h b core)" ]'
check "ui-only change: base layer identical"    '[ "$(h a base)" = "$(h b base)" ]'
check "core-only change: core layer differs"    '[ "$(h a core)" != "$(h c core)" ]'
check "core-only change: ui layer identical"    '[ "$(h a ui)" = "$(h c ui)" ]'
echo "$pass passed, $fail failed"; [ "$fail" = 0 ]
