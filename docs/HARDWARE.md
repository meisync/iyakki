# Which machines IYAKKI runs on

What is supported, what is not, what has only been tried in a virtual machine, and the quirks to know before you boot
a real computer. Updated 2026-10-27. Real hardware so far: a Mac mini 2018 (tested 2026-10-18/19), a 2020 MacBook Pro with the T2 chip
(MacBookPro16,2: booted from a USB SSD, and its internal disk booted after an install on 2026-10-24, owner report) and one AMD PC
(booted; model, GPU and what worked were not written down: **untested in detail**). Every other row is QEMU or untested; the evidence
column says what was actually run. The PC image was built on 2026-10-26 for an Intel + NVIDIA PC and an AMD + NVIDIA PC; it was booted only in QEMU
so far, and both PCs are tested on 2026-10-27 (sheet: `docs/project/PC-TEST-SHEET.md`); they stay **untested** until a result is written down.
**Per machine and capability (verified / partly / failing / untested, with date and evidence), and what the owner has to run, is in
[project/HARDWARE-MATRIX.md](project/HARDWARE-MATRIX.md) (2026-10-27); this page is the prose summary.**

## Tested machines (the lab list)

| Machine | Image and medium | Result | Where it is written down | Not tried / open |
|---|---|---|---|---|
| Mac mini 2018 (Macmini8,1, T2, Intel UHD 630) | macweb install image on a USB SSD (Samsung T7) | Booted through GRUB (UEFI), data volume and `/home` persist, HDMI audio, built-in speaker and headphone jack, USB webcam, k3s dev cluster, Quick install items, full web engine (Google, YouTube VP9). 2026-10-18/19 | `KNOWN-ISSUES.md` "Hardware: what was tested", `ALPHA-GATE.md` items 46 to 54 | Wi-Fi (needs Apple firmware, never associated), Bluetooth, internal-disk install, the newest image, Secure Boot |
| 2020 MacBook Pro with the T2 chip (MacBookPro16,2) | USB SSD (Samsung T7), then the **internal disk** | Booted. Keyboard, trackpad, speaker sound (plughw), GPU browser helper, YouTube autoplay and sound, page fullscreen, Touch Bar backlight (owner report, 2026-10-23/24). First internal-disk install booted 2026-10-24 (owner). Seen on 2026-10-26/27 through developer mode (coordinator, no saved diag file): macweb image booted from the internal disk, k3s Ready (v1.37.1) and started again at boot, internal panel 2560x1600 plus a DisplayPort monitor 1920x1080 (owner: external monitor features are good), T2 audio (AppleT2x4 card), Wi-Fi with Apple's firmware (addresses by DHCP), built-in camera at 640x480 YUYV, external USB camera (EMEET, 328f:00af) detected and its USB microphone confirmed working by the owner, developer mode over Wi-Fi | `CHANGELOG.md` [Unreleased], FEATURES rows F-INST-03, F-KBD-11, F-SND-05, F-LOSS-01 to F-LOSS-06 (the boot drive vanished for 0.6 s once), `docs/MAC-HARDWARE.md` "MacBookPro16,2" | Bluetooth (BCM4364, no patch file: fails), Touch Bar strip (IYAKKI-drawn, never opens: USB configuration switch falls back), built-in microphone (off on purpose). Fixes built and **not yet verified**: touch panel on the monitor hub reacts on the wrong display, camera picker, developer-mode port 7979 blocked by the device firewall, TALES highlight drift inside sentences, dev cluster after an address change. Not tried: upgrade of the internal install, battery (kernel options not in an image), sleep, shutdown, Secure Boot |
| An AMD PC | Not recorded | Booted (owner report). Nothing else is written in the repository | none (owner report only) | Everything: which model and GPU, sound, Wi-Fi, suspend, internal-disk install. Treat as **untested** until a hardware report (`iyakki hwreport`) is saved |
| Intel + NVIDIA PC | PC image (`./run.sh build-pc-layered`), built 2026-10-26, QEMU/UEFI boot only | **Untested.** Test sheet for 2026-10-27: `docs/project/PC-TEST-SHEET.md` | none | All of it; NVIDIA runs on `nouveau` unless the vendor driver is installed (`docs/GPU-K3S.md`) |
| AMD + NVIDIA PC | PC image, built 2026-10-26, QEMU/UEFI boot only | **Untested** (the owner reports it booted; no log, no details). Test sheet for 2026-10-27 | none (owner report only) | All of it |

