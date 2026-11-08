# HP laptop bring-up plan (hand-off brief)

Status: plan written 2026-11-07. Nothing has run on this laptop yet. This page is written to be handed to another model or engineer who has **not** seen the conversations that produced the rest of the repository: it says what IYAKKI is, what to do in what order, how to reach the machine, what to measure, the rules that apply, and what to write down at the end. The exact HP model is **not known yet**; step 1 finds it. Where this page says "the owner", it means the person who owns the laptop and the repository and approves anything risky.

## What IYAKKI is, in four lines

IYAKKI is an operating system for AI agents: a Rust workspace (`crates/`) plus a Buildroot-built Linux image. It boots to its own shell (tiles: Home, Chat, Browser, Settings, Store, Voice, ...), keeps its data on a separate volume, runs k3s for local services (Lemonade for models, Ollama, and others), and updates itself from an update pack with the previous version kept in the boot menu. Start reading at `README.md`, `docs/ARCHITECTURE.md`, `docs/HARDWARE.md`, `docs/BOOT-TEST.md`, `docs/DEVELOPER-MODE.md`, `docs/project/HARDWARE-MATRIX.md`.

## The goal

On this HP laptop, in the owner's order of importance: **(1) it boots, (2) Wi-Fi and Bluetooth work, (3) the update framework can put a new image on it and roll back, then (4) sound and microphone, (5) display, touch and the browser, (6) battery and suspend, (7) the GPU for local models**. Each item ends as one row in the hardware matrix with *verified / partly / failing / untested*, the date and the evidence. Anything not actually seen working is written as untested; do not round up.

## Step 0: before touching the laptop

