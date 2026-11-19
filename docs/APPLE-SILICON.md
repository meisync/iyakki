# IYAKKI on Apple Silicon

Two ways, and only the first exists today.

1. **In a virtual machine on any Apple Silicon Mac** (M1 to M6, every model): works now with the `aarch64` image. Section 2.
2. **On the bare metal** (dual boot next to macOS) through the Asahi Linux boot chain: **planned, not built, not booted**.
   It can work on M1 and M2, probably on M3 (with limits), and **not at all on M4, M5 or M6 yet**. Sections 1, 3 and 4.

**Owner decision (2026-10-27):** for Apple Silicon IYAKKI goes the VM route (a light VM with k3s and a bridge to a model runner on the Mac);
the bare-metal Asahi route below is out of scope for now. Design and plan: [`development/APPLE-SILICON-VM-PLAN.md`](development/APPLE-SILICON-VM-PLAN.md)
(it replaces recipe C of section 2 and route 3 of `APP-MODE-AND-APPLE-SILICON.md`).

Facts below were read on 2026-10-18 from the Asahi Linux documentation (feature-support pages for each generation),
the `asahi-installer` and `asahi-installer-data` repositories, and news coverage. Asahi's own pages say they give no
estimates for unfinished work; status words change often, so re-read them before you spend hours.

## 1. Status per chip (Asahi Linux, 2026-10-18)

"linux-asahi" means the feature works in Asahi's kernel fork but is not in the mainline Linux kernel yet.

| Chip family | Asahi installer | Device trees | Screen | What an IYAKKI alpha could use | Verdict |
|---|---|---|---|---|---|
| M1, M1 Pro/Max/Ultra | yes | mainline | display controller (DCP) upstream since 5.17 | Everything: NVMe, USB-C (upstream 7.0), keyboard/trackpad, Wi-Fi (6.1), Bluetooth (6.2), speakers, sleep | **Supported** (best) |
| M2, M2 Pro/Max/Ultra | yes | mainline | DCP in linux-asahi | The same, plus webcam | **Supported** |
| M3, M3 Pro/Max | yes (added to the installer on 2026-09-06 according to press) | 7.2 / 7.3 | **boot framebuffer only**; DCP work in progress; brightness WIP | NVMe, USB-C, keyboard, touchpad, Wi-Fi, Bluetooth, speakers, battery: all in linux-asahi. No GPU (WIP on base M3 only), no external display, no Thunderbolt | **Possible for IYAKKI**: its shell is software-rendered on a framebuffer, so it needs no GPU |
| M3 Ultra (Mac Studio) | no | linux-asahi (7.3) | none | nothing usable | Not supported |
| M4, M4 Pro/Max, A18 Pro | **no** | 7.4 (M4/A18 Pro); WIP (Pro/Max) | boot framebuffer only | NVMe and PCIe still unstarted (NVMe "WIP"), no keyboard, no Wi-Fi, no USB | **Not supported** |
| M5 family | **no** | not started | none | nothing; every block "TBA" (no active work) | **Not supported** |
| M6 (T8152; Mac mini 2026) | **no** | not started | none | nothing; only video decoder and NVMe marked WIP | **Not supported** |

Plain statement for the owner: if the Macs are M4 or newer, **bare metal is not possible today for anyone**; the VM is the way.
The "no installer" line comes from the Asahi pages themselves, so it is the project's own statement, not ours.

