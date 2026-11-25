#!/usr/bin/env bash
# The UI program recipe (F-SPLIT-03): ui-program.sh builds iyakki-ui with the socket-shell feature, lean-image.sh places it at usr/bin/iyakki-ui (the ui
# layer's file) and only writes the flag file for a test image. No cargo, no QEMU. Run: dist/buildroot/test-ui-program.sh
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
pass=0; fail=0
check() { if eval "$2"; then pass=$((pass+1)); echo "ok   - $1"; else fail=$((fail+1)); echo "FAIL - $1"; fi; }
cmd="$("$HERE/ui-program.sh" --print-command | head -n1)"
check "the recipe builds iyakki-ui-bin with socket-shell"  'echo "$cmd" | grep -q -- "-p iyakki-ui-bin --features socket-shell"'
check "the recipe builds for the image target (musl)"      'echo "$cmd" | grep -q -- "--target x86_64-unknown-linux-musl"'
check "the program path ends in the file the layers reserve" '"$HERE/ui-program.sh" --print-command | tail -n1 | grep -q "/iyakki-ui$"'
check "lean-image.sh places IYAKKI_UI_BIN at usr/bin/iyakki-ui" 'grep -q "cp \"\$IYAKKI_UI_BIN\" usr/bin/iyakki-ui" "$HERE/lean-image.sh"'
check "the layer rules give usr/bin/iyakki-ui to the ui layer" 'grep -qx "rule ui usr/bin/iyakki-ui" "$HERE/layers.txt"'
check "the flag file is written only for IYAKKI_UI_PROCESS=1" 'grep -q "IYAKKI_UI_PROCESS:-0}\" != 1" "$HERE/lean-image.sh"'
check "the flag file name is the one the core reads"        'grep -q "etc/iyakki/ui-process.enabled" "$HERE/lean-image.sh" && grep -q "/etc/iyakki/ui-process.enabled" "$HERE/../../crates/iyakki-core/src/uiprocess.rs"'
check "no default image build turns the mode on"            '! grep -rq "ui-process.enabled" "$HERE/board" "$HERE/build.sh" "$HERE/configs" 2>/dev/null'
check "build.sh builds it only when asked (IYAKKI_WITH_UI_PROGRAM=1)" 'grep -q "IYAKKI_WITH_UI_PROGRAM:-0}\" = 1" "$HERE/build.sh" && grep -q "ui-program.sh" "$HERE/build.sh"'
check "post-build.sh installs the program the build made"   'grep -q "usr/bin/iyakki-ui" "$HERE/board/iyakki/post-build.sh"'
check "an unknown option is refused"                        '! "$HERE/ui-program.sh" --nope >/dev/null 2>&1'
echo "$pass passed, $fail failed"; [ "$fail" = 0 ]
