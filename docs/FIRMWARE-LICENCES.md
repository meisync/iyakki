# Firmware and GPU driver licences: what ships, what is downloaded

Not legal advice. This is the owner's working audit of 2026-10-21; `LICENSING.md` items 7, 13 and 14 are the rules, this page gives the reasons. The line IYAKKI follows: **only the legal route**. Anything whose licence does not clearly allow redistribution is downloaded for the person at their request, never shipped.

## Policy since 2026-10-23: when in doubt it is not shipped; the user installs it

The owner has no lawyer, so the rule for doubt is mechanical: **a file whose licence we are not sure we may redistribute is not in the images we publish.** Instead (A) the person downloads and installs it at their click, after reading and accepting the vendor's licence (Store, Setup, Hardware tiles, or `iyakki quick-install fw-<name>`; from the upstream repository at a pinned commit, every file checked for size and SHA-256; or from a folder or stick), and (B) a person can build an image that includes it for their own use (`IYAKKI_INCLUDE_RESTRICTED_FIRMWARE=1`; the image says it is not for redistribution by the project). How, what degrades per chip, and what is accepted: [`RESTRICTED-FIRMWARE.md`](RESTRICTED-FIRMWARE.md).

Applied in this audit (72 files, four packs, table `dist/buildroot/board/iyakki/restricted-firmware.table`):

| Pack | Files | Reason |
|---|---|---|
| Qualcomm Bluetooth | the 20 `qca/*` files under `LICENSE.qcom` (WCN3950, WCN3988, QCA6390, WCN7850, QCA6698) | "shall not subject the Materials to any third party license terms (e.g. open source license terms)" |
| Realtek network | the 30 PCIe `rtl_nic/*` files under `LICENSE.r8169` | two-line "hexadecimal or equivalent format" wording |
| Broadcom network | `tigon/*`, `bnx2/*`, `bnx2x/*` | the same wording, files "derived from proprietary unpublished source code" |
| Atheros AR9170 | `carl9170-1.fw` | GPL-2.0 or later binary, no source offer in linux-firmware (see "Gap: carl9170" below) |

What stayed because the text is clear: Broadcom `brcmfmac` (Macs), Cypress, Intel, AMD, NVIDIA nouveau (PC), MediaTek, Ralink, the old Atheros files, the Realtek USB files (`rtl8153*`, `rtl8156*`, `LICENCE.rtlwifi_firmware.txt`), the Qualcomm Atheros files under `LICENSE.QualcommAtheros_ath10k` (QCA61x4, QCA9377, WCN399x, WCN6750, QCA2066 Bluetooth; ath10k/11k/12k Wi-Fi), `e100`. Where it is enforced: the Mac and PC keep lists (`!` lines and fewer patterns), `post-build.sh` (strips every file of the table from every variant, `restricted-firmware.sh strip`), the `hw` Buildroot selection (RTL_8169, TIGON3, BNX2, BNX2X removed), and tests (`crates/iyakki-core/tests/pc_image.rs`).

### Gap: carl9170

`carl9170-1.fw` is GPL-2.0-or-later and `legal-info/` does not contain the carl9170fw source, so shipping it needed a source pointer or a drop. It is **dropped from the images** (Mac and PC keep lists, and stripped from `hw`) and offered as the "Atheros AR9170 USB Wi-Fi firmware" pack. Someone who downloads it receives the binary from the upstream repository; the corresponding source is the carl9170fw project named in `WHENCE`. If the pack ever returns to an image, the written source offer must name that project (`carl9170fw/`, linuxwireless.org) and its commit.

## What the images carry

