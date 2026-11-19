#!/usr/bin/env bash
# NIGHTLY PUSH for a lean install: build, pack, boot-test, and PRINT the commands. It never sends anything to a device.
#
#   nightly-push.sh -o PACKDIR [--version V] [--level 19] [--wait SECONDS] [--no-cargo] [--layers] [--publish] [--device-ui NAME] [-- extra quick-push.sh arguments]
#
#   1. quick-push.sh --cargo --lean --lean-level N -o PACKDIR  (device-profile build, repack, lean image, pack from the lean image; signed:
#      IYAKKI_SB_KEY and IYAKKI_SB_CERT must be set, quick-push.sh refuses to start without them). N is 19 here (a nightly may take its time;
#      the pack is smaller); `--level 3` is the quick development setting. `--no-cargo` uses the program already built.
#   2. The lean image is booted in QEMU through the real chain (UEFI, GRUB, kernel, first stage): dist/buildroot/qemu-lean-test.py IMAGE
#      --entry 1 --serial-only. The serial console MUST show `root boot/root.sfs mounted` and `switching root`; if either line is missing
#      the script stops with a non-zero exit code and prints no command (the boot-test rule: a new image is booted before it goes anywhere).
#   3. It prints the exact `iyakki dev update PACKDIR` and `iyakki dev reboot` commands for the person to run, and nothing else touches a device:
#      this script has no network step, no `dev update`, no `dev reboot`, no ssh. (tests: dist/buildroot/test-quick-push.sh checks that.)
#   --publish: AFTER the boot test passed, also PRINT the command that shares the pack with the team (dist/release/publish-build.sh,
#      docs/development/BUILD-SHARING.md). Printed only; this script never runs it.
#
#   --layers (docs/development/LAYERED-ROOT-DESIGN.md): the layers are built once (lean-image.sh --layers inside quick-push.sh), the pack is a v2
#      pack made against the last offered build (IYAKKI_PUSHED_DIR, default ~/iyakki-work/pushed-layers/pc; not in the repository), BOTH boot tests
#      run (the layered image: `root layers mounted: base core ui`; and the single-root image made from the same system: `root boot/root.sfs
#      mounted`), the build is then recorded as the base of the next difference, and the pack sizes are printed. Nothing is sent to a device.
#
#   --device-ui NAME: after the boot test, ALSO print (never run) the command that drives the paired device NAME through the screen scenarios of
#      dist/devtest (screenshots, checks, an HTML report) once the person has installed the pack and the device is back at Home. Still no network step here.
#
# Variables: IYAKKI_SB_KEY, IYAKKI_SB_CERT (required), IYAKKI_PUSH_OUT (the image folder, default target/buildroot-pc-push),
# IYAKKI_QEMU_TEST (path of the boot-test script, for tests), IYAKKI_IYAKKI (the iyakki program to print in the commands, default `iyakki`).
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="${IYAKKI_ROOT:-$(cd "$HERE/../.." && pwd)}"
# a full disk crashed the editor and the agents on 2026-11-12: refuse to start a build with less than 40 GB free
free_gb="$(df -Pk "$ROOT" | awk 'NR==2 {print int($4/1048576)}')"
[ "${free_gb:-999}" -ge "${IYAKKI_MIN_FREE_GB:-40}" ] || { echo "REFUSED: only ${free_gb} GB free on the disk of $ROOT (need ${IYAKKI_MIN_FREE_GB:-40})" >&2; exit 1; }
PACK=""; VER=""; LEVEL=19; WAIT=60; CARGO=--cargo; LAYERS=0; PUBLISH=0; DEVUI=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) PACK="$2"; shift ;;
    --version) VER="$2"; shift ;;
    --level) LEVEL="$2"; shift ;;
    --wait) WAIT="$2"; shift ;;
    --no-cargo) CARGO="" ;;
    --layers) LAYERS=1 ;;
    --publish) PUBLISH=1 ;;
    --device-ui) DEVUI="$2"; shift ;;
    --) shift; break ;;
    -h|--help) sed -n '2,/^set -eu/p' "$0" | sed '$d'; exit 0 ;;
    *) echo "nightly-push.sh: unknown option $1" >&2; exit 2 ;;
  esac
  shift
done
[ -n "$PACK" ] || { echo "nightly-push.sh: -o PACKDIR is required" >&2; exit 2; }
[ -n "${IYAKKI_SB_KEY:-}" ] && [ -n "${IYAKKI_SB_CERT:-}" ] || { echo "nightly-push.sh: IYAKKI_SB_KEY and IYAKKI_SB_CERT must be set (an unsigned update is refused by the device)" >&2; exit 2; }
OUTDIR="${IYAKKI_PUSH_OUT:-$ROOT/target/buildroot-pc-push}"
QEMU="${IYAKKI_QEMU_TEST:-$HERE/qemu-lean-test.py}"
IYK="${IYAKKI_IYAKKI:-iyakki}"
t0=$(date +%s)
step() { echo "nightly-push: [$(( $(date +%s) - t0 )) s] $*"; }

