# Core access model: apps reach devices and the internet only through `iyakki-core`

Status: design, 2026-11-10; slices 1 and 2 built 2026-11-11 (section 8). Written for the owner and for whoever builds the next slice. Section 9 says what is
open. It records the owner's decisions of 2026-11-10 and what follows from them.

Compatibility rules for everything in this document (versions, additive changes, deprecation, rollback): [CORE-COMPATIBILITY-DESIGN.md](CORE-COMPATIBILITY-DESIGN.md).

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

## 6d. App bundles: one package, several kinds of content (owner, 2026-11-11)

An IYAKKI app bundle can carry more than the app itself: **skills, workflows (flows), tools, MCP servers, agents, templates and themes** next to the tile or app.
Today a package holds exactly one kind (docs/STORE.md); flows are not a package kind at all. The bundle changes that without changing the rules:

* **One manifest, one signature, one install card.** `app.json` gains a `contents` list: each member names its kind, its file and its own `capabilities`.
  The card shows everything the bundle brings and everything it asks for, grouped by member, so a person approves a bundle once, not eleven things.
* **The bundle is the unit of trust.** It is signed and versioned as a whole; the broker's approval record is per bundle and member (`<bundle>/<member>`), so a skill in
  the bundle never holds more than its own declared capabilities, and an app in the bundle cannot lend its capabilities to a skill.
* **Atomic install, update and removal.** All members install, or none; an update shows only members or capabilities that are new; removing the bundle removes
  every member it installed and their approvals (shared data the person created stays, as today).
* **Flows get a kind.** A workflow member is a flow definition the Flows tile already runs; it runs as the person who installed it, with the capabilities the
  bundle declared for it, and hard asks keep their per-use card.
* **Where a member lives is unchanged:** a skill goes through the skills store, a tool or MCP server through its tile, a tile into the custom-apps folder. Only the
  way they arrive is new.
* **Cloud and the clients:** the same bundle installs on a bare-metal instance and on a cloud instance (a member that needs a device the instance does not have
  is shown as "needs a client device" or "not available here" on the card, see CLIENT-SPLIT-DESIGN.md, section on client devices).

Open: the size cap of a bundle (today a tile bundle is 1.5 MB), whether members may be updated separately by their own authors, and whether one bundle may depend on another.

**Status (2026-11-11, tested with fakes and temporary folders, the card never seen on a screen):**
* **Built.** `app.json` takes `manifest_version`, `requires` (`core_interface` range such as `>=0.3 <1`, `capabilities_needed`, `capabilities_optional`) and `contents`
  (`iyakki-core-api::bundle`, pure serde). Unknown fields are kept and written back. A manifest without them is read as before. The Store refuses an install the running core
  cannot serve (a range the core is below, a needed capability it does not have, a newer `manifest_version`) naming the versions and the side to update; an update shows
  "Needs a newer IYAKKI". A member has `id`, `kind`, `file`, `title`, `optional`, its own `capabilities`, `requires` and `wizards`; paths (no `..`, no leading `/`), ids, at most 16 members,
  each file present and at most 512 KB (the existing per-file cap; the 1.5 MB bundle cap is unchanged) and unknown capabilities are refused when the app is read.
  Unknown kinds are kept and shown as "not supported by this IYAKKI"; such a member blocks the install unless it is `optional`, and then the rest installs. The approval book holds one record per member
  (`<installation>/<member>`), and a member's wizard may use only the member's capabilities. Install, update and remove of the members are all or none (`store::bundle`): the first failure undoes the
  earlier members, the kept copies and the app's own file. The install card has a "This bundle also brings" section grouped by member.
* **Wired member kinds:** `skill` (through the skills store; a name that already exists is a failure), `flow` (a draft in the person's own `Flows` folder through `FlowStore::create` after the import sanitiser,
  never production, triggers off; only for an install for oneself), `agent`, `template`, `theme`, `extension` (the verified file is kept in `store/bundles/...`; no tile reads it from there yet).
* **Known but not wired:** `tool`, `mcp`, a nested `tile`. They show as not supported (like an unknown kind) because they need their own signed package and administrator rules.
* **Not built:** a top-level `flow` package kind, a bundle of a kind other than `tile` (only a tile package has an `app.json`), updating one member on its own, a bundle that depends on another, enforcement of a member's
  capabilities (the broker has no store-app session yet), the update-check badge wording beyond the review, the "needs a client device" card line, an update keeps nothing of the person's edits to a flow member.

## 6e. Install and update wizards: an app brings its own setup and migration (owner, 2026-11-11)

