# AMD Kria KV260: design, boot path and what is verified

**For:** the owner and whoever works on the board. **Status 2026-11-21:** the SD-card image recipe is written and its layout is tested; the userspace boots in QEMU; **nothing has run on a real KV260.** Hand-off for the real test: `docs/BOARD-KV260-FIRST-BOOT.md`. Rows: F-KV260-01 to 04.

Honesty tags: **verified** (read from the named source on 2026-11-21), **from memory** (check before relying), **unverified**.

## 1. The board

K26 SOM (Zynq UltraScale+ MPSoC, quad Cortex-A53 at about 1.3 GHz, Mali-400 GPU, 4 GB DDR4, 16 GB eMMC on the SOM that the starter-kit device tree turns off, QSPI flash) on the KV260 carrier card: DisplayPort, HDMI (PL-driven, not used), 4 USB 3.0 ports through a hub, Gigabit Ethernet, microSD slot, micro-USB with an FTDI bridge to a serial console and JTAG, Pmod, camera connectors, a TPM. **No Wi-Fi, no Bluetooth, no audio codec** worth planning for (from memory). The programmable logic (PL) needs no bitstream for a first boot.

## 2. Boot path (the question the owner asked)

The board does not boot from the SD card by itself. Its **primary boot firmware lives in the SOM's QSPI flash** (AMD's build of the platform management firmware, Arm Trusted Firmware and U-Boot) and starts a Linux kernel from the SD card with U-Boot's standard "distro boot".

**Verified (upstream U-Boot, `board/xilinx/zynqmp/zynqmp_kria.env`, read 2026-11-21):**

* On a KV260 (card name `SCK-KV-G`) the boot order is `mmc1 usb0..usb4 pxe dhcp jtag`: **the SD card is tried first**, then USB storage, then the network.
* For each bootable FAT partition of the card, in this order: `extlinux/extlinux.conf` (in `/` and `/boot/`), then `boot.scr.uimg` / `boot.scr`, then the EFI removable-media file `efi/boot/bootaa64.efi`. The partition must be flagged **bootable** (`part list -bootable`; if none is flagged, partition 1 is used).
* The filesystem must be FAT (`bootfstype=fat`).
* The kernel is loaded at `0x18000000`, the device tree at `0x40000000`, the ramdisk at `0x02100000`; U-Boot's own device tree (`fdtcontroladdr`) is passed to the kernel when the extlinux entry names no `fdt`.

**From memory (not verified; the AMD pages are JavaScript and could not be read here):** AMD's shipped firmware is a build of this same U-Boot recipe, so the order above is what a board does; the K26 firmware must be a 2022.1 or newer build for standard distro boot (older boards need AMD's firmware update through their own tool); a `boot.bin` on the card is **not** needed for this path (it is used by the board's alternative "SD boot" mode with a boot-mode switch for recovery images); AMD's Ubuntu images boot the same way.

**Decision (default, owner may overturn): extlinux.** Reasons: it needs no GRUB and no EFI stub tricks; the existing aarch64 image carries GRUB for EFI and would also be found through `bootaa64.efi`, but then the kernel gets the device tree through EFI and GRUB adds a layer that gives the serial console nothing. The extlinux file has five entries (default with the board's own device tree, the kernel's two KV260 device trees as fall-backs, self-test, SSH). **No vendor blob, no boot loader and no `boot.bin` is written to the card or kept in the repository**, so the "firmware by button" rule has nothing to fetch: if the person's board has old firmware, AMD's own update procedure applies (nothing to download, so `legal/downloads.txt` has no row for it: the licence gate ties rows to things the code installs). IYAKKI never writes the QSPI flash; the kernel has no SPI NOR, no MTD block device and no QSPI controller driver at all (`board/iyakki-kv260/linux.fragment`, checked by `verify-kernel-config.sh`).

Stage 1 (`crates/iyakki-stage1`): not used on this image. The image is an initramfs boot like the existing aarch64 image (kernel + `rootfs.cpio.gz`, no `IYAKKI-DATA` pointer scheme, no lean/full switch), so no stage-1 change is needed; the boot partition's `board` file (`kv260`) is there for the later installer and for stage 1 to compare with `/proc/device-tree/model` when the fast-start path comes to this board. This differs from the Pi plan (direct kernel boot with stage 1 doing GRUB's job): here U-Boot's extlinux is the boot menu, so the menu entries survive.

## 3. Kernel

