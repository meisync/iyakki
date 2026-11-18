# Alpha.7 plan (draft, opened 2026-11-17)

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
