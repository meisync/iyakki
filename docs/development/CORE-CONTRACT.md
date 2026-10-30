# The core contract: one typed API for tiles, a developer's computer and a cloud UI

Status: defined 2026-10-28 (owner: "yes please go ahead, define a futuristic contract"); migration steps 1 to 3 done the same day.
The cross-cutting types and the first three domains (settings, vault, flows, plus consent) are in code in
`crates/iyakki-core-api/src/contract/`, with in-memory fakes in `crates/iyakki-core-fake/src/contract.rs` **and now a real
implementation over the device's own services** in `crates/iyakki-agent/src/contract_local/` (section 6). One conformance suite runs
against both. Three screens read through it (Settings > Time, Settings > Vault, the FLOWS "next runs" list), with no change in what
they show or do. It travels over the developer-mode TLS channel as an **opt-in, off-by-default** transport (`devmode/contract.rs`,
section 6); no cloud relay exists. It extends `docs/development/UI-OS-SPLIT-PLAN.md` (see its addendum) from "a few kernel-facing
traits" to every domain that is today reachable only through the `Services` god object.

The rule behind all of it: tiles and any UI never touch the kernel, and a cloud or web UI must be able to link to a machine later and do
everything a person may do remotely, and nothing a person must do at the machine.

## 1. Principles

**(a) Three transports, one typed API.** A call is `(domain, call, JSON) -> JSON | CallError`, always with a [`CallContext`] (who, over which transport).

| Transport | When | How it reaches the machine |
|---|---|---|
| `in_process` | today: tiles, the shell | a trait call; the caller is a person at the machine |
| `dev_mode` | later: a paired computer | the existing TLS developer-mode channel (pairing, ed25519 signed requests, scopes) |
| `relay` | later: a cloud UI | the **machine dials out** to the relay and keeps the link; no port is opened, no inbound firewall rule |

Only `in_process` is "local". The transport sets the context; a tile cannot make a remote caller look local. There is no second auth scheme.

**(b) Version and capability discovery.** The first call on any transport is `hello`. The machine answers `Hello`: contract version
(`major.minor`), the oldest minor it still serves, machine id, build, every domain with the calls and events it supports, the hardware
`Capabilities`, and its rate limit. A client sends `ClientHello` (its version, the domains its screens want) and `negotiate` yields the
agreed version (the lower minor; a different major, or a client older than the machine serves, gets no calls), the domains it can use and
the domains the machine lacks. The client hides what is missing instead of failing, so a newer cloud UI works with an older machine and the
reverse. Rule for contract authors: a minor release only adds (calls, events, fields with defaults); anything else is a major.

**(c) Events are first-class.** One shape for every stream: `Event { seq, at_ms, topic, body }`. `seq` is strictly increasing per log and never
reused. A subscriber passes `Resume::After(n)` (or `Latest`) and gets `EventBatch { events, next, gap }`: exactly the events it missed, in
order, paged by `limit`. `gap = true` means events it never saw were already dropped from the bounded log: re-read the state first. No
polling loop is needed; the transport may push batches (WebSocket) but the shape is the same as a pull.

**(d) Large and streaming data.** A call or event never carries a big payload. It carries a `StreamHandle { id, kind, total?, max_chunk,
media_type }` (kinds: screenshot, log, camera frame, file). The client reads `Chunk { stream, offset, data, last }` by offset, so a dropped
link resumes at the offset it reached, and closes the handle (it also expires). Reading checks the same scope as the call that made the handle.

