# One core, many clients: design for the split and the WASM client

Status: design, 2026-11-11. Section 6 says what is built (steps 1 to 4); sections 5 to 7 are otherwise a design. It joins four earlier documents and does not replace them:
[UI-OS-SPLIT-PLAN.md](UI-OS-SPLIT-PLAN.md) (the layers), [CORE-ACCESS-MODEL.md](CORE-ACCESS-MODEL.md) (what an app may reach),
[CONNECTOR-APP-PLAN.md](CONNECTOR-APP-PLAN.md) (the small connector and sign-in) and [CORE-CONTRACT.md](CORE-CONTRACT.md) (the typed calls).

## 1. Goal

One IYAKKI **core** runs on a machine (a desktop, a cloud pod). Any number of **clients** show it and control it: the screen at the
machine, a window program on another computer, a browser tab, a phone or tablet app. A client holds no data and no devices. Everything it
wants from the instance goes through the published core interface, so the same approvals, roles and access log apply to every client.

## 2. Pieces

| Piece | What it is | Where it runs |
|---|---|---|
| **Core** | the services plus the broker (`iyakki-core-api`, `iyakki-core-local`, the agent services, the supervisor) | the instance |
| **Session endpoint** | one authenticated, encrypted connection per client (WebSocket over pinned HTTPS), carrying calls, events, pictures and input | the instance |
| **Client engine** | one program written once in Rust, built for WASM and for native: decodes pictures, draws, turns input into messages, runs tiles that are client-side WASM | every client |
| **Client host** | the thin per-platform shell around the engine: a browser page, a native window, a phone app, the framebuffer at the machine | every client |

The engine contains no operating-system code and no policy. Policy stays in the core: a client is an untrusted app with the capabilities the
person approved.

## 3. The four client forms

