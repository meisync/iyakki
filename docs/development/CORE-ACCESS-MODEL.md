# Core access model: apps reach devices and the internet only through `iyakki-core`

Status: design, 2026-11-10. Written for the owner and for whoever builds the first slice. Nothing here is built yet; section 9 says what is
open. It records the owner's decisions of 2026-11-10 and what follows from them.

## 1. Decisions

1. `iyakki-core` is a **shared module** (a shared library) that apps use at run time, not code linked into each program.
2. **Every app** reaches devices and the internet through it. No app opens a device node or a network socket itself.
3. **At install** the person is asked to approve the devices (capabilities) the app wants.
4. The **default (built-in) apps** hold their access by default, without an install card.
5. Apps can live in their own repositories (the SRE pilot and the browser add-on already do) and depend only on the public interface.

## 2. What exists today

* The permission unit is the role policy (`iyakki_sec::policy::App`, plus file, network and CLI limits): it says which person may use which app.
* Store apps declare a `permissions` list in `app.json`; `iyakki dev app permissions` shows it. It is not enforced as device access.
* Tiles that run behind the tile interface (`iyakki-tile-api`, `iyakki-tile-host`: calendar, cameras, remote files, usage) already see only
  narrow service traits, never the device. The camera tile is the closest thing to this model.
* Kernel access is kept in a few shared modules, and two gates only ever shrink: `kernel_single_reader` and `dependency_ratchet`.
* The WASM tile sandbox gives a tile no host access except what the host passes in.

## 3. Parts

| Part | Where it runs | Trust |
|---|---|---|
| **Core** (`libiyakki_core.so`): the capability functions and the broker | the system process | system |
| **Broker**: decides and audits every call; talks to devices and the network | inside the core | system |
| **SDK client**: the thin shared library an app links; it speaks to the broker | the app's own process | none (it is only a convenience) |
| **Built-in apps** (the default apps of the OS) | in the shell's process, calling the core directly | trusted, by a signed table |
| **Store and third-party apps** | their own sandboxed process: own user, no device nodes, no network, no raw file access outside their folder | untrusted |
| **WASM tiles** | the tile sandbox; the same functions are given to them as host imports | untrusted |

The point that decides the rest: a check that runs inside the app's own process only stops honest apps, because an app that has the library in its
own memory can skip it and call the kernel itself. So for untrusted apps the enforcement is outside: **the sandbox removes the direct route, and the
broker is the only way to a device**. The shared library is the comfortable way to reach the broker, not the guard.

## 4. Capabilities (catalogue, version 0)

Every capability has a name, parameters, a default for built-in apps, and whether using it is a **hard ask** (a card with Allow on the screen for every use,
whatever was approved at install, as today for recording, listening and the SFTP server).

| Capability | What it gives | Parameters | Built-in default | Hard ask |
|---|---|---|---|---|
| `camera.view` | pictures from a camera | which camera | yes | no |
| `camera.record` | recording | which camera | may ask | **yes** |
| `microphone.listen` | sound in | which microphone | may ask | **yes** |
| `speaker.play` | sound out | none | yes | no |
| `bluetooth.use` | scan, pair, connect | none | settings app only | no |
| `usb.use` | list and open a USB device | vendor and product | no | no |
| `files.read`, `files.write` | files by scope | folder | per role policy | no |
| `net.connect` | network | host and port | per role policy | no |
| `net.listen` | accept connections | port | no | **yes** |
| `screen.capture` | a picture of the screen | none | no | **yes** |
| `input.inject` | keys and pointer | none | no | **yes** |
| `notify` | inbox notes | none | yes | no |
| `notify.receive` | notes that match the app's filter | filter | yes | no |
| `schedule.use` | triggers at a set time | schedule | yes | no |

"May ask" means a built-in app may request the capability, and the person still answers each time.

## 5. Manifest and approval

* An app lists its capabilities in `app.json` (`capabilities: [{ "name": "camera.view", "why": "show the live view" }]`); the reason text is shown to the person.
* At install the Store shows a card: the app, its publisher, its capabilities with reasons, Approve or Cancel. The answer is stored per account and
  device, with an audit entry (who, what, when, app version and hash).
* An update that asks for more asks again; an update that asks for the same or less does not.
* An administrator can revoke a capability at any time (Settings, Apps); the broker applies it at the next call.
* The role policy still applies on top: a capability the person's role forbids is refused whatever the app holds.

## 6. The interface

* A C interface (`extern "C"` functions and plain structs), because Rust has no stable binary interface and apps may be written in other languages.
  The header is generated by our own small script, not a third-party generator.
* `iyakki_core_abi_version()` returns a major, minor and patch number; apps check it at start. During the alpha the interface can change, and it is
  frozen at beta with versioned symbols.
* Handles, not pointers into the core: `iyakki_open(capability, params) -> handle`, `iyakki_read(handle, buf, len)`, `iyakki_close(handle)`. Errors are numbers
  with a text function (`iyakki_error_text`). No Rust type crosses the boundary.
* An app that holds no approval gets `IYAKKI_DENIED` and the attempt is audited.
* Built-in apps call the same functions through a direct function table (no socket). Store apps go through the broker socket; the broker finds out who is
  calling from the connection's credentials, never from what the app says about itself.

## 6b. Access log: network connections and device use, by domain, per app