**(e) Identity, scopes, audit, risk.** A remote caller is a paired computer or a cloud session with scopes. Scope ids reuse developer mode's
(`debug`, `config`, `files`, ...) and add per-domain ones (`settings:read`, `settings:write`, `vault:status`, `vault:lock`, `vault:use`,
`flows:read`, `flows:run`, `flows:approve`, `consent:read`). A remote caller with no scopes is allowed nothing (unlike a local session,
which is limited by the person's role). Scopes only grow by an administrator's hand at the machine. Every call writes an audit line:
`<who> via <transport>: <call> [<risk>] <outcome>`; it names keys and actions, never values, passwords or secrets. Every call has one risk class:

| Risk | Meaning | Remote caller |
|---|---|---|
| `read` | no side effect | with the read scope |
| `change` | changes state the scope covers | with the write scope |
| `ask_person` | raises a question a person must confirm | may start it with the scope; the answer follows the consent rule (f) |
| `local_only` | only the machine's own screen | never, whatever scopes it holds |

**(f) Consent.** Some questions only the local screen can answer: camera, microphone, a tile's permission, a licence, a vault unlock,
pairing a new remote client. A remote client may *see* them (`consent.pending`, to show "waiting for a person at the machine") but cannot answer:
`consent.answer` takes a `LocalSeat`, a value that can only be made from a local `CallContext` (`LocalSeat::claim`), so there is no remote
code path to write. Pairing a new remote client always needs a person at the machine. A flow approval flagged `local_only` follows the same
rule; an ordinary approval can be answered remotely by a client holding `flows:approve`. Requests expire (the answer then is `expired`).

**(g) Offline-first.** The machine works with no cloud and never depends on one for a local feature. The cloud copes with a machine that is
off: the relay keeps `last_seen`, answers `offline` (a safe-to-retry code) for live calls, and may queue a *change* request with an expiry
(default 24 h, at most 7 days); the machine runs queued requests on reconnect unless they expired (then `expired`, never silently late).
Every queued or retried change carries an `IdempotencyKey` (8 to 64 characters), so a repeat does the work once and returns the first result.
The same key with a different request is `conflict`. Queued requests are never `local_only` and never raise a consent question nobody can see.

**(h) One error type.** `CallError { code, message_key, message, hint, retry_after_ms }`. Codes are stable strings, never renamed or reused:
`not_found denied unsupported invalid_input busy io unauthenticated rate_limited needs_consent locked conflict expired offline
version_mismatch cancelled internal` (golden test). Only `busy`, `io`, `rate_limited`, `offline` are safe to retry. A limiter
(`RateLimit` in `hello`, `RateLimiter` in code) answers `rate_limited` with `retry_after_ms`. The old `ApiError` converts to `CallError`.

**(i) Messages as codes plus text.** Every message has a stable `message_key` (`settings.unknown_key`, `vault.locked`) and plain English
words. A UI looks its own translation up by the key and falls back to the text. Keys are never reused for a different meaning.

**(j) Schema as the single source.** The Rust types in `iyakki-core-api::contract` are the schema (serde, snake_case tags), and
`contract::catalog::DOMAINS` is the call table as data: the `hello` answer, the fakes and a test that checks this document all read it.
When the web client is generated (later), it is generated from these types and the catalogue, so Rust, web and fake cannot drift. (The
`iyakki-schema` crate versions the JSON files on disk; it is a different thing and is not used for the wire.)

**(k) The vault rule.** The contract exposes only: whether the vault is locked (`vault.status`), unlock with the machine login (`vault.unlock`,
local seat only), lock (`vault.lock`), and "use a secret for a named action" (`vault.use_secret`: the caller names an action registered on the
machine, such as `sign_in:git`; the machine picks the secret and performs the action itself and answers with words). **No call ever returns a
secret value**, and the trait has no method with a string or byte return. A test reads the trait and fails if a method returns anything but
`VaultStatus` or `ActionOutcome`.

**(l) Settings.** A typed key/value store with a schema: each key has a kind (bool, int range, text with a length, choice), a default, a read
and write scope, a write risk (a key can be `local_only`, such as the vault timeout) and a label key. Calls: `settings.schema`, `settings.get`,
`settings.set` (idempotent with a key), `settings.watch` (events with resume). Invalid values are `invalid_input` with a hint. There is no secret
kind: secrets are not settings.

**(m) Flows.** `flows.list`, `flows.start` (idempotent), `flows.stop` (harmless twice), `flows.status`, `flows.events` (the run event stream:
started, step, approval_needed, approval_answered, frame, finished), `flows.approve` (rule (f)). A camera frame handed to a run arrives as a
`frame` event holding a `StreamHandle`, read with (d); the camera consent itself is a local question.

**(n) Stays local, never in the contract.**
- Display and input drivers (`fb.rs`, `kms.rs`, `input.rs`), the framebuffer and the keyboard/pointer; pixel streaming stays `iyakki-remote`.
- Kernel access: `/proc`, `/sys`, `/dev`, modules, firmware, boot files, system image update internals, raw block devices, any general shell.
- Raw secrets: vault values, passwords, tokens, API keys, private keys, provider addresses with keys. Only their *use* by a named action.
- Answering consent prompts (camera, microphone, permission, licence, vault unlock, pairing), and user or scope management.

## 2. Domains

Risk classes per call; "Local-only" is what never crosses; "Fake" is the in-memory implementation; "First tile" is the first screen to migrate.
Built domains have their calls listed as `domain.call` (a test checks this document names each one). Planned domains are listed by name
in slice order; their calls are a proposal to be fixed when the slice starts.

| Domain | Calls (risk) | Events | Local-only parts | Fake | First tile to migrate | Slice |
|---|---|---|---|---|---|---|
| `settings` | `settings.schema` (read), `settings.get` (read), `settings.set` (change), `settings.watch` (read) | `changed` | keys marked local-only (vault timeout, firewall lockdown) | `FakeSettings` | SETTINGS (display page) | 1 |
| `accounts` | list, who-am-i (read); add/remove/role (change, admin); sign-in is local | `changed` | passwords, login, lock screen, pairing | planned | SETTINGS (accounts page) | 1 |
| `vault` | `vault.status` (read), `vault.unlock` (local_only), `vault.lock` (change), `vault.use_secret` (change) | none | unlock, every secret value | `FakeVault` | VAULT tile status badge | 2 |
| `consent` | `consent.pending` (read), `consent.answer` (local_only) | `asked`, `answered`, `expired` | answering | `FakeConsents` | permission dialog | 2 |
| `flows` | `flows.list` (read), `flows.status` (read), `flows.events` (read), `flows.start` (change), `flows.stop` (change), `flows.approve` (ask_person) | `started`, `step`, `approval_needed`, `approval_answered`, `frame`, `finished` | camera/microphone approvals | `FakeFlows` | FLOWS tile run list | 3 |
| `jobs` (with flows) | list, status (read); start, stop (change); log as a stream | run events | none | planned | JOBS | 3 |
| `chat` | send (change), history (read), stop (change); tokens as events | `token`, `done` | which model/provider keys are used | planned | CHAT | 4 |
| `models` | list, status (read); pull, remove, select (change) | `progress` | model files on disk | planned | MODELS | 4 |
| `store` | catalog, installed (read); install, remove (change); stores add/remove (change) | `progress` | package signature keys | planned | STORE | 5 |
| `apps` | list, start, stop (change); logs as a stream | `state` | tile process internals | planned | APPS | 5 |
| `updates` | check (read); apply (ask_person) | `progress` | writing boot files, rollback slot | planned | SETTINGS (update page) | 5 |
| `downloads` | manifest (read); fetch, cancel (change); chunked | `progress` | the vendor URL allow list | planned | DOWNLOADS | 5 |
| `files` | list, stat (read); read as stream; write, move, delete (change) | `changed` | paths outside the jail, system files without `files:system` | planned | FILES | 6 |
| `sharing` | list, add, remove (change) | `changed` | share credentials | planned | SHARING | 6 |
| `backup` | list, run (change); restore (ask_person) | `progress` | encryption keys | planned | BACKUP | 6 |
| `cluster` | status, nodes (read); join, leave (ask_person) | `node` | cluster tokens | planned | CLUSTER | 6 |
| `git` | repos, status, log (read); fetch, pull, push (change) | `progress` | SSH keys | planned | GIT | 6 |
| `network` | interfaces, wifi list (read); connect, forget (change) | `link` | wifi passwords | planned | NETWORK | 7 |
| `firewall` | rules (read); add, remove (ask_person) | `changed` | lockdown switch | planned | FIREWALL | 7 |
| `browser` | tabs, state (read); open, close (change); screenshot as a stream | `navigated` | cookies, profile files, camera prompts | planned | BROWSER | 8 |
| `media` | now-playing (read); play, pause, queue (change) | `state` | audio device | planned | RADIO | 8 |
| `doctor` | run (change); report as a stream | `check` | raw kernel logs | planned | DOCTOR | 9 |
| `diagnostics` | summary (read); bundle as a stream (ask_person) | none | secrets stripped before any bundle | planned | DIAGNOSTICS | 9 |
| `inbox` | list (read); mark, delete, reply (change) | `new` | connector credentials | planned | INBOX | 10 |
| `notifications` | list (read); dismiss (change) | `new` | none | planned | notification bar | 10 |
| `power` | status (read); sleep, reboot, shut down (ask_person) | `state` | the power switch | planned | SETTINGS (power) | 11 |
| `time` | zone, clock (read); set zone (change) | `changed` | setting the hardware clock | planned | SETTINGS (time) | 11 |
| `display` | modes, brightness (read); set brightness (change) | `changed` | the mode switch itself, drivers | planned | SETTINGS (display) | 11 |

## 3. What is built (contract 0.1)

`iyakki-core-api::contract`: `ContractVersion`/`CONTRACT_VERSION`, `Hello`/`DomainSupport`/`ClientHello`/`negotiate`; `Transport`, `Scope` (ids),
`Risk`, `CallContext`, `guard`; `CallError`/`ErrorCode`/`IdempotencyKey`/`RateLimit`/`RateLimiter`; `Event`/`EventLog`/`EventBatch`/`Resume`;
`StreamHandle`/`Chunk`/`Streams`; `ConsentRequest`/`LocalSeat`/`Consents`; traits `Settings`, `Vault`, `Flows`; `catalog` (call table, `hello_for`).
`iyakki-core-fake::contract`: `FakeSettings`, `FakeVault`, `FakeConsents`, `FakeFlows` (they enforce the same guards, idempotency, sequence numbers and audit lines
a real machine must).
`iyakki-agent::contract_local`: `CoreSettings`, `CoreVault`, `CoreConsents`, `CoreFlows`, `CoreStreams` and `Core` (the set for one device, `core_for(&services)`).
`iyakki-agent::devmode::contract`: the dev-mode transport. Not built: any cloud relay, the accounts domain, the other planned domains.

## 4. How a slice is done

1. Write the domain's calls into `catalog::DOMAINS` (scope, risk) and this document's table; the test fails until both agree.
2. Define the trait and types in `contract/`, with `CallContext` first and `CallResult` out; no secret values; large data as streams; changes take an `IdempotencyKey`.
3. Add the fake in `iyakki-core-fake::contract` with the same guards and a test per rule above.
4. Implement it in `iyakki-agent::contract_local` (no separate crate yet), add it to the conformance suite so the fake and the real adapter are held to the same rules, migrate the named first tile through `core_for(&cx.services)`, lower the ratchet numbers where the migrated line was a violation.
5. Only then wire a transport; a transport is never added before the domain's refusal tests exist (scope missing, local-only, rate limit, audit).

## 5. Open questions for the owner

All five are **awaiting the owner**; nothing below is decided and the code does not depend on an answer (the transport that exists is
the developer-mode one, which has no relay and no queue).

1. **Relay ownership.** Who runs the cloud relay (IYAKKI as a service, a self-hosted relay a person runs, or both), how a machine is claimed by a
   cloud account (pairing needs a person at the machine, as with a computer today), and how long a relay may keep queued requests and `last_seen`.
2. **First-use consent for `vault.use_secret`.** Whether the first time a given remote client uses a given action raises a consent question on the
   machine's screen (proposed: yes; today there is no such question, and no action is registered by default, so nothing is exposed).
3. **Queued-request expiry.** Default 24 h and at most 7 days (proposed). Needed only when a relay can queue a change for an offline machine.
4. **When contract major 1 is declared.** At the first transport that ships enabled by default (proposed), or at v1.0.0. Today the contract is 0.1, the
   dev-mode transport is off by default, and a minor still only adds.
5. **Slice order.** The order of section 2 (settings and accounts, vault and consent, flows and jobs, chat and models, store/apps/updates/downloads,
   files/sharing/backup/cluster/Git, network and firewall, browser and media, doctor and diagnostics, inbox and notifications, power/time/display) is
   a proposal. Steps 1 to 3 of it (the three first domains, one tile path each, one remote proof) are done; the next slice is accounts, or whichever tile
   the owner touches next.

## 6. The local adapters and the developer-mode transport

**Local adapters** (`iyakki-agent::contract_local`, one file per domain). They call the services that exist; no behaviour moved.

| Adapter | Over | Notes |
|---|---|---|
| `CoreSettings` | the settings store (`settings.json`) | eight keys today (`time.zone`, `time.zone_auto`, `time.no_ntp`, `time.ntp_server`, `tool_log_days`, `media_download_max_mb`, `show_all_ai`, `vault.lock_on_admin_logout`); the store still validates and saves, so its own rules (a known zone, a valid host) come back as `invalid_input`. `vault.lock_on_admin_logout` is local-only. A local person who is not an administrator reads but cannot write. A change made behind the adapter's back (a screen) becomes a `changed` event by `machine`. No key can hold a secret. |
| `CoreVault` | the person's own vault | status (locked, how many entries), unlock (local seat; the login password), lock, `use_secret` for a named action registered on the machine (`register_action`). The action gets the secret; the caller gets words back, scrubbed if the secret appears in them. A remote call acts for the account its computer is paired to. No method returns a secret (a test reads the file). No action is registered by default. |
| `CoreFlows` | one account's flows engine | list, start (idempotent; the engine's key window), stop, status, events, approve. The engine has no event log, so the adapter derives `started`, `step`, `approval_needed`, `approval_answered`, `finished` by comparing runs; every call catches the log up first; runs that existed when the adapter was made are the baseline. `blocked` ("needs you") is `waiting_approval`. No approval a flow can ask today is local-only, so the local-only rule is proved against the fake and in the shared code path, not against a real camera approval. `hand_frame` stores a picture behind a stream handle and announces it as a `frame` event. |
| `CoreConsents` | memory | the queue of questions for the person at the machine; they expire after 5 minutes; answering needs a `LocalSeat`. Nothing raises a question yet except a remote `vault.unlock` attempt. |
| `CoreStreams` | memory | chunks of at most 16 KiB by offset, scope of the call that made the handle, ten-minute handles, 64 handles and 64 MiB at most (`busy` beyond). |

**Developer-mode transport** (`iyakki-agent::devmode::contract`, listener hook in `iyakki-remote::devserver`). Opt-in twice and off by default:
an administrator switches it on for the device (`DevMode::set_core_enabled`) and grants contract scopes to each paired computer
(`DevMode::set_core_scope`; stored in `<data>/devmode/core.json`). A newly paired computer holds none. The scopes `settings:read`, `settings:write`,
`vault:status`, `vault:lock`, `vault:use`, `flows:read`, `flows:run`, `flows:approve` and `consent:read` can be granted; `local` cannot.
Routes (signed requests as today): `POST /v1/dev/core/hello`, `POST /v1/dev/core/call` (`{domain, call, args, contract?}` answered
`{ok, result}` or `{ok:false, error}`), `POST /v1/dev/core/stream/read` (`{id, offset, max_len}`), `POST /v1/dev/core/stream/close`.
`watch` and `events` take `resume` and may wait up to 10 s for news (`wait_ms`). `vault.unlock` and `consent.answer` always answer
`needs_consent`; an unlock attempt also raises a `vault_unlock` question on the machine's screen (one at a time). Each computer is limited
by the `rate_limit` of `hello`. Every call goes to the developer-mode activity log, and every non-read call to the system audit log.

