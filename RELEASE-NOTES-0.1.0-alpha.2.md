# IYAKKI 0.1.0-alpha.2: release notes

*An OS for agents.* Second alpha, 2026-11-10. Licence: Apache-2.0.

**This alpha is still a preview for developers and testers.** Do not use it in production or on a machine that holds data you cannot lose: the installer
erases the drive you pick. It has been tried on **a Mac mini 2018 (Macmini8,1, T2)** more than anything else, and on a MacBook Pro 14,1, an HP laptop and a
Lenovo laptop, plus QEMU, a desktop window and containers. [KNOWN-ISSUES.md](KNOWN-ISSUES.md) lists what is not proven.

## What changed since alpha.1

The full list is in [CHANGELOG.md](CHANGELOG.md). In short:

* **Boot and updates:** our own boot loader (menu on any key, previous-version rows, automatic fall-back), now in its own repository; component updates with a
  signed manifest and roll-back.
* **Bluetooth, written in-house:** pairing with passkey or PIN, keyboards and mice (HID, BLE), a phone's sound (A2DP) and its internet (tether); the Bluetooth page
  was redesigned and Scan opens a popup.
* **Encrypted home folders:** a private folder per account, a recovery key shown once, unlock at sign-in, lock at sign-out, password change, an administrator's reset
  keeps the folder sealed until the recovery key is used. This is new and has had **one real-hardware run**: do not put the only copy of anything in it yet.
* **Lock screen, Power for everyone at the machine, screen behaviour in developer mode.**
* **Cameras:** live view, recording, motion detection, two USB cameras at once; **PDF** reader, writer and viewer; **accessibility** in the OS and the browser.
* **Files and network:** an SFTP server and client, a Domains page, flow webhooks.
* **Voice:** the "My IYAKKI" session. The wake phrase is **experimental** and off by default.
* **Developer mode:** the screen answers when it is locked, dimmed or off; smaller fixes.
* **Removed:** the SRE PILOT demo app (its source lives in its own repository).

## What was tried where

| Feature | Where it was tried | Result |
|---|---|---|
| Boot of the image | Mac mini 2018, MacBook Pro 14,1, HP laptop, Lenovo laptop (owner, image 1109a) | works |
| Component update and boot confirmation | Mac mini 2018 | works |
| Bluetooth: pair, phone sound, tether with internet | MacBook Pro 14,1 (owner) | works |
| Bluetooth scan popup | Mac mini 2018 | works; nameless devices sit behind "Show N more" |
| Encrypted home: move files in, recovery key, sign-out and sign-in, Lock now, password change, administrator reset | Mac mini 2018 | works after four fixes found by this run |
| Encrypted home: Make on a fresh account (last fix) | not yet on hardware | unverified |
| Lock screen from the Power menu | Mac mini 2018 | works; the keyboard chords are not tried |
| Power menu for a plain user | Mac mini 2018 | works |
| Two USB cameras live | Mac mini 2018 | works |
| The final alpha.2 image (build alpha2c, from one T7 stick) | Mac mini 2018, MacBook Pro 14,1, Lenovo laptop, HP Pavilion 16 (owner, 2026-11-10) | runs on all four; the problems found are listed under Known limits |
| New logo on the boot menu, splash and sign-in screen | Mac mini 2018 (owner saw it) | works |
| Camera on the HP laptop | HP Pavilion 16 | works; a closed privacy shutter gave a black picture with no explanation (fix planned) |
| Everything else in the changelog | tests and fakes | see the notes on each row in FEATURES.md |

## Known limits

* The wake phrase has no real-voice measurement; the only model is synthetic. Leave it off.
* The encrypted home is a first version: whole-disk encryption, TPM sealing and exporting the container are not built.
* Bluetooth scan results arrive when the scan ends.
* **Mac profile: Ctrl+Cmd+Q does not lock the screen** (Ctrl+Cmd+L does) (KI-081). Use Ctrl+Cmd+L or the Power menu.
* **The camera light can stay on after leaving the Cameras tile** (seen on a Lenovo laptop) (KI-080). Sign out or switch off if the light stays on.
* **A covered camera shows a bare black picture** with no explanation (a closed shutter on an HP laptop looked like a broken camera) (KI-082).
* **A driver download with no network spins instead of failing** (KI-083). Cancel and retry once a network is up.
* Secure Boot is verified in QEMU only on most machines.
* Most feature rows are still `partial`: the matrix has a priority list and a plan (docs/development/PARTIALS-PLAN-2026-11-10.md).

## Getting it

The images, the window build and the checksums are made by `./run.sh release` ([docs/RELEASING.md](docs/RELEASING.md)); `SHA256SUMS` and the signed `release.json`
come with them. Verify before you use them.
