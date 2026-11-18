#!/bin/sh
# Cache of the pruned and signed kernel-modules + firmware part of a repack (docs/development/COMPILE-SPEED-PLAN.md, "Measured results").
# Sourced by repack.sh (and by test-repack-cache.sh); nothing runs when it is sourced.
#
# repack.sh used to unpack the donor's /lib/modules, copy the whole linux-firmware set, sign every kernel module and prune 3 233 firmware files
# (1 280 MiB) on EVERY run, although none of that depends on the IYAKKI program. This file names the cache that holds the result:
#   rc_key    prints the cache key (a hash of every input that decides the result), or prints nothing and returns 1 when the cache must not be used
#   rc_file   prints the cache file for the key
#   rc_trim   keeps the newest two cache files of a variant, removes the older ones
# The cache is a zstd-compressed tar of lib/modules and lib/firmware taken by post-build.sh right after the prune (so the same restricted-firmware
# strip, licence texts and release stamps still run on a restore). Where: $IYAKKI_REPACK_CACHE, default $ROOT/target/repack-cache
# (never /tmp). IYAKKI_REPACK_CACHE=0 turns it off. A cold cache behaves exactly like before: the run does the work and saves the result.
#
# Inputs read from the caller's variables: HERE ROOT VARIANT IMAGE_PROFILE BASE IMG KFROM FWFROM OVERLAYS; and IYAKKI_SB_KEY / IYAKKI_SB_CERT.

rc_dir() {
  case "${IYAKKI_REPACK_CACHE:-}" in
    0|off|no) return 1 ;;
    "") printf '%s\n' "$ROOT/target/repack-cache" ;;
    *) printf '%s\n' "$IYAKKI_REPACK_CACHE" ;;
  esac
}

# a file's identity without reading it: name, size, modification time
rc_stat() { for f in "$@"; do [ -e "$f" ] && stat -c '%n %s %Y' "$f" || echo "missing $f"; done; }
# a file's content hash
rc_sum() { for f in "$@"; do if [ -f "$f" ]; then sha256sum < "$f" | cut -d' ' -f1; else echo "missing $f"; fi; done; }

rc_key() {
  [ -n "$(rc_dir)" ] || return 1
  # a release image has no vendor firmware to prune and its donor is read differently: no cache
  [ "${IMAGE_PROFILE:-}" != release ] || return 1
  # an overlay that ships its own firmware or modules would have to be pruned after the restore: no cache then
  for d in $OVERLAYS; do
    if [ -d "$d" ] && find "$d" \( -path '*/lib/firmware*' -o -path '*/lib/modules*' \) -print -quit 2>/dev/null | grep -q .; then return 1; fi
  done
  {
    echo "rc-v1 variant=$VARIANT"
    if [ -n "${KFROM:-}" ]; then
      # modules come from the donor's packed rootfs, plus the out-of-tree ones its target/ may hold newer
      rc_stat "$KFROM/images/rootfs.cpio"
      find "$KFROM/target/lib/modules" -path '*/updates/*.ko' -printf '%p %s %T@\n' 2>/dev/null | LC_ALL=C sort
    else
      rc_stat "$IMG/rootfs.cpio.zst"
    fi
    if [ -n "${FWFROM:-}" ]; then echo "fwfrom=$FWFROM"; rc_sum "$FWFROM/WHENCE" "$FWFROM/copy-firmware.sh"; fi
    # the prune logic and what it keeps
    rc_sum "$HERE/board/iyakki/prune-firmware.sh" "$HERE/board/iyakki/iwlwifi-keep.sh" "$HERE/board/iyakki/pc-firmware.keep" \
           "$HERE/board/iyakki/mac-firmware.keep" "$HERE/board/iyakki/post-build.sh"
    # the signing: the certificate and key decide the signature bytes, the tool decides how
    if [ -n "${IYAKKI_SB_KEY:-}" ] && [ -n "${IYAKKI_SB_CERT:-}" ]; then
      echo "signed"; rc_sum "$IYAKKI_SB_CERT" "$IYAKKI_SB_KEY"
      rc_stat ${IYAKKI_SIGN_FILE:-} $(ls "${KFROM:-$BASE}"/build/linux-[0-9]*/scripts/sign-file "$BASE"/build/linux-[0-9]*/scripts/sign-file 2>/dev/null | head -n1)
    else
      echo "unsigned"
    fi
  } | sha256sum | cut -c1-24
}

rc_file() { printf '%s/fwmod-%s-%s.tar.zst\n' "$(rc_dir)" "$VARIANT" "$1"; }

rc_trim() {
  d="$(rc_dir)" || return 0
  # shellcheck disable=SC2012
  ls -1t "$d"/fwmod-"$VARIANT"-*.tar.zst 2>/dev/null | tail -n +3 | while IFS= read -r old; do rm -f "$old"; done
}
