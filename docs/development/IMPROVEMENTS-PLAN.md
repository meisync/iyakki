# Improvements plan (P1 to P14)

Planning document for the fourteen items the assistant proposed on 2026-10-27 (`docs/project/BACKLOG.md`, section "Proposed by the assistant"). The owner asked to plan them and to go slowly, one at a time. Nothing here is built except P2 (see the P2 section). Sizes: S under 3 days, M 3 to 8, L more.

**How to read the "exists" lines.** *Verified* means the file or function was opened or searched in the tree on 2026-10-27. *From memory* means it is what I remember of the code and was not re-checked for this document; check before relying on it. FEATURES ids below are proposals only; none is in `FEATURES.md` yet (P2 has real rows).

**Shared rules for every item.** Tiles follow the shell conventions (a tile app under `crates/iyakki-shell/src/apps/`, keyboard first, buttons with letters, drawn in one `render`, tests at four screen sizes as the Settings pages do). Nothing leaves the device without a preview and a click. Every change an administrator makes is audited; no secrets, no personal names in generated names, logs or files. Cloud mode hides what needs local hardware. Each item adds a developer-mode hook only where noted.

## Suggested order

P2 (done in this pass), P4, P1, P3, P5, P6, P7, then P8 to P14 after the beta. P12 needs P2 and P11. P3 needs the startup registry (built). P5 and P6 need the upgrade tests.

## P1 Check-up tile (hardware self-test wizard)

**Purpose.** A guided test of display, touch, keyboard, speakers, microphone, cameras, Wi-Fi, Ethernet, USB, battery, GPU and the k3s start, ticked or failed step by step. It writes one result file whose rows map to the cells of `docs/project/HARDWARE-MATRIX.md`.

**Exists.** Verified: `iyakki hwreport` in `crates/iyakki-core/src/main.rs` (listed in the usage text, handled at the `"hwreport"` arm); `crates/iyakki-hwinfo` (device facts); the sound, camera, battery and Bluetooth probes in `iyakki-devctl`; the startup registry for the k3s step (`crates/iyakki-agent/src/startup.rs`); `docs/project/HARDWARE-MATRIX.md` and `PC-TEST-SHEET.md`. From memory: the exact fields `hwreport` prints (re-read before reuse).

**Design.**
- Data: `checkup.json` in the person's data folder (schema via `iyakki-schema`): machine facts from `hwreport`, one entry per step `{id, matrix_cell, result: pass|fail|skip, note, at}`; the result is signed off with the account name only if the person ticks "include my account name" (default off, the file otherwise names the machine by its hardware, not a person).
- Screen CHECK-UP: left list of steps with a tick, right pane with the instruction and the live probe (a tone and "did you hear it?", a colour grid, a key-echo line, the camera preview, an `iw`/link status line). Buttons: Pass (`p`), Fail (`f`), Skip (`s`), Back (`b`), Export (`e`).
- Export: to a folder or USB stick as `checkup-<date>.json` plus a text version for the matrix; a `iyakki checkup` CLI prints the same.
- Developer mode: `iyakki dev checkup --export FILE` (read scope `debug`) so a result comes back without a USB stick.

**Security and privacy.** The file holds hardware ids, MAC addresses and the Wi-Fi network name; the export preview shows them and offers "hide MAC addresses and network names" (default on). No camera frame or microphone sample is saved.

**Tests.** The step list covers every matrix column that has a probe (a test reads the matrix file and fails when a column has no step or a step names no cell); result file round trip and schema upgrade; export redaction; each probe against a fake machine; screen layout at four sizes.

**Size** M. **Depends on** the matrix file format staying stable. **Proposed ids** F-CHECKUP-01 (steps and result file), F-CHECKUP-02 (export and dev command).

**Open questions.** Which of the 210 cells can be probed by software at all (suspend, HDMI audio and the like need a human)? Decision D1.

## P2 Stable device name (built in this pass)

**Purpose.** The device announces `iyakki-<short random id>.local`, and `iyakki dev` finds it by that name, so a DHCP change no longer breaks developer mode.

**Exists (verified before building).** An mDNS *client* in `crates/iyakki-netscan/src/discover.rs` (one query burst, answers parsed into hosts); developer-mode pairing and `devices.json` in `crates/iyakki-agent/src/devmode/client.rs` (a device had `host`, `port`, `fingerprint`, `account`); the pinned-fingerprint TLS client (`Pin` verifier in the same file); the developer-mode firewall rules (`crates/iyakki-fw/src/devrules.rs`, applied by `crates/iyakki-remote/src/devfw.rs` every two seconds); the startup registry. The minimal web service exists (`crates/iyakki-core/src/svc.rs`, `docs/development/MINIMAL-WEBSERVICE.md`) and has its own listen address.

