# Release page text: v0.1.0-alpha.6

Text for the GitHub release page of `v0.1.0-alpha.6`. Replace `OWNER/REPO` with the public repository and fill the checksums section when the files are final. Everything between the two lines below is the page.

---

## IYAKKI 0.1.0-alpha.6: an OS for agents (alpha preview for developers and testers)

IYAKKI is an operating system written in Rust, built around an AI assistant that may *do* things (read files, run tools, open pages) only through one permission model, with a yes from you for anything risky and a tamper-evident log of every decision.

* **Alpha. Not for production.** Data formats and upgrade paths are not promised to stay stable. The USB writer erases the drive you pick.
* It ships **no AI model**: you connect your own (Ollama, an OpenAI-compatible server, or a cloud provider with your key in the encrypted vault).
* New in this release: the core split is finished (the screen and apps no longer reach into the agent crate directly), app events for the SDK, a QEMU session test layer, developer-mode updates tried on a Lenovo laptop and a Mac mini 2018, and a faster release gate.
* It booted from a USB drive on six real computers over the alpha series (Mac mini 2018 and 2014, MacBook Pro 14,1 and 16,2, an HP Pavilion 16 and a Lenovo Legion Pro 5); every other machine is untested.
* It is Apache-2.0, built from permissive dependencies only; the images contain GPL/LGPL base programs as separate executables with their source offer.
* Of 1775 feature rows, 431 are done (written and proven), 1253 are partial and 63 are not started; the progress page shows the numbers.
* Please report what you find: Issues for bugs and hardware reports, Discussions for questions.

### Try it in 3 steps

1. **Download** the install image and `SHA256SUMS` below, and check them (see "Checksums"). Never use a file whose checksum does not match.
2. **Boot it**: in a virtual machine (QEMU, no risk) or from a USB stick or SSD on a spare 64-bit PC or an older Intel Mac. A stick is erased when written. Secure Boot: switch it off for the alpha, or enrol the project's key once (the steps are in the quickstart).
3. **Create the administrator account, connect a model** (Settings, Overview, "Choose the AI model") and ask your first question in CHAT.

The first-hour guide, step by step: https://github.com/OWNER/REPO/blob/v0.1.0-alpha.6/docs/QUICKSTART.md (PLACEHOLDER: set the real address)

### What works

* Boot from a USB drive (UEFI, and legacy BIOS in tests), a data volume that keeps your account, files and settings across restarts, sign-in with a password, the launcher with its tiles.
* CHAT with a model you connect, agent mode with approval cards (Allow once, for this session, deny), a log of every run, an encrypted vault for keys, DOCTOR and Share diagnostics (a support file you read and send yourself; nothing is uploaded).
* The built-in browser engine for simple pages and a full web engine for the web (YouTube with VP9 video plays), a local k3s dev cluster, the AI IDE, developer mode for pushing updates from another computer.
* Settings, System update (install from a stick or folder, keeps the previous version, Roll back); the boot menu keeps "IYAKKI (previous version)" after an update.
* A desktop window on Linux (`./run.sh window`) and a QEMU path for trying everything without a stick.

### What does not work, or was not tried (honest list)

* **No AI model is included**; chat answers only after you connect one.
* **Netflix and Prime Video do not play** (they need Widevine, which is not shipped). **TikTok's video does not play** (H.264/AAC are not shipped; an optional add-on is the planned way).
* **Wi-Fi and Bluetooth on Macs** need Apple's firmware that you install yourself; the Mac mini 2018 has no working Bluetooth; Macs with the Broadcom BCM4360 card (2013 to 2015) have no Wi-Fi driver. Use a cable or a USB adapter.
* A direct USB-C to USB-C link between two computers does not work yet. Touch on a Mac mini 2018 after a cold boot was fixed in the shell; gestures with several fingers are still open.
* Apple Silicon Macs are not supported on bare metal (a VM path exists and was never run on a Mac).
* Secure Boot with the project's key: verified in QEMU and on two laptops; not walked through from a public release by a newcomer.
* Suspend, battery reporting, Linux apps (Debian sandbox) on an installed image, and most hardware other than the machines listed on the tested-hardware page were not tried or failed.
* The UI as its own program (a separate process under the core) and installing a site as an app tile are planned for alpha.7.

### Checksums and signature

PLACEHOLDER: fill in when the files are final. One line per file as `sha256sum` prints it, then the public key for `iyakki release verify DIR --pubkey HEX`, and the fingerprint of the project's Secure Boot certificate (compare it with the one MokManager shows).

```
SHA256SUMS: <paste here>
Release signing public key (hex): <paste here>
Secure Boot certificate fingerprint: <paste here>
```

### More

* Release notes: https://github.com/OWNER/REPO/blob/v0.1.0-alpha.6/RELEASE-NOTES-0.1.0-alpha.6.md (PLACEHOLDER: set the real address)
* Tested hardware (what ran where, what did not): https://github.com/OWNER/REPO/blob/v0.1.0-alpha.6/docs/TESTED-HARDWARE.md (PLACEHOLDER: set the real address)
* Known issues: https://github.com/OWNER/REPO/blob/v0.1.0-alpha.6/KNOWN-ISSUES.md (PLACEHOLDER: set the real address)

---

## Notes for whoever publishes this

* Sources: `RELEASE-NOTES-0.1.0-alpha.6.md` (on branch `cut/alpha.6`, identical in this tree) and the totals line of `FEATURES.md` (the cut-day numbers were about 1250 partial and 63 planned; the totals above are those of the tree this page was written from, so recompute them from the cut commit before publishing).
* The list of real computers is the alpha-series list (see `docs/TESTED-HARDWARE.md`); the alpha.6 candidates themselves ran on the Lenovo and the Mac mini 2018 only.
* No file names are given because the final asset names are not known yet.
