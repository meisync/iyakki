# Using the iyakki-boot loader with another Linux (for example Alpine)

Status: written 2026-11-07 from the loader's source and its QEMU tests. The loader is not tied to IYAKKI: it reads a plain config file,
starts any Linux kernel that has an EFI stub, and offers a menu. **Not tried with Alpine or any other distribution on real hardware** (the
steps below are what the code does; the Alpine details are marked where they come from the distribution's usual layout and were not run here).
UEFI only (the BIOS front end does not exist yet).

## What you need

* A UEFI computer (or a virtual machine with OVMF).
* The loader program `boot-uefi.efi`, built from this folder:
  `cargo build --release --target x86_64-unknown-uefi -p boot-uefi` (needs the `x86_64-unknown-uefi` Rust target). The config file name is
  fixed in the program: `/boot.cfg` by default, or whatever `BOOT_CONFIG_PATH` was set to at build time (the IYAKKI image uses
  `/EFI/iyakki-boot/boot.cfg`).
* A kernel built with the **EFI stub** (`CONFIG_EFI_STUB=y`; Alpine, Debian, Fedora, Arch and the kernel.org defaults have it) and its initramfs.
* The kernel and initramfs **on a FAT volume** (normally the EFI system partition). The loader reads FAT only: it cannot read ext4, btrfs or
  the root file system, so files that sit only in `/boot` on an ext4 root are not visible to it (see "Keeping the copies current").

## 1. Put the files on the EFI system partition

Layout (names are yours; keep paths free of spaces and quotes, the config parser is strict):

```
/EFI/BOOT/BOOTX64.EFI        the loader (boot-uefi.efi), if it is the only boot program; or any path a firmware entry points at
/boot.cfg                    the config (path as built, see above)
/alpine/vmlinuz-lts          the kernel
/alpine/initramfs-lts        the initramfs
/splash.bmp                  optional logo (24 or 32 bit BMP)
```

Make it the boot program either by copying it to `\EFI\BOOT\BOOTX64.EFI` (the removable-media path every firmware tries) or by adding a
firmware entry, for example `efibootmgr --create --disk /dev/nvme0n1 --part 1 --label "Alpine (iyakki-boot)" --loader '\EFI\iyakki\boot-uefi.efi'`.
Keep the old boot program until you have started once through the new one.

## 2. Write `boot.cfg`

An Alpine example (root on a separate partition; replace the UUID with `blkid` output):

```
timeout 3
default alpine
tries 3
scan yes

entry alpine
  title Alpine Linux
  kind linux
  kernel /alpine/vmlinuz-lts
  initrd /alpine/initramfs-lts
  cmdline root=UUID=1111-2222-3333 rootfstype=ext4 modules=sd-mod,usb-storage,ext4 quiet
  default

entry alpine-previous
  title Alpine Linux (previous kernel)
  kind linux
  kernel /alpine/vmlinuz-lts.old
  initrd /alpine/initramfs-lts.old
  cmdline root=UUID=1111-2222-3333 rootfstype=ext4 modules=sd-mod,usb-storage,ext4
  more
  requires /alpine/vmlinuz-lts.old
```

What the keys do (full list at the top of `crates/boot-core/src/config.rs`):

| Key | Meaning |
|---|---|
| `timeout N` | seconds the countdown waits; `0` starts at once (hold a key at power-on, or press Esc/Space at the start, to open the menu) |
| `default NAME` | the entry started when nobody chooses |
| `tries N` | failed starts counted before the menu opens and waits (see "Confirming a good start") |
| `scan yes` | find other systems on the FAT volumes (Windows Boot Manager, other `.efi` loaders) and list them under "More options" |
| `splash PATH`, `splash-bg #RRGGBB`, `splash-ms N` | optional logo; with a logo the menu opens on the first key or touch |
| `entry NAME` | starts a block: `title`, `kind linux` (or `chain` for another `.efi`, `firmware` for the firmware setup), `kernel`, `initrd` (up to 4), `cmdline`, `default`, `hidden`, `more` (second level), `requires PATH` (leave the entry out unless the file exists), `order N`, `hold KEY` (a key held at power-on starts this entry) |
| `override ID` | rename, hide or reorder an item the scan found |
| `lock-cmdline yes` | the person cannot edit the command line from the menu |

`{tree}` in `kernel`/`initrd` becomes `current` or `previous` and is for distributions that keep two sets in two folders (IYAKKI does).
Other Linux does not need it.

## 3. Start it and check

1. Boot the computer, choose the loader's entry in the firmware boot menu (or let it be first).
2. The log of the last start is written to the boot volume next to the config (IYAKKI: `/EFI/iyakki-boot/last-boot.log`); it lists the items
   found, the choice, and the kernel line it started. Read it first when something does not start.
3. Try it in a virtual machine first: `tools/qemu-test.sh` shows how the repository's tests run the loader with OVMF and throwaway disks.

## 4. Secure Boot

* **Secure Boot off:** nothing to do.
* **Secure Boot on:** the firmware must trust the loader **and the kernel**. Alpine's and most distributions' kernels are signed with the
  distribution's key, which the firmware does not trust directly. Route: start the loader through `shim`, sign the loader and your kernel with
  your own key and enrol that key as a MOK. The loader asks shim to verify the kernel it starts. Steps, scripts (`tools/add-sbat.sh`,
  `tools/sign.sh`) and the QEMU test are in [SIGNING.md](SIGNING.md). Not tried with a distribution kernel.

## 5. Keeping the copies current (the part you must do for another distribution)

The loader does not follow a distribution's package manager. After every kernel or initramfs update the files on the FAT volume must be
refreshed, or the machine keeps starting the old kernel. For Alpine the usual way is:

* mount the EFI system partition at `/boot/efi` (or at `/boot`, so the kernel and initramfs are written straight onto FAT; then no copy is needed),
* keep the old pair as `*.old` before copying the new one (the example above has a "previous kernel" row for it),
* run the copy from a small hook that runs after the kernel package or `mkinitfs` (for example a script in `/etc/kernel-hooks.d/` on a
  distribution that has such a directory; check yours, this was not tested here).

Write the new files beside the old ones and rename them into place: a power cut during a copy must not leave a half kernel under the
name the config points at.

## 6. Confirming a good start (optional rollback)

The loader counts starts that were never confirmed: when the running system is up, it writes the UEFI variable `BootConfirm` (any content,
under the loader's vendor GUID, `crates/boot-uefi/src/vars.rs`); the loader reads it at the next start, refills the tries and deletes it.
After `tries` starts without a confirm the menu opens and waits. Another Linux can write the variable from a service started late in the
boot (for example with `efivar`/a few lines of code writing to `/sys/firmware/efi/efivars/`), or ignore the feature: set `tries` high
and the loader behaves as a plain menu.

## 7. Limits (said plainly)

* No BIOS/legacy start. No reading of ext4, btrfs, LUKS or LVM: the kernel and initramfs must be on FAT.
* No GRUB-style scripting, `search` or `chainloader` commands: entries are fixed lines, not a language.
* It starts Linux kernels with an EFI stub and `.efi` programs; it does not start BSD or other kernels directly.
* Tested only in QEMU/OVMF and on IYAKKI's own machines; the Alpine example above is what the config format allows, not a recorded run.

Licence: MIT OR Apache-2.0 (see the licence files in this folder).
