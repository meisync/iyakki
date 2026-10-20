# Wi-Fi and Bluetooth on a T2 Mac (Mac mini 2018 and others): setup guide

**For:** the person holding a T2 Mac (tested model: Mac mini 2018, `Macmini8,1`) that runs IYAKKI from a USB stick or disk and has no working Wi-Fi or Bluetooth. **You will get:** two ways to bring Apple's firmware onto the machine (a script route and a chat route), the checks that show it worked, and how to undo it.

Decided 2026-10-20 (owner): **IYAKKI ships no Apple or Broadcom firmware.** The files are Apple's; they are copied or downloaded to your own machine at your request and stay there. Background: [`MAC-HARDWARE.md`](MAC-HARDWARE.md) section 6 (Bluetooth) and section 9 (driver packs); the plan is [`BETA-PLAN.md`](BETA-PLAN.md) (priority 2).

## What is verified and what is not

| Statement | State |
|---|---|
| The file names the kernel asks for on the Mac mini 2018 (`brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0.bin`, `.txt`, `.clm_blob`) | Verified in the kernel log of the Mac mini 2018 |
| The Bluetooth controller (BCM4364 on a UART) now answers, `hci0` exists, and the kernel asks for `brcm/BCM.hcd`; it has no address yet | Verified in the kernel log of the Mac mini 2018 (newest check, as in `KNOWN-ISSUES.md`; the older silent-controller notes in `MAC-HARDWARE.md` section 6 predate it) |
| The script `dist/drivers/mac-wifi-firmware-from-macos.sh` produces those names | Tested on a copy of the firmware folder layout, **not** on the real macOS volume of a Mac |
| The Linux-side route (macOS recovery download, extraction, copy) | **Not verified end to end.** Written from the t2linux method and the existing script; commands may need small changes |
| Wi-Fi associates after the files are installed | **Not verified.** No Wi-Fi adapter of any kind has associated with a real access point yet |
| Bluetooth works after `BCM.hcd` is installed | **Not verified.** The controller answers but waits for the patch file and has no address yet; whether the file is enough is untested (see [`MAC-HARDWARE.md`](MAC-HARDWARE.md) section 6) |

Until you have done a step and seen its check pass, treat it as untested. Report what you see (the `dmesg` lines) so the table can be updated.

## Before you start

* You need a way online that is not the built-in Wi-Fi for route B (Ethernet or a USB Wi-Fi adapter), or a second computer for route A.
* The macOS volume of the Mac is **read-only** from IYAKKI. Never write to an internal disk from these steps.
* Know your model: `cat /sys/class/dmi/id/product_name` (for example `Macmini8,1`). The Wi-Fi and Bluetooth chip differs per model (BCM4364 or BCM4377); see the table in [`MAC-HARDWARE.md`](MAC-HARDWARE.md) section 6.
* Check the licence notice of the script before you run it: the files are Apple's, for use on the same Mac they came from.

## Route A. Script on macOS (the safe, short route)

Use it when the Mac still has macOS (a second partition, or before you wipe it).

1. Boot macOS on the Mac. Plug in a USB stick formatted FAT32 or exFAT (called `STICK` below). Copy `dist/drivers/mac-wifi-firmware-from-macos.sh` from the IYAKKI release or repository to the Desktop.
2. Open Terminal and run:

   ```bash
   bash ~/Desktop/mac-wifi-firmware-from-macos.sh /Volumes/STICK
   ```

