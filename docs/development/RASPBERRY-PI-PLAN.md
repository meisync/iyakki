# Raspberry Pi (and a note on NVIDIA Jetson): design and plan

**For:** the owner and whoever builds it. **You will get:** what each Pi model can run, how a Pi boots and what that means for our image, the kernel choice, a feature-by-feature list of what works or needs work on a Pi 5, the external-GPU experiment, the "IYAKKI lite" plan for the Pi 3, the build plan with what QEMU can and cannot test, licences, phases in days, numbered owner decisions with recommendations, and a hardware test sheet. **Status:** plan only, nothing built, nothing booted on a Pi (2026-10-26). No code, image or drive was touched to write this. Request: run IYAKKI on the owner's Raspberry Pi 5 (16 GB) and a second, lower Pi; possibly an external GPU on the Pi 5; NVIDIA Jetson later, separately.

Today (`docs/HARDWARE.md`): Raspberry Pi is **not supported**. This plan does not change that. The row may say "planned, see plan" and nothing more.

Honesty tags used below: **verified** (read in this repository, or fetched from the upstream page on 2026-10-26, source named), **from memory** (I know it from the Pi documentation or earlier reading but did not re-fetch it today; check before relying on it), **estimate** (a number I derived or a single community figure), **unverified** (could not be confirmed).

## 0. Which Pi, and what each can run

The owner owns a **Pi 5, 16 GB**, and a second board. The coordinator relayed on 2026-10-26: the second board is a **Raspberry Pi 3** (3B or 3B+ not stated). **Pi 3: asked and answered 2026-10-26.** That decision is closed; the plan below treats the Pi 3 as the "IYAKKI lite" board. The other models stay in the table only so the choice is on record.

