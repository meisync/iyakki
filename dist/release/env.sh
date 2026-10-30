#!/usr/bin/env bash
# Shared by the release scripts (source it): the repository root and a disk-backed TMPDIR.
# Tests and QEMU scripts leak temporary directories; on a RAM-backed or small /tmp that runs out of space or inodes
# ("df -i /tmp" full). TMPDIR is therefore a folder on the disk, and old iyakki-* entries are removed at the end.
RELDIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$RELDIR/../.." && pwd)"
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
if [ -z "${IYAKKI_TMP_SET:-}" ]; then
  export TMPDIR="${IYAKKI_TMPDIR:-$ROOT/target/tmp}"
  mkdir -p "$TMPDIR" 2>/dev/null || { export TMPDIR="$HOME/.cache/iyakki-tmp"; mkdir -p "$TMPDIR"; }
  export IYAKKI_TMP_SET=1
fi
# remove iyakki-* entries older than a day from TMPDIR (and /tmp when it is ours to clean)
iyakki_tmp_clean() {
  find "$TMPDIR" -maxdepth 1 -name 'iyakki-*' -mtime +0 -exec rm -rf {} + 2>/dev/null || true
}
# percent of inodes used on a path ("" when unknown)
inode_pct() { df -i "${1:-/tmp}" 2>/dev/null | tail -n1 | awk '{gsub(/%/, "", $5); print $5}' | tr -dc '0-9'; }

# KI-060: tests must leave nothing behind. A test run gets its OWN folder inside TMPDIR (iyakki_run_tmp_begin), removed whole at the end
# (iyakki_run_tmp_end); the entries "iyakki-*" of the system /tmp are counted before and after, and growth fails the step (a test that
# ignores TMPDIR and writes to /tmp itself). Usage:  iyakki_run_tmp_begin; cargo test ...; iyakki_run_tmp_end   (the end returns 1 on growth)
iyakki_count_sys_tmp() { find "${IYAKKI_SYS_TMP:-/tmp}" -maxdepth 1 -name 'iyakki-*' 2>/dev/null | wc -l; }
iyakki_run_tmp_begin() {
  IYAKKI_TMP_BEFORE="$(iyakki_count_sys_tmp)"
  IYAKKI_RUN_TMP="$(mktemp -d "$TMPDIR/run-XXXXXX")"
  export IYAKKI_RUN_TMP_PREV="$TMPDIR" TMPDIR="$IYAKKI_RUN_TMP"
}
iyakki_run_tmp_end() {
  local left after rc=0
  left="$(find "$IYAKKI_RUN_TMP" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l)"
  rm -rf "$IYAKKI_RUN_TMP"
  export TMPDIR="$IYAKKI_RUN_TMP_PREV"
  after="$(iyakki_count_sys_tmp)"
  echo "test temp folders: $left were left in the run folder (removed); system /tmp iyakki-* entries: $IYAKKI_TMP_BEFORE before, $after after"
  if [ "$after" -gt "$IYAKKI_TMP_BEFORE" ]; then
    echo "FAIL: the tests left $((after - IYAKKI_TMP_BEFORE)) new iyakki-* entries in the system /tmp: they ignore TMPDIR" >&2
    rc=1
  fi
  return $rc
}
