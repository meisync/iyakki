#!/usr/bin/env bash
# Test of the lean mode of quick-push.sh and of nightly-push.sh on STUBS: no Buildroot, no cargo, no QEMU, no image, nothing real is touched
# and nothing is ever sent to a device. Run: dist/buildroot/test-quick-push.sh
set -eu
REAL="$(cd "$(dirname "$0")" && pwd)"
W="$(mktemp -d "${TMPDIR:-$HOME/.cache}/iyakki-quick-push-test.XXXXXX")"
trap 'rm -rf "$W"' EXIT
pass=0; fail=0
ok()   { pass=$((pass+1)); echo "ok   - $1"; }
bad()  { fail=$((fail+1)); echo "FAIL - $1"; }
check() { if eval "$2"; then ok "$1"; else bad "$1"; fi; }

# a private dist/buildroot with the two real scripts and stubs for everything they call
HERE="$W/here"; mkdir -p "$HERE" "$W/repo/target/x86_64-unknown-linux-musl/device"
cp "$REAL/quick-push.sh" "$REAL/nightly-push.sh" "$REAL/linker.sh" "$HERE/"
LOG="$W/calls.log"; : > "$LOG"
cat > "$HERE/repack.sh" <<EOF
#!/usr/bin/env bash
echo "repack \$*" >> "$LOG"
echo "sb=\${IYAKKI_SB_KEY:-unset}" >> "$LOG"
OUT="\$2"; mkdir -p "\$OUT/images"; echo img > "\$OUT/images/iyakki-install-x86_64.img"
EOF
cat > "$HERE/lean-image.sh" <<EOF
#!/usr/bin/env bash
echo "lean-image \$*" >> "$LOG"
[ "\${STUB_LEAN_FAIL:-0}" = 0 ] || exit 1
mkdir -p "\$2"; echo lean > "\$2/iyakki-lean-x86_64.img"
case " \$* " in *" --layers "*) mkdir -p "\$2/layers"; echo '{"schema":1,"layers":[]}' > "\$2/layers/layers.json"; echo x > "\$2/layers/base-aaaaaaaaaaaa.sfs" ;; esac
EOF
cat > "$W/iyakki-bin" <<EOF
#!/usr/bin/env bash
echo "bin \$*" >> "$LOG"
EOF
cat > "$HERE/qemu-stub.py" <<'EOF'
import os
mode = os.environ.get("STUB_BOOT", "ok")
print("grub at 1.0 s")
if mode in ("ok", "noswitch", "nolayers"):
    print("stage1: root boot/root.sfs mounted at 2100 ms")
if mode in ("ok", "noswitch"):
    print("stage1: root layers mounted: base core ui (seed layers)")
if mode in ("ok", "nosquash", "nolayers"):
    print("stage1: switching root")
EOF
chmod +x "$HERE"/*.sh "$W/iyakki-bin"
printf k > "$W/sb.key"; printf c > "$W/sb.crt"
PACK="$W/pack"
export IYAKKI_ROOT="$W/repo"
unset IYAKKI_SB_KEY IYAKKI_SB_CERT IYAKKI_PUSH_TARGET IYAKKI_STAGE1 || true
QP() { "$HERE/quick-push.sh" --bin "$W/iyakki-bin" -o "$PACK" "$@"; }

# 1. lean without the signing environment: fails loudly, builds nothing
: > "$LOG"
check "--lean without IYAKKI_SB_KEY/CERT fails with a message" "! QP --lean >/dev/null 2>$W/err && grep -q 'IYAKKI_SB_KEY and IYAKKI_SB_CERT' $W/err"
check "... and ran no repack and no lean image" "[ ! -s '$LOG' ]"
check "an unreadable key file fails too" "! IYAKKI_SB_KEY=$W/none IYAKKI_SB_CERT=$W/sb.crt QP --lean >/dev/null 2>&1"
export IYAKKI_SB_KEY="$W/sb.key" IYAKKI_SB_CERT="$W/sb.crt"

# 2. lean with the environment: repack, then lean-image (--root sfs --level 3), then the pack comes from the LEAN image
: > "$LOG"; rm -rf "$PACK"
check "--lean runs to the end on stubs" "QP --lean >$W/out 2>&1"
check "repack ran first with the signing key in its environment" "head -2 '$LOG' | grep -q '^repack pc ' && grep -q 'sb=$W/sb.key' '$LOG'"
check "lean-image.sh got the repacked images dir and --root sfs --level 3" "grep -q 'lean-image .*/images .*/lean --root sfs --level 3' '$LOG'"
check "update-pack was made from the lean image, variant pc" "grep -q 'bin dev update-pack .*/lean/iyakki-lean-x86_64.img -o $PACK --variant pc' '$LOG'"
check "the order is repack, lean-image, update-pack" "[ \"\$(grep -o '^repack\\|^lean-image\\|^bin' '$LOG' | tr '\\n' ' ')\" = 'repack lean-image bin ' ]"
check "the last line says nothing was sent and names the command" "tail -1 '$W/out' | grep -q 'nothing was sent' && tail -1 '$W/out' | grep -q 'iyakki dev update'"
: > "$LOG"
QP --lean --lean-level 19 >/dev/null 2>&1
check "--lean-level 19 reaches lean-image.sh" "grep -q 'lean-image .*--level 19' '$LOG'"
check "--lean-level must be a number" "! QP --lean --lean-level x >/dev/null 2>&1"

