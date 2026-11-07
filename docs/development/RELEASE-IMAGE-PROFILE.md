# Release image profile

Owner decision, 2026-10-27: the base ISO and install image keep **only Apache-2.0, permissive or otherwise clearly non-violating
items**. Everything doubtful or proprietary (vendor firmware, codecs, anything the licence gate flags) is a click-to-install download,
never shipped. This page defines the **release** profile that does that, how to build it, and what is and is not verified.
Status: defined and checked without a build; **no release image has been built or booted yet**.

## 1. Base image or download

| Item | Licence | In the base image? | Where the person gets it |
|---|---|---|---|
| IYAKKI programs, samples | Apache-2.0, CC0 | yes | in the image |
| Linux kernel, BusyBox, GRUB, GStreamer, Mesa, WPE WebKit, git ... | GPL/LGPL/MIT/BSD (Buildroot `legal-info`) | yes, with `legal-info/` | in the image |
| Out-of-tree drivers apple-bce, facetimehd, snd-hda-macbookpro, NVIDIA open modules | GPL-2.0 / MIT or GPL-2.0 (kernel code) | yes | in the image |
| `regulatory.db` (wireless-regdb) | ISC | yes | in the image |
| linux-firmware (GPU, Wi-Fi, Bluetooth, Ethernet blobs; 355 to 1,421 files) | vendor licences, "Proprietary" for the gate | **no** | Store -> Setup -> Hardware firmware (`iyakki quick-install fw-hardware`) |
| NVIDIA GSP / booter firmware (438 files) | NVIDIA | **no** | same item (`--with nvidia`) |
| Apple Wi-Fi/Bluetooth/camera firmware | Apple | **never** | the person's own Mac (Mac drivers page, Camera firmware job) |
| H.264 / AAC / MP3 (FFmpeg libraries, x264, FAAD, fdk) | LGPL/GPL + patents | **no** | the audio and video add-on (Store -> Setup) |
| `gst-libav` plugin glue | LGPL-2.1+, no codec code | **parked, inactive** in `/usr/lib/iyakki-av` (reviewed `except` row; `IYAKKI_RELEASE_AV_BRIDGE=0` drops it) | activated by the add-on download |
| ffmpeg program, Claude Code, other CLIs | vendor / GPL | **no** | Quick install |
| NVIDIA user-space driver, CUDA | NVIDIA | **no** | Quick install "NVIDIA GPU driver" |
| GRUB menu and terminal fonts | DejaVu (Bitstream Vera) | yes | made from the repository's DejaVu files |

## 2. What a release ISO contains

The macweb/PC recipe (web engine, git, Mac and PC kernel with all drivers as modules) minus everything above: `/lib/firmware` holds
only `regulatory.db` and `regulatory.db.p7s`; `/usr/lib/iyakki-av` holds the inactive bridge; `/usr/libexec/iyakki/gpu-fw-guard`
and `/etc/modprobe.d/iyakki-release.conf` hold the GPU drivers back until firmware exists; `/etc/iyakki-release` carries
`IMAGE_PROFILE=release`, `FIRMWARE=none`, `CODECS=no-h264-aac`, `AV_BRIDGE=parked`; `/usr/share/iyakki/firmware-notice*.txt` are the first-boot texts.
It is written to `target/buildroot-VARIANT-release` (the development images are not touched), so the licence gate scans it as its own image,
and for a folder ending `-release` it also fails on any firmware file beyond the allowed ones.

## 3. What the person gets without firmware

| Hardware | Without the firmware files | After "Hardware firmware" |
|---|---|---|
| **Intel GPU** | the firmware framebuffer (UEFI GOP via simpledrm, the mode the firmware chose) and Mesa software rendering; the web image has no Intel GL driver anyway unless built `WEB_IRIS=1`. No mode changes, no second screen, no brightness through the GPU. Older generations (to about Gen 11) would run i915 without firmware, but newer ones need GuC and can fail after taking the screen over, so the guard holds all of i915/xe back | i915/xe loads at the next start (DMC, GuC, HuC) |
| **AMD GPU** | amdgpu and radeon cannot start without their firmware. Guard: not loaded, firmware framebuffer, software rendering | amdgpu/radeon at the next start |
| **NVIDIA GPU** | nouveau needs signed firmware on Maxwell and newer, GSP on Turing and newer. Guard: not loaded, firmware framebuffer, software rendering. The NVIDIA user-space driver is a separate download | nouveau (GSP) or the NVIDIA open modules plus the vendor driver |
| **Wi-Fi** | none for chips that need firmware (Intel iwlwifi, Realtek rtw88/rtw89, MediaTek, Qualcomm ath10k/11k/12k, Broadcom). Chips with on-chip firmware (some Atheros ath9k PCIe) can work if the kernel driver is built in: not tested | the card's own files |
| **Ethernet** | common chips work (e1000e, igb, r8169, USB adapters). Broadcom NetXtreme II and some server cards need files | rest |
| **Cable to another computer** | USB-C/USB network link, Thunderbolt and USB sticks work without firmware: they are how Wi-Fi-only computers fetch the firmware (`fwextra_net.rs`: Ethernet, shared connection over a cable, prepared stick, developer-mode push) | - |
| **Bluetooth** | none for Intel, Realtek, Broadcom, MediaTek controllers; generic USB dongles that need no firmware work | controller files |
| **Sound** | HDA, USB audio, apple-bce (T2) and CS8409 drivers need no firmware; Intel SOF laptops are not supported in this kernel in any profile | - |
| **Media in the browser** | VP8/VP9, AV1, Opus, Vorbis; no H.264/AAC until the add-on is installed | add-on |
| **Keyboard, mouse, storage, boot** | work | - |

