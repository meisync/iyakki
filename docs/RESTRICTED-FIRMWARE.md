# Restricted firmware: not in the images, yours at a click

Some hardware needs a small firmware file from the vendor. IYAKKI's rule (owner decision, 2026-10-23, no lawyer involved):

> **When we are in doubt whether we may redistribute a file, it is not shipped in the images we publish. You can download and install it yourself, and you can build your own image that includes it.**

This page says which files, what happens without them, how to install them, how to build an image with them, and what you accept by doing so. The licence audit behind it is [`FIRMWARE-LICENCES.md`](FIRMWARE-LICENCES.md); the rule is item 7 of [`../LICENSING.md`](../LICENSING.md). Not legal advice.

## 1. What is not in the images

Four packs, 72 files in all, pinned to one commit of the upstream `linux-firmware` repository (release 20251011, commit `1a935bfb330d4a46786eaa903580db9e757b84fa`, taken from `git.kernel.org`). The list is `dist/buildroot/board/iyakki/restricted-firmware.table` (written by `dist/release/pin-firmware.sh` from the real `WHENCE` file; size and SHA-256 of every file).

| Pack (Setup tile) | Files | Why it is not shipped | Chips |
|---|---|---|---|
| **Qualcomm Bluetooth firmware** (`fw-qualcomm-bt`) | 20 files `qca/*` under `LICENSE.qcom` (about 1 MB) | The text says the Materials may only be used with Qualcomm chipsets and that you "shall not subject the Materials to any third party license terms (e.g. open source license terms)". An operating system image is made of open-source parts; we do not want to argue that it is not a subject. | WCN3950, WCN3988, QCA6390, WCN7850, QCA6698 Bluetooth (newer Qualcomm laptop combo cards) |
| **Realtek network firmware** (`fw-realtek-net`) | 30 files `rtl_nic/rtl81xx`, `rtl8402`, `rtl8411`, `rtl8125*`, `rtl8126*`, `rtl8127*` under `LICENSE.r8169` (about 140 KB) | Two lines from 2011-2013: "permission for the distribution of this firmware data in hexadecimal or equivalent format, provided this copyright notice is accompanying it". Every distribution ships it; we are not sure it covers a binary image of an operating system, so it is left out. | PCIe RTL8101/8105/8106/8107, RTL8168/8111 (d, e, f, g, h, fp), RTL8402, RTL8411, RTL8125 (a, b, bp, d), RTL8126, RTL8127 |
| **Broadcom Tigon3 network firmware** (`fw-broadcom-net`) | `tigon/*` (4), `bnx2/*` (5), `bnx2x/*` (12) under `LICENCE.tigon`, `LICENCE.bnx2`, `LICENCE.bnx2x` (about 3 MB) | The same two-line "hexadecimal or equivalent format" wording, on files "derived from proprietary unpublished source code". | tg3 (BCM5700 to BCM57xx, the BCM57766 of the Mac mini 2014), NetXtreme II (bnx2), NetXtreme II 10 Gigabit (bnx2x) |
| **Atheros AR9170 USB Wi-Fi firmware** (`fw-atheros-carl9170`) | `carl9170-1.fw` (13 KB), GPL-2.0 or later | A GPL binary needs its corresponding source offered with it. `linux-firmware` holds only the binary; the source is the `carl9170fw` project (named in `WHENCE`). We do not ship a GPL binary without a source offer. | Old AR9170 USB Wi-Fi sticks (the carl9170 driver) |

Still in the images, because the audit found their text clear (`FIRMWARE-LICENCES.md`): Broadcom `brcmfmac` for the Macs' Wi-Fi, Intel (i915, xe, iwlwifi, Bluetooth), AMD, NVIDIA's nouveau files (PC image), MediaTek, Cypress, Ralink, the older Atheros files, the Realtek USB adapter files (`rtl8153*`, `rtl8156*`: they come under the Realtek WLAN/USB text, not `LICENSE.r8169`), the Qualcomm Atheros files under `LICENSE.QualcommAtheros_ath10k` (QCA61x4, QCA9377, WCN399x, WCN6750, QCA2066 Bluetooth; the ath10k, ath11k and ath12k Wi-Fi files) and `e100`. The Mac Bluetooth `.hcd` and the FaceTime camera firmware were never in an image: they are Apple's and fetched at your click ([`MAC-CAMERA.md`](MAC-CAMERA.md)).

