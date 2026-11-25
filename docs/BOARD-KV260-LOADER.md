# AMD Kria KV260 with the IYAKKI boot loader: first boot of the loader variant

**For:** the owner, after the first-boot test of the baseline card (`docs/BOARD-KV260-FIRST-BOOT.md`). **Status 2026-11-24:** the card was built and booted in QEMU only (the aarch64 edk2 firmware instead of the board's U-Boot); **it has never run on a KV260.** "You should see" is what we expect. Decision and background: `D-BOOT-ARM` in `docs/development/ALPHA-8-DECISIONS.md`.

## What is different from the baseline card

The baseline card has `extlinux/extlinux.conf` and the board's U-Boot starts the kernel from it. The loader card has **no** `extlinux.conf`: the board's U-Boot (its order is extlinux, then `boot.scr`, then the EFI removable-media file, `docs/development/KV260-PLAN.md`) finds `EFI/BOOT/BOOTAA64.EFI`, which is `iyakki-boot` (`iyakki-boot/` repository). The loader reads `boot.cfg` on the card, counts starts that were never confirmed, and starts the same `Image` and `rootfs.cpio.gz` through the kernel's EFI stub. The kernel gets U-Boot's own device tree (no `fdt` line, as in the baseline's first entry). Keep the baseline card: it is the fallback.

## What you need

The same as the baseline test (board, 12 V supply, microSD, micro-USB serial cable, Ethernet, optional DisplayPort), and the image `iyakki-kv260-loader.img` (made next to the baseline by `iyakki-boot/tools/make-arm-variant.sh kv260 BASELINE.img OUT.img`; its SHA-256 is in the build report).

## 1. Write the card

```
sudo dist/buildroot/kv260-write-sd.sh iyakki-kv260-loader.img /dev/sdX     # the WHOLE card; ends with "read-back: the first ... bytes match"
```

## 2. Serial console (115200 8N1, the second of the four ports is usually the console) and power on

You should see, in this order:

* U-Boot lines, then `Scanning mmc 1:1...`. It will **not** say `Found /extlinux/extlinux.conf`; then it tries the script and the EFI path and prints something like `Found EFI removable media binary efi/boot/bootaa64.efi`, `Booting /efi\boot\bootaa64.efi`.
* the loader's lines: `boot: loader started`, a `hw:` summary (firmware, memory, disks, screen), `config: /boot.cfg read`, `item: 0 iyakki ...`, `state: tree=current tries_left=8`, `decide: start iyakki (default)`, `start: iyakki kernel=/Image tree=current`.
* the Linux text on `ttyPS1`, then the IYAKKI lines.

## 3. Checks, in this order

| # | Check | You should see | If not |
|---|---|---|---|
| 1 | U-Boot reaches the loader | the `bootaa64.efi` line above | U-Boot ran the baseline path or the network: write down what it printed; the firmware may be older than 2022 |
| 2 | The loader runs | `boot: loader started`, `config: /boot.cfg read` | the loader printed nothing: an EFI start problem in U-Boot, send the lines before it |
| 3 | The kernel starts | `start: iyakki kernel=/Image`, then `Linux version` | send the loader's last line (it names the failing step) |
| 4 | The system | Ethernet address, DisplayPort shell, USB keyboard (as in the baseline checks) | if the monitor stays black, the baseline's second entry (device tree from the card) has no twin here: say so |
| 5 | The counter | after a reboot the line says `tries_left` one lower unless the system confirmed the start | this is the open question: see below |
| 6 | The loader's log | on the card, `EFI/iyakki-boot/last-boot.log` (mount the card on a PC) | |

**The open question (check 5).** After a good start the running system writes the UEFI variable `BootConfirm`; the loader reads it at the next start and refills the tries. U-Boot's EFI variables are kept in a file on the card (`ubootefi.var`) for the loader's own writes; whether the running kernel can write the variable at run time through U-Boot is **unknown**. If it cannot, the counter goes down at every start: after 9 starts the loader waits with its menu (no second entry exists for the KV260). Send the `tries_left` lines of three restarts.

## What to send back

The whole serial log from power-on; the first U-Boot lines (firmware year); the result of each check (works / does not / not tried); `EFI/iyakki-boot/last-boot.log` and whether a file `ubootefi.var` appeared on the card; the carrier card's label (rev A or B).
Checklist for the Doctor tab: `dist/autotest/checklists/F-BOOTARM-03.json`.

To go back to the board's baseline: write the baseline card again. Nothing here writes the board's flash.

## Result on a real board (2026-11-25): works, with limits

Board: AMD Kria KV260 starter kit, carrier card rev B, the board's own U-Boot 2023.01 from the SOM's flash. The card had no `extlinux.conf`; the serial console showed this order:

1. U-Boot scans the SD card and finds `efi/boot/bootaa64.efi`, then starts it.
2. The loader starts, reads `boot.cfg`, lists the hardware and the disks (the SD card and its boot partition are readable), and starts the default entry.
3. The loader hands the kernel the device tree named by the entry's `fdt` line (rev B: `/dtb/zynqmp-smk-k26-revA-sck-kv-g-revB.dtb`). The kernel's EFI stub prints "Using DTB from configuration table", Linux 6.12.40 boots, `zynqmp-dpsub` probes at 1920x1080, and IYAKKI starts (the kernel took 5.7 s, IYAKKI about 2 s more, ethernet came up).

Limits found:

* **No picture from the loader.** U-Boot gives no graphics output ("no graphics output protocol"), so the loader's logo and menu do not show and a key press cannot choose an entry on a screen. The default entry starts by itself; to choose another one, the menu is only readable on the serial console.
* **Without the `fdt` line the display fails**: the board's own tree gives a `zynqmp-dpsub` probe error and IYAKKI runs headless (KI-1031). Every KV260 entry in the generated `boot.cfg` now carries the line.
* **The loader cannot tell the boards apart.** On this U-Boot the machine name is "Unknown", so a card that holds entries for several boards (an attempt with the Pi 5 and the KV260 on one card) must have the right default entry for the board it is plugged into. The row for the universal card (F-BOOT-47) stays open.
* The screen itself was not looked at in this run; the display is judged from the kernel log.
