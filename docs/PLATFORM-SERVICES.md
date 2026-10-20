# Platform services in k3s (design)

Status: **built and tested with fakes** (a fake `kubectl`/`podman`/`git`, fake HTTP, an in-memory vault, temp folders). It has **not been run
against a real cluster**: see "Not verified" at the end.

A platform service is something IYAKKI deploys *into a cluster the person picked*, to be used by the person, their workspaces and the assistant:
today **Gitea** (a git server) and, registered by the store side, the **private store**. The Registry (option A of the dev cluster setup,
`docs/K3S-DEV.md`) shows in the same list as installed or not, but keeps its own setup.

Nothing here is silent. Every change is a **plan** the person reads, then **approves**; nothing runs on a cluster the person did not pick.

## What the person sees

* **Settings → Dev cluster → Platform services** (button, key `P`): the list of services (Gitea, Private store, Registry) with the installed
  instances under each (live state, version), and the buttons *Deploy to k3s…*, *Connect to existing*, *Open*, *Details*, *Stop/Start*,
  *Upgrade*, *Backup*, *Connect workspace*, *Remove*. The same panel opens from the **STORE → Settings "Private store" card** (preselecting the
  store service) and from the **AI IDE's K8S panel** (*Services…*, and *Git* to connect the open workspace to Gitea).
