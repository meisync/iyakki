# Developer mode: install and run apps from your own computer

**For:** people who build tile apps, skills, MCP servers or extensions for IYAKKI and want to try them on a real device
without publishing to a store first, and the administrators who allow that. **You will get:** setup, pairing, every `iyakki dev`
command, the IDE target, and the security model. In short: it is `adb install` for IYAKKI.

Code: `crates/iyakki-agent/src/devmode/` (`mod.rs` state, keys, pairing and logs; `auth.rs` the two proofs; `ops.rs` install and
remove through the STORE; `client.rs` and `cli.rs` the computer's side), `crates/iyakki-remote/src/devserver.rs` (the listener),
`crates/iyakki-shell/src/apps/settings_developer.rs` (Settings), `crates/iyakki-shell/src/apps/ide_tile.rs` (the IDE target). Tests:
`crates/iyakki-agent/src/devmode/tests.rs`, `crates/iyakki-remote/tests/devmode.rs`, `settings_developer_tests.rs`, `ide_tile_tests.rs`.

## 1. Switch it on (administrators, on the device)

Settings → **Developer mode** (System group; administrators only):

| Key | What it does |
|---|---|
| `e` | Switch on / off. Off by default. While off, nothing listens and every request is refused. |
| `h` | The automatic switch-off: 4 h (default), 1 h, 8 h, 24 h, 72 h, or never. Changing it while on restarts the countdown. |
| `p` | Pair a computer: shows a one-time code (10 characters, five minutes, five tries) and the device's certificate fingerprint. |
| `c` | Cancel the pairing code. |
| Up/Down, `x` | Choose a paired computer, revoke it (its next request is refused). |
| `l` | The activity log: every action, who did it (computer and account), when, and whether it was allowed. |

While developer mode is on the device listens on **TCP 7979** (TLS) on all its addresses; the page shows the address to type.
The listener starts within two seconds of switching on and stops within two seconds of switching off (by hand or by the timer).

For the IYAKKI menu and the top bar (another part of the shell): `iyakki_agent::devmode::set_enabled(data_dir, policy, user, on, hours)`
switches it (administrators only, logged), `devmode::is_on(data_dir)` says whether it is on, and `devmode::indicator(data_dir)` returns
`Some("DEV")` (or `"DEV · pairing"`) while it is on, for a sign in the top bar.

## 2. Pair your computer (once)

Install the `iyakki` program on your computer (the same binary as on the device: `cargo build --release -p iyakki-core`, or the
release tarball). Then, with a pairing code on the device's screen:

```text
$ iyakki dev pair 192.168.1.20
Device iyakki-box (IYAKKI 0.1.0-alpha.1) at 192.168.1.20:7979
Its certificate fingerprint: 3F:A2:...:91
Check that the device's screen shows the same fingerprint.
Pairing code shown on the device: K7QFM-2XPRT
Paired: this computer is "laptop" on 192.168.1.20 and acts as the account root. Saved as device "iyakki-box" in ~/.config/iyakki/dev.
```

* `--code CODE` gives the code without the question; `--name NAME` names this computer (default: its host name); `--as NAME` names the
  device on this computer; `--fingerprint FP` refuses to pair unless the device shows exactly that certificate.
* The computer's key is made the first time, in `~/.config/iyakki/dev/id.key` (mode 0600; `$IYAKKI_DEV_HOME` or `--home DIR` move it).
  `devices.json` beside it keeps each paired device: its address, port and the pinned certificate fingerprint.
* The new key acts as the **account of the administrator who opened the pairing**. (`DevMode::start_pairing_as` pairs it for another
  account; the Settings page uses the administrator's own account.)

## 3. Commands

Every command takes `--device NAME` (default: the first paired device, or the one set with `devices --default`) and `--home DIR`.

| Command | What it does |
|---|---|
| `iyakki dev devices [--default NAME \| --forget NAME]` | The paired devices, and this computer's public key. |
| `iyakki dev whoami` | The device, this computer's name there, the account and role it acts with, and whether developer mode is on. |
| `iyakki dev install <pkg.iyp \| folder> [--shared] [--open]` | Install, upgrade or reinstall. A folder is packed first: a project with `iyakki-app.json` (any kind `iyakki pkg pack` makes: tile, skill, MCP server, WASM tool, extension, template, theme, agent preset; there is no package kind for FLOWS workflows yet, so a workflow travels as an `extension` or `template` package that nothing reads automatically) or a bare tile app folder (`app.json` + `index.html`). Folder builds are signed with this computer's key. `--shared` installs for everyone (needs a role that may). |
| `iyakki dev uninstall <id>` | Remove something developer mode installed (`kind:name` or `name`). Its saved data is kept. Things installed from a store are not touched (remove them in the STORE). |
| `iyakki dev list [--all]` | What developer mode installed that this account can see; `--all` lists everything installed through the STORE that it can see. |
| `iyakki dev logs <id> [--follow]` | The app's log on the device: each install, reinstall and removal, and for a tile app the page's console (`console.log`, errors) while it runs on the screen. `--follow` keeps reading. |
| `iyakki dev run <id>` (or `open`) | Open the tile app on the device's screen (for the person signed in, if they have it; queued until someone signs in). |
| `iyakki dev push <folder> [--watch] [--open]` | Pack and install the folder; with `--watch`, again after every change (live reload: an open copy of the app reloads by itself on the device). A build that fails is reported and the watch goes on. |
| `iyakki dev screenshot [-o FILE.png]` | A PNG of whatever the device's screen shows. |

Versions: the same version can be installed again (a rebuild) as often as you like; a higher version is an upgrade; a lower version is
refused, as in the STORE (the high-water rule). Raise the version in `iyakki-app.json`/`app.json` when you mean a new release.

**From the device itself.** The IDE's **Publish** dialog lists **This device (developer mode)** while developer mode is on, and
**Paired device NAME (developer mode)** for each device the signed-in account paired with. Both install through the same code as
`iyakki dev install` (the first directly, the second over the network), and a tile app opens straight away. The IDE keeps each account's
key and paired devices in `<data>/devmode/client/<account>/`; to pair from the device, run
`iyakki dev pair ADDRESS --home /var/lib/iyakki/devmode/client/<account>` in its terminal.

## 4. What is installed, and how it is labelled

There is no second installer. Every package goes through the STORE's own review and install (`Store::review` then `Store::install`)
with the source `developer:<computer>`:

* The hash and any signature are checked, the permissions are worked out from the payload, the tile app's lint and the version rules
  apply, and the account's role decides: installing for yourself needs the STORE and `skills`, for everyone `manage_skills`, a tool (MCP)
  server an administrator and is always for everyone. Unsigned packages are accepted from a paired computer (the pairing is the
  administrator's consent), but they are never shown as anything other than Developer.
* The trust level is **Developer (local)**, whatever key signed it: an amber shield in the STORE, "Developer (local)" and a DEV ribbon on
  the launcher tile, "DEVELOPER (LOCAL)" in the app's bar and info panel. It is **never promoted**: the STORE's update check says "Installed
  from a paired developer computer: it is never updated from a store", and a store version of the same app is refused until the developer
  build is removed (the publisher rule: a different signer).
* **Refused:** driver packs and kernel modules (`extension` with `ext_kind: driver`; use the STORE's Drivers page, where the driver-pack
  rules apply), command-line tools (the CLI tile installs those) and `app` packages (not supported yet). A revoked package or key is
  refused as everywhere.

## 5. Security model

| Question | Answer |
|---|---|
| Who can turn it on? | Administrators only (`manage_settings`), in Settings or through `devmode::set_enabled`. Off by default; optional automatic switch-off (default 4 h). Every request checks that it is on. |
| What listens? | One TLS listener (port 7979 by default), only while developer mode is on. The certificate is the web front end's (`<data>/tls`, self-signed, made once). No plain HTTP, no CORS, at most 16 connections, a body limit of 12 MiB + 64 KiB, two-minute request limit. |
| How does a computer get in? | Pairing: the administrator opens a code on the device; the computer sends its ed25519 public key and `HMAC-SHA256(argon2id(code, salt = the certificate fingerprint), "IYAKKI-DEV-PAIR v1" + fingerprint + key + name)`. The fingerprint is the one the computer saw in the TLS handshake, so a machine in the middle (which has to present its own certificate) produces a proof the device refuses; argon2id makes guessing the code from a captured proof slow; five wrong tries or five minutes close the code. The device answers with a second HMAC so the computer knows it reached the device that shows the code. The computer pins the certificate from then on. Both sides print the fingerprint so a person can compare. |
| How is each request authenticated? | An ed25519 signature (from `ed25519-dalek`, already used by the STORE) over `"IYAKKI-DEV-REQ v1"`, the method, path, sorted query, time, a random nonce, the SHA-256 of the body and the device's certificate fingerprint. The device checks the key is paired and not revoked, the time is within two minutes, the nonce is new (replays are refused) and the signature is valid (`verify_strict`). Refusals are logged and rate-limited per address. |
| What can a paired computer do? | Only the routes above, with the role of its account: install / upgrade / remove what developer mode installed, list, read the logs of the account's own apps, open its tile apps, take a screenshot. It cannot run commands, read files, reach the vault, change settings, install drivers, or remove store installs. Deleting the account, revoking the key or switching developer mode off ends its access at once. |
| What is recorded? | `<data>/devmode/activity.log` (time, computer, account, action, detail, allowed or refused; capped, rotated once), shown in Settings, plus the system audit log (`devmode.pair`, `devmode.install`, `devmode.uninstall`, `devmode.open`, `devmode.screenshot`, `devmode.on`, `devmode.off`, `devmode.revoke`). No code, key or secret is ever written to either. |
| Cryptography | Nothing new: TLS from rustls (the web front end's), ed25519, SHA-256, HMAC-SHA256 (RFC 2104, checked against the RFC 4231 vectors) and argon2id, all crates already in the tree. |

Files on the device: `<data>/devmode/state.json` (on/off, by whom, until, port), `keys.json` (paired computers: name, public key, account,
when, by whom, last use; mode 0600), `pairing.json` (the open code, deleted when used, cancelled or expired), `activity.log`,
`logs/<app>.log` (256 KiB, rotated once).

## 6. Limits (alpha)

* **Discovery:** there is no LAN discovery for developer mode yet; type the device's address (shown on the Settings page). NET SCAN finds
  devices on the network if you need the address.
* **The desktop window** (`iyakki-desktop`) does not start the listener; developer mode works on the booted image, `iyakki shell`,
  `iyakki web` and `iyakki serve`. Only tested over loopback in the test suite: no real network, device or second machine was used.
* **Logs** of tile apps are the page's console as the device's shell sees it while the app is open; MCP servers and skills have only
  their install and removal lines.
* The pairing code is shown on the device's screen only; the IDE cannot pair to another device by itself yet (use the terminal command above).
