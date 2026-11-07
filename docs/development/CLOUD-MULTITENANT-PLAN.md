# Multi-tenant cloud service: plan

Status: idea recorded 2026-11-02 (owner's notes); step 1 built the same day (section 7). Decisions taken to start, owner to confirm: pod per Tenant-level configuration (shared agents, models, git, k3s per team and environment, egress, media): `TENANT-CONFIG-PLAN.md`, core built 2026-11-04 (F-TCFG-01 to F-TCFG-09).
tenant, accounts held in each tenant, the common vault written by an operator role, certificates supplied by the person, a file bundle
as the first image target. Read with `../CLOUD-MODE.md`, `MINIMAL-WEBSERVICE.md`,
`UI-OS-SPLIT-PLAN.md`, `VPN-CONNECTOR-PLAN.md`, `../K3S-DEV.md`, `../K8S-TILE.md`, `../FIREWALL.md`.

## 1. The request, as written

* A **multi-tenant cloud service**, deployed to a **multi-node cluster**, driven by configuration.
* **Vaults can be shared**: by tenant and namespace (common to all of them) **and** personal.
* A **namespace switch in the OS**: a person signs in **personal** or in **cloud mode**; in cloud mode they can be signed in to **several
  tenants at once** and switch between them.
* A **desktop connector** to a cloud instance (the local desktop works on a cloud tenant).
* A **"cloud mode Docker image" button**: generate the image from the OS.
* **Create it** on a local or a remote k3s, "or anywhere the user asks".
* **Listen and port-forward by domain**, for HTTP and WebSocket.

## 2. What exists and is reused

| Need | Exists | Gap |
|---|---|---|
| Cloud mode | `cloudmode.rs` hides hardware tiles; `iyakki serve --profile cloud` runs the limited web service (HTTPS, sign-in, allow-listed routes), `/healthz`, `/readyz` | one tenant per process; accounts are the machine's |
| Container | `dist/Dockerfile` (musl, `FROM scratch`, non-root), `dist/k3s-web.yaml` (namespace, PVC, probes, hardened pod) | no Helm chart, no per-tenant template, no multi-replica story |
| Cluster tools | K8S tile and `k8s*.rs`, `kubeprofiles.rs` (several clusters), the dev k3s | create-on-remote, node join |
| Vault | `vault.rs` (`LocalVault`, `Secrets`), `vaultpending.rs`, `secretreq.rs` (per account, unlocked by the sign-in password) | a shared scope; key sharing between people |
| Reach a remote machine | `sshtunnel*.rs`, WireGuard plan (`VPN-CONNECTOR-PLAN.md`), `iyakki-remote` (browser desktop), developer-mode pairing | a desktop *client* for a cloud tenant; domain routing |
| Egress and policy | `iyakki-proxy`, `FIREWALL.md` | ingress by domain |

## 3. Design

**3.1 Tenants and namespaces.** A **tenant** is an organisation; its data lives in its own Kubernetes **namespace** with its own volume,
accounts, vaults, audit log and quotas. The service pod serves one tenant (simple, strong isolation); a **control plane** (small, one per
cluster) creates and removes tenants from configuration. Alternative kept open: one process serving many tenants with the tenant id in every
store path; cheaper, weaker isolation, and every store would need auditing. Recommendation: pod per tenant first.

**3.2 Vaults.** Three scopes: **personal** (today's: the account's, unlocked by the sign-in password); **tenant** (all members of one
tenant); **common** (every tenant on the instance, read-only for tenants, written by the operator). Shared scopes use a vault key wrapped for
each member (public-key wrapping, so adding or removing a member changes only the wrap; removal rotates the key). Names: `personal:`,
`tenant:`, `common:` prefix; a flow or tool asks for a secret by name and the scope is part of the permission it shows.

**3.3 Namespace switch in the OS.** Sign-in gains a mode: **Personal** (this machine, as today) or **Cloud** (one or more tenants). In cloud
mode the shell keeps one session per tenant; the header gets a switcher (like the existing account menu); each tenant has its own tiles,
data, vault and audit, and nothing crosses without an explicit copy. The switch never mixes secrets: a tool call is bound to the tenant of
the session that started it.

**3.4 Two ways in (owner, 2026-11-02).** The cloud version of IYAKKI is deployed to the cluster, and a person uses it in either way:
(a) **web login**: open the tenant's address in any browser and sign in (exists today: `iyakki serve --web` streams the desktop, the cloud
profile serves the limited API); (b) **the desktop connects to it**: the local IYAKKI desktop opens a tenant of a cloud instance, as one
more session next to Personal. Build (a) first (it is what step 1 deploys), then (b) in two stages: the connector shows the tenant's
desktop inside the local shell (the pixel stream, which needs no new server work), then a native connector that shows the tenant's tiles
with the local look (the UI/OS split in `UI-OS-SPLIT-PLAN.md`). Authentication is the same for both: the tenant's accounts; the desktop
also remembers the tenant like a paired device (address, pinned certificate fingerprint, a key) so it can sign in without typing the
address again.