* **The deploy wizard**: 1. choose the cluster (every cluster the person can see, with the badges: reachable, k3s, this machine or remote,
  in-cluster, production-looking, read-only; refused ones are listed with the reason and cannot be chosen); 2. the options (made from the
  service's schema: namespace, version, storage size and class, exposure ClusterIP / NodePort / LoadBalancer / Ingress, admin user,
  mirror images, and the service's own); 3. **review**: the exact commands, the images, the storage, what it exposes, the addresses, the
  credentials it creates (names only), warnings and blockers, the full manifest (selectable text), and an example NetworkPolicy that is *not*
  applied; 4. **approve**; 5. progress with live output; 6. the result: addresses and how to use it, and a masked one-time reveal of the
  generated admin password (Show, Copy, Done; Copy puts it in the clipboard history and Done takes it out again).
* While the panel is open it is the only owner of keys and clicks of its host.

## The engine (`crates/iyakki-agent/src/platform/`)

| Piece | What it does |
|---|---|
| `ServiceDef` (mod.rs) | One service: id, title, kind, pinned `versions()` (the last is the default; upgrades only go forward in the list), `images(version)` with architectures (and an optional digest), the options schema, `build()` (namespaced objects as block-style YAML, the Secrets to generate, the addresses), workloads, health path, `how_to_use`, backup hint and plan, `exposes`, `detect_existing`, `post_deploy`, `network_policy_example`. |
| `register` / `specs` | The registry of services. `register(Arc<dyn ServiceDef>)` adds one (the store does this at start). |
| `plan` / `preview` | Checks the options, probes the cluster read-only, builds and lints the manifest, returns a `Plan` with `blockers` (cannot be applied) and `warnings`. `preview` is the exact text the person approves. |
| `apply` | Needs the approval flag; re-checks the cluster; namespace, mirror, Secrets, manifest, wait, first-run setup, health, record. |
| `status` / `power` / `plan_upgrade` + `upgrade` / `backup` / `plan_remove` + `remove` | The lifecycle, each with its own preview and approval. |
| `installed.rs` | `installed-services-<user>.json`: the instance id, cluster reference, namespace, version, options, addresses, vault entry **names**, connected workspaces. Never a value. |
| `secrets.rs` | Generation, the vault, the 0600 temp file that carries a value to `kubectl create secret --from-file`. |
| `connect.rs` | HTTP through curl (token in a private config file), workspace connect and push. |
| `tools.rs` | The assistant's tools. |

### Choosing and guarding the cluster

* The clusters are the dev-cluster profiles (`devcluster::resolve_hiding`): hidden kubeconfigs are never probed or listed.
* `deploy_refusal` refuses: unreadable, unreachable, **production-looking** (`prod` in the name, context or server), and anything
  `devcluster::read_only_reason` says is read-only (a remote cluster that only came from `$KUBECONFIG` or `~/.kube/config` and was never chosen,
  an in-cluster service account that is cluster-admin). To use such a cluster the person makes it their choice first (Settings → Dev cluster,
  *My default*).
* Every write (`apply`, `power`, `upgrade`, `backup`, `remove`) inspects the profile **again** and refuses if it became unusable or the API
  server behind the kubeconfig changed since the service was deployed.
* **Namespaces** reuse the dev-cluster rules (`k8stools::protected_namespace`, the `app.kubernetes.io/managed-by=iyakki` label). Default is
  `iyakki-<service>`; a missing namespace is created as a **visible, approved step** and labelled `managed-by=iyakki` and
  `iyakki.dev/service=<id>`. An existing namespace must already carry `managed-by=iyakki` (the refusal prints the exact `kubectl label`
  command); one that belongs to another service is refused. A namespace is only ever deleted when IYAKKI created it for that service, it still
  carries the service label, and nothing else is in it.
* Objects carry `iyakki.dev/service` and `iyakki.dev/instance`; status, stop and remove select by the instance label only.

### Capacity precheck and images

Before the plan is shown the engine reads the nodes (Ready, architecture, pod capacity), the StorageClasses (default or `local-path`), the
Services (taken NodePorts) and the workloads. Blockers: no Ready node, pod limit reached, **no StorageClass** for the PVC, an unknown
class, a taken NodePort, a namespace that is not IYAKKI's. Warnings: NotReady nodes, few free pod slots, an image **not published for the
nodes' CPU architecture** (an amd64-only image on arm64), an existing service of the same kind.

Images are **pinned tags** pulled by the cluster from their public registry (Gitea: `docker.io/gitea/gitea:<version>-rootless`); the
`ImageRef` can carry a digest (the reference becomes `repo@sha256:...`). On an air-gapped or local cluster the option **Mirror images** pulls
them on this machine (podman or docker; the Docker Hub pull is stated in the plan), tags them and pushes them to the in-cluster registry
(option A); the pods then pull `localhost:<nodePort>/iyakki-platform/...`. It is a blocker if the registry is not set up.

Every container the built-in services run: non-root (`runAsNonRoot`, uid 1000 for Gitea), `allowPrivilegeEscalation: false`, all
capabilities dropped, a RuntimeDefault seccomp profile, requests and limits, readiness, liveness and startup probes, no service account token,
no privileged, no hostPath, no host networking. The same manifest linter as `k8s_apply` (`deploylint`) must pass; for the built-in services
with **no warnings**.

### A program the image is built from (`platform/localbin.rs`)

A service whose image is built on this machine (the private store) declares `ServiceDef::local_binary()` (the file name, the cargo package, the
folder). `apply` then looks for the program **before the namespace is touched**, for the architecture of the Ready nodes (`Plan::target_arch`),
in this order: shipped with the install (`/usr/share/iyakki/<sub>/<arch>/`, beside the window binary), the cache `<data>/<sub>/`, the source tree's
`target/<musl triple>/release/`. A file counts only if its **ELF headers** say static (no `PT_INTERP`, no `DT_NEEDED`) and the machine is the
cluster's; the running program is never a candidate, and nothing is ever run to find out. When nothing is found the wizard's result is a red
card (the reason, the list of what is missing, "Nothing further was done"), with these buttons: **Build now** (offered only when the source tree,
`cargo`, the musl target and a linker exist; it shows the exact command for approval, runs it as a detached job through the jobs engine with a
moving bar, and the deploy continues by itself when it succeeds, or shows the last lines when it fails), **Choose a binary file...** (a typed path,
checked by its headers and copied into the cache; a static file of the other CPU is called out as such), **Open docs**, **Try again**. Re-running
resumes: an existing namespace and Secret are reused. The plan shows a warning when the program is not at hand yet. The assistant's
`platform_deploy` gets the same text as its error. Details: docs/STORE.md, section 13.1.

### Secrets

A generated password or token exists in exactly three places: the Kubernetes Secret, the person's vault (`uvault:svc.<instance>.<key>`), and
the one-time reveal returned to the host that shows the masked dialog. The order is: check the vault is writable (a locked vault stops
everything before the cluster is touched) → generate → write the vault → create the Secret from a 0600 temp file in RAM
(`--from-file=key=path`; the command line holds a path, never a value) → on failure the vault entries are removed again. An existing Secret
is never regenerated. Values never appear in an argument, an environment variable of the engine's own calls, the manifest, a log, the audit
log (action, service, namespace and server only), the installed-services file, a tool result or the model's context. The Gitea admin user is
created inside the pod from the pod's own environment (`$GITEA_ADMIN_PASSWORD` comes from the Secret through `secretKeyRef`), so the command
line holds the variable name only. Tests plant a known value and scan every command, environment, audit line, file and output.

The exception to "values are never shown" is the reveal dialog, and *Details → Reveal password*, which reads the vault entry on an explicit
action. Connecting an existing Gitea stores the token the person typed in the vault only.

## Why Gitea

The need: a small, self-hosted git server for a single person or a small team, running in k3s on modest hardware (a Mac mini, an arm64 board),
with an HTTP API IYAKKI can drive (create a repository, open a pull request) and permissive licensing.

| Candidate | Licence | Weight | Verdict |
|---|---|---|---|
| **Gitea** | MIT | one Go binary, ~100-250 MB RAM with SQLite, official rootless image for amd64 and arm64 | **Chosen.** Light, complete REST API, Actions-compatible runners if wanted later, SQLite for zero extra services and Postgres when needed, a rootless image fits the non-root rule, the person already runs it (`meisync-gitea`) so nothing new to learn. |
| Forgejo | GPL-3.0-or-later since v9 (hard fork of Gitea) | the same | Technically close and API compatible, but **GPL**: the project's rule is permissive licences only for what it bundles or ships. IYAKKI *detects* an existing Forgejo and connects to it (the API is the same); it does not deploy one. |
| Gogs | MIT | lightest | Much less active, fewer features (no Actions), weaker API. |
| soft-serve | MIT | tiny | SSH and a TUI only: no web UI, no pull requests, no API IYAKKI could use for them. |
| GitLab CE | MIT core | very heavy (several GB RAM, Postgres, Redis, Gitaly) | Wrong size for one machine; slow to start; far more to secure and upgrade. |
| Plain git over SSH or `git http-backend` | GPL-2.0 (git) | smallest | No web, no pull requests, no API; more to hand-build than to adopt. |

### The Gitea deployment

One replica (`Recreate` strategy, because the PVC is `ReadWriteOnce`), SQLite on a PVC mounted twice with sub paths (`data` →
`/var/lib/gitea`, `config` → `/etc/gitea`), configuration through `GITEA__section__KEY` environment variables: `INSTALL_LOCK=true`,
`DISABLE_REGISTRATION=true`, `REQUIRE_SIGNIN_VIEW=true`, `DEFAULT_PRIVATE=private`, `ROOT_URL` from the chosen exposure, SSH on 2222 in the pod
(built-in server) and the advertised port from the exposure, LFS on, `webhook.ALLOWED_HOST_LIST=external` (webhooks may only call public hosts).
Two Services: `gitea-http` and `gitea-ssh`. **NodePort** (default) uses the first free ports from 30300 and 30222 (the free ports are found
in the cluster, an existing install keeps its own) and the clone URLs are shown; ClusterIP, LoadBalancer (needs a *Public host*) and Ingress
(needs a host; SSH stays a ClusterIP Service) are options.

After the pod is ready: create the administrator (`gitea admin user create`), create an **API token for IYAKKI** (`gitea admin user
generate-access-token`, scopes `write:repository,read:user`) and store it in the vault as `uvault:svc.<instance>.token`. The token is never
shown. Health: `GET /api/healthz` on the first address.

**Backup**: `gitea dump` inside the pod into the volume (`/var/lib/gitea/backups/gitea-dump-<time>.zip`), then optionally `kubectl cp` to
`<services dir>/platform-backups/<instance>/`. Each needs the person's approval. Copy dumps off the cluster too.

**Upgrade**: change the pinned image tag with `kubectl set image` and wait for the rollout; offered only along `versions()` (forward);
a backup first is the default (a failed backup stops the upgrade); IYAKKI does not roll back by itself because the data may already be
migrated: it says so and points at the backup.

**NetworkPolicy**: an example restricting ingress to the LAN is part of the plan text and is **not applied**.

**Postgres** is a later option (a second Deployment with its own PVC); SQLite is fine for one person and a few workspaces.

### An existing Gitea or Forgejo

`detect_existing` looks at every Deployment, StatefulSet and DaemonSet of the cluster (image names `gitea/gitea`, `forgejo/forgejo`, or a name
starting with `gitea` or `forgejo`) and offers it in the plan as a warning: "A Gitea is already running here: ns/name. Connect to it instead of
installing a second one", with its NodePort address as a hint. **Connect to existing** asks for the address and a token the person
creates in that service (a masked field), checks `/api/v1/version` and `/api/v1/user`, stores the token in the vault, and records the service
as *connected*: IYAKKI deploys, stops and removes nothing for it (Remove only forgets the record and the token).

## Connect a workspace to Gitea

IDE → K8S panel → *Git* (or Services → *Connect workspace*): 1. the review shows the repository name (from the workspace, lowercased), the
privacy (**private by default**), the origin change and warnings (plain http is called out); approve → IYAKKI asks the service who the token
belongs to, creates the repository through the API (an existing one is reused), and sets `origin` to the repository's HTTPS address (the
address never holds a credential; an `origin` that points somewhere else is not replaced unless the plan said so). 2. A **second approval card**
for the push (`git push --set-upstream origin <branch>`). The token is given to git through its environment for that one command
(`http.extraHeader` as basic authentication; inherited credential helpers are switched off), never in the address, an argument or a file; the
push is refused when `origin` is on another host than the service or holds a credential. The repository link is shown in the result.

