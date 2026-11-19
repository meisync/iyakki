# Alpha.5 plan (draft, written 2026-11-15 while alpha.4's last gate runs)

Goes into `docs/project/ALPHA-5-PLAN.md` on `release/alpha.5` after the alpha.4 tag. The order is the owner's alpha.4 order, with what did not finish moved up.

## 0. The split, second half (top priority again)
a. **`iyakki-ui` as its own program** (rows F-SPLIT-03): the shell is a thread of PID 1 and the apps call `iyakki_agent::` 3,589 times in 354 files. Move domain by domain onto the core contract (`core_iface` / `base_abi`), with a ratchet on the `iyakki_agent::` count that only shrinks; then cut the process over a local socket. Design: `docs/development/UI-BOUNDARY.md`.
b. **UI restart without the device** and `iyakki dev push-apps` measured on a real device: goal edit -> running on the Lenovo under a minute for an app-only change (today 2-3 minutes because the device restarts).
c. **Apps in their own crates by group** (Settings, Browser, IDE, Chat, Store first).
d. Measure and publish the numbers in `FAST-LOOP.md`.


## 0b. Split the tests by layer and run only what changed (owner 2026-11-15, top of alpha.5)
Gates 71-75 each cost 55 minutes and each failure sat in one layer (apps/shell tests or agent tests) while the other stages passed; every fix meant a full rerun.
a. **Two test sets:** `./run.sh test-core` (base + core crates, the contract tests: boundary, dependency_ratchet, kernel_single_reader, feature_matrix, compat) and `./run.sh test-apps` (iyakki-apps, iyakki-shell, iyakki-ui-sdk, sample apps), built on `dist/scripts/crate-layers.txt` and `dist/scripts/test-affected.py` (exists, `--quick` and full modes).
b. **Run by change:** the loop and the pre-push check run `test-affected.py` (changed crates plus their dependents); a change under `crates/iyakki-apps` never runs core tests and the other way round, with the contract tests added when a layer boundary may have moved.
c. **Green stamps:** the gate records, per layer, the hash of that layer's source tree and the commit at which its tests last passed in a full run (stored in `target/alpha-check/stamps`, written only by a passing full run). A later gate reruns a layer only when its hash changed or a dependency below it changed; a failure in one layer reruns only that layer. A release tag still needs every layer green at the tagged commit (stamps for each layer at the same tree, or a full run).
d. **Flaky parallel tests:** keep the rule (known-flaky list only shrinks) and add the per-test isolation checks learned in alpha.4 (shared temp-folder names, process-wide red-sign flags, fixed sleeps, live hardware text).
e. Measure and publish times in FAST-LOOP.md.

## 1. UI SDK, second round (F-SDK-20, F-SDK-21)
Browser/client renderer (Canvas2D), core-backed dock and wallpaper ports, a storage port, `iyakki new-app`, WASM tile build of an SDK app, the shell's pane manager onto `Pane`/`ViewInfo`, then migrate existing apps (27 to ports) a group at a time.

## 2. Shared builds on k3s (branch `alpha.4/build-share`, rows F-BSHARE-01..05)
The store is built and proven in a real container; the pod never stayed up because the host disk was full. Free the disk, scale the deployment, re-run publish/fetch against the pod, merge the branch (it still needs the apps reachability test, the BOUNDARY row, feature_matrix), then per-person tokens and HTTPS, a device fetching from the store, promote `dev` to `beta`.

## 3. USB-C link (branch `alpha.4/usbc-link`, `docs/development/USBC-LINK-FINDINGS-2026-11-15.md`)
Finding: controllers and drivers are fine on the Mac mini 2018 and MacBook Pro 16,2, but the ports never enter Thunderbolt mode with the cable used. Do: compile and test the branch (directlink, usbports, the Settings Replug test), merge; test with a lightning-marked cable and a Thunderbolt dock or drive; test the USB Ethernet adapter route on two real machines; **fix the lockdown noise first** (`usbports.rs` reads debugfs on every poll and the kernel refuses it on Secure Boot images).

## 4. Looks leftovers
Menu entries, top-bar order, dock auto-hide and magnify, glass blur; try the Looks on the Lenovo and both Macs; contrast on pale wallpapers (device UI runner).

## 5. Model picker leftovers
Settings, Models still lists the stored NVIDIA catalog until fetched again; a "show all" fold for dropped non-chat models; the long "Choose another model" label; mouse-wheel and click checks on real hardware; local-model chips with a real Ollama.

## 6. Voice and router on real services
Gemini voice assistant and the model router with real keys on the Lenovo, honest rows, then decide the default.

## 7. Real-hardware checks owed
Home swipe on the Lenovo and MacBook trackpads (thresholds, three-finger reporting on the Apple pad); `push-apps` on a device; the device UI test runner on the alpha.4/5 Lenovo build (needs the owner's go: it flips theme and wallpaper briefly).

## 8. Image work still not done
Kernel rebuild (fanotify) and GRUB loadenv for the F-UPD-02 boot counter; one full image rebuild; Apple Silicon VM; the HP laptop plan.

## 9. Housekeeping
Disk at 95%: old packs (~23 GB) need the owner's yes; the progress page: publish alpha.4 numbers as a separate in-progress section (owner has not answered); fleet (paired systems sharing IDE work) and the model-router strategy plugin from the alpha.4 plan.