Checked: the Mac, macweb, PC and `hw` image recipes. The libre, aarch64, VM and cloud images carry no linux-firmware at all.

## 2. What happens without the file, chip by chip

From how the Linux drivers behave when the file is missing; **not tried on a device here without the files** (we have no such machine at hand: this is the honest limit of this page).

| Chip | Without the file |
|---|---|
| Realtek RTL8111/8168, RTL8101/8105/8106/8107, RTL8402, RTL8411, RTL8125/8126/8127 (PCIe, r8169) | The driver starts, the kernel logs "unable to load firmware patch", the port normally comes up. What is missing are Realtek's fixes for that chip: on some boards that shows as dropped links, a lower speed or higher power use. If your wired network is flaky, install the pack. |
| Realtek USB adapters (RTL8153, RTL8156, r8152) | Not affected: their files stay in the images. |
| Broadcom Tigon3 (tg3) | The port works with reduced function: the oldest chips (5700 to 5705) lose TCP segmentation offload, the BCM57766 loses Energy-Efficient Ethernet. The kernel logs "TSO capability disabled" or "EEE capability disabled". |
| Broadcom NetXtreme II (bnx2), NetXtreme II 10 Gigabit (bnx2x) | **The port does not work**: the driver cannot start without its file. These are server cards. You need the pack, or a USB Ethernet adapter, or the offline route below to get online. |
| Qualcomm WCN3950, WCN3988, QCA6390, WCN7850, QCA6698 Bluetooth | Bluetooth does not work (the controller cannot be set up without its patch file). The Wi-Fi of the same card is not affected. |
| Atheros AR9170 USB sticks (carl9170) | The stick does not work. AR9271 adapters (ath9k_htc) are not affected. |

A common PC with a Realtek or Broadcom NIC that cannot reach the network at all is the case to plan for: on that machine use the offline route (section 3b) with a stick made on another computer.

## 3. Install it yourself

### 3a. At a click (needs the network)

**Store, Setup, Hardware.** A tile for a pack appears **only when the matching hardware is on this computer** (PCI or USB ids in `fwpack.rs`; never in cloud mode). Administrators only (other roles see the tile but cannot press it).

