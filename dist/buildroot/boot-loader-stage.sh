#!/bin/sh
# Lay the extra boot loader (iyakki-boot, docs/BOOT-LOADER.md) into a boot volume tree: ROOT/EFI/iyakki-boot/{loader.efi,boot.cfg,splash.bmp}.
#   boot-loader-stage.sh ROOT [INITRD-PATH]     INITRD-PATH is the RAM disk on the boot volume (default /boot/initrd; the lean image: /boot/initrd-lean)
# The program is `iyakki-boot` built once with (the file name of the config is compiled in):
#   (cd iyakki-boot && BOOT_CONFIG_PATH=/EFI/iyakki-boot/boot.cfg cargo build --release -p boot-uefi --target x86_64-unknown-uefi)
# or IYAKKI_BOOT_EFI=/path/to/that/boot-uefi.efi. Without a built program nothing is staged and the image has no extra entry (GRUB hides it
# when /EFI/iyakki-boot/loader.efi is missing), so a build that never built the loader works as before. It is NOT signed here: under Secure
# Boot it must be signed with the image's key (iyakki-boot/tools/sign.sh INPUT OUTPUT --key K --cert C); unsigned it starts only with Secure Boot off.
# The menu: the first level is "IYAKKI" (default), "Install to a disk" (only on install media: the file EFI/iyakki-boot/install-media, which update packs and installed disks leave out) and "More options"; every other start of board/iyakki/grub.cfg is on the second level, from the
# ENTRIES table below (id | level | title | kernel command line exactly as in grub.cfg after the kernel path, without $iyakki_usbq). The table is
# the ONE list the loader's second level is made from; crates/iyakki-core/tests/boot_menu.rs checks that every GRUB entry has a twin in it, so
# the two menus cannot drift apart. Added after the table: "Previous version" (the loader before the last update, EFI/iyakki-boot/loader.prev.efi),
# "Recovery (GRUB menu)" (the GRUB kept as EFI/BOOT/grub-recovery.efi on a loader-default image), "Full start" (lean images only), "UEFI firmware
# setup". The previous version of the SYSTEM ("IYAKKI (previous version)", two rows, shown on the first level only when the update kept one on this
# boot volume: EFI/iyakki-boot/prev/{bzImage,initrd} plus the marker set-plain, or set-oldroot when the update also replaced the system image and
# the old one must start with iyakki.root=previous; docs/BOOT-LOADER.md) is the same menu row as GRUB's entry of that name, and the default entry
# names both in `previous`, so an update that is never confirmed good falls back to it by itself. Each of those is shown only when its file / the firmware feature exists (`requires`, kind firmware). The Compatible USB mode
# variable of GRUB (iyakki_usbq, Settings -> Storage) is not carried over; the explicit "Compatible USB mode" entry is.
# The splash is the IYAKKI emblem on black (board/iyakki/boot-splash.py).
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"; REPO="$(cd "$HERE/../.." && pwd)"
ROOT="${1:-}"; INITRD="${2:-/boot/initrd}"
SCAN="${IYAKKI_LOADER_SCAN:-yes}"   # yes (owner 2026-11-04): the loader also lists other systems it finds on the machine under More options (not tried on real hardware yet); no turns it off
[ -d "$ROOT" ] || { sed -n '2,3p' "$0" >&2; exit 2; }
EFI="${IYAKKI_BOOT_EFI:-$REPO/iyakki-boot/target/x86_64-unknown-uefi/release/boot-uefi.efi}"
if [ ! -s "$EFI" ]; then echo "boot-loader-stage: no loader program ($EFI): the image gets no extra boot loader entry" >&2; exit 0; fi
# id | level | title (at most 64 characters) | kernel command line
ENTRIES='
iyakki|top|IYAKKI|console=tty0 console=ttyS0,115200 panic=20 quiet loglevel=3 vt.global_cursor_default=0 initcall_blacklist=hpet_init,crb_acpi_driver_init
install|top|Install to a disk|console=tty0 console=ttyS0,115200 panic=20 quiet loglevel=3 vt.global_cursor_default=0 iyakki.install=1 initcall_blacklist=hpet_init,crb_acpi_driver_init
safe|more|Safe graphics (no GPU drivers)|console=tty0 console=ttyS0,115200 panic=20 quiet loglevel=3 vt.global_cursor_default=0 nomodeset modprobe.blacklist=i915,xe,amdgpu,radeon,nouveau initcall_blacklist=hpet_init,crb_acpi_driver_init
screentest|more|Screen test (no simple framebuffer)|console=tty0 console=ttyS0,115200 panic=20 quiet loglevel=3 vt.global_cursor_default=0 initcall_blacklist=hpet_init,crb_acpi_driver_init,simpledrm_platform_driver_init
ssh|more|Remote diagnostics (SSH, keys only)|console=tty0 console=ttyS0,115200 panic=20 quiet loglevel=3 vt.global_cursor_default=0 iyakki.ssh=1 initcall_blacklist=hpet_init,crb_acpi_driver_init
usbcompat|more|Compatible USB mode (no UAS)|console=tty0 console=ttyS0,115200 panic=20 quiet loglevel=3 vt.global_cursor_default=0 usb-storage.quirks=04e8:4001:u initcall_blacklist=hpet_init,crb_acpi_driver_init
trouble1|more|Trouble 1: early messages on the firmware screen|console=tty0 console=ttyS0,115200 panic=20 ignore_loglevel loglevel=8 earlycon=efifb keep_bootcon initcall_blacklist=hpet_init,crb_acpi_driver_init
trouble2|more|Trouble 2: messages, PCI and power saving off|console=tty0 console=ttyS0,115200 panic=20 ignore_loglevel loglevel=8 earlycon=efifb keep_bootcon pci=realloc,nocrs pcie_aspm=off intel_idle.max_cstate=1 initcall_blacklist=hpet_init,crb_acpi_driver_init
trouble3|more|Trouble 3: messages, one CPU, no APIC timer|console=tty0 console=ttyS0,115200 panic=20 ignore_loglevel loglevel=8 earlycon=efifb keep_bootcon nosmp noapic nolapic_timer initcall_blacklist=hpet_init,crb_acpi_driver_init
trouble4|more|Trouble 4: messages, ACPI limited|console=tty0 console=ttyS0,115200 panic=20 ignore_loglevel loglevel=8 earlycon=efifb keep_bootcon acpi=noirq pci=nomsi initcall_blacklist=hpet_init,crb_acpi_driver_init
trouble5|more|Trouble 5: safe graphics, all messages|console=ttyS0,115200 console=tty0 panic=0 ignore_loglevel loglevel=8 nomodeset modprobe.blacklist=i915,xe,amdgpu,radeon,nouveau initcall_blacklist=hpet_init,crb_acpi_driver_init
verbose|more|Show boot messages|console=tty0 console=ttyS0,115200 panic=20 initcall_blacklist=hpet_init,crb_acpi_driver_init
hwcheck|more|Hardware check (keyboard, mouse, touch, display)|console=tty0 console=ttyS0,115200 panic=20 quiet loglevel=3 vt.global_cursor_default=0 iyakki.hwcheck=1 initcall_blacklist=hpet_init,crb_acpi_driver_init
selftest|more|Boot self-test (verbose, powers off)|console=tty0 console=ttyS0,115200 panic=20 iyakki.selftest=1 initcall_blacklist=hpet_init,crb_acpi_driver_init
nvidia|more|NVIDIA driver (nouveau off)|console=tty0 console=ttyS0,115200 panic=20 quiet loglevel=3 vt.global_cursor_default=0 modprobe.blacklist=nouveau initcall_blacklist=hpet_init,crb_acpi_driver_init
web|more|Also in a browser (HTTPS on port 8443)|console=tty0 console=ttyS0,115200 panic=20 quiet loglevel=3 vt.global_cursor_default=0 iyakki.web=1 initcall_blacklist=hpet_init,crb_acpi_driver_init
touch-probe|more|Touch screen: probe the monitor at start|console=tty0 console=ttyS0,115200 panic=20 quiet loglevel=3 vt.global_cursor_default=0 iyakki.touchscreen=probe initcall_blacklist=hpet_init,crb_acpi_driver_init
touch-nomst|more|Touch test: i915 without DP-MST|console=tty0 console=ttyS0,115200 panic=20 quiet loglevel=3 vt.global_cursor_default=0 i915.enable_dp_mst=0 initcall_blacklist=hpet_init,crb_acpi_driver_init
touch-nopsr|more|Touch test: i915 without power saving|console=tty0 console=ttyS0,115200 panic=20 quiet loglevel=3 vt.global_cursor_default=0 i915.enable_psr=0 i915.enable_fbc=0 i915.enable_dc=0 initcall_blacklist=hpet_init,crb_acpi_driver_init
touch-all4|more|Touch test: i915 all four switches|console=tty0 console=ttyS0,115200 panic=20 quiet loglevel=3 vt.global_cursor_default=0 i915.enable_dp_mst=0 i915.enable_psr=0 i915.enable_fbc=0 i915.enable_dc=0 initcall_blacklist=hpet_init,crb_acpi_driver_init
touch-nobs|more|Touch test: no boot screen|console=tty0 console=ttyS0,115200 panic=20 quiet loglevel=3 vt.global_cursor_default=0 iyakki.bootscreen=off initcall_blacklist=hpet_init,crb_acpi_driver_init
touch-fbskip|more|Touch test: boot screen leaves display open|console=tty0 console=ttyS0,115200 panic=20 quiet loglevel=3 vt.global_cursor_default=0 iyakki.fbset=skip initcall_blacklist=hpet_init,crb_acpi_driver_init
touch-full|more|Touch screen: full probe (screen may blink)|console=tty0 console=ttyS0,115200 panic=20 quiet loglevel=3 vt.global_cursor_default=0 iyakki.touchscreen=full initcall_blacklist=hpet_init,crb_acpi_driver_init

