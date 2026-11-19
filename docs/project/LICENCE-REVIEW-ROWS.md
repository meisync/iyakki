# Licence gate: the REVIEW rows and the image findings, for the owner to sign off

Written 2026-10-27. Nothing here is accepted: every row is a recommendation. A row becomes an accepted exception only when the owner says so and a
dated, reasoned row is written (legal/downloads.txt, legal/image-rules.txt or legal/release-profile.txt).

## 1. The 35 REVIEW rows of `legal/downloads.txt`

Checked again on 2026-10-28 with `./run.sh licence-check --no-images` (57 rows, 0 failures, 35 REVIEW). The 38 of the first version dropped to 35 because the
owner decision of 2026-10-27 made `kokoro` and `voice-pack` "nothing installed by IYAKKI" (Lemonade pulls the weights; rows now `n/a`) and the `indic-tts`
row is no longer in the file; those three rows below are kept only as history.

Kinds: **float+sum** = the version floats, the vendor's checksum is verified at every download; **none-vendor** = an exact version is pinned in
the code but the vendor publishes no checksum, and the item's screen shows a warning; **no pin** = nothing to pin in IYAKKI's code.
Recommendations: **accept** (in writing, with the reason), **pin** (chase a pin: a small code change), **replace**, **waive** (not a release matter).

