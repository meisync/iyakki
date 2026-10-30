# PC test sheet: Intel + NVIDIA and AMD + NVIDIA (2026-10-26)

Image under test: `target/buildroot-pc/images/iyakki-install-x86_64.img` (flash this one) and `iyakki-x86_64.iso` (live copy, optional).
Built 2026-10-26 by `repack.sh pc` (no Buildroot make, no engine rebuild) from the current uncommitted tree
(`/etc/iyakki-release`: VERSION 0.1.0-alpha.1, GIT_DESCRIBE v0.1.0-alpha.1-11-gd29c547-dirty, VARIANT pc-x86_64, KERNEL 6.12.112).

| File | Size (bytes) | SHA-256 |
|---|---|---|
| `iyakki-install-x86_64.img` | 538968064 (514 MiB) | `ce488a55ff14dfc671ed9bb09dba2a4546ff2c330776ec8a919ac0f00dc6c12a` |
| `iyakki-x86_64.iso` | 346222592 (330 MiB) | `9db350d35e959d58b21dde8bf1e99cbf7a5a660261fee097fe4bc9c7402948ab` |

Check before flashing: `sha256sum target/buildroot-pc/images/iyakki-install-x86_64.img`.

## 0. What you need

* A USB stick of 4 GB or more (everything on it is erased). 64 GB is fine. The install image makes a 512 MB boot volume and a data volume that fills the rest.
* PC with **at least 4 GB RAM, 8 GB or more recommended**: the whole system (1.26 GB unpacked, plus the web engine) lives in RAM. QEMU with 1 GB panics at boot (initramfs does not fit); with 4 GB it boots.
* UEFI, **Secure Boot OFF** (this image is not signed).
* Wired network for the first run (Quick install downloads k3s and others from the internet).
* A second computer on the same LAN with the `iyakki` program (`target/release/iyakki`) for the `iyakki dev` step.

## 1. Flash the stick (on your development computer; the project's own tool)

```
cargo build --release -p iyakki-usb              # incremental; the existing binary is from 2026-10-17 and the crate is unchanged
target/release/iyakki-usb --list                 # no root: shows which drives may be written (your system disk and NVMe system disks are refused)
sudo target/release/iyakki-usb --install target/buildroot-pc/images/iyakki-install-x86_64.img
#   arrow keys: choose the STICK, Enter, read the red warning, type the drive name (for example sdb), wait for "verified"
```

**WARNINGS.** Check the drive name against `lsblk` (size and model) before typing it. The tool lists USB drives only unless you add `--all`;
never use `--all` for this test. Wrong drive means lost data. Fallback: `sudo dd if=iyakki-install-x86_64.img of=/dev/sdX bs=4M conv=fsync status=progress`.

## 2. First boot

1. Firmware settings: UEFI, Secure Boot off. One-time boot menu (F12, F10, F8 or Esc), pick the stick (the UEFI entry).
2. GRUB menu entries (default starts after 3 s):
   * **IYAKKI** normal; **IYAKKI - hardware check**: run this FIRST; **IYAKKI - safe graphics** (simple framebuffer, no GPU drivers): use it if the screen is black or garbled;
   * **IYAKKI - show boot messages** (kernel text), **compatible USB mode (no UAS)** for sticks that stall, **IYAKKI - NVIDIA driver (nouveau off)** (needed for the vendor driver, section 5),
   * **IYAKKI - Install to a disk**, fast start (lean boot), remote diagnostics (SSH, keys only: needs an image built with your key, so not in this one), also in a browser (HTTPS 8443).
3. Pick **hardware check**. Press `S` to save the report (it survives only when the data partition exists; the stick has one) or photograph it. Over SSH/CLI: `iyakki hwreport`.
4. Reboot, pick **IYAKKI**. First start formats the data partition (seconds), then asks for the administrator account.

## 3. Test list (write PASS / FAIL / note for each, per machine)