| Model | CPU, RAM | Boots from | What IYAKKI could run (estimate unless noted) | Plan |
|---|---|---|---|---|
| **Pi 5** (4/8/**16** GB) | 4 x Cortex-A76 2.4 GHz, VideoCore VII, PCIe 2.0 x1 external, RP1 southbridge (from memory) | SD, USB, NVMe (via the PCIe M.2 HAT), through the board's SPI EEPROM bootloader (from memory) | The full image: shell, browser, k3s with quick-install items, local models on the CPU (slow, section 3.8) | **First target** (phases 1 to 4) |
| Pi 4B (2/4/8 GB) | 4 x Cortex-A72 1.5 GHz, VideoCore VI, no PCIe slot | SD, USB; EEPROM bootloader (from memory) | Full shell with 4 GB or more; k3s tight at 4 GB; no useful local models; software-rendered browser slower than Pi 5 | Not planned; the Pi 5 work covers most of it (needs `start4.elf` from the SD card, section 6.2) |
| **Pi 3B / 3B+** | 4 x Cortex-A53 1.2 / 1.4 GHz, **1 GB**, USB 2 only (Ethernet shares it), no PCIe, CYW43438 (3B) or CYW43455 (3B+) Wi-Fi/BT | SD (bootrom + `bootcode.bin` on the card); 3B+ also USB (from memory) | **IYAKKI lite only** (section 5): agent, flows, minimal web service, no shell, no k3s | **Second target** (phase 5), answered 2026-10-26 |
| Zero 2 W | 4 x Cortex-A53 1 GHz, **512 MB**, one OTG USB, CYW43436 | SD | A trimmed lite (no TLS-heavy flows, no models); the same image as the Pi 3 if it fits | Not planned |
| Pi 400 / CM4 | Pi 4 silicon | as Pi 4 | as Pi 4 | Not planned |

Small open question (decision 11): 3B or 3B+? The plan covers both; the differences that matter are Wi-Fi/BT firmware (section 6.3), 5 GHz Wi-Fi (3B+ only) and USB boot (3B+ out of the box).

## 1. Boot path

### 1.1 What the Pi does (from memory, check against the Pi documentation before step 1)

* **Pi 5 (and Pi 4):** the first stage is in the board's SPI EEPROM, which also carries the VideoCore firmware. The EEPROM reads the boot order (SD, USB, NVMe), finds a FAT partition, reads `config.txt`, `cmdline.txt`, a device tree (`bcm2712-rpi-5-b.dtb`), an optional initramfs and the kernel (`kernel_2712.img`, falling back to `kernel8.img`). **A Pi 5 needs no `start*.elf` and no `bootcode.bin` on the card** (the Pi 4 still needs `start4.elf` and `fixup4.dat`). This is the single most useful fact for the licence question: for the first target we need no Raspberry Pi firmware blob in the image at all (section 6.2). The EEPROM itself is updated by the person with Raspberry Pi's own tools; **IYAKKI never writes the EEPROM** (hard rule, decision 3 safeguards).
* **Pi 3:** no EEPROM. The SoC's ROM reads `bootcode.bin` from the FAT partition of the SD card, which loads `start.elf` and `fixup.dat` (VideoCore firmware), which read `config.txt`, the device tree and `kernel8.img`.
* **No UEFI, no GRUB** unless something provides them (below). The DTB is chosen by the firmware, which also patches it (memory size, MAC, overlays from `config.txt`).

### 1.2 Options

| | (a) Direct kernel boot | (b) UEFI (community edk2 for Pi 5) then GRUB | (c) U-Boot then GRUB or `extlinux` |
|---|---|---|---|
| What boots | EEPROM -> `config.txt` -> kernel + DTB + our initramfs (the stage-1 `/init`) | EEPROM -> `RPI_EFI.fd` -> `EFI/BOOT/BOOTAA64.EFI` (GRUB) -> kernel | EEPROM -> U-Boot (GPL-2.0+) -> script |
| Reuses our image | The initramfs, stage 1, data volume, update, rollback pointers (`current`/`previous`/seed) as they are. Replaces only GRUB's role | The aarch64 disk image almost unchanged (ESP + GRUB), plus the UEFI firmware file on the same FAT | GRUB or a boot script on top |
| State | Pi's documented path | **Verified (2026-10-26, `worproject/rpi5-uefi` README):** **archived, "no longer maintained"**; tested only on early **BCM2712C1** boards; newer **D0** boards "may need a newer EEPROM", reported to break graphical output; works in firmware: RP1 USB, PCIe, SD, HDMI display via VPU firmware; **not** working: RP1 Ethernet, RP1 GPIO and PWM, EEPROM for NVRAM; the bundled DTB targets the downstream 6.1 kernel; ACPI is incomplete. Licence: mostly BSD-2-Clause-Patent (EDK2 default), TF-A under its own licence (BSD-3-Clause, from memory) | Maintained for Pi 4 and earlier; Pi 5 support in mainline U-Boot: unverified |
| Secure Boot | none (see 1.5) | the edk2 build has no shim chain we control | none |
| Verdict | **Recommended** | Not for now: the project is archived and the D0 graphics risk is on the owner's likely board (the 16 GB Pi 5 is a late model; I believe it is a D0 stepping: unverified) | Fallback if `tryboot` (below) cannot give us rollback |

### 1.3 Recommendation: direct boot, with stage 1 doing GRUB's job

Stage 1 (`crates/iyakki-stage1`) already finds the `IYAKKI-BOOT` FAT and `IYAKKI-DATA` ext4 by label, reads `iyakki/root/current`, then `previous`, then the seed `boot/root.sfs`, and writes `boot/grub/iyakki-lean-failed` when a fast start fails (`docs/development/SELF-PREPARING-BOOT.md`, section 2). None of that needs GRUB. GRUB's only extra jobs are the menu and "after a failed fast start, pick the full initrd". On a Pi:

* `config.txt` is ours, small, and ends with `include iyakki-boot.txt` (the `include` option exists, from memory). `iyakki-boot.txt` holds one line, `initramfs initrd-lean followkernel` or `initramfs initrd followkernel`.
* **PROPOSED:** when stage 1 fails a fast start it rewrites `iyakki-boot.txt` to the full initrd (the same atomic write it uses for the marker) before the 30-second restart; the running system rewrites it back when the fast tree is ready. That keeps the two-stage logic and rollback pointers byte for byte as designed; only the "which initrd" switch moves from `grub.cfg` into a file.
* The menu entries (hardware check, safe graphics, remote diagnostics, web) become **named `cmdline.txt` variants selected the same way** (`iyakki-boot.txt` can `include` `cmdline`-like parameters? **unverified**: `cmdline.txt` is one file, so the entries are chosen by rewriting it from the running system, or through `[tryboot]` below). Honest limit: **no boot menu at power-on** with direct boot. The hardware-check and safe-graphics entries need either a serial console / keyboard-free switch or the `tryboot` feature or U-Boot. This is the real cost of (a).
* **`tryboot`** (Pi 4/5 firmware, from memory): `reboot '0 tryboot'` makes the next boot read `tryboot.txt` instead of `config.txt`, once. Use it for update trial boots (boot the new kernel once; if the system does not mark success, the next normal boot uses the old one). This maps onto our `current`/`previous` pointers. Details unverified; test in phase 1.
* Kernel command line: `console=tty1 console=ttyAMA10,115200` for the Pi 5 debug UART (the 3-pin header) is from memory; the existing GRUB lines use `ttyAMA0`, so the serial name must be checked on the board.

### 1.4 Partition layout (all of SD, USB, NVMe)

The firmware reads a **FAT32** partition; the ROM of a Pi 3 reads an **MBR** table (GPT: unverified on the Pi 3; the Pi 4/5 EEPROM reads GPT, from memory). Use **MBR on the Pi 3, GPT or MBR on the Pi 5** (default MBR for one layout everywhere, because the existing x86 installer already has an MBR layout and a 4Kn path).

| Partition | Label | Type | Size | Content |
|---|---|---|---|---|
| 1 | `IYAKKI-BOOT` | FAT32 | 256 MB (320 MB on 4Kn media) | `config.txt`, `iyakki-boot.txt`, `cmdline.txt`, `kernel_2712.img` or `kernel8.img`, `bcm2712-rpi-5-b.dtb` / `bcm2710-rpi-3-b-plus.dtb`, `overlays/`, `initrd`, `initrd-lean`, `boot/root.sfs` seed, release file. **Pi 3 only, downloaded at write time:** `bootcode.bin`, `start.elf`, `fixup.dat` |
| 2 | `IYAKKI-DATA` | ext4 | rest | accounts, models, `iyakki/root/` trees (marker `IYAKKI-FORMAT-ME-v1` on first boot, as today) |

Device names: SD `mmcblk0p1/p2`, NVMe `nvme0n1p1/p2`, USB `sdX1/sdX2` (all handled by `iyakki-usb`'s `drives.rs` and the HARDWARE.md notes). **Pi 5 NVMe** needs a PCIe M.2 HAT (the Pi's external PCIe lane, so it competes with the eGPU in section 4: one lane, one device) and EEPROM boot order containing NVMe (person sets it; from memory `PCIE_PROBE` and `BOOT_ORDER`).

### 1.5 Installer and safeguards

* `iyakki-usb` already writes images with read-back verify, refuses system disks, mounted disks and loop devices, and has a planner for MBR/GPT/4Kn. New work: a **Pi boot pack** next to `bootpack.rs`/`efi.rs` (the GRUB/ESP content) that lays out the FAT as in 1.4. I did not read `bootpack.rs` in depth; step 1 of phase 2 is to read it and decide reuse versus a sibling module.
* **Safeguards (carry over, plus Pi-specific):** (1) the disk that holds the running boot partition is never offered (on a Pi that is often `mmcblk0`; today's rules treat an SD slot as an internal disk, so a Pi needs a rule "offer the SD card only when the system runs from USB/NVMe/RAM, never the card we booted from"); (2) the NVMe system disk of a **PC** is never written by a Pi image tool and the reverse; (3) **never write the SPI EEPROM or any `mtdblock`/`mtd` device**; the drive list excludes `mtd*` (to be added to `drives.rs`; today it enumerates block devices only); (4) typed confirmation of name and size as today; (5) the image refuses to start on the wrong board: stage 1 reads `/proc/device-tree/model` and the boot partition's `board` file and stops with a plain message if a Pi 3 image meets a Pi 5.
* **Secure Boot:** the Pi has its own signed-boot scheme (EEPROM + OTP keys, from memory). `docs/SECURE-BOOT.md` (shim + MOK) does not apply; **the Pi would be outside the Secure Boot promise** (BACKLOG, "Secure Boot: in or out of the promise"). Say so in HARDWARE.md when the Pi is listed.

## 2. Kernel

| | Raspberry Pi downstream `raspberrypi/linux`, branch `rpi-6.12.y` | Mainline (our current `6.12.112`) |
|---|---|---|
| Pi 5 | Complete: RP1 (Ethernet, USB, GPIO, camera), `pcie-brcmstb`, vc4/v3d, PiSP, `bcm2712_defconfig` (from memory) | **Not enough at 6.12.** Phoronix/LWN (searched 2026-10-26): RP1 was still downstream-only in 2024; SUSE patches to upstream the RP1 driver and RP1 Ethernet were still being posted in 2025 (RP1 Ethernet speculated for v6.18); PiSP back end merged upstream late 2024, the front end and libcamera pipeline were in progress in March 2025. I could not confirm the state of v3d/vc4 for the Pi 5 in a given mainline tag |
| Pi 3 / 4 | `bcm2711_defconfig` family covers both (from memory) | Pi 3 and Pi 4 are in mainline for years; fine |
| Licence | GPL-2.0, source already the deliverable of `legal-info/` | same |
| Follows our version | the branch tracks the 6.12 LTS series, so the 6.12.x we build for others stays the base (verify the tag matches) | yes |

**Recommendation: use the Pi downstream 6.12 tree for the Pi 5** (a Buildroot custom tarball or git source at a pinned tag, `BR2_LINUX_KERNEL_CUSTOM_TARBALL_LOCATION`), and mainline-with-the-Pi-defconfig for the Pi 3 only if the downstream tree is not needed there (it is simpler to use one source for both). Our kernel fragment (`iyakki-aarch64/linux.fragment`) applies on top as it does on Apple's tree: add a small `iyakki-rpi/linux.fragment` for the Pi drivers and keep the shared options (zram, cgroups, nftables, WireGuard if the VPN plan lands). Size estimate: a 6.12 arm64 `Image` is 20 to 30 MB; the modules directory several tens of MB; the Pi OS kernel is a useful comparison, not measured here.

Open kernel questions (all **unverified**, answer in phase 1):

* **Page size.** The Pi 5 default kernel in Raspberry Pi OS uses **16 KB pages** (`kernel_2712.img`, from memory); some programs (jemalloc builds, some binaries, container tooling) break on 16 KB. We want **4 KB** for k3s and musl safety (`kernel8.img` style). I believe a 4 KB kernel boots a Pi 5; confirm on the board, and check the Rust musl binaries and the k3s/containerd arm64 binaries under both sizes.
* Which options the Pi needs on top of our fragment: `CONFIG_DRM_VC4`, `CONFIG_DRM_V3D`, `CONFIG_PCIE_BRCMSTB`, `CONFIG_MFD_RP1`/RP1 drivers and Cadence GEM Ethernet (`MACB`), `CONFIG_BRCMFMAC` + SDIO (the aarch64 fragment already has `BRCMFMAC_SDIO=y`), `CONFIG_I2C_BCM2835`, `CONFIG_SND_SOC_*` for HDMI audio, `CONFIG_BCM2835_THERMAL`/`RASPBERRYPI_*` mailbox drivers (for temperature and throttling), the Pi RTC and fan (PWM) drivers, `CONFIG_NVME` (already built in). The names are from memory; the list comes from `bcm2712_defconfig` itself in step 1, not from this page.
* **Device tree and overlays:** DTBs and overlays are built by the kernel (GPL-2.0 for the Pi tree; upstream `.dts` are often dual GPL/MIT) and copied by the board script. Overlays are enabled through `config.txt` (`dtoverlay=`): we offer none by default; Settings could later offer a short list (`vc4-kms-v3d` is already default behaviour on the Pi 5, from memory).

## 3. What works and what needs work on a Pi 5

Status column: "reuse" means the existing code should work once the kernel/drivers exist; "work" means a change; "new" means new code. Nothing was run on a Pi.

| Area | Expectation | What to do |
|---|---|---|
| 3.1 Shell on the display | The shell draws to `/dev/fb0` (`fb.rs`) or KMS (`backend_kms.rs`). The Pi 5's vc4 DRM driver gives an fbdev-emulated framebuffer on HDMI-A. `backend_kms.rs` picks the first `/dev/dri/cardN` with **two or more connected connectors** (`pick_card`); the Pi 5 has two micro-HDMI ports, so with one monitor it falls to the framebuffer, with two it should use KMS. Card numbering on the Pi 5 (display and v3d as separate DRM devices) is unverified | reuse; test both one and two monitors; the "safe graphics" entry already blacklists `vc4`, which on a Pi means no display at all because the simple framebuffer comes from firmware (`simpledrm`) only if the firmware sets one up: check on the board |
| 3.2 Input | USB keyboard, mouse (xHCI over RP1; `USB_XHCI_PLATFORM=y` is in the fragment). Touch: the official Pi display (DSI) needs its overlay: out of scope | reuse |
| 3.3 Sound | Pi 5 has **no headphone jack** (from memory). Outputs: HDMI audio (vc4-hdmi ALSA devices) and USB audio. The mixer uses `aplay -D <output>` with `plughw` (`AUDIO-MIXER.md`) | reuse; audio device naming and the default output pick need a Pi row in `audioout.rs` tests; HDMI audio only plays when a display is connected |
| 3.4 Wi-Fi and Bluetooth | On-board CYW43455 over SDIO (Pi 5), `brcmfmac`; Bluetooth over UART (`hciattach`/`btbcm`). Firmware: section 6.3. The shell's Wi-Fi code uses wpa_supplicant + `iw` (in the defconfig) | work: firmware placement, the board-specific NVRAM text file name (`brcmfmac43455-sdio.raspberrypi,5-model-b.txt`, from memory), regulatory domain; Bluetooth needs a patch-RAM file that is not in linux-firmware (same situation as the Mac, `FIRMWARE-LICENCES.md`): download at click |
| 3.5 Ethernet | Gigabit through RP1 (Cadence GEM). Needs the downstream RP1 drivers (section 2) | work |
| 3.6 Camera | **USB webcams first:** `USB_VIDEO_CLASS=m` is in the fragment, the camera bridge (`iyakki-hw`) reads V4L2. **CSI cameras** need libcamera and the PiSP pipeline: userland pieces under LGPL-2.1+ (libcamera), BSD-2-Clause (libpisp, rpicam-apps), IPA modules and tuning files with their own texts (from memory, **licence check not done**) | USB: reuse. CSI: later, behind its own decision (decision 9); not before phase 6 |
| 3.7 Browser | The built-in engine (`iyakki-web` + QuickJS) is software drawing and runs. The **full web engine** (WPE WebKit 2.50 helper, `FULL-BROWSER.md`) exists for x86-64 variants (`web`, `macweb`); the doc says the **aarch64 variant is not done**. Rendering in the helper is software (WPEBackend-fdo with shm buffers, Mesa llvmpipe), so the Pi's v3d GPU would not be used even if present. Expected speed (estimate): Pi 5 CPU is about 2 to 3 times a Pi 4 and probably comparable to an older x86 mini PC for page loading; YouTube at 720p with CPU decode plausible, 1080p60 doubtful. **The Pi 5 has no hardware H.264 decoder** (it has HEVC only, from memory), so H.264 is CPU decoded through the optional user-downloaded ffmpeg add-on path (`BETA-PLAN.md` section 10) | work: build `iyakki_web.fragment` for aarch64 (Mesa llvmpipe without LLVM issues on arm64; WPE cross-build hours); measure |
| 3.8 Local models | CPU only (no GPU compute on VideoCore for LLMs; v3d has Vulkan 1.2 through `v3dv`, from memory, but llama.cpp over it is not a practical path; unverified). Decode speed is memory-bandwidth bound: a Pi 5 has about 17 GB/s (from memory), so an 8B Q4 model (about 5 GB) has an upper bound of about 3 tokens per second. **A single community figure I found (searched 2026-10-26, benchmarklist.com, Pi 5 8 GB, llama.cpp): Llama-3.1-8B Q4_K_M about 2.1 tokens/s decode, Phi-3.5-mini (3.8B) Q4_0_4_4 about 4.4 tokens/s.** My estimates: 3B Q4 4 to 6 tokens/s, 1B Q4 10 to 15, 14B Q4 (9 GB, fits only on the 16 GB board) about 1. Prompt reading is slower than a laptop: long contexts take tens of seconds | work: the AI hardware profile (`aiprofile.rs`) says "No usable GPU: CPU only" and **hides Lemonade/Ollama on CPU-only machines** (`AI-HARDWARE.md` table). A Pi row is needed: recognise the board from `/proc/device-tree/model`, count RAM minus 4 GB, and show the small-model tools with the honest line "about 2 to 5 words per second" (decision 8). Also: llama.cpp for arm64 in pods needs an arm64 container image; unverified whether the project's Vulkan/CPU images have one |
| 3.9 k3s and the quick-install items | k3s ships arm64 binaries (from memory). Baseline: about 2 GB per local cluster is the number the project uses (`K3S-DEV.md`: "every local cluster is about 2 GB of memory, CPU cores and 3 GB of disk"). Pi 5 16 GB: one cluster plus the system is comfortable; 8 GB: one cluster, no big model beside it; 4 GB: not advised; Pi 4 4 GB: tight. Quick-install items: Elasticsearch is already "not recommended under 6 GB of RAM"; the data-science add-on (`DATA-SCIENCE-ADDON-PLAN.md`) and any image that is x86-only must be checked for arm64 images one by one | work: an arm64 audit of the catalogue (which Store entries have arm64 images); kernel options already in the aarch64 fragment |
| 3.10 TALES and the mixer | Read-aloud TTS (Kokoro etc.) runs on the CPU; the frame-time problems fixed on the T2 (`BACKLOG.md`, TALES section) were CPU copies of a 20 MB frame; a 1080p Pi 5 framebuffer is 8 MB (4K: 33 MB) and the Pi's memory bandwidth is lower than the Mac's. The mixer thread's own CPU cost is small | reuse; measure TALES frame time and the speech real-time factor on the board (test sheet) |
| 3.11 Thermals and power | Pi 5 throttles without a fan/heat sink (from memory). SYSTEM tile should show the SoC temperature and throttling flags if the kernel exposes them (hwmon) | work (small): read `thermal_zone0` and the firmware throttling flag; unverified path |
| 3.12 Updates and rollback | `iyakki --update`/`iyakki-usb --update` flow and the dev-mode update recipe | reuse after the Pi boot pack (1.4) |

## 4. External GPU on the Pi 5 (experiment only)

**Hardware:** the Pi 5 exposes **one PCIe 2.0 x1 lane** on an FFC connector (from memory; Gen 3 x1 can be forced with `dtparam=pciex1_gen=3`, **unofficial and not rated by the Foundation**, which the edk2 README also states: "Gen 3 is available but not officially rated"). A graphics card needs an adapter (M.2 HAT to OCuLink or an x16 riser) and its **own ATX supply**; the Pi supplies only the signal and the 3.3 V logic.

**Verified (searched 2026-10-26, Jeff Geerling's blog and press coverage):** AMD cards work on the Pi 5. The first demo used an RX 460 over an M.2-to-OCuLink adapter; his tag page later says a **stable `amdgpu` kernel patch** exists for **RX 400, 500, 6000 and 7000** series, with stable **Vulkan graphics and compute**. Reported remaining problems: memory-alignment bugs in some applications, Chromium UI freezes on the external card (Firefox fine), driver not fully compatible. **NVIDIA: not working** as far as I could find: a 2024 post says NVIDIA parts hit initialisation problems, and a reader reported a GTX 760 detected by `lspci` and then failing with "fallen off the bus". I found no current patch; check his blog before buying anything NVIDIA. The NVIDIA proprietary arm64 driver exists for Arm servers, but its use on the Pi 5's PCIe root complex is unverified; `nouveau`/NVK on the Pi is unverified.

**What it means for IYAKKI:**

* Needs the patched `amdgpu` in our Pi kernel (we have precedent for carrying patches: `apple-bce` 0001 to 0004) and the AMD GPU firmware, which our PC image already ships under `LICENSE.amdgpu` (`FIRMWARE-LICENCES.md`); the aarch64 fragment does not set `DRM_AMDGPU`.
* **k3s GPU (F-GPU-01, stage 1, Vulkan):** the card is found by the read-only PCI scan and the render node is handed to pods by the generic device plugin; Mesa RADV is in the pod image. Whether the project's llama.cpp Vulkan pod image has an arm64 build: unverified. **Stage 2 (ROCm, F-GPU-02) is not an option** (ROCm targets x86-64 hosts). Stage 3 (NVIDIA CUDA) is not an option on this board.
* **Models:** the x1 link mostly costs load time (estimate: 10 to 20 seconds for a 5 GB model over a Gen 2 x1 link; then decode runs at GPU speed from VRAM), so an 8 GB RX card could take a Pi 5 from about 2 tokens/s to tens (estimate; no measurement). The AI profile already counts a discrete card's VRAM minus 1.5 GB.
* The display could be driven by the card (HDMI/DP on it) or stay on the Pi's vc4; mixing is the kind of thing that fails in the shell's KMS card pick (`pick_card`).
* Risks: instability and data-bus errors, power (separate PSU, common ground), Gen 3 retraining failures, no Foundation support, our test budget (one board, one card), and the PCIe lane is **shared with NVMe boot** (one device, or a switch board: unverified).

**Recommendation:** an **experiment on the owner's bench** after phase 3, AMD only, never a supported target, never in the default image. A one-page experiment sheet is in section 9.3. Do not buy NVIDIA hardware for this.

## 4b. NVIDIA Jetson Orin / Nano: a different port

Not part of this work. Why it is a separate item:

* A Jetson boots through NVIDIA's own chain (bootloaders in QSPI flash flashed by NVIDIA's host tool, then UEFI or extlinux with NVIDIA's device trees), not the Pi's firmware and not generic UEFI. (From memory; recent JetPack versions moved toward UEFI on Orin.)
* The supported kernel is NVIDIA's **L4T/Jetson Linux** kernel with its own device trees and out-of-tree drivers (display, GPU, power, camera). Mainline support for Orin is partial (unverified). So the kernel is a vendor tree again, like Apple's for the Macs and the Pi's here, but with a much larger out-of-tree part.
* The point of a Jetson is the **GPU compute**, which needs NVIDIA's **proprietary userspace** (CUDA, cuDNN, TensorRT, the Tegra graphics libraries). By our rule these are **downloaded by the person at their click from NVIDIA, never shipped** (`FIRMWARE-LICENCES.md`, "NVIDIA driver user-space"; `GPU-K3S.md` stage 3 is the pattern, with NVIDIA's installer terms shown first). Whether the Jetson userspace can run on our musl image (NVIDIA ships glibc binaries) is unverified and is probably the hardest problem: it likely needs a glibc container or a glibc variant.
* Flashing uses NVIDIA's host tool on a Linux PC; a stick image like ours is not enough (unverified for new boards).

Recommendation: write nothing now; after the Pi 5 phase 2 ask the owner for the exact Jetson model and decide then (decision 12).

## 5. IYAKKI lite for the Pi 3 (3B / 3B+, 1 GB)

Decision made (2026-10-26): the Pi 3 gets **IYAKKI lite**, not the shell and not k3s. Reasons, in the order that matters: 1 GB RAM, USB 2 shared with the Ethernet chip, the Cortex-A53 cores, SD-card I/O.

**What it is:** the `iyakki` binary in a headless profile: agent (chat and tools against **cloud or remote** models), flows scheduler, data tools, MCP, the vault, the HTTPS/WebSocket service of the minimal-web-service plan (`MINIMAL-WEBSERVICE.md`: service profiles, reading A), `iyakki dev` over the network or USB gadget (`USB_GADGET`, `CONFIG_USB_CONFIGFS_NCM` are in the fragment; the Pi 3's OTG port is the only one with device mode: unverified). A person uses it from a browser on another machine. This plan depends on the minimal-web-service work; until that has a headless profile, the Pi 3 lite image is "SSH and CLI only".

**What fits in 1 GB (all estimates until measured on the board):**

| Part | Memory | Note |
|---|---|---|
| Kernel, modules, firmware in use | 40 to 60 MB | smaller kernel config for the Pi 3 (no GPU stack, no PCIe, no NVMe, no DRM at all) |
| Unpacked system (RAM root) | **target under 90 MB** vs about 170 MB for the standard image | cut: the Noto fonts the shell ships, Mesa, WPE helper, k3s tooling, the shell tiles' assets, sample data, extra locales, wasmtime kept (sandbox for tools) but profile-gated features off. Use the **lean first stage** so the big tree lives on the data volume and not twice in RAM (`SELF-PREPARING-BOOT.md`) |
| `iyakki` service (agent, flows, web) | 60 to 150 MB | grows with chats and flow runs |
| zram | 25 % of RAM as a ceiling | the default tuning already exists (`iyakki-zram`); on 1 GB leave it on |
| Free for page cache and spikes | 500 MB or more | a TLS handshake storm or a data-tool job is the risk |

**What to cut, explicitly:** the framebuffer shell and every tile; the built-in browser and the WPE helper; k3s, containerd, the Store's cluster items, all GPU and model-host cards; local LLMs (a 0.5B model may run at 1 to 3 tokens/s on the A53: not worth shipping; use cloud or a Pi 5/Mac model host through the Models settings); Elasticsearch, Polars engine and heavy data add-ons; TTS voices; audio mixer and radio unless a USB sound card is used; camera bridge. Keep: Ethernet and Wi-Fi (3B+: dual band; 3B: 2.4 GHz only), SSH, update/rollback, vault, flows, data tools, the web service, the firewall and VPN helper if built.

**Concerns:** the A53 has no ARMv8 crypto extension (from memory), so TLS with AES-GCM is software; prefer ChaCha20 where the peer allows, and measure the handshake rate. SD-card wear: keep logs in RAM, the data volume on a good card, or boot from a USB stick on a 3B+ (USB 2 speeds). 3B+ Ethernet tops out near 300 Mbit/s (USB 2 bridge, from memory).

**Build:** a Pi 3 defconfig on the same aarch64 base with `BR2_cortex_a53`, kernel `bcm2711_defconfig`-family or mainline arm64 defconfig plus a Pi 3 fragment, profile-trimmed overlay, and the Pi 3 boot files downloaded at write time (6.2).

## 6. Build plan

### 6.1 Image recipe (incremental only)

Owner rule (`feedback-incremental-builds`): build only what changed, reuse outputs. So:

* **No new full Buildroot run for userland.** The aarch64 userland (musl toolchain, BusyBox, Rust workspace for `aarch64-unknown-linux-musl`, dropbear, wpa_supplicant, iproute2, alsa, GRUB not needed) is already what `iyakki_aarch64_defconfig` builds. Add `dist/buildroot/configs/iyakki_rpi.fragment` (kernel source and fragment only: `BR2_LINUX_KERNEL_CUSTOM_TARBALL...`, no GRUB, no genimage ESP) and `board/iyakki-rpi/` (`config.txt`, `cmdline.txt`, `genimage.cfg` with an MBR table, `post-image.sh`). One **kernel-only** build with a new output directory, then `repack.sh --kernel-from` / graft style to put the Pi kernel and modules into the existing aarch64 root, as `repack.sh macweb` does for x86. `repack.sh` is documented "x86_64 only"; extending it for aarch64 and the Pi boot files is one of the build tasks (about 2 days, phase 2).
* The target CPU: keep the generic ARMv8-A target the aarch64 defconfig already uses for the shared userland (it runs on the Pi 3's A53 and the Pi 5's A76 alike). Only the kernel differs per board. A `cortex-a76` userland would add speed (llama.cpp on the Pi 5 benefits from dot-product and i8mm-class features that the A76 has partly: **unverified which**) but breaks the one-image rule; llama.cpp comes in a pod/image of its own anyway.
* The Buildroot tree has its own Raspberry Pi 5 defconfig (`raspberrypi5_defconfig`; I believe, **unverified**). Read it for the boot-file list, **do not use its `rpi-firmware` package** for the Pi 5: it downloads Broadcom binaries into the image at build time.

### 6.2 Pi boot firmware (what is shipped and what is downloaded)

**Verified (fetched 2026-10-26, `raspberrypi/firmware`, `boot/LICENCE.broadcom`):** binary redistribution and use, "without modification", "provided that" (1) "This software may only be used for the purposes of developing for, running or using a Raspberry Pi device" (or an authorised derivative device from the element14 customisation service), (2) the copyright notice and disclaimer are reproduced "in the documentation and/or other materials provided with the distribution", (3) Broadcom's name is not used to endorse. It says nothing about reverse engineering or about operating-system licences. That covers `bootcode.bin`, `start*.elf`, `fixup*.dat` (the file names are from memory; I did not list the directory).

What can be shipped, by the project's own rule ("a file whose licence we are not sure we may redistribute is not in the images we publish"):

* **Pi 5: nothing from this set is needed** (1.1). The first target ships no Raspberry Pi VideoCore blob. The DTBs, overlays and kernel are built from GPL-2.0 source.
* **Pi 3 / Pi 4: `bootcode.bin`, `start*.elf`, `fixup*.dat` are needed.** The text is clear and permits binary redistribution for Pi devices, so shipping inside a Pi-only image would be defensible, but "only for a Raspberry Pi device" is a use restriction like the Broadcom Wi-Fi text we already carry only for matching hardware, and the owner has no lawyer. **Recommendation: not shipped; `iyakki-usb` downloads them at write time** from the Foundation's repository at a pinned commit, with size and SHA-256 checks and the licence shown and accepted first, using the machinery behind `fetch.rs` and `RESTRICTED-FIRMWARE.md` (a pack "Raspberry Pi boot firmware"). The licence file travels with the files. The same pack is what a person who builds their own Pi 3 image uses (`IYAKKI_INCLUDE_RESTRICTED_FIRMWARE=1` pattern).
* **EEPROM images** (`rpi-eeprom`): never shipped, never written by us.

### 6.3 Wi-Fi and Bluetooth firmware

* **CYW43455** (Pi 3B+, Pi 4, Pi 5): the files `brcmfmac43455-sdio.*` live in linux-firmware under the Cypress/Broadcom texts. `FIRMWARE-LICENCES.md` already audited `LICENCE.cypress` and `LICENCE.broadcom_bcm43xx`: unmodified binary, "solely for use with Cypress/Broadcom integrated circuit products", licence text shipped. A Pi's chip is exactly that product, so the existing rule is met for these files in a Pi image; I could not re-fetch `LICENCE.cypress` today (the URL I tried returned 404), so the conclusion rests on the repository's earlier audit, not on today's reading. The board-specific NVRAM text files and CLM blob: check `WHENCE` for the licence line of the Pi names (unverified).
* **CYW43438** (Pi 3B) and CYW43436 (Zero 2 W): not covered by the earlier audit; licence unverified. **Download at click** until read.
* **Bluetooth patch RAM** (`BCM4345C0.hcd` and kin): not in linux-firmware, Raspberry Pi packages it in `bluez-firmware`; licence not read. Treat like the Mac's: never shipped, offered as a pack.
* `LICENCE.*` files are copied into `/usr/share/doc/linux-firmware/` by `firmware-licences.sh` as for every image.

### 6.4 What QEMU can test and what only the board can

| Test | QEMU (`virt`, `-cpu cortex-a76` or `cortex-a53`) | QEMU `raspi3b` / `raspi4b` | Only on the real Pi |
|---|---|---|---|
| Rust workspace tests, aarch64 musl binary starts | yes (already done: `qemu-test-aarch64.sh`) | partly | |
| Stage 1 against Pi-style partitions (MBR, FAT label, ext4 label, pointers, failure -> rewrite `iyakki-boot.txt`) | yes, with a disk image and a direct kernel boot, no firmware needed | | |
| Boot-pack writer output (file list, `config.txt` text, sizes), installer refusals, no `mtd*` | yes (unit tests, no QEMU at all) | | |
| 4 KB vs 16 KB page-size behaviour of the binaries | yes under `virt` (TCG, slow; 16 KB support in TCG: unverified) | | confirm on the board |
| Pi 3 kernel + DTB boot to the self-test (UART, SD, basic Ethernet) | | `-M raspi3b` exists in QEMU; device coverage is partial (USB, Wi-Fi, GPU absent). A Pi 3 DTB boot to `IYAKKI-SELFTEST` is plausible, **unverified** | the rest |
| Pi 5 | no `raspi5` machine to my knowledge (**unverified**: check the QEMU version in use) | | everything board-specific: EEPROM boot, RP1, vc4/v3d, HDMI, USB, NVMe, PCIe, Wi-Fi, fan, power |
| `tryboot`, firmware `config.txt` handling, DTB patching | | | yes |
| Performance numbers, thermals, throttling | | | yes |

### 6.5 Existing scripts to extend

* `qemu-test-aarch64.sh`, `qemu-uefi-aarch64-test.sh`: unchanged. Add `qemu-rpi-stage1-test.sh` (direct kernel + MBR disk) and optionally a `raspi3b` smoke test.
* `verify-kernel-config.sh`: add the Pi fragment's required options.
* `changed.sh`/`smart-build.sh`: a new variant name `rpi5-aarch64` and `rpi3-aarch64`; `iyakki-release` and `/etc/iyakki-release` carry it; the developer-mode update recipe checks the name (the Mac variant-name mismatch in `BACKLOG.md` is the warning).

## 7. Licences and SBOM entries

To add to `LICENSING.md` (as an item) and the SBOM when built; none applied now:

| Piece | Licence | Shipped? |
|---|---|---|
| Raspberry Pi kernel tree (`raspberrypi/linux`, rpi-6.12.y), DTBs and overlays built from it | GPL-2.0 (device-tree sources often GPL-2.0 or MIT: per file) | yes; source through `legal-info/` (a custom-tarball kernel is covered by `make legal-info` as for other custom kernels: check) |
| Pi boot blobs `bootcode.bin`, `start*.elf`, `fixup*.dat` (Pi 3/4) | Broadcom binary licence (verified above) | **no**; downloaded at write time with the licence shown |
| Pi EEPROM images | Broadcom licence (same repository family) | never |
| CYW43455 Wi-Fi firmware | `LICENCE.cypress` / Broadcom text (earlier audit) | yes, as Mac images, if the plan's firmware step confirms the file names |
| CYW43438/43436 Wi-Fi, Pi Bluetooth patch RAM | not read | download at click until read |
| Patched `amdgpu` (eGPU experiment) | GPL-2.0; AMD firmware `LICENSE.amdgpu` as in the PC image | experiment image only |
| libcamera, libpisp, rpicam-apps (CSI, later) | LGPL-2.1+, BSD-2-Clause, BSD-2-Clause (from memory, unchecked) | not in the first slices |
| edk2 Pi 5 UEFI (if (b) is ever used) | BSD-2-Clause-Patent; TF-A BSD-3-Clause (from memory) | not planned |
| U-Boot (if (c)) | GPL-2.0+ | not planned |
| Trademark | "Raspberry Pi" is the Foundation's trademark | nominative use only, with the usual non-endorsement sentence |

## 8. Phases, sizes, risks

Days are one person plus agents, working days, not including waiting for builds; the owner's board time is separate.

| Phase | What | Days | Exit |
|---|---|---|---|
| 0 | This plan, decisions 1 to 6 answered; owner updates the Pi 5 EEPROM with the Foundation tools and notes the board stepping and EEPROM version | 0.5 (owner) | decisions recorded |
| 1 | Pi 5 kernel: pinned `rpi-6.12.y` tag, 4 KB pages, fragment, kernel-only build, boot **headless on serial with a stock-style initramfs** (our stage 1 + `iyakki`), Ethernet, USB, SD | 4 | `IYAKKI-SELFTEST` end on the board over serial and SSH |
| 2 | Boot pack and installer (`iyakki-usb` Pi layout, safeguards, `iyakki-boot.txt` flip in stage 1, `repack.sh` aarch64), SD and USB boot, update/rollback on the board | 6 | write a stick, boot, update, roll back, recover from a failed fast start |
| 3 | Shell on HDMI (fbdev, KMS), keyboard/mouse, sound (HDMI, USB), Wi-Fi + firmware, NVMe boot with the M.2 HAT, SYSTEM temperature | 6 | the Pi 5 test sheet, section 9.1, passes |
| 4 | k3s + quick-install items on arm64, memory fit on 16 GB (measure), Pi AI-profile row, honest model speeds, browser aarch64 (WPE) | 8 | k3s up, one model answering, browser loads a normal site; numbers written into this doc |
| 5 | IYAKKI lite for the Pi 3: trimmed profile and kernel, boot files download pack, headless service, memory budget measured on the board | 6 (plus the web-service profile work, estimated in `MINIMAL-WEBSERVICE.md`) | Pi 3 sheet 9.2 passes |
| 6 | Later, separately decided: CSI camera, Pi 4, eGPU experiment (3 days of bench time if the owner buys the parts), Jetson | n/a | |

Total for phases 1 to 4: about 24 working days; with phase 5, about 30. Risks, most likely first: (1) the 16 GB Pi 5 may be a newer stepping with an EEPROM/firmware quirk we cannot see without the board; (2) 4 KB pages on the Pi 5 kernel may need patches; (3) the Pi kernel tree moves faster than our 6.12.112 base; (4) no power-on menu in direct boot (hardware-check, safe-graphics entries); (5) WPE on aarch64 is a multi-hour cross build and unmeasured; (6) software-rendered browser and k3s together on the CPU may feel slow; (7) SD-card corruption on power cut (the system runs from RAM and the data volume is ext4: same behaviour as the stick, but cards are worse); (8) `aiprofile` hides model tools on CPU-only machines unless changed.

Open questions: exact Pi 5 stepping and EEPROM date; whether the Foundation's firmware/EEPROM licence text applies to files in other Pi repositories (`rpi-eeprom`, `bluez-firmware`); `tryboot` behaviour with an initramfs; Pi 5 card numbering for KMS; 16 KB vs 4 KB compatibility of the k3s binaries; whether stage 1's FAT write is safe on a Pi's firmware FAT driver (case, long names, `config.txt` size limits).

## 9. Hardware test sheets (for the owner, when there is something to boot)

### 9.1 Pi 5 (16 GB)

1. Note: the label under the board (revision), `vcgencmd bootloader_version` from the stock OS **before** wiping (date and version), RAM, power supply (27 W USB-PD recommended, from memory), cooler fitted yes/no.
2. Boot from SD, then USB stick, then NVMe (with HAT): power on to sign-in time; record whether the hardware-check entry exists and works.
3. Serial log (3-pin debug header) saved; `dmesg` after sign-in saved.
4. HDMI: one monitor, two monitors, 1080p and 4K, hot-plug; does the shell use the framebuffer or KMS (`IYAKKI-HWREPORT`)?
5. Keyboard, mouse, USB hub, USB audio, HDMI audio (volume works, radio plays without breaks for 10 minutes).
6. Ethernet link and speed; Wi-Fi join, 5 GHz; Bluetooth (expected: needs the firmware pack).
7. USB webcam in the Camera tile and in a browser page.
8. Browser tile: three normal sites, a 720p video, a 1080p video: dropped frames and CPU use.
9. K3S: start the cluster, deploy a quick-install item, record free memory (`/proc/meminfo`, SYSTEM tile). Then a 3B and an 8B model: tokens per second (the Models page), load time, temperature at the end.
10. TALES read-aloud: frame times (the frame log) and whether speech keeps up.
11. Update: apply an update, reboot, roll back.
12. Power cut during a write (pull the cable) three times: data volume intact?
13. Thermal: 15 minutes of a model prompt loop; max temperature; throttling flag.

### 9.2 Pi 3 (lite)

1. Model (3B or 3B+), power supply, card brand/size.
2. Boot, time to the network, SSH from another machine.
3. Free memory at idle, after 10 minutes of a flow every minute, after a chat with a cloud model, with the web service open in a browser from another PC.
4. Wi-Fi band and speed; Ethernet speed (expect USB 2 limits).
5. Power cut x3; update and roll back.
6. Temperature without a heat sink under the flow loop.

### 9.3 eGPU experiment (Pi 5, AMD only)

Record card model, adapter, PSU, kernel tag and patch source; `lspci -vv` link speed and width; `dmesg` for amdgpu messages; Vulkan device listing; llama.cpp Vulkan token speed with the same model as 9.1 item 9; 30-minute stability run; reboot loop (10 times) to see if the card comes back.

## 10. Feature matrix rows to propose (ids only; `FEATURES.md` is not edited)

* `F-RPI-01` Raspberry Pi 5 direct-boot image (kernel, boot files, stage-1 switch), SD/USB.
* `F-RPI-02` `iyakki-usb` Pi boot pack and safeguards (no EEPROM/mtd, running-card refusal, board check).
* `F-RPI-03` Pi 5 shell on HDMI (fbdev/KMS), input, sound.
* `F-RPI-04` Pi 5 Wi-Fi/Ethernet/Bluetooth with firmware handling.
* `F-RPI-05` Pi 5 NVMe boot (M.2 HAT).
* `F-RPI-06` Pi 5 k3s, quick-install arm64 audit and memory-fit figures.
* `F-RPI-07` Pi hardware profile row in AI-HARDWARE (CPU-only small models, honest speeds).
* `F-RPI-08` IYAKKI lite on the Pi 3 (headless profile).
* `F-RPI-09` Pi boot firmware download pack (Pi 3/4).
* `F-RPI-10` Pi 5 CSI camera (libcamera/PiSP), later.
* `F-RPI-11` Pi 5 external GPU experiment (AMD), experimental.
* `F-RPI-12` Pi 5 browser (WPE aarch64).

## 11. Owner decisions

1. **Which second board?** Answered 2026-10-26: **Pi 3** (3B or 3B+: see decision 11). Closed.
2. **Start with the Pi 5 only, then the Pi 3?** Recommended yes: Pi 5 phases 1 to 4, Pi 3 lite in phase 5. The Pi 3 work depends on the minimal web service anyway.
3. **Direct boot, UEFI, or U-Boot?** Recommended **direct boot** with stage 1 flipping `iyakki-boot.txt`; accept "no power-on menu" for now; U-Boot only if `tryboot` is not enough; UEFI not now (archived project, D0 risk). Also confirm the safeguard that IYAKKI never writes the EEPROM or `mtd` devices.
4. **Kernel source and page size?** Recommended the Raspberry Pi `rpi-6.12.y` tree for the Pi 5, **4 KB pages**; mainline for the Pi 5 after RP1 and Ethernet land upstream (revisit at 6.18 or later).
5. **Headless first?** Recommended yes: serial and SSH first (phase 1), shell on HDMI in phase 3.
6. **Pi firmware: ship or download?** Recommended: **nothing from the Foundation's boot set is shipped** (the Pi 5 needs none; Pi 3/4 files are downloaded at write time with the licence shown); CYW43455 Wi-Fi firmware shipped under the already audited Cypress/Broadcom text; every other Pi radio file and Bluetooth patch RAM downloaded at click until its text is read.
7. **Support level?** Recommended: HARDWARE.md says "planned, see plan; not supported today" now, "experimental" after the Pi 5 sheet passes, "supported" only after the sheet passes twice (SD and NVMe) and an update/rollback works on the board. Nothing is claimed before.
8. **Local models on a CPU-only Pi 5:** show Lemonade/Ollama with small models and an honest speed line (about 2 to 5 words per second for 3B to 8B; estimates), or keep them hidden (current CPU rule)? Recommended show, with the warning, on 8 and 16 GB boards only.
9. **CSI cameras:** later, after USB. Recommended defer to phase 6; needs a libcamera/PiSP licence check first.
10. **External GPU:** recommended an **experiment only**, AMD RX 6000/7000-class or older RX 400/500 per the patched driver reports, never a supported target, never NVIDIA; buy only after phase 3 passes; the owner decides whether to buy a card, adapter and PSU.
11. **Pi 3B or 3B+?** (and Pi 4 later?) Recommended: ask which one; the plan covers both, 3B+ preferred for 5 GHz Wi-Fi and USB boot.
12. **Jetson:** recommended no work now; decide the model and the glibc-userspace question after the Pi 5 phase 2.
13. **Names and wording:** "Raspberry Pi" used nominally with a non-endorsement sentence (LICENSING item); recommended yes.

## 12. What I could not verify

* Everything marked "from memory": the Pi 5 boot sequence and file names, that the Pi 5 needs no `start*.elf`, `config.txt` options (`include`, `initramfs ... followkernel`, `tryboot`), `BOOT_ORDER`/`PCIE_PROBE`, 16 KB default pages, RP1 contents, the Pi 5 having no headphone jack and no H.264 hardware decoder, Pi 3 USB boot specifics, A53 crypto extensions, Buildroot's own `raspberrypi5_defconfig`.
* Whether `LICENCE.cypress`/`LICENCE.broadcom_bcm43xx` apply unchanged to the Pi board files (I relied on the repository's earlier audit; the fetch of today's copy returned 404); the licences of the CYW43438/43436 firmware, the Bluetooth patch RAM files, `rpi-eeprom`, libcamera/libpisp/IPA modules.
* The 16 GB board's SoC stepping (D0 assumed) and whether the EEPROM state affects our boot.
* The state of v3d/vc4 and PiSP front-end in a given mainline tag; the exact Pi tree tag for 6.12 and its option list.
* The Pi 5 under a 4 KB-page kernel; `tryboot` with an initramfs; the DRM card numbering; `raspi3b`/`raspi4b` coverage of our boot; whether QEMU has a Pi 5 model.
* The eGPU: only the AMD successes and the NVIDIA failures reported on Jeff Geerling's pages (via search results, 2026-10-26; I did not fetch the articles). The x1 load-time and tokens-per-second figures are my estimates; no ROCm/Vulkan arm64 pod image check; PCIe-switch boards unverified.
* Token speeds: one community leaderboard (Pi 5 8 GB) for 8B Q4_K_M (about 2.1 tokens/s) and a 3.8B model (about 4.4); all other speeds, memory budgets for the Pi 3 lite, and image sizes are estimates. The 14B-on-16-GB figure is derived, not measured.
* Jetson boot and userspace statements are from memory and were not searched.
* Nothing was built, booted or downloaded into the repository.