An app, or a bundle member (6d), can declare a **wizard** that the install or update runs: add or change its configuration, and migrate the data it already has.

* **Declared, not coded into the shell.** `app.json` names the wizards by trigger: `on_install`, `on_update` (with a `from` version range), `reconfigure` (a button on the app's page and a chat tool) and `on_remove` (offer to export the data). Each wizard is a list of steps: a **form** (the shell's own form component: text, choice, secret, path, toggle, with validation rules), a **check** (a connectivity or permission test the core runs), a **migration**, and a **summary**. The shell draws it; the app supplies no UI code, so the wizard looks the same on the screen, in the client page (CLIENT-SPLIT-DESIGN.md) and in chat.
* **Migrations are declared too.** A data migration is a versioned list of transforms with the same engine the system's own files use (`iyakki_schema`: schema version, ordered migrations, a dry run). A migration that needs real code is a sandboxed WASM module that sees the app's data folder and the broker only, with the capabilities the manifest declares; it never gets raw file or network access.
* **Safe by construction.** Before any migration the core takes a snapshot of the app's data (kept until the person confirms or the wizard's retention ends); a **dry run** shows what would change; the migration is resumable and either completes or rolls back to the snapshot; progress and the result go to the audit log. A failed wizard on an update leaves the old version running.
* **Capabilities and asks.** The wizard may only use capabilities the manifest declared and the person approved on the install card; a step that needs a new one asks first (hard asks keep their per-use card). Secrets typed in a form go to the vault, never to the app's config file or the log.
* **Everything reachable by chat.** Each wizard has a chat tool that runs the same steps with the same approvals ("set up the weather app", "migrate my notes to the new version"), so nothing exists only as a screen.
* **Bundles and updates.** In a bundle each member can have its own wizard; the install card lists them in order. An update shows only the wizards whose `from` range matches the installed version and whose steps are new.
* **Where it runs.** On the instance (bare metal or cloud pod), against the instance's data. A client only draws the steps and returns the answers.

* **The wizard is a shared core service, not part of each app (owner, 2026-11-11).** Like the broker and the capability catalogue, the **wizard engine** lives in `iyakki-core`
  and is part of the published interface (6c): one implementation for every app, bundle member and client. An app does not ship wizard code; at its **install hook** (and
  its update, reconfigure and remove hooks) it calls the core: `wizard.run(spec)` with the declarative spec from `app.json`, or a spec built at run time (the hook can
  look at the machine first: existing config, an older data folder, what is already installed, then ask only what is still needed). The engine renders the steps,
  validates answers, takes the snapshot, runs the declared migration, writes the result through the broker and reports back to the hook (`done`, `cancelled`, `failed`
  with the reason). The hook runs with the app's capabilities plus one catalogue entry, `wizard.run`; it cannot draw its own UI or reach the data folder except
  through the engine. Because the engine is the only wizard code, a fix or a new step kind reaches every app at once, the look is consistent, and the chat tool,
  the screen and the client page all get it for free.

Open: the exact form-step vocabulary (reuse the existing `Form` field kinds first), the snapshot retention default, whether a wizard may be skipped for unattended installs (cloud: a default-answers file), and the WASM migrator limits.

**Status (2026-11-11): the spec is built, the engine is not.**
* **Built (data and validation, `iyakki-core-api::wizard`, `wizard_version` 1, serde only, golden JSON in `crates/iyakki-core-api/tests/golden/`).** The manifest field `wizards` (and `wizards` on a bundle member) is parsed and validated.
  Triggers `on_install`, `on_update` (with a `from` version range), `reconfigure`, `on_remove`. Steps: `form` (field kinds `text`, `choice`, `secret`, `path`, `toggle`; rules `min_len`, `max_len`, `no_whitespace`, `absolute_path`, `one_of`),
  `check` (`net.reach`, `path.readable`, `path.writable`, `capability.held`; a check may use only a capability the manifest or member declared), `migration` (a reference to a name or file inside the app, with optional `from` and `to` versions) and `summary` (last).
  Limits: 8 wizards, 12 steps, 12 fields per form, 6 rules per field, 24 choices, labels 80 and texts 600 characters, a secret has no default (secrets go to the vault, never into a manifest).
  Unknown fields are kept. An unknown step kind, field kind, rule, check or trigger, or a newer `wizard_version`, is parsed and kept and refused at run time by `WizardSpec::check_runnable` ("Needs a newer IYAKKI" or "does not know ..."), not at parse time.
* **Not built:** the wizard engine (`wizard.run`, the `wizard.run` catalogue entry), the form screen and the chat tool, the data snapshot and dry run, the migration runner and the WASM migrator, the install and update hooks that call the engine, the retention default,
  default-answers files for unattended installs, and showing the wizards on the install card in order. The Store reads and validates the field today and does nothing else with it.