AMD's `linux-xlnx`, tag `xlnx_rebase_v6.12_LTS_2025.2` = commit `31626ef92ff11f9d3dee01926720572666fcc65a` (**verified**: `git ls-remote`, the tree's Makefile says 6.12.40; our x86 kernels are 6.12.112, userspace is the same). Fetched by Buildroot at build time (GPL-2.0, recorded by `make legal-info` like every other kernel; pins in `dist/kv260/PINS`). Base `xilinx_defconfig` (verified to exist, 671 lines, already sets the ZynqMP platform, Cadence GEM, DWC3, Arasan SDHCI, the PS UART, DPSUB and DPDMA), then `board/iyakki-aarch64/linux.fragment` (shared), then `board/iyakki-kv260/linux.fragment` (what a first boot needs, written to survive kconfig; the verify script checks every line). **Device trees verified in the tree:** `zynqmp-smk-k26-revA-sck-kv-g-revA.dtb` and `...-revB.dtb` (SOM plus the KV260 carrier, rev A or B; the card's rev is on the carrier's label). **Verified from `zynqmp-sm-k26-revA.dts`:** console `stdout-path = "serial1:115200n8"` (PS UART1, Linux `ttyPS1`), SD is `sdhci1` (`mmc1`, device `mmcblk1`), the eMMC node is disabled by the starter-kit file.

Not verified (needs the kernel build to finish, then the board): that every symbol in our fragment exists under that name at that tag (`verify-kernel-config.sh` with `VARIANT=kv260` tells; see the result in the F-KV260-01 row), `clk_ignore_unused` on the command line (from memory: AMD recipes use it; remove it if the owner sees everything fine without it), whether `drm/zynqmp-dpsub` brings up DisplayPort without a PL design (from memory: yes for the PS-side DisplayPort; the KV260's monitor port goes through the PS DisplayPort), whether the USB hub (USB5744) needs its initial setup by the firmware (from memory: the board's U-Boot does it; the kernel's `USB_ONBOARD_DEV` driver is on).

## 4. Image, sizes, what fits in 4 GB

`ARCH=aarch64 dist/buildroot/build.sh kv260` makes `iyakki-kv260.img` (MBR; partition 1 FAT32 `IYAKKI-BOOT` 256 MB, bootable: `extlinux/extlinux.conf`, `Image`, `rootfs.cpio.gz`, `dtb/`, `board`; partition 2 the 1 MiB data marker that `kv260-write-sd.sh` stretches to the card). The whole userspace is unpacked into RAM like the other images (about 190 MB unpacked; sizes of the first build are in the F-KV260-01 row). 4 GB: the shell, apps and tiles; no local models (a remote model host), no full web engine (the aarch64 web variant does not exist), k3s only as an experiment.

Headless first: serial console on the micro-USB and Ethernet are the steps that must work; the shell on DisplayPort is the second part of the checklist.

## 5. The SD writer

`dist/buildroot/kv260-write-sd.sh IMAGE DEVICE`: whole card only (`sdX`, `mmcblkN`), never loop, ram, zram, dm, md, mtd, nvme or sr devices, never a mounted card or the running system's disk, refuses over 256 GB unless asked, checks the image is a KV260 image (MBR, bootable FAT32 first, Linux second), asks for the device name, writes with fsync, **reads back and compares**, then stretches the data partition. The test (`test-kv260-layout.sh`) runs it against ordinary files.

## 6. QEMU (result 2026-11-21)

`dist/buildroot/qemu-test-kv260.sh` first tries QEMU's `xlnx-zcu102` machine (quad A53, Cadence GEM, Arasan SD, DWC3, PS UART). **On the build host's QEMU 10.2.1 that machine aborts at start-up** (`xlnx-zynqmp-efuse ... is not an instance of type`), with no kernel given, so it cannot be used there; the script then boots the same kernel and initramfs on `virt`. Result: PASS on `virt`: the linux-xlnx 6.12.40 kernel starts, the IYAKKI self-test runs from begin to end (log kept in `~/iyakki-work/images/kv260/qemu-boot.log`). That proves the kernel configuration produces a booting kernel and the userspace runs on it; it does **not** exercise the ZynqMP drivers, U-Boot's extlinux, DisplayPort, the USB hub or the SD slot: those are the real-board steps. A newer QEMU with a working `xlnx-zcu102` would add the SD, Ethernet and USB controllers; the script is ready for it.

## 6b. First build (2026-11-21)

`ARCH=aarch64 IYAKKI_PROFILE=fast build.sh kv260` took about 1 h 10 min on the AMD host (a toolchain from scratch, the 4 GB linux-xlnx git fetch, the Rust cross build in 9 min, the kernel). kconfig on the real tree kept every line of `board/iyakki-kv260/linux.fragment`; MTD SPI NOR, the quad-SPI controller and MTD block are off. Sizes: kernel `Image` 28.8 MB, `rootfs.cpio.gz` 90 MB, device trees 45 KB each, SD image 258 MiB (103 MB gzipped), the boot FAT partition holds 119 MB. A release-profile (fat LTO) build would be smaller and slower; the first board test uses this fast-profile image.

## 7. Open questions for the owner

1. Extlinux or EFI (section 2): default extlinux.
2. Do you want the DisplayPort shell in the first test, or serial only? (the checklist asks for both, in that order).
3. If the board shows old firmware at power-on (U-Boot banner year before 2022): update it with AMD's tool first; we write nothing to the flash.