QP=(--lean --lean-level "$LEVEL" --out "$OUTDIR" -o "$PACK")
[ "$LAYERS" = 0 ] || QP+=(--layers)
[ -z "$CARGO" ] || QP=("$CARGO" "${QP[@]}")
[ -z "$VER" ] || QP+=(--version "$VER")
step "build and pack (quick-push.sh ${QP[*]})"
IYAKKI_QUICK_PUSH_NO_HINT=1 "$HERE/quick-push.sh" "${QP[@]}" "$@"

IMG="$OUTDIR/lean/iyakki-lean-x86_64.img"
[ -s "$IMG" ] || { echo "nightly-push.sh: $IMG is missing after quick-push.sh" >&2; exit 1; }
step "boot test in QEMU (serial console only, $WAIT s after Enter on the lean entry)"
LOG="$(mktemp "${TMPDIR:-$HOME/.cache}/nightly-boot.XXXXXX")"; trap 'rm -f "$LOG"' EXIT
# one boot test: IMAGE, the lines it must show. The test exits 0 even when the boot failed in --serial-only mode: the lines are the verdict.
boot_test() {
  local img="$1"; shift
  python3 "$QEMU" "$img" --entry 1 --serial-only --wait "$WAIT" >"$LOG" 2>&1 || { echo "nightly-push.sh: the QEMU test itself failed:" >&2; tail -20 "$LOG" >&2; exit 1; }
  local miss=0 want
  for want in "$@"; do
    if grep -q "$want" "$LOG"; then echo "nightly-push: boot test saw: $want"; else echo "nightly-push: boot test did NOT see: $want" >&2; miss=1; fi
  done
  if [ "$miss" = 1 ]; then
    echo "nightly-push.sh: the image did not boot to the real root in QEMU: NOT offering it. Serial lines:" >&2
    grep -E "stage1|lean|panic|Kernel" "$LOG" | tail -30 >&2 || true
    exit 1
  fi
}
if [ "$LAYERS" = 1 ]; then
  step "boot test 1: the layered image"
  boot_test "$IMG" "root layers mounted: base core ui" "switching root"
  # the single-root path must keep working: the same system as one root.sfs, in its own image (lean-image.sh without --layers)
  SINGLE="$OUTDIR/lean-single"
  step "building the single-root image of the same system for boot test 2 ($SINGLE)"
  SI=(--root sfs --level "$LEVEL"); [ -z "${IYAKKI_STAGE1:-}" ] || SI+=(--stage1 "$IYAKKI_STAGE1")
  "$HERE/lean-image.sh" "$OUTDIR/images" "$SINGLE" "${SI[@]}"
  step "boot test 2: the single-root image"
  boot_test "$SINGLE/iyakki-lean-x86_64.img" "root boot/root.sfs mounted" "switching root"
else
  boot_test "$IMG" "root boot/root.sfs mounted" "switching root"
fi
step "boot test passed; nothing was sent to a device"
if [ "$LAYERS" = 1 ]; then
  # record this build as the base of the next difference (layers.json + the .sfs files, hard links: lean-image.sh replaces its own folder next time)
  REC="${IYAKKI_PUSHED_DIR:-$HOME/iyakki-work/pushed-layers}/pc"
  rm -rf "$REC.new"; mkdir -p "$REC.new"
  cp -al "$OUTDIR/lean/layers/." "$REC.new/" 2>/dev/null || cp -a "$OUTDIR/lean/layers/." "$REC.new/"
  rm -rf "$REC.old"; [ ! -d "$REC" ] || mv "$REC" "$REC.old"; mv "$REC.new" "$REC"; rm -rf "$REC.old"
  step "recorded $REC as the base of the next difference pack (if you do not push this pack, delete that folder)"
  echo "nightly-push: pack sizes:"; du -sh "$PACK"; ls -l "$PACK/boot/layers" 2>/dev/null | awk 'NR>1 {printf "  %8.1f MB  %s\n", $5/1048576, $9}'
fi
cat <<EOF

The pack is in $PACK (signed, lean image, level $LEVEL). To install it, the person runs, on the paired computer:

  $IYK dev update $PACK
  $IYK dev reboot

(each asks for the on-device approval; this script has done neither.)
EOF
if [ "$PUBLISH" = 1 ]; then
  cat <<EOF

To share this boot-tested pack with the team (the person runs it; the update key and the publish token stay on this PC):

  $ROOT/dist/release/publish-build.sh $PACK --variant pc-x86_64 --channel dev --build-id nightly-$(date +%m%d)-$(git -C "$ROOT" rev-parse --short=7 HEAD 2>/dev/null || echo local) --store http://HOST:30580

(HOST is this PC's address; see docs/development/BUILD-SHARING.md. Only the newest build per machine type and channel is kept.)
EOF
fi
if [ -n "$DEVUI" ]; then
  cat <<EOF

After the person has installed the pack and $DEVUI shows the Home screen (signed in), the screen scenarios are run from this computer with:

  ./run.sh device-ui --device $DEVUI --out \$HOME/iyakki-work/devtest-runs/$(date +%Y%m%d-%H%M)

(screenshots, checks and report.html land in that folder; this script has not run it.)
EOF
fi