**Built.** See "What P2 built" below. **Size** S (turned out M with tests). **Proposed ids** F-NAME-01 to F-NAME-05 (these are real rows in `FEATURES.md`).

**Open questions.** Decisions D2 (announce the web service), D3 (name shown on the screen for a person to type).

## P3 Doctor tile (explain and fix)

**Purpose.** One screen that lists what is wrong in plain words, each with a fix button: cluster stopped, firewall blocking the developer port, missing firmware, camera busy, clock wrong, disk nearly full, service failed.

**Exists.** Verified: the startup registry with failure reasons and a retry hook (`startup.rs`, `Def.can_retry`, `Goes`), the firewall hint for the developer port (`devrules::hint`), the firmware item (`crates/iyakki-agent/src/fwextra*.rs`, Setup item `fw-hardware`), Settings pages the fixes can jump to (`Goes::item`). From memory: the disk-space and clock checks exist as separate probes but not as a list.

**Design.**
- A `Finding {id, severity, text, fix: Option<Fix>}` list produced by pure functions over a facts struct (registry rows, firewall hint, free space, clock sync state, camera holders, firmware missing), so every rule is testable with a made-up machine.
- Screen DOCTOR: findings sorted by severity; Enter runs the fix (restart service, open the Firewall page with the rule prefilled, open Setup firmware, free space dialog); "all good" state is a single calm line.
- CLI: `iyakki doctor` prints the list; developer mode: `iyakki dev doctor`.
- Fixes that change something ask first and are audited; none runs silently.

**Security and privacy.** Reasons can hold paths and addresses: administrators see them, others see the plain sentence (same split as the startup strip).

**Tests.** One test per rule with a fixture machine and one for the sort order; fix actions against fakes; the screen at four sizes; a test that every registry service with `can_retry` has a finding rule.

**Size** M. **Depends on** startup registry (built), firmware item (built). **Proposed ids** F-DOCTOR-01 (rules), F-DOCTOR-02 (tile and CLI).

**Open questions.** D4: may "fix" actions restart k3s without a question?

## P4 Share diagnostics button

**Purpose.** A redacted support bundle (version, services, logs without bodies, hardware report) previewed on screen and then saved to a file or USB stick, or sent only by an explicit click.

**Exists.** Verified: `iyakki dev diag` and `crates/iyakki-agent/src/devmode/{diag,api_diag,cli_diag}.rs`; `hwreport`; startup registry; the audit log; crash log settings page (`settings_crashlog.rs`). From memory: how much of diag is already redacted.

**Design.**
- A `Bundle` builder in the agent: sections `{version, services, hardware, network (no addresses by default), logs (levels and counts, message text with paths and addresses redacted), settings (names only, no values)}`; a redaction function with one table of patterns (home paths, IPv4/IPv6, MAC, e-mail, tokens like `sk-`, long hex) and a test corpus.
- Screen: Settings -> System -> "Share diagnostics": scrollable preview of exactly what will be written, a checkbox per section, Save to folder or stick (`s`), Copy path.
- CLI `iyakki diag bundle -o FILE`; developer mode: the existing `diag` gets `--bundle`.
- The file is a plain tar of text files so the person can read it.

**Security and privacy.** Redaction is the whole feature: default off for anything with addresses; the preview is the source of truth (what is shown is what is written, same code path); no upload code in this item.

**Tests.** Redaction corpus (every pattern, false-positive cases); preview equals file byte for byte; a bundle built from a machine full of fake secrets contains none of them; size cap.

**Size** S. **Depends on** none. **Proposed ids** F-DIAGSHARE-01.

**Open questions.** D5: is "send" ever allowed, or file/stick only for 1.0?

## P5 Update centre

**Purpose.** Show the installed and the previous version, check for a newer image, install from the internet, a USB stick or the USB-C cable, roll back in one click, keeping data.

**Exists.** Verified: `iyakki dev update` and its safety tests (`devmode/update.rs`, `tests_update*.rs`, boot files plus `boot-next/` and `boot-prev/` on the data volume); `crates/iyakki-relcheck` (release checks); schema upgrade framework `iyakki-schema`; `upgrade_tests.rs` files. From memory: no tile or Settings page does any of this today.

