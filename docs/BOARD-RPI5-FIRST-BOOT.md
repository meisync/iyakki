# Raspberry Pi 5: first boot (the Monday test)

**For:** the owner, with a 16 GB Raspberry Pi 5, a microSD card, a serial adapter and an Ethernet cable. **You will get:** what to write to the card, how to connect the serial console, what you should see line by line, what is expected NOT to work yet, and exactly what to send back. **Status:** nothing here has been run on a real board. The same files were booted in QEMU (stage 1, the data volume and the agent start); QEMU cannot prove the board, so every board step below is a question, not a fact. Plan and reasons: [development/RASPBERRY-PI-PLAN.md](development/RASPBERRY-PI-PLAN.md). Hardware rows: [HARDWARE.md](HARDWARE.md).

## What you need

* The card image `iyakki-rpi5.img` (made by `dist/rpi/make-pi-image.sh`; its SHA-256 is in `image.sha256` beside it) and a microSD card of 16 GB or more. Everything on the card is erased.
* A computer with Linux or macOS and the `iyakki-usb` tool, to write the card.
* A USB serial adapter at **3.3 V** logic (never 5 V). The Pi 5's debug header is the small 3-pin connector between the two micro-HDMI ports (JST-SH). Which pin is TX, RX and GND: read the Raspberry Pi 5 documentation before you connect (I did not re-check it; a wrong wire does no harm at 3.3 V, swap RX and TX if the screen stays empty).
* Ethernet to your router (the on-board Wi-Fi is not expected to work on this first test, see "Not expected to work").
* A 5 V / 5 A (27 W) power supply, an HDMI display and a USB keyboard are welcome but not needed for the first look.

## 0. The board's own firmware (EEPROM): IYAKKI never touches it

A Pi 5 starts from its own EEPROM, which already knows how to read an SD card. Use a current EEPROM: boot Raspberry Pi OS once, or use Raspberry Pi Imager's "Bootloader" tools, and update it with Raspberry Pi's own tools. IYAKKI does not write the EEPROM, ever, and the card holds no Raspberry Pi boot firmware (a Pi 5 needs none). Boot order SD first is the default.

## 1. Write the card

On the computer (the SD slot or a USB reader; check the name first, the tool refuses system disks and mounted disks):

```
iyakki-usb --list iyakki-rpi5.img
sudo iyakki-usb --erase sdX iyakki-rpi5.img      # sdX: the card as listed (name only, no /dev/); on macOS diskN
```

It checks the image against `image.sha256` when that file sits next to it, writes, stretches the data partition to fill the card, and reads everything back. The card has two partitions: `IYAKKI-BOOT` (FAT32, 256 MB or more: `config.txt`, `cmdline.txt`, `iyakki-boot.txt`, `kernel8.img`, the device tree, the RAM disks and the seed root) and `IYAKKI-DATA` (ext4, formatted by the first start).

## 2. Connect the serial console

3.3 V adapter on the 3-pin header, 115200 baud, 8 data bits, no parity, 1 stop bit, no flow control. For example `screen /dev/ttyUSB0 115200` (Linux) or `screen /dev/tty.usbserial-XXXX 115200` (macOS). Start it BEFORE you power the board and keep the log: send it back.

## 3. Power on and what you should see