| Part | Licence | Shipped? | Condition we must meet | Where it is met |
|---|---|---|---|---|
| Linux kernel, nouveau, amdgpu, i915/xe, apple-bce, NVIDIA open modules (when built) | GPL-2.0 (NVIDIA modules: MIT/GPL-2.0) | yes | corresponding source offered | `legal-info/` from `make legal-info` |
| Mesa (RADV, ANV, NVK) | MIT | in pod images only, pulled by the cluster | notice | not shipped by us |
| AMD GPU firmware (`LICENSE.amdgpu`) | binary redistribution, no reverse engineering | PC and Mac images | reproduce the notice with the files; unmodified | `/usr/share/doc/linux-firmware/` (new) |
| Intel i915/xe/GuC/HuC/DMC, iwlwifi, Bluetooth firmware (`LICENSE.i915`, `LICENSE.xe`, `LICENCE.iwlwifi_firmware`, `LICENCE.ibt_firmware`) | binary redistribution without modification; Intel's name not used to endorse; limited patent licence only for use with an OSI-licensed OS | PC and Mac images | reproduce notice and disclaimer; do not modify | same folder |
| NVIDIA GSP/ACR/GR firmware from linux-firmware (`LICENCE.nvidia`) | redistribution allowed **only for operating systems under an OSI-approved licence, binaries unmodified, with a copy of the licence** | PC image only (not in the Mac image) | licence text travels with the files; files not modified (the image stores them uncompressed, as copied) | same folder |
| Broadcom, Realtek, MediaTek, Atheros, Marvell, Cypress, ... | each its own text (see `WHENCE`) | Mac and `hw` images; PC image drops Broadcom | per text: Broadcom: unmodified, for Broadcom hardware only | same folder |
| NVIDIA driver user-space, `nvidia-smi`, CUDA | NVIDIA driver licence | **never** | n/a | downloaded by the person's click (`docs/GPU-K3S.md`) |
| Apple Wi-Fi/Bluetooth firmware | Apple's | **never** | n/a | extracted/downloaded on the person's own machine (`LICENSING.md` item 13) |

## File-by-file audit of the Mac image (2026-10-23)

Method: the file list of the Mac image's `/lib/firmware` (385 files, 46 MB: `target/buildroot-mac/target/lib/firmware`) was matched against the real `WHENCE` of the linux-firmware tree that built it (release 20251011, `target/buildroot-mac/build/linux-firmware-20251011/`), and the named `LICENCE.*` / `LICENSE.*` files were read. Nothing was guessed from memory. Every shipped file has a `Licence:` line in `WHENCE`; **none is "Unknown", none forbids redistribution, none needs a click-through**. They are all *redistributable binary blobs, not open source*.