## 6f. The guarded Os: the app layer asks permission before it touches the machine (owner, 2026-11-14; F-ARCH-05/06, F-CORE-12/13)

The rule: the app layer does not talk to core Linux or to devices directly, it asks permission. Before this section `cx.os` only removed the direct `/proc`, `/sys` and `/dev` reads from the tiles; any tile holding an `Os` could still switch a radio, change the brightness or reconfigure swap. Now:

- `iyakki_core_api::guarded::GuardedOs` wraps an `Os` for **one app identity** (`Caller`, built-in kind) and the broker of section 6. Every call is one `Broker::decide`, so the catalogue decides, a hard ask shows its per-use card, and every decision (allowed or refused) is one line in the access log of 6b.
- Reads need `system.read` (built-in default for every built-in app, no card). Changes need their own name: `system.radio.set`, `system.brightness.set`, `system.volume.set` (table-only: Settings; the shell holds brightness and volume for its keys) and `system.memory.set` (table-only: Settings, and a **hard ask**: compressed memory and swap ask at every use). Store apps hold them only by an approval, and the install card lists them like any capability.
- A refused read answers with the empty figure (an empty list, zero statistics, no backlight), as if the machine had none; a refused change answers with the denial in words (`denied (not_granted): ...`, codes as in section 6). Nothing a refused call asked for reaches the inner `Os`.
- The shell (`shell_osguard.rs`) holds the one raw `LocalOs` and the broker and builds each tile's `cx.os` as a `GuardedOs` for the id of the screen the tile runs on (`PApp::id()`; `shell` outside an app). A tile has no way to the raw handle: `AppCx.os` is the guarded one, and the dependency ratchet (`iyakki-core/tests/dependency_ratchet.rs`, `apps_do_not_name_the_raw_os_crate`) fails if a file under `apps/` names `iyakki_core_local` beyond its list, which may only shrink.
- The per-use card of the shell is a `GestureCard` bound to the event being handled: the shell handles each key or click inside `GestureCard::dispatch`, and a hard ask is answered Allow only for a call made inside that dispatch on the same thread. A call after the dispatch returned, from another thread, or when nobody pressed anything is refused with `no_card`; an earlier press never counts, however recent. Work the press starts on a job thread (Settings, Memory, Apply) takes a single-use `GestureTicket` in the dispatch (`gesture_ticket()`, good for one second) and runs through `redeem`. The on-screen card with the name of the app and Allow/Deny buttons (as the Cameras recording card will have) is not built yet; the dispatch-bound press is the stand-in until then (KI-940).
- Pages moved so far that go through it: Memory (Settings), Devices, Network, Display and Sound (Settings, through `SettingsApp::new` taking `cx.os.hardware()`), System and Settings overview (disk and address reads). Pages that still hold their own hardware handles are the ratchet's list in `data/shell_kernel_access.allow` plus the two `iyakki_core_local` uses the new ratchet test names (`devices.rs` default pane before it is given the guarded one, `browser_speed.rs` for startup).

## 7. Packaging