# 3. full mode is as before: no lean image, pack from the repacked install image; the environment variable chooses lean, --full overrides it
: > "$LOG"
QP >/dev/null 2>&1
check "default (no flag) does not run lean-image.sh" "! grep -q lean-image '$LOG' && grep -q 'update-pack .*/images/iyakki-install-x86_64.img' '$LOG'"
: > "$LOG"
IYAKKI_PUSH_TARGET=lean QP >/dev/null 2>&1
check "IYAKKI_PUSH_TARGET=lean turns lean on" "grep -q lean-image '$LOG'"
: > "$LOG"
IYAKKI_PUSH_TARGET=lean QP --full >/dev/null 2>&1
check "--full overrides IYAKKI_PUSH_TARGET=lean" "! grep -q lean-image '$LOG'"
check "a wrong IYAKKI_PUSH_TARGET is refused" "! IYAKKI_PUSH_TARGET=maybe QP >/dev/null 2>&1"
check "a failing lean-image.sh stops the script (no pack)" "! STUB_LEAN_FAIL=1 QP --lean >/dev/null 2>&1"

# 4. nightly-push.sh
NP() { IYAKKI_PUSH_OUT="$W/repo/target/buildroot-pc-push" IYAKKI_QEMU_TEST="$HERE/qemu-stub.py" "$HERE/nightly-push.sh" -o "$PACK" --no-cargo -- --bin "$W/iyakki-bin"; }
: > "$LOG"
check "nightly without the signing environment fails" "! env -u IYAKKI_SB_KEY -u IYAKKI_SB_CERT '$HERE/nightly-push.sh' -o '$PACK' >/dev/null 2>&1"
check "nightly passes on a boot that shows both lines" "STUB_BOOT=ok NP >$W/nout 2>&1"
check "nightly used level 19 and the lean mode" "grep -q 'lean-image .*--level 19' '$LOG'"
check "nightly prints the dev update and dev reboot commands" "grep -q 'iyakki dev update $PACK' '$W/nout' && grep -q 'iyakki dev reboot' '$W/nout'"
check "nightly does not print the publish command unless --publish is given" "! grep -q 'publish-build.sh' '$W/nout'"
NPP() { IYAKKI_PUSH_OUT="$W/repo/target/buildroot-pc-push" IYAKKI_QEMU_TEST="$HERE/qemu-stub.py" "$HERE/nightly-push.sh" -o "$PACK" --no-cargo --publish -- --bin "$W/iyakki-bin"; }
check "nightly --publish prints (only prints) the publish command after a passed boot test" "STUB_BOOT=ok NPP >$W/pout 2>&1 && grep -q 'publish-build.sh $PACK --variant pc-x86_64 --channel dev --build-id nightly-' '$W/pout' && ! grep -q 'publish-build' '$LOG'"
check "nightly --publish prints no publish command when the boot test fails" "! STUB_BOOT=noswitch NPP >$W/pout2 2>&1 && ! grep -q 'publish-build.sh' '$W/pout2'"
check "nightly never calls the program with dev update or reboot" "! grep -q 'bin dev update \\|bin dev reboot' '$LOG' && ! grep -q 'bin dev update ' '$LOG'"
check "nightly fails when 'root boot/root.sfs mounted' is absent" "! STUB_BOOT=nosquash NP >$W/nout2 2>&1 && grep -q 'did NOT see: root boot/root.sfs mounted' '$W/nout2'"
check "nightly fails when 'switching root' is absent" "! STUB_BOOT=noswitch NP >$W/nout3 2>&1 && grep -q 'did NOT see: switching root' '$W/nout3'"
check "a failing boot test prints no command to run" "! grep -q 'dev reboot\\|dev update' '$W/nout2' '$W/nout3'"
# the script itself: no network or device verbs at all (comments excluded)
check "nightly-push.sh has no ssh/scp/curl/sftp outside comments" "! grep -v '^ *#' '$REAL/nightly-push.sh' | grep -E '(^|[^a-z])(ssh|scp|curl|sftp)([^a-z]|\$)'"
check "nightly-push.sh never runs the program: the commands exist only as printed text" "! grep -E '^[^ #].*[$]IYK|^[$]IYK' '$REAL/nightly-push.sh'"
check "both scripts parse" "bash -n '$REAL/quick-push.sh' && bash -n '$REAL/nightly-push.sh'"

