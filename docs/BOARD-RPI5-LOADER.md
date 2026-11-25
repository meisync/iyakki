# Raspberry Pi 5 with the IYAKKI boot loader: first boot of the loader variant

**For:** the owner, after the first-boot test of the baseline card (`docs/BOARD-RPI5-FIRST-BOOT.md`). **Status 2026-11-24:** the card was built and its files booted in QEMU only (aarch64 edk2 instead of U-Boot); **it has never run on a Pi 5.** Everything below that says "you should see" is an expectation. Decision: `D-BOOT-ARM` in `docs/development/ALPHA-8-DECISIONS.md`.

## The chain

Pi 5 EEPROM, then `config.txt` with `kernel=u-boot.bin`, then **U-Boot** (the person's download, see 0), then U-Boot's distro boot finds `EFI/BOOT/BOOTAA64.EFI` (the loader) on the first FAT partition, then the loader reads `boot.cfg`, then the kernel `kernel8.img` and the RAM disk through the EFI stub. The baseline's `cmdline*.txt`, `iyakki-boot.txt` and `tryboot.txt` are not on this card: the loader's menu (second level) has the same modes, and its rollback (nine unconfirmed starts, then the entry `full` with the full RAM disk, then it waits) replaces the lean/full switch. The device tree comes from the firmware through U-Boot. Keep the baseline card as the fallback.

## 0. U-Boot (never shipped: you download it)

The card has no `u-boot.bin`. It is GPL-2.0-or-later and comes from a pinned Debian package (`u-boot-rpi 2025.01-3+deb13u1`, built from upstream U-Boot 2025.01 with the BCM2712 device tree, the EFI loader and EFI variables kept in a file on the card). The pin (taken 2026-11-24, `board-cfg` crate, `RPI5_UBOOT`):

* package: `https://deb.debian.org/debian/pool/main/u/u-boot/u-boot-rpi_2025.01-3+deb13u1_arm64.deb`, 540796 bytes, sha256 `c9e1aa3a17ccc1ff85858ace8cc43c917d1e0f70f04258438f1ae9b84b8b8c72`
* licence text of that version: `https://sources.debian.org/data/main/u/u-boot/2025.01-3+deb13u1/debian/copyright`, 906557 bytes, sha256 `303b441e3c744e78b8fdbf7388a869de978cf88afbf683ae02cdbecdc7f9efb3`. **Read it before you accept;** nobody at the project has.
* the file: `./usr/lib/u-boot/rpi_arm64/u-boot.bin` inside `data.tar.xz` of the package, 681256 bytes, sha256 `ec5a73ac3f8774445260d50acb60c43e581a2bc7036c2c8b3c8760f80f0fdffb`.

The button that does this inside IYAKKI is **not written yet** (`F-BOOTARM-06`). By hand, on a PC: download the package, compare size and sha256, `ar x` it, `tar -xJf data.tar.xz`, compare the sha256 of `u-boot.bin`, and copy it to the root of the card's first partition after step 1. If a later Debian update removes that exact package the download fails; the checksum then stops you, it never installs something else.

## 1. Write the card

```
iyakki-usb --list iyakki-rpi5-loader.img
sudo iyakki-usb --erase sdX iyakki-rpi5-loader.img      # as in the baseline guide; the image is a plain argument
```

Then put `u-boot.bin` (step 0) on the first partition. Serial console as in the baseline guide (3.3 V, 3-pin header, 115200 8N1).

## 2. What you should see

| Step | You should see | If not |
|---|---|---|
| a | the EEPROM lines, then U-Boot's banner (`U-Boot 2025.01`) on the serial console | nothing: `u-boot.bin` missing or the file not for the Pi 5; try the baseline card to prove the board |
| b | `Scanning ... mmc` and `Found EFI removable media binary efi/boot/bootaa64.efi`; U-Boot may pick another storage first (USB, network): remove other media | U-Boot lists no card: its SD numbering on the Pi 5 is unproven, send the lines |
| c | the loader: `boot: loader started`, `hw:` lines, `config: /boot.cfg read`, `state: tree=current tries_left=8`, `start: iyakki kernel=/kernel8.img` | the last loader line names the step that failed |
| d | the kernel and then the baseline's lines: `stage1: started`, `volumes found`, `root boot/root.sfs mounted`, `switching root` | as the baseline guide |
| e | the system on HDMI and Ethernet as in the baseline | |
| f | restart: `tries_left` is one lower unless the system confirmed the start | see the open question |

Stage 1 still writes `iyakki-boot.txt` after a failed fast start; on this card nobody reads that file, so a failed fast start restarts with the same entry until the loader's counter reaches `full` (nine starts). Send the log if that happens.

**Open questions:** (1) U-Boot's SD numbering and its EFI start on the Pi 5; (2) whether the running system can write `BootConfirm` through U-Boot's runtime variables (if not, the counter only goes down); (3) the display: U-Boot may hand a framebuffer to the EFI stub; the baseline's `vc4` overlay lines are kept. The loader's own log: `EFI/iyakki-boot/last-boot.log` on the card.

## What to send back

The whole serial log from power-on, a photo of the screen, the result of each row above, `last-boot.log`, and whether `ubootefi.var` appeared on the card. Checklist: `dist/autotest/checklists/F-BOOTARM-04.json`. To go back: write the baseline card.

## What happened on a real board (2026-11-25): not proven

On the same Pi 5 D0 board (see `BOARD-RPI5-FIRST-BOOT.md`) the loader never started, with every combination tried, each time with no loader log on the card (`EFI/iyakki-boot/last-boot.log` missing) and no picture:

* Debian U-Boot 2025.01 (`u-boot-rpi`), with our D0 device tree, with the display overlay on and off;
* U-Boot 2026.10-rc5 (Debian experimental), with our D0 device tree and with Raspberry Pi's own device trees and D0 overlay files;
* the community UEFI firmware for the Pi 5 (`worproject/rpi5-uefi` v0.3, March 2024, not updated since).

No serial adapter was connected, so the point where it stops is not known (KI-1042). The cause may be the D0 stepping: a 2026 U-Boot patch series adds a D0 device tree, and a forum report describes the same silent stop with U-Boot 2025.10 until the device tree from Raspberry Pi OS was used. The next step is a serial log of U-Boot. Until then the Pi 5 uses the direct start, which has no menu. The loader itself, the card format and the U-Boot route are proven on the KV260 (see `BOARD-KV260-LOADER.md`).