| Row | Kind | What is verified | Recommendation | Reason |
|---|---|---|---|---|
| nodejs, cli:nodejs | float+sum | nodejs.org SHASUMS256.txt | accept | Newest LTS is the point of the item; the checksum file comes from the vendor over HTTPS. A pin would go stale in weeks. |
| k3s, cli:k3s | float+sum | GitHub release sha256sum file | accept | Same reason; the cluster tool should follow upstream fixes. |
| cli:kubectl, cli:helm, cli:gh, cli:grpcurl, cli:arduino-cli, cli:uv, cli:opentofu | float+sum | the vendor's checksum file | accept | Vendor checksum verified at every download; downloaded at the click, never shipped. |
| cli:terraform | float+sum | HashiCorp SHA256SUMS | accept | Checksum verified; the licence (BUSL-1.1) is stated on the screen; OpenTofu is the open alternative. |
| claude-code, cli:claude-code | float+sum | SHA-256 in Anthropic's manifest.json | accept | Vendor manifest checksum; proprietary, downloaded not shipped. |
| github-copilot, cli:github-copilot | float+sum | GitHub SHA256SUMS.txt | accept | Same. |
| ollama, cli:ollama | float+sum | sha256sum.txt | accept | Same. |
| ffmpeg, cli:ffmpeg | float+sum (MD5 only) | johnvansickle.com MD5 file | pin or replace | MD5 is weak. Option: pin one static build version and record its SHA-256 computed once over a trusted link; or leave and accept in writing that the builder publishes MD5 only. **Mechanism built 2026-10-28, value open:** a manifest `sha256` for the pinned version and file is checked in addition to the MD5; the owner picks the build and records it (DOWNLOADS-MANIFEST.md section 10). |
| python, cli:python | none-vendor | uv's built-in SHA-256 table (3.12.16) | accept | uv verifies each python-build-standalone file against its own table and the project publishes SHA256SUMS; IYAKKI adds nothing but pins the version. A pin of the SHA256SUMS digest is possible later. |
| duckdb, cli:duckdb | none-vendor | pip, version 1.3.2, wheel only | pin | PyPI lists a SHA-256 per file. Next step: `pip install --require-hashes` with a small requirements file (IYAKKI must then also pin the wheel's dependencies; duckdb has none). **Mechanism built 2026-10-28, values open:** `--require-hashes` is used as soon as the manifest holds the hashes of the pinned version (`dlmanifest pin ... --pypi-json`, DOWNLOADS-MANIFEST.md section 10). |
| cli:esptool, cli:azure-cli, cli:aider | none-vendor | pip, pinned version | pin (esptool), accept (azure-cli, aider) | esptool has few dependencies, so a hash-locked requirements file is realistic (**mechanism built 2026-10-28**; the hashes of esptool and all its dependencies are still to be recorded by the owner). azure-cli and aider have large dependency trees; hash-locking them is a maintenance burden: accept with the warning on the screen. |
| cli:codex, cli:gemini-cli | none-vendor | npm integrity of the package, pinned version | accept | npm checks the registry's sha512 integrity; dependencies float. The warning is on the screen. Alternative: a lock file shipped by IYAKKI (a larger change). |
| cli:aws-cli | none-vendor | version 2.38.6 | pin | Amazon publishes a PGP signature. Next step: verify it with an in-house OpenPGP check (Amazon's public key must be pinned in the repository). **In-house OpenPGP check built 2026-10-28** (`openpgp.rs`, tested with throw-away keys); the Amazon public key is not in the repository yet (`legal/aws-cli-pgp.asc` says `unset`), so the warning stays until the owner adds it. |
| cli:gcloud | none-vendor | version 590.0.1 | accept | Google publishes no checksum file for the archive. Accept with the warning, or replace by the apt/yum repository route (not applicable on IYAKKI). |
| kokoro, voice-pack | no pin | Lemonade fetches the model by name | pin | Needs Lemonade to take a revision or file hash (an upstream feature) or IYAKKI to fetch the weights itself and verify the Hugging Face file hash (recorded in the row: Kokoro-82M revision f3ff357, kokoro-v1_0.pth sha256 496dba11...). Until then accept in writing: weights are Apache-2.0 / MIT licensed, so the risk is integrity, not licence. |
| indic-tts | no pin | the person supplies the files | waive | Nothing is downloaded by IYAKKI. The weights have no stated licence, so the item is hidden by default (Store, Setup) until the owner decides; recommendation: keep it hidden, ask AI4Bharat for a licence statement or replace with an Apache-2.0 voice. |
| mac-firmware, mac-camera | no pin | per-file hashes in the extraction table | waive | Apple's files for the person's own Mac, hashed per file in the code; they cannot be pinned by version because they come from the person's Mac or Apple's CDN. Never shipped. |
| linux-apps | float+sum | Ubuntu SHA256SUMS / Docker Hub digest | accept | Distribution checksums; the GPG signature of SHA256SUMS is not verified (stated in the row); recommendation: add the GPG check later. |
| linux-windows | no pin | apt inside the sandbox | accept | apt verifies its own signed indexes; each Debian package keeps its licence. |

## 2. The image findings (`./run.sh licence-check`, image scan), analysed

The scan reads the existing `rootfs.cpio` of each image folder. None of the findings below is a reviewed-and-accepted row; vendor firmware is
handled by the release profile (`legal/release-profile.txt`, `forbid-config` rows for `BR2_PACKAGE_LINUX_FIRMWARE*`, enforced by
`board/iyakki/release-profile.sh` and `dist/buildroot/test-release-profile.sh`), not by an accept row.

### PC image (buildroot-pc, 41 finding groups)

This image was built with `repack.sh --firmware-from` (the development image with the whole linux-firmware set, pruned by `pc-firmware.keep`). It is
not a release image. All groups are vendor firmware or leftovers:

| File group | Owner | Licence evidence | Verdict |
|---|---|---|---|
| `lib/firmware/nvidia/*` (438 files) | none (denied rule) | NVIDIA GSP and booter firmware, vendor redistribution licence | belongs in the release exclusion (forbid-config `BR2_PACKAGE_LINUX_FIRMWARE_NVIDIA*`); offered as a download (Store, Setup, hardware firmware, `--with nvidia`) |
| linux-firmware, 1375 files (amdgpu, i915, Wi-Fi, Bluetooth...) | package `linux-firmware` | recipe licence is "Proprietary" (many vendor licences listed in its WHENCE) | release exclusion (same forbid-config); download item |
| `lib/firmware/ar3k`, `ath3k-1.fw`, `ath10k`, `ath11k`, `ath12k`, `qca` | none (grafted from the donor, not in a package list) | Qualcomm/Atheros firmware, WHENCE entries | release exclusion; download item `fw-qualcomm-bt` and friends |
| `lib/firmware/iwlwifi-*.ucode`, `.pnvm`, `intel/`, `xe/` | none | Intel firmware licence in WHENCE | release exclusion; download item `fw-hardware` |
| `lib/firmware/mediatek`, `rtl_bt`, `rtlwifi`, `rtw88`, `rt3290.bin` | none | MediaTek, Realtek, Ralink firmware, WHENCE | release exclusion; download items `fw-realtek-net` |
| 16 packages (cage, cog, wlroots, libinput, eudev, cairo, pixman, clang, libclc, hwdata, libdisplay-info, libevdev, seatd, spirv-*, nvidia-open) with a recipe licence but no legal-info row | leftovers of deselected packages (see section 3) and nvidia-open | recipe licences are MIT, LGPL, Apache-2.0 with exceptions, NCSA | remove from the release image (prune-stale-packages.sh); nvidia-open: row added through legal/extra-licences |

Nothing was added to `legal/image-rules.txt`: no group is a justified accept case.

### Mac and macweb images

linux-firmware 355 amdgpu files: same verdict, release exclusion. nvidia-open: legal-info row added (section 4).

### Web and web.pre-media images

15 packages without a legal-info row: leftovers of deselected packages (section 3). Fix: prune them (release) or rebuild the web tree clean.

## 3. Leftover files of deselected packages

Where they come from: not from the staging folder. Buildroot does not delete files that a package installed into `target/` before it was
deselected, and `build/packages-file-list.txt` lists every package build folder still on disk. The Oct 23 web tree was built when cage, cog, wlroots,
libinput, eudev (and cairo, pixman, hwdata, libdisplay-info, libevdev, spirv-*) were still selected; the current `.config` (no compositor,
`iyakki_web.fragment`) no longer selects them, so `make legal-info` has no rows for them while the packed image still carries their files.
Fix: `dist/buildroot/board/iyakki/prune-stale-packages.sh`, called by `repack.sh` before the hooks. Default: on for `IYAKKI_IMAGE_PROFILE=release`,
off otherwise (`IYAKKI_PRUNE_STALE=1` switches it on). A dry run on the current web tree finds 12 deselected packages and 191 files; no
remaining ELF file outside those packages needs their libraries (checked with readelf on the web target). Recommendation: also switch it on by
default for development repacks, after one look at the dry-run list; a clean rebuild of the web tree removes the cause. The scan keeps failing on
the existing images until they are repacked.

## 4. nvidia-open

The recipe says MIT or GPL-2.0 (NVIDIA_OPEN_LICENSE, COPYING), and the source tree's COPYING says the files are MIT, the kernel interface files dual
MIT / GPL-2.0. The Mac `.config` does not select the package any more, yet four .ko files are in the Mac image. `legal/extra-licences/nvidia-open/`
carries COPYING, a note and `manifest-row.csv`; `dist/buildroot/legal-extras.sh` (run by `legal-collect.sh`, or alone) adds the row, the licence
folder and the source archive to the Mac `legal-info`. The package folder is untouched.