| Step | What you should see | If not |
|---|---|---|
| a | A moment after power: the bootloader prints its own lines on the serial console (we set `uart_2ndstage=1`). The activity LED blinks. | Nothing at all and the LED never blinks: wrong card write, or the EEPROM is old. Try the card in Raspberry Pi OS Imager's reader test; send the photo. |
| b | `Linux version 6.12.115-iyakki-rpi5` and kernel messages. | Bootloader lines but no kernel: the kernel or device tree did not load. Send the lines. |
| c | `stage1: started`, `stage1: volumes found at ... ms (boot: /dev/mmcblk0p1, data: /dev/mmcblk0p2)`, `stage1: boot volume mounted`. | `no IYAKKI-BOOT or IYAKKI-DATA volume appeared`: the SD controller driver did not come up. Send the lines. |
| d | `stage1: root boot/root.sfs mounted` and `stage1: switching root`. | `no usable system root`: stage 1 then rewrites `iyakki-boot.txt` to the full RAM disk and restarts in 30 s; the next start uses the full RAM disk by itself. |
| e | The system starts (IYAKKI banner or login). On HDMI you may see the boot picture and the shell: **the display is the least certain part** (vc4 on the Pi 5). | A black screen with a working serial console is a result, not a failure: send it. |
| f | The board gets an address by DHCP on Ethernet (shown on the serial console or your router's list). | No address: the Ethernet (RP1) did not come up. Send `dmesg` lines with `macb` or `rp1`. |

The first start formats the data partition (a few seconds). Restart once: the second start should say `stage1: data volume mounted`.

## 4. Choose another start (there is no boot menu on a Pi)

A Pi has no GRUB menu. With the card in your computer, copy one of `cmdline-hwcheck.txt`, `cmdline-safegraphics.txt`, `cmdline-selftest.txt`, `cmdline-ssh.txt`, `cmdline-web.txt` or `cmdline-seed.txt` over `cmdline.txt` (the last ignores updates and boots the seed root: the rollback of last resort). `cmdline-selftest.txt` runs the boot self-test and powers off: the quickest first run. To go back, copy `cmdline-normal.txt` over `cmdline.txt`. (A chat tool that switches the start from the running system is planned, not written.)

## Not expected to work on this first test

On-board Wi-Fi and Bluetooth (the firmware is a separate download: `iyakki-usb rpi firmware fetch --pack rpi5-wifi`, and the running system does not yet install it), sound (HDMI audio only, untested), camera (CSI), NVMe, the 16 KB page size (we use 4 KB on purpose), the full web engine (no aarch64 build), local models on the GPU (CPU only), hardware video decoding, fan and temperature display, trial-boot updates (`tryboot`, written as a plan, not tried), Secure Boot (a Pi is outside that promise).

## What to send back

1. The whole serial log from power-on to the point it stops (a text file).
2. A photo of the HDMI screen if one is connected (or "black").
3. The card's `boot/stage1.log` and `iyakki-lean-failed` if present (put the card in your computer; the boot volume mounts as FAT).
4. The Pi's board revision line from the first bootloader lines on the serial console, and the bootloader (EEPROM) version date.
5. For each row of the checklist (`dist/autotest/checklists/F-RPI-08.json`, in the Doctor tab or the offline page): pass, fail or cannot-test, with the machine named "other".
6. Whether the board got a network address, and how long from power-on to the first `stage1:` line and to the system.

## Safety

The write tool refuses a mounted disk, a system disk and any `mtd` flash device, asks you to type the disk name, and verifies what it wrote. Nothing on the card or in the image is a Broadcom or Raspberry Pi firmware file: `iyakki-usb rpi check` fails the build if one is there.

## What happened on real boards (2026-11-24 and 2026-11-25)

Board: Raspberry Pi 5 Model B Rev 1.1, 16 GB, which has the **D0** SoC stepping. No serial adapter was connected; the evidence is the screen and the kernel log that the first stage saves on the data volume (`stage1-kmsg.log`).

* **Direct start (the Pi firmware starts `kernel8.img`, no U-Boot, no loader): works.** It reaches the login screen. It needs the D0 device tree: with the C0 tree only, the kernel panics in `bcm2712_pull_config_set` about 2 seconds after it starts (KI-1030). The card needs `bcm2712d0-rpi-5-b.dtb` and `device_tree=bcm2712d0-rpi-5-b.dtb` in `config.txt`. The command line carries `cgroup_enable=memory cgroup_memory=1` (the default device tree turns the memory cgroup off, and k3s needs it).
* **Display.** With the display overlay off, the firmware frame buffer shows the boot picture and the shell. With `dtoverlay=vc4-kms-v3d-pi5` in `config.txt` the kernel starts and IYAKKI runs (ethernet at 1 Gbit/s, USB keyboard and mouse found), but `vc4-drm` never finishes: it stays in "deferred probe pending", no `/dev/fb0` appears, the shell runs headless and the monitor stays on the last kernel lines (KI-1041). On 2026-11-24 the same overlay in `config-user.txt` did give `/dev/dri` and both HDMI sound cards; what differs is not found. Without `overlay_map.dtb` on the card the plain name `vc4-kms-v3d` does nothing, so the Pi 5 name is needed (KI-1037).
* **Not proven:** sound through the GPU overlay on this card, Wi-Fi, the serial console, the menu route (see `BOARD-RPI5-LOADER.md`).
