# IYAKKI 0.1.0-alpha.1: release notes

*An OS for agents.* First public alpha, 2026-10-19. Licence: Apache-2.0.

**This alpha is a preview for developers and testers.** Do not use it in production or on a machine that holds data you cannot lose: the
installer erases the whole drive you pick. It has been tried on **one real computer, a Mac mini 2018 (Macmini8,1, T2)**, plus QEMU, a desktop
window and containers.

## What this is

IYAKKI is a small operating system and desktop built around an AI assistant that works on your behalf, with the controls to keep it
safe: roles, scopes, budgets, approvals, vaults, a sandbox for code it runs, and a record of what it did. It boots from a USB stick
(or runs in a window on Linux) and brings its own tools: chat and agent mode, a browser, an editor and an AI IDE, K3s deploy tools,
data and dashboards, flows, an app store with signed packages, and the system tiles to see what the machine is doing.

## What is in this alpha

| Download | What it is | Notes |
|---|---|---|
| `iyakki-x86_64.iso` | The live image for PCs (BIOS and UEFI). Runs from RAM; keeps data if a data disk exists. | about 60 MB |
| `iyakki-install-x86_64.img` | The install image: write it with `iyakki-usb` to get a stick that boots and **keeps your data**. | about 260 MB, erases the stick |
| `iyakki-aarch64.img` | ARM64 UEFI disk image. Meant for VMs (for example on Apple Silicon); not for bare metal. | slow under emulation |
| `iyakki-<version>-linux-x86_64-window.tar.gz` | The shell in a desktop window on Linux x86_64. The quickest way to try it. | `./run-window.sh` |
| `iyakki-web-x86_64.*` (optional) | The big image with the full web engine (WPE WebKit, Mesa, GStreamer) for modern sites and video. | **about 2 GB**, needs 16 GB RAM to build, 4 GB+ to run; only if it is in the folder |
| `SHA256SUMS`, `release.json` + `.sig` | Sums, the signed manifest and signatures. | verify before use |

The Mac mini 2018 was tested with the **macweb install image** (the install image plus the full web engine and the Mac drivers,
`./run.sh build-macweb`); it is in the release folder only if it was built for it.

Not in this alpha: macOS and Windows window builds, an installer for the ARM64 image, Secure Boot on real hardware (it is verified in QEMU only),
Apple Silicon bare metal.

The full list of what exists is in [CHANGELOG.md](CHANGELOG.md); every feature, its status and its tests are in [FEATURES.md](FEATURES.md).

## What works, in plain language

* **Desktop shell and tiles:** a desktop of app tiles that follow the person's role (chat, editor and AI IDE, browser, system, inbox,
  calendar, radio, media, camera, TALES stories, data and dashboards, flows, store, settings), a clipboard, screenshots, help and UI sizes.
* **AI assistant and external agents:** chat and agent mode with roles, scopes, budgets, approvals (Ask first, Auto-approve safe, Autopilot,
  Allow for this session), vaults, a sandbox and an audit record. Local models (Ollama, Lemonade on the LAN) and cloud providers by key.
  Claude Code and GitHub Copilot CLI can be installed and signed in from the CLI tile; Jobs run long commands detached.
* **Apps, store, private store and Developer mode:** signed packages and catalogs, several stores with a Store switcher, a private store
  server, authoring kits for skills, MCP servers and tile apps, one-step publishing, and `iyakki dev ...` for developers.
* **Browser and media:** a small built-in browser, and (in the web and macweb images) a full web engine, WPE WebKit, that opens Google and
  YouTube (VP9/Opus video plays). A media player built on `ffmpeg`, a radio (MP3), and a CAMERA app (photo, video, passport picture).
* **Cluster and platform services:** connect to a cluster or set up a local k3s dev cluster, build, deploy and verify flows, Quick install of
  developer tools (Node.js, Python, DuckDB, Elasticsearch, `ffmpeg` and others, downloaded from their vendors, never shipped), Gitea and the
  private store as platform services.
* **Hardware (Mac):** on the Mac mini 2018: GRUB boot with a logo, HDMI audio and the built-in speaker/headphone jack (`apple-bce`), a USB
  webcam, a persistent `/home`, clock and time zone.
* **Installer and boot:** `iyakki-usb` writes and verifies an install stick (GPT or MBR, BIOS and UEFI); the data volume is formatted on first
  boot (178 ms on the Mac mini) and persists; a boot screen with progress; compressed swap (zram); an installer on the device.
* **Security:** capability tokens, encrypted vaults, an egress proxy, a hash-chained audit log, signed releases and packages, and Secure Boot
  with shim and MOK (verified in QEMU only).

## What to expect

* **On the Mac mini 2018 it boots from a USB SSD,** but slowly at the start: GRUB waits 20 to 30 s at the logo (a message asks you to wait 30 s
  to 1 min) while the initramfs is read through the Mac firmware's USB driver; the shell appears about 9 s after the kernel starts. In QEMU it
  takes about two seconds. On any other real PC it may not boot at all; that is the first thing we want to know.