**3.5 "Create the Docker image" button.** In the cloud settings page: pick variant (cloud, with or without the web engine), tenant
configuration, registry or file; the OS runs the same build the release uses (`dist/Dockerfile`, musl binary) and produces an OCI image
and a manifest. It is a button over a documented command (`iyakki cloud image ...`), not a separate path. Third-party tools needed to
build (a container builder) are fetched on the person's request only, never shipped (standing rule).

**3.6 Create on local or remote k3s, or anywhere.** One command and button: `iyakki cloud deploy --target <profile>`; the target is a
kube profile (`kubeprofiles.rs`: local k3s, a remote cluster, any cluster the person has a kubeconfig for), an SSH host that runs the
container, or a plain file bundle (image plus manifests) to take elsewhere. The deploy is a set of manifests (namespace, PVC, config
secret, deployment, service, ingress) shown before they are applied.

**3.7 Domains, HTTP and WebSocket.** Ingress maps `tenant.example.org` (or a path) to the tenant's service; TLS per domain
(certificates supplied or issued); WebSocket upgrade passes through (the desktop stream and the API use it). For the local machine,
**listen and port-forward by domain**: a small in-house reverse proxy (permissive-licence rule: own code) that routes by `Host` header and
SNI to a local port, a pod or a tunnel, with the firewall opened only for what is routed. Port-forward to a pod reuses the developer-mode
`forward` scope pattern.

## 3a. Additions of 2026-11-03 (owner): the launch button, a vault of its own, roles

**Launch button.** "Launch cloud mode" in the K8S tile, on the selected cluster (the local dev k3s or a remote profile): tenant name and
image, the manifests shown first, applied on confirmation, the guards of the tile (production-looking clusters stay blocked until
changes are allowed). Local k3s: the one-file image is imported without a registry; a remote cluster pulls from a registry the person names.

**No vault given: make one.** When the launch has no remote vault (neither a shared nor a personal one is configured), the launch creates
a **vault service inside the same cluster**, in its own namespace (`iyakki-vault`): one pod, its own volume, a NetworkPolicy that lets only
the tenant pods in, and no ingress. It holds the common vault (`commonvault.rs`) and the tenant vaults; its keys are wrapped per member
(3.2), so the service stores ciphertext and the unwrap happens with the member's key. A separate cluster for the vault is the alternative
(stronger isolation, much heavier); the button offers "use this vault service" or "point to another one". Recommendation: in-cluster first.

**Roles, deny by default.** Two membership lists, checked on the server for every route and every vault read, never in the UI alone:
* **Tenant**: a person who is not a member of the tenant sees nothing of it (the sign-in says "no access", not what exists). Roles: owner
  (members, namespaces, billing-like settings), admin (members, namespaces), member.
* **Namespace** (inside a tenant): a member of the tenant who is not a member of a namespace cannot open it, list its content, or read its
  vault; at most the name is visible to the tenant's admins so they can add people. Roles per namespace: admin, editor, viewer.
* Being a tenant owner or admin does **not** open a namespace's content or vault (least privilege): they add themselves to it, which is
  logged. (Owner to confirm: or tenant owners may read everything.)
* Enforcement in depth: the vault key of a namespace is wrapped only for its members, so a removed or never-added person cannot read
  it even through a bug in a route; on Kubernetes a Role and RoleBinding per namespace carry the same lists for `kubectl` users.
