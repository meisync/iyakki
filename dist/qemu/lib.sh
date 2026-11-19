# Shared helpers of the dist/qemu scripts (sourced, not run). Opt-in: nothing in ./run.sh or alpha-check calls these.
#   OUT     the build output folder with images/bzImage and images/rootfs.cpio.zst (first argument of each script)
#   QEMU    the emulator (default qemu-system-x86_64); KVM is used when /dev/kvm is writable, else TCG (slow: raise TIMEOUT)
#   TIMEOUT seconds one guest run may take (default 240)
here=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$here/../.." && pwd)
OUT="${1:-$ROOT/target/buildroot-libre}"
QEMU="${QEMU:-qemu-system-x86_64}"
WORK="${WORK:-$(mktemp -d "${TMPDIR:-/tmp}/iyakki-qemu.XXXXXX")}"
mkdir -p "$WORK"
CLEANUP=""
cleanup() { eval "$CLEANUP"; rm -rf "$WORK" 2>/dev/null || true; }
trap cleanup EXIT
fail() { echo "FAIL: $1"; CLEANUP="$CLEANUP"; trap - EXIT; eval "$CLEANUP"; echo "(work directory kept: $WORK)"; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || { echo "needs $1 (see dist/qemu/README.md)" >&2; exit 2; }; }
want() { tr -d '\r' < "$1" | grep -aq -- "$2" || fail "$3 (log: $1, wanted: $2)"; echo "ok: $3"; }
unwanted() { tr -d '\r' < "$1" | grep -aq -- "$2" && fail "$3 (log: $1, found: $2)"; echo "ok: $3"; return 0; }
check_images() {
  [ -f "$OUT/images/bzImage" ] && [ -f "$OUT/images/rootfs.cpio.zst" ] || { echo "no images in $OUT/images (build one first: ./run.sh build)" >&2; exit 2; }
  need "$QEMU"
  [ -w /dev/kvm ] || echo "note: no write access to /dev/kvm, running without KVM (slow; set TIMEOUT higher)"
}
# boot LOG "kernel args" [qemu args...]: direct kernel boot, serial on stdout, powers itself off when the self-test ends
boot() {
  log="$1"; args="$2"; shift 2
  timeout "${TIMEOUT:-240}" $QEMU -accel kvm -accel tcg -m "${MEM:-2048}" -smp 2 -nographic -no-reboot \
    -kernel "$OUT/images/bzImage" -initrd "$OUT/images/rootfs.cpio.zst" -append "console=ttyS0 panic=-1 $args" "$@" >"$log" 2>&1 || true
}
# result LOG MODE: the guest's own PASS/FAIL lines decide
result() { python3 "$here/selftest-result.py" "$1" "$2" || fail "the guest self-test $2 did not pass (log: $1)"; }
