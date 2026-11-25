# AMD Kria KV260: the first-boot test (about 30 minutes)

**For:** the owner, with the board, a microSD card, a micro-USB cable, an Ethernet cable and a serial program. **Status 2026-11-21:** the image was built and its userspace booted in QEMU; **it has never run on a KV260.** Everything below that says "you should see" is what we expect, not something observed. Background and what is verified: `docs/development/KV260-PLAN.md`.

## What you need

* The KV260 Starter Kit with its power supply (12 V). A microSD card (8 GB or more; **everything on it is erased**). A card reader on a PC.
* The micro-USB cable from the board's **UART/JTAG** connector to the PC (an FTDI chip, shows up as several serial ports). A terminal program (115200 baud, 8 data bits, no parity, 1 stop bit, **no flow control**).
* Ethernet cable to a router with DHCP. A DisplayPort monitor and cable (optional for step 3).
* The image `iyakki-kv260.img` (from the build: `ARCH=aarch64 dist/buildroot/build.sh kv260`; the path is in the build report).
* Nothing from AMD to download for this test. If the board's own boot firmware is very old (the first U-Boot lines on the serial port show a year before 2022), update it first with AMD's own procedure; IYAKKI never writes the board's flash.

## 1. Write the card (on a PC)

```
sudo dist/buildroot/kv260-write-sd.sh iyakki-kv260.img /dev/sdX     # the WHOLE card, check the name with lsblk first
```

The script refuses system disks, mounted cards, partitions, NVMe and anything larger than 256 GB; asks you to type the device name; writes, reads the card back and compares. It must end with `read-back: the first ... bytes match`. Eject the card.

## 2. Serial console and first power-on

1. Put the card in the board's microSD slot (it is on the carrier card; AMD's Getting Started guide shows the place). Connect Ethernet. Connect the micro-USB cable. Open the serial ports the PC shows (Linux: `/dev/ttyUSB0` to `ttyUSB3`; the board's PS UART console is **usually the second one**, try each: the right one prints the boot text). Settings: 115200 8N1, no flow control.
2. Plug in the power. You should see, in this order (from the upstream U-Boot script and our menu; wording may differ):
   * U-Boot lines, then `Scanning mmc 1:1...`, `Found /extlinux/extlinux.conf`
   * our menu `IYAKKI (AMD Kria KV260)` with a 3 second timer, the first entry (the device tree for carrier card rev B) is the default; if your carrier card is rev A, pick the rev A entry
   * `Retrieving file: /Image`, `Retrieving file: /rootfs.cpio.gz`, `Starting kernel ...`
   * the Linux boot text on **`ttyPS1`**, then the IYAKKI lines (`Booting IYAKKI`, `supervisor up`).
3. If it does not boot, write down the last lines you see (see "What to send back"). If U-Boot boots something else (a different OS from the board's flash or the network), the SD card was not found: check the card, try another card, check the card is in the slot all the way.

## 3. What to check, in this order

| # | Check | You should see | If not |
|---|---|---|---|
| 1 | Serial console | U-Boot, the menu, Linux text, IYAKKI start | see "what to send back" |
| 2 | Ethernet | the boot text names an `eth0` link and an address from DHCP (`iyakki hwreport` or the sign-in screen later) | try the other cable/port; the board is 1 Gb |
| 3 | DisplayPort | the monitor shows the IYAKKI start screen (the shell) | the boot menu has two fall-back entries with the kernel's own device tree (`IYAKKI (device tree from the SD card)` and `... rev A`): arrow keys at the 3-second menu on the serial console, try them |
| 4 | USB keyboard | typing moves the shell (use a USB port; the board has a hub) | |
| 5 | Shutdown | power off from the shell or hold the button, then power on again: it boots the same way | |

Not part of this test: Wi-Fi and Bluetooth (the board has none), sound, camera, the programmable logic, local models (4 GB: use a remote model host).

## What to send back

* The whole serial log, from power-on to where it stopped or worked (copy it from the terminal program), as a text file.
* The first U-Boot lines (they show the firmware year) and the line `Machine model:` if it shows.
* The result of each check above: works / does not / not tried. The checklist file `dist/autotest/checklists/F-KV260-01.json` has the same steps for the Doctor tab or the offline page.
* A photo of the carrier card's label (it says rev A or rev B: it decides which device tree fits), and whether the monitor stayed black.

Nothing in this test writes to the board's flash. To go back to the board's own boot, remove the card and power-cycle.

## Update 2026-11-25: the card menu and the display

* The generated `extlinux` menu now starts "IYAKKI (carrier card rev B, default)", which passes the rev B device tree from the card. The board's own tree is a separate entry, because it gives no display on carrier card rev B (KI-1031).
* The data partition on the card is a 1 MiB marker, not a file system; the first start formats it.
