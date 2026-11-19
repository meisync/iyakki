# Cloud topology: separate control, workflow and execution planes, and a shared database

Status: PLAN only (2026-11-04). Nothing here is built. Owner idea: deploy separate clusters (or node pools, or namespaces) for (A) a
SANDBOX that runs untrusted work, (B) a CONTROL cluster for identity and secrets, (C) a WORKFLOW cluster that runs schedules and keeps
approvals and notifications in a SHARED DATABASE (DuckDB, NoSQL or an RDBMS). Read with `CLOUD-MULTITENANT-PLAN.md`,
`TENANT-CONFIG-PLAN.md`, `FLOW-AUTOPILOT-PLAN.md`, `../CLOUD-MODE.md`. The plans `CLOUD-FLOWS-PLAN.md`, `NOTIFICATIONS-PLAN.md` and
`CLOUD-SANDBOX-PLAN.md` did not exist when this was written (other agents were writing them); this plan was based on the code that
exists (`cloudflows/`, `notifyhub/`, `tenantconfig/`, `cloud*.rs`, `inbox.rs`) and must be re-read against those plans before step 1.
Example topology file: `dist/cloud/topology.example.yaml`.

Words: SURE = read in the code or a property of the technology I am confident about; GUESS = an estimate or a judgement to check.

## 0. The recommendation in short (revised after the owner's decisions of 2026-11-04)

1. Name three PLANES (control, workflow, execution). They are a code boundary first and a deployment choice second.
2. Put every piece of persisted workflow state behind STORAGE TRAITS (crate `iyakki-store`, the seam). `FileStore` (today) stays the default and
   `single` changes nothing for anyone. This refactor is the real first step and is worth doing even if split never ships.
3. DECIDED: PostgreSQL is the shared database; NO in-house client. Use the famous permissive sync client family (`postgres` + `tokio-postgres`,
   MIT OR Apache-2.0) with a rustls adapter and `r2d2_postgres` pooling, pinned, in `PgStore` only (section 2.4). DuckDB for reports only;
   SQLite optional for `single`.
4. DECIDED: the operator runs the database; an optional in-cluster PostgreSQL (StatefulSet + PVC + backup CronJob) is outlined (section 2.7).
5. DECIDED: the control plane SCOPE is configurable: `control.scope: installation | tenant | team`, recommended default `tenant` (section 1.5).
6. DECIDED: execution targets use the same cluster-binding abstraction as `k3s.cluster`; kept separate in v1, converging later (section 4).
7. DECIDED: when control is down, runs are QUEUED durably and DELIVERED LATE WITH A WARNING (`late_by`), not just blocked (section 3.4);
   the dashboard shows plane health and admins act on it; a chat agent only diagnoses and PROPOSES (section 3.5).
8. Three profiles, one topology file: `single`, `split2` (control + execution), `split3` (control, workflow, execution).
9. Planes talk through one versioned internal API over mutual TLS with per-plane identities and short-lived signed run tokens.
10. The execution plane never holds a vault key, a database credential or a control credential: only a run token and the egress proxy.
11. Advice: stay on `single` unless you need more; `split2` when untrusted code is the worry; `split3` when schedules and approvals must
    survive a tenant pod and scale out.

## 1. Planes, trust, topologies

### 1.1 What runs where

| Plane | Owns | Today's code (SURE, read) | Persists |
|---|---|---|---|
| CONTROL (identity, config, vault) | accounts, tenants, tenant and namespace roles, groups, approver lists (`approvers` in `roles.json`), tenant config (`tenantconfig/`), the vault service (`cloudvaultd.rs`, `cloudkeys.rs`), cluster bindings with credential REFERENCES, the internal CA, run-token signing, audit of identity and config changes | `cloudroles.rs`, `cloudtenant*.rs`, `cloudvaultd.rs`, `cloudkeys.rs`, `cloudrotate.rs`, `tenantconfig/*` (`store.rs` files `tenant-config*.json`, `versions/`) | JSON files + vault ciphertext; low volume, rarely written |
| WORKFLOW | scheduler (`iyakki-flow` `sched.rs`), flow engine control (start, retry, cancel, heal), run history (`runstore.rs`), approvals (`tenantconfig/approvals.rs`, `approvals.json`), notification hub (`notifyhub/`: queue, escalation, reminders, digest, inboxes `inbox.rs`), leases (`cloudflows/lease.rs`, `notifyhub/lease.rs`), quotas and fair share, the public/web API for these | `cloudflows/*`, `notifyhub/*`, `iyakki-flow` | high volume, many writers: this is the part that needs a database |
| EXECUTION | sandbox Jobs and pods (CLIs, flow steps), the optional IYAKKI browser sessions, the egress proxy (`iyakki-proxy`), the result uploader | `cloudflows/executor.rs` + `job.rs` (Job manifest as text), `browsermedia.rs`, `iyakki-proxy` | nothing durable except scratch; results go to the workflow plane |

The tenant data directory (files, clones, snapshots; `cloudstorage.rs`, `cloudsnapshot.rs`) is a fourth concern, not a plane: it stays
a volume per tenant, mounted by the pod that serves that tenant's files and by execution pods only as a prepared work volume (section 5.3).

### 1.2 Trust boundaries and who may call whom

```
 browser / desktop connector
        |  HTTPS (tenant sign-in, session)
        v
 +------------------+  mTLS, internal API v1   +--------------------+
 | WORKFLOW plane   |------------------------->| CONTROL plane      |
 | scheduler        |  authorize(person,scope) | roles, config      |
 | approvals        |  config.get, approver    | vault service      |
 | notify hub       |  lists, run-token.mint   | internal CA        |
 | run history      |<-------------------------| (events: role/cfg  |
 +--------+---------+  push: config changed    |  changed, key rot.)|
          |  ^                                  +----------+---------+
          |  | mTLS + run token                            ^
          |  | results, logs, heartbeat                    | secret-for-run:
          v  |                                             | ONLY the control plane
 +------------------+   NetworkPolicy: egress proxy +      | releases it, to the run
 | EXECUTION plane  |   workflow result endpoint only      | token's bearer, once
 | sandbox pods     |--------------------------------------+
 | browser pods     |      (never to control or database)
 +------------------+
        ^ Job create via kube API, credential = vault ref held by CONTROL,
        | applied by a "launcher" in the workflow plane that asks CONTROL for a short-lived kube token
```

Rules (these are the checks the tests must prove):

* Execution -> workflow result endpoint, egress proxy. Nothing else. Execution holds no kubeconfig, no database password, no vault key.
* Workflow -> control (authorize, config, approver lists, mint run token, ask for a kube token). Workflow -> database. Workflow ->
  execution kube API only through a token minted by control for one Job, for one namespace, for minutes.
* Control -> workflow (push events, never commands that run things). Control -> database only for the audit sink and, optionally, its own
  config store; control has its own database role.
* Control never executes user code. Workflow never sees a secret value: it sees references and tokens. A secret is released to a run
  only on the run's own request, bound to that run (section 3.2), logged by control.

### 1.3 Why split, why not

