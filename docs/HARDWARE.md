# Which machines IYAKKI runs on

What is supported, what is not, what has only been tried in a virtual machine, and the quirks to know before you boot
a real computer. Updated 2026-11-12 for the alpha.3 line. Real hardware so far: a Mac mini 2018 (tested since 2026-10-18; the most-used machine), a Mac mini 2014,
a 2020 MacBook Pro with the T2 chip (MacBookPro16,2), a MacBook Pro 14,1, an HP Pavilion 16 and a Lenovo Legion Pro 5 (the last three booted the alpha.2 final image, owner,
2026-11-10, with the Mac mini 2018), and one AMD PC (booted; model, GPU and what worked were not written down: **untested in detail**). Every other row is QEMU or untested; the evidence
column says what was actually run. "Seen" below means a person or the check scripts (`dist/hwtest`, through `iyakki dev`) saw it on that machine; the `Manual` column of
[`FEATURES.md`](../FEATURES.md) says `passed <date> <machine>` for the rows where a note reports such a run.
**Per machine and capability (verified / partly / failing / untested, with date and evidence), and what the owner has to run, is in
[project/HARDWARE-MATRIX.md](project/HARDWARE-MATRIX.md); this page is the prose summary.**

## Tested machines (the lab list)

| Machine | Image and medium | Result | Where it is written down | Not tried / open |
|---|---|---|---|---|
| Mac mini 2018 (Macmini8,1, T2, Intel UHD 630) | macweb install image on a USB SSD (Samsung T7) | Booted through GRUB (UEFI), data volume and `/home` persist, HDMI audio, built-in speaker and headphone jack, USB webcam, k3s dev cluster, Quick install items, full web engine (Google, YouTube VP9). 2026-10-18/19 | `KNOWN-ISSUES.md` "Hardware: what was tested", `ALPHA-GATE.md` items 46 to 54 | Wi-Fi (needs Apple firmware, never associated), Bluetooth (the chip needs a patch file macOS does not carry), Apple's own keyboard, Secure Boot. Later: IYAKKI was installed on the Mac's internal disk (`docs/development/GRUB-REPLACEMENT-PLAN.md`, 2026-11-04), the alpha.2 final image booted (owner, 2026-11-10), and builds up to alpha.3-dev4 were checked through developer mode (read-only page and state checks, the lock and sign-out chords on a USB keyboard, a Debian sandbox created and removed, zram swap, the dev cluster's clock wait; the sixth batch commit records 19 passed on what was observed and KI-095 failing) |
| 2020 MacBook Pro with the T2 chip (MacBookPro16,2) | USB SSD (Samsung T7), then the **internal disk** | Booted. Keyboard, trackpad, speaker sound (plughw), GPU browser helper, YouTube autoplay and sound, page fullscreen, Touch Bar backlight (owner report, 2026-10-23/24). First internal-disk install booted 2026-10-24 (owner). Seen on 2026-10-26/27 through developer mode (coordinator, no saved diag file): macweb image booted from the internal disk, k3s Ready (v1.37.1) and started again at boot, internal panel 2560x1600 plus a DisplayPort monitor 1920x1080 (owner: external monitor features are good), T2 audio (AppleT2x4 card), Wi-Fi with Apple's firmware (addresses by DHCP), built-in camera at 640x480 YUYV, external USB camera (EMEET, 328f:00af) detected and its USB microphone confirmed working by the owner, developer mode over Wi-Fi | `CHANGELOG.md` [Unreleased], FEATURES rows F-INST-03, F-KBD-11, F-SND-05, F-LOSS-01 to F-LOSS-06 (the boot drive vanished for 0.6 s once), `docs/MAC-HARDWARE.md` "MacBookPro16,2" | Bluetooth (BCM4364, no patch file: fails), Touch Bar strip (IYAKKI-drawn, never opens: USB configuration switch falls back), built-in microphone (off on purpose). Fixes built and **not yet verified**: touch panel on the monitor hub reacts on the wrong display, camera picker, developer-mode port 7979 blocked by the device firewall, TALES highlight drift inside sentences, dev cluster after an address change. Not tried: upgrade of the internal install, battery (kernel options not in an image), sleep, shutdown, Secure Boot |
| An AMD PC | Not recorded | Booted (owner report). Nothing else is written in the repository | none (owner report only) | Everything: which model and GPU, sound, Wi-Fi, suspend, internal-disk install. Treat as **untested** until a hardware report (`iyakki hwreport`) is saved |
| Mac mini 2014 (Macmini7,1, no T2, Intel HD 5000, 16 GB, NVMe) | lean install image (0.1.0-alpha.3-dev4), wired network | Booted (UEFI). The owner reports that everything works except Wi-Fi, and that Bluetooth and sound work (2026-11-12). Kernel log: the card is a Broadcom BCM4360 (PCI AC); the `b43` driver finds it and fails with `FOUND UNSUPPORTED PHY (Type 11 (AC))`, error -95 | the owner's report; `iyakki dev info` and the kernel log | **Wi-Fi does not work (KI-094).** Use the cable or a USB Wi-Fi adapter. Nothing else is written down beyond the batch result: the commit that records its sixth check batch says 39 recipes passed on what was observed and 3 need a person |
| MacBook Pro 14,1 (13-inch 2017, no T2; Wi-Fi BCM4350, Bluetooth BCM4350C0, Cirrus CS8409 sound) | the T7 install (lean image) | Booted the alpha.2 images (owner, 1109a on 2026-11-09 and the final alpha2c on 2026-11-10). Bluetooth: pairing, a phone's sound (A2DP) and tether with internet (owner, 2026-11-08; Manual `passed` on F-NET-21 and F-NET-24). Quiet speakers on the CS8409 codec were measured and handled in code (2026-11-08) | `docs/MAC-HARDWARE.md` (MacBookPro14,1 sections), FEATURES rows F-NET-21 and F-NET-24 | The camera, battery, suspend and the rest: nothing is written down. The Bluetooth patch pack is not needed there (the installer leaves it out) |
| HP Pavilion Laptop 16-af0xxx (board 8CB6, Insyde F.11; Intel Meteor Lake with Arc graphics, NVIDIA 10de:25aa without display output, WD NVMe, Realtek RTL8852BE Wi-Fi, 32 GB) | the T7 install (PC lean image), Secure Boot on with the project's key enrolled | The first loader-default stick stopped before stage 1 (no x2APIC in the kernel); after the kernel was rebuilt with `CONFIG_X86_X2APIC` and `CONFIG_IRQ_REMAP` it booted (owner, 2026-11-07), and the loader's log shows shim and MOK accepting the signed chain. The alpha.2 final image booted (owner, 2026-11-10), and its camera worked. A closed privacy shutter gave a black picture with no message (KI-082, fixed in code) | `docs/development/HP-LAPTOP-PLAN.md`, `docs/project/HARDWARE-MATRIX.md` section E | Wi-Fi (Realtek firmware pack, not tried), sound, touch, battery, suspend, GPU for models, internal-disk install: untested |
| Lenovo Legion Pro 5 (82WM; AMD Ryzen with Radeon graphics, NVIDIA dGPU, Realtek audio codec, MediaTek Wi-Fi and Bluetooth) | the T7 install, a flash drive written with `iyakki-usb`, Secure Boot on with the key enrolled | Boots the PC image (loader default, owner). The alpha.2 final image booted (owner, 2026-11-10). Found there and since fixed in code: the camera light staying on (KI-080, KI-093) and a slow driver download with no network (KI-083, the fix seen working). Manual `passed` 2026-11-11 on F-PKG-06 (the USB writer) and F-LOCK-02 (lock keys). Wi-Fi and Bluetooth firmware is installed by the person with a button; the mixer starts muted or low | `docs/development/HP-LAPTOP-PLAN.md` (what the Lenovo taught), FEATURES rows F-PKG-06 and F-LOCK-02, `KNOWN-ISSUES.md` KI-080, KI-083, KI-093 | The camera light fix is not confirmed on it. Everything not named: untested |
| Another Intel + NVIDIA PC | PC image (`./run.sh build-pc-layered`), built 2026-10-26, QEMU/UEFI boot only | **Untested.** (The HP Pavilion above is the Intel + NVIDIA laptop that has since booted.) Test sheet: `docs/project/PC-TEST-SHEET.md` | none | All of it; NVIDIA runs on `nouveau` unless the vendor driver is installed (`docs/GPU-K3S.md`) |
| AMD + NVIDIA PC | PC image, built 2026-10-26, QEMU/UEFI boot only | **Untested** (the owner reports it booted; no log, no details). Test sheet for 2026-10-27 | none (owner report only) | All of it |

**The T7 stick.** One install on a Samsung T7 SSD (a data volume on an external drive) has been started on the Mac mini 2018, the MacBook Pro 14,1, the HP and the Lenovo
(hardware test sheet, 2026-11-09). IYAKKI keeps the settings of each computer apart (see "One install, several computers" below); `iyakki dev info` shows which computer it
is running on, so look at its `device` line before you read any result.

**A 120 GB drive:** this update was asked to list one, and nothing in the repository records a 120 GB drive or what was tried on it. Its status is **unknown** and it is left out of the table
until a result is written down.

A Linux development PC with an AMD Radeon 780M ran GPU stage 1 (Vulkan, k3s) under its own operating system (`docs/GPU-K3S.md`); that is not
a boot of IYAKKI on it. Run the **hardware check** boot entry first on any machine
(`docs/BOOT-TEST.md`): it lists what the machine reports and lets you test keyboard, mouse, touch and display.

## Machine classes

| Class | Status | Evidence and notes |
|---|---|---|
| x86-64 PC or laptop, UEFI, Secure Boot **off** (desktop, laptop, mini PC) | **Supported; an HP Pavilion 16 and a Lenovo Legion Pro 5 booted the alpha.2 image (owner), one AMD PC booted (no details); every other PC untested** | Boots in QEMU with OVMF (UEFI): `dist/buildroot/qemu-install-test.sh` installs the template on a virtual disk and boots it through GRUB. Real laptops and desktops are the first thing to try; see `docs/BOOT-TEST.md`. |
| x86-64 PC with legacy BIOS (or UEFI with CSM/"Legacy" boot) | **Supported, untested on real hardware** | The same test boots the installed disk under legacy BIOS in QEMU. The MBR install layout carries MBR boot code, so both firmwares work from one disk; the GPT layout (BIOS boot partition plus EFI System Partition) does the same, tested in QEMU with SeaBIOS and OVMF. |
| Intel Mac with a T2 chip (Mac mini 2018, MacBook Pro 2019/2020) | **Booted on a Mac mini 2018 and a MacBookPro16,2** (table above); Secure Boot on a T2 Mac is not recorded | Wi-Fi and Bluetooth need firmware the person installs; Bluetooth does not work on the Mac mini 2018; see the table above for what else is open. |
| Intel Mac without a T2 chip (Mac mini 2014 and similar) | **Booted: a Mac mini 2014 (lean image, over a cable; its Wi-Fi card has no driver, KI-094) and a MacBook Pro 14,1 (Bluetooth works there); other models untested** | See `docs/MAC-HARDWARE.md`: Ethernet, Intel graphics and USB work with the standard image; built-in Wi-Fi, fan sensors and some audio need the optional Mac image or driver packs. Hold **Option** at power-on to choose the USB stick. |
| Apple Silicon Mac (M1 and later) | **Not supported on bare metal (out of scope, owner decision 2026-10-27)**; **VM path only** | IYAKKI runs in a lightweight virtual machine with k3s and a bridge to the model runner on the Mac (`docs/MACOS.md`, `docs/MAC-MODEL-HOST.md`). The VM recipe (a stock Debian arm64 cloud image with k3s, via Lima) was run once on an Apple Silicon Mac mini on 2026-11-19 and reached a Ready k3s node; other chips are untested. The separate desktop window (not the OS) builds and runs natively on an Apple silicon Mac mini, with limits (KI-078, `docs/MACOS.md`). The Asahi plan (`docs/APPLE-SILICON.md`) is kept as a note; F-BOOT-09, F-DRIVERS-12 and F-DRIVERS-19 are `deprecated`. |
| aarch64 server or board with UEFI firmware (SystemReady-class) | **Boots in QEMU only** | `dist/buildroot/qemu-uefi-aarch64-test.sh` boots the ARM64 disk image under UEFI. No real board has been tried. The aarch64 image is a single FAT boot partition with GRUB, kernel and initramfs; it has no data partition, so there is no disk installer for it yet (see "Install to disk"). |
| Raspberry Pi | **Alpha, first real boot 2026-11-24. Pi 5 (16 GB): booted to the IYAKKI home screen on HDMI with USB keyboard and mouse, once the D0 device tree was on the card (KI-1030); the userland was old (KI-1034); Ethernet, sound and restart not tried** (see `docs/BOARD-RPI5-FIRST-BOOT.md`); Pi 3 (IYAKKI lite): design only | The Pi boots through its own firmware (EEPROM, `config.txt`, device tree), not UEFI plus GRUB: the Pi 5 image has its own boot volume, kernel (Raspberry Pi tree, 4 KiB pages) and no Raspberry Pi firmware blob. Nothing is proven on a real board until the owner's first-boot checklist (F-RPI-08) comes back; every Pi cell stays `not supported` until then. Plan: `docs/development/RASPBERRY-PI-PLAN.md` section 13. |
| AMD Kria KV260 Starter Kit (K26 SOM, Zynq UltraScale+ MPSoC, 4 GB) | **Alpha, first real boot 2026-11-24: Linux and IYAKKI boot from the SD card; DisplayPort shell at 1920x1080 with the card's device tree for carrier rev B (KI-1031), USB keyboard, mouse and Gigabit Ethernet work; developer mode pairing is open (KI-1033); shutdown and restart, sound and camera not tried. The test is described in `docs/BOARD-KV260-FIRST-BOOT.md`** | SD-card image (`ARCH=aarch64 dist/buildroot/build.sh kv260`, written with `dist/buildroot/kv260-write-sd.sh`): AMD's linux-xlnx kernel at a pinned commit, no GRUB; the board's own boot firmware in its QSPI flash (U-Boot) reads `extlinux/extlinux.conf` from the card. IYAKKI never writes that flash and ships no AMD boot firmware. Headless over serial and Ethernet first; DisplayPort shell if the kernel's display driver works. 4 GB: shell, apps and tiles only, local models go to a remote model host. `docs/development/KV260-PLAN.md`. |
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
  laptops with RAID or "Intel RST/VMD" mode in the firmware put the NVMe disk behind a VMD bridge: the VMD driver is built into the
  kernel (images after 2026-11-07), so the disk shows up as usual; on an older image set the disk mode to AHCI in the firmware instead.
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

## One install, several computers

The same install (a data volume on an external SSD) can start on several computers: a Mac mini, a Legion laptop, an HP laptop. IYAKKI
recognises which computer it started on and keeps the settings that belong to a computer apart from the settings that belong to you.

* **Which computer is it?** A short id made from the computer's model names (DMI vendor, product, version and board name). No serial
  number and no unique machine id is read or stored. A virtual machine gets its own id the same way.
* **Kept per computer:** the sound output, volume, mute and per-card levels and boosts; the microphone; the sign-in screen saver
  timings and the text size of each account; keyboard and pointer choices (Apple or PC profile, Fn keys, trackpad and wheel);
  power and battery choices; and whether Lemonade runs on the GPU or the CPU. The list is one table in `crates/iyakki-shared/src/hwprofile.rs`.
* **Goes with you:** models and providers, speech engines, the store, the firewall, the time zone, the host name, accounts and the vault.
* **First start on a new computer:** it starts from automatic (sound output automatic, keyboard profile automatic, normal text size,
  Lemonade on the GPU); it never copies another computer's values. Administrators get one inbox card, "New computer: ...". The first
  computer an install ever sees keeps the settings the install already had. Starting on a known computer again restores its own values.
* **Settings, System, This computer** shows the computer and the others seen, and lets an administrator share a kind of setting between
  all computers instead of keeping it per computer, copy another computer's settings here, reset this computer's settings, or forget
  another computer. The chat assistant has the same through `this_computer_status` (free) and `this_computer_set_sharing`,
  `this_computer_copy_from` and `this_computer_reset` (each asks). `iyakki dev config list` names the computer its values are for.
* **Files:** `computers/<id>.json` (settings values of that computer), `computers/sharing.json` (the kinds shared by all) and
  `computers/current` in the data folder. A damaged profile is moved aside (`.corrupt-<time>`) and the global values are used; the
  global `settings.json` always holds the last values as the fallback.