Not verified on any hardware. The guard is a design choice made because an amdgpu/i915/nouveau probe that fails for missing firmware has
usually already replaced the firmware framebuffer, which can leave a black screen; the kernel command line `iyakki.gpu_fw_guard=0` loads
the drivers anyway, and the existing boot entry "safe graphics" still works.

## 4. First boot

The image writes `FIRMWARE=none` into `/etc/iyakki-release` and ships `/usr/share/iyakki/firmware-notice.txt`:
"Optional hardware firmware is not included (licence). Install it: Store -> Setup -> Hardware firmware." (the item is in Store -> Setup in
the code today; the owner's wording said Settings). The longer text is `firmware-notice.long.txt`. **The shell does not show it yet**: the
screen, the button that opens the item and the one-time dismissal are the shell owner's change (read the two keys/texts above). Until then
the image is correct but silent.

## 5. How it is built (not run)

Switch `IYAKKI_IMAGE_PROFILE=release` (not `IYAKKI_PROFILE`, which is the cargo profile):

| File | Effect |
|---|---|
| `dist/buildroot/configs/iyakki_no_firmware.fragment`, `iyakki_release.fragment` | Buildroot symbols off (linux-firmware, FFmpeg programs, ugly, x264/x265/OpenH264/FAAD/fdk, mpegpsmux ...) and the reasons |
| `build.sh` | full build: leaves out `iyakki_firmware.fragment`, appends the two fragments, output `-release`, refuses `hw`, `WEB_H264`, `WITH_FFMPEG` |
| `repack.sh` | takes only `regulatory.db` from the kernel donor, refuses `--firmware-from`, does not use `pc-firmware.keep`/`mac-firmware.keep` |
| `board/iyakki/post-build.sh` -> `release-profile.sh` | prunes `/lib/firmware` with `release-firmware.keep`, removes codec leftovers, installs `board/iyakki/release/`, stamps, verifies |
| `board/iyakki/post-image.sh` | boot fonts from the repository's DejaVu files, `FONTS-LICENSE.txt` |
| `legal/release-profile.txt` | forbidden symbols, reviewed gst-plugins-bad plugins, allowed firmware paths |

Real build (needs the owner's word; none of it was run):

```
dist/buildroot/legal-collect.sh --dry-run          # done: 189 + 78 packages, no archive missing, nothing would compile
dist/buildroot/legal-collect.sh --check            # real: make legal-info for web and mac, adds the apple-bce licence text
make -C target/buildroot-web gst1-libav-reinstall  # optional, builds one package: puts the inactive bridge into the web tree
IYAKKI_IMAGE_PROFILE=release WEB_IRIS=1 dist/buildroot/repack.sh pc --base target/buildroot-web --kernel-from target/buildroot-mac --graft git --cargo
./run.sh licence-check --strict --image target/buildroot-pc-release/images
```

Estimate: `legal-collect.sh` 5 to 15 minutes (copies, no compile); the repack minutes (unpack, zstd -19 of about 0.9 GB, genimage), about
30 minutes with the cargo build; the `make` step for the bridge a few minutes. The base trees are reused, nothing else rebuilds.
`legal-collect.sh` works around this machine's uutils `install` for its own run; a later `make` in those trees needs
`update-alternatives --install /usr/bin/install install /usr/bin/gnuinstall 100`.

## 6. Checks without a build

`cargo run -p iyakki-relcheck -- release-image-check .` (rules, keep list, fragments; `--config FILE` for a Buildroot `.config`;
`--simulate-cpio target/buildroot-pc/images/rootfs.cpio` applies the profile to a finished image and runs the licence scan on what is left).
`cargo test -p iyakki-relcheck` runs `tests/release_image.rs`: the release package set (`tests/data/release-image-packages.txt`) against the real
`legal/image-rules.txt`, one negative case per removed item, and `dist/buildroot/test-release-profile.sh` (the real `post-build.sh` hook chain on a
fake root, the guard with a fake modprobe, the refusals). Measured today with `--simulate-cpio` on the existing macweb and PC rootfs: after the
profile only "64 packages without a legal-info row" is left (fixed by `legal-collect.sh`).

## 7. Gate rows that should turn green

| Finding | Fixed by | State |
|---|---|---|
| linux-firmware Proprietary (355 / 1,421 files) | profile (no firmware) | green in simulation |
| NVIDIA GSP (438 files) | profile | green in simulation |
| 318 unowned firmware files | profile (PC firmware set not copied) | green in simulation |
| gst1-plugins-bad UNKNOWN | reviewed `accept` row with the files read, fragment pins the plugin set | green in simulation |
| ~64 packages without legal-info rows | `legal-collect.sh` (web, mac) | needs the real run |
| apple-bce without licence text | `legal/extra-licences/apple-bce` copied by `legal-collect.sh` | needs the real run |
| menu fonts licence | `FONTS-LICENSE.txt`, DejaVu from the repository | rows updated; install-root already passed |
| skeleton-init-common, ifupdown-scripts, dejavu, clang helpers | rows now cite their evidence (2026-10-27) | rows only |
| AV bridge missing in macweb | `gst1-libav-reinstall` in the web tree, then the repack | needs the one-package make |

Still red or REVIEW, not touched here: 9 unverified assets, floating download pins, catalogue gaps, the lock-only MPL crate.

## 8. Not verified

No image was built, booted or scanned as a release; the simulation edits a file list, not files. The GPU guard and every row of the hardware
table are reasoning, not hardware tests (the black-screen risk is the likeliest to matter). apple-bce's licence is a record of `MODULE_LICENSE("GPL")`; upstream has no licence file. dirac_parse,
a few small headers and BSD-style WebM helper files in gst-plugins-bad have no LGPL header. The recipe-level licence strings in the fixture are
snapshots. The first-boot screen and button do not exist in the shell yet. `WEB_MEDIA=1` adds unreviewed plugins and fails the config check.

## 9. The image scan of the release image: exact steps and what it checks (prepared 2026-10-28, not run)

`./run.sh licence-check --no-images` (the part that needs no image) was run on 2026-10-28: **PASS**, 57 download rows, 35 REVIEW rows
(`docs/project/LICENCE-REVIEW-ROWS.md`), legal-info of libre, libre-aarch64, mac, web, web.pre-media all 0 failures. One real failure was found and
fixed: the web tree's `legal-info` was stale and lacked the licence text of `iyakki-webview-helper-1` (`legal/extra-licences/iyakki-webview-helper`
was already in the repository; `dist/buildroot/legal-collect.sh web` was run, copies only, no compile). `release-image-check .` and
`cargo test -p iyakki-relcheck` (43 + 7 tests) also pass: 31 rows of `legal/release-profile.txt`, keep list and fragments are consistent.
A simulation on the existing (old) PC image still lists two groups the release repack must clear: 11 deselected packages without legal-info rows
(cage, cairo, cog, eudev, hwdata, libdisplay-info, libevdev, libinput, pixman, seatd, wlroots: removed by `prune-stale-packages.sh`, on by default for
`IYAKKI_IMAGE_PROFILE=release`) and the two files of gst-plugins-bad's WPE plugin option (`libgstwpe.so`, `libgstwpeextension.so`), which that script now
removes when `BR2_PACKAGE_GST1_PLUGINS_BAD_PLUGIN_WPE` is not `=y` (checked in `dist/buildroot/test-release-profile.sh`).

The owner (or a build agent with the owner's word) runs, in this order, on the machine that has the web and mac Buildroot trees:

1. `dist/buildroot/legal-collect.sh --check web mac` (legal-info of both base trees; copies, about 5 to 15 minutes; needs GNU `install`, see section 5).
2. `IYAKKI_IMAGE_PROFILE=release WEB_IRIS=1 dist/buildroot/repack.sh pc --base target/buildroot-web --kernel-from target/buildroot-mac --graft git --cargo`
   (writes `target/buildroot-pc-release`; the development images are not touched).
3. `./run.sh licence-check --strict --image target/buildroot-pc-release/images` (exit 1 on any FAIL; `--strict` turns REVIEW into FAIL).
4. Same for the macweb release image when it is built (`target/buildroot-macweb-release`).

What the scan checks on each image (`crates/iyakki-relcheck/src/imagescan.rs`, rules in `legal/image-rules.txt`, `legal/release-profile.txt`):
every file of `rootfs.cpio` is mapped to its Buildroot package and that package's licence in `legal-info/manifest.csv`; it fails on (a) a file that
matches a `deny` rule (vendor blobs, patent-encumbered codecs, restricted fonts) without an `except` row, (b) a file of a package whose licence is
proprietary, unknown or has no manifest row (unless an `accept` row names it), (c) a file no package owns and no `own`/`generated` rule explains,
(d) a font, icon, sound, video or model file neither owned by a licensed package nor listed in `legal/assets.txt`, (e) a `require` path (licence and
notice files) missing from the root file system, and, for a folder ending `-release`, (f) any file under `/lib/firmware` beyond the allowed ones.
The scan does not read the `/etc/iyakki-release` stamp (`IMAGE_PROFILE=release`, `FIRMWARE=none`, ...): check that by hand in the unpacked image.
Expected result with the current rules: every group of section 7 green; anything else printed is a finding to fix in the profile or to put in
a dated, reasoned row for the owner to sign. Estimate: scan under a minute (it reads the cpio, never unpacks it).