**Design.**
- Reuse `devmode/update.rs` as the engine (it already knows payloads, the previous version and rollback); a tile shell around it: Versions (installed, previous, available), Check (`c`, asks the release index the person configured), Install from internet/stick/cable, Roll back (`r`, asks first).
- Release index and signature check: reuse the signing the update payload already has; the check refuses an unsigned or older payload unless forced in developer mode.
- Developer mode: the existing `update --status` feeds the same view.

**Security and privacy.** Signature verification is mandatory; the check sends the version and variant only (shown on screen before the first check); no automatic install.

**Tests.** The engine's tests already exist; add UI state machine tests, a fake release index, rollback with the data volume untouched, interrupted download.

**Size** L. **Depends on** the upgrade tests (gate item 5). **Proposed ids** F-UPDATE-01 to F-UPDATE-03.

**Open questions.** D6: where is the release index hosted and who signs?

## P6 Backup and restore

**Purpose.** Snapshot of `/var/lib/iyakki` and the person's home to a USB stick or network share, with a restore check and a schedule; the vault stays encrypted.

**Exists.** Verified: `dev files pull` (`devmode/files.rs`), the vault and lock design (`docs/development/LOCK-AND-ENCRYPTION.md`), shares settings (`settings_shares.rs`), the storage crates. From memory: no archive format for the whole data volume exists.

**Design.**
- Archive: tar plus a manifest with per-file SHA-256 and the schema versions (`iyakki-schema` stamps), compressed by the existing in-house compression if there is one, else stored; encrypted when the person sets a backup password (key derived in-house; the vault file is copied as is, never decrypted).
- Screen BACKUP: destinations, last backup, Back up now (`b`), Verify (`v`), Restore (`r`, restores to a staging folder first and shows a diff), schedule (daily/weekly).
- CLI `iyakki backup run|verify|restore`.

**Security and privacy.** A backup of a home folder is the person's data: encryption default on for network shares; restore never overwrites newer files without a question; no credentials of shares inside the archive.

**Tests.** Round trip, verify catches a flipped byte, restore of an older schema is migrated, interrupted run leaves no half archive, schedule logic with a fake clock.

**Size** M. **Depends on** upgrade tests. **Proposed ids** F-BACKUP-01 to F-BACKUP-03.

**Open questions.** D7: encrypt by default also for a USB stick?

## P7 Recipes gallery

**Purpose.** Ready-made flows and agents installable in one click, each with a plain description of what it does and needs and a "not installed yet" state.

**Exists.** Verified: flows templates and the flows tile (`crates/iyakki-flow`, `iyakki-shell/src/apps/flows.rs`); the toolkit and messaging "not installed yet" checks were mentioned in the backlog evidence. From memory: the 13 templates list and their location.

**Design.** A `recipes/` folder of signed-by-the-image JSON (name, description, requires: toolkit/channel/model, flow body); the gallery tile lists them with a requirement chip each; Install writes the flow disabled; Enable asks for the permissions it shows. Developer mode: `iyakki dev recipe add FILE` for authors.

**Security and privacy.** A recipe is a flow: it goes through the same approval and permission checks, installs disabled, and names every tool and channel it will use before enabling.

**Tests.** Every shipped recipe parses and validates; a recipe with a missing requirement shows "not installed yet" and cannot be enabled; install is idempotent.

**Size** S. **Depends on** toolkit and messaging first slice. **Proposed ids** F-RECIPES-01.

**Open questions.** D8: ship recipes in the image or fetch from the official store?

## P8 Usage and budget

**Purpose.** Tokens and cost per model and agent, daily and monthly caps, an alert before a cap, a per-flow limit.

**Exists.** Verified: `crates/iyakki-agent/src/usage.rs` (a year of counts per model, merged lines) and the USAGE tile (`iyakki-shell/src/apps/usage.rs`); `token_price_cents_per_mtok` setting (cost budgets for agents exist per the settings comment); `docs/development/TOKEN-BUDGET.md`. From memory: tool routing is the `no_tool_routing` setting.

**Design.** Add the agent and flow id to usage lines (schema bump), caps in settings (`budget.daily_tokens`, `budget.monthly_cents`, per-flow limit), a check before each model call (refuse with a plain message at 100%, notification at 80%), USAGE tile gets an "per agent" view and a Budget row.

**Security and privacy.** Usage holds counts, not text. The cap check must fail closed for flows (stop, notify) and open for a person's own chat (warn).

**Tests.** Cap arithmetic with a fake clock across midnight and month change; alert fires once; old usage files load; per-flow limit stops a loop.

**Size** M. **Depends on** token routing (built). **Proposed ids** F-BUDGET-01, F-BUDGET-02.

**Open questions.** D9: hard stop or ask?

## P9 Model advisor

