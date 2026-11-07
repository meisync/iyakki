# Tenant-level configuration (shared agents, models, git, k3s, egress, media)

Status: plan and core built 2026-11-04 (module `crates/iyakki-agent/src/tenantconfig/`, API `/api/v1/tenant/config...`). Wired end to end: the
shared-agent switch at the web service's run start, the media policy for browser sessions, and (second pass, 2026-11-04) the egress proxy's narrowing rule, the web clone guard,
the gate for team providers and local hosts at run start, the fair-share call site `runtime::call_local`, the approval-filing deploy gate `runtime::deploy_gate`
(no caller yet: no tenant deploy exists), model discovery, the real vault binding `vault::for_user`, the page `ui/tenantcfg.js`, and the tenant directory writer
`iyakki cloud tenants`. What is still open is in section 10. Owner's request: "options for tenant level configuration for
different items like shared Claude agent, shared Copilot, k3s, git shared and others". Related: `CLOUD-MULTITENANT-PLAN.md` (tenants, roles,
the common vault), `MINIMAL-WEBSERVICE.md`, `../FIREWALL.md`, `BROWSER-MEDIA-DEVICES.md`.

## 1. The model: config plus vault

A tenant's configuration is two things together (owner, 2026-11-04):

1. a declarative CONFIG document: non-secret settings only, one schema, versioned. Safe to export, diff, review and commit.
2. the tenant's common VAULT: every secret. A config field that needs a secret holds only a reference, `vault:<ns>/<name>`
   (for example `vault:agent.claude/token`). The vault is never exported. A reference is resolved only at the enforcement point, by
   "use a secret for a named action" (`SecretStore::use_secret`): the point gets the value for that call, the vault records the action and
   the reference, nobody gets a copy.

Effective state of an item = config + vault status:

| state | meaning |
|---|---|
| `ready` | switched on, valid, every secret it needs is set |
| `secret_missing` | switched on, a referenced secret is not in the vault (the item stays off; set the secret later, the config needs no edit) |
| `vault_locked` | the vault is locked or not bound; same, nothing is used |
| `disabled` | switched off by its own level or by a higher one (`reasons` says which), or not configured |
| `invalid` | switched on but a required reference is absent |

An item that is off asks nothing of the vault. Reading never opens the vault, only asks whether an entry exists.

## 2. Where the config lives

Per tenant, under the tenant's data directory `cloud/config/`:

| file | who writes | what |
|---|---|---|
| `tenant-config.json` | the operator or deployment (for example a Kubernetes ConfigMap mounted read-only) | the declarative document; read on every use, so a reload is an edit of the mounted file; the service never writes it |
| `tenant-config.local.json` | the API, atomic write (`iyakki_schema::write_atomic`, mode 600) | what tenant admins edited: `{schema, version, items}` |
| `versions/NNNNNN.json` | the API | a snapshot per change with version, time, who, hash, summary; the last 10 kept |
| `usage.json` | the enforcement points | per-day counters of shared agent use (names and numbers only) |

Format (same for both documents):

```json
{"schema": 1, "items": [
  {"kind": "egress.policy", "enabled": true, "managed": true, "values": {"allowed_domains": ["*.corp.example"]}},
  {"kind": "agent.claude", "enabled": true, "values": {"mode": "per_person", "allowed_models": ["*"]}},
  {"kind": "k3s.cluster", "namespace": "web", "name": "prod", "enabled": true,
   "values": {"api_server": "https://web-prod.k3s.example.com:6443", "credential": "vault:k3s.cluster/web-prod-credential",
              "allowed_registries": ["registry.example.com"], "deploy_role": "admin"}}
]}
```

