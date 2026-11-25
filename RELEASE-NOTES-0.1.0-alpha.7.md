# IYAKKI 0.1.0-alpha.7: release notes

*An OS for agents.* The release after [alpha.6](RELEASE-NOTES-0.1.0-alpha.6.md), cut on 2026-11-21 (tag `v0.1.0-alpha.7`, commit 26faa589). The plan was `docs/development/ALPHA-7-PLAN.md`: implementation first, no new requirements. What did not make it is listed at the end and moves to `docs/development/ALPHA-8-PLAN.md`.

**Preview for developers and testers.** Most of what is new was checked by unit tests, fakes, loopback sockets and QEMU guests; each feature's row in [FEATURES.md](FEATURES.md) says what was and was not tried on a real machine. The release gate (`release-green --image`) passed on this commit and the signed lean image booted in QEMU.

## The numbers

On 1,760 active rows (25 deprecated rows not counted; two parked rows, F-ARCH-08 and F-BOOT-11, counted at 0): implementation 99.6 percent, test 67.1 percent. 1,732 rows have all their code written and unit-tested; 26 are below 100 and the rest of this file names them. Hardware, real-service and owner items stay in each row's Manual column.

## What is new

* **The UI/OS split is complete in code.** The shell and the apps run as their own program, `iyakki-ui --socket`, talking to the core over a Unix socket (`/run/iyakki/ui/ui.sock`, mode 0600, peer checked by the kernel). Flows, the Store (including its tile apps), Lab, Radio, Media, Tales, sound, power, chat and developer-mode state are served by the core. Security of the socket is written down in [docs/UI-SOCKET-SECURITY.md](docs/UI-SOCKET-SECURITY.md): five layers, a table that rewrites every claimed right from the core's own accounts, and an audit test that fails when a new served call carries an unreviewed authority argument.
* **More of the kernel work moved into `iyakki-core-local`.** The agent has no direct device-crate dependency and no program start of its own; 25 kernel-reader pairs remain, all in the crates that own those facts.
* **UI SDK.** `dock.open` from a tile, tile accessibility names that keep state and value, pinch (ABI 3, only for tiles that declare it), the Edit and Cli apps written on the SDK (switch in Settings, Appearance), the terminal service for the CLI tile. Apps moved onto the app standard: Board, Firewall, Hostfiles and Flows (as job traits), plus ports for Runs, Doctor, Checkup, Mesh, Scan and LinuxWin.
* **Browser.** Hardware video decode on by default where the driver and plug-in exist, GPU capability probe with CPU fallback, a per-person, per-site and administrator switch (Settings, Browser, Graphics) and chat tools; script cache and style sharing measured by counts; Web Push in the Browser (no push service is set up by default); tables in PDF text, Unicode CMaps.
* **Install and storage.** exFAT files are moved below the new end before a shrink; the Install-to-disk, startup-disk and firmware code goes through typed core handles.
* **Testing.** Load-flake fixes with in-house locks and manual clocks, a rule that new tests lock shared resources, a test runner (`./run.sh autotest`) with evidence kept apart by level and strict rules for raising a Test number, and a Manual tests tab in the Doctor app with 30 human checklists.
* **Also:** a relay server tried live over pinned TLS, signed downloads manifest re-signed with the existing key, radio retries a station twice and then moves to the next, VPN and Kubernetes chat tools.

## What did not make it

* F-ARCH-07 (92): the 25 reader pairs in the owner crates, and a decision on typed traits for the tile device types.
* F-SDK-34 (99): the other old apps moving onto the SDK (22 were to do at the start; the small ones go in alpha.8, the medium ones in alpha.9) and the browser tiles for Edit and Cli.
* F-K8S-26, F-STORE-05, F-VPN-05, F-VPN-16, F-PDF-17, F-PDF-18, F-TALES-18, F-INBOX-09 and the downloads, Secure Boot and Apple Silicon rows: see the Notes of each row.
* Parked, never deprecated: F-ARCH-08, F-BOOT-11, F-BROX-26 (the real GPU path in the shell), the browser JSON viewer and cookie editor.
* Real-device runs of the UI program under the supervisor, a touch screen, sound by ear, Bluetooth, 4K and the other human checks (the Manual tests tab is the way to do them).