**Purpose.** On first run and after hardware changes, recommend local models that fit this machine and download them on request.

**Exists.** Verified: `aiprofile.rs` (memory a model can really use, one table for the AI tools), `docs/AI-HARDWARE.md`, `modelhost.rs` (hosts including the Mac bridge), Ollama and Lemonade probes. From memory: the model catalogue with sizes is not in the tree.

**Design.** A small catalogue file (model, size on disk, memory needed, licence) ranked by the profile; Setup shows "fits / tight / too big"; download uses the existing provider download path with the consent step; recommendation re-runs when the profile changes (hash of the profile).

**Security and privacy.** Only licences the owner allows are listed (permissive or the model's own terms shown); download from the vendor on click, never shipped.

**Tests.** Ranking for fixture profiles (Pi 5, Apple Silicon, big GPU, no GPU); hash change triggers a new advice once; catalogue licence field present for every entry.

**Size** M. **Depends on** the AI-HARDWARE profile. **Proposed ids** F-MODELADVICE-01.

**Open questions.** D10: who maintains the catalogue and how often?

## P10 Activity tile

**Purpose.** A readable timeline of what agents and flows did, filterable by agent, with undo where the action allows.

**Exists.** Verified: the audit log and hash-chained ledger (`iyakki-ledger`, `runs.rs` reads transcripts and checks the chain), the RUNS tile. From memory: which actions record a reversible inverse (probably none).

**Design.** A reader that turns ledger and audit lines into sentences (`agent X wrote file Y`, `approved by you`); filters by agent, day, kind; Undo only for actions that registered an inverse (file write with backup, flow enable) via a new optional `undo` field in the record; never for sent messages.

**Security and privacy.** Reads only what the signed-in person may read; administrators see all; the sentence builder never prints message bodies.

**Tests.** Sentence for every record type (a test fails when a new type has none); chain verification still shown; undo restores a file and is itself recorded.

**Size** M. **Depends on** audit log (built). **Proposed ids** F-ACTIVITY-01, F-ACTIVITY-02.

**Open questions.** D11: new tile or a tab in RUNS?

## P11 Phone-friendly web UI

**Purpose.** Chat, inbox, flow runs and approvals on a phone, behind the web service login.

**Exists.** Verified: the minimal web service (`crates/iyakki-core/src/svc.rs`, `docs/development/MINIMAL-WEBSERVICE.md`) another agent is working on; `crates/iyakki-remote` (page, session, server, codec); TOTP design (`docs/development/TOTP-2FA.md`). From memory: how the remote pages are drawn (pixel streaming) versus HTML.

**Design.** Depends on the web service decisions (HTML pages versus pixel streaming). Plan after that service lands: server-rendered pages with no framework, four routes, same session cookies and login as the service, approvals as signed POSTs with CSRF tokens.

**Security and privacy.** Everything behind the service login (2FA when enabled), HTTPS only, no cached sensitive pages (`Cache-Control: no-store`), approvals need a fresh confirmation.

**Tests.** Route tests with a fake env, CSRF refusal, session expiry, layout snapshot at phone widths.

**Size** M. **Depends on** minimal web service. **Proposed ids** F-PHONE-01.

**Open questions.** D12: HTML pages or the existing pixel stream scaled for a phone?

## P12 Pairing by QR code

**Purpose.** Pair developer mode or the phone UI by scanning a code from the device screen that includes the address, name and fingerprint.

**Exists.** Verified: the in-house QR encoder (`crates/iyakki-agent/src/qr.rs`, up to 858 bytes) and its decoder (`qr_decode.rs`); the pairing code and fingerprint on the Developer mode page; P2's name in the pairing record. From memory: the sign-in address QR is already shown somewhere.

**Design.** The QR holds `iyakki-pair:v1?name=<iyakki-id.local>&port=7979&fp=<sha256>&code=<one-time code>`; `iyakki dev pair --qr IMAGE` reads a picture (screenshot or photo) with the in-house decoder; a phone page posts the same payload. The one-time code expires in 10 minutes as today.

**Security and privacy.** The QR contains the one-time code, so it is shown only while a pairing is open and only to the administrator at the screen; the fingerprint inside means a machine in the middle cannot pair.

**Tests.** Encode/decode round trip of the payload, expired and reused codes refused, tampered fingerprint refused.

**Size** S. **Depends on** P2 (and P11 for the phone). **Proposed ids** F-QRPAIR-01.

**Open questions.** D13: pair from a phone photo, or only from the CLI?

## P13 Accessibility pass

**Purpose.** A keyboard-only path through install, sign-in, one tile and Settings; text scaling audited at 150% and 200%; a screen-reader hook for the reader browser.

**Exists.** Verified: keyboard-first tiles everywhere; the four-screen-size layout tests in Settings; `docs/KEYBOARD-AND-FN-KEYS.md`; `iyakki-ui` toolkit. From memory: whether the shell has any text-scale setting (not checked); the reader browser has no accessibility tree export (not checked).

**Design.** An audit test that walks each tile at scale 1.5 and 2.0 and asserts nothing draws outside its window and every button is reachable by keys; a focus-order list per screen; an optional "speak what has focus" hook using the local TTS (`TALES`/voice) behind a setting; a written checklist for gate row 26.

**Security and privacy.** The speech hook must not speak secret fields (password, vault) and is off by default.

**Tests.** The scale audit itself; focus reaches every actionable element on the sign-in and Settings pages; secret fields excluded from speech.

**Size** M. **Depends on** none. **Proposed ids** F-A11Y-01 to F-A11Y-03.

**Open questions.** D14: is a screen reader for 1.0 or a documented limit?

## P14 Power

**Purpose.** Sleep and suspend, a power profile (quiet, balanced, fast) and a battery estimate.

**Exists.** Verified: battery settings (`crates/iyakki-agent/src/battery.rs`, `settings_battery.rs`, `docs/BATTERY.md`); F-PWR-01 planned in `FEATURES.md`; the matrix proposes waiving sleep for 1.0.0. From memory: no suspend code.

**Design.** Write to `/sys/power/state` and cpufreq governors/`energy_performance_preference` through a small root helper; Settings page Power; wake sources listed; battery estimate from the last hour of discharge rate. Hardware risk is high (Mac suspend is known troublesome), so each machine class gets an allow-list.

**Security and privacy.** Lock the session before suspend; the vault locks on suspend.

**Tests.** Pure logic with fake sysfs; the allow-list; estimate maths. Real suspend only on hardware.

**Size** L. **Depends on** none. **Proposed ids** F-PWR-02 to F-PWR-04.

**Open questions.** D15: waive sleep for 1.0 as the matrix proposes?

## Numbered decisions for the owner

| # | Question | Recommendation |
|---|---|---|
| D1 | Which matrix cells does the check-up cover? | Only those a program can probe; mark the rest "by hand" in the file. |
| D2 | Announce the web service (`_iyakki-web._tcp`) too? | Yes when the service is on; the code supports it, the wiring waits for the `serve` owner. |
| D3 | Show the random name on the screen so a person can type it? | Yes: on the Developer mode page and Settings, Network. |
| D4 | May the Doctor restart k3s without asking? | No, always ask. |
| D5 | Is a "send" button allowed for diagnostics in 1.0? | No: file or stick only. |
| D6 | Who signs and hosts the release index? | The owner's release key; start with a static file next to the images. |
| D7 | Backups encrypted by default? | Yes, for shares; asked for sticks. |
| D8 | Recipes in the image or in the store? | In the image for six starters, the rest in the official store. |
| D9 | Budget at 100%: stop or ask? | Stop flows, ask in chat. |
| D10 | Who maintains the model catalogue? | A file in the repo, reviewed each release. |
| D11 | Activity: new tile or a RUNS tab? | A tab in RUNS first. |
| D12 | Phone UI: HTML or scaled pixel stream? | HTML pages, small and cacheless. |
| D13 | QR pairing from a phone photo? | CLI first. |
| D14 | Screen reader for 1.0? | Documented limit plus the focus-order work. |
| D15 | Waive sleep for 1.0? | Yes. |
| D16 | P2: is the random name's 24 bits enough? | Yes, with the conflict probe and suffix; a clone of a disk image is the only realistic clash. |
| D17 | P2: announce while developer mode is off? | The host name yes (switch on), the developer service only while developer mode is on. |

## What P2 built (summary, details in the code and docs)

- `crates/iyakki-netscan/src/mdns/`: `wire.rs` (packets, hardened decoder), `responder.rs` (probe, announce, defend, goodbye, rate limits, legacy unicast), `net.rs` (interfaces, shared socket on 5353, announcer thread, resolver and scan that do not need port 5353), `tests.rs`.
- Settings switch `announce_name` (default on, off in cloud mode unless the person turns it on), device id file with random bytes, firewall rule "device name (mDNS)" that exists only while the switch is on, startup registry line, developer-mode state output, `iyakki dev scan`, `pair <name>.local`, re-resolve with the pinned fingerprint.
- Not verified: see the report; mainly a real network with access points that block multicast, and real avahi or mDNSResponder neighbours.