Decision of 2026-11-10: the system keeps a log of network connections and device use, by domain, per app. The broker is the one place every call passes, so it is
where the log is written; nothing else writes it.

* **What is recorded.** For each network connection: the app, the account, the host name the app asked for (or the address, when it asked by address), the port,
  allowed or denied, the time, and the bytes sent and received when the connection closes. For each device use: the app, the account, the device (camera,
  microphone, speaker, Bluetooth, USB, screen capture, input), which unit, the time, how long, allowed or denied, and for a hard ask whether the person pressed Allow.
* **What is not recorded.** No content, no paths, no query strings, no headers, no sound or pictures. A domain is kept, a full address is not.
* **Two tables.** A ring of the latest events (default: the last 5 000 per account) and daily totals by (day, account, app, capability, domain or device) with
  counts and bytes. The ring answers "what just happened"; the totals answer "what did this app do this week" and are kept 30 days by default (a setting).
* **Who sees it.** A person sees the apps they use; an administrator sees every account. The log is not shared outside the machine, and an export is a file the
  person saves.
* **Where it is shown.** (1) Settings, Apps, one page per app: the capabilities it holds (with revoke) beside its access log for the last day, week and month,
  by domain and by device. (2) An **Access** tab in the USAGE tile: the overview, app by domain and app by device, with a range (today, 7, 30 days).
  (3) The assistant has a read tool, `access_log_query`, to answer "which apps contacted which domains today". The decisions themselves (approvals, revokes,
  denials) stay in the tamper-evident audit chain under Users, Audit log; this log is for volume and is not part of that chain.
* **Where it starts.** Before the broker exists, the log is written at the places that already see the calls: the shared HTTP client (`iyakki-proxy`), the firewall
  rules (`iyakki-fw`) and the camera and sound services. Each entry carries the app id of the caller; calls from code that has no app id are logged as `system`.
* **Cost and privacy.** The write is one small append per connection or device use, off the hot path. Retention limits keep it small; clearing it per app is
  allowed and is itself audited.

## 6c. Published interface and two-way delivery

Decisions of 2026-11-10:

* **The interface is published.** The `iyakki-core` C interface (header, capability catalogue, error numbers, event format, version rules) is a public document and the
  only supported way for an app to reach a device, the network or the system. A generated reference page and a conformance test (a sample app and a fake core) ship
  with it, so an app in its own repository can check itself without an image. Until the beta tag it is marked unstable (section 9).
* **Core to app: notifications with filters.** An app subscribes with a filter (source, kind, tag, minimum importance, text match). The core delivers only the
  notes that match; an app that holds no subscription sees nothing. The subscription is a capability (`notify.receive`) shown at install, like the others.
* **Core to app: scheduled triggers.** An app registers a schedule (once, interval, or a calendar rule) and the core delivers a trigger event at that time; the
  core wakes the app if it is not running. A schedule is a capability (`schedule.use`), limited per app (count and shortest interval) and listed in Settings, Apps.
* **Same channel both ways.** Apps call the core (open, read, close); the core calls the app through events on the same handle or broker connection
  (`iyakki_poll_event` for built-in apps, a message on the broker socket for store apps). Every delivery is logged in the access log (section 6b): app, kind,
  allowed or dropped, time, never the content.
* **Limits.** A filter cannot read other apps' notes; delivery is rate limited per app; a trigger never runs an app that the person's role does not allow.

## 7. Packaging

* `iyakki-core` becomes `crate-type = ["cdylib", "rlib"]`; the boot and supervisor program (today's `iyakki-core` binary) becomes a thin main that uses it.
* The image today links statically against musl. A shared library needs dynamic musl linking and the dynamic loader on the image; Buildroot carries musl, so this
  is doable but changes how the image is built and how the Secure Boot chain treats one more file. It is its own step and has its own test: boot in QEMU.
* One copy of the core is in memory for all built-in apps; an update of the core does not rebuild the apps, as long as the interface version matches.

## 8. Slices

1. **Catalogue and manifest.** The capability names, the `app.json` field, the approval record and its audit entry, and the install card in the Store. Tests with fakes.
2. **The broker and one capability, with the access log.** `camera.view` through the broker and through the direct table; the camera tile as its first user; a denied call is refused and audited; the first access-log table and the Settings, Apps page that shows it (section 6b).
3. **The ratchet.** A gate that fails when app code (outside the core and the platform modules) calls a device node or opens a socket; it only shrinks, like the others.
4. **The shared library.** `cdylib`, the header script, `iyakki_core_abi_version`, the loader on the image, a QEMU boot test.
5. **The sandbox for store apps** and a sample app in its own repository (the SRE pilot is the natural first: it needs no device at all).
6. **The remaining capabilities**, one per change, with the hard asks keeping their per-use card.

## 9. Open questions

* Are third-party apps native processes, WASM modules, or both? The sandbox in slice 5 differs.
* Is in-process loading of a plug-in into the shell ever allowed for a store app? The proposal: no, built-in apps only.
* Network grants: per host, per app, or per host and app? The proposal: per app and host, shown at install.
* How long may the interface stay unstable? The proposal: until the beta tag.
* Should the access log be on by default for every account, or only for administrators until a person opts in? The proposal: on for everyone, stored on the machine only.
* Where does the broker run when the OS is only a cloud profile (no devices)? The proposal: the same broker with fewer capabilities.