1. Read the owner rules below. Build nothing the owner has not asked for; do not commit or push unless asked.
2. Know the two machines this has already been done on, because HP laptops share most of their trouble with them:
   * A Lenovo Legion Pro 5 (AMD Ryzen + Radeon iGPU + NVIDIA dGPU, Realtek audio codec, **MediaTek** Wi-Fi/Bluetooth). It boots the PC image. Findings: Wi-Fi/Bluetooth firmware is not shipped and is installed by the person with a button (licence acceptance is the person's click); the mixer starts muted or low at every boot; the ALSA percent scale is in decibels (70 % is about 40 % of the raw range); Settings -> Sound -> Advanced has the Boost row.
   * A T2 Mac mini and MacBook Pro (Intel + T2). Different problems (Apple firmware); only useful as a source of the update-framework recipe.

## Step 1: find out what the machine is (no installation)

1. Boot the **hardware check** entry of the stick/image (`docs/BOOT-TEST.md`), or a live Linux, and record: product name and version (`/sys/class/dmi/id/`), CPU, RAM, storage controller (NVMe? RAID/Intel VMD can hide the disk), GPU(s) (`lspci -nn`), Wi-Fi and Bluetooth chip (vendor:device ids, USB or PCIe), audio codec (`/proc/asound/card*/codec#0`), touch screen / touchpad, fingerprint, camera, firmware mode (UEFI, Secure Boot state, TPM), BIOS version.
2. Decide, from the chip ids and `docs/FIRMWARE-LICENCES.md` / `crates/iyakki-agent/src/fwpack.rs`, which firmware pack the Wi-Fi and Bluetooth need (Realtek, MediaTek, Intel, Broadcom, Qualcomm/Atheros are the catalogue's families). Write the answer down before booting IYAKKI.
3. Open question to settle with the owner here: is the HP an AMD or an Intel machine, and does it have an NVIDIA GPU? It changes step 7 only.

## Step 2: boot

* Use the image variant the Lenovo used: the signed PC image (`dist/buildroot/lean-image.sh`, see `docs/BUILD-INCREMENTAL.md`) written to a stick. Secure Boot: the image is signed with the project's key; a laptop with Secure Boot on must either enrol that key (MOK flow) or have Secure Boot turned off in the BIOS; ask the owner which. The key itself is kept outside the repository and must not be printed or copied.
* **Boot-test every new image in QEMU (OVMF) before it goes near the laptop.** A PID-1 panic once reached a device because this was skipped.
* **First result, 2026-11-07:** the same loader-default stick that boots the Mac mini, the MacBook Pro and the Lenovo (Secure Boot on, the
  IYAKKI key in MOK on all of them) stops on the HP. The loader's log on the stick (`EFI/iyakki-boot/last-boot.log`, `docs/BOOT-LOADER.md`)
  answered step 1 and located the stop: **HP Pavilion Laptop 16-af0xxx**, board 8CB6, Insyde BIOS F.11 (2024-09-26), Secure Boot on, TPM 2.0,
  32 GB; Intel Meteor Lake (host bridge 8086:7d02, Arc iGPU 8086:7d45 driving the 1920x1200 panel, 2 touch devices), NVIDIA 10de:25aa as a
  second GPU without display output, WD NVMe 15b7:5036 on a plain PCIe bridge (**not** behind VMD; Windows on it), Realtek RTL8852BE Wi-Fi
  10ec:b852 (Realtek firmware pack) with its Bluetooth on USB 0bda:b85c, camera 05c8:0445, the stick on the USB4/Thunderbolt xHCI 8086:7ec0.
  The loader ran to the end: shim and MOK accepted it, the menu drew, the kernel and RAM disk were read, `LoadImage`/`StartImage` of the kernel
  never returned, and stage 1 never wrote its log. So the kernel starts and dies before stage 1.
* **Cause found in the kernel configuration:** `CONFIG_X86_X2APIC` and `CONFIG_IRQ_REMAP` were not set. Meteor Lake firmware enables x2APIC
  (and may lock the legacy xAPIC off); a kernel without x2APIC support then disables the APIC altogether (`check_x2apic`: "Kernel does not
  support x2APIC, please recompile with CONFIG_X86_X2APIC", an error line that `quiet loglevel=3` hides) and the start dies with no output.
  The Macs and the older Lenovo run in xAPIC mode, which is why the same kernel boots them. Both options are now in `linux.fragment`
  (`crates/iyakki-core/tests/kernel_platform.rs`); the next kernel build carries them. To confirm on the HP before that build: More options,
  "Trouble 1: early messages on the firmware screen" shows the x2APIC error lines. `CONFIG_VMD=y` was added the same day as a precaution for
  installs to disk; this HP does not need it.
* **Second round, 2026-11-07 evening:** kernel rebuilt with the options (`x2apic` interrupt remapping confirmed in QEMU with an emulated IOMMU:
  "DMAR-IR: Enabled IRQ remapping in x2apic mode"), PC image repacked, lean image `~/iyakki-work/images/pc-lean-1107g` (seed mode, no full RAM
  disk, loader default) boots in QEMU through the loader to the running system, and its boot files were written onto the T7 stick by hand
  (`~/iyakki-work/hp-patch/apply-image.sh`; the data partition untouched; the default entry got `iyakki.root=boot` so the root image next to
  the kernel is used instead of the data volume's older tree). Two gaps found and fixed on the way: `lean-image.sh` told the loader the wrong
  RAM-disk name in `--root sfs` mode (now per mode, with a build-time check), and a pack update of a GRUB-default image silently puts GRUB back
  as the stick's second stage, so the loader-default state of a test stick does not survive `iyakki dev update` from such an image.
* **Result, 2026-11-07 evening: the HP booted IYAKKI from the stick** (owner's report). Step 2 of the goal is done for the boot itself; the
  hardware matrix (machine E) has the first two verified cells. Next on this machine, in the owner's order: Wi-Fi and Bluetooth (Realtek
  RTL8852BE: the Realtek firmware card in Store -> Setup), then the update framework, then sound, display and touch, battery, GPU.
* Expect the first-run "create the administrator account" screen. Record: time to the screen, whether the keyboard, touchpad and (if any) touch screen work on it, resolution and scale, and anything printed on the serial or screen.
* Typical HP trouble to look for: Intel VMD or RAID mode hiding the NVMe disk (BIOS "SATA/NVMe mode" must be AHCI), Secure Boot refusing the loader, a black screen with a hybrid GPU (the display on the iGPU, the dGPU powered off), keyboard backlight and function keys, the touchpad needing the I2C-HID driver.

## Step 3: Wi-Fi and Bluetooth

**2026-11-07, checked in image pc-lean-1107i**: the pc image already carries what this HP needs: kernel modules `rtw89_8852be` (with
`rtw89_8852b`, `rtw89_8852b_common`, `rtw89_core`, `rtw89_pci`), firmware `rtw89/rtw8852b_fw.bin` and `rtw8852b_fw-1.bin`, `btusb` + `btrtl`
with `rtl_bt/rtl8852bu_fw.bin` and `rtl8852bu_config.bin`. So no firmware card should be needed for Wi-Fi or Bluetooth on this machine; the
step is to boot 1107i (or later) on the HP and verify the list below. If the card still asks for firmware, the kernel log names the file and
the Realtek firmware card in Store -> Setup offers it. The firmware helper names the chip (`RTL8852BE Wi-Fi`, `RTL8852BE Bluetooth`) since
this date.


* In **Store -> Setup** the firmware cards (the family that matches step 1, and "Hardware firmware" for the rest) offer the ways: download from the vendor, through a cable, from a USB stick or folder, or files sent over a cable. The person must accept the vendor licence themselves; do not accept it on their behalf, do not ship the firmware, do not host it.
* After install the driver needs a restart (the card says so). Then check: the interface appears, scan lists networks, join works, the hotspot works if the card supports AP mode, Bluetooth powers on and pairs a device.
* Evidence to keep: the kernel log lines of the driver, `ip link`, the firmware file names loaded.

## Step 3b: graphics (found 2026-11-07, image 1107i on the T7 stick, developer mode over the network)

The shell draws fine on the framebuffer (screenshots by `iyakki dev screenshot`), the browser's full engine stopped at once:
"the connection to the helper failed ... (ended by a signal) | MESA: warning: Could not get intel_device_info". Kernel log: i915 found
METEORLAKE 7d45, then `GuC firmware i915/mtl_guc_70.bin: fetch failed -ENOENT ... Failed to initialize GPU, declaring it wedged!`
at 0.4 s: on Meteor Lake GuC submission is required, i915 is built into the kernel and probes before the root is mounted, and the lean
RAM disk carried only the DMC files. The image's root has all five `i915/mtl_*` files; they were simply not reachable at probe time.
Three changes: (1) `lean-image.sh` puts the DMC, GuC, HuC and GSC files of Meteor Lake and DG2 into `initrd-lean` when the image has them
(dev images; a release image has none and the Intel graphics firmware card must one day also refresh the early RAM disk, open);
(2) the firmware detector understands the GuC loader's "fetch failed" wording so the card offers the right file; (3) the browser falls
back to software drawing in the same session when the helper dies right after a hardware start, and starts again (the owner's rule:
framebuffer/software when the card is not usable, the card when driver and firmware are there). Verify on the next image: the kernel
log without "wedged", `graphics: hardware Mesa Intel(R) Arc ...` in the engine log, pages rendering; and on the current image the
software restart with the page loading.

**Result the same evening (image 1107j by `iyakki dev update`, restart by the owner)**: DMC, GuC and HuC loaded, GuC submission enabled, no
"wedged"; the engine on "Mesa Intel(R) Graphics (MTL)", YouTube's watch page rendering and playing; the only firmware still missing is the
NVIDIA GSP (`nvidia/595.99.02/gsp_ga10x.bin`, the NVIDIA card in the Store, the person's click). "GT1: can't init GSC proxy due to missing
mei component" is logged: the media GT's GSC needs `mei_gsc`/`mei_pxp` (not in the kernel config); video decode on the card may want it, open.

## Step 4: the update framework (do this on the first good boot)

1. Turn on **Settings -> Developer mode** on the laptop; pair from the build PC (`iyakki dev pair`, a one-time code the person reads off the screen); the person grants the scopes (update, shell, files, debug, input) and may need to extend the default 4 hour timeout.
2. Build the next image, make a pack (`iyakki dev update-pack IMG -o DIR --version V --variant pc`), send it (`iyakki dev update DIR --force`), restart only with the owner's word (`iyakki dev reboot`). Check: the new version runs, the boot menu keeps "IYAKKI (previous version)", and booting the previous version works. Then try a pack that must be refused (wrong variant) and read the refusal text.
3. A change made live on the laptop (a hot patch, a config) must also land in the source and the next image; never leave a fix only on the device.

## Step 5: sound and microphone

* Run Settings -> Sound: output list, the speaker test (stereo), the mixer. On the Lenovo the hardware level was the cause of "low": check the codec levels in `/proc/asound/card*/codec#0` and `amixer`, and compare with the IYAKKI volume. `crates/iyakki-agent/src/micdefaults.rs` sets the outputs and the capture controls at every start; an HP codec may name its controls differently (Speaker, Headphone, Master, "Internal Mic Boost", digital mics on Intel SOF / AMD ACP paths). **Digital microphones on newer Intel (SOF) and AMD (ACP) laptops need a topology and firmware package; if no capture device appears, say so as the finding rather than guessing.**
* Check Settings -> Voice -> Test mic and a dictation (it ends by itself after a short pause).

## Step 6: display, touch, browser, battery

* Display scale and brightness keys, external monitor over HDMI/DisplayPort/USB-C, lid close and open, the touch screen and its gestures if present, the camera (`Camera` tile), the Browser on a normal page and on a video page (hardware graphics path; if the page freezes use Safe graphics mode and say so).
* Battery: percentage, charging state, estimated time; suspend and resume, if supported by the BIOS (S3 versus modern standby), and whether the screen and Wi-Fi come back.

## Step 7: local models (only if the owner wants it)

* Install k3s and Lemonade from Store -> Setup; the GPU step for the iGPU (Vulkan) or the NVIDIA step (the driver is downloaded from the vendor by the person's click). Lemonade runs on the GPU by default and can be set to the CPU (Settings -> Models, key `G`). Evidence: tokens per second from the GPU test on the Settings page, not the word "installed". A machine with an iGPU and an NVIDIA card is the multi-GPU plan (`docs/development/GPU-MULTI-PLAN.md`); do not start it here.

## How to reach and drive the laptop

* The owner's development PC builds and sends; the laptop is reached through developer mode (`iyakki dev --device NAME ...`: `screenshot`, `input` for clicks and keys, `shell` for a read-only-ish shell as an unprivileged user, `files`, `kube`, `forward`, `update`, `reboot`). Shell access is time-limited and must be approved on the laptop's screen by the owner each time.
* Things learnt the hard way: pressing the key `q` on the Home screen signs the person out; the dev shell runs as an unprivileged user (it cannot write sysfs or open sound devices), so changing the machine is done through IYAKKI's own tools or a new image; click targets in Store -> Setup are the cards, which are not reachable by the accessibility labels, use screenshots; the owner reads screens and photos, so say what to look at.
* Photos or screenshots the owner sends are evidence; keep the useful ones under the work folder, not in the repository.

## Rules that apply (from the owner; they are not optional)

1. **Honest reporting.** Say verified only for what you saw working on the laptop, and name what you did not test. A failing test or a skipped step is reported as such.
2. **Boot-test every image in QEMU before it goes to a device**; never reboot the laptop without the owner's word; always keep the previous version in the boot menu.
3. **Fixes made live must also land in the source and the next image.** Everything reachable by a click must also be reachable by the chat assistant as a tool with approvals.
4. **Build only what changed** (incremental builds, shared caches, memory caps: no more than about 9 GB for a build; artifacts go under the owner's work folder, never `/tmp`, which is wiped on reboot).
5. **No lawyer is available**: when a licence, patent or redistribution question is unknown, build the alternative that avoids it (the person downloads from the vendor by their own click; permissive-licence code; or do not ship it). Never claim something is legal or illegal.
6. **Prefer our own code to dependencies**, permissive licences only; third-party tools (ffmpeg, vendor drivers, firmware) are fetched from the vendor at the person's request and never shipped in the image.
7. **No personal names, home paths or secrets in the repository.** The Secure Boot signing key and any API key never appear in chat, logs or files in the repository.
8. **Kernel facts only through the shared modules** (`iyakki-hwinfo`, `iyakki-devctl`, `iyakki-storage`, `iyakki-netscan`): no new `/proc`, `/sys` or `/dev` parsing inside a tile; the gates `kernel_single_reader` and `dependency_ratchet` may only shrink.
9. **Track features in `FEATURES.md`** (a test validates it: every cited test must exist, the totals line must match the rows). Commit or push only when the owner asks.
10. **Do not promise to wait or report back** unless something will actually wake you; say what you will check and when, or ask the owner to tell you when it is done.

## What to deliver at the end

1. `docs/project/HARDWARE-MATRIX.md`: one block for this HP model (exact product name, BIOS version) with each capability and its state, date and evidence.
2. A list of **bugs and gaps found**, each with the evidence, the likely cause and whether the fix is in source (rows in `FEATURES.md`, tests that fail first and pass after the fix).
3. A short **test sheet** in `docs/project/` the owner can repeat on the next HP model, with the exact steps and what each result means.
4. The images and packs used, with their versions, under the owner's work folder, and the logs of the runs.

## Risks and unknowns to state up front

* The model, chips and BIOS are unknown until step 1; Wi-Fi/Bluetooth firmware and digital microphones are the most likely blockers.
* Secure Boot with the project's key on a corporate-style HP BIOS (a locked BIOS or an administrator password) may block the boot entirely: ask the owner before changing BIOS settings, and record the original values first.
* Suspend (modern standby) is the least predictable item on current laptops; it may be reported as unsupported rather than fixed in this pass.