Upstream kernel status (from the same pages): mainline has the interrupt controller, DART IOMMU, pin control, NVMe (ANS),
PCIe, I2C, the M1 display controller, Wi-Fi (brcmfmac, 6.1), Bluetooth (6.2), cpufreq (6.2) and USB2/USB3 on the Type-C
ports (7.0). Still only in linux-asahi: the GPU driver (Rust, `drm/asahi`, with Mesa's Honeykrisp), speakers/microphones
(macaudio plus the safety daemon in userspace), the SPI keyboard/trackpad for the newest laptops, sleep, the M3 blocks,
and an out-of-tree cpuidle driver that "can never be upstreamed". Thunderbolt and DisplayPort over USB-C are WIP everywhere.
Neural Engine is a separate module. Nothing for Touch ID or the secure enclave.

## 2. Run IYAKKI on an Apple Silicon Mac today (VM, all chips)

You need the image `iyakki-aarch64.img` (a UEFI disk image, GRUB + kernel + initramfs; `./run.sh build-arm` makes it on a
Linux PC, output `target/buildroot-libre-aarch64/images/`). Copy it to the Mac. The guest is a normal ARM64 Linux: the
Mac's 16 KiB host pages do not matter to it. The image boots through UEFI in QEMU on Linux (`dist/buildroot/qemu-uefi-aarch64-test.sh`);
it has **not been run on a Mac** (F-BOOT-08).

**A. UTM (easiest, free).** New, Virtualize, Other, choose **UEFI Boot**, skip the ISO,
RAM 3072 MB or more, 4 cores. Delete the default disk, add `iyakki-aarch64.img` as an NVMe or VirtIO drive. Display: `virtio-gpu-pci`
(no GL needed). Network: Shared. Start it. Details: `docs/BOOT-TEST.md` and `docs/INSTALL.md`.

**B. QEMU with the Hypervisor framework (fast, same CPU).**

```text
brew install qemu
cp "$(brew --prefix)/share/qemu/edk2-arm-vars.fd" iyakki-vars.fd
qemu-system-aarch64 -M virt -accel hvf -cpu host -m 3072 -smp 4 -name IYAKKI \
  -drive if=pflash,format=raw,readonly=on,file="$(brew --prefix)/share/qemu/edk2-aarch64-code.fd" \
  -drive if=pflash,format=raw,file=iyakki-vars.fd \
  -drive file=iyakki-aarch64.img,format=raw,if=none,id=hd0 -device virtio-blk-pci,drive=hd0 \
  -device virtio-gpu-pci -device qemu-xhci -device usb-kbd -device usb-tablet \
  -display cocoa -netdev user,id=n0,hostfwd=tcp:127.0.0.1:2222-:22 -device virtio-net-pci,netdev=n0
```

`-accel hvf` needs a Mac with Apple Silicon and an ARM64 guest (this one). Ctrl+Alt+G releases the mouse. These flags match
`./run.sh vm-arm` except `-accel hvf -cpu host` replaces the slow emulation (`-accel tcg -cpu cortex-a72`).

**C. Apple's Virtualization.framework (no third-party program).** A runner of about 40 lines of Swift using `VZEFIBootLoader`
(macOS 13 or later), a `VZVirtioBlockDeviceConfiguration` on the image, `VZVirtioGraphicsDeviceConfiguration`, a USB keyboard
and pointing device and NAT networking. Not written yet: UTM's "Apple Virtualization" backend does the same thing from a menu, so
write it only if a single-file runner is wanted.

Also, **natively on macOS without a VM**: `./run.sh window` runs the IYAKKI desktop window as a Mac program (F-SHELL-20).

## 3. How native boot works (Asahi chain)

1. The user runs the Asahi installer on macOS (`curl https://alx.sh | sh`). It resizes the APFS container, creates a **stub**
   macOS volume that holds **m1n1 stage 1** (MIT), signed by the machine's own policy, and the **ESP** and OS partitions.
2. At power-on the stub's m1n1 reads the `chosen.asahi,efi-system-partition` and `chainload=<ESP uuid>;m1n1/boot.bin` variables the installer
   baked in and runs **`m1n1/boot.bin` from the ESP**: m1n1 stage 2 + the device trees (`.dtb`) + **U-Boot** (Asahi's fork) as payload.
3. U-Boot provides UEFI; it starts `EFI/BOOT/BOOTAA64.EFI` (GRUB, as in our aarch64 image), which loads our kernel and initramfs.
   The kernel gets the device tree from m1n1/U-Boot and the screen from the framebuffer m1n1 set up (`simpledrm`).
4. **Firmware**: the installer downloads Apple's macOS restore image (IPSW) for the user's Mac and extracts the Wi-Fi, Bluetooth,
   camera and co-processor firmware into `vendorfw/` on the ESP (`firmware.cpio`, `firmware.tar`, `manifest.txt`). Apple's firmware is the
   user's copy from the user's machine; **IYAKKI never ships it** (same rule as `docs/MAC-HARDWARE.md` section 9). Our GRUB entry loads
   `/vendorfw/firmware.cpio` as an extra initrd so the files appear under `/vendorfw`; the kernel option `EXTRA_FIRMWARE_DIR`/`firmware_class.path`
   points there (an assumption to verify; Fedora copies the files to `/lib/firmware/vendor` with its own tool).

**Third-party OS package format** (from `asahi-installer`'s `src/osinstall.py`, `util.py` and the reference `installer_data.json`):

* `installer_data.json` has one key, `os_list`: entries with `name`, `default_os_name` (label in the macOS startup picker), `boot_object`
  (`m1n1.bin`, shipped inside the installer), `next_object` (`m1n1/boot.bin`, a path on the ESP), `package` (a zip: absolute URL, or a file
  under `$REPO_BASE/os/`), `supported_fw` (macOS versions the stub may be built from, e.g. `13.5`, `14.8.3`), optional `icon` (a file in the zip),
  optional `extras` (URLs downloaded into the ESP), and `partitions`.
* A partition has `name`, `type` (`EFI` or `Linux`), `size` like `524288000B`, optional `format: fat` and `volume_id`, `expand: true` (one
  partition takes the rest of the space the user chose), `copy_firmware`, `copy_installer_data` (writes `asahi/` metadata on it),
  and its content: `source: esp` (a folder `esp/` in the zip is copied into the FAT partition) or `image: x.img` (a raw image,
  size a multiple of 4 KiB, written to the partition).
* The reference file points the installer at it with `INSTALLER_DATA=<url>/installer_data.json` and `REPO_BASE=<url>`; the installer script is
  `src/install.sh` of the same repository (we serve our own copy of the metadata; we do not need alx.sh).

`dist/asahi/make-installer-package.sh` builds such a package and `dist/asahi/asahi_package.py validate` checks it against the rules above (all
of them read from the installer source, none from documentation; the installer does not publish a schema). Test: `dist/asahi/test-installer-package.sh`.

## 4. The `apple` image variant (plan)

| Piece | Choice | State |
|---|---|---|
| Kernel | `linux-asahi` (AsahiLinux/linux, GPL-2.0), a pinned tag, `ARM64_16K_PAGES`, Apple device trees t8103/t6000/t8112/t8122 | fragment written: `dist/buildroot/board/iyakki-apple/linux-asahi.fragment`, Buildroot options `dist/buildroot/configs/iyakki_apple.fragment`; pin in `dist/asahi/PINS` is **UNPINNED** |
| Boot payload | m1n1 (MIT) stage 2 + dtbs + Asahi U-Boot (GPL-2.0+) concatenated into `m1n1/boot.bin` | not packaged (needs the m1n1 and U-Boot builds, aarch64 toolchain) |
| Loader | GRUB arm64-efi, existing | exists in the aarch64 image |
| Root | the existing initramfs; `data.img` (ext4, the person's data) | to do: genimage for the ESP tree |
| Firmware | the installer's `vendorfw/`, loaded by GRUB as 2nd initrd; a Mac drivers page in Settings tells the person it came from their own macOS | to do (small) |
| Package | `make-installer-package.sh` | **done and tested with fake inputs** |
| Screen / input / disk | `simpledrm` boot framebuffer, SPI HID (laptops) or USB, NVMe (ANS), USB-C | options listed; **never resolved against the real tree** |

What was verified without hardware: the package generator, the validator and their tests; and every symbol of the kernel fragment was
looked up in the stock 6.12 tree: 18 symbols are **not** in 6.12 (SPI, SMC family, SPMI, USB DWC3/ATC, DCP, backlight, GPU, SPI HID, macaudio,
CS42L84), which is expected, they come from linux-asahi. `dist/asahi/verify-asahi-kernel-config.sh <tree>` repeats the check on the real
tree and then runs kconfig to find options dropped for an unmet dependency; do that first when the tree is cloned.

**Work and risk** (one person, alpha level; estimates, not promises):

| Step | Effort | Needs a Mac |
|---|---|---|
| 1. Clone and pin linux-asahi, m1n1, U-Boot; run the kernel verifier; fix the fragment | 1 day | no |
| 2. Build kernel + dtbs in Buildroot (16K pages, Rust for DRM_ASAHI, or drop that option); build time about 1 to 2 hours on 3 cores plus Rust host tools | 2 days | no |
| 3. Build m1n1 and Asahi U-Boot, concatenate, make the ESP tree and GRUB config, `apple` variant in `build.sh`, `./run.sh build-apple` | 2 days | no |
| 4. Package with `make-installer-package.sh`, serve it, dry run of the installer's parser on the output | 1 day | no (installer only runs on macOS to install) |
| 5. First boot on an M1 or M2 Mac: serial/framebuffer bring-up, input, firmware initrd, shell comes up | 3 to 5 days | **yes** (expect several rounds) |
| 6. M3 (framebuffer only): same kernel, extra device tree, test the SPI keyboard | 2 days | yes |
| 7. Wi-Fi/Bluetooth firmware flow, Settings page, docs, tests of the hardware list | 2 days | yes |

Image size: the same initramfs as `aarch64` (about 65 MB compressed) plus the Asahi kernel (about 40 to 50 MB), the dtbs and m1n1/U-Boot (a few MB), an ESP of
at least 500 MB (the installer's own examples use 500 MB) and the user's `vendorfw/` (about 70 MB, from the user's Mac). Main risks: Apple security
policy (the installer lowers the boot security of one volume to "reduced security"; the user must accept this), rapid changes in linux-asahi branches,
Rust in the kernel build, and nothing in this plan is testable in QEMU (QEMU has no Apple SoC).

## 5. Licences

| Component | Licence | Notes |
|---|---|---|
| m1n1 | MIT | stage 1 is installed on the user's Mac by the installer; stage 2 goes into `m1n1/boot.bin` |
| Asahi U-Boot | GPL-2.0+ | separate program in the boot payload; we distribute its source with the image |
| linux-asahi | GPL-2.0 | like our other kernels; source goes into `legal-info/` |
| asahi-installer, asahi-installer-data | MIT | we use the format; we may point users at the installer; no code copied |
| Mesa Honeykrisp (optional later) | MIT | not part of the alpha |
| Apple firmware (Wi-Fi, Bluetooth, camera, co-processors) | Apple's, not redistributable | extracted by the installer on the user's Mac from Apple's own download; **never in our image or package**; `asahi_package.py` refuses a package that contains it |

## 6. Questions for the owner

* Which exact Mac models ("M4 and M6": Mac mini M4? MacBook Air M4? Mac mini 2026 with M6?). M4 and M6 cannot boot IYAKKI on the metal; they run the VM (section 2).
  Is there an M1, M2 or M3 Mac to use for the native path?
* Is a Mac with a risk-free test partition available (the installer shrinks macOS)? Is the macOS version known (the installer needs 12.3 to 14.8 per its list)?
* Is the VM recipe enough for the alpha, with native boot as a later release?