'
D="$ROOT/EFI/iyakki-boot"; mkdir -p "$D"
# the program must have been built with BOOT_CONFIG_PATH=/EFI/iyakki-boot/boot.cfg (see the top): a loader built without it looks for /boot.cfg, finds nothing
# and shows a built-in "current / seed" menu instead of ours (it happened once, 2026-11-04). Refuse it rather than ship it.
if ! grep -aq '/EFI/iyakki-boot/boot\.cfg' "$EFI"; then
	echo "boot-loader-stage: $EFI was not built with BOOT_CONFIG_PATH=/EFI/iyakki-boot/boot.cfg: no loader is staged (rebuild it as the top of this file says)" >&2
	exit 1
fi
cp "$EFI" "$D/loader.efi"
[ "${IYAKKI_LOADER_INSTALL_MEDIA:-1}" = 1 ] && printf 'install media: shows "Install to a disk" on the first level of the loader menu\n' > "$D/install-media"
python3 -I "$HERE/board/iyakki/boot-splash.py" "$REPO/crates/iyakki-shell/assets/logo.png" "$D/splash.bmp"
{
cat <<CFG
# IYAKKI's boot loader. Written by dist/buildroot/boot-loader-stage.sh (the entries come from its ENTRIES table, the same starts as
# board/iyakki/grub.cfg). The first level lists the "level top" entries and "More options"; "level more" entries are on the second level.
# IYAKKI confirms a good start by itself (it writes the BootConfirm variable once the sign-in screen is up), so a start that never gets
# there is counted; after "tries" of them the loader starts the kept previous version by itself (no menu), and if there is none, or it fails
# "tries" times too, the menu opens and waits. The paths have no {tree}: the files started are always the same ones.
timeout 0
scan $SCAN
tries 9
splash /EFI/iyakki-boot/splash.bmp
splash-bg #000000
splash-ms 1200
CFG
printf '%s\n' "$ENTRIES" | while IFS='|' read -r id level title cmd; do
	[ -n "$id" ] || continue
	printf '\nentry %s\n  title %s\n  kind linux\n  kernel /boot/bzImage\n  initrd %s\n  cmdline %s\n' "$id" "$title" "$INITRD" "$cmd"
	[ "$level" = more ] && printf '  more\n'
	if [ "$id" = iyakki ]; then printf '  default\n  previous prev-plain prev-oldroot\n'; fi
	# "Install to a disk" is on the first level only on install media: the marker file below is left out of update packs and of installed disks
	[ "$id" = install ] && printf '  requires /EFI/iyakki-boot/install-media\n'
	# the previous version of the system: first level, shown only when the update kept one (the marker file names which of the two it is)
	if [ "$id" = iyakki ]; then
		printf '\nentry prev-plain\n  title IYAKKI (previous version)\n  kind linux\n  kernel /EFI/iyakki-boot/prev/bzImage\n  initrd /EFI/iyakki-boot/prev/initrd\n  cmdline %s\n  requires /EFI/iyakki-boot/prev/set-plain\n' "$cmd"
		printf '\nentry prev-oldroot\n  title IYAKKI (previous version)\n  kind linux\n  kernel /EFI/iyakki-boot/prev/bzImage\n  initrd /EFI/iyakki-boot/prev/initrd\n  cmdline %s iyakki.root=previous\n  requires /EFI/iyakki-boot/prev/set-oldroot\n' "$cmd"
	fi
done
# the full RAM disk start: only when the boot volume has both RAM disks (a lean image: INITRD is the lean one)
if [ "$INITRD" != /boot/initrd ]; then
	first=$(printf '%s\n' "$ENTRIES" | grep -m1 '^iyakki|' | cut -d'|' -f4)
	printf '\nentry full\n  title Full start (slower; if the fast start does not work)\n  kind linux\n  kernel /boot/bzImage\n  initrd /boot/initrd\n  cmdline %s\n  more\n  requires /boot/initrd\n' "$first"
fi
cat <<CFG

entry previous
  title Previous version of the boot loader (if this one has a fault)
  kind chain
  kernel /EFI/iyakki-boot/loader.prev.efi
  requires /EFI/iyakki-boot/loader.prev.efi
  more

entry recovery
  title Recovery (GRUB menu)
  kind chain
  kernel /EFI/BOOT/grub-recovery.efi
  requires /EFI/BOOT/grub-recovery.efi
  more

entry firmware
  title UEFI firmware setup
  kind firmware
  more
CFG
} > "$D/boot.cfg"
echo "boot-loader-stage: $D (loader.efi, boot.cfg with $INITRD, splash.bmp)"
