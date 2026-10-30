# Minimal sets as a web service: scoping and plan

Status: planning written 2026-10-25; the first slice (`agent-api` and the limited `cloud` profile) was built on 2026-10-27, see section 9. Sections 0-8 are the plan as written. Backlog item: `docs/project/BACKLOG.md`, v1.0.0 list, row 2
("option to run a minimal set of IYAKKI as a web service"). Read with `UI-OS-SPLIT-PLAN.md`, `../EMBEDDING-AND-BACKEND.md`, `../CLOUD-MODE.md`,
`../REMOTE-PROTOCOL.md`, `../DEVELOPER-MODE.md`, `SDK-API.md`.

## 0. What the request can mean, and what exists already

The request is ambiguous. Three readings, all kept useful below:

| | Reading | Short name |
|---|---|---|
| A | A small chosen subset of capabilities (agent chat, flows, data tools, tile runtime, MCP) as a headless service with HTTPS and WebSocket APIs and a web UI; no framebuffer, no hardware tiles | **service profiles** (sections 1-2) |
| B | A web-served UI for a chosen subset of tiles (today's pixel stream, or a real web front end) | **tile subset in the browser** (profile `full-ui-remote`, plus a later web front end) |
| C | IYAKKI components run as pods in a managed or customer Kubernetes cluster | **deployment forms** (section 3) |

Recommendation: do A first, deliver it in the forms of C, and treat B as what A's API makes possible later. Reason: A is the only reading that
forces the UI/OS boundary to carry real traffic (section 5), and the packaging for C exists already.

What exists today (verified in the tree):

* `iyakki serve --web[=ADDR]` / `iyakki web [ADDR]` / `IYAKKI_WEB`: the **whole shell** streamed as pixels to a browser by `crates/iyakki-remote`
  (4,238 lines): own HTTP/WebSocket server, rustls (ring) TLS with a made or supplied certificate, machine accounts sign-in, `HttpOnly` +
  `SameSite=Strict` + `Secure` cookie, CSRF token on the WebSocket and sign-out, 10 sign-in tries a minute per address, at most 32 sessions and 128
  connections, `/healthz`. Routes: `/`, `/decoder.js`, `/healthz`, `/api/session`, `/api/login`, `/api/logout`, `/ws`, `/login/..`.
  `iyakki-remote` depends on `iyakki-shell`, so the web server carries the whole UI (about 226k lines) even for an API-only use.
* `dist/Dockerfile` (musl, `FROM scratch`, user 65532, `CMD serve`, `VOLUME /var/lib/iyakki`, port 8443), `dist/k3s-web.yaml` (namespace, 2 Gi PVC,
  `Recreate`, one replica, init container adds the first account from a Secret, `runAsNonRoot`, `readOnlyRootFilesystem`, all capabilities dropped,
  seccomp `RuntimeDefault`, HTTPS probes on `/healthz`, requests 100m/128Mi, limits 2 CPU/1Gi), `dist/systemd/iyakki.service` (hardened unit).
  There is no Helm chart and no `/metrics`.
* Cloud mode (`cloudmode.rs`): hides hardware tiles (MEDIA, RADIO, TALES, VOICE, CAMERA; sound, display, keyboard, Bluetooth settings pages);
  Automatic when no display/keyboard/sound or `VARIANT=cloud*` in `/etc/iyakki-release`. It is a **hiding** table over a full shell, not a separate
  build. No cloud image variant exists (Buildroot configs: x86_64, aarch64, and fragments `web`, `media`, `apple`, `mac`, `cluster`, `firmware`, `nvidia_open`).
* Developer-mode TLS server (`iyakki-remote/src/devserver.rs`, port 7979, off by default, auto-off timer): pairing code, ed25519-signed requests,
  11 scopes (`install debug files files:system config input input:password shell power update forward`) decided per paired computer on the device,
  activity and audit log, ~60 routes under `/v1/dev/*` (`devmode/api.rs` is the route-to-scope table).
* A precedent for "a small headless HTTP service in this tree": `iyakki store-server` (`store/server*.rs`, ~860 lines, `httpmini.rs`, one static binary,
  about 1 MB per `dist/store-server/Dockerfile`, `FROM scratch`, user 10001, port 8080, bearer publish token, `--read-only`).
* Pieces that are already callable without a screen: `agent::start(backend, tools, history, limits)` (the run loop, returns a `RunHandle`),
  `RunStore::{pending_approvals, decide_pending}` (approval cards as data: `PendingApproval` has run, call, tool, summary, detail, risk `low|medium|high`),
  `FlowEnv::{build, summaries, import_json, export_json}` and the `iyakki-flow` engine, `datatools::{defs, call}`, `store/tilesdk.rs` (the `window.iyakki`
  backends, each a trait with a fake, `FakeLlm`), `mcp.rs` (MCP **client** over stdio, servers from `/etc/iyakki/mcp.json`; the bridge
  `iyakki --mcp-tools <socket>` serves a chat run to external agents). There is **no MCP server over HTTP** and no REST API for chat, flows or data.
* Not yet separated: `accounts.rs` and `AuditLog` live in `iyakki-shell`, so any server that signs people in with machine accounts links the shell
  crate (as `iyakki-remote` does). `iyakki-agent` itself links `iyakki-webview`, `iyakki-usb`, `iyakki-drivers`, `iyakki-devctl`, `iyakki-storage` (UI-OS-SPLIT
  step 7 moves those out). The whole `iyakki` binary in `target/release` is 46.4 MB (stripped, LTO, default features `swarm egress shell web`; a build
  of 16 Oct, libc linkage not checked).

## 1. Profiles

A profile is a **named list of services and routes switched on at start**. One binary, many profiles; choosing one never adds hardware access.

| Profile | Includes | Excludes | Needs (crates) | Data under `/var/lib/iyakki` |
|---|---|---|---|---|
| `agent-api` | sign-in, sessions; chat runs (`agent::start`, `assistrun`), run history (`RunStore`), approvals, providers/models, usage, vault by reference; read tools (`webtools`, `webresearch` through the egress policy), `datatools`; MCP **client** servers from the admin's list | flows, tile runtime, shell, any device tool (`audioout`, `camtools`, `disks`, `devtools`, `k8s`, `ssh`, `run_command`, `terminal`) | `iyakki-agent`, `iyakki-sec`, `iyakki-proxy`, `iyakki-data`, accounts | accounts, settings, runs, vault, audit, usage |
| `flows-runner` | `agent-api` minus chat UI plus `FlowEnv`: list/import/export/validate flows, start/stop runs, scheduler, flow approvals and `askform` | tile runtime, shell | + `iyakki-flow`, `iyakki-js` (expression nodes) | + `flows/` |
| `data-api` | `datatools` + `iyakki-data` (DuckDB/Parquet/CSV behind the user's jail), per-user workspaces, no model | chat, flows | `iyakki-data`, accounts | + workspaces |
| `tile-host` | the web-bundle **tile runtime** (`store/tile.rs`, `tilesdk.rs`, `tilehub.rs`): install/list/run/stop installed tiles, serve their page and the `window.iyakki` bridge over WebSocket; private store client | Rust tiles (they draw with `Gfx`), shell | + `iyakki-js`, store modules | + `store/`, per-app data |
| `full-ui-remote` | today's `iyakki-remote` pixel stream of the shell, cloud mode on | nothing hidden except hardware tiles | everything (`iyakki serve --web`) | everything |

Composition: profiles are sets, so `--profile agent-api,flows-runner` is allowed; `full-ui-remote` is exclusive. Defaults when none is named: today's
behaviour (`full-ui-remote` under `--web`), so nothing existing changes.

**How a profile is chosen.** Three layers, each simpler than the last to build:

1. **Runtime** (first): `iyakki serve --web --profile agent-api` or `IYAKKI_PROFILE=agent-api`. One binary, routes and services registered by name; unknown
   names are an error; a route of a switched-off profile answers 404 and is never routed. Cheapest, testable with fakes, no new build.
2. **Image/container**: the same binary with the profile in the image's `CMD`/env; one tag per profile is optional, one image is enough.
3. **Cargo features** (later, for size and attack surface): `headless` drops `shell`, `web` and `iyakki-webview`; it needs UI-OS-SPLIT steps 5-7 and
   accounts moved out of the shell crate. Only then does the binary of `agent-api` get meaningfully smaller than 46 MB (not measured; do not promise a number).

**Relation to cloud mode.** Cloud mode stays a UI-hiding table for the full-ui profile. Every other profile has no shell, so it needs no hiding; it sets the
mode "cloud" for the services that read it (sound probe and camera look stay off). A cloud image variant is only `VARIANT=cloud-<profile>` in the release file.

**Footprint (what is measurable now).** Release binary 46.4 MB; `FROM scratch` container adds a few KB of folders; private store server about 1 MB (the
size a minimal HTTP-only static binary reaches in this tree, a floor and not an estimate for any profile here); k3s manifest requests 128 Mi and caps 1 Gi
for the full UI; a pixel-stream viewer costs about 100 MB at 4K. Per-profile idle RSS and image size: **not measured**; step 1 of section 6 adds a script
that records them (RSS after 60 s idle, image size, start time), as RELEASE-READINESS item 11 already asks for.

## 2. API surface

One definition, JSON over HTTPS plus WebSocket (UI-OS-SPLIT decision 7, still open). Base `/api/v1`. Everything needs a session unless marked.

| Profile | Routes (sketch) |
|---|---|
| all | `GET /healthz` (no auth), `GET /readyz` (no auth; 503 until the data dir is writable and the profile's services started), `GET /metrics` (admin or a metrics scope; Prometheus text), `GET /api/v1/openapi.json`, `GET /api/v1/whoami`, existing `/api/login`, `/api/logout`, `/api/session` |
| `agent-api` | `POST /chat/runs` (prompt, provider, model, tool set) -> run id; `GET /chat/runs/{id}` ; `GET /chat/runs/{id}/events` (WebSocket: text deltas, tool calls, results, `approval` objects, usage); `POST /chat/runs/{id}/stop`; `GET /chat/runs` (own history); `GET /approvals`; `POST /approvals/{run}/{call}` `{verdict}`; `GET /models`; `GET /usage` |
| `flows-runner` | `GET/POST /flows`, `GET /flows/{id}` (export), `PUT /flows/{id}` (import, validated by `issues`), `POST /flows/{id}/runs`, `GET /flow-runs/{id}`, `POST /flow-runs/{id}/stop`, `POST /flow-runs/{id}/form` (answers an `askform` node), events WebSocket |
| `data-api` | `GET /data/sources`, `POST /data/query` (read-only SQL with row and byte caps), `GET /data/files` (own jail), `POST /data/import` (size-capped) |
| `tile-host` | `GET /tiles`, `GET /tiles/{id}/app` (the page, strict CSP, own origin or sandboxed iframe), `WS /tiles/{id}/bridge` (the `window.iyakki` calls, permissions from the tile's manifest), `POST /tiles` (install package, same review path as the STORE) |
| MCP bridge (flag on `agent-api`) | `POST /mcp` Streamable-HTTP style JSON-RPC exposing the profile's tools to an external agent, each call passing the **same gate** (policy, approval) as a chat run. Design only; protocol version to be confirmed when built. |

**Auth: nothing new.** Browser clients: the existing machine-account sign-in, cookie and CSRF (header `X-CSRF` on every `POST`/`PUT`/`DELETE`, token in the
WebSocket query as today). Programs: developer-mode style pairing, ed25519-signed requests and per-computer scopes. New **scope names** only, added to the
`Scope` table (so they are shown, granted, revoked and audited by existing code): `api:chat`, `api:flows`, `api:data`, `api:tiles`, `api:metrics`. A pairing
carries none of them by default (the default stays `install`). A request can never widen its own scopes (existing rule). Pairing needs someone to read a
code on a screen, which a headless host lacks; the first cut is **`iyakki dev pair --code-stdout` run by the operator inside the container** (or the existing
`user add --password-file` pattern) and is a decision for the owner (section 7).

**Multi-user and roles.** Accounts, roles and `Policy` (`allows_app`, `allows_mcp`, `manage_skills`) apply per request exactly as in the shell; runs, flows and
data are per user already (`RunStore::load(user, id)`, `datatools::service_for(user)`). Administrators see only admin routes (`/metrics`, tile install for all,
MCP server list). **Rate limits:** sign-in 10 a minute per address (exists); add per-session request budgets (default 60 a minute, 4 concurrent runs per user),
body caps (1 MiB JSON, 8 MiB uploads), and the existing `RunLimits` and usage budgets for model spend. **Audit:** every state-changing route and every approval
decision goes to the existing `AuditLog` with `src` (`api:<scope>`, using the attribution list in SDK-API section 10) and client address.

**Schema and front ends.** Request/response types are serde structs in one new crate `iyakki-api-types` (plain data, no IO; or a module of `iyakki-core-api`,
which already holds `ApiError` with stable codes `not_found denied unsupported invalid_input busy io`). Generate JSON Schema from them with a build-time
derive **written in-house or an Apache-2.0/MIT crate (`schemars` is MIT; check `deny.toml` and the tree before adding)**; assemble an OpenAPI 3.1 document in a
test that fails when the checked-in `docs/api/openapi.json` is stale (same style as `feature_matrix`). A React or other front end then generates its
client from that file (any permissive generator, outside the repo's build). The WebSocket events are described as a JSON Schema `oneOf` keyed on `t`, as
`REMOTE-PROTOCOL.md` already does for the pixel stream.

## 3. Deployment forms

| Form | State | What to do |
|---|---|---|
| Single static binary | `iyakki` (musl build via the Dockerfile) | add the profile flag; document the unit file; the hardened `dist/systemd/iyakki.service` is the supervisor-less-systemd answer; **systemd-less**: run under any supervisor (runit, s6, container runtime) as the binary is one foreground process; `serve` already refuses PID-1 boot logic when `/proc/self` exists |
| Container, rootless, read-only fs | `dist/Dockerfile` exists | `USER 65532`, volume `/var/lib/iyakki`, `/tmp` emptyDir; add `HEALTHCHECK`-equivalent by `readyz`; one image, profile by env |
| Cloud image variant | none | `VARIANT=cloud-<profile>` release file + Buildroot fragment `iyakki_cloud.fragment` (no graphics/audio packages) over the existing x86_64/aarch64 defconfig; incremental build only (owner rule); lowest priority, containers cover most uses |
| Kubernetes | `dist/k3s-web.yaml` (single manifest) | step: split into `dist/k8s/` base manifests (Deployment, Service, PVC, NetworkPolicy deny-all-egress except model/egress hosts, PodDisruptionBudget not needed for 1 replica); a Helm chart is a thin wrapper (values: profile, image, storage, ingress/TLS, existing Secret names) and is optional |

**Persistence.** One data directory (accounts, settings, runs, flows, vault, audit, store). One writer: `replicas: 1`, `Recreate` (as now); no shared
writers until the data layer is made multi-process-safe, which is not planned. Vault: stays on the volume, encrypted as `LOCK-AND-ENCRYPTION.md` says;
cloud keys are referenced, never copied into responses. Backup = snapshot the volume. **Upgrades:** image tag change with `Recreate`; the data format
versions already in the files are the contract; add a startup check that refuses a data dir written by a newer version rather than migrating down.
**Health/readiness:** `/healthz` (process alive, exists), `/readyz` (new). **Metrics:** `/metrics` Prometheus text written by hand (a counter/gauge
writer is ~100 lines; no crate): sessions, requests by route and status, runs started/finished/denied, approvals pending, tool calls by outcome, model
tokens, flow runs, process RSS, data-dir free bytes. **Logs:** `IYAKKI_LOG` to stderr, one JSON object per line in service mode, secrets and prompts
never logged at `info` (the existing credential masking in `toollog` applies).

## 4. Security gate for v1.0.0

A profile may ship only when every row has a test that fails if the property is lost.

| Threat | Control | Refusal test |
|---|---|---|
| Cleartext credentials | TLS only. Plain HTTP only with explicit `http:PORT` behind a proxy, then `Secure` cookies still set and a `X-Forwarded-Proto` check; default bind is HTTPS | start with no cert makes one; plain listener without the explicit flag is refused |
| Hardware access | service profiles do not link or call `iyakki-devctl/usb/drivers/netscan`; seccomp `RuntimeDefault`, no devices mounted, `PrivateDevices` | `boundary`/ratchet test: the profile's route table never reaches a tool in the device group; tool list of `agent-api` is asserted by name |
| Tool abuse by the model | approvals as data (`PendingApproval`: tool, summary, detail, risk). Risk `high` (`mcp__*`, `run_command`, `write_file`, `http_request`, `terminal`, deletes) is **off by default** in service profiles and needs an admin to enable per tool; the verdict comes only from the same user (or an admin) through `POST /approvals`, never from model text; pending items time out to "refused" | a run waits and times out refused; another user's verdict is 403; a verdict for an unknown call is 404; replay of a verdict is 409 |
| Secrets in responses | responses are typed structs with no secret fields; provider keys write-only (as `config/provider`); vault values never returned; tool results pass the existing credential masking | a test greps every route's golden response for planted secrets (key, token, password) |
| SSRF / egress | all outbound HTTP through `iyakki-proxy` `EgressProxy` host allow-list (`allow_host`); block loopback, link-local, metadata (169.254.169.254), private ranges and redirects to them, DNS-rebinding by resolving once and pinning; in k8s a `NetworkPolicy` as a second layer | requests to `127.0.0.1`, `169.254.169.254`, `10.0.0.1`, a redirect to each, and a rebinding host are refused |
| CSRF / XSS / clickjacking | cookie + CSRF header (exists), `Origin` must match, strict CSP, tile pages in a sandboxed iframe with own origin, `frame-ancestors 'none'` on the API | cross-origin POST without token refused; tile page cannot read the parent's cookie |
| Brute force, DoS | 10 sign-ins a minute per address (exists), lockout of accounts (exists), session and connection caps (32/128), body and frame caps, per-user run and concurrency limits, request timeouts, container memory/CPU limits | oversized body, 5th concurrent run, slow-loris header, 33rd session refused |
| Privilege escalation across users | every handler takes the session's user and `Policy`; ids are looked up inside the user's own store only | user B asks for user A's run, flow, data file, tile data: not found |
| Over-broad programmatic access | scopes per paired computer; none by default; revoke ends sessions at once (exists) | missing scope 403; revoked key 401 mid-stream closes the socket |
| Audit gaps | every mutation and approval logged with account, source, address; log capped and rotated | audit line exists for each mutating route (table-driven test over the route table) |
| MCP bridge | off by default, bound to the same gate; tool names `mcp__<server>__<tool>` keep the high risk class | an external call to a high-risk tool without approval is refused |
| Supply chain | any new crate permissive per `deny.toml`; `cargo deny check` in the step's checklist | CI |

Not claimed: a third-party security review (UI-OS-SPLIT risk list already requires one before any remote transport is enabled); multi-tenant isolation
between customers (one pod per tenant is the model, not shared processes); denial of service by a model provider.

## 5. Relation to the UI/OS split

* The profile server is the **first real consumer of `iyakki-core-api`**: handlers take `Arc<dyn Os>`/service traits, so tests run on `iyakki-core-fake`
  (`FakeOs`) and a headless build never names a kernel crate. It is also the **first consumer of the connector idea** in the other direction: a React UI in a
  cloud talks to this API, which is the "remote connector" transport of step 9 for service-level calls (chat, flows, data), not for hardware calls.
* Needed from the plan: steps 1-4 are done and enough for the first cut. Step 5 (`Os`/`Sysinfo`, `AppCx.os`) is only needed for `/readyz` host info and the
  SYSTEM-like read routes. Steps 6-7 (extract `iyakki-core-local`) are needed for the **smaller binary** (cargo feature `headless`), not for correctness. Step 8
  (split `Services`) is the real enabler for per-profile construction: today `Services::open` builds a 40-field god object, so the first cut builds all of it
  and exposes only the profile's routes. Step 9 (dispatcher `(domain, method, JSON) -> JSON | ApiError`, scopes, refusal tests) is the same work as section 2
  and should be **done once**: this document's route layer should become the HTTP transport of that dispatcher, not a second mechanism.
* Least code first: a module `api` in `iyakki-remote` (it already has the server loop, sessions, CSRF, TLS, `httpmini`, the dev server's pairing) with
  3 routes over services already in the tree (`agent::start`, `RunStore`, `FlowEnv`), a fake model (`FakeLlm`) in tests. No new dependency. Accept for now
  that it links the shell. Move to its own crate (`iyakki-service`) when accounts leave the shell crate.

## 6. Implementation plan (fakes first, each step builds and keeps tests green)

Sizes: S under 300 changed lines, M 300-1500, L over 1500. One commit per step; no whole-shell test runs (filter by module).

| # | Step | Size | Proof |
|---|---|---|---|
| 1 | Measure: script that builds the current release binary and image, starts `serve --web` in a container, records binary size, image size, start time, idle RSS (60 s) | S | numbers in this document; first budgets |
| 2 | Route table + profile registry (`--profile`, `IYAKKI_PROFILE`, `readyz`, 404 for off routes), no handlers yet; all-profiles default keeps today's behaviour | S | unit tests: table lists each route with profile and scope; unknown profile errors |
| 3 | Typed API structs + error mapping (`ApiError` codes to HTTP status) + OpenAPI generator + stale-file test | M | golden `openapi.json`; schema round-trip tests |
| 4 | `agent-api` read side: `whoami`, `models`, `usage`, `chat/runs` list/get from `RunStore` | S-M | tests on a temp data dir; other user's id not found |
| 5 | `agent-api` run side: start a run with `FakeLlm`, events WebSocket, stop; per-user concurrency cap | M | scripted fake model; cap and body-limit refusals |
| 6 | Approvals as API objects: list, decide; timeouts; high-risk tools off by default | M | the approval refusal tests of section 4 |
| 7 | Metrics, structured log mode, audit for every mutation (table-driven) | S-M | route-table test; metric names golden |
| 8 | `flows-runner`: list/import/export/start/stop, form answers | M | `iyakki-flow` fakes; existing flow tests unchanged |
| 9 | `data-api` with the data jail and caps | S-M | planted-secret and path-escape tests |
| 10 | Scopes `api:*` in the dev-mode table + signed-request path for programs | M | scope tests next to `devmode/tests.rs`; revoked key closes stream |
| 11 | SSRF/egress refusal suite for service profiles (section 4) | M | the listed hostile hosts |
| 12 | Container: profile env, read-only rootfs run, `dist/k8s/` manifests with NetworkPolicy; smoke test with the fake model in a local container | M | smoke script; manifest lint (`dockerlint.rs`/`deploylint.rs` exist) |
| 13 | `tile-host` (needs tile runtime off the shell: check `store/tile.rs` for shell types first) | L | `tile_test` headless harness |
| 14 | MCP HTTP bridge; cloud image fragment; Helm wrapper; headless cargo feature after split steps 5-8 | L | per item |

Rough effort for steps 1-7 (the first useful profile, `agent-api`): about 2-3 weeks for one engineer; whole list 8-10 weeks. These are estimates from
comparable steps in `EMBEDDING-AND-BACKEND.md` (C4 8-10 days, C5 10+ days), not measurements.

**Risks.** The route layer becomes a second mechanism beside the dispatcher of step 9 (mitigation: one table, same types); `Services` coupling makes every
profile pull the whole object and its start-up cost (mitigation: measure in step 1, split in step 8); blocking calls inside handlers (use the poll/stream
model already used by `RunHandle`); a long-lived run holds a thread (cap); tile pages are untrusted code (sandbox); the single-writer data dir limits
scaling (documented, one pod per tenant); `iyakki-remote` linking the shell inflates the first cut.

## 7. Decisions for the owner

1. Interpretation: A first, delivered as C, B later (recommended), or B first (cheap: it is `full-ui-remote` plus a docs page, and already works)?
2. First profile: `agent-api` (recommended: smallest, highest value for cloud, uses approvals and the egress policy) or `flows-runner`.
3. Protocol: JSON over HTTPS plus WebSocket (recommended, = UI-OS-SPLIT decision 7), MCP bridge later and off by default.
4. Auth: reuse sign-in/CSRF for browsers and signed pairing with new `api:*` scopes for programs (recommended); how a headless host pairs (operator command
   in the container, or a one-time code written to the log with an explicit flag).
5. Delivery: container first, the static binary second, cloud image variant last (recommended); Helm chart wanted, or plain manifests enough?
6. Where the first cut lives: module in `iyakki-remote` (least code, links the shell) or new crate `iyakki-service` (needs accounts moved out of the shell first).
7. High-risk tools in service profiles: off by default (recommended); may an admin enable them per tool?
8. New crates: none required. If a schema generator is wanted, `schemars` (MIT) is a candidate; allowed licences are in `deny.toml` (Apache-2.0, MIT,
   BSD-2/3, ISC, Unicode-3.0, Zlib, CC0, 0BSD, Unlicense, BSL-1.0). In-house is the project preference.
9. Multi-tenant: one pod per tenant (recommended) versus shared process (not planned).

## 8. What could not be verified

No profile was built or run. Binary and image sizes quoted are existing figures (46.4 MB binary from a 16 Oct build, `FROM scratch` and store server sizes
from comments in the Dockerfiles, install media sizes from `INSTALL.md`); per-profile RSS, start time and size are unknown. Whether `iyakki-remote` can run
without the shell's render path, whether `store/tile.rs` and `FlowEnv` depend on shell types, the exact contents of the 11-field `Scope` plumbing for new
scopes, and how `Services::open` behaves with no display were read at signature level only. The MCP transport version and `schemars`' current licence and
version were not checked. The in-tree files were being edited by others while this was written.

## 9. What is built (2026-10-27, first slice)

The owner accepted the recommendations of section 7 (decisions 1-9) on 2026-10-27: reading A delivered as C, first profile `agent-api`,
JSON over HTTPS, browsers and programs sign in with the machine accounts, container first, high-risk tools off, no new crates beyond
`rcgen` (already in the tree through `iyakki-remote`), one pod per tenant. Added by the owner: a **limited cloud deployment** whose way
to interact with it is this service (profile `cloud`).

**Where the code is** (decision 6 was "module in `iyakki-remote` or new crate"; it went to neither, to keep `iyakki-remote` and the
shell out of the service layer):

| Piece | File | Notes |
|---|---|---|
| Traits for accounts and audit | `crates/iyakki-core-api/src/service.rs` | `Principal`, `Authenticator`, `AuditSink`; in-memory `FakeAccounts` in `iyakki-core-fake` |
| Service layer | `crates/iyakki-agent/src/webservice/` | `routes.rs` (the one table), `openapi.rs`, `server.rs` (listener and pipeline), `handlers.rs`, `runs.rs`, `tools.rs` (profile filter), `auth.rs` (tokens, limits), `tls.rs`, `config.rs`, `deps.rs` (the traits handlers see), `real.rs` (over `Services`), `tests.rs` |
| Wiring | `crates/iyakki-core/src/svc.rs`, `main.rs` (`serve --profile`) | adapts the shell's `UserStore`/`AuditLog` and the firewall engine to the traits; no logic |
| Startup registry | `startup.rs` row `service` ("Web service") | starting, ready, failed (with the reason), stopped |
| Config examples | `dist/service/service.local.json`, `service.cloud.json` | the cloud one is parsed and checked by a test |

**Design as built.** One route table says which routes exist, for which profile, who may call them (public, signed in, administrator), which
app permission they need and whether they are audited. The dispatcher and the OpenAPI 3.1 document are both generated from it. The
pipeline per request: HTTPS only (a plain connection gets no answer) -> body cap in the parser -> `Origin` check -> per-address limit ->
route lookup in the running profile (else 404, 405 for a wrong method) -> bearer token -> account looked up again (a removed account ends its
tokens) -> administrator and app-permission checks (403) -> per-person limit (429) -> handler -> audit line -> response. Handlers see
only `Deps` (accounts, audit, `RunStore`, a `ChatBackend`, a `ToolSource`, `Extras`): no shell type, no kernel crate, no device.

Routes: `GET /healthz`, `/readyz`, `/api/v1/version` and `POST /api/v1/login` are public; signed in: `logout`, `openapi.json`, `whoami`,
`models`, `tools`, chat runs (`GET/POST /chat/runs`, `GET /chat/runs/{id}`, `/events?after=&wait=`, `POST /chat/runs/{id}/stop`),
approvals (`GET /approvals`, `POST /approvals/{run}/{call}`), administrators `GET /admin/status`; profile `cloud` adds `GET /flows`,
`GET /flows/{id}`, `POST /data/call`.

Decisions taken where the plan gave none (the safest option each time; change any of them by editing one line):

1. **Tokens, not cookies.** Browsers and programs both use `Authorization: Bearer` tokens from `POST /api/v1/login` (machine account and
   password, 256-bit random token, only its SHA-256 kept, 8 h idle, 24 h absolute, at most 256). No cookie means no CSRF surface.
   Developer-mode pairing with `api:*` scopes (section 2) is **not** built; it needs a screen or `--code-stdout` and is a later step.
2. **Tool lists by name** (`Profile::tools`): `agent-api` has 12 read tools, `cloud` adds 4 read-only flow tools. Anything else is not
   shown to the model and a call to it is refused without running. `enable_tools` is accepted by the config but the list of tools an
   administrator may enable is empty in this slice (`high_risk_enableable`), so no high-risk tool can be switched on yet. The `cloud`
   profile additionally drops any tool that needs local hardware (`tool_hardware`, tied to `cloudmode::Need`).
3. **Approvals:** `auto` is false and cannot be set; no "always" or "session" verdict exists over the API; grants from the desktop do not
   apply (`ProfileTools` overrides `pre_approved`, `approved_always`, `grant_blocker`); timeout 300 s (config) means refused; replay is 409,
   an unknown call 404, another person's call 403, an administrator may answer for anyone.
4. **LAN opt-in** needs `"lan": true` and `allow_from` networks; `0.0.0.0/0` and `::/0` are refused; one firewall rule per network for the
   port (`FirewallOpener`, comment "IYAKKI web service", replaced on each start, kept at once). If the rule cannot be added the service
   does not listen. A rule left from an earlier LAN start stays until removed in Settings, Firewall.
5. **Events by long poll** (`?after=N&wait=S`, at most 25 s) instead of a WebSocket in this slice.
6. **Cloud profile = `agent-api` + flows read + data tools.** It is cloud mode applied to the service: its apps (Chat, Flows, Data) are
   `Class::Remote` in `cloudmode.rs` (a test), no route or tool names hardware, and nothing of developer mode, firmware, USB, firewall,
   sound or camera exists in it.
7. **The service links the shell crate for accounts** (decision 6 of section 7, "accept for now"), so the binary is the same one.

**Binary size.** The profile is a runtime choice, so the profile "build" is the same binary. Measured: `target/release/iyakki` of
16 Oct (before this slice, default features) is 46,385,464 bytes. No release build was made for this slice (owner rule: no release
build while the image engine builds), so the size added by about 1,700 lines of service code is **not measured**; `rcgen` and `rustls`
were already linked. A smaller binary needs the `headless` cargo feature of section 1, which needs accounts moved out of the shell
crate (UI-OS-SPLIT steps 5-8) and is not started.

**Tests** (`cargo test -p iyakki-agent --lib webservice`, 28; `cargo test -p iyakki-core --lib svc`, 3): route table consistency and
public list; OpenAPI covers every route of each profile and nothing else, and every named schema exists; every route has a handler;
no token / bad token -> 401; no default account, one answer for wrong name and wrong password; guest and non-administrator -> 403;
route or tool outside the profile -> 404 (also camera, audio, usb, firmware, firewall, dev install, shell, metrics paths, with and without
a token); removed account ends tokens; approvals (wait, owner only, replay 409, unknown 404, bad verdict 400, administrator may answer,
denied call never runs, desktop grant ignored, timeout refuses and a late answer is 409); per-person run cap and isolation; rate limits;
body cap (parser and over TLS); CORS locked, own origin, listed origin, preflight; JSON content type; audit line for every audited route
and no password, token, prompt or file name in the log; no secret fields in responses; cloud profile hides every hardware route and tool
(and a hardware tool a model asks for is refused and never runs); the exact default tool list; config checks (loopback default, LAN
opt-in, typos, half a certificate, zero limits, example file); `run` asks the firewall only for a LAN start, reports to the startup
registry and refuses to listen when the firewall refuses; TLS pin (right fingerprint accepted, wrong refused, plain HTTP not served,
certificate kept with a private key, own certificate used); a client signs in, chats and signs out over TLS on `127.0.0.1:0`;
readiness 503 without a reason in the body; graceful shutdown; token store hashed, 0600, restart, expiry, revoke; strict route matching.

**Not built** (still the plan above): `flows-runner` write side, `data-api` and `tile-host` profiles, `/metrics`, the WebSocket
stream, developer-mode pairing and `api:*` scopes, MCP bridge, the SSRF refusal suite for the service's web tools (the tools use the
existing egress policy; the hostile-host tests of section 4 were not written for this route), structured JSON logs, a `headless`
feature, container manifests, a Helm chart, per-request token budgets beyond the role's `AgentBudget`.

**Unverified.** `iyakki serve --profile ... ` was exercised only through `--check` and the unit and in-process tests; the real
`Services` implementations (`real.rs`: models, flows, data, tool construction, readiness probe) compile but have no test against a real
`Services`, and the firewall rule was built as data but not applied to a kernel. No run with a real model provider, no container run, no
load test, no browser client. The `boundary` test currently fails on `crates/iyakki-agent/src/clis.rs` (a row in BOUNDARY.md for a
file that no longer spawns), which is another change and not this one; the new files add no process spawn or system call.