Why: BLAST RADIUS (a sandbox escape lands in a cluster with no vault and no database); SECRETS ISOLATION (the vault service and the
CA live where no user code ever runs); SCALING (sandbox needs many bursty nodes, the vault needs two small ones, the workflow plane needs
a database and steady CPU); UPGRADE CADENCE (sandbox images and kernels change weekly, the vault almost never); COMPLIANCE (auditors
want identity and secrets on a small, separately administered system; data residency per execution cluster); running UNTRUSTED code far
from the vault (the owner's point).

Why not: more to operate (three clusters, a database, a CA), LATENCY on every authorize call (mitigated by caching and push, section 3.4),
MORE FAILURE MODES (partial outages, version skew, clock drift), harder debugging, a larger test matrix that CI cannot cover (no multi-node
kind in CI). A single pod per tenant with the egress proxy and today's guards is already reasonable for a small team. Do not split by default.

### 1.4 The three topologies (deployment profiles)

| | `single` | `split2` | `split3` |
|---|---|---|---|
| Planes | one process per tenant pod (as today) | control (own cluster) + workflow-in-tenant-pod | control, workflow, execution on separate clusters or pools |
| Execution | in pod or Job in the same cluster | Jobs in a separate sandbox cluster | same, several clusters allowed |
| Workflow state | JSON files on the tenant volume (`FileStore`) | files on the tenant volume, or the database | database (`PgStore`) required |
| Control state | files on the tenant volume + vault service | own control deployment | same |
| Needs mTLS / CA | no | yes (workflow <-> control, workflow <-> execution) | yes |
| Needs a database | no | no | yes (PostgreSQL) |
| Use when | one team, trusted-ish code | untrusted code, one workflow owner | schedules and approvals must outlive pods, many tenants, HA |

One file chooses: `topology.yaml` (schema in the example file). The same binary serves all three; a plane is selected by
`iyakki serve --plane control|workflow|execution|all` (`all` is `single`). The topology file is read by `iyakki cloud plan` (to produce
manifests) and by each plane at start (to know its peers); a plane refuses to start if the file's `topology` and its own plane flags disagree.
Node pools or namespaces instead of clusters are the same model: `clusters:` entries may point at one kube profile with different
namespaces; the isolation then is weaker (shared kernel, shared API server) and the plan output says so.

### 1.5 Control plane scope (owner, 2026-11-04: "per team or per tenant"): `control.scope: installation | tenant | team`

| scope | what one control plane serves | isolation | cost / operations |
|---|---|---|---|
| `installation` | every tenant of the installation | weakest: one vault service, one CA, one identity store hold all tenants (isolation by tenant keys and roles, as today's vault service already does) | cheapest; one thing to run and patch |
| `tenant` (RECOMMENDED default) | one tenant, all its teams | a tenant's vault, CA and role data are separate from other tenants; matches today's "pod per tenant" and the existing per-tenant namespace | one small control deployment per tenant (2 pods + volume); the operator automates it with `iyakki cloud plan` |
| `team` | one team (namespace) of a tenant | strongest: a team's secrets and approvers live nowhere else | many control deployments; tenant-wide roles, groups and tenant-level config (templates, freezes, tenant admin approvals) then need a tenant-level parent, so it adds a "tenant admin" control above the teams: much more complexity; only for customers who ask |

What changes with the scope:
* Internal API: the base URL gains the scope owner (`https://control.<scope-id>.<domain>/internal/v1`); the workflow plane has a small
  directory (tenant or team to control endpoint + CA fingerprint) kept in the topology file or the `tenants` table. Every call already
  carries `tenant`; with `team` it also carries `team`, and a tenant-wide call (tenant admin escalation) is answered by the tenant-level
  control, which then must exist (so `team` means two levels). `authorize` caching is keyed by scope id.
* Certificates: SPIFFE names include the scope (`.../control/<scope-id>/<replica>`); one internal CA per scope owner for `tenant` and
  `team` (stronger: a leaked CA key affects one scope), or one installation CA with name constraints per scope (cheaper; GUESS: name
  constraints are honoured by rustls/webpki for the verification we do; confirm in a spike). A workflow plane serving many scopes
  trusts all their CAs, so the trust list is itself data to protect.
* Database: the workflow data is one database for all scopes (the workflow plane is shared) with `tenant_id` and, for `team` scope, a
  `team` column on the rows that have one (runs, approvals, schedules); that is the default. A SCHEMA PER TENANT is the alternative
  (stronger separation, easier per-tenant restore and drop, migrations run N times); GUESS: use the column plus RLS in v1 and offer schema
  per tenant as a flag only when a customer wants per-tenant restore. Control's own data stays in its files/volume per scope owner, not in the
  shared database, so a control scope never shares storage with another.
* Failure and queue behaviour (3.4) is per scope: one tenant's control being down delays only that tenant's runs.

DECIDED 2026-11-04: `team` is NOT built in v1 but nothing may assume only `installation` or `tenant`. Rules for the code and formats:
* A scope is an opaque pair `ScopeId { kind: installation | tenant | team, id: string }` (id for a team is `<tenant>/<team>`), written as
  `kind:id` in text. It is carried in run tokens (`scope` claim), in certificate names (`spiffe://iyakki/<install>/control/<kind>/<id>/<replica>`),
  in every internal API path or header (`X-Iyakki-Scope`), in the topology file, and as the leading key columns of every table (`scope_kind`,
  `scope_id`, then the row key; `tenant_id` stays as a plain column for filters). No code compares a scope to the string "tenant".
* The storage traits take `&ScopeId` next to `&TenantId`; `FileStore` maps it to a folder, `PgStore` to key columns and the RLS setting `app.scope`.
* The parser accepts `control.scope: team` and then stops at plan time with: "control.scope team is not built yet; use tenant or installation".
* What a later team scope would ADD (only this): (1) a tenant-level parent control for tenant-wide roles, groups, templates, freezes and tenant-admin
  escalation, with a call from team control to parent for those; (2) a directory (team to control endpoint and CA) and CA-per-team issuance; (3) approver
  routing that crosses levels (step 3 of the router asks the parent); (4) `iyakki cloud plan` creating one control deployment per team and a cost
  report; (5) a migration tool from `tenant` to `team` that splits one tenant control's data by namespace (vault keys are already per namespace);
  (6) dashboard grouping by team. Tests that keep the door open from day one: a conformance test that runs the stores with a `team` scope id, and a
  grep-style test that no source file compares `kind == "tenant"` outside the scope module.


## 2. The shared database decision

### 2.1 Requirements (what the workflow plane actually does with state)

Queues with many workers (notification delivery, run dispatch) and no double delivery; leases (one scheduler leader, one escalation
sweeper); approvals that expire and are used once (atomic compare-and-set); run history that grows (append, query by tenant, state, time);
audit (append only); change notification (wake a worker when work arrives); per-tenant filtering everywhere; backups and a restore drill;
several writers across clusters; durability on power loss; no secrets. Today every one of these is a JSON file, atomic rename and a
file lease (SURE: `store.rs`, `runstore.rs`, `notifyhub/lease.rs`, `approvals.json`); that works for one pod and cannot work across clusters
(a ReadWriteOnce volume cannot be shared and renames are not transactions across files).

### 2.2 Candidates

| Option | Licence (GUESS: from memory, not looked up now; check before relying) | Fits | Verdict |
|---|---|---|---|
| PostgreSQL | PostgreSQL Licence (permissive, BSD/MIT-like) | multi-writer, transactions, row locks, `SELECT ... FOR UPDATE SKIP LOCKED` queues, advisory locks for leases, LISTEN/NOTIFY, row-level security, mature backup (base backup + WAL, `pg_dump`), HA operators exist (Patroni-style, CloudNativePG; operator licences vary: Apache-2.0 for the ones I know) | RECOMMENDED for split3 |
| MySQL / MariaDB | server GPLv2 (MySQL), GPLv2 (MariaDB); client libraries GPL/LGPL variants | works technically (InnoDB has `SKIP LOCKED` since 8.0 / 10.6), but GPL server is only fine as a separate service and the CLIENT licence is what would touch our binary; we would write our own protocol client anyway | Not preferred: no technical gain over PostgreSQL, licence friction, weaker RLS and advisory-lock story |
| SQLite | public domain | embedded, single node, WAL mode, transactions, easy backup; one writer at a time and one machine | Good optional `SqliteStore` for `single` (fixes torn multi-file updates); NOT for multi-cluster |
| DuckDB | MIT | embedded analytical columnar engine; single writer process; not built for many small concurrent transactions or network access | NOT a shared OLTP store. Useful later to query exported audit/run history (Parquet/CSV) for reports |
| etcd / Kubernetes API objects (CRDs) | Apache-2.0 | fine for small config and leases (`coordination.k8s.io/Lease`); size limit ~1.5 MB per object, poor for queues and history, load on the API server, cluster-bound (not cross-cluster) | Use only for per-cluster Leases if wanted; not for run/notification history |
| Redis-like (Redis, Valkey, KeyDB) | Redis moved away from BSD in 2024 (source-available, later added AGPL); Valkey is BSD-3 (GUESS on details) | fast queues; durability optional (AOF) and weak by default; no relational queries | Avoid: durability and licence concerns; not needed |
| CockroachDB | source-available BSL then proprietary-ish licence changes (GUESS) | PostgreSQL wire compatible, horizontal | Avoid on licence; but our `iyakki-pg` client would speak to it if a customer insists, because the wire protocol is shared (not a promise) |
| FoundationDB | Apache-2.0 | strong transactions, key-value only, awkward client (C library) | Interesting, too different; no |
| MongoDB / document NoSQL | SSPL (not permissive) | no | No |
| NATS JetStream / Kafka | Apache-2.0 | queues only, not state | Not now |

I did not run a web search for this document (no search was used); licence statements above are from memory and flagged GUESS where
they matter. Before the decision is final, someone should confirm the PostgreSQL licence text (SURE enough: it is the "PostgreSQL
License", permissive) and the licence of whichever HA operator the docs recommend.

### 2.3 Recommendation: traits, three backends, PostgreSQL as the multi-cluster default

Define in a new crate `iyakki-store` (traits + `FileStore` + `SqliteStore` behind a feature; no network code), so planes depend on traits:

```rust
// PROTOTYPE signatures only (inside this plan; not code in the repo). All methods take TenantId; every row is tenant scoped.
pub type Result<T> = std::result::Result<T, StoreError>;      // Conflict, NotFound, Unavailable, Corrupt, Denied
pub struct Tx; // a unit of work: Stores that can group writes expose `transact`, FileStore implements it with one lock + staged renames

pub trait NotificationStore: Send + Sync {
    fn emit(&self, t: &TenantId, n: &NewNotification, idem: &IdemKey) -> Result<NotifId>;      // fan-out job + inbox rows, one tx
    fn claim_due(&self, now: Ts, worker: &WorkerId, max: usize, lease: Secs) -> Result<Vec<Delivery>>; // SKIP LOCKED; at-least-once
    fn complete(&self, d: &DeliveryId, outcome: Outcome) -> Result<()>;                          // retry/backoff/dead letter
    fn inbox(&self, t: &TenantId, person: &str, page: Page) -> Result<Vec<InboxItem>>;
    fn mark_read(&self, t: &TenantId, person: &str, ids: &[NotifId]) -> Result<()>;
}
pub trait ApprovalStore: Send + Sync {
    fn request(&self, t: &TenantId, a: &NewApproval, idem: &IdemKey) -> Result<ApprovalId>;
    fn decide(&self, id: &ApprovalId, by: &str, d: Decision) -> Result<Approval>;                // CAS pending -> decided, once
    fn take(&self, id: &ApprovalId, for_run: &RunId) -> Result<bool>;                             // "used once"
    fn pending_for(&self, t: &TenantId, approver: &str) -> Result<Vec<Approval>>;
    fn expire(&self, now: Ts) -> Result<Vec<Approval>>;
}
pub trait ScheduleStore: Send + Sync {
    fn upsert(&self, t: &TenantId, s: &Schedule) -> Result<()>;
    fn due(&self, now: Ts, worker: &WorkerId, max: usize) -> Result<Vec<DueRun>>;                // claims; advances next_run in same tx
    fn set_state(&self, t: &TenantId, id: &ScheduleId, st: SchedState) -> Result<()>;
}
pub trait RunStore: Send + Sync {
    fn create(&self, t: &TenantId, r: &NewRun, idem: &IdemKey) -> Result<RunId>;
    fn transition(&self, id: &RunId, from: RunState, to: RunState, note: &str) -> Result<()>;    // CAS
    fn append_log(&self, id: &RunId, seq: u64, chunk: &[u8]) -> Result<()>;                      // idempotent on (id, seq)
    fn get(&self, t: &TenantId, id: &RunId) -> Result<Run>;
    fn list(&self, t: &TenantId, q: &RunQuery) -> Result<Vec<RunSummary>>;
    fn prune(&self, older_than: Ts) -> Result<u64>;
}
pub trait LeaseStore: Send + Sync {
    fn acquire(&self, name: &str, holder: &WorkerId, ttl: Secs) -> Result<Option<Lease>>;       // fencing token increases
    fn renew(&self, l: &Lease) -> Result<bool>;
    fn release(&self, l: Lease) -> Result<()>;
}
pub trait AuditSink: Send + Sync { fn append(&self, t: &TenantId, e: &AuditEvent) -> Result<()>; fn query(&self, t: &TenantId, q: &AuditQuery) -> Result<Vec<AuditEvent>>; }
pub trait ConfigStore: Send + Sync {
    fn get(&self, t: &TenantId) -> Result<ConfigDoc>;                                              // effective local doc + version
    fn put(&self, t: &TenantId, expect_version: u64, d: &ConfigDoc, by: &str) -> Result<u64>;    // optimistic; keeps last N versions
    fn versions(&self, t: &TenantId) -> Result<Vec<VersionInfo>>;
}
```

Backends:

* `FileStore`: wraps today's behaviour (files, atomic rename, file leases). `single` default. No behaviour change.
* `SqliteStore`: optional, single node; makes each multi-file update one transaction; same traits. Needs a SQLite binding (public domain C library; a permissive crate such as `rusqlite`, MIT, is the usual way). GUESS: skip in the first round, it is a nicety. Decide at step 3.
* `PgStore`: multi-cluster default, uses the client crates of section 2.4 (decided: not in-house).

ConfigStore note: tenant config today is a document the OPERATOR may mount read-only (`tenant-config.json`, ConfigMap) plus an API-edited
local document. In `split*` the control plane keeps these two files (small, rarely written, needs the audit and the managed flag semantics);
`PgStore` still offers `ConfigStore` so the control plane can use the database where it has one, but the default for control in `split2` and
`split3` is its own volume plus replicas reading through the control API. Keep config out of the shared database unless the owner wants one place.

### 2.4 The PostgreSQL client: a famous permissive crate (DECIDED 2026-11-04, replaces the earlier in-house `iyakki-pg` idea)

Decision and dependency-policy exception, to be written into `CONTRIBUTING`/the dependency notes when built: the standing rule "prefer our own
code over dependencies" is set aside for the DATABASE CLIENT, because a hand-written wire-protocol, SCRAM and TLS client is security-sensitive,
needs lasting maintenance and gives the product nothing visible. The exception does not loosen the other rules: permissive licences only,
versions pinned (exact `=x.y.z` in `iyakki-store`'s Cargo.toml, lock file committed), the licence check below must pass, the crate is used only
behind the storage traits (so a different client, or our own code, can replace it later without touching the planes), and no default features that
pull OpenSSL, native-tls or a C library.

What I checked in this repo (SURE, read just now): `Cargo.lock` has `tokio 1.53.2`, `tokio-rustls 0.26.6`, `rustls 0.23.45`, `ring 0.17.14`,
`rustls-pki-types`, `webpki-roots 1.0.9` (licence exception for CDLA-Permissive-2.0 already in `deny.toml`), `socket2`, `mio`, `futures`.
`tokio` is pulled only by `iyakki-core` (optional feature `swarm`) and `iyakki-swarm`; `iyakki-agent` does NOT depend on it today. `iyakki-agent`
uses `rustls 0.23` with `ring`, `std`, `tls12` and no default features (no aws-lc, no OpenSSL). The code is mostly threads and its own HTTP server.
Not checked here (no network, no web search was used): the current versions on crates.io and the exact feature names of the crates below.
Everything about those crates is FROM MEMORY and must be re-verified in the spike (`cargo tree`, `cargo deny check`) before it is trusted.

| Candidate | Licence (from memory) | Sync/async | Fit | Verdict |
|---|---|---|---|---|
| `postgres` (rust-postgres family, blocking wrapper) 0.19.x | MIT OR Apache-2.0 | blocking API; internally runs `tokio-postgres` on a tokio runtime thread | fits a thread-based codebase: `Client::query/execute/transaction`, prepared statements, `notifications()` for LISTEN/NOTIFY, SCRAM-SHA-256 (and `-PLUS` with a TLS adapter that offers channel binding) | RECOMMENDED |
| `tokio-postgres` 0.7.x | MIT OR Apache-2.0 | async; needs tokio | same engine; only if the workflow plane becomes async | Used underneath; not our API |
| TLS adapter `tokio-postgres-rustls` (+ `postgres-rustls`-style sync use through `MakeTlsConnect`) | MIT (from memory; maintained by a single author: a risk) | rustls-based, no OpenSSL | uses our `rustls 0.23`/`ring` `ClientConfig` (`verify-full`, custom CA) | DECIDED 2026-11-04: NOT used. We write our own thin adapter `iyakki-pg-tls` over `tokio-rustls` (already in the lock, MIT OR Apache-2.0), see 2.8. The Postgres client itself stays the famous crate |
| `postgres-native-tls`, `postgres-openssl` | MIT/Apache | pull native TLS / OpenSSL | breaks the musl static, no-C-library rule | NO |
| `sqlx` 0.8.x | MIT OR Apache-2.0 | async only; runtime features; macros; its own migration machinery | heavy compile time and binary size (GUESS: several MB and minutes), macro-based compile-time checks need a live database, large dependency tree | NO |
| `diesel` | MIT OR Apache-2.0 | sync ORM; needs `libpq` (C, PostgreSQL licence) | C library; musl static link pain | NO |
| Pooling: `r2d2` + `r2d2_postgres` | MIT OR Apache-2.0 (from memory) | sync, thread based | simple, widely used, fits | RECOMMENDED |
| `deadpool-postgres`, `bb8-postgres` | MIT OR Apache-2.0 / MIT (from memory) | async | only with an async design | no |
| `pg_wire`/other small crates | various | | not famous, not maintained enough | no |

Consequences, honestly: `PgStore` brings `tokio` (rt, net, time features only, no macros) into the CLOUD binary; the swarm already uses it
elsewhere in the lock file, so no new vendor, but the binary grows and a runtime thread exists. GUESS: +1 to 2 MB and +1 to 2 minutes of
compile time for the cloud profile; measure in the spike and keep `PgStore` behind a cargo feature (`pg`) so desktop and `single` builds do not
carry it. MSRV is not a concern (the toolchain here is rustc 1.99; the crates need far older, from memory). musl static: all candidates above
are pure Rust (from memory: they use `sha2`, `hmac`, `md-5`, `pbkdf2`, `rand` from RustCrypto, permissive); verify with the existing static-build
check (`dist/cloud/build-cloud.sh`) in the spike.

Licence gate (what exists, SURE, read): crate licences are enforced by `deny.toml` (allow-list of permissive licences, `unknown-registry =
deny`, crates.io only; run with `cargo deny check`; the release gate `crates/iyakki-relcheck` lists the third-party licences and the SBOM).
`crates/iyakki-agent/tests/licence_gate.rs` is different: it cross-checks the Quick install / catalogue inventory (tools downloaded at the
person's request) against the code and fails on a missing licence or a restricted licence on a shipped file; it does NOT scan crate
dependencies. So adding the client means: (1) `cargo deny check licenses` must pass with the new crates (MIT and Apache-2.0 are already on
the allow-list; any new licence string, for example `Unicode-3.0` style data licences, needs a reasoned entry; no `exceptions` should be
needed); (2) the new crates appear in `THIRD_PARTY_LICENSES.txt` and the CycloneDX SBOM that the release already generates; (3) a new test in
the licence-gate style, `pg_client_is_permissive_and_pinned`, reads `Cargo.toml`/`Cargo.lock` and asserts the `=` pins and that no
`openssl`, `native-tls`, `libpq` or `aws-lc-sys` package is present in the cloud profile's tree; (4) the PostgreSQL SERVER is not shipped, so
it is not in the catalogue (it is the operator's, or pulled at deploy time; section 2.7).

Test strategy (smaller than before because the protocol is not ours): trait conformance suite against `FileStore` always; against `PgStore` with
`#[ignore]` tests using a real PostgreSQL container ONLY if docker is available (queue claims with N workers and `SKIP LOCKED`, LISTEN/NOTIFY,
TLS `verify-full` with a generated CA through `rcgen`, version matrix); unit tests of SQL building and error mapping without a server; migration
tests. Cost: about 2 sessions for `PgStore` plus the TLS adapter and pool glue (was 4 for the in-house client).

### 2.4a The "secure package" standard and the TLS adapter `iyakki-pg-tls` (DECIDED 2026-11-04)

STANDARD for any crate we write for a security-relevant job (this one first; later the internal-API client, the run-token library, the cert
renewal client). All must hold, checked by a script in CI and listed in the crate's own `SECURITY.md`:
1. `#![forbid(unsafe_code)]`; no `build.rs` that runs commands; no proc-macros of ours.
2. Minimal dependencies: only what the job needs, each justified in a table in the crate README; versions pinned (`=`), lock file committed.
3. Licence and advisory gate: `cargo deny check licenses advisories bans sources` passes (permissive allow-list of `deny.toml`; RustSec advisories
   fail the build; no yanked crates).
4. A written THREAT MODEL (`THREAT-MODEL.md`: assets, attackers, trust boundaries, what is out of scope, abuse cases and the test that covers each).
5. Parsing parts (anything that reads bytes from outside) have property or fuzz tests (`proptest`-style generated input at least, `cargo fuzz`
   targets where a fuzzer is available); no panic, no unbounded allocation; size limits as constants.
6. Golden tests: recorded byte-for-byte expected outputs kept in the repo.
7. Reproducible build: same inputs, same bytes (`--locked`, fixed `SOURCE_DATE_EPOCH`, `--remap-path-prefix`), checked by building twice.
8. `SECURITY.md` with a contact note (the repo's security contact, how to report, expected response) and supported versions.
9. An SBOM entry (CycloneDX component with version and hash) generated by the release step.
10. No network in tests: loopback only, with generated certificates (`rcgen`); no test needs the internet or a real database.
11. A clear typed API with NO stringly-typed security options (enums and newtypes, not "verify-full" strings); invalid combinations do not compile or return a typed error.
12. TLS 1.2 or newer only; plaintext (no TLS) does not exist as a mode; certificate verification is FORCED BY DEFAULT. There is no UNAUDITED skip
    path in any build: the only way to skip verification is the administrator break-glass mode `Unverified` (2.4b), which is explicit, named, reasoned,
    audited, time-limited and absent from environment variables and CLI flags. A `cfg(test)`-only helper for failure-path tests must not exist in release builds.
13. Secrets are never logged and never in `Debug`; error messages carry no key material.

Crate `crates/iyakki-pg-tls/` (name chosen; layout):
```
src/lib.rs        public API: PgTls::new(TlsPolicy) -> impl MakeTlsConnect; TlsPolicy { roots: Roots, mode: Mode, pins: Vec<Sha256Pin>, client_auth: Option<ClientCert> }
src/policy.rs     Roots::{SystemBundle(path) | PemBundle(bytes) | WebpkiRoots}, Mode::{VerifyFull | VerifyCa | PinOnly(Pin) | Unverified(BreakGlass)} (no Disable/Prefer/Require/plaintext type exists), pins
src/verify.rs     the rustls ServerCertVerifier wrapper: webpki verification first (always), THEN optional SPKI-hash pin check; never replaces verification
src/connect.rs    the tokio-postgres TLS handshake glue (MakeTlsConnect / TlsConnect / TlsStream), channel-binding data (tls-server-end-point) exposed for SCRAM-PLUS
src/error.rs      typed errors, no secrets
tests/            local rustls test server on loopback (rcgen certs), golden and property tests
THREAT-MODEL.md  SECURITY.md  README.md
```
Size GUESS: 300 to 500 lines of code and 400 to 700 of tests; 1 to 2 agent-sessions. Depends on: `tokio-rustls`, `rustls` (ring, already used), `tokio-postgres`
(for the traits), `sha2` (already in the tree), `rustls-pki-types`; nothing else.

SCRAM channel binding: if the chosen client crate supports `SCRAM-SHA-256-PLUS` (from memory: tokio-postgres supports it when the TLS stream
reports channel-binding data; confirm in the spike), the adapter returns `tls-server-end-point` data (SHA-256 of the server certificate, or the
hash its signature algorithm requires per RFC 5929) and the policy `channel_binding: Require | Prefer` (default Prefer; Require for `split3`
production). If not supported, say so in the README and in the dashboard.

Acceptance tests (all offline, loopback, rustls test server):
* verify-full with the right CA and name: connects (a minimal fake postgres speaks `SSLRequest` -> `S` then a TLS handshake, then ends).
* wrong hostname (SAN mismatch): refused with `HostnameMismatch`; wrong CA / unknown issuer: refused; expired certificate: refused; not yet valid: refused;
  revoked is not checked (documented); certificate chain with an intermediate missing: refused; key usage / EKU not serverAuth: refused.
* Pinning: the correct SPKI pin connects; a different pin refused even though the CA verifies; a pin with an invalid CA still refused (pin does not replace verification).
* Plaintext refusal: the server answers `N` to `SSLRequest` -> the connection fails with `ServerRefusedTls` (no fallback); a server that never answers
  times out; a server that answers `S` and then speaks plaintext fails the handshake.
* TLS 1.1 or lower offered by a server: refused; TLS 1.2 and 1.3 accepted.
* `VerifyCa` (chain verified, name check relaxed) exists only for named, logged cases (an IP-addressed in-cluster service); a plan-time warning is shown; `iyakki cloud plan` defaults to `VerifyFull`. See 2.4b for all four modes and their tests.
  and a plan-time warning is shown; `iyakki cloud plan` defaults to `VerifyFull`.
* Property tests on the pin parser and the PEM bundle reader (garbage never panics); golden test of the channel-binding bytes against a fixed certificate.
* Release-build check: the only code path that skips verification is `Unverified`, reachable only from a `BreakGlass` value built from a verified admin record (reason, admin, expiry); a test greps the public API and the environment/CLI parsing for any other route, and a unit test proves there is no plaintext mode (a server answering `N`, or a policy asking for plaintext, never yields a connection).

Risk note on maintaining it: we own security-critical glue. The mitigations are its small size, delegation of all cryptography and chain
validation to rustls/webpki (we only wrap), the standard above, and an upstream-watch duty (rustls and tokio-postgres releases; a monthly
`cargo deny` run; rustls major bumps need a reviewed port). If tokio-postgres changes its TLS trait, the adapter breaks loudly at compile time, not silently.
The risk of a subtle bug (for example accepting a certificate for the wrong name) is covered by the negative tests above; it is smaller than the
risk of relying on a single-maintainer adapter, but not zero. Re-review at every dependency bump.

### 2.4b Certificate verification modes: forced by default, break-glass by an administrator (DECIDED 2026-11-04)

Owner: "force it, and the admin can disable it where verification cannot be done." Written as follows; the TRADE-OFF is stated plainly: every
way to relax verification is a way for a person (or an attacker who has an admin's session) to make the database link open to interception.
So the relaxations are ordered, each weaker one needs more ceremony, and the weakest is time-limited and loud.

| mode | what is verified | when to use | ceremony |
|---|---|---|---|
| `VerifyFull` (DEFAULT) | chain to a trusted root AND host name (SAN) | normal | none; mandatory unless an admin changes it |
| `VerifyCa` | chain to a trusted root; name check relaxed | in-cluster service addressed by IP | named connection in the topology file; warning at `iyakki cloud plan` and on the dashboard |
| `PinOnly` | exactly ONE certificate or SPKI SHA-256 fingerprint the admin gave; the chain and name are not needed | self-signed database certificates; RECOMMENDED before anything weaker. This is real verification (a MITM cannot present that key) | pin in the topology file (`pin: sha256:...`); shown with the certificate's subject and expiry; expiry of the pinned certificate warns 30 days ahead; rotating the certificate needs a new pin (two pins may be listed during a rotation) |
| `Unverified` (break-glass) | nothing about the server's identity (TLS still encrypts; plaintext still does NOT exist) | only where verification truly cannot be done (broken managed service chain, a legacy appliance); an emergency, not a setting | see below |

Break-glass rules for `Unverified`:
1. Only an ADMINISTRATOR enables it (tenant admin for a tenant's own connection, operator for platform ones), explicitly, for a NAMED connection
   (`database`, a named plane link), never globally. Enabled through the signed topology file or the admin API/dashboard; NEVER by an environment
   variable, a kernel command line or a CLI flag alone (a flag may only point at a topology file that carries the signed record).
2. A required free-text `reason` (non-empty, at least 20 characters, stored) and the admin's identity; an `expires_at`.
3. TIME-LIMITED: default 7 days, bounds 1 hour to 30 days. At expiry the connection reverts to the PREVIOUS strict mode (the one recorded when it
   was enabled) and the admins are notified; an admin may renew with a click (new reason shown, audited). A revert that would break the link is
   still done: the dashboard says why and offers the renewal (loud failure, not silent).
4. AUDITED: `tls.unverified.enabled|renewed|expired|disabled` with who, why, connection, previous mode, expiry. A PERSISTENT WARNING is shown on the
   dashboard (banner while active, with time left) and in EVERY `iyakki cloud plan` output and `iyakki cloud topology verify`.
5. REFUSED in `split3` and any profile marked `production` unless the tenant policy explicitly allows it (`tenant policy: tls.allow_unverified: true`,
   set by a tenant administrator, itself audited); in that case the maximum duration is 7 days and a second administrator must approve (the approvals router).
6. Never the default, never inferred from a failed verification (a verification failure is an error, not a prompt to disable), not available to the
   assistant (`ops.*` cannot propose it as a one-click step; it may only explain the choice).
7. Unit tests (offline): enabling without a reason or without expiry fails; a reason shorter than the minimum fails; an environment variable or flag
   alone does nothing; reverts to the previous mode at expiry (fake clock) and notifies; refused in a production profile by default; allowed there only
   with the policy and a second approver; audit lines written for each transition; a server certificate for the wrong name connects only in
   `Unverified`, never in the other three; plaintext is refused in all four modes.

### 2.4c One-off audit of existing security code against the standard (DECIDED 2026-11-04: a checklist audit, not a rewrite)

Run the secure-package standard (2.4a) once as a CHECKLIST over the existing security code; record findings in `docs/security/` (or the repo's
audit folder), fix only what is wrong, open items for the rest. Checklist per crate or module: (1) `forbid(unsafe_code)` or a list and reason for each
`unsafe`; (2) dependency list justified, pinned; (3) `cargo deny` clean; (4) a short threat model; (5) parsers have property or fuzz tests;
(6) golden tests; (7) reproducible build; (8) `SECURITY.md` pointer; (9) SBOM entry; (10) no network in tests; (11) typed security options (grep
for string-compared modes); (12) TLS minimum and mandatory verification (and no unaudited skip); (13) no secret in logs or `Debug`.
Audit FIRST, in this order: `crates/iyakki-sec` (policy, media, WPA); the vault and key code in `crates/iyakki-agent/src` (`cloudkeys.rs`,
`cloudvaultd.rs`, `cloudvaultguard.rs`, `cloudvaultlink.rs`, `cloudrotate.rs`, `vault.rs`, `vaultpending.rs`, `secretreq.rs`); the tenant-config secret paths
(`tenantconfig/schema.rs` `check_value`, `tenantconfig/vault.rs`, `tenantconfig/runtime.rs` secret use); the TLS code in `crates/iyakki-remote` and
`crates/iyakki-link` (`certinfo.rs`, `cloudclient.rs`, `sigscheme.rs`, `wsproto.rs`) plus `cloudlisten*.rs`/`cloudroute.rs` (certificates, SNI, mTLS);
web service authentication (`webservice/auth.rs`, `webaccess.rs`, `cloudaccess.rs`). Size: 1 session per area, M in total; effort is mostly reading and tests.

### 2.5 Schema sketch (PostgreSQL)

Every table has `tenant_id text not null` as the first column of its primary key or an index; every query passes it (the trait does).
Row-level security is the second line of defence: each plane connects as its own database role, sets `SET app.tenant = '<id>'` per
transaction (or, for the workflow service that serves many tenants, uses a role that sees all tenants but is limited by tables); policies
`USING (tenant_id = current_setting('app.tenant', true))` apply to the roles used for tenant-scoped reads (the support/report role). GUESS:
the workflow workers serve all tenants, so RLS protects mainly against a bug in a per-tenant API path and against reporting roles, not
against a compromised workflow plane. State this plainly.

```
schema_version(version int primary key, applied_at timestamptz, checksum text)
tenants(tenant_id pk, created_at, state)                       -- reference only; identity truth stays in control
schedules(tenant_id, schedule_id, owner, flow, spec jsonb, next_run timestamptz, state, version, pk(tenant_id,schedule_id))
   index (state, next_run)                                      -- due scan
runs(tenant_id, run_id uuid, schedule_id null, flow, owner, env, state, due_at, late_by_secs null, late_reason null, created, started, finished, exec_cluster, idem_key,
     pk(tenant_id,run_id)); unique(tenant_id, idem_key); index(tenant_id, state, created desc)
run_logs(tenant_id, run_id, seq bigint, chunk bytea, pk(tenant_id,run_id,seq))   -- idempotent append; partition by month (later)
approvals(tenant_id, approval_id, kind, team, item, requester, state, approvers jsonb, escalated, created, expires, used_by_run null)
   index(tenant_id,state,expires)
notifications(tenant_id, notif_id, kind, severity, subject, body_ref, created, dedupe_key)       -- unique(tenant_id,dedupe_key)
inbox(tenant_id, person, notif_id, read_at null, pk(tenant_id,person,notif_id))
deliveries(tenant_id, delivery_id, notif_id, channel, state, attempts, next_attempt timestamptz, locked_by, locked_until, last_error)
   index(state, next_attempt)                                    -- claim with FOR UPDATE SKIP LOCKED
escalations / reminders / digests(tenant_id, ...)               -- the state machines of notifyhub, one row each
leases(name pk, holder, token bigint, expires)                   -- or pg_advisory_lock where a session lives long enough; table is simpler and portable
audit(tenant_id, seq bigserial, at, actor, action, target, detail jsonb)                          -- append only; INSERT-only grant; hash chain column optional
plane_health(plane, scope_id, replica, state, detail jsonb, reported_at)   -- dashboard source
idempotency(tenant_id, key, response_hash, created)             -- for API retries
```

Claim query shape (SURE as a pattern): `UPDATE deliveries SET locked_by=$1, locked_until=now()+$2 WHERE delivery_id IN (SELECT delivery_id FROM
deliveries WHERE state='due' AND next_attempt<=now() ORDER BY next_attempt FOR UPDATE SKIP LOCKED LIMIT $3) RETURNING ...`. Leader
election uses the `leases` table with a fencing token the work carries.

Migrations: forward-only numbered SQL (`0001_init.sql`...) embedded in the binary; each with a checksum stored in `schema_version`;
checked at start: if the database is NEWER than the binary, the plane refuses to write (reads allowed) and says so; if OLDER, only the
`migrate` role applies migrations, run by `iyakki cloud db migrate` (or an init Job), never by every pod racing (a transaction-level advisory
lock guards it anyway). Backward-compatible changes only inside a minor version window (expand, then contract in a later release).

### 2.6 Data classification, encryption, backup

* NOTHING SECRET in the database: only vault references (`vault:ns/name`), ciphertext produced elsewhere, tokens' hashes, and ordinary
  business data. The existing schema check (`schema::check_value`) rules for config apply to anything stored from config. Notification bodies
  follow the redaction rules of `notifyhub/redact.rs`; run logs are the sensitive part (a command may print a secret): redaction before
  storage, and log retention short. Classify: logs = confidential, audit = confidential, approvals = internal, schedules = internal.
* In transit: TLS `verify-full` required outside `single` (plain refused); the client library does not offer plain for the cloud profile.
* At rest: volume or storage-class encryption (operator's); optionally column encryption of `run_logs.chunk` and `notifications.body` with a
  per-tenant data key wrapped by the vault (reuse the XChaCha20-Poly1305 wrap in `cloudkeys.rs`) so a database dump alone does not read bodies
  (GUESS: adds a control-plane call per key and complicates search; make it a flag, default on for logs only).
* Backups: continuous WAL archive + periodic base backup is the operator's job (an HA operator does it); IYAKKI ships a `pg_dump`-style
  exporter instead (below) for portability. A RESTORE DRILL is a documented command and a scheduled report: `iyakki cloud db drill` restores the
  newest backup into a scratch database, runs the integrity checks (row counts per table, hash chain of audit, no orphan deliveries) and records
  the result; cadence monthly; the drill's result is a notification to the platform admins (ties in with the notification hub).
* Retention: per table in the topology file (`retention_days`); a sweep by the leader; audit longest; run logs shortest; legal hold flag per tenant
  disables pruning.

### 2.7 Who runs the database: the operator, or optionally IN the cluster (DECIDED 2026-11-04)

Default: the team's OPERATOR runs PostgreSQL (a managed service or their own); the topology file only names host, TLS and secret references.
IYAKKI does not ship the PostgreSQL server (its licence, the PostgreSQL Licence, is permissive, but shipping a database is not our product);
the image is pulled at deploy time from the operator's registry (`database.in_cluster.image`, a digest-pinned reference they mirror).
For trials and small installs, `database.kind: postgres` with `database.in_cluster.enabled: true` makes `iyakki cloud plan` emit an OUTLINE
(full manifests are for the later implementation, step 4):

* `StatefulSet` `iyakki-pg` (1 replica; 2+ only with an operator), `volumeClaimTemplates` for the data PVC (`storageClass` and `size`
  configurable, default the class k3s ships, `local-path`; warn that `local-path` is node-local and does not follow the pod to another node, and that
  it is `Delete` reclaim: advise a `Retain` class for production), headless `Service` for the stable pod name plus a normal `ClusterIP` Service `pg`.
* `Secret` with the superuser, `iyakki_migrate`, `iyakki_workflow`, `iyakki_control` (audit), `iyakki_report` (read only, RLS) passwords and the
  server TLS key: GENERATED by `iyakki cloud plan` at creation, written straight to a Secret manifest or applied by `kubectl`, NEVER printed (the
  plan shows only names). Rotation: the plan regenerates and rolls the pods; the apps read credentials from `vault:` references held by the control plane.
* Settings: `listen_addresses` the pod IP, `ssl = on` with cert/key from the internal CA (or the operator's), `ssl_min_protocol_version =
  TLSv1.2`, `password_encryption = scram-sha-256`; `pg_hba.conf`: `hostssl iyakki all <pod CIDR> scram-sha-256` only, `host ... reject`; no
  `trust`, no local TCP without TLS; `max_connections` sized from pools (sum of `pool.max` + headroom), `shared_buffers` from the memory limit,
  `synchronous_commit on`, `fsync on`. Resource requests/limits from the topology file (defaults: 500m/1Gi request, 2 CPU/2Gi limit; GUESS),
  `securityContext` non-root, read-only root file system except the data and run directories, `fsGroup`.
* `CronJob` `iyakki-pg-backup`: `pg_dump --format=custom` of the `iyakki` database (and `pg_dumpall --globals-only`) to a SECOND PVC, file name
  with UTC time, checksum file, retention (`keep_daily`, `keep_weekly`), plus an optional copy to an object store reference the operator names.
  A node-local second PVC on the same node is not a real backup: the plan warns unless the class differs or an off-cluster copy is set.
* RESTORE DRILL (monthly `CronJob` and `iyakki cloud db drill`): start a throw-away `postgres` Pod with an `emptyDir`, restore the newest dump,
  run the integrity checks (row counts against a stored count file, audit hash chain, no orphan deliveries, schema_version equals the binary's),
  delete the pod, record pass/fail with timing in `audit` and emit a notification to platform admins; a failed or missing drill is shown on the
  dashboard (3.5). Real restore procedure: pause intake (3.5), scale the planes to zero, restore into a new database, run the integrity checks,
  switch the Service, resume; documented as numbered steps in the generated README.
* `NetworkPolicy`: ingress to the PG pod on 5432 only from pods labelled as the control and workflow planes' namespaces (and the backup/drill
  jobs); execution namespaces are denied; egress from the pod limited to DNS (and the backup target if remote).
* Alternative operators (optional, we depend on none): CloudNativePG (from memory: Apache-2.0, CNCF project; verify), Zalando postgres-operator (MIT; verify),
  Crunchy PGO (Apache-2.0 with branding terms; verify). The topology file's `database.host` simply points at whatever service they create. Only the
  database is touched; IYAKKI never installs an operator.


## 3. Cross-plane communication

### 3.1 mTLS and service identities

Each plane (and each replica) has a leaf certificate with an SPIFFE-like name (`spiffe://iyakki/<installation>/<plane>/<replica>`), issued by
an internal CA operated by the CONTROL plane (CA key in the control vault; `iyakki cloud plan --init-ca` creates it). Leaves live 24 hours
by default and are renewed through a control endpoint authenticated by the previous leaf (bootstrap: a one-time token from the plan output).
rustls server and client with client auth (`cloudlisten_mtls.rs` already has the per-route client-CA verifier and `cloudroute.rs` the
certificate checks: reuse). This is exactly the "mTLS on the listener-to-pod link" not-built item from `CLOUD-MULTITENANT-PLAN.md`: build one
TLS-wrapping dialer once and use it for listener-to-pod, workflow-to-control and execution-to-workflow. Authorization after TLS: the
peer's plane name decides which internal calls are allowed (a table in code, tested); an execution identity may call only `results.*`.
Revocation: short lifetimes plus a deny list pushed by control; no OCSP.

### 3.2 Run tokens

A run token is minted by control for ONE run: Ed25519 signature (the repo already has Ed25519 in `cloudkeys.rs`), claims `{tenant, team,
environment, run_id, owner, flow, allowed_secret_refs[], exec_cluster, nbf, exp (default 15 min, max the run's time limit), jti}`. The
sandbox pod gets it in a Secret mounted in the Job (the launcher writes it; scoped to that namespace and TTL-deleted). Uses: (a) call the
workflow result endpoint (workflow verifies signature with control's public key and checks `jti` unused for conflicting results); (b) ask
control for a secret listed in `allowed_secret_refs` (control re-checks the run is still active and the approval, if any, was taken); a
secret is released at most N times, logged. The token is bound to the pod's mTLS identity where the cluster can issue one (GUESS:
per-pod certificates need a small issuer; the first version binds to the run only).

### 3.3 The internal API (v1; JSON over HTTPS/mTLS; header `X-Iyakki-Internal: 1`, `Idempotency-Key`, `X-Request-Id`)

Control serves:
`POST /internal/v1/authorize` (person, tenant, team, action -> allow/deny + reason; reuses `cloudroles`), `GET /internal/v1/tenants/{t}/config`
(effective view, version), `GET /internal/v1/tenants/{t}/approvers?scope=` (the router input), `POST /internal/v1/run-tokens` (workflow -> control,
checks `k3s_decide` already allowed or approval taken), `POST /internal/v1/secrets/use` (run token -> value, once, audited),
`POST /internal/v1/kube-token` (workflow -> short-lived kube credential for one Job in one namespace), `POST /internal/v1/pki/renew`,
`POST /internal/v1/health` (every plane reports to workflow's `plane_health`), `POST /internal/v1/audit` (workflow and execution events that belong in the identity audit; others go to the database `AuditSink`),
`GET /internal/v1/events` (server-sent stream: role changed, member removed, config changed, key rotated, cert denied).

Workflow serves:
`POST /internal/v1/results` (run token: state, exit code, log chunks with seq, artifacts refs), `POST /internal/v1/heartbeat`,
`POST /internal/v1/notify` (control and execution emit notifications through the hub, same idempotency key rules as `sink.rs`),
`POST /internal/v1/approvals/decide` (called by the web layer after control authorized the approver),
public API stays as today under `/api/v1` and is served by the workflow plane for flows/approvals/inbox and by control for roles/config/vault.

Execution serves almost nothing: a health port and, if the browser extra is on, the browser session endpoint reachable only from the
workflow plane. Versioning: path `/internal/v1`; every response carries `X-Iyakki-Api: 1.x`; a plane supports the current and the previous
major for one release; unknown fields are ignored; a missing capability returns 501 with the minimum version.

### 3.4 When a plane is down (revised: queue and deliver late with a warning)

Principle (owner, 2026-11-04): never silently lose or just block work. What cannot start is QUEUED DURABLY in the workflow plane's store with
its ORIGINAL due time and released when the dependency returns, with a visible warning that it ran late.

| Down | Effect | Policy |
|---|---|---|
| Control | no new authorize beyond cache, no run tokens, no secret fetches, no kube tokens, no cert renewals | see "late delivery" below. Cached authorize answers (60 s, GUESS) keep the web UI usable; leaf certificates keep working until they expire (24 h gives the operator time); running pods keep running until their token expires |
| Workflow | new runs not started; sandbox pods finish but cannot upload results | pods retry results with backoff until the Job deadline, then keep them in the pod's scratch for a last try (lost if the pod is gone: say so); missed schedules follow `late_policy` on return; control's own notifications queue in a local outbox file and flush later |
| Execution | runs cannot start | per schedule `on_execution_down: queue` (default, same late-delivery path) `| fail | fallback` (another execution cluster listed for the tenant, never control or workflow) |
| Database | workflow plane read-degraded | readiness fails, API says 503, a small bounded outbox file per replica; control and execution continue; `single` unaffected |

LATE DELIVERY (control, execution or a secret store down). Items affected: scheduled runs, manual runs, flow steps needing a secret, approvals
(filing needs approver lists), notifications whose recipient expansion needs groups. The workflow plane writes the item to its store in state
`queued_late` with `due_at` (original), `reason` (`control_unavailable`, `execution_unavailable`), `first_attempt_at`. A releaser (leader, on a
lease) probes the missing dependency (health endpoint with backoff) and, when it answers, releases items IN ORDER OF `due_at` with throttling
(`release_rate`, default 5 per second per tenant and a global cap, so a 6-hour outage does not stampede the sandbox or the vault). Each released
item carries `late_by = released_at - due_at` and `late_reason`. Visible warnings: a banner on the dashboard while anything is queued; the run history
row shows "ran 37 minutes late (control plane unavailable)"; the run's own notification text gains that sentence; an audit line
`run.late`. Per schedule: `late_policy: deliver_late | skip | fail` (default `deliver_late`), `max_late` (default 6 HOURS; four-eyes production environments default to 1 hour with `on_expired: skip`; per-schedule overrides stay), and beyond it
`on_expired: skip | notify` (default `notify`: the run is skipped and a notification says which due time was lost). Overlap policy of the schedule
still applies (a catch-up storm of the same schedule collapses to the policy's `catch_up: one | all | none`, default `one`; GUESS: reuse the
missed-run policy already in `sched.rs`). Approvals filed during an outage are queued with their original time; their expiry clock starts on
release, not on filing (so an approver is not penalised), and the request shows "filed 37 minutes ago while control was down".

`allow_cached` (opt-in per schedule, default off): a run that needs NO control-plane data (no secret, no new token, no config beyond the cached
effective view of the last N minutes) and whose schedule already holds a still-valid cached run token may proceed during the outage. The run is
marked `degraded: control_down` and warns the same way. Never allowed for production environments with four eyes, for runs that need a secret, or when
the cached token has expired.

Clock and time sync: all planes run NTP-synced hosts; tolerance 30 s; token `nbf`/`exp` allow 30 s skew; the database's `now()` is the time
source for leases, queues and `due_at` comparisons (never a client clock) so skew between workflow replicas does not matter. A plane compares its
clock with the peer's `Date` header on each internal call and reports `clock_skew_seconds` as a health metric; above 60 s it refuses to mint or verify tokens.

Idempotency: every state-changing internal call carries an `Idempotency-Key` (runs: `<schedule>/<fire-time>`; deliveries: `<notif>/<channel>`;
results: `<run>/<seq>`); the database enforces uniqueness (`unique(tenant_id, idem_key)`), so a release that crashes half way is safe to repeat;
delivery is at-least-once with idempotent receivers; channels that cannot dedupe (email) accept a rare duplicate and say so.

### 3.5 Operations: the plane health dashboard, admin actions, and the assistant

DASHBOARD (page in the web service, workflow plane serves it from health data each plane reports every 10 s to `POST /internal/v1/health`,
stored in table `plane_health`; admins of the scope see their own scope, platform operators see all):

| per plane (control, workflow, execution, database) | shown |
|---|---|
| state | `up | degraded | down` with the reason and since when |
| queue | depth, oldest queued age, `queued_late` count, late deliveries in the last 24 h and their largest `late_by`, dead letters |
| leases | who holds scheduler, releaser, escalation sweeper (replica, fencing token, expiry) |
| sync | last successful sync with each peer, clock skew, certificate expiry (leaf and CA), API versions seen |
| database | connections used/max, replication lag if any, last backup, last restore drill and its result, schema version |

ADMIN ACTIONS (each calls an internal endpoint, is authorised by `cloudroles` (platform operator, or tenant/team admin for their scope), written to
the audit with who, scope, before/after; DESTRUCTIVE ones are approval-gated through the same approvals router (`tenantconfig::approvals::route`:
approver role, then admins, then escalation; nobody approves their own):

| action | effect | gated? |
|---|---|---|
| pause / resume plane intake | stop/start accepting new runs, schedule firing and notification emits for a scope (items queue, none are lost) | pause: no; resume: no |
| check details | per item: why queued, last error, attempts, trace id | read only |
| retry / release now | release one or all `queued_late` items immediately (still throttled unless `--no-throttle`) | no for one; all needs approval above N items (default 50) |
| drain the queue | run everything queued under the throttle then report | no |
| skip a late item | mark it `skipped` with a reason (notification sent to the owner) | yes (destructive: work is dropped) |
| purge dead letters | delete failed deliveries | yes |
| rotate a plane certificate | issue a new leaf now and deny the old one | yes for the CA; no for a single leaf |
| fail over a lease | force release a lease so another replica takes it (fencing token makes the old holder harmless) | yes |
| switch a tenant's execution cluster | change `tenants.<t>.execution` | yes (config change, normal tenant-config flow) |

ASSISTANT (chat model/agent, read-only diagnosis; fixes only as proposals). A tool set `ops.*` is offered to the assistant for admins, one
registry entry each, all READ-ONLY, redacted (no secret values, no log content beyond error classes and counts, no run output), scoped to the
signed-in admin's scope, rate-limited and audited as `assistant.ops.read`:

* `ops.plane_health(plane?)`: state, since, reason, versions, clock skew.
* `ops.queue_summary(scope?)`: depth, oldest age, `queued_late`, dead letters, per schedule top offenders.
* `ops.late_items(scope?, limit)`: the late items (ids, due_at, late_by, reason, schedule, owner), no payload.
* `ops.recent_errors(plane?, since, limit)`: error classes with counts and sample ids (no messages that may hold data).
* `ops.leases()`: holders and expiry.
* `ops.lag(plane?)`: workflow release lag, database replication lag, notification delivery lag.
* `ops.certificates()`: expiry and issuer of leaves and CAs.
* `ops.backup_status()`: last backup and last drill.
* `ops.topology()`: the effective topology file (secret references shown as names only).
* `ops.explain_item(id)`: the decision trail of one queued or failed item.

ASSISTED FIX (DECIDED 2026-11-04: the assistant may suggest and help fix, with the admin present). Interaction:
1. DIAGNOSE: the assistant uses the read-only `ops.*` tools freely (read steps need no click) and explains what it found.
2. PROPOSE A PLAN: `ops.propose_plan(title, steps[], expected_effect, rollback[])` stores a reviewable plan; each step is typed: `read` (an `ops.*` call),
   `action` (one of the dashboard's admin actions with exact parameters, from the table above) or `manifest` (a manifest or command shown as text, to be
   applied through the existing guarded apply path `cloudlaunch`/`kubectl` flow that shows what it will apply first). Free-form shell is not a step type.
   Every step has a `destructive` flag computed by the platform from the action table (the assistant cannot lower it) and a rollback entry.
3. REVIEW: the admin sees the plan in the dashboard (steps, commands/manifests as text, expected effect, rollback), may edit, remove a step or deny.
   Approval is a normal approval of kind `ops.plan` through the approvals router (destructive plans need an approver who is not the requester
   admin when the scope's four-eyes setting says so).
4. CARRY OUT, ONE STEP AT A TIME, ADMIN PRESENT: after approval the assistant asks "run step N?"; `read` steps run freely; every `action` and
   `manifest` step marked destructive needs the admin's click for THAT step (the click is a signed UI event bound to the plan id, step number and
   parameters hash; changing a parameter invalidates it); a step runs as the admin through the same endpoint the dashboard button uses, so roles and
   gates are identical. A STOP button halts after the current step and cancels the rest; the session ends if the admin leaves (no click for 10 minutes
   default; an administrator (tenant or operator level) may override it in `ops.assisted_fix_idle_timeout`, bounds 1 to 60 minutes) or the browser closes, and an unfinished plan stays at the step reached, with the rollback offered.
5. AUDIT: `ops.plan.proposed|approved|step_started|step_done|step_failed|stopped|rolled_back` with plan id, step, parameters hash, admin, the
   model name, and the outcome; the plan text is stored in the run history of the plane.
No autonomous infrastructure changes and no assistant-held credentials: the assistant's session has no kube token, no database password, no
vault access; the execution of a step is done by the platform with the admin's identity, and the assistant only receives the step's redacted
result. The registry marks `ops.propose_plan` as the only plan-writing tool and `ops.run_step` as the only execution tool; `ops.run_step` refuses
without a matching admin click record for destructive steps (test). A test asserts that no other `ops.*` tool mutates anything, and a second test
that a plan edited after approval cannot run until re-approved. The assistant's own text is shown as "suggested by the assistant" and never as an approval.
Nothing in this section is built.


## 4. Execution cluster registration

* A cluster is registered through the existing k3s binding model (`k3s.cluster` items in tenant config, `TENANT-CONFIG-PLAN.md` section 6): the
  binding holds `api_server`, a `credential` that is a vault reference, allowed registries, quotas, roles, four eyes. In `split*`, the
  CONTROL plane's vault holds the kubeconfig/token; the workflow plane never keeps it: it asks `kube-token` for a short-lived token for one
  Job. `enforce::k3s_decide` (binding exists, not frozen, credential belongs to the team and environment, promotion order, role, registry,
  four eyes) runs BEFORE the token is minted, so the environment rules still apply exactly as for a deploy (SURE: `cloudflows/executor.rs`
  already calls it).
* Execution targets are modelled with the SAME cluster-binding abstraction as `k3s.cluster` (decided 2026-11-04): a binding is `{name, api_server,
  credential: vault ref, allowed_registries, quotas, node_labels, roles, four_eyes, frozen}`. Platform execution clusters (the `clusters:` list in
  the topology file) are bindings OWNED BY THE PLATFORM (operator-level, in the control scope of the installation); tenant `k3s.cluster`
  bindings stay owned by the tenant. In v1 they stay SEPARATE: a tenant's sandbox Jobs run only on platform execution clusters named in
  `tenants.<t>.execution`, and a tenant's own bound clusters are deploy targets only. This keeps untrusted code off clusters a tenant bound
  for deployment (which may hold production workloads).
  CONVERGENCE PATH (long term): (1) share the code: one `ClusterBinding` type and one `decide` (the `k3s_decide` order of checks) used for both
  roles, with a `purpose: deploy | sandbox` field (default `deploy`); (2) platform bindings are written in the same item format (kind `exec.cluster`,
  same fields) so tooling, validation and the dashboard treat them alike; (3) later a tenant may bind its own cluster with `purpose: sandbox`
  (tenant-supplied execution capacity, its own cost and residency), allowed only when the platform enables `execution.allow_tenant_clusters`
  and only after the cluster passes a posture check (no hostPath, Pod Security restricted, NetworkPolicy enforced, egress proxy reachable); (4)
  `purpose: deploy` and `purpose: sandbox` on the same cluster stay forbidden unless two namespaces with separate credentials are used. Migration
  of the decision: no data change in v1; step 1 adds a `purpose` field defaulting to `deploy` (backward compatible), platform clusters get
  `purpose: sandbox` in the topology file, and tenant bindings are untouched.

* Network paths: sandbox namespaces get a default-deny NetworkPolicy (egress) allowing only the egress proxy service and the workflow result
  endpoint (and DNS, and the cluster's own kube-dns); ingress denied except from the workflow plane for browser sessions. NetworkPolicy needs
  a CNI that enforces it (k3s's default kube-router does; SURE for kube-router netpol support, GUESS for the exact config shipped here).
  Cloud metadata addresses and cluster-internal ranges are blocked at the proxy as well.
* Per-tenant namespaces versus per-tenant clusters: default is a namespace per tenant in a shared execution cluster, with ResourceQuota,
  LimitRange, Pod Security `restricted`, no hostPath, runtime sandbox class (gVisor/Kata if the operator installs it; not shipped). `isolation:
  cluster` for tenants that need a kernel boundary of their own: same code, another `exec_cluster` value. Say plainly: namespaces share the
  kernel; untrusted code that matters needs a sandbox runtime or a cluster of its own.
* The work volume: execution pods get a freshly prepared copy or a PVC in the execution cluster, never the tenant's main volume; results and
  artifacts come back through the result endpoint (size limits) or an object store reference. Cross-cluster volume sharing is not supported.

## 5. Migration from `single` to `split3`

Steps (one tenant at a time, reversible until step 6):

1. Upgrade the single deployment to the release that has the storage traits and the migrations (still `FileStore`; no change in behaviour).
2. `iyakki cloud plan --topology topology.yaml` prints manifests and a checklist; create the clusters, the database, the internal CA
   (`--init-ca`), the roles in the database.
3. Stand up CONTROL: import identity and config: roles, tenant config (`tenant-config*.json`, versions), the vault service data
   (ciphertext copied as is; keys unchanged). Verify with `iyakki cloud topology verify` (read-only comparison of counts and hashes).
4. `iyakki cloud migrate export --tenant T --to DIR` writes a versioned, checksummed export of every trait's data from `FileStore` (schedules,
   runs and logs, approvals, notifications, inbox, deliveries, escalation state, audit) in an engine-neutral format (JSON lines + manifest, same
   spirit as `cloudsnapshot.rs`); `import` loads it into `PgStore` in one transaction per table with idempotent keys, so it can be re-run.
5. Cutover options: (a) ONE-SHOT: freeze the tenant (read-only maintenance flag: schedulers and hub workers stop, API says maintenance), export,
   import, verify counts and a sampled hash compare, switch the tenant pod to `--plane workflow` with `PgStore`, resume. Downtime = copy time
   (GUESS minutes for typical tenants). (b) DUAL-WRITE (later, only if needed): a `DualStore` writes both and reads the file store, a
   background reconciler compares; riskier (two sources of truth), avoid unless downtime is unacceptable. Recommended: one-shot.
6. Point-of-no-return: when the freeze ends on the new store and the old files are kept read-only for N days (rollback = export from Pg back to files
   with the same tool, tested). Then retire the file state.
7. Add EXECUTION: register clusters, switch `executor` from in-process/same-cluster to remote Jobs per schedule (the executor already decides).
8. Upgrade ORDER for every release: database migrations first (expand only), then control, then workflow, then execution; the previous major of
   the internal API must remain accepted by the newer plane. Rollback order is the reverse; contract migrations only a release later.

Data safety rules: no export without a snapshot taken first (`cloudsnapshot.rs` exists); import refuses a non-empty tenant unless `--replace`;
checksums per table in the manifest; the audit gets `migrate.*` events.

## 6. Implementation roadmap (agent-sessions: S = 1, M = 2 to 3, L = 4 to 6; GUESS) (revised 2026-11-04)

| # | Step | Size | Notes / acceptance |
|---|---|---|---|
| 1 | Storage traits (`iyakki-store`) and move `notifyhub`, approvals, `cloudflows` leases/quotas, `iyakki-flow` run store and scheduler state, audit sink and tenant config store behind them with `FileStore`; NO behaviour change; add `queued_late` / `due_at` / `late_by` fields to the run and approval records (unused until step 7) | L | all existing tests pass unchanged; a trait conformance suite exists and passes for `FileStore`; no file path built outside the store |
| 2 | Client spike: `postgres` + `tokio-postgres` + our `iyakki-pg-tls` with the four modes of 2.4b (2.4a, built here first as the secure-package pilot, 1 to 2 sessions extra; break-glass needs the audit and notification pieces, so `Unverified` is switched on only after step 5/9) + `r2d2_postgres` in `iyakki-store` behind feature `pg`: SCRAM over TLS `verify-full`, prepared statement, `SKIP LOCKED` loop, LISTEN/NOTIFY; measure binary size and compile time; `cargo deny check`; musl static build; write the exception note and the `pg_client_is_permissive_and_pinned` test | S | gate: licences pass, no OpenSSL/native-tls/libpq in the tree, static musl links, size delta recorded. If it fails, fall back to the adapter variant in 2.4 or back to the owner |
| 3 | `PgStore`, schema, migrations, RLS, conformance suite against Pg, `iyakki cloud db migrate|drill`, retention sweep | M | same suite passes for both stores; migration checksum and version-skew tests |
| 4 | Topology file (incl. `control.scope`, `database.in_cluster`, late-policy defaults), validator, `iyakki cloud plan` (manifests per plane, in-cluster Postgres outline made real, secrets generated and never printed, shown first like `cloudcli.rs`), `--plane` flag | M | example file validates; output passes `kubectl apply --dry-run=client` with the fake kubectl pattern |
| 5 | Internal API v1 + mTLS (TLS dialer, internal CA per scope, leaf renewal, authorize cache + events, run tokens, secret-use endpoint, health endpoint) | L | negative tests: wrong plane identity, wrong scope, expired token, replay, skew; fixes the listener-to-pod mTLS gap too |
| 6 | `split2` profile (execution remote Jobs via kube-token, NetworkPolicy generator, result endpoint, `ClusterBinding` shared type with `purpose`) | M | local multi-process test |
| 7 | Late delivery: durable `queued_late`, releaser with throttling, `late_policy`/`max_late`/`on_expired`/`allow_cached`, warnings in run history and notifications | M | fake-clock tests of a 6-hour outage, order, throttle, expiry, catch-up collapse |
| 8 | `split3` profile (workflow plane on Pg, multi-replica leases and SKIP LOCKED, notify outbox for control outage) | M | multi-replica soak test with a real Pg container |
| 9 | Dashboard (`plane_health`, page, admin actions with approval gating) and the read-only `ops.*` assistant tools | M | test: read tools mutate nothing; approvals required for destructive actions |
| 9b | Assisted fix: `ops.propose_plan`, plan review page, `ops.run_step` with per-step admin click, stop button, audit (2.. 3.5) | M | tests: destructive step refuses without a click record; edited plan needs re-approval; assistant session holds no credentials |
| 10 | Migration tool (`export`/`import`/`verify`, rollback) | M | round trip on generated data sets; kill mid-import and resume |
| 11 | Reports with DuckDB over exports (optional) | S | MIT; CLI only; off the cloud path |
| 11b | One-off audit of existing security code against the secure-package standard (2.4c) | M | findings file; fixes listed; no rewrite |
| 12 | Hardening: rate limits, vault replay nonces, drill schedule, docs; the `secure package` CI script applied to every security crate; `team` scope only if asked (see 1.5 list) | M | |

Total GUESS: about 25 to 34 sessions (no in-house client; the TLS adapter, dashboard, late delivery and assisted fix are added). Step 1 alone is valuable and comes first.


### 6.1 What can be tested without a multi-node cluster in CI (it is not available)

* Pure logic: topology validation, plan generation, authorization table per plane, token mint/verify, migration checksums: unit tests.
* Stores: the conformance suite against `FileStore` always; against a fake pg always; against a real Pg only in `#[ignore]` tests when docker is present.
* Planes as separate processes on loopback ("docker-compose style" without compose being required): three `iyakki serve --plane ...` processes
  with different ports, certificates from `rcgen`, a Pg container, and a fault-injection wrapper (kill a plane, drop a port, skew the clock via an
  injected clock value) to test the outage table. Manifests are validated with the fake `kubectl` and `--dry-run=client` only.
* NOT testable here: real NetworkPolicy enforcement, real node loss, volume behaviour, kube-token minting against a live API server, cross-cluster
  latency. Mark these as "not verified" in the doc status line when steps 4 to 7 land, as the other cloud plans do.

## 7. Risks

* Complexity and operations burden is the main risk: three clusters, a database, a CA. Mitigation: `single` stays default and complete.
* The Postgres client adds `tokio` and a TLS adapter (possibly single-maintainer) to the cloud binary. Mitigation: feature `pg`, pins, licence test, the trait seam, spike first.
* Authorize latency and cache staleness (revocation window up to the cache TTL). Mitigation: push events, short TTL, deny on doubt.
* Late delivery can surprise people (a run at the wrong hour) or stampede a recovering plane. Mitigation: visible `late_by`, `late_policy`, `max_late`, throttled release, catch-up collapse.
* Run logs may hold secrets; the database becomes a high-value target. Mitigation: redaction, encryption flag, short retention, least-privilege roles.
* Version skew between planes during upgrades. Mitigation: expand/contract migrations, one-major API window, start-time checks.
* RLS gives a false sense of safety (workflow workers see all tenants). Mitigation: say so; tests with a per-tenant role for the reporting path.
* The not-yet-read plans (`CLOUD-FLOWS-PLAN.md`, `NOTIFICATIONS-PLAN.md`, `CLOUD-SANDBOX-PLAN.md`) may change store shapes; re-read before step 1.

## 8. Owner decisions (2026-11-04) and what is still open

Decided:
1. No in-house PostgreSQL client: use the famous permissive `postgres`/`tokio-postgres` family with a rustls adapter and r2d2 pooling, pinned, behind `iyakki-store` (2.4). Exception to "build in-house" recorded there.
2. PostgreSQL is the shared database; DuckDB for reports only; SQLite optional for `single`.
3. The operator runs the database; an optional in-cluster StatefulSet + PVC + backup CronJob + restore drill + NetworkPolicy is outlined (2.7).
4. Control plane scope is configurable (`installation | tenant | team`), default `tenant` (1.5).
5. Platform execution clusters and tenant `k3s.cluster` bindings stay separate in v1 and converge through one cluster-binding type with `purpose` (4).
6. Control down: queue durably, deliver late with a warning (`late_by`), per-schedule policy; operations dashboard and assistant as in 3.4 and 3.5.

Also decided 2026-11-04 (answers to the four questions):
A. `team` control scope is kept possible but NOT built in v1; scope is an opaque `(kind, id)` pair everywhere; the parser accepts `team` and says "not built yet" (1.5).
B. `max_late` default 6 hours, `on_expired: notify`; four-eyes production environments 1 hour then `skip`; per-schedule overrides (3.4).
C. We write our own thin TLS adapter `iyakki-pg-tls` as a "secure package" under a written standard; the Postgres client stays the famous crate (2.4a).
D. The assistant may suggest and help fix with the admin present, step by step, with per-step clicks for destructive steps, a stop button, full audit and no credentials (3.5).

Decided 2026-11-04 (answers to the last three questions):
1. The secure-package standard is applied to existing security code as a one-off checklist audit, not a rewrite (2.4c; roadmap 11b).
2. Assisted-fix idle end defaults to 10 minutes; an administrator (tenant or operator level) may override it, bounds 1 to 60 minutes; a sole admin runs
   non-destructive steps alone, and each destructive step still needs its own click.
3. Certificate verification is forced by default; the modes are `VerifyFull`, `VerifyCa`, `PinOnly` and the admin break-glass `Unverified`, time-limited,
   audited, loud, refused in production unless tenant policy allows; plaintext does not exist (2.4b).

Still open: none that block starting step 1. (Smaller points to settle during build: the exact minimum reason length, the renewal approver for
`Unverified` in production, and the `PinOnly` rotation procedure; each has a recommended default in 2.4b.)

## 9. Not done / not verified

Everything here is a plan (updated for the owner's answers of 2026-11-04). No code was written; no cluster was touched; no web search was used (licence facts are from memory, marked GUESS where
doubtful); sizes are estimates; the sibling plans were not available to read; multi-cluster behaviour has not been tried anywhere.