For your own terminal pushes afterwards there is no persistent credential helper (IYAKKI's vault is not reachable from a git subprocess
outside IYAKKI): use SSH (your key added in Gitea) or `git credential approve` yourself. For parallel tasks (`tasks.rs`) pushing a task
branch and opening a pull request (`POST /api/v1/repos/{owner}/{repo}/pulls`) is **not wired**: it would be a third approval (Ask level) using
the same `connect::push` and the same token handling.

## The assistant

| Tool | Level |
|---|---|
| `platform_list`, `platform_status`, `git_service_status` | Safe |
| `platform_deploy {service, profile, namespace?, options?}` | Ask, and **hard-ask** (never run by Auto or Autopilot, never covered by an "always" grant) |
| `platform_upgrade {instance, version?, backup_first?}` | Ask, hard-ask |
| `platform_remove {instance, delete_data?, confirm}` | Ask, hard-ask; `confirm` is the instance id typed by the person |

The tools exist in a workspace chat that has the cluster tools. `profile` must be an id `platform_list` gave (written to
`platform-profiles-<user>.json`); an unknown, hidden, production-looking or read-only cluster is refused, so the assistant cannot invent or
pick a cluster. The approval card names the cluster and its **server**, and its preview is the same exact plan. Results carry the vault
entry names, never a value; the person reveals them in Details.

