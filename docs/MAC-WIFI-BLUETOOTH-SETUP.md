# Wi-Fi and Bluetooth on an Intel Mac (T2: Mac mini 2018 and others; no T2: MacBook Pro 2017): setup guide

**For:** the person holding a T2 Mac (tested model: Mac mini 2018, `Macmini8,1`) that runs IYAKKI from a USB stick or disk and has no working Wi-Fi or Bluetooth. **You will get:** the in-product route (IYAKKI menu, Mac drivers: one click downloads or reads Apple's firmware and installs it), the older routes (a script on macOS, a manual route from Linux, a prompt for an AI agent), the checks that show it worked, a numbered real-device test list, and how to undo it.

Decided 2026-10-20 (owner): **IYAKKI ships no Apple or Broadcom firmware.** The files are Apple's; they are copied or downloaded to your own machine at your request and stay there (never uploaded, not part of IYAKKI). The only Apple-related code IYAKKI does ship is the T2 sound driver `apple-bce` (t2linux, GPL-2.0, source in `legal-info`; `LICENSING.md`). Background: [`MAC-HARDWARE.md`](MAC-HARDWARE.md) section 6 (Bluetooth) and section 9 (driver packs); the plan is [`BETA-PLAN.md`](BETA-PLAN.md) (priority 2).

## What is verified and what is not

| Statement | State |
|---|---|
| The file names the kernel asks for on the Mac mini 2018 (`brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0.bin`, `.txt`, `.clm_blob`; shorter fallbacks `...-u-7.5.bin`, `-u.bin`, `apple,lanai.bin`, `brcmfmac4364b2-pcie.bin` are tried too) | Verified in the kernel log of the Mac mini 2018 (2026-10-20). Without the files brcmfmac ends with `Dongle setup failed` and a `__flush_work` warning; no `wlan0` |
| The Bluetooth controller (BCM4364 on a UART, `BCM2E7C`) answers (`BCM4364B0 Maui Olympic GEN`, chip id 123, build 0343), asks for `brcm/BCM.hcd` and reports `failed to write update baudrate (-16)`; `hci0` has no address | Verified in the kernel log of the Mac mini 2018 (2026-10-20) |
| The macOS firmware lives in `/usr/share/firmware/wifi/C-4364__s-B2/` (`P-lanai-...txt`, `lanai.trx`, `.clmb`, `.txcb`) and `/usr/share/firmware/bluetooth/` | The Wi-Fi layout and the Mac mini's platform name are verified; the **Bluetooth file names in macOS are not**: the shapes in the extractor come from the t2linux / Asahi naming rules and the fixtures in the tests are plausible, not copied from a Mac |
| The Rust extractor (`iyakki-drivers::appfw`) and the macOS script `dist/drivers/mac-wifi-firmware-from-macos.sh` give the same names and bytes for the same folder | Tested on fixtures (a test runs both when bash is there); **not** on a real macOS volume |
| Apple's recovery protocol client (`macrecovery.rs`: session cookie, `POST /InstallationPayload/RecoveryImage`, `AU`/`AT`, resume, chunk list check) | Tested against fake servers only (including a real TCP socket). **Never run against Apple's real service.** The wire details are from the OpenCore `macrecovery` tool |
| The board id of the Mac mini 2018 (`Mac-7BA5B2DFE22DDD8C`) and of the MacBookPro14,1 (`Mac-B4831CEBD52A0C4C`) | The first given by the owner, the second read from the MacBook's DMI; the other models' ids in `iyakki-drivers::macboard` are from the OpenCore lists and marked **unverified**. Any Intel Mac now uses its own DMI `board_name` |
| Apple's recovery service download for the Mac mini 2018 | **Verified on the Mac mini 2018 (2026-10-21):** the session, request and download worked; `BaseSystem.dmg` is 884,317,790 bytes (the chunk-list check is in the code and was part of that run) |
| 7-Zip reads the image | **Verified on the Mac mini:** `7zz l` shows Type = Dmg with one item `4.hfs` (2.9 GB); extracting **without naming it** opens the nested HFS and gives `macOS Base System/usr/share/firmware/{wifi,bluetooth}`; extracting by the name `4.hfs` gives 0 files. With the job's `-ir!` name patterns it unpacked 2738 files (59 MB). A full extract reports about 1000 sub-item errors (links and special files) and exits with code 2, which the job accepts when the firmware files exist |
| The recent image's `wifi/C-4364__s-B2/` | **Verified (names only):** most entries are symbolic links (`lanai.trx -> ekans.trx`, `lanai-{ID,X0,X2,X3}.{trx,clmb,txcb} -> lanai.*`, `P-lanai-X0_M-HRPN_V-u__m-7.5.txt -> P-lanai_M-HRPN_V-u__m-7.5.txt`); the NVRAM files themselves have **no antenna in their name** (`P-lanai_M-HRPN_V-u__m-7.5.txt`). The extractor follows links inside the tree, never out of it |
| Bluetooth patch file for the BCM4364 in the **recent** macOS image | **Not there (verified on the Mac mini):** `usr/share/firmware/bluetooth` has only BCM4377B3, 4378, 4387 and 4388 PCIe files. t2linux's own script skips the UART chips (it keeps only `.bin` and `.ptb`) and tells you to use a Monterey to Sonoma image for the PCIe ones, so there is no documented source for `BCM.hcd` |
| The job's second try for Bluetooth: the other macOS version Apple offers for the board (`os=default` against `os=latest`), scanning for any `BCM4364*.hcd` | **Verified on the Mac mini 2018 (2026-10-21): nothing there.** Both images (the latest and the other version Apple offered) contain **no** patch file for the BCM4364 UART Bluetooth; the job log says `no BCM4364 UART Bluetooth patch file in macOS`. There is no legal source for one and none is invented. Bluetooth stays without a firmware file (see "Bluetooth on the Mac mini 2018" below) |
| The 7-Zip download (GitHub release of `ip7z/7zip`, `7z<ver>-linux-x64.tar.xz`, the static `7zzs`) | The first device run **failed** here: `7z2605-linux-x64.tar.xz` uses a filter chain (BCJ x86 plus LZMA2) that IYAKKI's own `.xz` reader did not support. The reader now decodes the x86, ARM, ARM Thumb, ARM64, PowerPC, SPARC and delta filters (tested against `xz` itself); **not yet retried on the Mac** with the real file. The project publishes no checksum file, so the install is HTTPS only and shown as "not verified". If the install fails the job says where to put the official `7zz` by hand |
| The packs persist across boots and the drivers load by name | Tested with fakes (`apply_boot` after a simulated restart); not yet on the Mac |
| Wi-Fi associates after the files are installed | **Not verified.** No Wi-Fi adapter of any kind has associated with a real access point yet |
| Bluetooth works after `BCM.hcd` is installed | **Cannot be done on the Mac mini 2018:** macOS has no `BCM4364*.hcd` (verified above). Known limit F-NET-11: the controller runs in its ROM state with no valid address; use a USB Bluetooth adapter |

Until you have done a step and seen its check pass, treat it as untested. Report what you see (the `dmesg` lines) so the table can be updated.

## Bluetooth on the Mac mini 2018: measured, what works, what does not

Measured on the Mac mini 2018 (Macmini8,1, T2, ACPI `BCM2E7C`, UART `serial0-0`), 2026-10-21:

| Fact | Detail |
|---|---|
| The chip | BCM4364B0 ("Maui Olympic GEN"), chip id 123, build `001.000.122 0343`, on a UART, not USB or PCIe. The driver that binds it is `hci_uart` (with `btbcm`), not `hci_bcm4377` |
| macOS has no file for it | The recovery image for this board (latest) and the other version Apple offered contain **no** BCM4364 patch file (job log: `no BCM4364 UART Bluetooth patch file in macOS`). There is no legal source for one; IYAKKI does not invent or fetch one |
| With the driver loaded anyway | `hci_uart_bcm serial0-0: No reset resource, using default baud rate`, `Bluetooth: hci0: BCM: failed to write update baudrate (-16)`, `Failed to set baudrate`, `BCM: chip id 123`, `BCM4364B0 Maui Olympic GEN (MFG)`, `BCM (001.000.122) build 0343`, `BCM: firmware Patch file not found, tried: 'brcm/BCM.hcd'` |
| What exists afterwards | `/sys/class/bluetooth/hci0` exists; it has **no** `address` file (current kernels do not make one) |

**What works.** The controller registers (so it is not dead), and IYAKKI can read it through the kernel's Bluetooth management channel: Settings → Bluetooth (and the Devices popover) shows `Controller hci0: BCM4364 (ROM state, no firmware file)`, its address (all zero or `AA:AA:AA:AA:AA:AA` is shown as "no valid address"), and whether it is powered. **Power** and **Scan for devices (10 s)** are offered as the proof that the controller works; the scan lists name, address and signal. This is read, power and scan only; nothing pairs.

**What does not work (yet).** A valid address (the firmware file normally gives it one), and audio. Pairing and using classic keyboards, mice and game pads is written (see [`BLUETOOTH-INPUT.md`](BLUETOOTH-INPUT.md)) but needs a controller that works. Audio needs A2DP, which is not written; the page says so.

**Why the Bluetooth drivers wait.** On a T2 Mac without Apple's firmware IYAKKI sets `brcmfmac.ko`, `hci_uart.ko` and `hci_bcm4377.ko` aside at boot (renamed to `*.ko.iyakki-held`). They come back when a Bluetooth driver pack is installed (the whole Bluetooth group, `btbcm` first, then `hci_uart`, then `hci_bcm4377`, whatever files the pack has), or when you press **Load the Bluetooth driver anyway** (Mac drivers page, Devices popover, Settings → Bluetooth `l`): that gives the files back, loads them in that order, and remembers the choice for the next starts. A Wi-Fi-only install leaves the Bluetooth drivers set aside. Nothing is held on a machine that is not a T2 Mac.

**The USB adapter route (the reliable option).** This Mac's Bluetooth chip has no firmware file in macOS: the controller runs in its ROM state (limited); a USB Bluetooth adapter is the reliable option. A USB adapter with a chip the kernel supports (`btusb`: Intel, Realtek, Broadcom and MediaTek adapters) brings its own firmware path and an address of its own, and shows up in Settings → Bluetooth the same way. Pairing and audio then wait for the Bluetooth stack as above; a USB or 2.4 GHz wireless keyboard and mouse work today without Bluetooth at all.

**Not yet verified on the Mac** (the code is tested with fakes and byte fixtures): that the kernel lists this controller through the management channel (a controller without a valid address is "unconfigured" and appears in the extended index list only), that Set Powered is accepted without an address (the page says plainly if the kernel refuses), and that a scan finds devices.

## Macs without a T2 chip (MacBookPro14,1, 13-inch 2017)

Facts read from the real machine (2026-10-21, over SSH by the owner): `product_name` `MacBookPro14,1`, `board_name` `Mac-B4831CEBD52A0C4C`, `sys_vendor` `Apple Inc.`. Wi-Fi is a PCIe **BCM4350** (`brcmfmac: using brcm/brcmfmac4350c2-pcie for chip BCM4350/5`); the owner says Wi-Fi works with the generic linux-firmware `.bin`, but the kernel could not load `brcmfmac4350c2-pcie.Apple Inc.-MacBookPro14,1.bin` (the first, board-specific name), `brcmfmac4350c2-pcie.txt` (NVRAM) or the `.clm_blob`: so Wi-Fi may run with reduced performance and country/channel limits. Bluetooth is a **BCM4350C0 on a UART** (`hci_uart_bcm serial0-0`, `BCM4350C0 UART 37.4 MHz Gamay USI UHE`, `BCM (003.001.134) build 1532`); the kernel says `firmware Patch file not found, tried: 'brcm/BCM.hcd'` and `hci0` registers in its ROM state. `/lib/firmware/brcm` has only `BCM-0bb4-0306.hcd`.

What the product does for such a Mac (IYAKKI menu, Mac drivers, **Download from Apple**; still only on your click):

* **The board id** is the DMI `board_name` on every Intel Mac (`iyakki_drivers::macboard::resolve`). The model table is only a cross-check, a name source and the "verified" flag; a model that is not in it is allowed, and the page and the log say it is not on the verified list. The Mac drivers page and the log say **"This Mac has no T2 chip"** and what will be fetched: the Wi-Fi NVRAM and CLM files for the BCM4350 / BCM43602 chip and the Bluetooth patch file for the UART chip; Wi-Fi works without them, with limits.
* **Nothing is held back.** The hold-back of `brcmfmac`, `hci_uart` and `hci_bcm4377` (F-DRIVERS-22) is only for the T2 models; the MacBookPro14,1 keeps loading them as before. The job loads `brcmfmac` and `hci_uart` again at the end, so the network can drop for a moment.
* **Wi-Fi names.** `brcmfmac` builds the board part from the DMI as `<sys_vendor>-<product_name>` (`drivers/net/wireless/broadcom/brcm80211/brcmfmac/dmi.c`, Linux 6.12.112) and tries `<generic name>.<board>.<ext>` before the generic name for the `.bin`, `.txt`, `.clm_blob` and `.txcap_blob`. The Apple-only parts (module, vendor, version, antenna) exist only on Apple Silicon (device tree). So here the files are `brcmfmac4350c2-pcie.Apple Inc.-MacBookPro14,1.txt`, `.clm_blob` and `.txcap_blob` (BCM43602: `brcmfmac43602-pcie.Apple Inc.-<model>.*`). The `.bin` is **not** taken from macOS: linux-firmware's `brcmfmac4350c2-pcie.bin` stays. A name with a space is allowed in a driver pack for this reason (one space inside a name, none at its ends).
* **Which NVRAM file is this Mac's.** macOS keeps one NVRAM file per *platform* (`P-<platform>_M-<module>_V-<vendor>__m-<version>.txt` in `wifi/C-4350__s-C2/`), and the Mac's platform is not in the DMI. The extractor reads the `<platform>-PlatformConfig.plist` files under `wifi/` and takes a platform only when its plist contains this Mac's board id and **exactly one** platform of the chip folder does; then it takes that platform's NVRAM, `.clmb` and `.txcb` (`<platform>.clmb`, else the single distinct `<platform>-*.clmb`), and when several different NVRAM files remain (module/vendor variants with different contents) it takes none. If the board id is in no plist, or in several, **nothing is taken and the log says why**; nothing is guessed. Wi-Fi then keeps the generic firmware.
* **Bluetooth.** `btbcm_initialize` (`drivers/bluetooth/btbcm.c`) looks the controller's subversion up in its table; `0x6186` (003.001.134) is not in it, so `hw_name` is empty, and there is no board name (the device-tree source is Arm only) and no USB postfix: the only name tried is **`brcm/BCM.hcd`** (the table order is copied into a test: `<hw>.<board>`, `<hw>`, `BCM.<board>`, `BCM`, with `-vid-pid` for USB). The job reads the chip name from the kernel log (`hci0: BCM4350C0 UART ...`) and the names btbcm said it tried, takes the `.hcd` in macOS's `bluetooth` folder whose name starts with the chip name (`BCM4350C0`), only when exactly one distinct file exists (several: reported, none used; only other steppings such as `BCM4350C5`: named in the log, not used), and installs it as the **first name btbcm tried** (`BCM.hcd`). It also asks for `os=latest` after `os=default`, as for the Mac mini.

### Verified and unverified for the non-T2 case

| Statement | State |
|---|---|
| DMI `board_name` is the board id the recovery service wants, on this Mac | `Mac-B4831CEBD52A0C4C` read from the real machine (the id is now the verified entry for `MacBookPro14,1`); that Apple's service has an image for it is **not yet tried** |
| Names brcmfmac and btbcm ask for | Verified from the kernel source (6.12.112) and the real kernel log: `brcmfmac4350c2-pcie.Apple Inc.-MacBookPro14,1.bin` failed with -2, then `brcmfmac4350c2-pcie.txt`, `.clm_blob`; Bluetooth `brcm/BCM.hcd` |
| macOS recovery images contain `wifi/C-4350__s-C2/` (or `C-43602__s-*`) with NVRAM, `.clmb`, `.txcb` | **Verified: no.** Opened on the real MacBook (the image Apple sends for its own board has only `C-4355`, `C-4364`, `C-4377`, `C-4378`, `C-4387`, `C-4388`) and, on 2026-10-23 on a PC, the recovery images of macOS 10.12.6, 10.14.6, 10.15.7, 11.6, 11.5.2, one 12.x-or-later and 15.4.1: none has a `C-4350`/`C-43602` folder under `usr/share/firmware/wifi`. The 10.12.6 recovery system has only the host-MAC `AirPortBrcm4360` kext (8.8 MB Mach-O whose strings mention the 4350; no firmware image of the shape `brcmfmac` loads was found in it, which is a search, not a proof of absence), and the later recovery systems have no Broadcom Wi-Fi kext binaries at all (the code is in the kernel collection). **There is no Wi-Fi extra file for this Mac in any macOS recovery image.** Wi-Fi keeps the generic linux-firmware blob |
| The `<platform>-PlatformConfig.plist` files list board ids per platform | **Taken from the owner's description, not seen.** The rule (a literal board id in exactly one platform's plist) is the only mapping the code uses; if the real files differ, the log says it cannot tell and nothing is installed |
| macOS carries a `BCM4350C0*.hcd` | **No `.hcd`, but the patch is there as Intel HEX in macOS 11 (verified on real bytes, 2026-10-23):** `IOBluetooth.framework/Versions/A/Resources/BCM4350-Updater.hex` (181,600 bytes, sha256 `a12022a520c79bd84e35ef426e84567279f8eac78d6b4c7c648730aab3130aa2`, 2358 data records, 75,456 bytes of patch RAM at 0xFF004000) and `BCM4350-MiniDriver-uart.hex` (20,513 bytes, sha256 `e8a603335f39af16786b08ec64c0804b3e58c266ae0627301e64b8262a3fd1cd`, not used). Identical in macOS 11.6 (20G165) and 11.5.2. Not in 10.12.6, 10.14.6, 10.15.7, the 12.x-or-later image, 13.x (as seen on the Mac) or 15.4.1. See "Where the BCM4350 files really are" below |
| The unfiltered Wi-Fi works as before after the pack | Not verified on the Mac |
| Unpacking with the extra `-ir!*BCM4350*.hcd` name filter | Tested only as a command line |
| The converted patch makes the controller start and gives it an address | **Not verified.** `iyakki_drivers::ihex::to_hcd` turns the hex into `BCM.hcd` (77,598 bytes, sha256 `7836b3e63134fb8aa3ab559502ae6f44e6d10dae58ed61ad043a320e079c95fd`, 75,456 data bytes: the output was built from the real file and is well-formed, checked by `check_hcd`). Whether the BCM4350C0 of this Mac accepts a patch built this way is for the real Mac to say (commands below) |

