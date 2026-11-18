# IYAKKI 0.1.0-alpha.5: release notes

*An OS for agents.* The release after [alpha.4](RELEASE-NOTES-0.1.0-alpha.4.md), cut on 2026-11-17. The plan was `docs/project/ALPHA-5-PLAN.md`; what did not make it is listed at the end and moves to alpha.6.

**Preview for developers and testers.** Most of what is new was checked by tests, fakes and loopback servers; each feature's row in [FEATURES.md](FEATURES.md) says what was and was not tried on a real computer, a real model or in a real room. [KNOWN-ISSUES.md](KNOWN-ISSUES.md) lists what is only partly true. The live counts are on the progress page.

## What is new

* **The iyakki-core split.** The UI (`iyakki-apps`, `iyakki-shell`) reaches the core through facets (`iyakki-core-api`, `iyakki-agent-api`), each with a fake, instead of calling the agent directly: the direct `iyakki_agent::` uses of the UI went from 2,433 to 509 (a ratchet test keeps the number from growing, and counts re-export seams as uses). New leaf crates carry the pure parts: `iyakki-shared`, `iyakki-dash`, `iyakki-flowsx`, `iyakki-extagentx`, `iyakki-disksx`, `iyakki-memx`. The scripts that did the moves are in `dist/split/`; the design is `docs/development/UI-BOUNDARY.md`.
* **UI SDK for app authors.** Embeddable components that reach everything through the core with approvals: Chat assistant (`ChatView`), Browser (`BrowserView`), Files (with approved changes), Terminal, Editor, Audio and Media player (`AudioView`, `MediaView`: Motion-JPEG video, in-house FLAC and Opus), Camera (`CameraView`). Design tokens, shipped skins and font roles; display list version 2 (letter spacing, blur) and an image operation; a core-backed tile bridge for the browser and files.
* **Developer guide and skills** (`docs/sdk/`, `.claude/skills/`): first-app guide, concepts, recipes that compile, `iyakki new-app`, `app-check`, a barcode helper, and a graded benchmark: Haiku, Sonnet and Opus all passed its three tasks.
* **Fast loop and layered tests.** `./run.sh test-core`, `test-apps`, `test-affected`, green stamps per layer, `iyakki dev loop`; a live re-exec of the running program with the vault-key hand-over. The edit-to-running time without a device restart was not yet measured end to end on a device.
* **Dev machines.** "Trusted dev computer" and "Patches without asking": one persistent, audited switch (a tap on the device to turn it on) that answers the shell, patch and restart approval cards of a paired computer; restart and patch grants survive restarts.
* **Browser video.** YouTube playback recovers from a refused proof-of-origin token by reloading and a real click; sound sink fixes; an in-house Opus decoder (CELT, SILK, hybrid) written for the radio.
* **Fleet and model router.** Paired systems share IDE work; the router runs steps in parallel and can use read-only tools.
* **Many smaller rows** (PDF CFF fonts, CCITT fax, Thai line breaks, right-to-left text; web service data approvals and metrics scope; passkeys, Web Push and Matrix E2EE building blocks; framing and drawing 4x faster) are in the matrix.

## Tried on real machines

Lenovo Legion Pro 5 (browser video, dev-mode loop, push-apps, restart grants). The USB Ethernet adapter link was verified by the owner. Not tried on a real device: the fleet, the router with real keys, most voice and camera paths, the embedded components on a screen, the live re-exec on a booted machine.

## Not in this release (moves to alpha.6)

* The UI as its own process over the contract socket (F-SPLIT-03 is partial); `Services` and the stateful handles the UI still holds (`tasks`, `autopilot`, `k8sexp`, the cluster registry, the store tile): 509 direct uses remain.
* The USB-C to USB-C (Thunderbolt/USB4) link between two Macs: parked until a Thunderbolt-rated cable or dock is tried.
* Video: H.264, VP8/VP9 and AV1 have no in-house decoder; pan/tilt/zoom for network cameras; the SDK tile route for camera views.
* Hardware and person rows listed in the matrix (about 270), and the rows still partial or planned: about 1,230 partial and 50 planned on cut day. Impl was 94.0% and Tested 64.9% on the progress page.
* The GitHub public export of this release is the owner's decision.