1. **Read the licence and install.** A form shows the vendor and its licence summary (the card's Details show the same words), a field for a folder or stick (leave it empty to download) and the choice **I have read the licence and accept**. Choosing "I do not accept" does nothing: no file is downloaded or installed.
2. On accept, IYAKKI writes the acceptance (who, when, and the SHA-256 of the exact licence texts of this pin) next to the pack and a line `firmware.accept` in the audit log, then starts a detached job (`iyakki quick-install fw-<name>`; progress on the tile and beside the clock).
3. The job downloads each file and the vendor's licence texts from `git.kernel.org` at the pinned commit (https only, resumable), checks **size and SHA-256 of every file against the pinned table** (one wrong byte: the file is deleted and nothing is installed), and installs them as a **driver pack** on the data partition (`/var/lib/iyakki/firmware/fw-<name>`). The licence texts are kept next to the files (`licences/fw-<name>/`).
4. The pack is linked into `/lib/firmware` **at every start** from the data partition, so it survives restarts and in-place stick updates. The matching driver is loaded again (`modprobe -r` then `modprobe`: `btusb` and `hci_uart`, `r8169`, `tg3`, `bnx2`, `bnx2x`, `carl9170`).
5. The tile says plainly what happened. **Reboot to finish** appears only when the driver cannot pick the files up now (it is built into the kernel, did not load again, or the device has no driver bound yet). The button asks "Restart this machine now?" first and uses the normal restart path; nothing restarts without a click. The wish ends once the machine has started.

On the command line (over SSH, for a machine without a screen): `iyakki quick-install fw-realtek-net --accept-licence` prints the licence summary and records the acceptance for the user, then installs. `--from FOLDER` is the offline route, `--force-hardware` skips the hardware check, `--data DIR` names the data folder. Without an acceptance on record the job installs nothing.

Remove a pack in Store, Drivers (the pack id is `fw-<name>`); the acceptance stays.

### 3b. From a folder or USB stick (no network on the target)

On any computer with `curl` and `sha256sum` (a Linux or a Mac with the tools): in the IYAKKI source tree, `sh dist/buildroot/board/iyakki/restricted-firmware.sh fetch /path/to/stick` downloads **all** packs, or only one: `... fetch /path/to/stick realtek-net` (the other names: `qualcomm-bt`, `broadcom-net`, `atheros-carl9170`). Each file is checked against the pinned table on the way. Put the stick into the target and use the same tile: in the form, name the folder (the Browse button opens the file picker). The job reads the files from there (the repository layout `rtl_nic/...`, or flat file names), checks the **same table**, and installs the same way. A file that does not match refuses the whole stick; files that are not in the table are ignored. Licence acceptance is the same step.

## 4. Build an image that includes them (your own use)

```
IYAKKI_INCLUDE_RESTRICTED_FIRMWARE=1 ./run.sh build-pc-layered     # or build-mac, build-macweb, hw
```

Default **off**; the scripts and `run.sh` never set it. With it set, `post-build.sh` (via `dist/buildroot/board/iyakki/restricted-firmware.sh add`) fetches the files from the same pinned table (or from the folder `IYAKKI_RESTRICTED_FIRMWARE_DIR`, for an offline build; the table can be replaced with `IYAKKI_RESTRICTED_FIRMWARE_TABLE`), **verifies each size and SHA-256**, adds the files to `/lib/firmware` and the vendor licence texts to `/usr/share/doc/linux-firmware-restricted/<pack>/`, and stamps the image:

* `/etc/iyakki-release` gets `RESTRICTED_FIRMWARE=included`, so the About page of the System tile ends with "includes third-party firmware under its own licences: not for redistribution by the IYAKKI project";
* `/etc/os-release` gets `IYAKKI_RESTRICTED_FIRMWARE="..."` with the same words;
* `/usr/share/doc/linux-firmware-restricted/NOT-FOR-REDISTRIBUTION.txt` says it again.

An incremental build notices the switch (`changed.sh` hashes it), so `./run.sh build-...` with and without it rebuilds the root file system; the kernel and the packages are reused. The Buildroot selection for the `hw` image does not pick these files up either, so the switch is the only way they get into an image.

## 5. What you accept

* The files are third-party firmware under **the vendor's own licence**, not IYAKKI's Apache-2.0. The licence texts are kept next to the files.
* When you install them from a tile you accept the licence at that click, and the acceptance is in the audit log. When you build an image with the switch you accept it by setting the switch, and **the image is yours**: the IYAKKI project does not publish or distribute an image that includes these files, and you must not pass it on without your own check of the licences in the image (`NOT-FOR-REDISTRIBUTION.txt`).
* Qualcomm's text limits the files to Qualcomm chipsets and forbids placing them under third-party licence terms; Broadcom's and Realtek's texts ask that their copyright notice accompany the data (it does); the GPL file may be passed on only with its source (the carl9170fw project).
* IYAKKI fetches the files from the upstream repository and checks them; it does not host them.

## 6. What needs a device or the network to verify

Nothing here has been run on a machine with the hardware: the tests use fakes (a fake network that serves bytes, a fake `/sys` tree with ids, a fake world for the job). Still to check on devices: that a given NIC or Bluetooth chip is detected (the id tables in `crates/iyakki-agent/src/fwpack.rs` come from the kernel drivers' tables and may miss a board; `--force-hardware` and the folder route cover that), the exact degradation per chip in section 2, and that `modprobe -r` of the driver while in use behaves as the job expects (the job then says "Reboot to finish"). The download itself was exercised against `git.kernel.org` when the table was pinned (every file downloaded once, size and SHA-256 written down, all matching the files of the Buildroot tree that built the Mac image).
