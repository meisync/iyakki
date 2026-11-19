#!/bin/sh
# Ignored tests that check IYAKKI-made data and IYAKKI clients with the real programs everybody else uses; no root needed, nothing outside a scratch folder is touched.
#   dist/linux-host/real-tools-tests.sh            see docs/development/LINUX-GATE-HOST.md ("Real tools")
# iyakki-agent disks_tests: the partition tables (GPT and MBR) and FAT32 volumes our DISKS partition manager writes, read by sfdisk --verify, fsck.fat, blkid and mtools.
# iyakki-sshfs real_sshd_tests: our own SSH and SFTP client against a real OpenSSH sshd started as the normal user on a free 127.0.0.1 port (own host key, own authorized_keys).
# iyakki-vt real_ssh_tests: our terminal emulator and pseudo-terminal session driving the real OpenSSH client against the real sshd (command, resize, vi, top, Ctrl+C, exit status).
# iyakki-sshfs real_vsftpd_tests: our own FTP and explicit-TLS FTPS client against a real vsftpd started as the normal user (anonymous login, throw-away certificate). When vsftpd is
# not installed the Ubuntu/Debian package is DOWNLOADED (apt-get download, no root) and unpacked into a scratch folder; nothing is installed. IYAKKI_VSFTPD=/path/to/vsftpd uses that one.
# iyakki-agent voicehelper_browser and iyakki-remote client_browser, client_browser_media, window_browser: the browser voice helper page and the client and window pages in a real headless Chrome
# driven over the DevTools protocol by node (needs google-chrome and node 22; without them they are reported NOT RUN).
# Needs: sfdisk, fsck.fat, blkid, mdir (mtools), ssh, vi, top, sshd, ssh-keygen, openssl, and vsftpd or network access to the package archive. A tool that is missing makes the test skip itself:
# this script reports that as NOT RUN, never as passed.
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"; cd "$ROOT" || exit 2
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
SCRATCH=""
trap '[ -z "$SCRATCH" ] || rm -rf "$SCRATCH"' EXIT
if [ -z "${IYAKKI_VSFTPD:-}" ] && ! command -v vsftpd >/dev/null 2>&1 && [ ! -x /usr/sbin/vsftpd ]; then
  SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/iyakki-vsftpd-pkg.XXXXXX")"
  if (cd "$SCRATCH" && apt-get download vsftpd >/dev/null 2>&1 && dpkg -x vsftpd_*.deb root 2>/dev/null) && [ -x "$SCRATCH/root/usr/sbin/vsftpd" ]; then
    IYAKKI_VSFTPD="$SCRATCH/root/usr/sbin/vsftpd"; export IYAKKI_VSFTPD
    echo "real-tools-tests: vsftpd unpacked from the package archive into a scratch folder"
  else
    echo "real-tools-tests: vsftpd is not installed and could not be downloaded: the FTP tests will skip" >&2
  fi
fi
fail=0; skipped=0
run() { # crate pattern
  out="$(cargo test --locked -p "$1" --lib -- --ignored --nocapture --test-threads=1 "$2" 2>&1)"; rc=$?
  echo "$out" | grep -E "^test |test result|skipped" | cut -c1-200
  [ "$rc" = 0 ] || { echo "$out" | tail -25; fail=1; }
  echo "$out" | grep -q "skipped:" && skipped=1
  return 0
}
run iyakki-agent real_sfdisk_fsck_fat
run iyakki-sshfs real_sshd
run iyakki-sshfs real_vsftpd
run iyakki-vt real_ssh
runt() { # crate test-file (an ignored integration test that needs a real Chrome and node 22)
  if ! command -v google-chrome >/dev/null 2>&1 || ! command -v node >/dev/null 2>&1; then echo "real-tools-tests: $2 needs google-chrome and node: NOT RUN" >&2; skipped=1; return 0; fi
  out="$(cargo test --locked -p "$1" --test "$2" -- --ignored --nocapture --test-threads=1 2>&1)"; rc=$?
  echo "$out" | grep -E "^test |test result" | cut -c1-200
  [ "$rc" = 0 ] || { echo "$out" | tail -25; fail=1; }
  return 0
}
runt iyakki-agent voicehelper_browser
runt iyakki-remote client_browser
runt iyakki-remote client_browser_media
runt iyakki-remote window_browser
[ "$fail" = 0 ] || { echo "real-tools-tests: FAIL"; exit 1; }
[ "$skipped" = 0 ] || { echo "real-tools-tests: a test skipped itself: NOT RUN" >&2; exit 2; }
echo "real-tools-tests: PASS"