| Form | Host | Engine runs as | Notes |
|---|---|---|---|
| **Screen at the machine** | the existing framebuffer shell (`iyakki-shell`) | native, in process | stays as it is; later it becomes one more client of the same session interface (the split's end state) |
| **Window program** | `iyakki-connect` plus a window (installed web engine first, our own renderer later) | WASM in the web engine, or native | Windows, macOS, Linux; no browser tab |
| **Browser** | a page served by the instance or opened from a saved address | WASM | experimental, off by default, labelled so everywhere |
| **Phone and tablet app** | a small native app with the same engine | native library or WASM in the system web view | later; touch input and background limits are the work |

The same engine file serves the browser and the window program. The phone app reuses it, so a bug fixed once is fixed everywhere.

## 4. What the client draws

Two modes, chosen by the instance per tile, not by the person:

1. **Picture mode (first).** The instance renders the tile and sends changed regions (the binary tile stream that exists in `iyakki-remote`
   and `pixelcodec`). Input goes back as small messages. Simple, works for every tile today, costs bandwidth. The owner accepted pixel
   streaming for the first tier (tier plan, 2026-10).
2. **Display-list mode (next).** The instance sends a compact list of draw commands plus text and image resources; the engine draws them
   with the same font and drawing code. Much less bandwidth, sharp at any size, selectable text, accessible. Needs the drawing layer
   (`iyakki-gfx`, planned in the split) to be a crate with no shell dependency.
3. **Client-side tile (later).** A tile that is itself WASM runs inside the engine's sandbox and talks to the core through the same
   interface as any app. It is what makes a tile change deployable without relinking the shell (compile-speed plan).

Sound goes as audio chunks with a small jitter buffer; the camera and microphone of the client's own device are offered to the instance only
through a capability the person approves on the client.

## 5. Connecting

**Link.** The instance creates a one-time link (`https://host/connect#token`). Opening it in a browser, or in the window program, gives the
engine the token. The token is single use, expires in minutes, and is carried in the URL fragment so it never reaches a server log.

**Device identity.** On first use the engine creates an Ed25519 device key (kept in the browser's non-extractable key storage, or the
operating system's key store for the window and phone forms). The token enrols the public half with a name ("Maya's laptop"). After that
every connection is a signed challenge (`IYAKKI-SIG`, the scheme the vault and the connector already use); no password is stored on the
device, and revoking the device in Settings cuts it off at once. Mutual TLS is the alternative for the window program, where a client
certificate is easy to hold; the signed challenge is the one scheme that works in every form, so it is the default.

**Server identity.** The window and phone forms pin the instance's certificate on first use (as `iyakki-connect pin` does today). A browser
relies on the public certificate chain for a cloud domain and on a pin shown in the link for a machine on a local network.

**Person.** The device proves *which client*; the person still signs in with their password (or passkey later), so the vault unlocks and
roles apply. A device key alone never opens a vault.

**Not dev pair.** Developer-mode pairing stays what it is: a separate, approval-gated path for tile development and diagnostics, with its
own scopes. Client enrolment is a different record with different scopes.

## 6. The split behind it (what has to move)

The client needs the core to be reachable without the shell. In order:

1. **Catalogue and manifest** (core access model slice 1; under way).
2. **Broker with one capability and the access log** (slice 2): `camera.view` first, because the client form needs frames.
3. **Session endpoint**: one WebSocket per client carrying `hello` (version, capabilities), calls, events with sequence numbers and resume,
   picture and audio streams, and input. Built on the contract's three transports; this is the fourth, for clients.
4. **Engine, picture mode**: decode, draw, input; the browser page first (it exists in part as `/` in `iyakki-remote`; the engine replaces the
   hand-written script with our WASM build).
5. **Window program**: `iyakki-connect` grows a window host and the device key.
6. **Display-list mode** and the drawing crate.
7. **Phone app**.
8. **The screen at the machine as a client**: only when 4 to 6 are solid; until then the shell keeps its direct path, so nothing regresses.

### Status of the steps (2026-11-11)

| Step | State |
|---|---|
| 1. Catalogue and manifest | done (F-CORE-12) |
| 2. Broker with `camera.view` and the access log | done in tests (F-CORE-13) |
| 3. Session endpoint | **built, experimental, partial (F-CORE-14)**, details below |
| 4. Engine, picture mode (WASM) and the browser page | **built, experimental, partial (F-CLIENT-01)**, details below |
| 5. Window program | **built, experimental, Linux and Chrome only (F-CLIENT-02)**, details below |
| 6 to 8 | not started |

Step 3, done: protocol types in `iyakki-core-api::client` (hello with version range and capabilities, call, events with sequence numbers and
resume, input, the picture stream as a tagged binary message around the existing tile codec, stable error codes); the one-time link
(`iyakki-agent::clientlink`: single use, 10 minutes, stored as a hash, in the URL fragment) and device enrolment with an Ed25519 key, a name,
last seen and revoke; the WebSocket `/client/ws` in `iyakki-remote::clientsession` (a plain 404 while the setting is off), the `IYAKKI-SIG`
challenge per connection, then the person's password through the browser's gate (a device key alone gives no person, no vault, no picture, no
calls), calls through the broker as `CallerKind::Client` so the access log shows device name and person, one picture stream and input to the
person's screen (only keys, mouse, size; never the microphone or camera switches), events with resume, rate limits on failed tokens,
signatures and passwords, and a revoke or switching the setting off closing open sessions within a second. Settings, Remote access has the
section "Connect a client (EXPERIMENTAL)": switch, link as text and QR, devices, revoke. Tested over local plain sockets with fakes.

Step 3, not done (the media items moved to "The media half" below): a role table (today: the person must be an account of the machine); a way to grant a device
capabilities (grants are read from the approval book under `client/<device id>`, nothing writes them); the link carrying the certificate pin
for local-network machines; file exchange and a settings screen for what each device may do; sharing the picture pacing code with the
browser stream (it is copied); TLS, a real browser and hardware have not been tried; a second process spending one token at the same
instant is not excluded.

Step 4, done (2026-11-11, F-CLIENT-01): `crates/iyakki-client-engine` is the engine in picture mode. It is Rust, builds natively (tests) and for
`wasm32-unknown-unknown` with no wasm-bindgen: a module of about 0.4 MB with no imports, exports a handful of plain `extern "C"` functions
(`iy_alloc`, `iy_cmd`, `iy_on_text`, `iy_on_binary`, `iy_next_out`, `iy_fb_*`), and a hand-written glue file (`js/engine.js`) turns them into a small
class. The module is committed (`assets/iyakki-client-engine.wasm`, like `iyakki-js.wasm`); `dist/client-engine/build.sh` rebuilds it and
`build.sh --check` (a step of `alpha-check`) fails if the build gains an import or loses an export. Inside: the state machine of the slice 3 protocol
(`engine.rs`, no sockets and no clock, so it is tested against a fake instance), a wasm-safe copy of the tile decoder (`picture.rs`; PNG tiles are not
decoded because the engine always says it can inflate; a test makes both decoders agree on real encoder output), the `IYAKKI-SIG` answer (`sign.rs`,
golden vectors shared with the instance's own signing code and with the node test of the real module), and the input builders. The device key is a
WebCrypto non-extractable Ed25519 key in IndexedDB; if the browser has none, or the page is not a secure context (plain http on a local network), the
engine signs with a seed kept in IndexedDB, and the page says in words that this is weaker. The page lives in `iyakki-remote::clientpage` and is
served at `/connect` and `/connect/*` (a plain 404 while the setting is off): a strict content security policy (no inline script or style, own origin
only, WebAssembly only from the page's own file), the link read from the fragment and removed from the address bar and the history before anything else
runs, the password never logged and the field cleared on send, touch input (tap, long press, drag, two-finger scroll, pinch zoom, on-screen keyboard),
reconnect with back-off and event resume, and a plain message with an action for each of the 21 error codes. Tried: engine tests (flow, decoder
agreement, signing), node against the real module, and one end-to-end run in headless Chrome 157 against the slice 3 server (both key paths: add the
device, wrong password, sign in, the picture on the canvas, a key and a click arriving at the screen, a revoke closing the session, no request to any
other address, no console error). Not tried: TLS, Firefox, Safari, a real phone, hardware. Not built: file exchange, display-list mode (sound, clipboard, camera frames and the client's own devices are in "The media half" below; the window program is F-CLIENT-02). Known limit: the machine clamps its screen to at least 640x400, so a portrait
phone gets a small picture to pinch into; a phone-shaped screen size is a change on the instance, not in the engine.

Step 5, done (2026-11-11, F-CLIENT-02): `iyakki-connect window LINK|ADDRESS|NAME` (`crates/iyakki-connect`, `window.rs`, `windowcmd.rs`, `link.rs`,
`browser.rs`, `instances.rs`; CONNECTOR-APP-PLAN section 5). No engine is bundled and no GUI toolkit is used: the program (1) reads the one-time link
(`https://host/connect?pin=SHA256#token`; the instance now puts its certificate's fingerprint in the link, `clientlink::link_url_pinned`, used by Settings,
text and QR) or a saved instance name; (2) pins the instance certificate (the link's pin is checked against what the instance shows and remembered at once;
without one it asks, like `pin`; a changed pin, or a link that disagrees with the stored pin, stops with status 2 before anything is sent); (3) starts a
loopback server on `127.0.0.1` (random port, one-time launch address that sets a cookie, Host, Origin and CSRF checks as `serve` does) that serves the
instance's own `/connect` page and engine fetched over the pinned connection and relays `/client/ws` byte for byte (the instance sees its own Host and
Origin, no cookie), so the browser needs no public certificate authority; (4) opens the installed Chrome, Chromium or Edge with `--app=`, a private
temporary profile and no extensions (Firefox and Safari: an ordinary window, said plainly), found by well-known paths on Linux, macOS and Windows,
never downloaded; (5) keeps the **device key in the program**: one Ed25519 key per instance in the configuration folder (mode 600), and the page gets
`/connect/keys.js` from the program in place of the instance's, so `page.js` and the engine are not touched; the page asks `/local/sign`, and the
program signs only a client-session sign-in text for this device over a challenge it saw come from the instance on the socket it relays (once each,
within a minute of its clock); the device id the page reports is believed only if the instance sent it in `enrolled`. A revoked or unknown device is
forgotten (record and key). The browser's own WebCrypto key stays for plain-tab use. Tried: 15 + 8 connect tests (link parsing, pin decisions,
browser discovery with fake paths for three systems, the server's refusals, signing against the engine's golden vectors, relay and enrolment against a
fake instance over TLS) and one end-to-end run in headless Chrome 157 against the real session endpoint over TLS (`iyakki-remote/tests/window_browser.rs`):
pinned link, enrolment, wrong password, sign in, picture, key and click, no key in the browser's storage, a second run by name with the program
signing, a revoke forgotten by the program. Not tried: macOS, Windows, Edge, Firefox, Safari, a headed (visible) window, a cloud domain, hardware. Not built:
the operating system's key store (Keychain, DPAPI, Secret Service) instead of a key file; a Windows build (the crate uses Unix file modes); packaging
and signing of the program; the enrol form's sentence about the key still says "the browser will keep it" (it is in `page.js`, see KI-088).

### The media half (2026-11-11, F-CLIENT-03 to F-CLIENT-06, EXPERIMENTAL)

Built after the picture, following 7b. Protocol: capabilities `audio.out`, `clipboard.text`, `camera.frames`, `client.camera`, `client.microphone` in `hello`
(offered by the instance only when its media host is wired; a client that did not offer one gets a `capability_missing` media answer for it); binary streams
2 (sound to the client: `[seq u32][16-bit stereo PCM, 20 ms, 48 kHz]`), 3 (camera frame to the client), 4 and 5 (the client's microphone and camera to the
instance); messages `audio_open`/`audio_ok`/`audio_flush`/`audio_closed`, `clipboard_put`/`clipboard_get`/`clipboard_text`/`clipboard_ok`, `camera_open`/`camera_ok`/
`camera_closed`, `offer_open`/`offer_ok`/`offer_closed`, and one refusal message `media_err` with stable codes (the broker's denial codes, `in_use`,
`consent_required`, `too_big`, `empty`, `not_found`, `unsupported`, `capability_missing`, `bad_request`, `not_open`, `bad_data`). The clipboard messages may be up to
`MAX_CLIPBOARD_MESSAGE` long; every other text message is still held to 64 KB. Recorded in `tests/compat/alpha3/`.

* **Sound (rule 3)**: `iyakki-agent::clientmedia::AudioRoutes` is fed by the mixer's tee; one route and one side per session (default `client`, `both` by name); at
  most 100 ms queued, oldest dropped; flushed when the mixer closes or aborts its output, and the client is told; the machine's speakers are silenced only while a live
  route plays on the client alone and the route's person owns the machine's sound; a session that ends or stops pulling for 4 s gives them back. The page plays it
  with a hand-written audio worklet (100 ms bound, 40 ms prefill, silence when dry).
* **Clipboard (rule 5)**: text only, 64 KB; the press of the page's button is the per-use answer for `clipboard.client` (and only for it); the access log has device
  and person, the audit has the size.
* **Camera frames (rule 4)**: through the broker (`camera.view` with the client as caller, re-checked every second) and a shared lease; JPEG, 5 per second at most; the
  hub fans one capture out, so the machine's own view is untouched; an exclusive holder, the hub's Busy status or the person at the machine answers `in_use` with the
  holder's name.
* **The client's devices (rules 2, 4, 5, 7)**: `VirtualSources` (named `<device name> camera`, per tenant, one per device and kind, session scoped), `Leases`
  (one holder for exclusive devices, local wins), hard-ask capabilities `client.camera.offer` and `client.microphone.offer` (a card on the machine each time; the browser
  asks its own permission), `clientmedia::indicator()` for the shell's red band, a red band on the page. What a client sends is checked and dropped: nothing on the
  instance consumes a client source yet.
* **Wired into the shell (2026-11-11, F-CLIENT-07 to F-CLIENT-09)**: whose sound (`iyakki-shell::soundowner`: the person signed in last; nobody else's client is sent
  anything), one red sign (the REC sign also says when a client's camera or microphone streams to the machine), the machine's card (`clientmedia::DeviceCard`: a
  question in the shell's approval store for the person the device is signed in as, shown over the screen, Allow this time or Deny, denied after 60 s, never answered by
  developer-mode input), the camera hub (`HubFeed`) with a `camera_list` request for the page's picker, per-device permissions in Settings (`d` then a number) and by
  chat (`client_devices`, `client_device_permission`, `client_device_revoke`), the window program passing `media.js` and `audio-worklet.js`, and Cmd+V on macOS.
  "On" in the permissions means allowed to ask: the hard asks still ask at every use.
* **Not done**: local apps taking leases, a consumer for client sources (so "the instance views a client device" is only the streaming that the red sign reports), lip-sync of sound
  and picture, a card for a person other than the one the device is signed in as, one sound per person (the machine has one mixer), anything on a real phone or in Safari or Firefox,
  and every part of this wiring on real hardware (it ran in tests with fakes). A host without a camera feed (a cloud pod) answers `unsupported` for instance cameras and still serves
  the client's own devices; there is no separate cloud switch.

## 7. Security rules for all clients

* A client sees only what its signed-in person's role allows; the instance enforces it, never the client.
* Every call is audited with the device name and the person; the access log (CORE-ACCESS-MODEL 6b) shows what a client used.
* Clipboard, file exchange, camera and microphone each need an approval on first use, per device, and are listed in Settings, Devices.
* A lost device is revoked in one click; its tokens and key enrolment stop working immediately; open sessions close.
* No new third-party code in the engine beyond permissive-licensed libraries already reviewed; fonts and decoders are ours or already in the image.
* Experimental label, off by default, and a refusal test for every route before the setting can be turned on (as the connector plan requires).
* The one-time link and device key are never written to logs, screenshots or crash reports.

## 7b. Devices: the client's and the instance's never mix (owner, 2026-11-11)

Audio, microphone and camera exist on two sides: the **client device** (the browser, window program or phone running the client) and the **instance**
(a bare-metal machine, or a cloud pod that has no devices at all). Rules:

1. **Every device use names its side:** `client` or `instance`. There is no default that picks one silently. Screens say "On this computer (browser)" and
   "On the IYAKKI machine".
2. **A client's mic and camera reach the instance as their own virtual sources**, named after the device ("Maya's laptop camera"), session-scoped and tied to the
   person and tenant. They never replace or take over the instance's physical devices, vanish on disconnect, and another tenant never sees them.
3. **Sound has one side per session.** A remote client hears the sound on the client and the machine's speakers stay quiet; the person at the machine's screen
   hears it on the machine. Playing on both is an explicit choice. The client keeps the browser's echo cancellation on for its mic, so a browser in the same room
   as the machine does not feed back.
4. **Instance devices are leased.** The broker gives one lease per device: a camera may be shared for viewing (the hub already fans one capture out to viewers),
   a microphone or any exclusive device has one holder. A second request is refused ("in use by X") or waits; the person at the machine has priority.
5. **Hard asks ask every time**, on the client for its devices and on the machine for the instance's.
6. **A cloud instance advertises only client-device capabilities**, so nothing can conflict with hardware that is not there.
7. **Both ends show it:** a red indicator on the instance screen and on the client page whenever a client's mic or camera streams to the instance, or the instance
   uses the client's; every use goes to the access log with side, device and person.

Tests (fake devices): two holders of one exclusive device are refused and the local one wins; a lease is released on disconnect; audio routing never opens both
sinks by accident; several clients and tenants get separate virtual device names; a cloud profile offers no instance device. Status: built and tested with fakes, see "The media half" in section 6; rule 7's red band on the instance screen is drawn (the existing REC sign) from the client sources that are streaming; the page shows its own.

## 7c. Display-list mode (additive, 2026-11-19, D-CLIENT-PROTO-DISPLAYLIST)

A client that offers `display.list` in `hello` (the instance always offers it) may show an SDK app of the instance as a display list instead of
(beside) the machine's picture. Client to instance: `app_open {app, w, h}` (`app` is an id of `LIST_APPS`: `sample`, `markdown`; w and h are
the pane in logical units, 100 to 4096), `app_input {lines}` (the SDK's text input lines, one batch, answered by the next frame), `app_close`.
Instance to client (`iyakki_core_api::client::AppMsg`, a text message with its own tags): `app_list {app, text}` (a frame, at most 512 KB),
`app_closed {app, reason}` (`closed`, `left`, `too_big`), `app_err {code, message}` (`unsupported`, `not_found`, `denied`, `bad_request`,
`not_open`, `too_big`, `failed`). One app per session; the role, the cloud mode's hidden apps and the audit log apply as when the app is opened on
the machine; the input lines are never audited. The existing messages and the picture stream are unchanged; a client that does not offer the
capability agrees on exactly what it did before. The page draws the list with the engine's `ui_*` commands (`js/uidisplay.js`) in a window or,
on a narrow or upright touch page, filling the page.

## 7d. File transfer through the page (additive, 2026-11-19, F-CLIENT-01)

Capability `files.transfer` (offered in `hello` by an instance whose media half is wired; never needed). Client to instance: `file_list {dir}`, `file_get {id, path}`,
`file_put {id, path, size}`, `file_end {id}`, `file_cancel {id}`; the bytes travel as binary chunks of at most 32 KB, `[id: u32 big endian][bytes]`, stream 6 to the
client and stream 7 from it. Instance to client (`iyakki_core_api::client::FileMsg`, texts with `file_` tags that the engine passes on and the page reads):
`file_list`, `file_get_ok`, `file_done`, `file_put_ok`, `file_saved`, `file_err {id, code, message}` (codes: `unsupported`, `capability_missing`, `role_denied`,
the broker's denial codes such as `not_granted` and `card_declined`, `bad_request`, `too_big`, `not_found`, `busy`, `failed`). Everything goes through the core's files
facet (the home folder only, no links, no hidden names, 8 MB at most, `Files::read_bytes` and the change `SaveBytes` through the change book). The device must have
been given `files.client` (Settings or the chat tool `client_device_permission`, role `full`); the machine then asks on its own screen once for browsing and for every
file that goes either way, and nothing is read or saved before the yes. One upload and one download at a time; the audit lines carry sizes, never names.

## 7e. Sound for each person (2026-11-19, F-CLIENT-03)

By default one sound per machine goes to the client of the person the machine's sound belongs to (decision D-CLIENT-03). With the setting `sound-per-person` (the
chat tool `client_sound_mode`, which asks every time) the mixer builds one bus per owner of a sound (`SourceWriter::set_owner`; the embedded player and the sound of
an app set it), and each client hears only its own person's bus; the machine's own sounds (no owner) go to the person the machine's sound belongs to. Opus is not
written: an in-house encoder (the CELT and SILK analysis, the range coder, the pulse search) is several thousand lines and has no encoder in the repository to
check it against; PCM stays the only coding (about 190 KB/s a client).

## 8. Open questions for the owner

1. Window program: installed web engine first (small, as the connector decision says), or our own renderer from the start?
2. Phone app: one app for Android and iOS built from the same engine, or the browser form only for the first beta?
3. Offline: should a client keep the last picture and queue input when the instance is unreachable, or just show "disconnected"?
4. Cloud instances: is the link created from the tenant's admin page, or by each person for their own devices?
5. Does display-list mode wait for the compile-speed split of the drawing code, or is picture mode enough for beta?

## 9. First deliverables in alpha.3

* Slice 1 and 2 of the access model (the catalogue, the broker, the access log).
* The session endpoint with `hello`, one picture stream and input, tested against the fake core with a socket pair.
* The engine in picture mode in a browser, behind the experimental setting, with the one-time link and the device enrolment.
* Not in alpha.3: display-list mode, the phone app, the screen as a client.
