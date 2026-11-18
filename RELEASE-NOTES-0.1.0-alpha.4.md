# IYAKKI 0.1.0-alpha.4: release notes

*An OS for agents.* The release after [alpha.3](RELEASE-NOTES-0.1.0-alpha.3.md), cut on 2026-11-15. The plan was `docs/project/ALPHA-4-PLAN.md`; what did not make it is listed at the end and moves to alpha.5.

**Preview for developers and testers.** Most of what is new was checked by tests and fakes only; each feature's row in [FEATURES.md](FEATURES.md) says what was and was not tried on a real computer, with a real model, or in a real room. [KNOWN-ISSUES.md](KNOWN-ISSUES.md) lists what is still unproven. Matrix totals at the cut: done 431, partial 1035, planned 72, deprecated 24 (1562 rows).

## What is new

* **Faster development: the apps are their own crate** (`iyakki-apps`). The shell depends on it, never the reverse; an app change no longer runs the shell's tests. `iyakki dev push-apps` ships the program as one small binary delta through the hot-patch path (the device answers its own approval card; it never reboots by itself), and the pack tooling now reserves a `ui` layer so a UI-only change moves only that layer. Measured times are in `docs/development/FAST-LOOP.md`. Design of the next step (a separate UI program): `docs/development/UI-BOUNDARY.md`.
* **UI SDK for new apps** (`iyakki-ui-sdk`): one drawing interface, a plain-text display list, size classes, gestures, widgets and look, dock and wallpaper hooks; one sample app (a to-do list) built only on it. How-to: `docs/development/UI-SDK.md`. In-memory ports only; the browser renderer and core-backed ports come next.
* **Looks and wallpapers:** a theme can change icons (shape, size, style including drawn 3D and glass), launcher layout, a dock of chosen apps, and bar items, not only colours; drawn wallpapers; Hari's UI changes are in. Settings, Appearance.
* **Chat model picker:** grouped by provider with a fold, Recent first, type to search, a scrollbar and wheel, local / network / internet chips, a tick on the current model; NVIDIA lists only chat models and learns which models the key can use ("N models are hidden", with a show link); the Model chip always stays beside an error.
* **Model router (experimental, off by default):** a planner model plans, lighter models do the steps; the person's own strategy is a sandboxed script; every change asks first.
* **Gemini voice assistant (experimental, off by default):** a live voice session started by Super+V or, if the person opts in, the wake phrase; every chat tool and skill is available to it with the same approvals; nothing leaves the computer without the person's consent.
* **Home swipe:** a three-finger (or two-finger) sideways swipe on a trackpad, or a one-finger swipe on a touch screen, moves between Home groups; off while an app, dialog or text field is open; a setting and a chat tool.
* **Device UI tests** (`dist/devtest`): scripted screens on a real device with screenshots and a safety guard, so the pipeline can check what the person sees.
* **Provider notes:** Claude and NVIDIA Nemotron were used live by the owner on the Lenovo (matrix rows F-AI-02, F-AI-12); locally hosted models (Ollama, Lemonade) were exercised earlier.

## Tried on real machines

Lenovo Legion Pro 5 (the picker, search, groups, wheel on the trackpad), Mac mini 2014 and 2018, MacBook Pro 16,2 (paired; signed `pc` image installed by the owner). Not tried on a real device: the swipe thresholds on a real trackpad, `push-apps`, the Gemini and router features against real services.

## Not in this release (moves to alpha.5)

* The separate UI program (`iyakki-ui` as its own process) and a UI restart without the device: the shell is a thread of PID 1 and the apps call the agent 3,589 times; it needs a domain-by-domain move to the core contract first. Rows F-SPLIT-03 and F-SPLIT-04.
* The shared build store on this project's k3s (built and tested against a real container, not yet running as a pod because the host disk was full), branch `alpha.4/build-share`.
* The USB-C (Thunderbolt/USB4) link between two Macs: the controllers and drivers are fine but the ports never entered Thunderbolt mode with the cable used; the findings, the diagnostics and the USB Ethernet adapter route are on branch `alpha.4/usbc-link` (`docs/development/USBC-LINK-FINDINGS-2026-11-15.md`).
* UI SDK: browser renderer, core-backed dock and wallpaper ports, `iyakki new-app`, moving existing apps onto it.
* Looks leftovers (menu entries, top-bar order, dock auto-hide and magnify, glass blur); the picker's "show all" fold for non-chat models and the Settings pages that still list the stored NVIDIA catalog.
