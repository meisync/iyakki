# Which machines IYAKKI runs on

What is supported, what is not, what has only been tried in a virtual machine, and the quirks to know before you boot
a real computer. Nothing here has been tried on real hardware by the project yet except where a row says so; the
evidence column says what was actually run. Run the **hardware check** boot entry first on any machine
(`docs/BOOT-TEST.md`): it lists what the machine reports and lets you test keyboard, mouse, touch and display.

## Machine classes

| Class | Status | Evidence and notes |
|---|---|---|
| x86-64 PC or laptop, UEFI, Secure Boot **off** (desktop, laptop, mini PC) | **Supported, untested on real hardware** | Boots in QEMU with OVMF (UEFI): `dist/buildroot/qemu-install-test.sh` installs the template on a virtual disk and boots it through GRUB. Real laptops and desktops are the first thing to try; see `docs/BOOT-TEST.md`. |
| x86-64 PC with legacy BIOS (or UEFI with CSM/"Legacy" boot) | **Supported, untested on real hardware** | The same test boots the installed disk under legacy BIOS in QEMU. The MBR install layout carries MBR boot code, so both firmwares work from one disk; the GPT layout (BIOS boot partition plus EFI System Partition) does the same, tested in QEMU with SeaBIOS and OVMF. |
| Intel Mac (Mac mini 2014 and similar) | **Supported with limits, untested** | See `docs/MAC-HARDWARE.md`: Ethernet, Intel graphics and USB work with the standard image; built-in Wi-Fi, fan sensors and some audio need the optional Mac image or driver packs. Hold **Option** at power-on to choose the USB stick. |
| Apple Silicon Mac (M1 and later) | **Not supported on bare metal** | Needs an Asahi-class boot chain (`docs/MAC-HARDWARE.md` section 7). The aarch64 image runs inside a virtual machine on it (`docs/MACOS.md`). |
| aarch64 server or board with UEFI firmware (SystemReady-class) | **Boots in QEMU only** | `dist/buildroot/qemu-uefi-aarch64-test.sh` boots the ARM64 disk image under UEFI. No real board has been tried. The aarch64 image is a single FAT boot partition with GRUB, kernel and initramfs; it has no data partition, so there is no disk installer for it yet (see "Install to disk"). |
| Raspberry Pi | **Not supported** | The Pi boots through its own firmware (VideoCore, `config.txt`, device trees), not UEFI plus GRUB. The image has none of that. |
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