3. Expected output: one line per Wi-Fi set, then `wrote 1 Wi-Fi firmware sets to /Volumes/STICK/iyakki-wifi-firmware`. For the Mac mini 2018 the first line should name `brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0`. If the script says `no BCM4364 UART Bluetooth patch file in macOS`, the chip starts from its ROM and there is no `BCM.hcd` to copy (see Bluetooth below).
4. Eject the stick, boot IYAKKI, plug the stick in, then follow [Install the files](#install-the-files-store-route-and-direct-route).

## Route B. From Linux (no macOS needed): macOS recovery image

This is the t2linux method (`https://github.com/t2linux/wiki`, file `docs/tools/firmware.sh`): the Mac's own model downloads Apple's macOS recovery image from Apple's servers, the firmware folder is read from it, and the same IYAKKI script turns it into the file names Linux asks for. It downloads Apple's files **to your own machine**; IYAKKI does not host or ship them. Do this on the Mac itself (booted into IYAKKI with Ethernet) or on any Linux PC and carry the result on a stick.

1. Get the downloader. `macrecovery` is a Python script from the OpenCore project (`https://github.com/acidanthera/OpenCorePkg`, folder `Utilities/macrecovery`). Download that folder (git clone or the ZIP) into a scratch folder. In an IYAKKI terminal use the Code tile or Claude Code; on a PC use any shell. Python 3 is required.
2. Find the board id of your Mac (the Mac mini 2018 is believed to be `Mac-7BA5B2DFE22DDD8C`; check it against the OpenCore list or the t2linux page, do not trust this line). Then download the recovery image into an empty folder:

   ```bash
   mkdir -p ~/macos-recovery && cd ~/macos-recovery
   python3 /path/to/macrecovery/macrecovery.py -b Mac-7BA5B2DFE22DDD8C -m 00000000000000000 download
   ```

   This produces `BaseSystem.dmg` (about 600 MB to 1 GB) and `BaseSystem.chunklist`. If the server refuses a board id, use the one printed by macrecovery's `-h` or the t2linux page; this step depends on Apple's servers and may change.
3. Read the firmware folder out of the image. Prefer a method that needs no root and writes nothing to a disk:

   ```bash
   mkdir -p fw-extract
   7z x BaseSystem.dmg -ofw-extract        # 7-Zip reads the HFS+/APFS inside; if it only gives a .hfs file, run 7z x on that too
   find fw-extract -type d -path '*usr/share/firmware*' | head
   ```

   You want the folder that contains `wifi/` (and, on newer macOS, `bluetooth/`): `.../usr/share/firmware`. If `7z` cannot open it, the t2linux script uses `dmg2img` and a read-only loop mount of the HFS+ image; use the same: `dmg2img BaseSystem.dmg out.img` then `sudo mount -o ro,loop out.img /mnt` (read-only; the image is a file in your scratch folder, not a disk).
4. Run the IYAKKI script against that folder (it accepts `IYAKKI_FW_SRC` for exactly this):

   ```bash
   IYAKKI_FW_SRC="$PWD/fw-extract/usr/share/firmware" \
     bash /path/to/dist/drivers/mac-wifi-firmware-from-macos.sh "$PWD/out"
   ls "$PWD/out/iyakki-wifi-firmware"
   ```

   Expected: files named `brcmfmac4364b2-pcie.apple,...-X0.bin`, `.txt`, `.clm_blob` (and `.txcap_blob` if present). If the recovery image is of a macOS that has no `wifi/` folder, the script says so; try another macOS version in step 2 (`-os` option of macrecovery, see its help).
5. Continue with [Install the files](#install-the-files-store-route-and-direct-route). Delete the scratch folder afterwards (about 1 GB).

## Install the files: Store route and direct route

**Store route (recommended, survives reboots).** Put the folder `iyakki-wifi-firmware` on a USB stick, plug it into the Mac running IYAKKI, open STORE, Drivers, the Wi-Fi entry (`mac-wifi-brcmfmac`), "Install from folder or USB", pick the folder, read the review (unsigned local pack: you must accept the warning), install. For Bluetooth use the Bluetooth entry (`mac-bt-broadcom`) the same way. The pack is stored in `/var/lib/iyakki/firmware/<pack>/` with a `pack.json`, linked into `/lib/firmware` at every boot, and the drivers reload.

**Direct route (a quick test, lost at reboot unless the Store route is also done).** Needs an administrator terminal. The Store pack layout is `/var/lib/iyakki/firmware/<pack>/files/brcm/<file>`; the kernel looks in `/lib/firmware/brcm/`, which IYAKKI fills with links at boot. For a one-off test:

```bash
PACK=mac-wifi-brcmfmac
sudo mkdir -p /var/lib/iyakki/firmware/$PACK/files/brcm /lib/firmware/brcm
sudo cp /path/to/iyakki-wifi-firmware/brcmfmac4364b2-pcie.apple,* /var/lib/iyakki/firmware/$PACK/files/brcm/
for f in /var/lib/iyakki/firmware/$PACK/files/brcm/*; do sudo ln -sf "$f" /lib/firmware/brcm/; done
sudo modprobe -r brcmfmac_wcc brcmfmac 2>/dev/null; sudo modprobe brcmfmac
dmesg | tail -30
```

Without a `pack.json` the boot step does not know about these files, so after a restart the links are gone: use the Store route to make it permanent.

## Check that it worked

```bash
dmesg | grep -iE 'brcmfmac|firmware'        # expect: "Firmware: BCM4364/3 wl0: ..." and no "Direct firmware load ... failed with error -2"
ls /sys/class/net                           # expect a wlan0 (or wlp...) next to eth0
iw dev                                      # expect an interface of type managed
ip link show wlan0                          # state DOWN is fine before you join a network
```

Then use Settings, Network to scan and join, or `iw dev wlan0 scan | grep SSID`.

For Bluetooth (needs `BCM.hcd`, only if your macOS had exactly one BCM4364 patch file):

```bash
dmesg | grep -iE 'bluetooth|bcm|hci'        # expect a "BCM ... build" line and no "Reset failed (-110)"
cat /sys/class/bluetooth/hci0/address       # expect a non-zero address; an empty or missing file means the controller did not answer
/usr/bin/btctl list                         # lists controllers that came up
```

## Route C. Let an AI agent do it with you: a prompt to paste

Give the text below to Claude Code or Copilot CLI (in the AI IDE tile) or to the IYAKKI assistant (in the CHAT tile). Each command still needs your approval; read it before you press yes. Replace the three `<...>` values first.

```text
You are helping me install Apple's Wi-Fi and Bluetooth firmware on my T2 Mac that runs IYAKKI from a USB stick. Work in small steps and ask me before every command that writes, downloads or uses sudo. Explain in one line what each command does.

GOAL: make the built-in Wi-Fi (Broadcom BCM4364) and, if possible, Bluetooth work. Model: <output of: cat /sys/class/dmi/id/product_name>. I have a USB stick with the folder iyakki-wifi-firmware at <path, or "none: download it">.

DO:
1. Read first: docs/MAC-WIFI-BLUETOOTH-SETUP.md and docs/MAC-HARDWARE.md (sections 6 and 9) if they are on this machine. Run: cat /sys/class/dmi/id/product_name; dmesg | grep -iE 'brcmfmac|bluetooth|firmware'; ls /sys/class/net; ls /lib/firmware/brcm | head.
2. Find the exact file names the kernel asks for in the dmesg lines ("Direct firmware load for brcm/... failed"). The expected Wi-Fi names are brcmfmac4364b2-pcie.apple,<board>-<module>-<vendor>-<version>-<antenna>.bin, .txt, .clm_blob. For Bluetooth the BCM4364 asks for brcm/BCM.hcd.
3. If the folder iyakki-wifi-firmware is on the stick: show me its file list and sizes. If I said "download it": explain the t2linux method (https://github.com/t2linux/wiki, tools/firmware.sh; macrecovery from OpenCore downloads Apple's macOS recovery image to this machine), ask my approval, download into a NEW empty scratch folder under my home directory, extract with 7z or dmg2img (read-only), then run dist/drivers/mac-wifi-firmware-from-macos.sh with IYAKKI_FW_SRC pointing at the extracted usr/share/firmware.
4. Install by copying the files to /var/lib/iyakki/firmware/mac-wifi-brcmfmac/files/brcm/ (and BCM.hcd to /var/lib/iyakki/firmware/mac-bt-broadcom/files/brcm/BCM.hcd), then tell me to finish in STORE > Drivers > "Install from folder or USB" so the pack survives a reboot. For a quick test only, link them into /lib/firmware/brcm and run modprobe -r brcmfmac; modprobe brcmfmac.
5. Verify and show me the evidence: dmesg lines with "brcmfmac" and the firmware version; ls /sys/class/net (a wlan0); iw dev; ip link; for Bluetooth: cat /sys/class/bluetooth/hci0/address and dmesg | grep -i bluetooth.

DO NOT:
- Never write to an internal disk, partition table or EFI/boot files. The macOS volume is read-only: mount nothing read-write. Use only the scratch folder, /var/lib/iyakki/firmware and /lib/firmware.
- Never print, copy or send tokens, passwords, Wi-Fi keys, API keys or the vault. Do not paste secrets into any tool or web page.
- Do not download anything except macrecovery and Apple's recovery image, and only after I approve the exact URL. Do not install packages without asking.
- Do not guess which .hcd file to use: if there is more than one BCM4364 .hcd, stop and ask me.
- Do not claim success without the verification output.

ROLLBACK (offer it if anything fails): remove the links (rm /lib/firmware/brcm/<the files>), remove /var/lib/iyakki/firmware/<pack>, or use STORE > Drivers > Remove; delete the scratch folder; restart. Nothing else was changed.

At the end, give me a short report: files installed (names only), the verifying lines, and what still does not work.
```

## Troubleshooting

| Symptom | Likely cause | What to do |
|---|---|---|
| `Direct firmware load for brcm/... failed with error -2` | file name differs from what the kernel asks for | Copy the exact name from the `dmesg` line; compare with the script's list; the script names follow the first (most specific) name brcmfmac tries |
| No `wlan0`, no firmware message | driver not loaded, or loaded before the files existed | `sudo modprobe -r brcmfmac; sudo modprobe brcmfmac`; check `lsmod | grep brcm` |
| `wlan0` exists, scan finds nothing | missing `.clm_blob` or `.txt` | Check the three files are all present; 2.4 GHz and 5 GHz regulatory data comes from the blob |
| Script says `no .../wifi - run this in macOS` | wrong source folder | In route B point `IYAKKI_FW_SRC` at the folder that has `wifi/` inside |
| 7z cannot open `BaseSystem.dmg` | APFS-only image | Use `dmg2img` and a read-only loop mount, or an older macOS in step 2 of route B |
| Bluetooth: `Direct firmware load for brcm/BCM.hcd failed`, or `command 0xfc18 tx timeout` / `Reset failed (-110)` | the patch file is missing, or (older images) the controller did not answer | Install `BCM.hcd` as above. If it still fails, the patch file may not be enough. Use a USB Bluetooth dongle or a USB keyboard/mouse (the documented route); report your `dmesg` |
| Files vanish after restart | direct route only | Install through STORE, Drivers so a `pack.json` is written |

## Rollback

Store route: STORE, Drivers, the pack, Remove, restart. Direct route: remove the links in `/lib/firmware/brcm/`, delete `/var/lib/iyakki/firmware/<pack>`, restart. Nothing in these steps changes the boot files, the internal disk or the data volume apart from that folder.

## Licence and legal notes

The firmware belongs to Apple and Broadcom. IYAKKI does not include it, host it or download it by itself; the scripts run on your machine at your request, on a machine you own, for that machine. This is not legal advice; the owner has a legal opinion on firmware redistribution as an open item in [`BETA-PLAN.md`](BETA-PLAN.md) section 8.
