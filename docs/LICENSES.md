# Licences, firmware and trademarks

*Not legal advice. This is what the project does and why; the texts are the authority.*

## IYAKKI itself

Apache License 2.0: [`LICENSE`](../LICENSE) and [`NOTICE`](../NOTICE). Copyright 2026 IYAKKI contributors. The policy and the reasoning
(why the GPL parts of an image do not infect our code) are in [`LICENSING.md`](../LICENSING.md).

## What is checked on every release

* `cargo deny check licenses` in both workspaces (the main one and `crates/iyakki-gui`): only the licences in `deny.toml` are allowed
  (Apache-2.0, MIT, BSD-2/3, ISC, Unicode-3.0, Zlib, CC0-1.0, 0BSD, Unlicense, BSL-1.0, plus two named exceptions). `./run.sh alpha-check` runs it.
* `THIRD_PARTY_LICENSES.txt` is regenerated with `cargo about` (command in `LICENSING.md`) and shipped inside every image in
  `/usr/share/doc/iyakki/` and in the window tarball.
* `SBOM.json` (SPDX-lite, from both `Cargo.lock` files and Buildroot's `legal-info/manifest.csv` when a build left one) is made by
  `iyakki-relcheck sbom` and shipped in the release folder. Licences in it are what each package states, not a legal conclusion.

## Components that are not purely permissive

Regenerate this list with `target/debug/iyakki-relcheck licenses .` (it reads both lock files, the local cargo registry and the Buildroot manifests that exist).

### Rust crates (statically linked or build-time)

"permissive alternative exists" means an `OR` expression where we use the permissive alternative (MIT or Apache-2.0). "copyleft only" means
no such alternative is stated.

| Component | Version | Licence | Origin | Use |
|---|---|---|---|---|
| attohttpc | 0.30.1 | MPL-2.0 | cargo:Cargo.lock | copyleft only |
| r-efi | 5.3.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later | cargo:Cargo.lock | permissive alternative exists |
| r-efi | 6.0.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later | cargo:Cargo.lock | permissive alternative exists |
| epaint_default_fonts | 0.36.2 | (MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0 | cargo:crates/iyakki-gui/Cargo.lock | includes a non-permissive part (AND) |
| self_cell | 1.3.0 | Apache-2.0 OR GPL-2.0-only | cargo:crates/iyakki-gui/Cargo.lock | permissive alternative exists |

How to read the rows: `r-efi` is `MIT OR Apache-2.0 OR LGPL-2.1-or-later` and `self_cell` is `Apache-2.0 OR GPL-2.0-only`: we take the permissive
side. `epaint_default_fonts` carries the bundled egui fonts under OFL-1.1 and the Ubuntu Font Licence, which cover only the font files (allowed by a
named exception in `deny.toml`; they are in the window build only). `attohttpc` (MPL-2.0, file-level copyleft) is in `Cargo.lock` only as a platform-specific dependency of another crate: `cargo tree -i attohttpc` prints "nothing to print" for the workspace's target, it is not linked into a shipped binary, and `cargo deny check licenses` passes. Re-check this before each release (`./run.sh alpha-check` runs `cargo deny`).

### Programs in the libre image (Buildroot; separate executables)

These are GPL/LGPL programs and libraries that are part of the image, not linked into the `iyakki` binary except where noted. Anyone who
redistributes an image must offer the corresponding source: `./run.sh build` runs `make legal-info` and leaves the source archives and the
licence texts in `target/buildroot-libre/legal-info/`. Ship that folder (or a written offer) with the images.

| Component | Version | Licence | Use |
|---|---|---|---|
| alsa-lib | 1.2.15.3 | LGPL-2.1+ (library), GPL-2.0+ (aserver) || copyleft only |
| alsa-utils | 1.2.15.2 | GPL-2.0 || copyleft only |
| busybox | 1.37.0 | GPL-2.0, bzip2-1.0.4 || copyleft only |
| ca-certificates | 20260223 | GPL-2.0+ (script), MPL-2.0 (data) || copyleft only |
| e2fsprogs | 1.47.3 | GPL-2.0, MIT-like with advertising clause (libss and libet) || copyleft only |
| gcc-final | 14.3.0 | GPL-3.0-with-GCC-exception || copyleft only |
| grub2 | 2.12 | GPL-3.0+ || copyleft only |
| iproute2 | 6.17.0 | GPL-2.0+ || copyleft only |
| kmod | 33 | LGPL-2.1+ (library) || copyleft only |
| libnl | 3.11.0 | LGPL-2.1+ || copyleft only |
| linux | 6.12.112 | GPL-2.0 || copyleft only |
| linux-headers | 6.12.112 | GPL-2.0 || copyleft only |
| util-linux | 2.41.4 | GPL-2.0+, BSD-4-Clause, LGPL-2.1+ (libblkid, libfdisk, libmount), BSD-3-Clause (libuuid), BSD-2-Clause (xxhash), ISC (rfkill), MIT (hardlink, flock) || copyleft only |

Also in every image but not in this table because it is permissive: Dropbear 2026.91 (MIT, BSD-2-Clause, public domain), one multi-call program that is the diagnostics server (`dropbear`) and the SSH client of the CLI tile (`dbclient`, `ssh`, `scp`, `dropbearkey`; [`CLI-SSH.md`](CLI-SSH.md)). OpenSSH (BSD/ISC) is not bundled; on a computer the CLI tile uses the one the system has.

The `web` and Mac images also carry **git 2.53** (GPL-2.0, parts LGPL-2.1+), **libcurl 8** (curl licence) and OpenSSH's **`ssh`** client alone (BSD/ISC; installed as `/usr/libexec/iyakki/git-ssh`, not the CLI tile's `ssh`) for the AI IDE's repositories ([`GIT-AND-SSH.md`](GIT-AND-SSH.md)); git is a separate program IYAKKI starts through an argument array, so the Apache-2.0 licence of IYAKKI is unaffected, and its source is in `legal-info/` like the rest (`make legal-info` was run for these three packages: git `COPYING` and `LGPL-2.1`, OpenSSH `LICENCE`, curl `COPYING`, with sources). About 14.3 MB in the file system, 3.4 MB packed. The libre image has none of them.

The optional `web` image adds LGPL libraries (WPE WebKit, GStreamer, glib, libsoup, libgcrypt, ffmpeg, ...); the dynamic linking keeps them
replaceable. `ffmpeg`/H.264 can have patent implications in some countries (`WEB_H264=1` is off by default). Its `legal-info/` is produced the
same way (`target/buildroot-web/legal-info/`).

## Firmware and drivers: what we ship and what you supply

| | Shipped in the images? | Where it comes from |
|---|---|---|
| Linux kernel and in-tree open drivers | yes (GPL-2.0, source in `legal-info/`) | Buildroot, kernel 6.12.112 |
| Proprietary firmware blobs (`linux-firmware`: Wi-Fi, NIC, GPU) | **No in the default `libre` image.** Only in the optional `hw` image (never built for this alpha) | each blob has its own licence |
| Mac drivers (applesmc, hid_apple, Cirrus audio) | only in the optional Mac variant (`./run.sh build-mac`, untested) | in-tree kernel options |
| apple-bce (t2linux; T2 Macs: sound, internal USB) | only in the optional Mac variant: out-of-tree kernel module, GPL-2.0 (`MODULE_LICENSE("GPL")`), plus two IYAKKI patches under the same licence; kernel code, source in `legal-info/` | `dist/buildroot/package/apple-bce`, built against the image kernel |
| Broadcom `wl` (BCM4360 Wi-Fi of the Mac mini 2014) | **never**: not redistributable. The owner builds it on their own PC (`dist/drivers/build-wl-pack.sh`, Broadcom's licence printed and accepted first) and installs it as a signed pack; output and `*.ko` stay out of git | the owner's own machine only |
| FaceTime HD camera firmware (Apple) | **never**: you supply Apple's `AppleCameraInterface` file after accepting Apple's licence notice; IYAKKI extracts and checksums it | the person, on their own Mac |
| Wi-Fi / Bluetooth firmware for your machine | not by default | you supply it as a driver pack from the Store (a USB folder or file), stored on the data volume under `firmware/<pack>/` |

The details, section by section, are in [MAC-HARDWARE.md](MAC-HARDWARE.md); the driver pack mechanism is in [STORE.md](STORE.md). The rule: **the
project does not redistribute firmware it has no licence to redistribute**, and never downloads Apple's files on the person's behalf in this alpha.

## Tools you install later

kubectl, k3s, aws, gcloud, Ollama, container images and other CLIs installed from the CLIs tab or pulled by deploy tools are downloaded from
their vendors at your request and keep their own licences. They are not part of IYAKKI or of the images.

## Trademarks and branding

IYAKKI is the name of this project. Do not use the names or logos of **Claude, Anthropic, GitHub, Apple, Microsoft, Google, OpenAI, Docker,
Kubernetes/CNCF** or any other company as IYAKKI branding, and do not imply endorsement. They appear in the product and the docs only to name
what IYAKKI can talk to (for example "Claude Code" as a coding agent it can drive, "macOS" as a platform), as nominative references, and
those names belong to their owners. The feature tour and screenshots use IYAKKI's own design; keep it that way in forks and in release art.

**The IYAKKI name and logo.** The Apache License 2.0 covers the copyright of the code and of the files in this repository, the logo file included, so
the software can be copied and redistributed with its logo. It does **not** give anyone the right to use the name or the logo as a trademark
(section 6 of the licence says so): a fork or a product built on IYAKKI should use its own name and its own logo and must not suggest that it is
IYAKKI or endorsed by the project. The logo files are kept apart from the code (`crates/iyakki-shell/assets/logo.png`, the GRUB background) and listed with their origin in `legal/assets.txt`.
Both are drawn by the project's own scripts (`dist/brand/draw-logo.py`, `dist/brand/make-brand-assets.py`) from fixed numbers; no third-party or AI-generated picture is used.
