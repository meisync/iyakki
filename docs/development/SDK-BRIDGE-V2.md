# SDK bridge v2: the rules and the namespace list

Status: decided and built 2026-11-12 (F-SDK-05 to F-SDK-13). The owner left the interface to the engineer: "if it is extensible, backward and forward compatible, then we are good".
The calls themselves are specified in [SDK-API.md](SDK-API.md) section 14; this note records the rules they follow and the list with versions.
It applies the compatibility charter ([CORE-COMPATIBILITY-DESIGN.md](CORE-COMPATIBILITY-DESIGN.md)) to the app bridge `window.iyakki`.

## The rules

1. **Additive and versioned.** The SDK version is `MAJOR.MINOR` (`iyakki.version`, `SDK_VERSION` in `crates/iyakki-core-api/src/sdkbridge/mod.rs`). A minor release only adds a namespace, a call, an optional argument, an event or a reply field.
   Nothing is renamed, removed or given another meaning. A removed thing would answer with the replacement named, in a major only.
2. **Every namespace is optional and found by name.** `iyakki.has('embed')`, `iyakki.has('speech.listen')` and `iyakki.capabilities()` answer from the manifest table, not from a version number. A name is "ready" only when the host knows it and the app declared the capability.
3. **Unknown is data.** A namespace or call the host does not know answers `Unsupported` ("iyakki.X is not available in this version of IYAKKI (SDK 1.2)") with `op`, `namespace` and `sdk`; unknown arguments of the new namespaces are ignored and listed in
   `ignored`; the unknown answer is the same whether or not the app holds the capability, so it does not leak. The generic door `iyakki.call(op, args)` lets a newer app try a name an older bridge does not have a function for.
4. **Old apps keep working on a newer host:** the original calls (SDK 1.0 and 1.1) are untouched and a recorded transcript of them is replayed by a test (`tests/compat/alpha3/sdk-bridge-v1.json`).
   **A newer app degrades on an older host:** it feature-detects (`typeof iyakki.has === 'function' && iyakki.has(...)`); its new capability keys are listed in `requires.capabilities_optional` so a newer-but-not-newest host ignores what it does not know instead of refusing the install;
   `app.json` `"sdk"` states the least version it needs, and a host that is too old says "update IYAKKI". A test plays an older host (`Version { 1, 1 }`) against a future-style call.
5. **One place lists the names.** `NAMESPACES` and `CAPABILITIES` in `iyakki-core-api::sdkbridge` (part of the published core interface, so the surface snapshot `core-surface.txt` and the compat gate see every addition).
   The typings, the emulator, the install card words and the discovery answer are generated from or checked against it.
6. **Every call goes through scope and approval.** Read-only calls are free; a call that changes state, spends or listens is never `none`
   (a test of the table enforces it). Classes: `none`, `first-use`, `each-call`, `limit`. Money: a per-app, per-day limit in the **minor units of one currency, default 0**, checked before every spend and again when the person answers;
   only the person raises it (Info panel, chat tool that asks, or the card of `money.requestLimit`); never auto-approved; every change and every spend is audited; **no payment provider is called**.
7. **No new surface without its parts.** A namespace comes with the chat tool or page that the repo rules ask for (CHAT-REACHABILITY.md), its test fake, its emulator service and its typings, in the same change.
8. **Backends are separate from the bridge.** The bridge side (arguments, scope, approval, limits, errors, audit) is in `crates/iyakki-agent/src/store/sdkv2*.rs`; what a call does on the machine is a backend trait with a fake and a "missing" default
   that answers `Unavailable` with a `fix` line. Wiring a real backend never changes the interface.

## The list (SDK 1.2)

| Namespace | Since | Capability in app.json | Calls | Approval |
|---|---|---|---|---|
| `app`, `storage`, `ui`, `host`, `clipboard`, `notify`, `agent` | 1.0 | `storage`, `files`, `clipboard`, `notifications`, `agent` (`app` and `ui` need none) | as before | as before |
| `llm`, `models` | 1.1 | `llm` | `chat`, `stream`, `cancel`, `usage`; `list` | first-use |
| `speech` | 1.1 | `speech` (`listen`: `microphone`, 1.2) | `say`, `stop`; `listen`, `cancel` | first-use; listen: each call |
| `sdk` and `iyakki.version/has/capabilities/call` | 1.1 | none | `capabilities`, `has` | none |
| `markdown` | 1.2 | `markdown` | `render`, `view`, `openFile` | none; openFile through the dialog |
| `embed` | 1.2 | `embed` (kinds), `embed_hosts` | `mount`, `update`, `call`, `unmount`, `list` | first-use (browser) |
| `dash` | 1.2 | `dash` (sources) | `sources`, `query`, `panel.add` | first-use per source; each call to add |
| `flows` | 1.2 | `flows` (names) | `list`, `run`, `status`, `cancel` | each call to run or cancel |
| `k3s` | 1.2 | `k3s` (`read` or `manage`) | `status`, `get`, `logs`; `apply`, `restart` | each call to change |
| `media` | 1.2 | `media` | `library`, `play`, `pause`, `resume`, `stop`, `state`, `queue.add`, `queue.list` | first-use list; each call for a file |
| `audio` | 1.2 | `audio` | `play`, `pause`, `resume`, `stop`, `setVolume`, `outputs` | first-use |
| `money` | 1.2 | `money` (currency) | `limits`, `spend`, `requestLimit` | limit; card on every spend by default |

## How a change is made later

Add the namespace row and the capability row (append only), the arms in `sdkv2*.rs`, the typings block, the emulator service, the install-card words, the tests and the FEATURES row; run
`IYAKKI_CORE_SURFACE_WRITE=1 cargo test -p iyakki-core-api --test core_surface` (additions only) and `cargo test -p iyakki-core-api`. Bump `SDK_MINOR`. A test fails if a row is edited or a version is newer than `SDK_VERSION`.

## Not decided here

The backends: the shell viewer and embed painting, the media and audio player calls, the dashboard, flow and cluster adapters, the microphone recognition and any payment provider are separate rows (F-SDK-14 to F-SDK-16).