| # | Test | How | Expected | Intel+NVIDIA | AMD+NVIDIA |
|---|---|---|---|---|---|
| 1 | Boot | stick boots from the UEFI menu; GRUB menu shows | menu, then the IYAKKI start screen, setup asks for an administrator | | |
| 2 | Display | Which port is the monitor on? Note it: NVIDIA card or motherboard (iGPU) | picture at the monitor's native resolution; hardware check lists the monitor name and resolution (see section 4 for the exact driver you get) | | |
| 3 | Keyboard, mouse | hardware check live tester; type in the setup form | `[keyboard]`, `[mouse]` listed; the 8 squares turn green | | |
| 4 | Sound | HDMI/DP audio through the monitor, and the 3.5 mm jack if any (Settings, Sound; a tone or the radio/music tile) | at least one working output; note which codec the hardware check lists. The mixer picks `plughw` | | |
| 5 | Wired network | plug Ethernet before boot | top bar says "Ethernet connected" and an address; note the NIC driver (r8169, igc, e1000e...) | | |
| 6 | Wi-Fi | Settings, Network, pick your network (only if the PC has Wi-Fi) | scan lists networks, connect works; note the chip (iwlwifi, rtw89, mt7921...). Wi-Fi 7 BE200 does not work (kernel 6.12 has no `iwlmld`) | | |
| 7 | Browser | BROWSER tile: open a simple page, then a modern site, then a YouTube video (press F for the full engine) | page draws; note the speed. On the NVIDIA screen expect software drawing (section 4); say if video stutters | | |
| 8 | k3s start | Store, Setup, Quick install, k3s (needs internet; first start takes a few minutes) | job ends OK; `kubectl get nodes` shows Ready (Installer runs shows the log) | | |
| 9 | GPU info | Store, Setup, GPU card, Open its page; also `dmesg | grep -i -E "nouveau|nvrm|firmware|gsp|i915|xe |amdgpu"` in the CLI tile | the page names every GPU, driver and render node; dmesg lines as in section 4. Send the text or a photo | | |
| 10 | `iyakki dev` over LAN | section 6 | pairing works, `iyakki dev diag` returns a bundle | | |
| 11 | Install to disk | section 7 (only to a disk you can erase) | boots from that disk, data survives a reboot | | |

## 4. What the owner gets for graphics today (no proprietary blobs shipped by us beyond linux-firmware)

Verified in this image (`rootfs.cpio.zst`, kernel `.config` of `target/buildroot-mac`, and QEMU boots, see the build report):

