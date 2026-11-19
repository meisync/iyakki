#!/usr/bin/env bash
# Self-test of the temp-folder guard in env.sh (KI-060): a run that stays inside TMPDIR passes and its folder is removed; a run that
# leaves an iyakki-* entry in the (fake) system /tmp fails. Uses a throwaway folder, never the real /tmp.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
w="$(mktemp -d)"; trap 'rm -rf "$w"' EXIT
export IYAKKI_SYS_TMP="$w/sys" IYAKKI_TMPDIR="$w/tmp"; mkdir -p "$IYAKKI_SYS_TMP"
. "$here/env.sh"
iyakki_run_tmp_begin; mkdir "$TMPDIR/iyakki-a" "$TMPDIR/iyakki-b"; d="$IYAKKI_RUN_TMP"
iyakki_run_tmp_end >/dev/null || { echo "FAIL: a clean run was refused"; exit 1; }
[ ! -e "$d" ] || { echo "FAIL: the run folder was not removed"; exit 1; }
iyakki_run_tmp_begin; mkdir "$IYAKKI_SYS_TMP/iyakki-leak"
if iyakki_run_tmp_end >/dev/null 2>&1; then echo "FAIL: growth of the system /tmp was not detected"; exit 1; fi
echo "temp-folder guard: ok"