An item is keyed by `(kind, namespace, name)`: `namespace` is the team (absent = the tenant level), `name` is `default` for a single-value kind
and the key of a collection otherwise. `managed: true` (only meaningful in the operator's file) makes the item read-only through the API
(409). The effective view says where each item comes from: `source: file | api`. An unmanaged file item is a default which an API item
with the same key replaces. A broken operator file leaves no operator items (the safe reading) and the list says `file_error`.

## 3. Registry of item kinds

Each kind has: id, schema (fields, types, limits, secret fields), cardinality (single value or keyed collection), whether a team may have its own
entries, whether a person may narrow it, who sees the values, who writes. Field types: bool, int range, text with a check (plain, host,
host pattern, registry, https url, base url, model, label, device), enum, bounded list, `secret_ref` (with the vault namespace allowed for the kind).
Unknown settings are refused. An inline secret in a `secret_ref` field is refused and the message never repeats the value.
A client cannot send a secret except in the write-only `secrets` object of a PUT.

First release (implemented, schema + validation + enforcement function):

| kind | collection | secret refs (vault namespace) | main fields | enforcement point | wired |
|---|---|---|---|---|---|
| `agent.claude` | single | `token` (`agent.claude`) | `mode` per_person/shared, `allowed_models`, `terms_ack`, `daily_requests_per_person`, `daily_requests_tenant` | `enforce::agent_gate`, `with_agent_token` | run start in the web service |
| `agent.copilot` | single | `token` (`agent.copilot`) | same | same | same |
| `agent.other` | by name | `token` (`agent.other`) | same | same | hook |
| `models.provider` | by name | `api_key` (`models.provider`) | `api`, `base_url`, `allowed_models`, `daily_tokens_per_person` | `enforce::provider_models` | hook |
| `git.remote` | by name | `credential` (`git.remote`) | `url_prefixes`, `access` read_only/push, `protected_branches` | `enforce::git_remote_allows` | hook |
| `k3s.cluster` | by team and environment | `credential` (`k3s.cluster`) | see section 6 | `enforce::k3s_decide` | hook |
| `k3s.environments` | single | none | `order` (promotion chain) | `enforce::promotion_order` | used by `k3s_decide` |
| `egress.policy` | single | none | `mode` allow_list/deny_all, `allowed_domains` | `enforce::egress_allows` | hook |
| `media.policy` | single | none | `media` allow/deny, `deny_devices` | `enforce::media_policy_for` | browser sessions (`Policy::for_tenant`) |
| `flows.quota` | single (team may lower) | none | `max_runs_tenant`, `max_runs_person`, `max_run_secs`, `max_schedules`, `history_keep`, `history_days`, `missed_notify_secs` | `cloudflows::quota::Quota::load` | scheduler service, engine admission gate, schedule route (2026-11-04) |
| `schedule.timezone` | single | none | `timezone` (UTC, an offset, or a zone name when the image has zone files) | `cloudflows::real::tenant_zone`, `cloudflows::tz` | cron triggers whose zone is `local`, preview route |
| `flows.unattended` | by name, per team | `secret` (`flows.unattended`, `vault:team/<team>/<name>`) | `secret`, `terms_ack` | `cloudflows::identity::secret_gate` | scheduled runs with nobody signed in; saving a schedule |

Named in the plan, not built (a request is refused as "planned"): `git.credentials` (per-person key pairs, today in the person's own
Git and SSH keys page), `mcp.server` (MCP and UTCP servers), `store.source` (`../STORE.md`), `quotas` (cpu and memory per team across kinds),
`retention.audit`, `sso`.

## 4. Scope, precedence, and who may do what

Layers, highest first: operator policy (items the operator's file marks `managed`), tenant, team (namespace), person. Rules:

* A lower layer can only NARROW. The merge is by field: lists intersect (with `*`, `*.example.com`, `claude-*` and URL prefixes as the
  "covers" relation), numbers take the smaller, booleans of permission are AND, restrictions (frozen, four eyes) are OR, an enum takes the stricter
  value, text takes the highest layer that sets it, lists of constraints (node labels, protected branches, denied devices) are united.
* A layer that is off switches the item off whatever the others say. Deny always wins. Defaults are off: nothing is configured until an admin does it.
* An egress or media policy that exists and is switched off denies everything (delete it to fall back to the machine's setting).
* A person layer counts only for kinds that allow it (`agent.*`, `media.policy`); no API stores person layers in the first release.
* A tenant-level `k3s.cluster` entry is a template: it carries defaults and rules (and `enabled: false` forbids that environment for every team);
  nothing in it is required.

| action | who |
|---|---|
| see that an item exists and its state | every member of the tenant (team entries: that team's people and tenant admins) |
| see the values | tenant admins; members for kinds that say so (egress, media); a team's people for their team's cluster bindings according to the binding's `read_role`, a team admin for other team entries |
| write | tenant admin or owner; a team's own admin for its team's entries of kinds that allow teams; never an item marked `managed` |
| use | every member, through the enforcement point; a secret is never visible to the user |
| operator | writes the file; there is no operator role in the tenant API |

## 5. Shared coding agents (Claude, Copilot, others): what is possible

Technically, three arrangements exist: (a) each person signs in with their own account, (b) the tenant holds one API key (Claude: an Anthropic
API key, pay per use; Copilot: not a key, an organisation seat model), (c) the tenant holds one login of a person and everybody uses it.

Honest assessment, to be confirmed against the providers' current terms by the owner before any tenant turns it on:

* Per-person login (default, `mode: per_person`): each person uses their own subscription or seat. No terms risk. The tenant's switch only
  decides whether the agent may run in the tenant at all and which models. This is the default and needs no secret.
* Shared API key (`mode: shared`, Claude): technically sound (an API key is meant for programs), billed to the tenant; the tenant is then the customer
  responsible for its users' use under the provider's usage policy. Per-person accounting and daily limits are ours, enforced at run start.
* Sharing one consumer subscription login among several people is, as far as the common terms of such plans read, not allowed ("one person, one
  account"), and for Copilot a seat is per person and assigned by the GitHub organisation. We do not build a way to share a person's login; `shared`
  holds a key meant for shared use. Because we cannot decide the contract, `shared` needs the admin's `terms_ack: true` (the admin confirms they
  checked) and every shared use leaves an audit line. For Copilot there is likely no honest shared mode beyond organisation-managed seats; the kind
  exists so the tenant can switch the agent on or off and restrict models, and `shared` stays unusable until the owner decides (open question 2).

Shared mode needs: `token` (vault reference), `terms_ack`, `daily_requests_per_person`; optional `daily_requests_tenant`, `allowed_models`.
Per use: `enforce::agent_gate` checks switched on, state ready (secret set, vault open), model allowed, the person's and the tenant's daily counters
(`usage.json`), then writes `tenant.agent.use` (kind, model, counters; never the token) or `tenant.agent.refused` (reason). The launcher then
calls `with_agent_token(..., action, f)`, which hands the token to `f` for the named action and writes `tenant.secret.use`. Rate per minute and
spend caps in money are not built.

## 6. k3s: clusters per team and environment

Not one cluster per tenant: a team has one for dev, one for preprod, one for prod, and a tenant may name its own environments.
`k3s.cluster` is a collection keyed by `(team, environment)`: API `/api/v1/tenant/config/k3s.cluster/{team}/{environment}`; tenant-level
entries (`/k3s.cluster/{environment}`) are templates.

Binding fields: `display_name`; `api_server` (https); `credential` (secret_ref; its name must start with `<team>-<environment>-` and no two bindings may
share one, so a dev credential can never serve prod: checked at write and again at the enforcement point); `allowed_registries` (cover
patterns); `quota_cpu_m`, `quota_mem_mib`, `quota_pods`; `node_labels`; `read_role`, `deploy_role`, `admin_role` (viewer, editor, admin, tenant_admin;
defaults viewer, editor for the first environment and admin after, tenant_admin); `four_eyes` (default on for the last environment of the chain);
`approver_role` (admin = team admin, the default; or tenant_admin); `frozen`. The effective view adds `reachability` (`unknown` until a probe is bound).

`k3s.environments` holds the promotion chain, default `dev, preprod, prod`. Decision `enforce::k3s_decide(env, ask)` with `(team, environment, requester, rank,
approval, promoted_from, registry)` answers `Allow`, `NeedsApproval{approver_role}` or `Deny(reason)`, in this order: binding exists, not switched off by any
level (a tenant template with `enabled: false` forbids an environment everywhere), not frozen, state ready, credential belongs to this team and
environment, environment in the chain and promoted from the previous one, the requester's role, registry allowed, four eyes (see below). Every decision writes `tenant.k3s.deploy` with team, environment, who, decision, approver, reason.
Precedence holds: a team cannot unfreeze or lower four eyes under a tenant that sets them. A team sees only its own bindings (others answer 404).
Needs a real cluster to verify: applying quotas, probing reachability, running the deploy, storing approvals.

`git.remote` and `models.provider` are collections too (several named entries, optionally per team), with the same rules.

### Approvals (owner decisions, 2026-11-04)

All approval goes to the team (namespace) of the requester. One router, `tenantconfig::approvals::route`, is used by every enforcement hook:

1. holders of the NAMED approver role for the scope (`approver:<environment>` or `approver:<kind>`, for example `prod`, `k3s.cluster`) in the requester's team, groups expanded to people at decision time, the requester excluded; with `approver_mode: role_only` they alone decide, otherwise the team's admins are approvers too (default: role only for the last environment when the team has any holder, else role or admin);
2. if none: the team's admins except the requester;
3. if none: the tenant admins, flagged as an escalation in the reason and the audit.

The tenant may only narrow (`approver_role: tenant_admin` for a kind or environment, or `approver_mode: role_only`); a team cannot widen. An admin or holder of another team is never eligible. Nobody approves their own request, even as the only holder or admin (the chain moves on). Guests never request a deploy. Re-evaluated at decision time: a person removed from a group or from the tenant loses the right at once.

| item kind | who requests | who approves | escalation |
|---|---|---|---|
| `k3s.cluster` deploy with four eyes (default for the last environment) | team members whose role allows the deploy (operator, admin, user; not guest) | approver-role holders of the team, then (unless role only) team admins | tenant admins when no one else in the team |
| shared agent quota override, git push to a protected remote, egress additions, provider enablement | any member | same router (kind as scope) | same |

Only the k3s decision asks for approval today; the other rows use `approvals::request` once their hooks are wired.

The approver role grants only "may approve requests of that scope in this team": no read or write of config, no deploy. It is a small list in the team record (`approvers` in `roles.json`: scope to people and groups), edited by the team admin or a tenant admin; grants and revocations are audit lines. Groups are named lists of people (no nesting, at most 50 groups of 200 people, 50 holders per role), defined per team or tenant-wide (tenant admins only); a team may use its own and the tenant-wide ones, never another team's.

Pending approvals: `approvals.json` beside the config; they expire (default 24 h, `IYAKKI_APPROVAL_TTL_SECS`, 60 s to 30 days). Audit: `tenant.approval.request|approve|deny|refused` with id, kind, team, item, requester, approver, who could approve, `escalated`, `via` (role, team_admin, escalation, tenant); no secret values. The effective view of a binding lists the expanded approver-role holders to team admins and tenant admins only.

## 7. API (cloud profile)

All routes need sign-in, origin and rate checks like the others; PUT, DELETE and POST are audited (`api.tcfg_*`).

| call | what |
|---|---|
| `GET /api/v1/tenant/config` | items with state, source, `can`, values where allowed; tenant admins also get the registry (kinds, fields, secret flags) |
| `GET /api/v1/tenant/config/{kind}` | single value, or all entries of a collection |
| `GET /api/v1/tenant/config/{kind}/{name}` | a named entry; for a single-value kind the team's value |
| `GET /api/v1/tenant/config/{kind}/{team}/{name}` | a team's entry |
| `PUT` the three forms | body `{enabled, values, secrets}`; `secrets` is write-only, stored in the vault, the config keeps the reference; a value `[set]` keeps the stored reference |
| `DELETE` the three forms | remove an entry (vault entries stay; remove them in the vault) |
| `POST /api/v1/tenant/config/{kind}/validate` | dry run: problems, the state it would have, which secrets it would store; body also has `namespace`, `name` |
| `GET /api/v1/tenant/config/versions`, `POST .../rollback {version}` | last 10 versions; a rollback restores one as a new version |
| `GET /api/v1/tenant/approvals`, `POST .../approvals/{id}/approve`, `POST .../approvals/{id}/deny` | pending requests only for people who may decide them; expire after 24 h by default |
| `GET /api/v1/namespaces/{team}/approvers`, `PUT`/`DELETE .../approvers/{scope}` body `{people, groups}` | approver roles of a team (team admins, tenant admins) |
| `GET /api/v1/namespaces/{team}/groups`, `PUT .../groups/{group}` body `{members}`, `DELETE` | groups of people; team `_tenant` means the tenant-wide groups (tenant admins) |
| `GET /api/v1/tenant/config/export`, `POST .../import {doc, dry_run, replace}` | config only, no secret; all or nothing; items the file manages are skipped and listed |

URL forms accept `k3s.cluster` or `k3s-cluster`. Errors: 400 invalid (messages name the field, never the value), 403 role, 404 not found or a team the
caller is not in, 409 managed by the file or the vault is locked when writing a secret.

Audit (also appended to `cloud/roles-audit.log`): `tenant.config.put|delete|rollback|import|export`, with `kind ns name before= after= fields= secrets_set=`: hashes of
the item (12 hex of SHA-256 over enabled and values, which hold references only) and the NAMES of changed fields, never values.

## 8. Where each item is enforced (the exact functions)

* Shared agent: the web service's `runs_start` (`webservice/handlers.rs`, `agent_gate`) calls `tenantconfig::enforce::agent_gate` when the request names provider
  `claude` or `copilot`; refusal is 403. Still to wire: the desktop and cloud shells' agent launchers (`agent_login.rs`, the Agents settings) and the coding-agent
  tile, which must call `agent_gate` and `with_agent_token`.
* Team providers, API-key integrations and local hosts: the same `agent_gate` in `runs_start` hands any other provider name that is a `models.provider`, `integration.apikey` or
  `models.local` entry to `webservice/tenantcfg.rs` `provider_entry_gate` -> `runtime::provider_gate` (ready, not blocked, model on the list or `provider_models`, `use_role`, per-person
  daily and team monthly request counters in `usage.json`, audit `tenant.provider.use|refused`). `daily_tokens_per_person` is not counted.
* Local hosts: `runtime::call_local(env, team, name, user, rank, model, now, clock, f)` is the call site for an actual request: gate, then `runtime::acquire` (the fair-share limiter with
  per-person and team slots and the queue timeout; the slot is released when `f` returns), then the bearer token from the vault for that call only. No chat backend calls it yet.
* Media: `browsermedia::Policy::for_tenant` (called by `iyakki-remote/src/screen.rs` when a browser session starts) reads `media.policy` from the folder named by
  `IYAKKI_TENANT_DATA_DIR`; the pod manifest (`cloudtenant.rs`) now sets it to `/var/lib/iyakki`. Without the item the machine's `IYAKKI_BROWSER_MEDIA` stays in force.
* Egress: `iyakki_proxy::EgressProxy::with_narrowing` takes a rule that can only deny; `AiConfig::build_proxy` installs `enforce::egress_narrowing` when `IYAKKI_TENANT_DATA_DIR` is set (read on every
  call; an unreadable configuration denies). The firewall rules (`iyakki-fw`) do not read the tenant policy yet.
* Git: `webservice/files.rs` `clone` asks `runtime::git_clone_guard`: no `git.remote` entry for the team or tenant changes nothing; any entry makes a ready entry's `url_prefixes` mandatory. There is no push route; a push path
  must call `enforce::git_remote_allows` (second value) and use the `credential` by reference.
* Models: `enforce::provider_models` is used by `provider_gate` for `models.provider` entries.
* k3s: there is no tenant deploy in `cluster.rs`, `clusters.rs` or `k3sdev.rs` (they manage a person's own clusters). A team deploy must call `runtime::deploy_gate(env, &DeployAsk, now)` before it touches a cluster:
  it returns `Go`, `Pending {id}` (a request was filed with `approvals::request`; a person approves it with their own click on the approvals page) or `Denied`; an approved request is consumed once.
* Vault: `vault::for_user(data_dir, user)` is what the handlers use: the bound store if any, else the vault service of `IYAKKI_VAULT_URL` with the member's key from `cloud/vault-keys/<user>.key`, else locked.
* Tenant directory: `iyakki cloud tenants add|remove|list|home` (`cloudtenantdir.rs`).

## 9. Export, import, rollback, validation

Export is config only (references, no values), same schema as the operator's file, so a tenant can be cloned or committed to git. Import validates every item first and
applies all or nothing, as one version. Every change is validated before it is saved (the same function as the dry run) and saved as the next version; the last ten
are kept; a rollback is itself a new version and cannot touch the operator's file.

## 10. Not done yet / needs the real thing

* Member keys for the vault: `vault::for_user` needs a key file for each person on the tenant volume. Nothing enrols members' keys automatically, so until a person has one the vault reads locked for them (a shared agent token must be readable by every member). Never run against a real vault service.
* A real browser look at the page `ui/tenantcfg.js` (it is tested under node with a fake page); the operator's `tenant-config.json` is not yet mounted as a ConfigMap by the manifest.
* A tenant deploy path (see section 8), the chat backend for a team's provider or local host (so `call_local` has a caller), a git push route, egress in `iyakki-fw`.
* Person layers storage, namespace-level read of `agent.other`, rate per minute, money caps, token counts for `daily_tokens_per_person`.
* Reload without reading the file on each request (a cache with mtime like `cloudaccess` if load shows).

## 11. Open questions for the owner (recommended answers)

1. DECIDED by the owner, 2026-11-04: a team admin can add Claude or other integrations using an API key (never a shared personal login), within the tenant policy; see section 12.
2. Copilot: offer `shared` at all? Recommended: no for now; the kind only gates and restricts models, until GitHub's organisation-seat terms are checked.
3. DECIDED by the owner, 2026-11-04: see section 6 (approval goes to the requester's team: approver-role holders, then team admins, then tenant admins as an escalation).
4. DECIDED in part by the owner, 2026-11-04: team admins add API-key integrations for their team (section 12); the tenant can forbid, narrow or require approval. Bindings, egress and git entries for a team follow the same narrowing rule.
5. Should the operator's file be able to hide kinds from tenant admins entirely (for example egress locked by the platform)? Recommended: yes, that is what `managed: true` is for; no extra switch.

## 12. Team integrations, providers and local hosts (owner decisions, 2026-11-04)

A team admin may add an integration with the team's own key, within the tenant's policy: `agent.claude`, `agent.copilot`, `agent.other`, `models.provider`, `models.local` and `integration.apikey` (a keyed entry per team and name: `PUT /api/v1/tenant/config/{kind}/{team}/{name}` with the write-only `secrets` object). The key goes to the team's slice of the vault (`vault:team/<team>/<name>`), is never readable, and is rotated by sending a new one (the old one is gone). The entry has an enabled switch, an allowed-model list, a per-person daily limit, a team cap per 30 days and `terms_ack` (billing goes to the key's owner, usage is audited per person). Members use it through the normal run start (`namespace` names the team). Sharing one person's Claude or Copilot sign-in with a team is NOT built.

Tenant policy (`integration.policy`, a single tenant-level item): `team_integrations` allow or deny, `allowed_providers`, `allowed_models`, `new_provider_needs_approval` (a team admin's new provider then waits for a tenant admin: the one exception to "approvals go to the team admin", because it is a tenant-level decision), `allow_private_endpoints`, `internal_hosts`, `deny_hosts`, `allow_same_host`, the safety thresholds and `require_four_eyes_prod`, `audit_enabled`. An operator-managed entry cannot be edited.

Catalog (`tenantconfig/catalog.rs`, data): anthropic, openai, gemini, openrouter (optional `site_name`, `referer`), azure_openai (`base_url`, `deployment`, `api_version`), mistral, openai_compatible (any base URL), git_host, other. Each has a default base URL, auth header style, key-shape hint (a warning only, never a refusal), the model-list path, a static model list, and the egress domains it needs; an enabled entry whose domain the tenant egress policy does not allow reads `disabled` with the domain named. Several entries of one provider per team are allowed. `POST .../{kind}/{team}/{name}/test` is the admin-triggered connection test (a trait `ConnectionTester`; at most once a minute per entry; stores ok, time and HTTP status only). The real caller is `realtest::RealTester` (the proxy's `UreqUpstream`, narrowed by the tenant egress policy); `NoTester` is the always-refusing stand-in for tests. Model discovery (`POST .../{kind}/{team}/{name}/models`, `discovery.rs`) parses Ollama `/api/tags` and OpenAI-style `/v1/models` lists.

Local model hosts (`models.local`): Ollama, Lemonade, LM Studio, vLLM and llama.cpp from the catalog with a suggested, editable port (none for Lemonade, where it is not known) and API style (ollama native or OpenAI-compatible). Address rules are the SSRF rules in `safety::ssrf_problem`: metadata and link-local, cluster API servers, denied ranges, loopback only with the tenant's same-host switch, and everything else only if the tenant lists it in `internal_hosts` (local hosts are not internet egress). Mandatory limits (per person, per team, queue timeout) with a pure fair-share limiter in `fairshare.rs` (`admit`, `next_to_run`) and a health state (`healthy`, `auth_failed`, `unreachable`, `failing`, `unknown`) from the last test. Built: `call_local` with the limiter and queue timeout (no chat backend calls it yet) and discovery. Not built: publishing a team host as a Service in the team's namespace the way `modelhost.rs` publishes a Mac host (`publish_to_cluster` is a stored hint only).

## 13. Safety rules and warnings

`tenantconfig/safety.rs` is a pure layer. Every PUT and dry run returns `warnings: [{code, severity, field, text, doc}]`; every read recomputes them (a stale key warns without a PUT), the highest severity is on the item, and a `block` also holds for an item the operator's file provides (it reads `invalid`, never silently applied; the hooks `agent_gate`, `k3s_decide`, `git_remote_allows`, `provider_models` consult the same rules). A `block` refuses the save; on enabling, each `warn` must be listed in `ack_warnings` (checked on the server); the audit line records the warning codes and the acknowledged ones, not the texts. No text carries a setting value. Facts the rules need (when a key was set, the last test status) are in `meta.json` beside the config.

| code | severity | when | why | how to fix |
|---|---|---|---|---|
| <a id="safety-spend-cap-missing"></a>`spend.cap.missing` | block | a paid provider (anthropic, openai, gemini, openrouter, azure_openai, mistral) is enabled without both a monthly team cap and a per-person daily cap | an unlimited key is an unlimited bill | set `monthly_requests_team` and `daily_requests_per_person`; self-hosted endpoints are exempt |
| <a id="safety-agent-shared-limits"></a>`agent.shared.limits` | block | a shared agent has no per-person daily limit or no monthly team cap | one login used by everyone | set both limits |
| <a id="safety-terms-ack-missing"></a>`terms.ack.missing` | block | a team key is enabled without `terms_ack` | billing goes to the key's owner and use is audited per person | tick the confirmation |
| <a id="safety-secret-inline"></a>`secret.inline` | block | a key, or a URL with credentials or a key in its query, is in a non-secret field | config is exported, diffed and committed | put the key in the secret box only |
| <a id="safety-secret-foreign"></a>`secret.foreign` | block | a `vault:team/<other>/...` reference | a team must not reach another team's secret | use your own team's reference |
| <a id="safety-endpoint-http"></a>`endpoint.http` | block | plain http to a host outside the private ranges | traffic and keys would cross the internet unencrypted | use https |
| <a id="safety-endpoint-private"></a>`endpoint.private` | block | a private or loopback endpoint on an API-key integration while the tenant policy does not allow it | SSRF and exposure | ask a tenant admin to set `allow_private_endpoints`, or use a local model host entry |
| <a id="safety-egress-domain-blocked"></a>`egress.domain.blocked` | block | the provider's domain is not allowed by the tenant egress policy (the entry reads `disabled`, the text names the domain) | the machine could not reach it anyway | ask for the domain to be added to the egress allow-list |
| <a id="safety-k3s-prod-four_eyes"></a>`k3s.prod.four_eyes` | block | a prod binding switches four eyes off while the tenant requires it | prod changes need a second person | remove the override |
| <a id="safety-egress-wildcard"></a>`egress.wildcard` | block | an egress allow-list with `*` or a whole top-level domain unless the operator's file allowed it | it opens the internet | list the real domains |
| <a id="safety-audit-disabled"></a>`audit.disabled` | block | audit is switched off in the tenant policy while an item that handles a secret is on | no trace of who used a key | switch audit on (the service writes the lines either way) |
| <a id="safety-local-limits-missing"></a>`local.limits.missing` | block | a local model host without a per-person limit, a team limit and a queue timeout | a local GPU is scarce and shared | set the three limits |
| <a id="safety-local-ssrf"></a>`local.ssrf` | block | the host is a metadata or link-local address, a cluster API server, in the tenant's denied ranges, loopback without the tenant's same-host switch, or not on the tenant's internal hosts list | a team admin must not aim the service at arbitrary internal systems | ask the tenant to list the range in `internal_hosts` |
| <a id="safety-data-leaves"></a>`data.leaves` | warn | a key-based provider is enabled | prompts, files and tool results are processed by a third party outside the tenant (the text names the provider) | acknowledge, or use a local host |
| <a id="safety-models-unrestricted"></a>`models.unrestricted` | warn | no model allow-list, or `*` | any model, including expensive ones, may be used | list the models |
| <a id="safety-cap-high"></a>`cap.high` | warn | a cap above the tenant's thresholds (`warn_monthly_cap` 100000, `warn_daily_per_person` 2000) | a leak or loop costs a lot | lower the cap or raise the threshold in the tenant policy |
| <a id="safety-key-age"></a>`key.age` | warn | the key was set 90 or more days ago (`key_warn_days`) | old keys are leaked keys | set a new key |
| <a id="safety-key-age-strong"></a>`key.age.strong` | warn | 180 or more days (`key_strong_days`) | same, strongly | rotate now |
| <a id="safety-key-shared"></a>`key.shared` | warn | the same reference is used by several entries | one leak affects all | give each entry its own key |
| <a id="safety-test-never"></a>`test.never` | warn | no connection test has been run | a wrong address or key is found at the first real use | run the test |
| <a id="safety-test-failed"></a>`test.failed` | warn | the last test failed | the entry will not work | fix the address or key, test again |
| <a id="safety-limits-person-missing"></a>`limits.person.missing` | warn | no per-person limit on a self-hosted integration | guests and normal users are not capped | set a limit |
| <a id="safety-k3s-registry-missing"></a>`k3s.registry.missing` | warn | a binding without an image registry allow-list | any image, or none, can be deployed | list the registries |
| <a id="safety-k3s-quota-missing"></a>`k3s.quota.missing` | warn | a binding without a namespace quota | one deploy can take the cluster | set cpu, memory or pod quotas |
| <a id="safety-git-push-unprotected"></a>`git.push.unprotected` | warn | a git remote with push and no protected branches | direct pushes to main | list protected branches |
| <a id="safety-egress-many"></a>`egress.many` | warn | an egress allow-list with more than `max_egress_domains` (50) entries | long lists are hard to review | trim it |
| <a id="safety-approver-sole"></a>`approver.sole` | warn | a team with one admin and no approver-role holders on a four-eyes binding | that admin's own deploys escalate to a tenant admin | add a second admin or an approver-role holder |
| <a id="safety-local-noauth"></a>`local.noauth` | warn | a local host with no token | everyone who can reach that address can use it | bind it to a private interface, or add a token or a reverse proxy |
| <a id="safety-local-http"></a>`local.http` | warn | plain http inside the network | traffic is not encrypted on the wire | use https if the server supports it |
| <a id="safety-local-model_files"></a>`local.model_files` | warn | a local host is enabled | model files and licences are the team's responsibility | acknowledge |
| <a id="safety-info-key-scope"></a>`info.key.scope` | info | a key is in use | where it lives and what uses it | none |
| <a id="safety-info-usage-audit"></a>`info.usage.audit` | info | a key is in use | who will see usage | none |
| <a id="safety-info-nonprod-approval"></a>`info.nonprod.approval` | info | a non-prod binding with four eyes off | no approval is needed there | none |
| <a id="safety-info-local-sleep"></a>`info.local.sleep` | info | a local host | a laptop or desktop host may sleep; health then reads unreachable, which is not an error | none |

