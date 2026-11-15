# Connector app: plan

Status: decided 2026-11-03 (owner): the client is a small program plus the browser already installed on the machine; it must be secure and
as fast as the IYAKKI desktop. Built 2026-11-04 (steps 1 to 5, rows F-CONN-01 to F-CONN-04): `iyakki-link`, `iyakki-connect probe/pin/login/serve/--share/key`, 2.2 MiB (gnu) and 2.35 MiB (musl); tested with fakes and headless Chrome against a fake tenant, not against a real desktop. Read with `WEB-UI-PLAN.md` and `CLOUD-MULTITENANT-PLAN.md` (section 3.4).

## 1. Shape

* One small static program `iyakki-connect` (estimate 2 to 4 MB, not measured). The client (`cloudclient.rs`) and the picture decoder
  (`pixelcodec.rs`) move from the agent crate into a small crate so the program does not pull in the agent.
* Modes: `probe` / `pin` (show and store the certificate fingerprint, trust on first use), `serve` (a page on the loopback address that shows
  the remote desktop in the installed browser), plain actions (`ls`, `get`, `put`, sign-in, tenants, flows), `--share DIR` for file exchange.
  Built 2026-11-04: `iyakki-connect tenants HOST USER` lists the tenants the account may use (`GET /api/v1/tenants`); a switch command and `ls` are not built.
* No browser engine and no GUI toolkit: the installed browser draws the page; clipboard, camera, microphone and audio use its own APIs.

## 2. Secure

* The page server listens on `127.0.0.1` only, on a random port, with a one-time token per launch; requests without it are refused.
* `Host` and `Origin` are checked on every request and on the WebSocket (no DNS rebinding, no other site can talk to it).
* Strict Content-Security-Policy, no external requests, no inline scripts from data; the page carries no secrets.
* Passwords and keys stay in the program; the browser never sees them. The remote certificate is pinned (SHA-256); a changed one is a hard stop.
* The shared folder is one folder chosen at start, read-only unless `--write`; no `..`, absolute paths or symlinks that leave it; asks before overwriting.
* The remote side still decides everything (roles, namespaces, media policy); the program only carries the session.

## 3. Fast

* The program relays the existing binary tile stream as it is (no re-encoding), over one WebSocket on the loopback address.
* The page decodes in a worker on an OffscreenCanvas (the decoder the web client already has), draws only changed tiles, input events are
  small binary messages; one connection, no polling.
* Audio and camera frames are sent as raw chunks with a small jitter buffer; nothing is converted unless the remote asks for it.
* Measure before claiming: frame time and input-to-picture delay against a local desktop, on loopback and over a real network.

## 3a. Sign-in (decided 2026-11-03: Ed25519)

* Transport is HTTPS with the pinned certificate only.
* People: account name and password give an opaque server-side session token (expires, revoked at once when a role is removed).
* Devices, scripts, the connector CLI: an Ed25519 key, enrolled once, then every request or sign-in is signed with the `IYAKKI-SIG` scheme
  the vault already uses (`cloudkeys.rs`); revoking the key cuts the device off. No password stored on the machine.
* An existing `ssh-ed25519` public key can be imported as that key and signed with `ssh-agent`; this is not an SSH server in the pod.
  RSA and other key types are not supported.
* JWT is not used. Revisit only if a third party must verify a sign-in offline (then a short-lived signed token on top).

## 4. Order

1. Move the client and decoder into the small crate (agent re-exports). 2. `probe`/`pin` and plain actions. 3. `serve` with the page and the
bridge, with the security checks above and tests for each refusal. 4. `--share` once the storage routes exist. 5. Media bridge from the page.

## 5. The window program (`iyakki-connect window`, EXPERIMENTAL; built 2026-11-11)

Status: built for Linux and tried with Chrome 157 only (see KI-088). Detail and security rules: `CLIENT-SPLIT-DESIGN.md` step 5 and section 5. Row F-CLIENT-02.

* `iyakki-connect window LINK|ADDRESS|NAME` shows the client page of an IYAKKI instance (a bare-metal machine or a cloud pod) in the installed
  browser as an application window (no tabs, no address bar). There is no bundled engine and no GUI toolkit: the page and the engine are the
  instance's own (`/connect`, `/connect/engine.wasm`), fetched over a pinned TLS connection, so the window always matches the instance's version.
* The link is the one from Settings, Remote access, Connect a client: `https://host/connect?pin=SHA256#token`. The pin in the link (added by the
  instance) lets the first connection pin the certificate with no question; without it the program asks like `pin` does. A pin that changed is a hard
  stop (exit status 2), a link that disagrees with a stored pin or with the certificate shown is a hard stop, and the pin is never replaced silently.
* A small server on `127.0.0.1` (random port; Host, Origin and a cookie from a one-time launch address; CSRF value for changes) serves the page,
  replaces `/connect/keys.js` and relays the session WebSocket to the instance over the pinned connection. The browser needs no public certificate
  authority and never sees the key. The instance's one-time token is never in an argument list: the browser opens a start page in its private profile
  folder, which sends it to the launch address, which redirects to `/connect#token`.
* The browser is found by well-known paths (Chrome, Chromium, Edge, then Firefox, then Safari; override with `--browser PATH` or
  `IYAKKI_CONNECT_BROWSER`), started with `--app=`, a temporary profile (mode 700, deleted afterwards), no extensions, no sync, and its password
  manager switched off in that profile. Firefox and Safari have no application mode: they get an ordinary window and the program says so. A
  browser is never downloaded or bundled.
* The device key lives in the program, one Ed25519 key per instance (`<config>/device-keys/*.key`, mode 600; the list is `instances.json`). The page
  asks `POST /local/sign`; the program signs only a sign-in text for the client session for this device, within a minute of its clock, over a
  challenge it saw come from the instance on the relayed socket (each challenge once). A plain browser tab keeps its own WebCrypto key as before.
* Commands: `window`, `devices` (list), `devices forget NAME`; `pin`, `unpin`, `pins` are shared. `window ... --no-browser` prints the local
  address instead of opening a browser; `--browser-arg ARG` (repeatable) appends an argument to the browser's command line.
* Later: the operating system's key store instead of a key file (Keychain, DPAPI, Secret Service; the file mode is the protection until then); a
  Windows build of the program (the crate uses Unix file modes today); signed and packaged installers; Edge, Firefox and Safari tried for real.