* A roles manager page (Settings, cloud): members of the tenant, namespaces, who is in which, roles, add and remove (removal rotates keys
  and ends the person's sessions), and an audit trail of every change.

## 3b. Storage: volumes, folders and how they are secured (owner's question, 2026-11-03)

**Today (step 1):** one PersistentVolumeClaim `iyakki-data` per tenant, mounted at `/var/lib/iyakki`; one pod (`replicas: 1`, `Recreate`).
The claim outlives the pod, so a restart, a node move or a reconnect keeps everything on it. What is lost is only what is outside it:
`/tmp` is an `emptyDir` and the root filesystem is read-only, so **clones, files and work folders must live on the volume, never in `/tmp`**
(a check at start-up refuses a work folder outside the volume).

**Layout (recommended first):** one volume per tenant, folders by scope, each person's work under their own folder:
`ns/<namespace>/<person>/...` (clones, files of that person in that namespace), `ns/<namespace>/shared/...` (what the namespace's
members share), `personal/<person>/...` (the person's own, outside any namespace). A person's folders are created on first use.

**Keeping the data:** the storage class must be `Retain` (a `Delete` class removes the volume with the claim, and so with the
namespace); the manifest sets `persistentVolumeReclaimPolicy` guidance in its README and `iyakki cloud deploy` warns when the class
deletes. Backups: a VolumeSnapshot per schedule where the class supports it, else a file archive into the tenant's vault target.
Quota per tenant (`ResourceQuota`, exists) and per person (a size cap checked when a clone or a write grows a folder).

**How the folders are secured** (honest: folders on one volume are only as strong as what enforces them; three layers, cheapest first):
1. **Path jail in the service (v1).** Every file operation takes (tenant, namespace, person) from the signed-in session, never from the
   request, maps it to a root, and opens beneath that root with `openat2(RESOLVE_BENEATH)`: no `..`, no absolute path, no symlink that
   leaves the root, no hard-link tricks (clones are checked after checkout). The membership check of section 3a runs first. Tests: traversal,
   symlinks planted by a clone, a namespace the person is not in. Weakness: one process and one user id serve everyone, so a bug in the jail
   exposes everything.
2. **The kernel enforces (v2, hard isolation).** One volume per namespace and a worker pod per active namespace that mounts only that
   volume (Kubernetes decides who sees what; nothing else in the pod can reach another namespace). Costs a pod per active namespace and a
   router in front. Chosen per tenant in the configuration (`isolation: folders | volumes`); owner to decide the default.
3. **At rest:** encrypt the volume through the storage class (cluster side); the vaults keep their own per-member key wrapping (3.2).
   Encrypting each folder with member keys is possible but slows clones and builds a lot: not planned.

**Decision (owner, 2026-11-03): keep it simple, keep it extensible.** First version: folders on one volume per tenant with the path jail
(layer 1). The code must not assume that: every place that touches files asks one resolver, `StorageLayout::locate(tenant, namespace,
person, scope) -> Place { volume, subpath }`, and never builds a path itself. With `isolation: folders` all places share the tenant's
volume; with `isolation: volumes` (later) the namespace gets its own volume and its own worker pod, and nothing outside the resolver and the
manifest generator changes. The configuration field `isolation` is accepted from the start (only `folders` works; `volumes` is refused with
a clear message until it is built), so a tenant can be moved later by copying a namespace's folder onto a new volume.

## 3c. Routing by domain name, HTTP and HTTPS, ports (owner's question, 2026-11-03)

One table of **routes**, in the tenant configuration and edited in the same roles/cloud page: `host` (and optional path prefix) -> target
(a tenant's service, a pod and port, or a local port) with `scheme: http | https`, `websocket: true` by default (an Upgrade passes
through untouched), and who may reach it (the tenant or namespace membership of section 3a; public only when the route says so).
* **On a cluster:** the router is an Ingress with one rule per route (the manifest generator writes them); TLS from a Secret per host
  (supplied by the person; an issuer such as cert-manager is the person's own). A host that has no certificate is served as plain HTTP
  only if the route says `http`, else it is not created. Plain HTTP can redirect to HTTPS (`redirect: true`).
* **On the local machine** ("listen and port-forward by domain"): a small in-house reverse proxy listens on 80 and 443 (or a port
  the person picks), chooses the route by the `Host` header (HTTP) or by SNI (HTTPS, so the TLS stays end to end when the route says
  `passthrough`), and forwards to a local port, a pod (port-forward) or an SSH/WireGuard tunnel. The firewall opens only the listening ports
  that have a route, and closes them when the last route goes.
* **Ports other than 80/443:** a route may name any listening port; the table refuses a port another service already holds and
  refuses privileged ports unless the proxy has the right to bind them.
* Names: a wildcard (`*.tenant.example.org`) maps each namespace to its own host if wanted. DNS is the person's (or `/etc/hosts` for a
  local trial): IYAKKI shows the records to create and checks them.
**Custom certificates (owner, 2026-11-03).** Each HTTPS route (and the listener as a whole) takes the person's own certificate: a PEM chain
and its private key, or a PKCS#8 key; an optional client CA turns on mutual TLS for that route. The listener picks the certificate by SNI
(rustls `ResolvesServerCert`, already a dependency of `iyakki-remote`, permissive licence), so many hosts share one port with their own
certificate each. Rules: the key is stored in the vault (never as a plain file on the volume, never in a manifest; on a cluster it is a
`kubernetes.io/tls` Secret the person creates, which IYAKKI only references); on adding, the certificate is checked (chain order, key
matches, host is in the subject names, not expired, expiry shown and warned 30 days ahead, key strength and signature algorithm
acceptable); minimum TLS 1.2; replacing a certificate takes effect for new connections without a restart; a route with no valid
certificate is not served over HTTPS (the page says why). For a local trial a self-signed certificate can be made by IYAKKI on request
and is marked as such (the pinned-fingerprint pairing of the desktop connector already trusts one by fingerprint).

Built at step 5 of the order of work.

## 4. Order of work (each step ships and is tested on its own)

1. **Config and one tenant per pod**: `service.cloud.json` gets `tenant`, `members`, quotas; image and manifest from one command.
2. **Deploy command and button** for local k3s and a kube profile; manifests shown first; a QEMU or kind-less test using the fake
   `kubectl` already used by the K8S tests.
3. **Shared vaults** (tenant and common scopes) with key wrapping; tests for add and remove of a member.
4. **Sign-in modes and the switcher** (personal or cloud, several tenants at once).
5. **Domain routing**: reverse proxy by `Host`, TLS per domain, WebSocket.
6. **Desktop connector**: browser first, then native.
7. **Control plane** for many tenants on a multi-node cluster (node join, per-tenant quotas, upgrades).

## 5. Open questions for the owner

* Pod per tenant (recommended) or one process for all tenants?
* Who signs in to a tenant: accounts held by IYAKKI in each tenant, or an external identity provider (OIDC) later?
* Is the "common" vault written by an operator role, or by any tenant administrator?
* Certificates: supplied by the person, or issued automatically (needs a public domain and a third-party service)?
* Which registry or file target for the image button first?

## 6. Not decided and not tested

Everything above is a plan. No tenant isolation property has been verified; the multi-node behaviour (volumes that follow a pod, upgrades
without downtime) needs a real cluster to confirm.

## 7. Built so far

**Step 1 (2026-11-02), tested; not tried on a real multi-node cluster.**

* `crates/iyakki-agent/src/cloudtenant.rs`: `TenantConfig` (tenant, domain, tls_secret, image, profile `web` or `cloud`, storage, limits,
  members) with strict checks (nothing but plain names, quantities and host names reaches the YAML), `manifests()` (namespace, quota, data
  volume, deployment with one init container per member, service, network policy, ingress with the WebSocket timeouts when there is a
  domain) and the cloud profile's `service.json`, checked by the service's own checker. No password is in any manifest: the first
  passwords come from the Secret `iyakki-accounts`, whose `kubectl create secret` command is printed.
* `crates/iyakki-agent/src/cloudcli.rs`, command `iyakki cloud example | manifests | image | bundle | deploy`:
  `image` makes the container image **from the running program** with no builder (`ocipush::build_image`, saved by the new
  `ocipush::layout_tar` as an OCI layout; it warns when the program is not a static one); `--push HOST:PORT/REPO:TAG` sends it to a plain-HTTP
  registry; `bundle` writes config, manifests, image and a README; `deploy` shows what it would apply and applies with `kubectl` only with
  `--apply`.
* Checked here: unit tests (12); the bundle made from the release's static program loads into Docker, the image has the right
  architecture, user and entry point, and starts (`supervisor up`); `kubectl apply --dry-run=client` accepts all seven objects.
* Step 4, first part (2026-11-04): **tenant after login**. `GET /api/v1/tenants` lists the tenants the signed-in person belongs to (membership is the tenant's own `roles.json`; `scope=all` for service administrators only); `POST /api/v1/tenant/switch` checks membership on the server, binds that session to the tenant (the volume the next request works on), stops the person's running chats, calls the vault-lock hook and audits; the web UI top bar (`ui/tenant.js`) shows the tenant with a role badge and a drop-down when there are several; `iyakki-connect tenants` lists them. The directory of tenants a pod knows is `cloud/tenants.json` (hand-written; the control plane will write it); a tenant on another host is opened at its address and needs a new sign-in there. Rows F-WEB-41 to F-WEB-43. Tested on fakes and under node; not with a real cluster, several pods or a real browser.
* Not done: shared vaults (step 3), the rest of sign-in modes and the switcher (4: the desktop shell's Personal/Cloud sessions), domain routing for a local machine (5), the desktop connector (6),
  the control plane (7). Not tried: a real ingress controller, the cloud profile in a pod, volumes that follow a pod.

**2026-11-03 (steps 2 to 5 in part), tested with unit tests and a fake `kubectl`; nothing applied to a real cluster.**

* `cloudstorage.rs`: the resolver (`StorageLayout::locate`), `isolation` in the tenant configuration (`volumes` refused), private
  folders, the path jail and the check for links that leave a checkout. Not used by the service yet.
* `cloudroles.rs`: tenant and namespace roles, deny by default, the join rule, the last-owner and last-administrator rules; command
  `iyakki cloud roles`. Not enforced by any route yet; no roles page; no key rotation.
* `cloudroute.rs`: the route table, the pick, the Ingress, the certificate check and self-signed certificates; commands `iyakki cloud routes`
  and `cert`. No local listener.
* `cloudvault.rs`: the vault service's manifests and the vault address of a tenant. `iyakki vault serve` is not built, so a launch refuses it.
* `cloudlaunch.rs` and `apps/k8s_cloud.rs`: the plan, the preview, the blockers and the apply, and the **Launch cloud mode** button of the
  K8S tile on the selected cluster.
* Still to build: the vault server and key wrapping (step 3), sign-in modes and the switcher (4), the local domain listener (5), the desktop
  connector (6), the control plane (7), the roles page, using the resolver and the roles in the service, snapshots and backups (the file routes and snapshots are now built, see the last entry).

**2026-11-03, later (steps 3 to 7 in part), built by parallel agents and checked by unit and loopback tests; nothing applied to a real cluster, nothing seen on a screen.**

* **Vault service (step 3):** `cloudkeys.rs` (a member's secret gives an X25519 pair for wraps and an Ed25519 pair for signing; a scope key is
  wrapped with a fresh X25519 key, HKDF-SHA256 and XChaCha20-Poly1305 bound to scope, member and generation) and `cloudvaultd.rs`
  (`iyakki vault serve | keygen | enroll`): tenant, namespace and common scopes, sealing on the member's side so the service holds only
  ciphertext, wraps and public keys, requests signed (valid 120 s, TLS carries them), deny by default from `cloudroles`, rotation on removal.
  Decisions taken as defaults, owner to confirm: signed requests instead of tokens; clients do the cryptography; tenant-scope writes by
  admins; operators write `common` and have no tenant access. Open: no nonce against replay inside 120 s, no rate limit. New dependency:
  `x25519-dalek` (BSD-3, already in the lock file).
* **Vault side, 2026-11-03 (later still), loopback tests only:** (1) *automatic rotation on revocation*: `cloudrotate.rs` and `Access::revoke`
  (web routes for members with role `remove`, `iyakki cloud roles member|ns-member remove --vault-key FILE`) rotate the tenant scope and every
  namespace the person held, as the acting administrator; the role change is never lost: no key, an unreachable or misconfigured vault, or an
  administrator with no wrap in a namespace gives an entry in `cloud/rotation-pending.json` and a 'rotation pending' audit line, finished by
  `iyakki cloud reconcile --retry-rotations --tenant-data DIR --as USER --vault-key FILE`; a pod normally holds no member secret, so a removal
  from the web page is usually pending until an administrator retries. (2) *enrolment by a tenant admin*: `POST /v1/enroll` and
  `iyakki vault enroll-member`, only owner or admin, only for a person in their tenant's roles with a role not above theirs, never over a different
  key. (3) *the tenant pod reads the vault address*: `cloudvaultlink.rs` (`IYAKKI_VAULT_URL`, `IYAKKI_VAULT_FINGERPRINT`, `IYAKKI_TENANT`, the last
  set by the manifests); `GET /readyz` of the cloud profile adds `vault`: `not_configured | invalid_address | unreachable | ok`, never the address.
  Still open: ending the removed person's sessions, the pod reading or writing secrets for its own use, replay inside 120 s, rate limit.
* **Enforcement and control plane (steps 1 and 7):** `cloudaccess.rs` wires roles and the storage resolver into the cloud profile (deny by
  default, a member outside a namespace gets the answer of a missing one, `data_call` with a `namespace`, a small roles and namespace API,
  bootstrap of the first owner needing a service administrator, every change audited); `cloudcontrol.rs` and `iyakki cloud reconcile`
  (plan and apply, prune only when asked, a config hash on the namespace). Open: the cloud profile has no file or clone routes yet, so the
  checkout link check has no caller; not a controller inside the cluster.
* **Local domain listener (step 5):** `cloudlisten.rs`, `iyakki cloud listen`: HTTP/1.1 routing by Host and path, streaming bodies, Upgrade
  tunnels, HTTPS by SNI with the person's certificates (checked, replaced without a restart), passthrough, redirect, `PortGate`. Open: a
  route for `members` is served like a public one (a plain proxy cannot know who asks), no HTTP/2. Added 2026-11-03 (listener side, unit and
  loopback tests only, nothing on a real cluster, vault or kernel): vault-backed certificates with a reload timer and the last good
  certificate kept (`cloudlisten_vault.rs`); mutual-TLS routes with a client CA per route host (`cloudlisten_mtls.rs`); service targets
  through `kubectl port-forward` or the cluster DNS name (`cloudlisten_pod.rs`); the firewall behind `PortGate` (`iyakki_fw::gaterule`,
  `cloudlisten_gate.rs`) with a dry-run render. Still open: `iyakki cloud listen` does not choose the vault source, the client CAs or the
  gate yet (they are `Options` fields); the gate has one reach for all ports; SSH and WireGuard tunnel targets.
* **Roles page:** Settings -> Cloud access (`settings_cloudroles.rs`).
* **Desktop connector (step 6, stage 1):** `cloudclient.rs` (probe, pinned sign-in, the browser's pixel-stream protocol, reconnect), the
  decoder moved to `pixelcodec.rs` (so the shell can use it without depending on `iyakki-remote`; new dependency `tiny-skia`, BSD-3, already
  used by `iyakki-remote`), `cloudtenants.rs` (remembered tenants, no passwords), `cloudsession.rs` and `shell_tenant.rs` (sign-in mode
  "This machine | Cloud", trust on first use with the fingerprint shown, a changed fingerprint is a hard stop, a switcher with several tenants
  open, Ctrl+Shift+J to cycle, input only to the shown tenant, Personal shows nothing of a tenant). Open: the shell side was tested against a
  fake network and the client against a fake screen, never together (done since, see the last entry); no menu entry to open Cloud after
  sign-in and the strip along the bottom edge (both done since); the native tile connector (stage 2) is not built.
* **Still not built:** sign-in modes for the roles inside the tenant's own desktop, snapshots and backups,
  `isolation: volumes`, tenant-pod use of vault secrets, a firewall implementation of `PortGate`.

**2026-11-03, storage side (files, clones, snapshots), unit and loopback tests only; no real cluster, no real clone over the network.**

* **File and clone routes in the cloud profile** (`webservice/files.rs`, `cloudfiles.rs`): `GET /api/v1/files`, `GET /api/v1/files/content`,
  `POST /api/v1/files/write`, `POST /api/v1/files/delete`, `POST /api/v1/git/clone`, `GET /api/v1/git/clones`. The person is always the
  signed-in account (a `person` field is refused), the scope is `person` (`ns/<ns>/people/<person>`), `shared` (`ns/<ns>/shared`) or
  `personal` (`personal/<person>`, the default without a namespace). Rights: `ReadNamespace` / `WriteNamespace` from `cloudroles` through
  `Access::check` (viewers read, editors write; a person outside the namespace gets the answer of a missing one; a tenant with no role
  lists allows nothing); `personal` needs only a place in the tenant. Folders come from `cloudstorage::resolve` (locate, mount, ensure,
  jail). Tested: `..`, absolute paths, links (to a folder, to a file, to `/`), another namespace, another person, a stranger, a viewer, a
  member removed from the namespace or the tenant. Text files only (UTF-8, at most 512 KiB); a listing shows links as links.
  Clone: https only, no credentials in the address, no localhost, cluster names or IP literals; `git` with an empty environment, no hooks,
  `core.symlinks=false`, a 5 minute limit, then the check for links that leave the folder. Tested with a local repository through the file
  protocol on the unit level; **the route itself was never run against a real remote** (the tests cover its refusals).
  Open: no per-person size cap yet (a clone or a write can fill the volume up to the claim's size), no limit on clones at the same time, a
  host name that resolves to a private address is not caught (the namespace's network policy is the second line), a clone holds its request
  for up to five minutes, no binary files, no file move/rename route.
* **Snapshots and backups** (`cloudsnapshot.rs`, `iyakki cloud snapshot create | list | verify | restore | prune | cronjob`): a snapshot is
  `<data>/cloud/snapshots/<id>/` (outside the live `files` folder, refused if nested) with `data.tar.zst`, a manifest of every file with
  size, mode and SHA-256, the archive's checksum and a checksum of the manifest; links are not archived and are listed as skipped. Restore
  verifies, unpacks into staging, checks each file, and by default writes to a new folder (`<data>/cloud/restored/<id>`); into an existing
  folder (`--to DIR`, or `--live` for the live files) it refuses before writing anything if a file is already there, unless `--overwrite`.
  Destinations go through the path jail. `prune --keep N` keeps the newest good ones (an unreadable snapshot is kept for a person to look
  at). `cronjob` prints a Kubernetes CronJob (the same hardening as the service, placed on the node of the service because the claim is
  ReadWriteOnce, optional second claim for the snapshots with `--backup-claim`); tested by string checks only, **not applied to a cluster**.
  Open: roles lists and the audit file are not part of a snapshot (restoring those is a privilege decision), no VolumeSnapshot variant, no
  incremental snapshots, not encrypted at rest (the claim's storage class or the backup claim must be), the CronJob is not created by
  `iyakki cloud deploy` yet.
* **`isolation: volumes`** is still refused with "planned and not built yet". The extension point is `StorageLayout::mount` (where a place's
  volume is mounted; `root` for `folders`) and `cloudstorage::resolve`, which the file routes use; tests pin that a place on another volume
  is not served by this mount and that every entry point refuses `volumes` with the same message.

**2026-11-03, shell side of step 6 (menu entry, strip layout, routes form, real client with real shell); unit and loopback tests; nothing seen on a screen.**

* **Open Cloud after sign-in:** the IYAKKI menu (F10, or a click on the name; so keyboard and touch) has a **Cloud** entry (key `c`). It opens the
  Cloud dialog (`shell_tenant.rs`), which now lists the tenants signed in to first (dot, "switch to ..." to show one), then the remembered ones
  (sign in, Forget), with **Add a tenant** (the address, certificate trust and sign-in dialogs) and **Personal**. Ctrl+Shift+J still cycles.
  F1, F8 to F12 and the open menus are the shell's and never go to a shown tenant (F10 used to be swallowed by the tenant's picture).
* **Strip layout:** the strip is drawn only where it can cover nothing: on the sign-in screen (under the card) and while a tenant is shown (the
  picture's area stops above it and starts below the whole top bar; the launcher's bar is taller than an app's, which the picture used to cut).
  On Personal after sign-in there is no strip, so no launcher tile, page dot or app control is under it; the menu entry and the cycle key
  reach the tenants. Layout test at 1280x960, 360x740 and four more sizes, on the launcher and in an app.
* **Routes in Launch cloud mode (`apps/k8s_cloud.rs`):** a "Routes" page of the card: domain, service and port, http or https, and for https
  the certificate by name, a Secret made from the certificate files ("file") or a vault entry; each route is checked with `cloudroute`'s own
  `RouteTable::check` before it is added, and the table is the launch's `routes` (the `20-routes.yaml` Ingress of the preview and the
  apply). A pasted certificate or key is refused without being echoed; names only reach the preview, the lines and the audit trail (which
  records the number of routes). Up to 12 routes in the form. Open: `CertRef` has no file variant, so "file" means the Secret the person makes
  from the files (the command is the one `iyakki cloud` prints); no path/listen/access/redirect settings in the form (defaults: `/`, 80 or
  443, members, WebSocket on); vault-certificate and local routes are left out of the cluster Ingress by `cloudroute::ingress`, as before.
* **Real client with the real shell:** `crates/iyakki-remote/tests/cloudshell.rs` drives the real `Shell` (its default `RealConnector`) with the
  mouse and keyboard events of a person against real HTTPS servers on the loopback: trust on first use with the real fingerprint, sign-in,
  the refused password, the picture in the shell's own frame, a key reaching the tenant, a second tenant and switching, input only to the
  shown one, Personal showing no tenant pixel, closing, the menu entry after a machine sign-in, and a changed certificate on the same address
  (hard stop, no login request reaches the new server, the pin is kept). The screen behind each server is a solid-colour stand-in, as in
  `cloudclient.rs`; the tenant's own desktop is not the thing tested.


## Wiring of `iyakki cloud listen` (2026-11-04)

`iyakki cloud listen` now uses the finished listener parts, chosen by flags in `cloudlisten_setup.rs` (`iyakki cloud listen --help` prints
them). Each step is behind a flag; the defaults fail closed. Tested with fakes for the vault, the firewall and kubectl, and loopback sockets;
nothing has run on a real network, a real vault service, a real cluster or the real kernel firewall.

* **Certificates from the vault:** `--cert-source vault --vault URL --vault-user NAME --vault-key FILE [--vault-fingerprint F] [--tenant T]
  [--vault-scope S]`. Chain and key are read into memory on every reload (60 s); nothing is written. A vault that is down means the https
  hosts are not served (the last good certificate stays while it is valid); there is no fallback to files, and `--cert-dir` together with the
  vault source is an error. The default stays `--cert-source file`.
* **Mutual TLS:** `--client-ca HOST=NAME` per https route; the CA comes from the same source (a vault secret). A certificate of another
  tenant's CA, none, an expired one or an unknown CA is refused (403, or the handshake), and the target is not contacted. A CA the source
  cannot give turns every client away (503). A name that is not an https route is a configuration error.
* **Pods:** `--service-dialer port-forward|cluster-dns --namespace NS` hands `Options::dialer` the `cloudlisten_pod` dialers. Without one,
  Service routes answer 502 and the start-up says so.
* **Firewall gate (fail closed):** the default gate is the real `FirewallGate` (`iyakki_fw::gaterule::sync` through the engine and its netlink
  writer). `gaterule::sync` now also checks `gaterule::enforcing`: the engine must be `On` (not off, not "not protected", not unavailable, not
  waiting for Keep), otherwise opening a port is an error and the listener does not start (ports bound so far are released). If the gate cannot
  be had at all, `build` fails before any port is bound. `--no-firewall` is accepted only with a loopback `--bind`. `--reach local|anywhere`
  (default local network). `--firewall-start` starts this process's firewall engine from the saved settings (as the shell does at boot),
  for a standalone command; without it a standalone `iyakki cloud listen` has a firewall engine that is not booted, so it refuses to open
  (use it inside the IYAKKI session, or `--firewall-start`).
* **Tenants:** `--route-tenant HOST=TENANT` (repeatable) and `--tenant-limit N` (default 64). With tenants configured every route must have
  one (start and `update` refuse a table with a route that has none); the tenant of a connection is that of the route its SNI (passthrough) or
  Host (http, https) picks; at most N requests or tunnels of one tenant are open at once (503 or a closed passthrough connection beyond that),
  slots are released when the request or tunnel ends. A mutual-TLS refusal happens before a slot is taken.
* **Still open:** mTLS on the link from the listener to a pod (`ServiceDialer` returns a plain `TcpStream`; the port-forward is loopback and the
  cluster-DNS path is plain TCP inside the cluster network; a TLS-wrapping dialer is needed); "control plane mTLS" is only the per-route client CA
  (no mTLS between `iyakki cloud reconcile` and anything); tenant limits are one number of open requests, not bandwidth or rate; the tenant map
  is by route host from flags, not read from the tenant configs; one reach for all ports; `members` routes are still not enforced; SSH and
  WireGuard tunnel targets; the real vault `Reader`, `PortForward` with a real kubectl and the real gate are not exercised by any test.

**2026-11-04, vault hardening, pod rotation, encrypted snapshots, binary files, volumes; unit and loopback tests only, nothing on a real cluster.**

* **Replay and rates (`cloudvaultguard.rs`, `cloudvaultd.rs`):** a signed request now carries a random nonce (`IYAKKI-SIG id,ts,nonce,sig`; the nonce is
  inside the signature, a request without one is refused). The service refuses a second request with the same (id, nonce) for as long as it could pass the
  120 s clock window (either direction), keeps the pairs in `replay.log` (0600, read again at start, so a process restart forgets nothing; an unreadable
  log means "nothing signed before the start is accepted"), and refuses new requests with 429 rather than accept unchecked if the cache (50,000) is full of
  live entries. Token buckets per key (300 burst, 50/s; counted after the signature verified) and per source address (1000 burst, 200/s; counted first).
  Five failures from one address inside a minute lock that address for a minute: its failing requests then get 429. **The lockout cannot lock out the
  owner:** failures count against the address, never against the name a request claims; a request that verifies always gets through, even from a locked
  address, and clears the lock. Owner recovery if a stolen key floods the service: replace the key with `iyakki vault enroll` (file access is the
  authority) and restart the service (counters are in memory). The client waits a second and retries on 429, three times. Untested: a full 50,000-entry cache;
  a power cut that loses the last unsynced lines of `replay.log` (one replay could pass inside its remaining window).
* **Rotation from a pod:** a pod with an account of its own (`IYAKKI_VAULT_POD_ACCOUNT`, secret in `cloud/vault-keys/NAME.key`, enrolled and given a wrap by a
  tenant admin) finishes the rotation that follows a removal when nobody else holds a key (`cloudrotate::rotate`). The vault decides by role: a namespace
  editor (a tenant admin for the tenant scope) may rotate a scope, and may remove only a person the roles no longer entitle to it (the roles file is the
  authority); it may also rotate without removing anyone (`Client::rotate_key`). A pod with no account, no wrap or too small a role leaves the entry pending
  and `iyakki cloud reconcile --retry-rotations` stays the fallback. New route `GET /v1/scope/S/rotation-members`.
* **Snapshots are encrypted at rest when a key is given** (`--key-file F` or the tenant vault): XChaCha20-Poly1305 in 64 KiB chunks bound to position, end of
  stream, snapshot id and key id; the file list is sealed too. The key is a vault secret `snapshot-key.<id>` in the tenant scope (`snapshot-key.current`
  names the one in use); `iyakki cloud snapshot key-rotate` makes a new one, `rekey ID` re-seals a snapshot under it (atomic swap). A changed, cut, extended
  or reordered archive, or the wrong key, is refused before anything is unpacked. Honest limits: the plain tar and compressed archive exist briefly in the
  snapshot's temporary folder while it is made and during a restore (deleted, not wiped); whoever holds the tenant vault key reads the snapshots.
* **Binary files:** `PUT /api/v1/files/upload` and `GET /api/v1/files/download` (any bytes, 128 MiB at most, `Content-Length` required and checked before a byte
  is read, optional `X-Content-SHA256`, streamed in 64 KiB pieces through a private temporary file and a rename; the download opens without following a
  link and sends exactly the promised length). The listener hands these two routes to `files::serve_stream` after the head; same rights and same jail as the
  text routes. Not done: range requests, resumable uploads, a total-time limit for a slow upload (each read has the 10 s timeout), the per-person size cap.
* **`isolation: volumes`** works in the storage layer: each namespace is its own claim (`<volume>-ns-<ns>`, mounted at `<root>/volumes/<claim>`), `personal`
  stays on the main volume; `resolve_with` asks a `VolumeProvisioner` first and refuses when the claim cannot be created or the mount is not a separate file
  system (so files never land on the main volume by accident); `volume_claim_manifest` writes the claim. **Still refused where a tenant is configured**
  (`Isolation::deployable`): the service's manifest does not mount a claim per namespace and there is no worker pod per namespace; moving a namespace's folder
  onto a new volume is not built.
