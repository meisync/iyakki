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
