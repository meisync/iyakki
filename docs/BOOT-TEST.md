# Testing IYAKKI on real hardware

This is the first cut for trying IYAKKI on your own machines: an Intel/AMD 64-bit PC (booted from USB) and an
Apple Silicon Mac mini (run in a virtual machine). **Nothing here has been tried on real hardware yet.** Only QEMU
(BIOS, UEFI without Secure Boot, and ARM64) was used. The first thing to run on any machine is the hardware check.

## What you need

| Machine | File | Where it is after a build |
|---|---|---|
| 64-bit PC (i7 and similar) | USB-bootable ISO | `target/buildroot-libre/images/rootfs.iso9660` |
| Apple Silicon Mac mini | UEFI disk image for a VM | `target/buildroot-libre-aarch64/images/iyakki-aarch64.img` |

Build them with `dist/buildroot/build.sh libre` and `ARCH=aarch64 dist/buildroot/build.sh libre`.

## 1. Intel/AMD PC from a USB stick

**Write the stick** with the IYAKKI USB writer (Linux and macOS):

    cargo build --release -p iyakki-usb          # once; the program is target/release/iyakki-usb
    sudo target/release/iyakki-usb               # finds the image, then:  ↑/↓ choose the drive, Enter, read the warning,
                                               # type the drive name to confirm, watch progress, wait for "verified"
    target/release/iyakki-usb --list             # no root needed: shows which drives are allowed

It asks whether to **install IYAKKI** on the stick (a real system: boot partition plus a data partition that keeps
your accounts and files; recommended) or **copy the live ISO** (runs from memory). `--install` / `--iso` skip the question.
It lists only USB drives (add `--all` for internal ones), always refuses the disk your system runs from, shows a
red erase warning, requires you to type the drive's name, and reads the stick back to check it. On Windows use
Rufus in **DD Image mode** or balenaEtcher for now (F-PKG-07). Manual fallback: `sudo dd if=rootfs.iso9660 of=/dev/sdX bs=4M conv=fsync status=progress`
(check the device with `lsblk` first).

**Firmware settings.** Turn **Secure Boot off** (IYAKKI images are not signed yet: F-BOOT-02). Legacy/CSM or
UEFI both work. Open the one-time boot menu (usually F12, F10, F8 or Esc) and pick the USB stick.

**Boot menu.** Choose **"IYAKKI - hardware check (keyboard, mouse, touch, display)"** first.

| Entry | Use |
|---|---|
| IYAKKI | normal start (login or first-run setup) |
| IYAKKI - hardware check | what the machine reports, plus a live tester |
| IYAKKI - safe graphics | if the screen stays black: no GPU drivers, simple framebuffer only |
| IYAKKI (verbose, boot self-test, power off) | prints diagnostics and powers off |

### What the hardware check shows

* **Attention** (top of the report): missing keyboard, mouse or display, only a touchpad, and so on.
* **Display**: framebuffer size, driver, and for each connected monitor its **name, native resolution and size**
  (read from the monitor's EDID). On a 4K monitor you should see `3840x2160` and a UI scale of 3.00.
* **Input devices**: `[keyboard]`, `[mouse]`, `[touchscreen] (multi-touch)`, `[touchpad]` with their device nodes.
* **Network, storage, audio, USB**: what the kernel found, including the USB device name of a touch monitor.
* **Live tester**: type, move the mouse, tap or click. Counters and the latest events appear on the right, and the
  pointer is drawn at its position. **Tap the eight squares** at the corners and edges: each turns green. A touch
  panel that is offset, mirrored or only covers part of a 4K screen shows up here.
* Keys: `Enter` continue to login · `S` save the report to `/var/lib/iyakki/hwreport.txt` (only survives if a data
  disk exists, see Settings → Storage; otherwise photograph the screen) · `R` refresh · `C` clear targets.
* Over a serial console or an SSH shell the same report is `iyakki hwreport`.

### Touch monitors

The touch layer travels on a separate USB cable from the video cable. If no `[touchscreen]` line appears, check
that cable first and look in the USB section for the monitor. IYAKKI reads single-touch and multi-touch panels
(first finger only); taps land where the finger is. Gestures (pinch, two-finger scroll) are not supported.

### If something goes wrong

| Symptom | Try |
|---|---|
| "Security Violation" or the stick is ignored | Secure Boot is on: turn it off. |
| Black screen after the menu | the "safe graphics" entry; then tell me the machine model and GPU. |
| No keyboard or mouse | another USB port (some ports sit behind a hub); note the report's USB list. |
| No WiFi | expected: the libre image has no firmware blobs (F-PKG-04). Use Ethernet or a USB tether. |
| Pointer or taps feel slow at 4K | note the resolution and CPU from the report. |

## 2. Apple Silicon Mac mini (M-series)

**IYAKKI cannot boot a Mac mini from USB.** Apple Silicon has no UEFI boot, and Linux runs on it only through the
Asahi project, which (as far as I know) supports M1/M2 but not M3/M4; check their current status before planning
anything (tracked as F-BOOT-09). The practical route is a **virtual machine**, which still lets you test the
display, scaling, mouse and keyboard handling:

**UTM (free, graphical):** New → Virtualize → Other → choose **UEFI Boot**, skip the ISO, add `iyakki-aarch64.img`
as a VirtIO drive, display **virtio-gpu-pci**, input **USB tablet**. Start the VM, pick the hardware-check entry in
the GRUB menu.

**QEMU from Homebrew (command line)**, untested on a Mac:

    qemu-system-aarch64 -M virt -accel hvf -cpu host -smp 4 -m 4G \
      -drive if=pflash,format=raw,readonly=on,file=$(brew --prefix)/share/qemu/edk2-aarch64-code.fd \
      -drive file=iyakki-aarch64.img,format=raw,if=virtio \
      -device virtio-gpu-pci -device qemu-xhci -device usb-kbd -device usb-tablet \
      -netdev user,id=n0 -device virtio-net-pci,netdev=n0

Notes: a touch monitor on the Mac reaches the VM as a **mouse pointer**, so you can check tap accuracy and
4K scaling but not multi-touch reporting. Retina settings in UTM change the VM's resolution; try both.

## 3. Results sheet

Copy this and fill it in (a photo of the hardware-check screen is enough for the rest):

| Check | i7 PC | Mac VM |
|---|---|---|
| Boots to the menu / hardware check | | |
| Machine, CPU, firmware (UEFI/BIOS) | | |
| Display name and resolution (1080p) | | |
| Display name and resolution (4K) | | |
| Keyboard found; every key types | | |
| Mouse found; moves and clicks | | |
| Touch screen found; 8/8 targets (1080p) | | |
| Touch screen found; 8/8 targets (4K) | | |
| Ethernet link and address | | |
| First-run setup and login work | | |
| Anything that felt slow | | |

Hardware compatibility results feed F-BOOT-01 in `FEATURES.md`.

## Boot screen (logo) check

`sudo python3 dist/buildroot/qemu-splash-test.py` boots the image with a virtual screen, takes screenshots while the
boot screen is up and checks that the logo, the name, the progress bar and the galaxy are drawn, that the picture
moves, and that pressing `L` shows the loading log. It saves the pictures and prints where. Without QEMU,
`cargo run --release -p iyakki-shell --example splashshot <dir>` writes pictures of the same screen at several sizes.
