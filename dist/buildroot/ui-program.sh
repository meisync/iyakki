#!/usr/bin/env bash
# Build the UI program `iyakki-ui` (F-SPLIT-03, docs/development/UI-BOUNDARY.md) for an image: the shell and the apps as their own static file
# with every service coming from the core over its UI socket (`--features socket-shell`). The file belongs to the ui layer (dist/buildroot/layers.txt).
#
#   dist/buildroot/ui-program.sh [--profile device|release|fast] [--target x86_64-unknown-linux-musl] [--print-command]
#
# Prints the path of the program on the last line. `--print-command` prints the cargo command and builds nothing (the test uses it). The image
# recipes take the file with `IYAKKI_UI_BIN=<that path> lean-image.sh ...`; the program is off unless the kernel parameter `iyakki.ui=process`
# (or the flag file written by `IYAKKI_UI_PROCESS=1`) asks the core to start it. Build ONE program at a time (docs/BUILD-PRACTICE.md).
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"; ROOT="$(cd "$HERE/../.." && pwd)"
PROFILE="${IYAKKI_PROFILE:-device}"; TARGET=x86_64-unknown-linux-musl; PRINT=0
while [ $# -gt 0 ]; do
  case "$1" in --profile) PROFILE="$2"; shift 2 ;; --target) TARGET="$2"; shift 2 ;; --print-command) PRINT=1; shift ;;
    *) echo "ui-program: unknown option $1" >&2; exit 2 ;; esac
done
if [ "$PROFILE" = release ]; then PFLAG=--release; else PFLAG="--profile $PROFILE"; fi
CMD="cargo build $PFLAG --target $TARGET -p iyakki-ui-bin --features socket-shell --locked"
OUT="${CARGO_TARGET_DIR:-$ROOT/target}/$TARGET/$PROFILE/iyakki-ui"
if [ "$PRINT" = 1 ]; then echo "$CMD"; echo "$OUT"; exit 0; fi
. "$HERE/linker.sh"; iyakki_export_rustflags "$TARGET"   # no builder path in the program
(cd "$ROOT" && $CMD)
[ -x "$OUT" ] || { echo "ui-program: no $OUT after the build" >&2; exit 1; }
echo "$OUT"