* `iyakki-core` becomes `crate-type = ["cdylib", "rlib"]`; the boot and supervisor program (today's `iyakki-core` binary) becomes a thin main that uses it.
* The image today links statically against musl. A shared library needs dynamic musl linking and the dynamic loader on the image; Buildroot carries musl, so this
  is doable but changes how the image is built and how the Secure Boot chain treats one more file. It is its own step and has its own test: boot in QEMU.
* One copy of the core is in memory for all built-in apps; an update of the core does not rebuild the apps, as long as the interface version matches.

## 8. Slices

1. **Catalogue and manifest.** The capability names, the `app.json` field, the approval record and its audit entry, and the install card in the Store. Tests with fakes.
   **Built (2026-11-11, tested with fakes, never seen on a real screen):**
   * `iyakki-core-api::appcaps`: the catalogue (19 names, the last three for client devices: `clipboard.client`, `client.camera.offer`, `client.microphone.offer`; before them `web.request.headers` for browser extensions with parameter kind, built-in default and hard-ask flag, version 0), `CapabilityRequest` (`name`, `why`, optional `param`),
     `validate_requests` (unknown name, missing reason, wrong parameter, duplicate, more than 24: refused with a plain message), `plan_ask` (first install asks for all; an update asks only for
     what the installed approval does not cover; a revoked capability asks again), `card_lines`, `Approval` (who, app, version, hash, time, grants, revoked), the `ApprovalBook` trait with
     `MemoryBook` as the fake, and `BUILTIN_TABLE` / `builtin_holds` (a static table compiled into the image, so the image signature covers it). Serde only; no IO.
   * `app.json`: `capabilities` may be this list (the catalogue) or the older tile object (`{"storage": true}`), never both; `TileManifest.core_caps` holds the list.
   * Store: `Review.core_requests` and `core_lines`; the card needs the person's answer before install (`NeedsConfirmation` otherwise); the approval is written to
     `<store>/approvals.json` per installation and dropped at remove; audit entry `store.approve` (who, app, version, hash, capabilities) beside `store.install` in the shell and in `apply_update`;
     an update that asks for more shows them as new permissions (`core:<name>`) and is not applied by Update all.
   * Shell: the Store review card has a "Device and system access" section, hard asks marked "every use still asks you", new lines marked on an update.
   **Not built (slice 2 and later):**
   * Nothing enforces these records yet: no broker, no call is stopped or allowed because of them. `builtin_holds` is not called by any code.
   * The older tile object form (`storage`, `network`, `clipboard`, ...) is not mapped onto the catalogue; it still works as before.
   * Revoke (Settings, Apps), the assistant tool for approvals and revoking, the access log (6b), the per-use card for hard asks.
   * Approvals exist for tile apps only; skills, tools and MCP servers keep their older permission words.
   * The built-in table names app ids by hand and is not cross-checked against the real app ids; `wasm32` compilation of `iyakki-core-api` was not tried (it depends on `libc` and `iyakki-sec`).
   * The audit entry is made by the callers (shell, update); an install through another path (assistant tool) would write the approval but no `store.approve` audit line until that path is wired.
2. **The broker and one capability, with the access log.** `camera.view` through the broker and through the direct table; the camera tile as its first user; a denied call is refused and audited; the first access-log table and the Settings, Apps page that shows it (section 6b).
   **Built (2026-11-11, tested with fakes, never seen on a real screen):**
   * `iyakki-core-api::broker` (pure; the crate now builds with `--no-default-features` without libc and `iyakki-sec`): `Broker::decide` with the order unknown name, held (built-in table or stored approval, read afresh), hard ask needs a card; stable codes `unknown_capability`, `not_granted`, `revoked`, `card_declined`, `no_card`; seams `ApprovalLookup`, `CardAsk` (`NoCard`), `AccessSink`, a clock; `AccessEvent`; `Approval::revoke`.
   * `iyakki-agent::accessbroker`: `FileLookup` (reads `<data>/store/approvals.json`), `FileLog` (the access log, `<data>/access/log.jsonl`, mode 0600, last 5 000 lines, app, person, device name, capability, camera name, time, allowed or the refusal code; no content, no pictures; a repeat use is folded into one line per minute, a repeat refusal per ten seconds), `local()` (the broker of the machine), `held_list`, `revoke`.
   * Camera hub: `Hub::view` and `Hub::snapshot` of this computer's own USB cameras ask the broker (app `cameras`, a built-in); a refusal comes back as a plain message starting `denied (code)` before the camera is opened. Release on leaving the tile and the Dark status are unchanged. The shell sets the local broker once, when it makes the hub.
   * Settings, Apps access (administrators, under Apps & AI): what installed apps were allowed, who approved it, Take away (audit `access.revoke`, applies at the next call), the log of the last uses, Clear the log (audit `access.log.clear`).
   **Not built (slice 3 and later):**
   * No session endpoint: no store app can reach the broker, so nothing but the Cameras hub asks it. The refusal tests for store callers go through the hub with a store caller and through the broker directly.
   * Only `camera.view` of this computer's own cameras is routed. Network cameras, recording, the assistant's `cameras_*` tools (they use the same hub, so they are covered as the `cameras` app for the USB camera), sound, Bluetooth, files and network are not.
   * The real per-use card for hard asks (the shell's consent mechanism) is not wired: the local broker has no card, so a hard ask is refused with `no_card`. The `CardAsk` trait is where it goes.
   * Section 6b is only started: one ring per machine (not per account), no daily totals, no Access tab in USAGE, no assistant tool `access_log_query`, no network log (the HTTP client and firewall do not write to it), no per-app page with day, week and month views. A person who is not an administrator does not see the page.
   * The role policy still applies in the hub (`usb_ok`) before the broker; the broker does not read roles yet.
   * `wasm32-unknown-unknown` is not installed on the build host, so the wasm check was not run; `cargo check -p iyakki-core-api --no-default-features` passes and the dependency tree is serde and thiserror only (both pure Rust).
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
