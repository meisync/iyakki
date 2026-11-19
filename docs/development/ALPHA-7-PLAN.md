# Alpha.7 plan (draft, opened 2026-11-17)

**Scope freeze (owner, 2026-11-18): the only target is Implementation 100%: every existing row's code is written. No new requirements and no new rows. Use the layer build for the loop (the UI as its own program in the ui layer: F-SPLIT-03 first). Implementation means code written; hardware proof stays Manual 'needed'. Rows that wait for an owner decision get a conservative default, listed in docs/development/ALPHA-7-DECISIONS.md. Lanes: impl-split, impl-sdk, impl-media, impl-sys (row lists in the lane briefs); side lanes: deflake (tests must not be flaky), user-ready docs.**

Carry-over from alpha.6 (cut 2026-11-18, tag v0.1.0-alpha.6 on cc2dec28; gate PASS; fast release checks and cargo deny licences pass):
0. First: the zero-warnings build check lists about 450 warnings left by the split (unused re-exports and imports): clear them (`cargo fix`, then by hand) so `alpha-check --release` passes fully; run `alpha-check --release` on a host where target is local (it cannot run through the remote build wrapper: it looks for its helper binaries in a local target folder). Then the deflake work (docs/development/FLAKY-AUDIT.md), the GitHub migration (GITHUB-MIGRATION-PLAN.md), F-SPLIT-03 (the UI as its own program), the SDK event sources and the dogfooding milestone.

Items asked for so far:

1. **Install a site as an app tile** (owner, 2026-11-17; F-BROW-100, F-BROW-101): a Browser menu item that turns the page being shown into its own tile (own window, own cookie and storage profile, origin allow-list, name and icon from the page). Examples named by the owner: Amazon Prime Video, Amazon shopping, TikTok, YouTube, Netflix.
   * Owner-observed 2026-11-17 on the Lenovo/Mac mini: YouTube plays in the Browser tile with the base image (VP9 and Opus). TikTok plays only when the audio and video add-on is installed (FFmpeg and the IYAKKI media plugin/extension, downloaded by the person at their own click, docs/FULL-BROWSER.md), because it needs H.264 and AAC. So the site tile must detect H.264-only sites and offer the add-on install card instead of a blank player. Netflix and Prime Video are not playable (F-BROW-101).
   * Works as a web-bundle-style tile whose page is the site's address, run in the browser engine with a per-tile profile.
   * Sites that need a content decryption module (Netflix, Prime Video) cannot play protected video on the release image (no Widevine in the permissive-only image). The tile says so plainly. Whether and how a person may supply a module is an owner decision (F-BROW-101).
   * Chat tool `browser_install_site` with approval; every feature reachable by chat and screen.
2. (from alpha.6) React and TypeScript helpers for web-bundle tiles: typed `window.iyakki` bridge package, app events and permission changes through the bridge, a React example in docs/sdk/recipes.

Carry-over from alpha.6 will be listed here at the alpha.6 cut.

## Later: ARM boards (owner, 2026-11-17), only after Implementation 100% and Tested 75% or more

* **AMD Kria KV260** (K26 SOM, Zynq UltraScale+ MPSoC, 4 GB, quad Cortex-A53). The owner will add an SD card. Needs an aarch64 image for the board (kernel fragment for the Zynq MPSoC: firmware and power domains, DisplayPort, Ethernet, USB, SD), a U-Boot boot path (the x86_64 loader and GRUB do not apply), a QEMU boot test where one exists, then a real boot over serial and Ethernet. 4 GB: shell, apps and tiles only; local models use a remote model host.
* **Raspberry Pi 5** (aarch64, 4 or 8 GB): the same aarch64 image family with a Pi 5 kernel and device tree.
* Start from `dist/buildroot/configs/iyakki_aarch64_defconfig` (boot-tested only in QEMU `virt` so far) and docs/development/APPLE-SILICON-VM-PLAN.md.

## Dogfooding: develop IYAKKI inside IYAKKI's AI IDE (owner question 2026-11-18)

State today: the AI IDE (editor, board, parallel tasks, agents, git, cluster view, preview) works for its project targets (tile app, web service on k3s...). The IYAKKI repository itself is not a target yet.
Needed, in order:
1. A project target "IYAKKI OS (Rust workspace)": open the repository, `cargo check/test -p CRATE` through the job runner with a memory-aware cap, the Rust toolchain installed by the person's click (rustup, a third-party tool, never shipped).
2. Builds offloaded to a paired build host from the IDE (Fleet: paired systems share IDE work), so a laptop does not compile the 100+ crate workspace alone.
3. Self-deploy: a UI-only update of the running machine (needs F-SPLIT-03: the UI as its own program in the ui layer), with the existing "previous version" boot entry as the rollback.
4. Git against the new GitHub `main` and the Gitea working remote (docs/development/GITHUB-MIGRATION-PLAN.md).
Apps are part of it (owner, 2026-11-18): (a) web-bundle tile apps: the IDE already scaffolds, previews and deploys them (F-IDE-11, F-IDE-25 partial); the typed `window.iyakki` bridge, a React example and app events in the bridge are the gaps; (b) Rust SDK apps (`iyakki new-app`, `app-check`, tests with the SDK fakes, SAMPLE-style preview, packaging): needs a project target "IYAKKI SDK app (Rust)" with the same cargo job runner and a checkout of the SDK crates; (c) the built-in apps live in the OS workspace (target above).
Milestone: one change to a Settings page made, tested and deployed to the machine from its own IDE, without a second computer.