| Shipped files (Mac image) | Licence text (in the image under `/usr/share/doc/linux-firmware/`) | What it allows and requires | Risk for IYAKKI |
|---|---|---|---|
| `amdgpu/` (217 files, 30 MB), `radeon/` (45) | `LICENSE.amdgpu`, `LICENSE.radeon` | binary redistribution free of fees; no reverse engineering; reproduce the notice with the files; export-law notice (`radeon` has some files marked `MIT` in WHENCE) | low |
| `i915/` (43 files: skl, kbl, cml, icl DMC, GuC, HuC) | `LICENSE.i915` | binary redistribution **without modification**; Intel's name not used to endorse; no reverse engineering; the patent licence covers use alone or with an OSI-licensed operating system only | low if the files stay byte for byte; the image stores them as copied |
| `brcm/brcmfmac4350-pcie.bin`, `4350c2`, `4358`, `43602` (and `.ap`), `4371` | `LICENCE.broadcom_bcm43xx` | reproduce and distribute **complete, unmodified**, "**solely for use with Broadcom proprietary integrated circuit products** sold by Broadcom with which the Software was designed to be used"; ship the agreement with the files; keep the copyright notices; US export-law clause; terminates on breach | **the most restrictive text we ship, but met**: Macs carry exactly these Broadcom chips; the files are unmodified and the licence travels with them. Never repack, compress per file or rename them. A user installing the image on a PC without Broadcom Wi-Fi gets files they may not use (the PC image drops them) |
| `brcm/brcmfmac4356-pcie.*`, `brcmfmac43570-pcie.*` (links to `cypress/cyfmac4356-pcie.*`, `cyfmac43570-pcie.*`, 4 files) | `LICENCE.cypress` | binary redistribution "solely for use in connection with Cypress integrated circuit products"; no modification, no copying beyond that; terminates on breach | same as Broadcom: unmodified, licence shipped, chips are Cypress/Broadcom parts |
| `brcm/brcmfmac4356-pcie.gpd-win-pocket.txt` | `GPL-2` (WHENCE: the board NVRAM text files are GPL-2.0) | GPL; the file is its own source | none; useless on a Mac (a GPD handheld's board file), harmless |
| `mediatek/` (MT7921 Wi-Fi and Bluetooth, 4 files), `mt7662*`, `mt7601u` | `LICENCE.mediatek`, `LICENCE.ralink_a_mediatek_company_firmware` | use and redistribution for devices with MediaTek chipsets, **but not as part of the Linux kernel or in any form that would put the files under the GPL/LGPL**; Ralink text: binary, unmodified, notice reproduced, no reverse engineering, patent licence alone or with an OSI-licensed OS | low: they are separate files in the root file system, not linked into or built into the kernel. Do not build them into the kernel image (`CONFIG_EXTRA_FIRMWARE`) |
| `ar9170-*`, `ar9271.fw`, `ar7010*`, `htc_7010.fw`, `htc_9271.fw`, `ath9k_htc/htc_9271-1.4.0.fw`, `htc_7010-1.4.0.fw` | `LICENCE.atheros_firmware` (old blobs), `LICENCE.open-ath9k-htc-firmware` (1.4.0, free software with source) | binary, unmodified, notice reproduced, no reverse engineering, patent licence for Atheros chipsets | low |
| `carl9170-1.fw` | `GPL-2` (GPLv2 or later; source is the carl9170fw project) | GPL: source on request | **removed from the images (2026-10-23)**: linux-firmware holds only the binary, so `legal-info/` does not contain the source; the file is now the "Atheros AR9170 USB Wi-Fi firmware" pack the person installs at a click |
| `rt2870.bin`, `rt3070.bin`, `rt3071.bin` | `LICENCE.ralink-firmware.txt` | binary, unmodified, notice reproduced, no reverse engineering | low |
| `rtl_bt/rtl8761b*` (4 files), `rtl_nic/` USB (r8152: rtl8153/8156) | `LICENCE.rtlwifi_firmware.txt` | binary, unmodified, notice reproduced, no reverse engineering | low |
| `rtl_nic/` PCIe (r8169, 23 files in the Mac set) | `LICENSE.r8169` | "permission for the distribution of this firmware **data in hexadecimal or equivalent format**, provided this copyright notice is accompanying it" | **removed from the images (2026-10-23)**: the wording is old and we are not sure it covers a binary image; the "Realtek network firmware" pack. The USB files (`rtl8153*`, `rtl8156*`) stay |
| `tigon/` (tg3, 4 files) | `LICENCE.tigon` | the same "hexadecimal or equivalent format" wording, derived from Broadcom's proprietary source | **removed from the images (2026-10-23)**: the "Broadcom Tigon3 network firmware" pack (tg3 works without it at reduced function) |
| `regulatory.db`, `.p7s` | `LICENSE.wireless-regdb` (ISC; from the `wireless-regdb` package, not from linux-firmware) | ISC: keep the copyright notice in copies | none now: the licence text is added to the image by `firmware-licences.sh` (it was missing before 2026-10-23) |

Nothing from Apple is in the image: the build tree has no `facetimehd` firmware directory and no `.hcd`, no `*apple*` file in `/lib/firmware` (checked on `target/buildroot-mac/target`), and `package/facetimehd` installs only the kernel module (`facetimehd.ko`). A `facetimehd` folder on a running device is made at run time (the Camera firmware job on the person's click, `docs/MAC-CAMERA.md`); if it holds `firmware.bin` it came from the person's own download.

### PC image (`pc-firmware.keep`) in the same audit

Matched the same way against `WHENCE`: AMD (`LICENSE.amdgpu`, `LICENSE.radeon`), Intel (`LICENSE.i915`, `LICENSE.xe`, `LICENCE.iwlwifi_firmware`, `LICENCE.ibt_firmware`, `LICENCE.e100`: BSD-style), Broadcom Ethernet (`LICENCE.tigon`, `LICENCE.bnx2`, `LICENCE.bnx2x`: hex-format wording; **removed from the images 2026-10-23**, a pack), Realtek, MediaTek, Ralink, Atheros (`LICENSE.QualcommAtheros_ath10k`, `LICENSE.QualcommAtheros_ar3k`: binary, unmodified, **solely with a Qualcomm Atheros chipset**), NVIDIA nouveau files (`LICENCE.nvidia`: OSI-approved operating systems only, unmodified, copy of the licence with the files, **only with NVIDIA-designed processors**). No `Unknown` licence is kept. Two groups were restrictive enough to act on:

| Group | Text | Action |
|---|---|---|
| `amdgpu/isp_4_1_1.bin` | `LICENSE.amdisp`: revocable, redistribution "for use with Licensed Products" = AMD processors with GFX 11.5.1 graphics only | **removed from the PC image** (`!amdgpu/isp_*` in `pc-firmware.keep`). Nothing breaks: our kernel has no ISP camera driver for it |
| `ath10k/WCN3990/hw1.0/qcm2290`, `qrb4210` | `LICENSE.qcom` (Qualcomm Technologies): Qualcomm chipsets only, **"you shall not subject the Materials to any third party license terms (e.g. open source license terms)"**, the agreement text must accompany the files | **removed from the PC image** (`!ath10k/WCN3990/*`): phone SoC parts, no PC has that chip |
| `qca/*` files under `LICENSE.qcom` (20 Bluetooth patch files for newer Qualcomm laptop chips) | same text as above | **removed from the PC image (2026-10-23)**, owner decision: no lawyer, so no risk. The "Qualcomm Bluetooth firmware" pack installs them at a click; those chips' Bluetooth does not work until then. (The `qca/` files under `LICENSE.QualcommAtheros_ath10k` stay.) |

A new `!pattern` line in a keep list drops files even when another pattern keeps them (`prune-firmware.sh`; tested in `crates/iyakki-core/tests/pc_image.rs`).

### Answer for the owner (plain)

* **Wi-Fi works "without firmware"?** No: it uses a firmware file, the generic Broadcom `brcmfmac4350c2-pcie.bin` (or `43602`) from linux-firmware, which is in the image. Its licence allows us to ship it, unmodified, for Broadcom hardware, with the licence text (it is in `/usr/share/doc/linux-firmware/LICENCE.broadcom_bcm43xx`). Apple's own board files (NVRAM `.txt`, `.clm_blob`) are not in linux-firmware; the Mac drivers page fetches them from Apple at the person's click.
* **Sound works "without firmware"?** Yes, truly: the CS8409 codec is driven by a kernel module (`snd-hda-codec-cs8409`, GPL-2.0), which loads no firmware file.
* **Bluetooth lists but has no firmware:** the controller (`hci0`) exists but needs Broadcom's patch RAM file (`brcm/BCM.hcd`), which is **not** in linux-firmware and which Broadcom/Apple do not allow us to ship. It is never shipped; the person fetches it from Apple's macOS download on their own Mac. The shell now says "Bluetooth needs firmware" instead of listing a device (below).
* **Licensing issues?** None found that stop publishing the Mac image, provided the images are rebuilt with the licence texts (`firmware-licences.sh`) and the Broadcom, Cypress and Intel files stay unmodified. The open points are in "Still open for a lawyer".

## Gap found and fixed (2026-10-21)

The licence texts were only in Buildroot's `legal-info/`, **not inside the images**, although the NVIDIA, AMD and Intel licences require the text or notice to accompany the files. `board/iyakki/firmware-licences.sh` (called from `post-build.sh`) now copies every `LICENCE.*`/`LICENSE.*` file and `WHENCE` (about 1 MB, 125 files; since 2026-10-23 also `GPL-2`, `GPL-3`, the Qualcomm `NOTICE.txt` files that `WHENCE` names next to `LICENSE.qcom` and the Atheros licences, and wireless-regdb's ISC `LICENSE` as `LICENSE.wireless-regdb`: 130 files) to `/usr/share/doc/linux-firmware/` in every image that carries firmware, and fails the build if it cannot find them. Images written before this change (the alpha.1 images and the first alpha.2 Mac and PC images) do not have them: rebuild before publishing any of them.

## Still open for a lawyer

There is no lawyer (owner, 2026-10-23): a doubt about a file is settled by the policy at the top (not shipped; a pack the person installs). What remains below are questions about files that **are** shipped and whose text we read as clear; if any is later doubted, it moves to a pack the same way (`pin-firmware.sh` and `docs/RESTRICTED-FIRMWARE.md`).

1. The wording of the download notice for NVIDIA's `.run` file and whether showing it before the click is enough.
2. `LICENCE.nvidia` ties the firmware to "operating systems distributed under an OSI-approved open source licence". The kernel and userland qualify; the combined image also contains Apache-2.0 code (OSI-approved too). Confirm for a distribution that also offers a store of third-party apps.
3. Broadcom's text limits the Wi-Fi firmware to Broadcom hardware and forbids modification: fine for the Mac images as copied; never repack or compress it differently without re-reading it.
4. Distributing the whole PC image counts as a firmware compilation: publish `WHENCE` and the source/hash links (done through `legal-info/`), and keep the "firmware is not open source" label next to the download.
5. Patent and export notes of the vendors (Intel's patent licence is limited to combinations with an OSI-licensed OS).