* **Wi-Fi and Bluetooth of the Mac mini do not work yet** (Wi-Fi needs Apple's firmware copied from macOS; Bluetooth needs a patch file). Use Ethernet.
* **Video:** YouTube plays (VP9/Opus, in software, so large sizes drop frames). H.264/AAC video does not play on purpose (patent policy).
* **The assistant needs a model.** The easy path is a local Ollama (IYAKKI can install and start it from the CLIs tab); cloud models need
  your own key in the vault. Small local models make mistakes: IYAKKI adds guard rails, but check what it proposes. Only Ollama has been
  tried live.
* **First start:** create the administrator account on the setup screen. Nothing is sent anywhere until you add a provider or a catalog.
* **Rough edges everywhere:** layouts that overflow at extreme sizes, English-only phrase handling in the assistant helpers, a browser that
  is a simple one of our own (no video, no canvas) unless you use the web image.
* **Data formats may change** between alphas without a migration.

## Known issues

The honest list is [KNOWN-ISSUES.md](KNOWN-ISSUES.md). The big ones:

* **Real hardware: one machine only** (Mac mini 2018, Macmini8,1, T2, checked on 2026-10-18/19). Other PCs and Macs are untested.
  Expected to work but untested: other Intel PCs and Intel Macs. T2 MacBook Pro 2019 patches were never booted; Apple Silicon is a VM only.
* **Not working or not verified:** the Mac mini's Wi-Fi and Bluetooth; H.264/AAC video; Copilot full tool use and Autopilot with external
  agents (needs 'Allow Auto'); tile-app publishing against a store on k3s; the in-cluster registry one-click (open on NodePort 30500, no
  password); Developer mode beyond loopback; Secure Boot on real hardware; live cloud assistants without keys; YouTube sound while the radio
  plays and the 5.3 s input-scan fix are in the newest image but not confirmed on the Mac.
* The root filesystem is in RAM (changes outside `/var/lib/iyakki` and `/home` vanish at reboot); IYAKKI accounts are not Linux users and user
  files are root-owned; the installer erases the whole drive; the data volume is not encrypted; the browser front end has a self-signed
  certificate and no outside review.
* Cloud model providers and dashboard backends were tested against stand-ins, not the live services.
* Payments, the official store root key and a hosted catalog, flow Thrift nodes, webhook triggers and the remote runner do not exist yet.
* Platform services (Gitea, the private store), Jobs, voice engines and TALES pictures were tested with fakes and scripted stand-ins, not real
  clusters, speakers or picture models. Claude Code's own foreground shell cannot be turned off by IYAKKI. Withdrawing a package from a private
  store does not disable copies already installed. INBOX approvals and parallel tasks have no session grants.
* The audit-log race is fixed for one process; a chain that already reads BROKEN stays broken until you start a new log
  ([docs/ADMIN-GUIDE.md](docs/ADMIN-GUIDE.md#the-audit-log-and-its-hash-chain)).
* Speech engines are GPL and are not shipped; reading aloud uses what the computer has or the browser's voice.

## Who should try it

* People who build or run agents and want to see an agent-first OS with permissions that bite.
* Developers who want a self-contained environment: assistant, editor, browser and K3s tools in one place.
* Anyone with a spare PC or Mac, a USB stick and an hour, to test hardware (see [docs/BOOT-TEST.md](docs/BOOT-TEST.md)).

Not yet for: daily use, production, machines with data you care about (the installer erases a drive), production clusters, anything exposed to the internet.

## Try it

Fastest (Linux desktop): unpack the window tarball and run `./run-window.sh`.
With the source tree: `./run.sh window`.
On a PC: verify, then write the install image to a stick you do not mind erasing:

    sha256sum -c SHA256SUMS --ignore-missing
    iyakki release verify . --pubkey <the key printed on the download page>
    sudo iyakki-usb --install iyakki-install-x86_64.img      # asks which drive; erases it

`iyakki-usb` also checks the image against `release.json` or `<image>.sha256` next to it before writing. Step by step:
[docs/INSTALL.md](docs/INSTALL.md) and [docs/QUICKSTART.md](docs/QUICKSTART.md).

## Verifying the release

Every release folder has `SHA256SUMS` (+ `.sig`) and `release.json` (+ `.sig`), signed with an ed25519 key. The public key is in the README
and on the download page. Compare it with a second place before you trust it; the key is new with this release.

## Reporting problems

See [SUPPORT.md](SUPPORT.md): include the version (`iyakki --version`, or SYSTEM, About), how you ran it, and for hardware the
`iyakki hwreport` output. Security problems: [SECURITY.md](SECURITY.md), not a public issue.

## Tests

The last full `cargo test --workspace` (2026-10-18 evening): 3,845 passed, 0 failed, 59 ignored (live-network and hardware harnesses). A final full run is made before the tag. QEMU: boot (BIOS and UEFI), install, persistence, Secure Boot (17 checks), the macweb image with Google, Bing and YouTube.

## Thanks and licences

IYAKKI's own code is Apache-2.0. The images contain GPL/LGPL programs as separate executables; their source is available through the build's
`legal-info/` ([docs/LICENSES.md](docs/LICENSES.md)). Third-party Rust crates are permissive (`THIRD_PARTY_LICENSES.txt`, `SBOM.json`).
