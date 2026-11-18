# IYAKKI 0.1.0-alpha.6: release notes

*An OS for agents.* The release after [alpha.5](RELEASE-NOTES-0.1.0-alpha.5.md), cut on 2026-11-18. The plan was `docs/development/ALPHA-6-PLAN.md`; what did not make it is listed at the end and moves to `docs/development/ALPHA-7-PLAN.md`.

**Preview for developers and testers.** Most of what is new was checked by tests, fakes, loopback servers and QEMU guests; each feature's row in [FEATURES.md](FEATURES.md) says what was and was not tried on a real machine.

## What is new

* **The core split is finished.** The UI crates (`iyakki-apps`, `iyakki-shell`) no longer depend on `iyakki-agent`: it is a dev-dependency for their tests only, the direct uses went from 2,433 (start of alpha.5) to 0, and the ratchet now pins the dependency itself. The UI reaches the core through `iyakki-core-api` and `iyakki-agent-api` facets and handles (about 25 facet files, `ServicesHandle`, per-domain read handles), shared data types in `iyakki-shared`, and a new composition-root crate, `iyakki-compose`, that is the only place naming both the agent and the UI. Guards against the failure that this kind of change can cause (a fake handle running in production): `ensure_real_agent`, `is_real` markers, a sweep over every facet default, and a real-wiring test that builds the program's own wiring.
* **App events for the SDK.** Apps register for named events and handle them in the foreground or after being away: online delivery in order, an offline queue per app with a `late` marker and a `missed` count, 41 events with scopes, rate limits and a switch per app in Settings > App events, real sources (connectivity, session, vault, display, clock, settings, flows, apps and updates, files, power), handler actions through the approval path, and the `app.permissions` event that tells an app when its permissions change. Chat tools, `app-check` rules, a recipe and `docs/sdk/events.md`.
* **QEMU session tests.** `./run.sh test-qemu`: one boot of the real image and named checks per feature row (74 rows were raised to Test 70 after their own check passed), with a scratch account, developer mode pairing by screen, a data volume profile and a k3s profile.
* **Developer mode on real machines.** The trusted dev computer switch was used to push and restart candidates on a Lenovo and a Mac mini without taps; a startup race that left the UI on a fake handle (KI-1010), and a production bug of the handle flip (the facets only accepted the concrete services type), were found by QEMU and by the gate before any device ran them.
* **A fast gate.** `./run.sh release-green`: both test layers, failures rerun alone to separate real failures from load flakes, one RESULT.txt, one lock; about 55 minutes. Feature work and unit tests run on the Macs (`mcargo`), the Linux host is for builds and the gate.
* **Smaller:** the browser data-protection and "correct my wording" paths work through the handle; the update delta patch refuses a live id (KI-999); private CA for saved remote machines; reconnect and `--follow` for the web service client; the router panel, deploy kit, git kit and speech kit facets.

## Tried on real machines

Lenovo Legion Pro 5 and Mac mini 2018 ran the signed candidates (developer mode, update and restart, Settings pages including App events). Not tried on a real device: the app events delivery on the real network beyond the QEMU guests, the AV add-on, Netflix and Prime Video (they need a Widevine module that is not shipped: F-BROW-101).

## Not in this release (moves to alpha.7)

* The UI as its own program (`iyakki-ui`) in the `ui` layer, supervised by the core over the local UI socket (F-SPLIT-03): the layered build builds and boots, but `usr/bin/iyakki` is still one program in the core layer.
* Install a site as an app tile (F-BROW-100, F-BROW-101); a typed bridge and React example for web tiles; more app event sources (rows F-SDK-202 to 217); the remaining SDK, WASM tile and client rows.
* Hardware and person rows listed in the matrix, and the rows still partial or planned: about 1,250 partial and 63 planned on cut day. Implementation was 93.5% and Tested 65.3% on the progress page (the owner's targets of 100% and 75% were not reached).
* The GitHub public export of alpha.4, alpha.5 and alpha.6 waits for the owner (decided after alpha.6).