A Linux development PC with an AMD Radeon 780M ran GPU stage 1 (Vulkan, k3s) under its own operating system (`docs/GPU-K3S.md`); that is not
a boot of IYAKKI on it. Run the **hardware check** boot entry first on any machine
(`docs/BOOT-TEST.md`): it lists what the machine reports and lets you test keyboard, mouse, touch and display.

## Machine classes

| Class | Status | Evidence and notes |
|---|---|---|
| x86-64 PC or laptop, UEFI, Secure Boot **off** (desktop, laptop, mini PC) | **Supported; one AMD PC booted (owner report, no details); every other PC untested** | Boots in QEMU with OVMF (UEFI): `dist/buildroot/qemu-install-test.sh` installs the template on a virtual disk and boots it through GRUB. Real laptops and desktops are the first thing to try; see `docs/BOOT-TEST.md`. |
| x86-64 PC with legacy BIOS (or UEFI with CSM/"Legacy" boot) | **Supported, untested on real hardware** | The same test boots the installed disk under legacy BIOS in QEMU. The MBR install layout carries MBR boot code, so both firmwares work from one disk; the GPT layout (BIOS boot partition plus EFI System Partition) does the same, tested in QEMU with SeaBIOS and OVMF. |
| Intel Mac with a T2 chip (Mac mini 2018, MacBook Pro 2019/2020) | **Booted on a Mac mini 2018 and a MacBookPro16,2** (table above) | Wi-Fi and Bluetooth need firmware the person installs; Bluetooth does not work on the Mac mini 2018; see the table above for what else is open. |
| Intel Mac without a T2 chip (Mac mini 2014 and similar) | **Supported with limits, untested** | See `docs/MAC-HARDWARE.md`: Ethernet, Intel graphics and USB work with the standard image; built-in Wi-Fi, fan sensors and some audio need the optional Mac image or driver packs. Hold **Option** at power-on to choose the USB stick. |
| Apple Silicon Mac (M1 and later) | **Not supported on bare metal (out of scope, owner decision 2026-10-27)**; **VM path only** | IYAKKI runs in a lightweight virtual machine with k3s and a bridge to the model runner on the Mac (`docs/MACOS.md`, `docs/MAC-MODEL-HOST.md`). The VM recipe has never been run on a Mac, so it is untested. The Asahi plan (`docs/APPLE-SILICON.md`) is kept as a note; F-BOOT-09, F-DRIVERS-12 and F-DRIVERS-19 are `deprecated`. |
| aarch64 server or board with UEFI firmware (SystemReady-class) | **Boots in QEMU only** | `dist/buildroot/qemu-uefi-aarch64-test.sh` boots the ARM64 disk image under UEFI. No real board has been tried. The aarch64 image is a single FAT boot partition with GRUB, kernel and initramfs; it has no data partition, so there is no disk installer for it yet (see "Install to disk"). |
| Raspberry Pi | **Not supported today; planned, see `docs/development/RASPBERRY-PI-PLAN.md`** | Nothing has been built or booted on a Pi. The Pi boots through its own firmware (VideoCore, `config.txt`, device trees), not UEFI plus GRUB. The image has none of that. |
| Other ARM boards that boot only through U-Boot with a device tree | **Not supported** | Same reason. |
| QEMU / KVM | **Tested** | All automated boot, persistence, installer and memory tests run here (BIOS and UEFI, x86-64; UEFI, aarch64). |
| VirtualBox, VMware (Workstation/Fusion/ESXi), Hyper-V generation 2 | **Expected to work, untested** | Standard x86-64 virtual hardware. Hyper-V generation 2 and any VM with Secure Boot on need Secure Boot **off** (see below). Use 2 GiB of RAM and a 16 GiB virtual disk. |
| 32-bit x86, PowerPC, RISC-V | **Not supported** | No image. |