* Kernel 6.12.112 with `nouveau`, `amdgpu`, `i915` and `xe` as modules, and `simpledrm`, `efifb` (EFI framebuffer), `fbdev` emulation built in.
* **NVIDIA, default boot (nouveau).**
  * Turing, Ampere and Ada (GTX 16xx, RTX 20, 30, 40): nouveau with **GSP firmware 535.113.01** (`DRM_NOUVEAU_GSP_DEFAULT=y`). The firmware is **in this image**: the repository's licence audit (`docs/FIRMWARE-LICENCES.md`) judged NVIDIA's linux-firmware licence (`LICENCE.nvidia`: redistribution for OSI-licensed operating systems, unmodified, licence text included) to allow it, and the licence text is in the image under `/usr/share/doc/linux-firmware/`. It is NOT the NVIDIA user-space driver. If you disagree with that audit, say so and it comes out of `pc-firmware.keep`.
    Expect: native-resolution kernel mode setting on the NVIDIA card. If GSP fails to load: dmesg shows `Direct firmware load ... failed`, nouveau fails to bind, and you fall back to the EFI/simple framebuffer (picture, no acceleration; possibly the firmware's resolution only).
  * Pascal and older (GTX 10xx, 9xx): nouveau without GSP: it works for display, but slow clocks. Fallback is the same simple framebuffer.
  * Blackwell (RTX 50): kernel 6.12 nouveau has no support: you get the EFI framebuffer (simpledrm/efifb) only. The open modules (section 5) are the only accelerated route.
  * The browser engine (WPE) uses Mesa **built with llvmpipe, radeonsi, iris, virgl only; no nouveau gallium, no NVK Vulkan**, so web drawing on an NVIDIA-driven screen is **software (llvmpipe) on the CPU**. It works, it is not GPU accelerated.
* **Intel iGPU** (Intel CPU, monitor on the motherboard): `i915` (or `xe` on Lunar Lake/Battlemage), DMC/GuC/HuC firmware in the image; Mesa `iris` is built, so the browser can draw on it (gen8 and newer). Not run on real hardware.
* **AMD iGPU** (Ryzen with graphics, monitor on the motherboard): `amdgpu` with the whole AMD firmware set; Mesa `radeonsi` is built. A Ryzen without an iGPU (F models, most AM4 non-G parts) just has no iGPU: nothing to load. `amdgpu` is not needed for NVIDIA-only boxes; it stays unloaded when no AMD GPU exists.
* Hybrid note: if both an iGPU and the NVIDIA card are active, which one carries the monitor decides everything above. Put the monitor on the NVIDIA card first (the case the owner plans), then, if time allows, repeat on the motherboard port and report the difference.

## 5. NVIDIA vendor driver (optional, only after the basic test works): what is possible tonight

The image carries NVIDIA's **open kernel modules** 595.99.02 (MIT/GPL-2.0, in `/lib/modules/6.12.112/updates/`: nvidia, nvidia-uvm, nvidia-modeset, nvidia-drm, nvidia-peermem). They need NVIDIA's GSP firmware and user-space libraries, which are proprietary and are **downloaded by you from NVIDIA**, never shipped by us:

1. Reboot, pick **IYAKKI - NVIDIA driver (nouveau off)** (nouveau must not hold the card).
2. Store, Setup, Hardware, **NVIDIA GPU driver (CUDA)** (shown only with an NVIDIA GPU of Turing or newer found). Read the licence notice, press Install: 423 MB download from `download.nvidia.com`, SHA-256 pinned and checked, k3s is installed first if missing.
3. Expected end: `nvidia-smi` prints 595.99.02 from a pod; node reports `nvidia.com/gpu`.

**Status: `docs/GPU-K3S.md` section 7: none of this has ever run on a real NVIDIA card.** Treat it as an experiment; send the job log (Installer runs) whatever happens.

What is **not possible tonight**, with the reason:
* **CUDA / `nvidia.com/gpu` without the person's download**: CUDA user-space, `nvidia-smi` and the GSP firmware for the vendor modules are NVIDIA proprietary software; the project's rule is that they are downloaded at the person's click and never shipped. Without Install there is no CUDA.
* **Pre-Turing cards (GTX 10xx and older) for the vendor-driver route**: NVIDIA's open modules support Turing and newer only.
* **GPU in k3s through nouveau (Vulkan "stage 1")** on NVIDIA needs Mesa NVK inside the pod image (pulled from the internet) and nouveau with GSP working on the card; stage 1 was proven only on an AMD Radeon 780M, never on NVK.
* **Wi-Fi 7 BE200** (needs a newer kernel), **Blackwell with nouveau** (kernel 6.12), **hardware-accelerated browser drawing on the NVIDIA card** (Mesa has no NVIDIA driver in the image, an engine rebuild would be needed; deliberately not done: WEB_MEDIA stays off).
* **Secure Boot**: image is unsigned.

## 6. Diagnostics and developer mode

Developer mode on the PC (Settings, System, **Developer mode**, administrators only; off by default; auto-off after 4 h by default):
1. Press `e` to switch on. The device listens on TCP **7979** (TLS) on all addresses; the page shows its address. A firewall on your computer must not block outgoing 7979; both machines must be on the same LAN.
2. Press `p`: a one-time pairing code (10 characters, five minutes) and the certificate fingerprint appear.
3. On your computer:
   ```
   iyakki dev pair <PC-address>            # type the code; check the fingerprint equals the one on the PC's screen
   ```
4. In Settings, Developer mode, press `G` (grant all diagnostic scopes: debug, files, input, kube, shell; a shell still asks for approval each time on the PC, answer `a`).
5. Collect and send:
   ```
   iyakki dev whoami
   iyakki dev diag -o pc-diag.txt          # one bundle: version, uptime, memory, disks, network, sound, services, cluster, recent errors, audit tail (scope debug; no secrets, no settings values)
   iyakki dev kube get pods -A             # after k3s is installed (scope kube, read-only; Secrets never shown)
   iyakki dev state                        # what the screen shows
   ```
   Also send: the saved hardware report (`/var/lib/iyakki/hwreport.txt`, or `iyakki hwreport` output), the `dmesg | grep -i -E "nouveau|nvrm|firmware|gsp|i915|amdgpu"` lines, and the Installer runs log of any failed job.
6. Switch developer mode off again (`e`) when done. Details: `docs/DEVELOPER-MODE.md`.

If the PC never reaches a usable screen, photograph the "show boot messages" entry's last screen and the hardware check.

## 7. Installer to a disk (with the required warnings)

Two ways; both ERASE the target:
* From the booted stick: Settings, System, **Install to disk**, or in the CLI tile: `iyakki install --list` (disks, their tables, which may be used), then `iyakki install --dry-run --disk <name>` (prints the plan, writes nothing), then `iyakki install`.
* GRUB entry **IYAKKI - Install to a disk**.

Warnings, in order of importance:
1. **Never choose an NVMe or SATA disk that holds an operating system or data you care about.** Everything on the chosen disk is erased. Use a spare SSD, or a second stick, for this test. The program refuses the disk the running system uses.
2. It shows the plan (created / kept / ERASED); a plan that erases needs the tick "I understand that everything on this drive will be erased" and the red button; Cancel has the focus at the start.
3. The installer's real-computer path (GPT/UEFI, 4096-byte sectors) has only been run in QEMU (OVMF and SeaBIOS); no real PC has tried it.
4. After install: remove the stick, boot from that disk, Secure Boot off.

## 8. What was verified by the builder, and what was not (2026-10-26)

Verified: workspace builds (`cargo build --release --target x86_64-unknown-linux-musl -p iyakki-core -p iyakki-store-server`, 7 min 38 s); repack finished in 197 s; the initramfs boots under KVM with 4 GB (`qemu-test.sh` checks, with `-m 4096`: PASS); the install image boots under UEFI (OVMF) from an NVMe drive, mounts the data volume and shows the administrator setup screen; contents list as above.
Not verified: any real hardware; Secure Boot; the browser tile on this build through the `qemu-web-test` script (its tile choice is out of date: it landed on the MCP tile, screenshots in `/tmp/iyk-webtest/` are not a browser test); nouveau/GSP on a real NVIDIA card; Wi-Fi; k3s start; installer to disk.
