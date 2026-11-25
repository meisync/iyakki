# IYAKKI 0.1.0-alpha.10: release notes

*An OS for agents.* The release after [alpha.8](RELEASE-NOTES-0.1.0-alpha.8.md), cut on 2026-11-24 (tag `v0.1.0-alpha.10`, commit @COMMIT@). Alpha.9 was not cut on its own (owner order 2026-11-21): its scope is part of this release. The release gate (`release-green --image`) @GATE@.

**Preview for developers and testers.** Most of what is new was checked by unit tests, fakes and QEMU; each feature's row in [FEATURES.md](FEATURES.md) says what was not tried on a real machine. The first real boots of an ARM board happened on the day of the cut and are written down honestly below.

## The numbers

On 1,874 active rows (1,899 in the matrix, 25 deprecated): implementation 99.4 percent, test 65.8 percent (1,843 rows with all code written). The test average is lower than in alpha.8 because 25 rows were split into a visible part and an API part (the new parts start at the evidence they really have), and new rows were added for the loader, the update faults and the lifecycle audit. Nothing was raised without proof: 34 rows went from 30 to 50 on end-to-end tests with fakes (lanes UT1 and UT2; the matrix totals line is the authority), one went from 50 to 70 on a booted QEMU proof, and one was put back to 30 because its real model server was never run.

## What is new

* **Settings is on ports.** Every Settings page group (account and system, apps and AI, browser and network, display, sound, devices and developer) takes small port traits instead of the shell context, with fake-port tests for a saved change, the audit line, a refused non-administrator, a busy answer and drawing at several sizes. The three big dispatch functions are thin: each lane's form, pick and confirm arms live in its own file. `settings.rs` went from 7,730 to 2,590 lines.
* **More tile crates.** Tales and Voice (`iyakki-tile-tales`, `iyakki-tile-voice`, with leaf crates for the story library, the voice model and the reading API), and the Store's engine, rating, drivers chip and Setup tab behind ports (`iyakki-tile-store`, `iyakki-store-model`). The crate move of the Store screens themselves is not done.
* **Sign-out ends what it should.** A registry of what a person's session started (sound, capture, processes): sign-out, a sound switch, lock and suspend each end the right things, the radio no longer plays on after sign-out, and a ratchet keeps new players from being forgotten. When Bluetooth headphones leave, the video player and the music player pause or stop too, and a wrong clock is warned about where it matters for the device key and the firewall.
* **The UI as its own program booted in QEMU** (`iyakki.ui=process`): process 1 is the core, `iyakki-ui` its supervised child, and three faults the first boot found were fixed (the boot screen drew over the program, the init reaper took the program's exit status, the connection cap closed the program after minutes).
* **Updates survive a power cut on the loader layout.** Eight cut points in QEMU and a 16-point unit sweep found and fixed two bugs in the update engine (a retried update kept half-replaced files as "previous", and one retry lost the old-root marker).
* **The boot loader for ARM boards.** `iyakki-boot` builds for aarch64 and passes its QEMU tests, with a generator for the Raspberry Pi 5 and KV260 boot files and card variants. These variants were not booted on a board yet.
* **Test tooling.** The Doctor "Manual tests" tab has 152 checklists (sound, voice, read-aloud, display, keyboard, battery and more); sound is to be proven by recording and listening (a listener and scenario verbs are in) and cameras by a fake network camera (a host-side server is in); evidence runs on a fresh image come after the gate. Documents for the beta (install, recovery, known limits) are in `docs/`.

## First boots on real ARM boards (2026-11-24)

* **Raspberry Pi 5 (16 GB):** reaches the IYAKKI home screen on HDMI with keyboard and mouse. The image carried only the C0 device tree and this board needs the D0 one (KI-1030); the first card's userland was old (KI-1034).
* **AMD Kria KV260:** U-Boot finds the card's menu, Linux and IYAKKI boot in about 8 seconds, DisplayPort shell at 1920x1080 with the card's device tree for carrier rev B (KI-1031), keyboard, mouse and Ethernet work. The SD template shipped a used data partition (KI-1032).
* **Both boards:** developer-mode pairing works but every signed request is refused (KI-1033); the Browser draws some pages badly (KI-1035); the Pi's Radio tile did not appear (the cause is not found yet; a missing sound device is the first guess); shutdown and restart cycles, sound and cameras were not tried.

## Fixes found by the gate

The first two gate runs found real failures that were fixed before the cut: a Rust check that did not accept the checklists of split rows, stale generated RPC code after the voice traits moved, poison-unsafe settings locks, a sleep-based flow test, a race in a permission test, and a Copilot test that only passed where its author was signed in.

## What did not make it

The Store tile crate move, the Browser, Chat and IDE tile crates (parked until after the beta), developer-mode requests on the ARM boards, the loader variants on real boards, and evidence runs of the autotest batches on the gate's image. The decisions taken on the owner's behalf are in `docs/development/ALPHA-8-DECISIONS.md`.
