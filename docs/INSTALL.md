# Installing IYAKKI

**For:** anyone who downloaded IYAKKI 0.1.0-alpha.1 and wants to run it.
**You will get:** IYAKKI running in one of three ways (a desktop window, a USB stick, a virtual machine), plus the
upgrade, backup and recovery steps.

> **Alpha.** The USB and VM paths were tested in QEMU only (BIOS and UEFI without Secure Boot). Nothing has been booted
> on real hardware yet. See [`../KNOWN-ISSUES.md`](../KNOWN-ISSUES.md) and [`BOOT-TEST.md`](BOOT-TEST.md).
> Zram (compressed memory) and the on-device installer are covered in [`ZRAM-AND-INSTALL.md`](ZRAM-AND-INSTALL.md);
> hardware notes are in [`HARDWARE.md`](HARDWARE.md). Both are written by the release team and may land after this file.

Contents: [Choose a way](#choose-a-way) · [1 Window mode](#1-window-mode-linux) · [2 USB stick](#2-usb-stick) ·
[3 Install to a disk](#3-install-to-a-computers-disk) · [4 Virtual machine](#4-virtual-machine) ·
[5 Variants](#5-variants-web-mac-arm64) · [6 Memory and zram](#6-memory-zram-and-boot-options) ·
[7 Upgrade, backup, uninstall](#7-upgrade-back-up-uninstall-recover) · [8 Boot problems](#8-boot-problems)

## Choose a way

| Way | Needs | Good for | Changes your computer? |
|---|---|---|---|
| **Window mode** | Linux desktop, Rust toolchain | Trying everything in 10 minutes | No (data in `~/.local/share/iyakki-dev`) |
| **USB stick, live or installed** | 64-bit PC, a USB stick (erased), the ISO or install image | The real OS on real hardware | Only the stick |
| **Virtual machine** | QEMU (Linux) or UTM/QEMU (Mac) | Testing without risk | No |

The images are produced by `./run.sh build` (see [`CONTRIBUTING.md`](../CONTRIBUTING.md)) or downloaded from the release
page. File names: `iyakki-x86_64.iso` (live ISO, about 60 MB), `iyakki-install-x86_64.img` (install image, about 270 MB),
`iyakki-aarch64.img` (ARM64 VM disk). A release also carries `SHA256SUMS` and a signature: check with
`iyakki release verify DIR --pubkey HEX` (the public key is printed on the release page).

## 1. Window mode (Linux)

IYAKKI as an ordinary window on your desktop. Fastest way to try it; it uses your real network and files, but shows
only **virtual disks**, never your real drives.

**Prerequisites**

* Rust (rustup) and a C toolchain; the window uses eframe, which needs the usual desktop libraries (Wayland or X11,
  xkbcommon, OpenGL). The first build downloads crates and takes several minutes.
* Optional, for the **full web engine** in BROWSER (press `F` in the tile): `python3-gi gir1.2-webkit2-4.1
  gir1.2-gtk-3.0` (WebKitGTK), and `xvfb` for video. Without them BROWSER uses its built-in reader, which handles
  articles and simple pages but not big web apps ([`FULL-BROWSER.md`](FULL-BROWSER.md)).
* Optional: `podman` (or `docker`) and a local k3s cluster for the AI IDE deploy flow; Node.js 18+ for installing Claude
  Code from the CLI tile; a speech engine for TALES and read-aloud ([`USER-GUIDE.md`](USER-GUIDE.md#voice)).

**Run**

```
git clone <repository> && cd <repository>
./run.sh window            # builds crates/iyakki-gui in release mode and opens the window
./run.sh window --real-disks   # also show this computer's real drives in Settings -> Storage (careful)
./run.sh disk add 8G | list | rm vda   # make or remove the window's virtual disks
```

* Data folder: `~/.local/share/iyakki-dev` (override with `IYAKKI_DEV_DIR=/path`). Reset everything:
  `rm -rf ~/.local/share/iyakki-dev`.
* `iyakki-desktop --version` prints the version. Panics are logged to `<data folder>/crash.log` and the window carries on.
* A second window on the same data folder asks "IYAKKI is already running on this data folder" before it continues.

**First run.** The setup screen asks for an administrator name (lowercase letters, digits, `-`, `_`) and a password of
at least 8 characters, twice. Then you sign in. `o` on the launcher signs out. Chat needs a model: see
[`QUICKSTART.md`](QUICKSTART.md#2-connect-a-model).

**Display notes.** The window works on Wayland and X11. If it does not open on a Wayland-only session, try
`WINIT_UNIX_BACKEND=x11 ./run.sh window` (standard winit variable; unverified here). The text size is set in
Settings -> Account / Display.

## 2. USB stick

`iyakki-usb` writes an image to a stick safely, on **Linux and macOS**. On Windows use Rufus in DD Image mode or
balenaEtcher for now (unverified).

```
cargo build --release -p iyakki-usb           # once; the program is target/release/iyakki-usb
target/release/iyakki-usb --list              # no root: shows the drives and whether each may be written
sudo target/release/iyakki-usb [IMAGE]        # choose with arrow keys, confirm, write, verify
```

Without `IMAGE` it uses an image in the current folder. **Live ISO or install?** With both images available and no
flag, you are asked.

| Option | Meaning |
|---|---|
| `--install` | Real partitioned system: FAT32 boot partition `IYAKKI-BOOT` (GRUB for BIOS and UEFI, kernel, initramfs) plus a data partition filling the rest of the stick. The data partition is formatted as the **IYAKKI-DATA** ext4 volume on first boot, so accounts and files survive power-off. Recommended. |
| `--table auto\|gpt\|mbr` | With `--install`: the partition table (default `auto`). **GPT** = BIOS boot partition, EFI System Partition `IYAKKI-BOOT` (FAT32) and a data partition filling the rest (can pass 2 TiB; 512 and 4096-byte sectors); boots on legacy BIOS and on UEFI. **MBR** = the classic table (data partition up to 2 TiB, 512-byte sectors), boots on any old PC. **auto** = GPT for drives over 2 TiB, drives with 4096-byte sectors, or when this computer was started through UEFI; MBR otherwise. The warning shows which one and why. |
| `--iso` | Copy the live ISO. Read-only, runs from RAM, loses everything at power-off unless a data disk is set up (Settings -> Storage). |
| `--erase <drive>` | No menu: write to this drive (`/dev/sdb`, `disk4`). Naming it is the confirmation; the warning is still printed. |
| `--all` | Also offer internal disks (never the one holding the running system). |
| `--no-verify` | Skip reading the stick back (not recommended). |
| `--no-zero-tail` | Do not wipe the last MiB of the stick. |
| `-h`, `-V` | Help, version. |

**What happens.** Only USB drives are listed (unless `--all`). The system disk, any disk with the running system mounted,
write-protected disks, swap and the disk holding the image file are always refused. You see a red **THIS WILL ERASE THE
WHOLE DRIVE** warning with the drive, model, size, mounted volumes and (for `--install`) the partition layout, and you
must **type the drive name** (`sdb`, or `/dev/sdb`) and press Enter; Enter alone cancels. After writing, the stick is read
back and checked ("verified").

**macOS.** Same program; it lists external disks and unmounts them with `diskutil`. Run it with `sudo`. Use the
`diskN` name. Intel Macs boot the stick with the Option key; Apple silicon Macs cannot boot it at all
([`MACOS.md`](MACOS.md), [`MAC-HARDWARE.md`](MAC-HARDWARE.md)). `iyakki-usb` on macOS is written but its macOS paths
have not been run on a Mac (unverified).

**Manual fallback (Linux):** `sudo dd if=iyakki-x86_64.iso of=/dev/sdX bs=4M conv=fsync status=progress` after checking
the device with `lsblk`. Wrong device means lost data.

**Booting the stick.** Turn **Secure Boot off** (or, for an image the owner signed, enrol its key once: `docs/SECURE-BOOT.md`). Open the one-time boot menu (usually F12, F10,
F8 or Esc; Option on Intel Macs) and pick the stick. The GRUB menu offers:

| Entry | Use |
|---|---|
| IYAKKI | normal start (setup or sign-in) |
| IYAKKI - show boot messages | kernel text instead of the boot screen |
| IYAKKI - hardware check (keyboard, mouse, touch, display) | first thing to run on a new machine (`iyakki.hwcheck=1`) |
| IYAKKI - safe graphics | black screen? no GPU drivers, simple framebuffer |
| IYAKKI (verbose, boot self-test, power off) | diagnostics, then powers off |
| IYAKKI - remote diagnostics (SSH, keys only) | `iyakki.ssh=1`; only works in an image built with your public key |
| IYAKKI - also in a browser (HTTPS on port 8443) | `iyakki.web=1` |

The menu now sits over the IYAKKI logo (text-only on a machine whose firmware cannot draw it) and the default entry starts after 3 seconds. First boot of an installed stick formats the data partition (a few seconds).

## 3. Install to a computer's disk

What is verified in this tree: `iyakki-usb --all --install` can write the install image to an **internal** disk of the
machine you run it on (never the disk of the running system), so the way to install onto a PC's disk today is:

1. Put the target disk in (or attach it by USB adapter) to a Linux or macOS computer.
2. `sudo target/release/iyakki-usb --all --install iyakki-install-x86_64.img`, pick the disk, read the warning, type its
   name. **Everything on that disk is erased**: the result is `IYAKKI-BOOT` (FAT32, about 256 MB) and the IYAKKI data
   partition (rest of the disk, formatted on first boot), with an MBR or a GPT (which also gets a 1 MiB BIOS boot
   partition) as `--table` says; add `--table gpt` for a disk over 2 TiB or with 4096-byte sectors (`auto` does that by
   itself).
3. Put the disk in the target PC, turn Secure Boot off, boot. The first boot formats the data partition, then setup asks
   for the administrator.

An installer that runs **on** the target machine from the USB stick is described in
[`ZRAM-AND-INSTALL.md`](ZRAM-AND-INSTALL.md): Settings, System, Install to disk, or `iyakki install`. It has the same
**Partition table: Auto / GPT / MBR** choice, shows the table the disk has now and the layout it will get, and verifies both
copies of a GPT after writing. GPT installs were booted in QEMU under legacy BIOS (SeaBIOS) and UEFI (OVMF), also with
4096-byte sectors under UEFI; no real computer has been tried yet.
Dual-boot is not supported: the disk is used whole.

## 4. Virtual machine

QEMU commands used by the repository (`run.sh`, after `./run.sh build`; Linux host, `qemu-system-x86_64`):

```
./run.sh vm            # ISO in QEMU: BIOS + GRUB + kernel + iyakki as PID 1 (1.5 GB RAM, 2 CPUs, KVM if available)
./run.sh vm-direct     # skip GRUB (faster); add --ssh to reach it on 127.0.0.1:2222 (needs an image built with your key)
./run.sh vm-disk       # a 4 GB virtual disk written like a stick; keeps your account between runs (add --uefi for UEFI)
./run.sh vm-reset      # forget that disk
./run.sh vm-arm        # ARM64 image as an Apple-silicon VM would run it (UEFI, emulated, slow); build with ./run.sh build-arm
```

The VM has internet through QEMU's NAT, so BROWSER can open real sites. The boot log is `/tmp/iyakki-vm-serial.log`.
Click in the window to capture the mouse; **Ctrl+Alt+G** releases it. If QEMU complains about io_uring, `run.sh` raises
the locked-memory limit with `sudo prlimit`. Details: [`LOCAL-TEST.md`](LOCAL-TEST.md).

* **UTM (Mac)**: New -> Virtualize -> Other -> UEFI Boot, add `iyakki-aarch64.img` as a VirtIO drive, display
  `virtio-gpu-pci`, input USB tablet ([`BOOT-TEST.md`](BOOT-TEST.md), untested on a Mac).
* **VirtualBox / VMware**: **untested**. The ISO should boot in any x86-64 hypervisor with EFI or BIOS; give it at least 2 GB RAM.

## 5. Variants: web, Mac, ARM64

* **Default `libre` image**: no proprietary firmware (so no Wi-Fi firmware blobs; use Ethernet or a USB Wi-Fi adapter whose
  driver is included).
* **`hw`**: `dist/buildroot/build.sh hw` adds linux-firmware blobs under their own licences. Never built or booted.
* **`web`** (`./run.sh build-web [--h264]`): the big image with the full web engine (WPE WebKit) for YouTube and modern
  sites. Hours to build, about 30 GB disk, 16+ GB RAM; booted in QEMU only ([`FULL-BROWSER.md`](FULL-BROWSER.md)).
* **`mac`** (`./run.sh build-mac`): optional image for Intel Macs (applesmc, hid_apple, Cirrus audio, Wi-Fi firmware).
  Not built or booted ([`MAC-HARDWARE.md`](MAC-HARDWARE.md)). It has no full web engine: YouTube and similar sites show a "needs the full web engine" banner.
* **`macweb`** (`./run.sh build-macweb [--h264] [--crocus]`): `mac` plus the full web engine, for Intel Macs that should run YouTube and modern sites.
  Big (about 200 MB compressed), hours to build. Not built or booted ([`YOUTUBE-ON-IYAKKI.md`](YOUTUBE-ON-IYAKKI.md)).
* **`aarch64`**: UEFI disk image for VMs on Apple silicon and ARM64 boards.
* **Browser front end, containers, other Linux distributions**: [`ADMIN-GUIDE.md`](ADMIN-GUIDE.md#network-exposure),
  [`OTHER-BASES.md`](OTHER-BASES.md).

## 6. Memory, zram and boot options

The live ISO unpacks into RAM (the libre image is about 170 MB unpacked); the web variant needs about 520 MB for the image
alone and 4 GB total is comfortable. Zram settings and the memory guidance are in
[`ZRAM-AND-INSTALL.md`](ZRAM-AND-INSTALL.md). Kernel command-line options IYAKKI reads (from `BOUNDARY.md`, table C;
edit them at the GRUB menu with `e`):

| Option | Effect |
|---|---|
| `iyakki.hwcheck=1` | start in the hardware check |
| `iyakki.ssh=1` | SSH diagnostics (dropbear, keys only, port 22). The images also carry Dropbear's SSH client (`ssh`, `scp`) for the CLI tile; see [`CLI-SSH.md`](CLI-SSH.md) |
| `iyakki.web=1` (or a port, `host:port`, `http:port`) | also serve the desktop to browsers |
| `iyakki.data=ram` | ignore the data volume (nothing is saved) |
| `iyakki.splash=N` | keep the boot screen up at least N seconds |
| `iyakki.shell=0` | do not start the shell |
| `iyakki.selftest=...`, `iyakki.env=...` | tests and forcing environment detection |

Zram boot options: see the zram document (not repeated here so the two cannot disagree).

## 7. Upgrade, back up, uninstall, recover

* **Window mode**: `git pull` and `./run.sh window` again. Data stays in `~/.local/share/iyakki-dev`.
* **Live ISO**: write the new ISO; nothing was kept unless you set up a data disk.
* **Installed stick or disk**: there is no in-place updater in the alpha. Back up (below), write the new install image
  with `iyakki-usb` (this erases the drive, data included), restore. The installed system is read-only except the data
  partition, so an upgrade replaces the whole image.
* **Back up** the data volume: everything IYAKKI stores is in `/var/lib/iyakki` (the IYAKKI-DATA partition): accounts
  (`users.json`), `audit.log`, vaults (`vault/`), settings, chats, documents. Mount the partition on another Linux
  computer (label `IYAKKI-DATA`, ext4) and copy it, or in window mode copy `~/.local/share/iyakki-dev`. The vaults are
  encrypted; the rest is not, so keep the copy private. Restore by copying back with the same ownership.
* **Uninstall**: window mode, delete the data folder and the checkout. Stick or disk: re-partition or overwrite it.
* **Forgot the admin password**: see [`ADMIN-GUIDE.md`](ADMIN-GUIDE.md#forgotten-admin-password).

## 8. Boot problems

| Symptom | Try |
|---|---|
| Stick not in the boot menu / "Security Violation" | Turn Secure Boot off. Try the other of "UEFI: stick" and "stick" entries. Rewrite the stick with `--install` if it was `--iso` or vice versa. |
| Which key opens the boot menu? | F12 (Dell, Lenovo), F9 (HP), Esc or F8 (Asus), F10, Option on Intel Macs. |
| Black screen after the menu | Pick **safe graphics**, then note the machine and GPU. Unplug and re-plug the HDMI cable before power-on on Macs. |
| No logo or boot screen at all, just black until the desktop | [`TROUBLESHOOTING.md`](TROUBLESHOOTING.md#no-boot-logo) (add `iyakki.splash=5` to keep the boot screen up, send `dmesg \| grep -i splash`). |
| No keyboard or mouse | Another USB port (avoid hubs); run the hardware check and read the USB list. |
| No network | Ethernet should come up by DHCP. Wi-Fi needs firmware that the libre image does not carry: use Ethernet, a USB tether or a supported USB Wi-Fi adapter. Check Settings -> Network. Wi-Fi has never been tested on a real access point. |
| No sound | Settings -> Sound (unmute) and Settings -> Voice; the web image's mixer starts muted. |
| Boots to a prompt-less screen / hangs | "Show boot messages" entry; press `L` on the boot screen for the log; on a VM read `/tmp/iyakki-vm-serial.log`. |
| Data not kept after reboot | You booted the live ISO, or `iyakki.data=ram`. Use `--install` or Settings -> Storage. |

More: [`TROUBLESHOOTING.md`](TROUBLESHOOTING.md), [`FAQ.md`](FAQ.md).
