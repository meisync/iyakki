# Alpha.6 plan

Opened from the alpha.5 cut (tag v0.1.0-alpha.5). Carry-over from alpha.5:

- UI crates still use `iyakki_agent::` directly 509 times (table: UI-AGENT-USES.txt). Goal: keep shrinking; the stateful handles
  (`Services`/`Tools`: tasks, autopilot, k8sexp, cluster registry, store tile, about 203 members) move with the process split (F-SPLIT-03).
- Video codecs beyond MJPEG-in-AVI, camera PTZ.
- Hardware sessions (about 267 rows), Opus leftovers (FEC repair, multichannel).
- Trusted dev computer switch (F-DEVMODE-30..32) onto the Lenovo and Mac minis; Mac mini 2018 YouTube retest.
- Public GitHub export of alpha.4, alpha.5 and alpha.6 after the alpha.6 cut.

## Goal: the UI crates do not depend on iyakki-agent

Finish line (owner question 2026-11-17): `iyakki-apps` and `iyakki-shell` drop `iyakki-agent` from their Cargo.toml; the ratchet then pins the dependency, not only a count. Needs: store tree behind StoreApi (in progress), chat/settings/vault facets and traits, blocked modules (cameras, clis, disks, procowner), Services read facets by domain with a mechanical call-site rewrite (plan in SPLIT-GROUPS.md), and a small composition-root crate that wires the agent to the UI. Estimate about two weeks. Checkpoint Thursday 2026-11-19 22:00 PT (Friday night spare): store done, 150-200 uses left, both layers green on the merged branch.

## Order (owner, 2026-11-17): finish the core split first, then the UI SDK

1. Direct `iyakki_agent::` uses in the UI to the composition-root floor (now 65): lanes on story types, Linux apps group, cloudlaunch/localreg/tasktools, the chat model modules, vault and cluster leftovers.
2. The `Services` read facets (about 1,480 reads of about 185 members; the `dir()` part is done), with the codemod `dist/split/rewrite_services.py`, then the composition-root crate that wires the agent to the UI.
3. Drop `iyakki-agent` from the Cargo.toml of `iyakki-apps` and `iyakki-shell`; the ratchet then pins the dependency.
3b. Check the layered build once the split is done and preserve it: build with `nightly-push.sh --layers` (the `ui` layer `usr/bin/iyakki-ui` + `usr/libexec/iyakki-ui/**` apart from core), run `dist/buildroot/test-ui-layer.sh`, boot the layered image in QEMU, push a UI-only delta to a device (fast loop), and add the check to `release-green --image` so the layer split cannot regress (the ratchet pins the dependency, the gate pins the layers); keep the layered pack of the last green candidate under ~/iyakki-work/packs.
4. Green gate (`release-green`) and a signed candidate on the Lenovo and the Mac mini.
5. Then the UI SDK: events on real devices, a headless conformance test with every component, more events sources, a second app built by a small model from the docs.

## Status 2026-11-18: split finish line reached; layered build checked

* UI crates no longer depend on `iyakki-agent` (dev-dependency only); UI direct uses 0; the ratchet pins the dependency (head 76824a5a).
* Layered build (`nightly-push.sh --layers`, candidate 4): builds and boots in QEMU (`root layers mounted: base core ui`). Layers: base 355.8 MB, core 44.6 MB, ui 7.7 MB. The ui layer holds fonts and the webview helper, NOT a UI program: `usr/bin/iyakki` (127 MB) is still one program in the core layer. Next step (F-SPLIT-03): build the UI as its own program (`iyakki-ui`, in the ui layer, supervised by the core, talking through the core contract over the local UI socket), now possible because the UI crates no longer depend on the agent. Then the fast loop pushes only the ui layer (a few MB).
* Gate: `./run.sh release-green --commit REV` (about 1 hour); candidate packs under ~/iyakki-work/packs/alpha6-cand*.