## Adding a service

1. Implement `platform::ServiceDef` (see `gitea.rs`). `build` returns namespaced objects without a `namespace:` key, without Secret values and
   without `hostPath`, privileged containers or added capabilities; use `labels` with `iyakki.dev/service` and `iyakki.dev/instance`
   (`BuildCtx::instance`); name Services `<x>-http` / `<x>-ssh` if you want their NodePorts kept across deployments. Declare generated
   values as `SecretSpec`/`SecretEntry` (vault name `format!("{}.<key>", ctx.vault_prefix)`) and read them in the pod through `secretKeyRef`.
   A service that builds its image from a local program also implements `local_binary()` and `prepare_images(pl, version, arch, progress)`;
   the finding, building and choosing come with it.
2. `platform::register(Arc::new(MyService))` at start. The list, the wizard, the tools and the registry file need nothing else.
3. Tests: plan text and manifest lint clean, options, refusals, apply order with a `FakeRunner`, no secret in any command (see
   `platform/tests.rs`).

## Not verified

* No real cluster, registry, Gitea, curl, podman or git push was used. The pinned tags (`1.23.8`, `1.24.5-rootless`) and the Gitea command
  line flags (`--scopes`, `--raw`, `--must-change-password=false`, `dump -f ... --type zip --tempdir`) are written from the documentation and must be
  checked on a real run; digests are not pinned yet.
* `kubectl` custom-columns output for several kinds in one `get`, the `kubectl cp` of a dump (needs `tar` in the image), and NodePort
  reachability on `localhost` depend on the cluster.
* Vault-held kubeconfig profiles are not offered as a target (they are not probed by the profile list).
