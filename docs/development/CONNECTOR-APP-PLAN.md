# Connector app: plan

Status: decided 2026-11-03 (owner): the client is a small program plus the browser already installed on the machine; it must be secure and
as fast as the IYAKKI desktop. Not built yet. Read with `WEB-UI-PLAN.md` and `CLOUD-MULTITENANT-PLAN.md` (section 3.4).

## 1. Shape

* One small static program `iyakki-connect` (estimate 2 to 4 MB, not measured). The client (`cloudclient.rs`) and the picture decoder
  (`pixelcodec.rs`) move from the agent crate into a small crate so the program does not pull in the agent.
* Modes: `probe` / `pin` (show and store the certificate fingerprint, trust on first use), `serve` (a page on the loopback address that shows
  the remote desktop in the installed browser), plain actions (`ls`, `get`, `put`, sign-in, tenants, flows), `--share DIR` for file exchange.
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
