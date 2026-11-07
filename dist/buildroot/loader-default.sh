#!/bin/sh
# Make IYAKKI's own boot loader (iyakki-boot) the default of a UEFI boot volume tree, GRUB kept as the Recovery entry.
# docs/development/GRUB-REPLACEMENT-PLAN.md step 2, docs/BOOT-LOADER.md. Only run when IYAKKI_LOADER_DEFAULT=1 (the image steps decide).
#   loader-default.sh esp ROOT          ROOT is a boot volume tree with EFI/iyakki-boot/loader.efi (boot-loader-stage.sh) and the GRUB layout
#                                       exit 0: done; exit 3: nothing to do (no staged loader: GRUB stays the default, say so)
#   loader-default.sh grubcfg FILE      drop the GRUB test entry "new boot loader (test)" from a grub.cfg (loader-default images)
# What it does to ROOT/EFI:
#   shim path (EFI/BOOT/grubx64.efi exists: secureboot.sh stage-esp made it)
#       grubx64.efi -> grub-recovery.efi          GRUB, already signed, moved to the name the loader's Recovery row chain-loads
#       grubx64.efi := the loader                  shim's second stage by name; SBAT section added and signed with the image key
#   plain path (no shim: EFI/BOOT/bootx64.efi is GRUB)
#       bootx64.efi -> grub-recovery.efi
#       BOOTX64.EFI := the loader                  the firmware (a plain PC, the Mac's picker) starts it directly; signed when a key is configured,
#                                                  otherwise UNSIGNED and then it starts with Secure Boot OFF only
#   EFI/iyakki-boot/loader.efi := the same bytes   the updater's copy (loader.prev.efi is made beside it by `iyakki dev update`)
# The signing uses the repo's flow only: secureboot.sh loader-image, with IYAKKI_SB_KEY / IYAKKI_SB_CERT from the environment (never read here).
# Running it twice is fine: when grub-recovery.efi is there already only the loader is refreshed (lean-image.sh does that after it staged a
# loader that was made for the lean RAM disk).
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
c="${1:-}"; [ $# -eq 0 ] || shift
case "$c" in
  grubcfg)
    f="${1:?usage: loader-default.sh grubcfg FILE}"
    grep -q '^[[:space:]]*# BEGIN loader-test-entry' "$f" || { echo "loader-default: $f has no loader test entry to drop" >&2; exit 0; }
    sed -i '/^[[:space:]]*# BEGIN loader-test-entry/,/^[[:space:]]*# END loader-test-entry/d' "$f"
    ! grep -q 'new boot loader (test)' "$f" || { echo "loader-default: the test entry is still in $f" >&2; exit 1; }
    ;;
  esp)
    ROOT="${1:?usage: loader-default.sh esp ROOT}"
    B="$ROOT/EFI/BOOT"; L="$ROOT/EFI/iyakki-boot/loader.efi"
    if [ ! -s "$L" ]; then echo "loader-default: no loader staged in $ROOT/EFI/iyakki-boot (boot-loader-stage.sh found no program): GRUB stays the default" >&2; exit 3; fi
    [ -d "$B" ] || { echo "loader-default: $B missing" >&2; exit 1; }
    W="$(mktemp -d)"; trap 'rm -rf "$W"' EXIT
    # the loader to install: signed with the image key when one is configured, else unsigned
    if [ -n "${IYAKKI_SB_KEY:-}" ] && [ -n "${IYAKKI_SB_CERT:-}" ]; then
      sh "$HERE/secureboot.sh" loader-image "$L" "$W/loader.efi" >&2 || { echo "loader-default: signing the loader failed" >&2; exit 1; }
      SIGNED=1
    else
      cp "$L" "$W/loader.efi"; SIGNED=0
    fi
    if [ -f "$B/grubx64.efi" ]; then
      SECOND="grubx64.efi"   # shim path
      [ "$SIGNED" = 1 ] || echo "loader-default: NO signing key configured: the loader is UNSIGNED, shim will refuse it with Secure Boot ON (set IYAKKI_SB_KEY and IYAKKI_SB_CERT)" >&2
      [ -f "$B/grub-recovery.efi" ] || mv "$B/grubx64.efi" "$B/grub-recovery.efi"
    else
      SECOND="BOOTX64.EFI"   # plain path
      for g in bootx64.efi BOOTX64.EFI; do
        if [ -f "$B/$g" ] && [ ! -f "$B/grub-recovery.efi" ]; then mv "$B/$g" "$B/grub-recovery.efi"; fi
      done
      [ -f "$B/grub-recovery.efi" ] || { echo "loader-default: no GRUB found in $B to keep as grub-recovery.efi" >&2; exit 1; }
      rm -f "$B/bootx64.efi" "$B/BOOTX64.EFI"
      [ "$SIGNED" = 1 ] || echo "loader-default: NO signing key configured: the loader is UNSIGNED and starts with Secure Boot OFF only" >&2
    fi
    cp "$W/loader.efi" "$B/$SECOND"
    cp "$W/loader.efi" "$L"
    cmp -s "$B/$SECOND" "$L" && [ -s "$B/grub-recovery.efi" ] || { echo "loader-default: the layout is not consistent" >&2; exit 1; }
    grep -q '^entry recovery' "$ROOT/EFI/iyakki-boot/boot.cfg" || { echo "loader-default: boot.cfg has no Recovery row" >&2; exit 1; }
    echo "loader-default: $B/$SECOND is the loader ($([ "$SIGNED" = 1 ] && echo signed || echo UNSIGNED)); GRUB is $B/grub-recovery.efi"
    ;;
  *) sed -n '2,3p;5,6p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2 ;;
esac