### Where the BCM4350 files really are (research of 2026-10-23, on real Apple images)

Method: the recovery protocol of `macrecovery.rs` was run against `osrecovery.apple.com` from a PC, the images were read with 7-Zip (`7zz l` and a name-filtered `x`), and the files below were hashed. Apple chooses the macOS version from the **board id and the serial part** (`sn`) of the request, not from the Mac that downloads; the board id only says which family of Macs asks.

| Request (`bid`, `sn`, `os`) | Product | Size | macOS | What it has for the MacBookPro14,1 |
|---|---|---|---|---|
| `Mac-B4831CEBD52A0C4C` (this Mac's own), zeros, `default` | 042-01871 | 706,568,592 | 13.x (Ventura) | no `C-4350` Wi-Fi, no BCM4350 Bluetooth file (as seen on the Mac) |
| same, `latest` | 042-23155 | 710,918,897 | 13.x | the same (as seen on the Mac) |
| `Mac-BE0E8AC46FE800CC`, zeros, `default` | 694-35959 | 668,153,735 | **11.6 (20G165)** | **`BCM4350-Updater.hex`, `BCM4350-MiniDriver-uart.hex`** in `IOBluetooth.framework/Versions/A/Resources` |
| same, `latest` | 071-78714 | 668,455,349 | 11.5.2 | the same two files, identical bytes |
| `Mac-E43C1C25D4880AD6`, zeros, `default` | 694-54378 | 653,691,436 | 12.x or later (version not read) | no BCM4350 file |
| `Mac-00BE6ED71E35EB86`, zeros | 2Z694-25616 | 498,626,206 | 10.15.7 | no BCM4350 file anywhere in `IOBluetooth.framework` or `IOBluetoothFamily.kext` (searched the bytes) |
| `Mac-7BA5B2DFE22DDD8C`, `00000000000KXPG00`, `default` | 041-94410 | 480,292,242 | 10.14.6 | no BCM4350 file, no `AirPort*` kext in the recovery system |
| `Mac-77F17D7DA9285301`, `00000000000J0DX00`, `default` | 061-31101 | 482,075,478 | 10.12.6 (16G29) | `AirPortBrcm4360.kext` (host-MAC driver, no firmware blob for `brcmfmac`); no Bluetooth patch for the BCM4350 |
| `Mac-7BA5B2DFE22DDD8C`, zeros, `default` | 696-28424 | 884,317,790 | 15.4.1 | no BCM4350 file |

* **The board id `Mac-BE0E8AC46FE800CC`** is another Mac's board id; Apple's service serves the same generic recovery image to anyone who asks with it. It is used only because the newest macOS (13) that Apple serves for the 2017 MacBook Pro's own board id no longer has the file, and macOS 11 does. Nothing is sent but the board id, a zero serial part and random request ids (the same as for the Mac's own request).
* **Wi-Fi: nothing to fetch.** The BCM4350 Wi-Fi of this Mac has no NVRAM, CLM or txcap file in any recovery image; the job does not try to get one and says so (the generic `brcmfmac4350c2-pcie.bin` from linux-firmware stays). A file for it exists only in the full macOS installation (not offered by the recovery service) or in the Boot Camp Windows driver package, neither of which this job reads.
* **Bluetooth: the patch is a hex file, not a `.hcd`.** The job reads `BCM4350-Updater.hex` and writes `brcm/BCM.hcd` with the standard conversion (`iyakki_drivers::ihex`: one `Write_RAM` HCI command, 0xFC4C, per run of up to 248 data bytes, then `Launch_RAM`, 0xFC4E, at 0xFFFFFFFF; the same as the community's `hex2hcd` and what BrcmPatchRAM does at run time). `BCM4350-MiniDriver-uart.hex` is not used: `btbcm` already sends `Download_Minidriver` first. If a Mac's controller rejects the file, the log says the controller still has no address and the file can be removed with Delete download plus the driver pack removal on the Mac drivers page.
* **Mach-O embedded firmware** was looked for and is not needed: no BCM4350 Bluetooth or Wi-Fi firmware was found inside a kext binary of any of these images (the strings and sizes of `AirPortBrcm4360` in 10.12.6 are the host driver and its PHY tables, 8.8 MB; the later Base Systems have stripped kext binaries, the code lives in the kernel collection and has no BCM4350 Wi-Fi driver). No Mach-O extractor was written.
* **Other chips seen while looking:** macOS 11 also keeps `BCM4364B0-MiniDriver-uart.hex` with two updaters (`BCM4364B0ID1-Updater.hex`, `BCM4364B0ID2-Updater.hex`) for the Mac mini 2018's controller. Which of the two its hardware id wants is not known, so the job takes none (it never picks between different files); this is a lead for a later check on the Mac mini, not a fix.

**Which images the job opens.** For a Mac whose own newest macOS is known to lack the files (MacBookPro13,x and 14,x with a BCM4350 controller) only macOS 11 is fetched; for other Macs the version Apple sends for the Mac's own board, the other version Apple offers for it (skipped without a download when it is the same image), and macOS 11 for a BCM4350 controller, at most three images. Each image is kept in `<data>/mac-firmware/download/<board>-<os>-<serial>/` with a `verified` marker once it matched Apple's chunk list; **a retry never downloads a kept image again** (and does not even ask Apple for it), a partial image resumes, an image already read with nothing in it is skipped, and the images are deleted when the files are installed or when you press Delete download. The result of each image is in `<data>/mac-firmware/state.json`. The job log names the host of every request (`osrecovery.apple.com`, `oscdn.apple.com`; no tokens) and the byte counts.

**What the owner should check on the MacBook (after the new build is on the stick).**

```bash
ls -la /var/lib/iyakki/mac-firmware/download/*/ /var/lib/iyakki/mac-firmware/state.json
cat /var/lib/iyakki/mac-firmware/last.json
ls -la /lib/firmware/brcm/BCM.hcd /var/lib/iyakki/firmware/mac-bt-broadcom/files/brcm/
sha256sum /var/lib/iyakki/firmware/mac-bt-broadcom/files/brcm/BCM.hcd   # expected 7836b3e63134fb8aa3ab559502ae6f44e6d10dae58ed61ad043a320e079c95fd
dmesg | grep -i -E "hci0|BCM|hci_uart|Patch" | tail -20
cat /sys/class/bluetooth/hci0/address 2>/dev/null; ls /sys/class/bluetooth
```

Expected: the job log shows `downloading macOS 11 (Big Sur)`, `BCM.hcd was made from BCM4350-Updater.hex ... sha256 7836b3e6...`, then the Bluetooth drivers reload and `hci0` gets `BCM4350C0 ... build` lines without `Patch file not found`; Settings → Bluetooth no longer shows "Bluetooth needs firmware". A second press of Retry must show `using the image kept from an earlier try` (no download). If the log instead says the controller still has no address, report the `dmesg` lines: that is the one thing here that can only be decided on the Mac.

### MacBookPro16,2 (BCM4364 B3, board `trinidad`), checked 2026-10-23

The kernel log of this Mac names the chip and the file: `brcmfmac: using brcm/brcmfmac4364b3-pcie for chip BCM4364/4`, then `brcmfmac4364b3-pcie.apple,trinidad-HRPN-u-7.7-X0.bin`, `...-HRPN-u-7.7.bin`, `...-HRPN-u.bin`, `...-X0.bin`, `apple,trinidad.bin`. The recovery image Apple serves for the Mac's own board (product 696-28424, macOS 15.4.1) has `usr/share/firmware/wifi/C-4364__s-B3/` with `P-trinidad_M-HRPN_V-u__m-7.7.txt`, `trinidad.clmb`, `trinidad.txcb` and `trinidad.trx` (a link to `borneo.trx`, 820,013 bytes). The job writes `.bin`, `.txt`, `.clm_blob` and `.txcap_blob` for each antenna variant (`ID`, `X0`, `X2`, `X3`, with the 7.7 `u` and 7.9 `m` NVRAM versions); put on the Mac they made `wlan0` appear (firmware `9.30.514.0.32.5.94`) and scan. The image download itself was run end to end with the current code and verified against Apple's chunk list (the owner's earlier "chunk 1 of 85" did not repeat). Bluetooth: the chip is a BCM4364B3 on a UART that asks for `brcm/BCM.hcd`; macOS 11 keeps two patches for it (`BCM4364B3ID2-Updater.hex`, `BCM4364B3ID3-Updater.hex`) and which one it takes is open, so the job installs none yet.

## The in-product route (start here)

Open the **IYAKKI menu** (click the name IYAKKI in the top bar, or F10), **Mac drivers**. The page shows Wi-Fi, T2 audio, Bluetooth and the USB camera as the Devices page reads them, the notice **"These files are Apple's. They stay on this machine, are never uploaded and are not part of IYAKKI."**, the model of this Mac with the board id the download would ask for, and these buttons (administrators only; each click is your request):

| Button (key) | What it does |
|---|---|
| **Download from Apple** (`a`) | Asks Apple's recovery service for the recovery image of **this Mac's model** (1 to 2 GB, needs Ethernet or a USB Wi-Fi adapter and about 2.5 GB free on the data volume, never in RAM), checks it against Apple's chunk list, installs 7-Zip from the CLI list if it is missing, reads `usr/share/firmware` out of the image, names the files the way the kernel asks for them, installs them as the driver packs `mac-wifi-brcmfmac` and `mac-bt-broadcom` and loads the drivers again. |
| **From a folder or stick** (`f`, or Enter in the field) | The path you typed: the folder the macOS script wrote (`iyakki-wifi-firmware`), or a copy of macOS's `usr/share/firmware`. No download, no 7-Zip. |
| **From a disk image** | The path you typed: a macOS disk image (`.dmg`, or an image of a macOS volume) on a stick or in the data folder. Read with 7-Zip; the image is never changed or deleted. |
| **Copy the prompt for Claude/Copilot** (`c`) | Copies the prompt of route C below to the clipboard. |
| STORE → Drivers, Devices | The existing driver list and the Devices page. |

The same job is **Store → Setup → Hardware → "Mac Wi-Fi and Bluetooth firmware (from your Mac)"** (shown on a Mac only) (it uses the download source).

What you see while it runs: a detached job in the top bar's installer list (the indicator beside the clock), a live log with `[n/6]` steps, **Retry** after a failure (a downloaded image is kept and read again without any request; a partly downloaded one **resumes**), and a stop button (a stopped job leaves the partial download, nothing else). Temporary files are in `<data>/mac-firmware/` and are removed at the end; the last result is `<data>/mac-firmware/last.json`, shown on the page as "Last run".

When it ends the page says, in plain words, whether `wlan0` appeared and whether the Bluetooth controller has an address, and the log keeps the kernel lines about the drivers. If Bluetooth firmware was installed but the controller still has no address, the result says so: that is the known limit **F-NET-11**; a USB Bluetooth adapter is the fallback.

**Why this is not a mount of the macOS volume.** The kernel of the image has no APFS driver (APFS is not in mainline Linux) and T2 Macs use APFS; HFS+ is not what they have. So IYAKKI never mounts a macOS volume, never reads an internal disk as a whole and never writes any drive. Folders and image files you point to are read; that is all.

**Why Wi-Fi and Bluetooth do not load by themselves on a T2 Mac.** Without Apple's files brcmfmac, hci_uart and hci_bcm4377 only produce errors (`Direct firmware load ... failed with error -2`, a `__flush_work` kernel warning, `failed to write update baudrate (-16)`), and a Bluetooth chip started that way keeps no address. So on a T2 Mac with no such firmware visible, IYAKKI writes `/etc/modprobe.d/iyakki-t2-holdback.conf` (`blacklist brcmfmac`, `hci_uart`, `hci_bcm4377`) at every boot; the driver packs load them **by name** once their files are linked (a blacklist does not stop that). Other machines are never touched. This also stops the kernel noise.

**Update (device run 2026-10-21).** The blacklist alone did not hold on the Mac mini: BusyBox `modprobe` honours `blacklist` for aliases and, for literal names, only with `-b`, and something still loaded `brcmfmac` and `hci_uart` within 4 seconds. BusyBox has no `install` lines either. So the hold is now also done on the files: `brcmfmac.ko`, `hci_uart.ko` and `hci_bcm4377.ko` are renamed to `*.ko.iyakki-held` in the RAM root at every boot (nothing can load what is not there), and the driver pack that brings the firmware renames them back just before it loads them. Whether this silences the kernel log on the Mac is for the checklist below.

**Bluetooth for the 2018 mini.** The recent macOS recovery image has no patch file for its controller (see the table). The job therefore, for a BCM4364 Mac whose first image lacks `BCM.hcd`, asks Apple whether the other macOS version (`os=latest` after `os=default`) is a different image; if so it explains that this is another download of about 1 GB (10 to 30 minutes), **keeps the first image** (it is deleted only after a successful install or by Delete download), downloads the second and looks for any `BCM4364*.hcd` in it. If Apple sends the same image, or the other image has none either, the job says so plainly and Wi-Fi is still installed. Kept downloads and the unpacked folder are listed on the page with their size and a **Delete download** button; a successful run deletes both itself. **Retry** (labelled "Retry (no new download)" while an image is kept) reads the kept images and never downloads one twice.

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

## Route B. From Linux, by hand (what the in-product download automates)

This is the t2linux method (`https://github.com/t2linux/wiki`, file `docs/tools/firmware.sh`): the Mac's own model downloads Apple's macOS recovery image from Apple's servers, the firmware folder is read from it, and the same IYAKKI script turns it into the file names Linux asks for. It downloads Apple's files **to your own machine**; IYAKKI does not host or ship them. The in-product **Download from Apple** button does these steps without a terminal; use this route only if that fails or on a PC. Do this on the Mac itself (booted into IYAKKI with Ethernet) or on any Linux PC and carry the result on a stick.

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
/usr/bin/btctl list                         # expect hci0 with a non-zero address; the controller is missing from the list when it did not finish starting
ls /sys/class/bluetooth                     # hci0 exists even then (current kernels have no sysfs address file)
```

## Route C. Let an AI agent do it with you: a prompt to paste

Press **Copy the prompt for Claude/Copilot** on the Mac drivers page, or copy the text below, and give it to Claude Code or Copilot CLI (in the AI IDE tile) or to the IYAKKI assistant (in the CHAT tile). Each command still needs your approval; read it before you press yes. Replace the three `<...>` values first.

```text
You are helping me install Apple's Wi-Fi and Bluetooth firmware on my T2 Mac that runs IYAKKI from a USB stick. Work in small steps and ask me before every command that writes, downloads or uses sudo. Explain in one line what each command does.

GOAL: make the built-in Wi-Fi (Broadcom BCM4364) and, if possible, Bluetooth work. Model: <output of: cat /sys/class/dmi/id/product_name>. I have a USB stick with the folder iyakki-wifi-firmware at <path, or "none: download it">.

DO:
1. Read first: docs/MAC-WIFI-BLUETOOTH-SETUP.md and docs/MAC-HARDWARE.md (sections 6 and 9) if they are on this machine. Run: cat /sys/class/dmi/id/product_name; dmesg | grep -iE 'brcmfmac|bluetooth|firmware'; ls /sys/class/net; ls /lib/firmware/brcm | head.
2. Find the exact file names the kernel asks for in the dmesg lines ("Direct firmware load for brcm/... failed"). The expected Wi-Fi names are brcmfmac4364b2-pcie.apple,<board>-<module>-<vendor>-<version>-<antenna>.bin, .txt, .clm_blob. For Bluetooth the BCM4364 asks for brcm/BCM.hcd.
3. If the folder iyakki-wifi-firmware is on the stick: show me its file list and sizes. If I said "download it": first tell me about the built-in route (IYAKKI menu > Mac drivers > Download from Apple; it needs no terminal), and only if I prefer a terminal or it failed, explain the t2linux method (https://github.com/t2linux/wiki, tools/firmware.sh; macrecovery from OpenCore downloads Apple's macOS recovery image to this machine), ask my approval, download into a NEW empty scratch folder under my home directory, extract with 7z or dmg2img (read-only), then run dist/drivers/mac-wifi-firmware-from-macos.sh with IYAKKI_FW_SRC pointing at the extracted usr/share/firmware.
4. Install by copying the files to /var/lib/iyakki/firmware/mac-wifi-brcmfmac/files/brcm/ (and BCM.hcd to /var/lib/iyakki/firmware/mac-bt-broadcom/files/brcm/BCM.hcd), then tell me to finish in STORE > Drivers > "Install from folder or USB" so the pack survives a reboot. For a quick test only, link them into /lib/firmware/brcm and run modprobe -r brcmfmac; modprobe brcmfmac.
5. Verify and show me the evidence: dmesg lines with "brcmfmac" and the firmware version; ls /sys/class/net (a wlan0); iw dev; ip link; for Bluetooth: ls /sys/class/bluetooth; /usr/bin/btctl list (a controller listed with an address has started; a sysfs address file may not exist on this kernel); dmesg | grep -i bluetooth.

DO NOT:
- Never write to an internal disk, partition table or EFI/boot files. The macOS volume is read-only: mount nothing read-write. Use only the scratch folder, /var/lib/iyakki/firmware and /lib/firmware.
- Never print, copy or send tokens, passwords, Wi-Fi keys, API keys or the vault. Do not paste secrets into any tool or web page.
- Do not download anything except macrecovery and Apple's recovery image, and only after I approve the exact URL. Do not install packages without asking.
- Do not guess which .hcd file to use: if there is more than one BCM4364 .hcd, stop and ask me.
- Do not claim success without the verification output.

ROLLBACK (offer it if anything fails): remove the links (rm /lib/firmware/brcm/<the files>), remove /var/lib/iyakki/firmware/<pack>, or use STORE > Drivers > Remove; delete the scratch folder; restart. Nothing else was changed.

At the end, give me a short report: files installed (names only), the verifying lines, and what still does not work.
```

## Real-device test checklist (Mac mini 2018, never run yet)

Boot the Mac with the Mac image and the **remote diagnostics** entry, Ethernet plugged in, and go through this in order. Write down the first step that differs from "expected" and send the lines it shows; each step names the file or line to copy.

1. **Hold-back.** `cat /sys/class/dmi/id/product_name` (expected `Macmini8,1`); `cat /etc/modprobe.d/iyakki-t2-holdback.conf` (expected three `blacklist` lines: `brcmfmac`, `hci_uart`, `hci_bcm4377`); `lsmod | grep -E 'brcmfmac|hci_uart'` (expected: nothing); `dmesg | grep -ciE 'Direct firmware load|__flush_work|update baudrate'` (expected `0`).
2. **Devices page** (F8): the Wi-Fi and Bluetooth rows say `needs firmware: brcm/...`; Enter on either opens IYAKKI menu, Mac drivers.
3. **Mac drivers page**: the notice about Apple's files; the line `This Mac: Macmini8,1 (Mac mini (2018)), board id Mac-7BA5B2DFE22DDD8C.`
4. **Download from Apple.** Watch the installer list (beside the clock) and open the job's log. Expected steps: session, `product ...`, `the image is N bytes in M chunks`, `downloading n%`, `checking the image against Apple's chunk list`, `every chunk matches`. If it stops early, copy the failing line (session, the POST answer, or the host named in the address) and `cat /var/lib/iyakki/mac-firmware/last.json` (on a failure the download and the unpacked folder stay for a retry or a look; **Delete download** clears them).
5. **Interrupt and retry** (optional): unplug Ethernet during the download, plug it back, press Retry. Expected: `resumed after N MB already on disk`.
6. **7-Zip.** `ls -l /var/lib/iyakki/cli/bin/7zz` (installed by the job); `/var/lib/iyakki/cli/bin/7zz | head -2` prints a version. If the install failed, copy the line.
7. **Reading the image.** Expected log lines `found macOS's firmware folder at ...`, `N Wi-Fi firmware set(s), M Bluetooth file(s)`. If it says `no firmware found`, the dmg is kept: run `7zz l /var/lib/iyakki/mac-firmware/download/BaseSystem.dmg | grep -i 'usr/share/firmware' | head -20` and send the output (this tells whether the image carries the folder and under which path).
8. **Names.** `ls /var/lib/iyakki/firmware/mac-wifi-brcmfmac/files/brcm` must contain `brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0.bin`, `.txt`, `.clm_blob`; `ls /var/lib/iyakki/firmware/mac-bt-broadcom/files/brcm` may contain `BCM.hcd` (only if macOS has exactly one BCM4364 patch file); `ls -l /lib/firmware/brcm` shows links into those folders.
9. **Drivers.** `dmesg | grep -iE 'brcmfmac|bluetooth|BCM'` (expected `Firmware: BCM4364/3 wl0` and no `-2` errors); `ls /sys/class/net` (a `wlan0`); `iw dev`; for Bluetooth `ls /sys/class/bluetooth` and `/usr/bin/btctl list` (a controller listed with a non-zero address has started; **hci0 present but not listed is the known limit F-NET-11**; the older `cat /sys/class/bluetooth/hci0/address` check does not work on current kernels).
10. **The page says it plainly.** The Mac drivers page "Last run" line and `cat /var/lib/iyakki/mac-firmware/last.json` match steps 8 and 9.
11. **Restart.** Without pressing anything: `ls /sys/class/net` still shows `wlan0`, `cat /etc/modprobe.d/iyakki-t2-holdback.conf` is still there, `dmesg | grep -ciE 'Direct firmware load'` is `0`.
12. **First association.** Settings, Network: scan, join your access point, `ip addr show wlan0`. (No Wi-Fi adapter has ever associated in IYAKKI yet: say so either way.)
13. **From a stick.** Run `dist/drivers/mac-wifi-firmware-from-macos.sh` on the Mac's macOS if it is still there, put the folder `iyakki-wifi-firmware` on a stick, and use **From a folder or stick** with its path; the result must name the same files as step 8.
14. **Undo.** STORE, Drivers, remove both packs, restart: back to step 1's state, no errors in `dmesg`.
15. **Send back:** the job log, `last.json`, `dmesg | grep -iE 'brcmfmac|bluetooth|firmware'`, and anything that differed.

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
| The job says "this Mac model is not in IYAKKI's table" | the model has no board id in `macboard.rs` | Use the macOS script or a disk image |
| The job says Apple's service has no recovery image for board ... | the board id is wrong or Apple changed the service | Use the macOS script, or a disk image; report the line |
| "7-Zip could not read the disk image" | an image format 7-Zip does not read, or APFS inside it | Try another macOS version, or the macOS script |
| "no firmware found ... neither macOS's firmware folder" | the image does not carry `usr/share/firmware` | The macOS script on the Mac itself always can |
| "found N BCM4364 .hcd files - not guessing" | macOS has more than one patch file | Nothing is guessed: Bluetooth stays without firmware; report the names |
| Wi-Fi/Bluetooth never load on a T2 Mac, no errors in `dmesg` | the hold-back is working and no firmware is installed | Install it (Mac drivers page) |

## Rollback

Store route: STORE, Drivers, the pack, Remove, restart. Direct route: remove the links in `/lib/firmware/brcm/`, delete `/var/lib/iyakki/firmware/<pack>`, restart. Nothing in these steps changes the boot files, the internal disk or the data volume apart from that folder.

## Licence and legal notes

The firmware belongs to Apple and Broadcom. IYAKKI does not include it, host it or download it by itself; the scripts run on your machine at your request, on a machine you own, for that machine. This is not legal advice; the owner has a legal opinion on firmware redistribution as an open item in [`BETA-PLAN.md`](BETA-PLAN.md) section 8.