# 5. layers mode (docs/development/LAYERED-ROOT-DESIGN.md)
export IYAKKI_PUSHED_DIR="$W/pushed"
: > "$LOG"; rm -rf "$PACK"
QP --layers >$W/lout 2>&1
check "--layers implies the lean image and passes --layers to lean-image.sh" "grep -q 'lean-image .*--root sfs .*--layers' '$LOG'"
check "with no earlier build every layer is carried whole (no --against)" "! grep -q -- '--against' '$LOG' && grep -q 'every layer is carried whole' '$W/lout'"
mkdir -p "$W/pushed/pc"; echo '{}' > "$W/pushed/pc/layers.json"
: > "$LOG"; rm -rf "$PACK"
QP --layers >$W/lout 2>&1
check "with a recorded build the pack is made --against it" "grep -q 'update-pack .*--against $W/pushed/pc' '$LOG'"
: > "$LOG"; rm -rf "$PACK"
QP --layers --no-against >/dev/null 2>&1
check "--no-against skips the comparison" "! grep -q -- '--against' '$LOG'"
rm -rf "$W/pushed"
NPL() { IYAKKI_PUSH_OUT="$W/repo/target/buildroot-pc-push" IYAKKI_QEMU_TEST="$HERE/qemu-stub.py" "$HERE/nightly-push.sh" -o "$PACK" --no-cargo --layers -- --bin "$W/iyakki-bin"; }
: > "$LOG"; rm -rf "$PACK"
check "nightly --layers passes when both boot tests show their lines" "STUB_BOOT=ok NPL >$W/lnout 2>&1"
check "... it saw the layers line and the single-root line" "grep -q 'saw: root layers mounted: base core ui' '$W/lnout' && grep -q 'saw: root boot/root.sfs mounted' '$W/lnout'"
check "... it built the single-root image without --layers for the second test" "grep -c 'lean-image ' '$LOG' | grep -q '^2\$' && grep 'lean-image ' '$LOG' | tail -1 | grep -qv -- '--layers'"
check "... and recorded the build as the base of the next difference" "[ -s '$W/pushed/pc/layers.json' ]"
check "... and printed the commands" "grep -q 'iyakki dev update $PACK' '$W/lnout'"
rm -rf "$W/pushed"
check "nightly --layers fails when the layered image does not show its line" "! STUB_BOOT=nolayers NPL >$W/lnout2 2>&1 && grep -q 'did NOT see: root layers mounted: base core ui' '$W/lnout2'"
check "... and records nothing" "[ ! -e '$W/pushed/pc' ]"
unset IYAKKI_PUSHED_DIR

echo "$pass passed, $fail failed"
[ "$fail" = 0 ]