## Minimum requirements

* **Processor:** a 64-bit x86 processor with the baseline x86-64 instruction set (no AVX or newer extensions are required:
  the kernel is built from the stock x86_64 configuration and the Rust code targets `x86_64-unknown-linux-musl` with its
  default CPU level). aarch64: ARMv8-A. A processor older than about 2008 may still lack virtualization or UEFI, which
  matters only for the virtual-machine and UEFI rows above.
* **Memory:** 1 GiB runs the live system (the whole system is unpacked into RAM at boot, about 170 MB; the QEMU tests use
  1 GiB). **2 GiB is recommended.** The full web engine image needs far more (see `docs/FULL-BROWSER.md`). zram (compressed
  swap in RAM) is on by default and helps small machines most; see `docs/ZRAM-AND-INSTALL.md`.
* **Disk for an install:** 4 GB is the minimum the installer accepts; **16 GB or more is comfortable** (the boot partition
  is 256 MB, the rest is the data volume for accounts, chats, models and downloads). Disks over 2 TiB need a GPT
  (the installer's `auto` chooses it; the data partition then uses the whole disk). A disk with 4096-byte logical sectors
  (4Kn) is installed with a GPT and starts through **UEFI only** (legacy BIOS boot is not supported on 4Kn); the boot partition
  is rebuilt at 320 MB. Other sector sizes are refused. 4Kn was tested on a QEMU virtual disk only, never on a real 4Kn drive.
* **Disk for the live stick:** any USB stick of 2 GB or more. The live ISO needs no disk at all, but forgets everything at
  power-off unless a data volume exists.

## Known quirks

* **Secure Boot must be off unless the image was signed by its owner (`docs/SECURE-BOOT.md`, QEMU-tested only).** Ordinary images are not signed and there is no shim: the kernel, GRUB and the boot partition are
  unsigned, so firmware with Secure Boot on refuses to start them. Turn Secure Boot off in the firmware setup (details vary
  by maker), or, on some machines, delete the platform keys. Signing is planned (F-BOOT-02). This is a property of the image,
  not of the installer.
* **USB boot order.** Open the one-time boot menu (usually F12, F10, F8, F9 or Esc) and pick the stick. Some firmware lists
  the same stick twice (once "UEFI: name", once plain); try the UEFI entry first. After the install, remove the stick or
  move the internal disk up in the boot order.
* **Mac:** hold **Option** (Alt) at the chime to get the boot picker; the stick shows as "EFI Boot".
* **NVMe:** works (the driver is built in). The disk is named `nvme0n1`; its partitions `nvme0n1p1`, `nvme0n1p2` (and `nvme0n1p3` on a GPT install). Some
  laptops with RAID or "Intel RST/VMD" mode in the firmware hide the NVMe disk from Linux: set the disk mode to AHCI.
* **SD cards and eMMC** (`mmcblk0`) are supported as install targets (`mmcblk0p1`, `mmcblk0p2`, and `mmcblk0p3` on a GPT install).
* **Wi-Fi and some graphics need firmware files** that are not in the default (libre) image: use the **hw** image variant
  (`./run.sh build` builds libre; see `dist/buildroot/build.sh hw`) or a driver pack from the Store.
* **The installed system still runs from RAM:** the boot partition holds GRUB, the kernel and the initial RAM disk; the data
  volume keeps your accounts and files. A power cut loses nothing that was saved to the data volume.
* **Two systems:** installing erases the whole disk you choose. There is no "install next to Windows" mode yet.

## Install to disk, in one paragraph

Boot the USB stick, open **Settings, System, Install to disk** (or run `iyakki install` in a terminal), pick the disk,
choose the partition table if you do not want **Auto** (GPT for big, 4Kn or UEFI cases, MBR for old BIOS-only PCs), answer the
questions (compressed memory, swap file, computer name), type the drive name and its size to confirm. Details and
the exact commands: `docs/ZRAM-AND-INSTALL.md`.
