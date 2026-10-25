# Build and deploy to k3s from IYAKKI (design)

Status: phases 1 to 3 are built (window mode, an existing cluster, registries); phases 4 (bare metal) and 5 (cloud mode) are **built and tested with fakes, not yet run on the real bare-metal machine or in a real pod**: see "What is built (phases 4 and 5)" at the end for what exists and what must be checked. Decisions so far: Podman is the container tool; the assistant may decide and draft
manifests but every write needs the person's approval; three modes (window, bare metal, cloud) with different
default clusters.

## What it does
"Build this folder and deploy it" runs the familiar four steps, each one visible, approved and audited:

1. **Build** the image with `podman build`.
2. **Get the image to the cluster**: import into the local k3s, or push to a registry for remote and cloud clusters.
3. **Apply** the manifest (drafted by the person or by the assistant, linted, shown as a diff, approved).
4. **Verify**: `rollout status`, pods, logs, with plain-English reasons for ImagePullBackOff, CrashLoopBackOff and so on.

**Quick install.** Store → Setup → k3s (or the IYAKKI menu, F10 → Quick install, which opens it) runs the same managed install (lookup, vendor checksum,
inside the data partition, refused in a desktop window or beside another cluster) as a detached job, then the shell's supervisor
starts it. DuckDB, Elasticsearch (one single-node Deployment in namespace `iyakki-data`) and Indic-TTS install k3s first when it is
missing. See `docs/USER-GUIDE.md`, The IYAKKI menu.

## Modes

| Mode | Where IYAKKI runs | Default cluster | Other clusters |
|---|---|---|---|
| **Window** | desktop window on a development machine | the machine's own k3s (`/etc/rancher/k3s/k3s.yaml` or `~/.kube/config`) | cloud or remote k3s through a tunnel (SSH port-forward to the API server, or the provider's CLI such as `aws eks update-kubeconfig`, `gcloud container clusters get-credentials`) |
| **Bare metal** | the IYAKKI image on a PC | optional k3s cluster installed by IYAKKI, started automatically | cloud or remote k3s (same profiles as window mode) |
| **Cloud** | the IYAKKI web/Docker/K3s deployment (`dist/k3s-web.yaml`) | none locally | cloud k3s or remote k3s through the pod's service account or a vault-held kubeconfig |

### Bare metal: installing and starting k3s
- Offered in Settings → Dev cluster as "Set up a local cluster". An administrator approves it; it shows what it will download and where it stores data.
- Two ways to run it, chosen at setup:
  1. **Native k3s** (what the sandbox cluster does today, `crates/iyakki-agent/src/cluster.rs`): the k3s binary started directly, inside the RAM-root jail on the image.
  2. **k3s in Podman**: the official k3s image run as a privileged rootful Podman container with a data volume on the data partition. This needs the Podman packages in the image (conmon, crun, netavark, overlay storage). It is an option because it keeps the cluster and its images in one container that is easy to stop, reset and upgrade, and it matches the build tool.
- Either way the cluster starts at boot when enabled (supervisor start hook), restarts if it dies, and shows its health in SYSTEM.

## Components

1. **Cluster profiles** (per account, Settings → Dev cluster): name, kind (`local`, `tunnel`, `remote`, `cloud-cli`), kubeconfig (a path, or a vault entry `uvault:<name>`), namespace, and registry address. A "Test connection" button runs `kubectl version` and `kubectl auth can-i`.
2. **Executor** (new module in `iyakki-agent`): the only place that starts programs: `podman`, `kubectl`, `k3s` (for `ctr` import on a local node), `ssh` (tunnels), and the cloud CLIs through the existing CLI catalog.
   - No shell; arguments are an array; output is streamed to the run log and capped.
   - Each program gets a row in `BOUNDARY.md`.
3. **Tunnel manager**: starts `ssh -N -L <local>:127.0.0.1:6443 user@bastion` (key from the vault), rewrites the kubeconfig server address to the local end, watches the tunnel, restarts it, and closes it when unused. The tunnel and every kubectl call go through the same approvals.
4. **Manifest workspace**: drafts live in the project folder (`deploy/*.yaml`) inside the account's file scope. The assistant may create and edit them (existing file tools and approvals).
5. **Surfaces**: a Deploy page (steps and live status), agent tools (`deploy_build`, `deploy_load`, `deploy_apply`, `deploy_status`, `deploy_logs`, `deploy_rollback`) and chat ("build and deploy this").

## Getting the image to the cluster
- **Local k3s**: `podman save <image> | k3s ctr images import -` (streamed, no temp file). It needs root.
  - Preferred: a one-time setup of a local registry (`localhost:5000`) plus a k3s `registries.yaml` mirror, after which step 2 is `podman push` with no root.
  - Alternative: a root helper started once by the administrator (a systemd unit or a sudoers rule for exactly that command); IYAKKI only talks to its socket.
- **Remote and cloud clusters**: there is no shared containerd, so a registry is required: `podman push` to the profile's registry, credentials from the vault, never in arguments.
- **Image pull policy**: for imported images the manifest must use `imagePullPolicy: IfNotPresent` or `Never`. The linter sets it automatically and explains why.
- **Tags**: default to a content hash, not `latest`, so each build rolls out.

## Who decides and who approves
- The assistant **may decide**: which Dockerfile, the image name and tag, the manifest layout (Deployment, Service, ConfigMap), ports and resource limits, and the order of steps.
- The person **approves every write**: build, push or import, apply, delete, rollback, tunnel start and cluster setup. Approvals show the real command and, for apply, a diff against the live object (`kubectl diff`).
- "Approve all for this deploy session" is available per profile and namespace, with the same expiry rules as the browser consent model; destructive actions (delete namespace, scale to zero in production profiles) always ask.
- Reads (status, logs, describe) need no approval.

## Safety
- **Namespaced identity**: the generated kubeconfig for the assistant uses a ServiceAccount with a Role limited to one namespace. It never uses cluster-admin.
- **Manifest linter** (before any apply): reject privileged containers, `hostPath`, `hostNetwork`, `hostPID`, `hostIPC`, added capabilities, cluster-scoped kinds (Namespace, ClusterRole, CRDs) unless the person unlocks them for that run, and images from unknown registries; require resource limits and a non-root user (warn).
- **Production-looking profiles** (names or labels matching `prod`) are read-only for the assistant.
- **Secrets**: Kubernetes Secrets are created from vault entries by the executor; values never pass through the model, the tool log or the audit log.
- **Audit**: every executor call is logged with program, subcommand, profile and namespace.

## Phases
1. **Phase 1 (window mode, local k3s)**: executor, profiles, connection test, the Deploy page running build → load → apply → verify against the local cluster. Tests use a fake executor.
2. **Phase 2**: agent tools and chat flow, manifest linter, namespaced kubeconfig generator, approvals and diffs.
3. **Phase 3**: tunnels and cloud CLIs, registry push for remote clusters, rollback and port-forward links that open in the browser tile.
4. **Phase 4 (bare metal)**: "Set up a local cluster" (native and Podman variants), auto-start, health in SYSTEM, and a Podman package in the web/default image variants (size and RAM to be measured first). **Partial**: Settings → Dev cluster, the native variant, the Podman variant (code and tests only; the image has no podman), auto-start, health and alerts are built; the Podman packages are listed in an optional fragment that is not wired in or built.
5. **Phase 5 (cloud mode)**: in-cluster service account profile and vault-held kubeconfigs for the web deployment. **Partial**: both built and tested with fake files and a fake runner; never run in a real pod.

## Open points
- Rootless Podman or rootful (rootless is safer but cannot run k3s; k3s needs rootful or a privileged container).
- Which cloud CLIs to ship in the catalog first (aws, gcloud, az).
- Whether the bare-metal local cluster should be the existing sandbox cluster or a separate one (current sandbox jobs and a dev cluster should not share a namespace).

## What is built (phase 1, window mode, an existing cluster)

* **Executor**: `wsexec::Runner` (real: `System`, tests: `FakeRunner`). Every `kubectl`, `podman`, `docker`, `k3s ctr` and `curl`
  call goes through it with an argument array, `KUBECONFIG` in the environment, a timeout and an output cap (`BOUNDARY.md`).
* **Cluster discovery and profile** (`devcluster.rs`): candidates in order (project.json, the person's earlier choice,
  `$KUBECONFIG`, `~/.kube/config`, `/etc/rancher/k3s/k3s.yaml`, k3s's data folder), `kubectl config current-context` and
  `kubectl version` to test each, a `ClusterProfile` (name, kubeconfig, context, server, reachable, k3s, local, version, what
  `auth can-i` says). Unreadable kubeconfigs are explained with the one-time fix, never `sudo`. The choice is remembered per workspace
  (`project.json`) and per user (`devcluster-<user>.json`). The K8S panel shows the chip and a **Cluster…** chooser.
* **Never start another cluster** (`devcluster::guard`, the `Tools::refusal` policy hook): `k3s server|agent`, the k3s kill and
  uninstall scripts, `systemctl start|stop|restart k3s`, `kubeadm`, `minikube`, `kind`, `k3d`, `microk8s`, cluster node containers and
  the k3s installer are refused, with the reason as the tool result and no approval prompt, in `run_command`, `terminal`, the IDE
  terminal and the sandbox cluster's `start()`.
* **Assistant tools** (`k8stools.rs`): `k8s_status`, `k8s_get`, `k8s_describe`, `k8s_logs`, `k8s_events`, `k8s_apply`, `k8s_delete`,
  `k8s_rollout_restart`, `deploy_build`, `deploy_load`, `deploy_status`; writes are confined to the workspace namespace (default
  `<workspace>-dev`, created on the first approved apply), ask first and are audited as `k8s.*`; a production-looking cluster is
  read-only; `deploy_load` prefers a registry on `localhost:5000` when k3s's `registries.yaml` mirrors it and otherwise imports with
  `k3s ctr images import`, explaining the two fixes when that needs root.
* **Manifest linter** (`deploylint.rs`): the rules under "Safety" for block-style YAML, plus the pull-policy rewrite for local images.
* **Deploy flow** in the K8S panel (`ide_deploy.rs`): form, four approved steps with the real commands and live output, "approve all
  steps", and Restart.
* Tests use a fake executor; one ignored live test reads this machine's real k3s and one ignored test applies a ConfigMap into a
  throwaway `iyakki-test-<random>` namespace that it deletes (approvals bypassed by the harness).

### What remains

* **Phase 2**: a namespaced ServiceAccount kubeconfig for the assistant (today it uses the person's own kubeconfig and confines
  writes in the tools), Secrets from the vault, rollback, port-forward links in the browser tile, YAML beyond block style in the linter.
* **Phase 3**: tunnels (SSH port-forward) and cloud CLIs, registry push for remote clusters, credentials from the vault.
* **Phase 4**: built (see "What is built (phases 4 and 5)"); what remains is running it on the real box, Podman in the image (packages
  listed, not built) and a measured size/RAM budget. **Phase 5**: built; what remains is a run in a real pod and with a real cloud kubeconfig.
* Window mode with several workspaces keeps one "connected cluster" for the guard (the latest resolution wins).

## Local image registry in one click (Quick install, private-store deploy)

`localreg.rs` creates registry option A (above) without a form: `check_dependency` (cluster answers, a node is Ready, a StorageClass exists), `create` (namespace `iyakki-registry`, the manifest of `regsetup::in_cluster_manifest_sized`, rollout, then `GET /v2/` from this machine), `inspect`/`status_line` (`registry ready · localhost:30500 · used by N images`), `remove` (typed namespace name). It reuses the NodePort design, so pods and the node pull `localhost:<nodePort>`; no `registries.yaml` is written and k3s is **not** restarted. `K3sKubectl` runs `k3s kubectl` for the managed cluster when the machine has no `kubectl`. It runs as the job `iyakki quick-install registry [--kubeconfig F --context C --node-host H --size GI --port P]`; the managed cluster's entry is Quick install → Cluster → Local image registry (depends on k3s). `ocipush.rs` pushes a one-file image to it with no docker or podman. Never run on a real cluster; nothing authenticates to the registry.

## What is built (phase 3 part: registry push for remote clusters)

* `deploy_load` on a cluster that is not on this machine pushes the image to the cluster's registry: `docker|podman tag <image> <registry>/<namespace>/<name>:<tag>`, then `push` (podman gets `--tls-verify=false`; the registry is plain HTTP).
* The registry is the `k8s.registry` setting (`host:port`) in `.iyakki/project.json`, else the first Service named like `*registry*` of type NodePort in any namespace, at `<API server host>:<nodePort>`. Either way `GET /v2/` must answer before anything is pushed. No address is hardcoded.
* **NodePort registries are pulled as `localhost:<nodePort>`.** The nodes' containerd cannot pull `<apiserver-host>:<nodePort>` (plain HTTP, "server gave HTTP response to HTTPS client") and IYAKKI has no root on nodes, but containerd allows plain HTTP for `localhost` and a NodePort is reachable on every node at localhost. So for a discovered NodePort registry `deploy_load` pushes via `<apiserver-host>:<nodePort>` and returns `localhost:<nodePort>/<namespace>/<name>:<tag>` as the image the manifest must use (the linter accepts it and keeps `IfNotPresent`). Caveat: it needs kube-proxy to expose NodePorts on localhost (default iptables/ipvs mode) and only works on Ready nodes. A registry set in `k8s.registry` keeps its own address, and the `registries.yaml` mirror stays the alternative. The pull diagnosis suggests `kubectl set image ... localhost:<port>/...` before the mirror fix.
* The result names the new full image reference; the manifest must use it. `k8s_apply` treats it as loaded (no unknown-registry warning, `imagePullPolicy: IfNotPresent`).
* NotReady nodes (`kubectl get nodes`) are named in the result as a scheduling hint; the cluster is never touched.
* Docker refusing a plain-HTTP registry gets the exact one-line fix (`insecure-registries` in `/etc/docker/daemon.json`, restart docker); IYAKKI never edits `/etc`.
* Tests use `FakeRunner`; the ignored live test only does `GET /v2/` against the discovered registry and never pushes.

## What is built (registry pull diagnosis and the registry option of the setup)

* `deploy_status`, `k8s_describe` and `k8s_events` append a **Diagnosis** when the text holds a registry pull error: "server gave HTTP response to HTTPS client" (plain-HTTP registry: ready-to-copy `/etc/rancher/k3s/registries.yaml` with the `http://` mirror, the `sudo tee` command, `sudo systemctl restart k3s` on the server and `k3s-agent` on EACH agent node, and the alternative of a registry hostname the nodes already trust), `x509` (untrusted certificate: `ca_file` or `insecure_skip_verify`), "no basic auth credentials" (a login in `registries.yaml` or an `imagePullSecret`), "connection refused" (reachability from the node). The registry address is taken from the error text, else from `k8s.registry`. IYAKKI never edits /etc or logs in to nodes, and never suggests Docker Hub.
* `registry_setup_help` (read-only) returns that fix for the project's registry (or `registry` host:port).
* `regsetup.rs` holds the setup plan's `RegistryOption` and `plan`/`preview`/`apply` through the `Runner`. `apply` needs the approval flag, never uses sudo, never overwrites an existing `registries.yaml` (the entry to add is shown), and records the choice in `.iyakki/project.json`: `k8s.registry` (where THIS machine pushes), `k8s.registry_pull` (what the pods pull, when different) and `k8s.registry_kind` (`none`, `in-cluster`, `system`, `remote`).

## What is built (choosing how images reach a local or remote k3s)

The K8S panel has a **Registry** chip (none / `in-cluster at localhost:30500` / system / remote; red when `GET /v2/` from this machine does not answer), a **Registry…** button (key `g`) and the **Set up registry** dialog; the assistant has `registry_setup` (Risk: ask) next to the read-only `cluster_setup_plan`. The dialog lists the options with their one line and what asks for root; after Save the panel shows one approval step with the exact plan (objects, files, commands, the root line) and nothing runs before **Approve**. Off in a read-only workspace and for a production-looking cluster, like Deploy.

| Option | What | Root | Best for |
|---|---|---|---|
| **A  Registry pod inside the cluster** (recommended) | namespace `iyakki-registry`, a `registry:2` Deployment (replicas 1, Recreate), a PVC (10Gi, the default StorageClass or `local-path`; `emptyDir` with a note when the cluster has neither), a NodePort Service (30500, or the next free port; re-running keeps our own port). Applied with kubectl through the Runner. | **none**, and no node configuration | a local or a remote k3s |
| **B  System registry** | `registry:2` container with podman/docker on `127.0.0.1:5000` (volume `iyakki-registry`, `--restart=always`); the node pulls `localhost:5000`. The one Docker Hub pull of `registry:2` is said plainly. | only to write the node's `/etc/rancher/k3s/registries.yaml` mirror (the `sudo tee` and restart commands are given; IYAKKI runs no sudo) | one local node (refused for a cluster on another machine) |
| **C  Existing registry** | host:port, http or https, optional insecure; recorded as given | only for plain-HTTP or untrusted-TLS registries (mirror commands given); `localhost:<port>` or trusted HTTPS needs none | a registry you already run |
| none | clears the saved choice | none | imports with `k3s ctr images import` (root) |

* **A needs no node setup** because pods pull `localhost:<nodePort>/<namespace>/<name>:<tag>`: containerd allows plain HTTP for localhost and a NodePort is reachable on every Ready node at localhost (verified live earlier). Pushes go to `<API server host>:<nodePort>` for a remote cluster and `localhost:<nodePort>` for one on this machine. Caveat as before: kube-proxy must expose NodePorts on localhost (default) and the node must be Ready.
* **The Namespace is cluster-scoped**, which the manifest check refuses in a workspace manifest, so it is created by its own `kubectl create namespace iyakki-registry` that is part of the plan and covered by the person's approval of that step; the Deployment, PVC and Service are namespaced, pass the same linter, and are applied with `-n iyakki-registry` (the file is kept in `.iyakki/deploy/iyakki-registry.yaml`). After the apply the rollout is awaited and `GET /v2/` is tried.
* **`deploy_load` honours the saved choice for every option.** A saved registry other than `localhost:5000` is used even for a cluster on this machine (before, a local cluster ignored it): the image is tagged `<push>/<namespace>/<name>:<tag>`, pushed there, and the manifest gets `k8s.registry_pull` (`localhost:30500/...`) when it differs. B keeps the existing `localhost:5000` path with the mirror check. The approval line for `deploy_load` says which address is pushed to and which the nodes pull.
* **Choice handling**: `k8s.registry_kind` in project.json is the single source of truth; the dialog preselects it, and Save writes the choice (A, B or C) at once, before any approval (only Save and the `registry_setup` apply write it; detection, probes and workspace switches only read). B is refused at Save for a cluster on another machine (its nodes cannot reach this machine's localhost:5000). Save on A records "A chosen, not set up yet" (chip `registry: in-cluster (not set up yet)`, any older registry is dropped) and leads to the approval step; saving other settings or choosing another cluster re-reads the registry fields from project.json first so they are never overwritten by a stale copy. A saved system registry (`localhost:5000`) on a REMOTE cluster is ignored by `deploy_load` (the nodes cannot reach this machine): it says so and uses the discovered NodePort registry, or points at Registry… A; `k8s_apply` refuses a bare `localhost/<image>` on a remote cluster.
* The deploy prompt gained one sentence: when `deploy_load` says there is no registry, offer `registry_setup`.
* Tests: plans and applies for every option with `FakeRunner` and temp dirs, the no-root path, validation (ports, addresses, B on a remote cluster, production), the IDE dialog, approval, cancel, read-only lock, chip and geometry. The ignored `live_in_cluster_plan_reads_the_cluster_and_applies_nothing` only runs `kubectl get` against the current cluster and never applies.
* **Not verified**: a real apply of A (the namespace, PVC binding on local-path, the NodePort answering from the node), a real B container start, and a k3s restart; the tests use a fake `kubectl`.
* The IDE K8S panel still does not show the pull-failure fix in a copyable block; the bare-metal "Set up a local cluster" (phase 4) will reuse these options.

## deploy_build pre-flight

`dockerlint.rs` reads the Dockerfile before any build: every COPY/ADD source (globs, `.dockerignore` respected, `--from` stages, URLs and variables skipped) must exist in the build folder, else `deploy_build` returns "COPY package*.json matches no file in <dir>. Files in that folder: Dockerfile. Found these likely app files in the parent: index.js. Fix: move the Dockerfile next to the app files (or pass dir=...), and create package.json." without running docker/podman. A `CMD`/`ENTRYPOINT` of `node`/`python` naming a file that is not in the context is a warning on the build result (app.js vs index.js). A failed build is reported as the first failing step and its message, without docker's legacy-builder deprecation banner (mentioned once as a note). The deploy prompt tells the model to keep the Dockerfile, app files and package.json in one folder and pass it as `dir`.

## Which cluster a workspace uses (no leaks from $KUBECONFIG)

* **Order**: the workspace's `project.json` kubeconfig, then the person's remembered choice, then **this machine's own k3s** (`/etc/rancher/k3s/k3s.yaml`, when readable and answering), then `$KUBECONFIG`, then `~/.kube/config`. A default is only a default: when nothing is chosen and more than one cluster answers, the chooser opens once (chips: server, `[remote]`, recommended local k3s) and the choice is saved per workspace. An explicit choice ends the search (other clusters are not probed).
* **No inherited environment**: runner children no longer inherit the process's `KUBECONFIG`; `run_command kubectl|helm` has `--kubeconfig/--context` pinned to the workspace's cluster (any `--kubeconfig`, `--context`, `--server` the model passes is dropped); the IDE terminal does the same and prints `kubectl → <server> (workspace X)`; `exec::run_with_env` drops inherited `KUBE*` variables when it sets `KUBECONFIG`.
* **Remote is visible and cautious**: the K8S header shows the selected server and an amber `remote` badge when it is not on this machine; a remote cluster that only came from `$KUBECONFIG` or `~/.kube/config` is read-only for the assistant until chosen for the workspace.
* **Hide / Disconnect**: Cluster… lists "Hide N." for every cluster that is not selected (stored per user in `devcluster-hidden-<user>.json`; the kubeconfig file is never touched; hidden clusters are skipped before anything runs, so no call reaches them and no tool sees them), "Show hidden clusters (n)", and "Disconnect this workspace" (clears `k8s.kubeconfig`/`profile` and the per-user choice; falls back to this machine's k3s).

## Tear down the dev environment, and the namespace

* **Namespace**: IYAKKI creates `<workspace>-dev` on the first approved apply and labels it `app.kubernetes.io/managed-by=iyakki` and `iyakki.dev/workspace=<name>`. A `kind: Namespace` document for it in a manifest is skipped (the rest is applied; the result says "skipped Namespace x-dev: it is created automatically"), any other cluster-scoped kind is refused with the way out (remove it, or ask the person to apply it by hand), and a plan row "Create namespace x-dev" is marked done after a successful apply.
* **Stop / Start** (`k8s_stop_env`, K8S panel → Tear down… → Stop): scales every Deployment and StatefulSet to 0 after saving the count in the annotation `iyakki.dev/replicas`; Start scales back (1 when nothing was saved). Services, ConfigMaps, Secrets and PVC data stay. Asks (approval card lists each workload).
* **Remove all** (`k8s_teardown`): deletes the namespace and everything in it. The card lists deployments, services, ingresses, pods, PVCs with their size (and says the data is deleted), configmaps and secret names (never values). It ALWAYS asks (hard ask: also in Auto and Autopilot, never covered by a grant or "always"); the panel's dialog requires typing the namespace name, the tool requires `confirm` = the name. Afterwards it runs `kubectl wait --for=delete` and, if the namespace stays Terminating, reports the conditions, finalizers and leftovers and what to check (IYAKKI never removes finalizers).
* **Images** (`k8s_registry_cleanup`, optional, asks): lists the repositories under `<namespace>/` in the registry IYAKKI uses and deletes their manifests; a registry with deletion disabled gets a plain "set REGISTRY_STORAGE_DELETE_ENABLED=true" (the in-cluster registry has it on; IYAKKI changes nothing); disk space is only freed by the registry's garbage collection.
* **Guards**: only the labelled namespace of this workspace; `default`, `kube-*`, `iyakki-registry` and unlabelled namespaces are refused with the manual command; never on a production-looking cluster or a remote cluster that only came from `$KUBECONFIG`. `k8s_delete kind=all` still fails, and its message points at `k8s_teardown`. Audit rows: `k8s.stop`, `k8s.start`, `k8s.teardown`, `k8s.registry_cleanup` (plus the usual `k8s.<tool>`).
* Autopilot ends a successful deploy by asking "Tear down the dev environment?" (Keep it running / Stop it / Remove all).

## The `k3s-dev-best-practices` skill

IYAKKI ships one skill for this flow, `dist/skills/k3s-dev-best-practices.md` (compiled into the shell, `samples::BUILTIN_SKILLS`). An administrator's first sign-in
after an update adds it once to the shared skills (SKILLS tile); a marker file means one that was deleted does not come back. It matches requests that mention k3s,
Kubernetes, deploy, Dockerfile, manifest, namespace, registry, kubectl or rollout, and it tells the model the safe order: look first, one folder for
Dockerfile and app, a small pinned non-root image, probes and limits, `deploy_build` then `deploy_load` and `k8s_apply`, verify with `deploy_status` and a real
request, diagnose one thing at a time, then offer Stop or Tear down. Two shorter skills sit beside it: `k3s-build-and-deploy` (the happy path for "build and deploy it": cluster named first, never switch the active cluster, a production-looking cluster is read-only, the failure table and the definition of done) and `k3s-dockerfile-patterns` (a Dockerfile per language); both are added the same way, once each. Edit the file and rebuild to change the shipped text; change the copy in SKILLS to adapt it
for one installation.

## Test containers (unit before pushing, integration after the rollout)

* **Unit-test image**: with a multi-stage Dockerfile that has a `test` stage (tests and coverage run in `RUN`), `deploy_build` with `target=test` (optional `build_args` object) builds `<image>-test:<tag>` and reports `TEST BUILD (not for deploy)` with the pass/fail counts and the coverage percentage read from the output (npm/jest `Tests:` and `All files |`, pytest `N passed` and `TOTAL ... 91%`, go test `ok`/`--- FAIL` and `coverage: 78.2% of statements`, cargo `test result:`). A failing test stage returns the failing test output as the error (the legacy-builder banner is dropped). The result says `deploy_load` is NOT for a test image, and `deploy_load` refuses one. The Dockerfile pre-flight reads only the stages up to the target, checks the target is a stage and that a `COPY tests` folder exists.
* **Integration-test Job**: `deploy_run_job image=<name deploy_load returned>` (optional `name`, `command`, `env`, `timeout_seconds` default 300 up to 1800, `keep`, `allow_root`) renders a Job (restartPolicy Never, backoffLimit 0, activeDeadlineSeconds, non-root, requests and limits, labels managed-by=iyakki and the workspace, `BASE_URL` = `http://<service>.<ns>.svc.cluster.local:<port>` when exactly one Service exists), runs it through the same manifest linter and apply as `k8s_apply` (the file is kept in `.iyakki/jobs/`), polls the Job status, then returns pass/fail, the pod logs (last ~6000 characters) and the counts and coverage found in them, and deletes the Job unless `keep`. A Job that does not finish reports the pod's reason (for example ImagePullBackOff, with the registry diagnosis); a bare `localhost/` image on a remote cluster is refused with "run deploy_load". It asks, works only in the workspace namespace and never on a production-looking or environment-only remote cluster.
* The skill `dist/skills/k3s-dev-best-practices.md` and the deploy prompt use these names.

## Pods that cannot start

* **Diagnosis** (deploy_status, k8s_describe, k8s_events, like the registry pull diagnosis): `runAsNonRoot` with an image that runs as root or has a name as USER ("add `USER 10001` (numeric) after installing, chown the app files, rebuild with a new tag, redeploy; or set `runAsUser: 10001`"), a missing ConfigMap/Secret or key, an invalid volume mount, and an ImagePullBackOff for an image name with no registry address that was never built or loaded in this workspace (it would be looked up on Docker Hub: "use the name deploy_load returned").
* **Next:** when pods are not Ready `deploy_status` ends with one line telling the model the next concrete action (add the USER, deploy_build + deploy_load, create the ConfigMap, read the previous logs, describe the pending pod).
* **Before applying**, `k8s_apply` (and its preview) warns about: `runAsNonRoot: true` while the workspace Dockerfile for that image has no USER or a non-numeric one; a second Deployment in the namespace whose selector shares a label with the one being applied but has another name (a stale manifest: delete it with `k8s_delete kind=deployment`, remove the stale file in deploy/, both named in the warning); and an image name with no registry address that deploy_build/deploy_load did not return in this session (a public image such as nginx is fine).

## Clusters: several, local and remote

One account can have **several clusters** and work against one of them at a time (`crates/iyakki-agent/src/clusters.rs`, Settings → Clusters).

* **The list (the registry).** `clusters-<user>.json` next to the other settings. Entries: `local` ("This machine", the managed k3s of this document, always there and always first), optional **extra local clusters** (named; each a complete k3s of its own) and **remote** profiles of the kinds `remote`, `tunnel` and `cloud-cli` (a kubeconfig path or a vault entry `uvault:<name>` / `vault:<name>`, a context, a namespace). A production-looking name stays read-only for the assistant, as before.
* **Migration.** The first time the list is read, it is built from the old files: the vault profiles of `devcluster-vault-<user>.json` become remote entries, and the remembered choice `devcluster-<user>.json` becomes the active cluster (a path that is not a managed cluster becomes a remote entry named after the file). The old files are never edited or deleted by the registry. When the person changes the active cluster the old remembered-choice file is written too, so everything that still reads it (Platform services, the registry row, the flows) follows. A vault profile added on the old Dev cluster page after the list was saved is appended to the list on the next read (`Registry::load_synced`; nothing else is changed). With only `local` nothing changes and no switcher is shown.
* **Extra local clusters.** The same supervisor code as the first one (`k3sdev::K3sDev::new_instance`): the same start gate (requirements, network, **clock**), watchdog, health and alerts apply to every instance. An extra cluster has its own folder `k3s-dev-<id>` (state.json, k3s.log, kubeconfig, data), its own API port (`6443 + 100 × slot`: 6543, 6643, ...), kubelet ports (`10250 + 10 × slot` and the four after it), node name `iyakki-dev-<id>`, and non-overlapping ranges (cluster `10.<42+2×slot>.0.0/16`, services `10.<43+2×slot>.0.0/16`, DNS `.10`), passed to k3s as `--https-listen-port`, `--cluster-cidr`, `--service-cidr`, `--cluster-dns` and `--kubelet-arg`/`--kube-*-arg` ports. `local` keeps exactly today's command line. Slots and ids are also unique across accounts (a local cluster belongs to the machine). The typed confirmation of Reset and Uninstall is the node name of that cluster. Another managed cluster running is not "a cluster found elsewhere"; a foreign k3s is still refused.
* **Limits and cost.** At most **2** local clusters by default (the built-in one included); **Advanced** raises it to 4. Every local cluster is about 2 GB of memory, CPU cores and 3 GB of disk, and the page says so when one is added. Starting a second running local k3s needs about 2.5 GB of `MemAvailable` (its own 2 GB plus the 512 MiB reserve of the memory gate); otherwise it is refused with the numbers ("only 1.0 GB of memory is free; a second k3s needs about 2.5 GB"). The first local cluster is never refused by this gate. If the memory cannot be read, a second one is refused.
* **Active cluster.** One per account, and an optional override per workspace (the AI IDE). `Registry::effective(workspace)` is the cluster the screens use: the K8S tile (its rows are the list's clusters; kubectl gets that cluster's kubeconfig and context), the AI IDE (`clusters::remembered_for(workspace)` is what the cluster resolution is given, so a workspace's own cluster wins over the account's; a pin in `project.json` still wins over both, and choosing in the switcher removes that pin), the assistant's cluster tools (their audit lines end with ` on cluster "gpu-box" (remote)`, and a production-looking cluster stays read-only for them), and Platform services and the registry row (through the remembered choice the switch writes). Switching never moves or deletes anything. Actions that change a cluster name it in their confirmation text (`clusters::confirm_text`, `confirm_text_on`, `confirm_text_from`): `Deploy Lemonade to cluster "gpu-box" (remote)`, `Restart deployment web on cluster ...`, `Delete pod web from cluster ...`, the IDE deploy's title and its apply step, the K8S tile's Allow changes message and every audit line.
* **Switcher.** A compact drop-down (`apps/cluster_switch.rs`) showing the active cluster's name and a status dot; its list has a dot (running, stopped, unreachable, stale, not checked), the kind, a GPU badge when the AI hardware profile knows one, and "Manage clusters...". It is hidden when there is one cluster (nothing drawn, nothing clickable). One shared host (`apps/cluster_host.rs`) does the rest for every screen: it reads the list at most every two seconds, builds the rows (a local cluster's dot from its supervisor, a remote one's from its last look), switches through `clusters::switch` (or `switch_workspace` when the workspace has its own cluster), audits it (`cluster.switch`) and asks the screen to open Settings → Clusters for "Manage clusters..." (`tab:clusters`). **Where it sits:** the K8S tile's header (in place of the plain cluster list, which stays for one cluster), the AI IDE's Kubernetes panel header (the Deploy, Registry and Services buttons are in the same panel), and the popup of the Store → Setup items that go into a cluster (Registry, Lemonade, Indic-TTS, DuckDB, Elasticsearch).
* **GPU badge.** `clusters::gpu_summary(id)` reads the AI hardware profile's cache of that cluster's nodes (`aiprofile_remote::clusters_cache`): `2 x NVIDIA GeForce RTX 4090 (24 GB each)`, with ` (last known)` when the last refresh failed, nothing for a cluster without GPUs or one never read. It only reads the cache. `clusters::refresh_gpus` fills it for the remote clusters of the list on a thread, with the same back-off as the chosen cluster (60 s after an answer; 30 s doubling to 10 min after a failure), from the same read-only `kubectl get nodes -o json` the Clusters page's Test runs (one answer feeds both). Clusters on this machine are not asked; tests never start it.
* **Settings → Clusters** (key `C` on the page, formerly Dev cluster): list, add remote (kubeconfig with the file picker, or a vault entry), add local (with the cost warning), rename, Test (read-only `kubectl get nodes`), Start / Stop (local only; each asks with the cluster's name), Set up (an extra local cluster: downloaded and checked against the vendor's checksum like the first), Remove (local: asks, keeps the data folder unless "Also delete data" is chosen; remote: only forgets the profile, and its vault entry only when asked), Autostart per extra local cluster (default: only `local`, as today), visibility to users, Advanced. Status per cluster: version, nodes ready, last error.
* **Roles.** Administrators and operators manage; users switch among the clusters marked visible to them; guests see none.
* **Not done / not verified.** Two k3s servers on one machine share the host network namespace; the ports and ranges above are distinct, but flannel's VXLAN device and the `cni0` bridge are per host, so two clusters' pod networks may still disturb each other. This was never run on real hardware; the tests use a stand-in k3s. The Podman variant is built the same way and untested. Nothing here talks to a remote cluster except a read-only `kubectl get nodes`.

## The K8S tile

To look around the managed dev cluster (or one of your saved kubeconfig profiles) in any namespace, without a workspace, use the **K8S** tile: see [`K8S-TILE.md`](K8S-TILE.md). It shows the cluster's state (*Running*, *Starting*, *Waiting for the network*, *Stopped*) with the same Start as Settings → Dev cluster, and is read-only until an administrator turns on Allow changes.

## What is built (phases 4 and 5)

Everything here is covered by tests that use fake runners, a stand-in `k3s` script, a fake downloader and temporary folders. **No real
download, no real cluster and no pod was involved.** The checklist at the end says what has to be looked at on the real machine.

### Settings → Dev cluster (administrators)

`crates/iyakki-shell/src/apps/settings_devcluster.rs`. A card for **this machine's own cluster** on top, the list of **profiles** below, a
strip with the selected profile's details, the namespace rule and the registry, and the profile buttons at the bottom.

* **Profiles**: this machine's k3s, the cluster IYAKKI installed (badge "managed by IYAKKI"), `$KUBECONFIG`, `~/.kube/config`, this pod's service account
  ("in-cluster") and kubeconfigs held in a vault ("vault"), plus the clusters the person hid. Each row has the server, the node name, the version, the
  reachability (`kubectl version`), the badges your default / default for a workspace / remote / k3s / production-looking / read-only, and the strip
  says why the assistant only reads (`devcluster::read_only_reason`, the same text the tools use). **Test** runs `kubectl version`, `get nodes` and
  `auth can-i` (and `auth can-i --list` for the pod's account); **My default** writes the per-user choice; **Workspace default** writes the workspace's
  `project.json` (a vault entry, or a file the account may not read, stays in the person's own choice); **Hide/Show** is IYAKKI's own list (the file is
  untouched, a hidden cluster is not probed at all); **Disconnect** shows what it will forget and asks; **Add vault profile** takes a name and the
  name of a vault entry that exists (only the reference is stored); **Registry** is the A/B/C dialog of the K8S panel, bound to the workspace chosen with
  **Next workspace**: it shows the exact plan (`regsetup::preview`) and nothing runs, and nothing is recorded in `project.json`, before **Approve**.
* **Who**: the page is for administrators. Ordinary accounts keep using the AI IDE's Cluster… chooser (hide, show, disconnect, registry live there too).
* **The card says what the machine allows**: *Managed by IYAKKI* (buttons below), *No cluster on this machine* (the Set up button, only when nothing is in
  the way), *This machine already manages its own cluster* (a k3s/rke2 file, service or process, a connected cluster or the sandbox cluster: only profile and
  registry management is offered, in a window too), *Desktop window* with none found (never installs), and why setup cannot run yet (not root, no cgroup v2,
  the data folder is in memory, less than 3 GB free).

### Set up a local cluster (bare metal)

`crates/iyakki-agent/src/k3sdev.rs`. **Never starts a second cluster**: `existing()` looks for `/etc/rancher/k3s/k3s.yaml`, `/var/lib/rancher/k3s`, k3s/rke2
programs and service files, processes named k3s/k3s-server/rke2/kubelet/… and a connected or sandbox cluster, and install and start refuse (and are audited) when it
finds one. In a desktop window they refuse always.

1. **The plan** (`lookup` then `plan_text`; only reads the network: the "latest" redirect, the checksum file, the headers of the download). It lists the k3s release URL,
   the official `sha256sum-<arch>.txt` and the expected hash, the size, where it installs (a folder in the data partition: `/var/lib/iyakki/k3s-dev/{bin/k3s,data,kubeconfig,k3s.log,state.json}`;
   nothing in `/usr/local`, `/etc` or `/var/lib/rancher`), the exact `k3s server …` command (traefik and servicelb off; metrics-server stays on), the ports (6443/tcp API on every
   interface with TLS and a token, 10250, 8472/udp, NodePorts 30000-32767), whether it needs root (it does), what it needs (2 GB, cgroup v2, 3 GB free) and the registry option
   (A in-cluster, the default, or B system, or none; applied afterwards as a separate step you approve).
2. **Approve** runs `install`: the download streams to a temp file inside the folder while its SHA-256 is computed, is compared with the vendor's file, and only then
   made executable and moved into place (on a mismatch the file is deleted, nothing is installed and `devcluster.checksum.mismatch` is audited); `state.json` records version, hash and who.
   Then `start`.
3. **The service**: a background process of IYAKKI in its own process group (in the RAM-root mount-namespace jail of `cluster.rs` when `/` is a RAM file system),
   kubeconfig written `0600` by k3s and registered as "k3s on this machine" for every cluster resolver. A supervisor thread restarts it with a back-off (5 s, 10 s, … 60 s) and gives
   up after 5 failures in 10 minutes (state *failed*, with the reason and the end of the log). k3s refuses to start without a network interface that has an IPv4 default route (`no default routes found in "/proc/net/route"`), and after a boot DHCP may not have installed one yet, so before every start (Autostart, Start, and a restart after k3s died while the route was missing) IYAKKI waits for a default route in `/proc/net/route`: the status says *starting · waiting for the network…*, after 90 s *no network yet: k3s needs a network interface with a default route*, the wait never gives up (it polls every 3 s and starts k3s when a route appears), Stop cancels it, and waiting is never counted as a failure or a restart and raises no alert. **The supervisor never goes quiet.** Every way out of its loop is audited as `devcluster.supervisor-stopped` with the reason (a panic included); if the cluster should have been running the state becomes *failed* and the administrators get the "keeps failing" alert, never *running* with no process. A lost process handle counts as an exit. How k3s ended is written to `k3s.log` (`iyakki: k3s exited (signal 9 (SIGKILL))`) and audited with the status or signal. A watchdog thread notices a supervisor that has shown no sign of life for 150 s, records the exit, starts k3s again (counted as a restart) and replaces the supervisor. Only one start runs at a time, so Autostart, a quick-install start request and the Settings button cannot start two k3s processes at boot. **Moving between networks:** if k3s ends after the IPv4 address of the default route's interface changed since it was started (the stick travelled), that is expected: IYAKKI starts it again at once, counts no failure and raises no alert (`devcluster.exited-ip-changed`). IYAKKI does not pass `--node-ip`: it is read again at every start and k3s picks the default route's address itself, and a fixed flag would go stale at the next move. A machine that is offline on purpose cannot run the cluster: k3s needs an interface with a default route (a dummy one with a default route is the usual workaround; IYAKKI does not add one). **Autostart** (default on) starts it when IYAKKI starts (`Services::open`), unless something else is in the way.
4. **Start, Stop, Restart** show the command or the signals first and need Approve. **Logs** shows the log tail as selectable text. **Upgrade** looks for a newer release, shows the
   plan, downloads and verifies first, stops, replaces `bin/k3s` (the old one stays as `k3s.prev`) and starts again if it was running. **Reset** (type `iyakki-dev`) stops the cluster and deletes
   only `k3s-dev/data` (never through a link, never outside the managed folder). **Uninstall** (type `iyakki-dev`) deletes the managed folder. Every call is audited as `devcluster.*` with the name of the person; values are never logged.
5. **The Podman variant** is offered only where `podman` is installed (the form refuses it otherwise and says the image needs the podman packages): `podman pull docker.io/rancher/k3s:<version>`
   (no separately published checksum; the digest it ends up with is recorded), `podman create --name iyakki-k3s --privileged --network=host …`, then `podman start/stop` and
   `podman exec … kubectl`. Only the container of that name is ever removed. **It has never run.**

### The clock at boot (wrong date, "sometimes runs, sometimes not")

Measured on a MacBookPro14,1: the hardware clock reads 2024-01-01 00:02 at boot and the time server sets the clock about 33 s later (the first try at 13 s fails: Wi-Fi/DNS not ready), a jump of 2.8 years. k3s used to start while the clock was still 2024; its certificates (made in 2026) were "not yet valid" (`CA cert validation failed` in `k3s.log`), and after the jump it sometimes recovered and sometimes not.

* **Clock floor** (`clockfloor.rs`, started from `boot.rs`; F-CLOCK-01): the clock is never left earlier than the largest of the compiled-in date, the image's build time and the saved time (`/var/lib/iyakki/clock-floor`, written atomically at shutdown, every 5 minutes and when the network time is set; a saved time more than 20 years beyond the build time, damaged or missing is ignored; first boot uses the build time). The clock is only moved forward, with the same `clock_settime` call the time client uses. It is applied at the very start of boot and again once the data volume is mounted (that is when the saved time can be read).
* **Start gate** (`k3sdev.rs`; F-K3S-27): the supervisor waits for a default route and then for the clock: the floor applied AND (a time server answered OR 120 s of uptime). Offline machines therefore still start k3s after two minutes. The status shows "waiting for the clock to be set…" and `devcluster.waiting-for-clock` is audited.
* **Clock step while running**: the supervisor compares the wall clock with the monotonic clock on every look (every 2 to 10 s). A jump of more than 3 minutes restarts k3s once per jump (at most 3 times per supervisor run), audited as `devcluster.restart-clock-step`, not counted as a failure and no alert; a k3s that ends right after a jump is started again the same way. If `k3s.log` shows `CA cert validation failed` or an x509 "not yet valid" within 2 minutes of a start, k3s is restarted once (`devcluster.restart-clock-cert`) when the clock is plausible. Native only; the Podman variant gets the start gate but no restart on a jump.
* **The audit log** keeps its format: the hash covers `ts|user|event|detail|prev`, so a jump of the clock only shows as a large step in `ts` between two entries (the chain stays intact). Entries written before the first time sync carry the floor time, not the true time.
* **Not done**: tried only with fake clocks and a stand-in k3s; nothing has run on a device. To check on the MacBook: `date` right after boot (not 2024), `dmesg | grep -i clock`, the audit entries above, `k3s.log` without `CA cert validation failed` after the first minute, and k3s up on five boots in a row.
* **Known noise**: `cannot find filesystem info for device /dev/sda3` lines from cadvisor; see [`TROUBLESHOOTING.md`](TROUBLESHOOTING.md). Not changed.

### Health in SYSTEM, and alerts

The Overview has a **Cluster** row when IYAKKI installed one: state (running / starting / failed / stopped), version, node Ready, pods running/all, size of the data folder, how full its
partition is, restarts and the last restart reason, and **View logs** (key `l`, selectable). The supervisor reads the figures every 10 s while it runs. One warning each goes to the
signed-in administrator's INBOX (and a toast) when the cluster goes down (the first unexpected exit of an outage), when the supervisor gives up or the node never becomes Ready, and when the
partition passes **85 %** (it re-arms under 80 %).

### Cloud mode (phase 5)

`crates/iyakki-agent/src/kubeprofiles.rs`.

* **In-cluster profile**: when `KUBERNETES_SERVICE_HOST` is set and the service account token and CA are mounted, a kubeconfig (mode 0600) is written that names the token file and the CA file (the
  token itself is never copied) and the resolver lists it as "In-cluster" (source "this pod's service account"). The profile's `can_i` is `kubectl auth can-i --list -n <workspace namespace>` and is shown on
  the Dev cluster page. A service account that may do everything (`*.*` with `*`) makes the cluster read-only for the assistant; otherwise writes go through the usual approvals, confined to the workspace namespace.
* **Vault-held kubeconfigs**: the profile's kubeconfig is `vault:<entry>` (system vault) or `uvault:<entry>` (the person's vault). `wsexec::System` (the only way programs are started for clusters) swaps the reference
  for a new `0600` file in `/dev/shm` for that one child (`--kubeconfig` or `KUBECONFIG`), and the file is removed when the child ends (`Drop`; stale ones are swept at sign-in). If the vault is locked the child is not started.
  The resolver is installed at sign-in for the signed-in person and removed at sign-out.

### What must be checked on the real bare-metal box (checklist)

1. Boot the image with a data disk (Settings → Storage); Settings → Dev cluster shows "No cluster on this machine" and **Set up a local cluster** (as root, cgroup v2 present, 3 GB free).
2. Press it, read the plan (URL, hash, size, paths), Cancel once (nothing must appear in `/var/lib/iyakki/k3s-dev`), then approve. Watch the status line: download, checksum, "Installed native k3s …".
3. The first start: the node becomes Ready within a few minutes (SYSTEM Overview: *running · v1.… · node Ready · N pods*); `k3s.log` has no `pivot_root` or `cannot find filesystem info for device rootfs` errors (the RAM-root jail with the new data directory is the least certain part).
4. The profile list shows the new cluster (managed by IYAKKI, reachable, node name `iyakki-dev`); Test connection works; make it your default; the K8S panel of the AI IDE connects to it.
5. **Registry** (g): choose A, read the plan, approve; `localhost:30500` must answer from the box; then a full Deploy (build, push, apply) of a small app.
6. Stop, Start, Restart; `kill -9` the k3s process and watch the supervisor restart it (SYSTEM shows the restart and its reason) and an INBOX warning appear once.
7. Reboot with Autostart on: the cluster comes up by itself (no one signed in); with Autostart off it does not.
8. Fill the data disk past 85 % (or check the number against `df`): the INBOX warning; the disk figure matches `df`.
9. Reset (type `iyakki-dev`): only `k3s-dev/data` is emptied; Start brings up a fresh cluster. Uninstall removes `k3s-dev`.
10. A foreign cluster: install k3s yourself on a second machine/VM and open the page there: it must say "This machine already manages its own cluster" and offer no Set up.
11. Pull the network cable before pressing Approve: the failure must be a clear message and leave nothing behind.

### The Podman variant: what the image needs

The default images (libre, hw, web) do **not** contain podman, and this work does not change them. The Podman variant of the page therefore refuses to run, saying so. To try it, the image needs, at least:
`podman`, `conmon`, `crun`, `netavark` (+ `aardvark-dns`), `iptables`, `iproute2` and CA certificates, a kernel with cgroup v2, overlayfs, veth, bridge, VXLAN and netfilter (nat, conntrack,
masquerade), and a writable `/var/lib/containers` on the data partition. `dist/buildroot/configs/iyakki_cluster.fragment` lists the Buildroot options; it is documented, **not built and not
wired into `build.sh`**, and the size and RAM it adds must be measured first. Rootful Podman only (k3s cannot run rootless).

### Not done

* Running any of this on the real machine, in a real pod, or against a real remote cluster; the Podman variant; multi-node (agent nodes) clusters; a rollback button for Upgrade (the old program is kept as `k3s.prev` but must be moved back by hand);
  if the IYAKKI shell process restarts while the cluster runs, the new process does not adopt the running k3s (it is refused as "a process named k3s-server is running", so no second cluster is started; stop it from SYSTEM → Processes or reboot);
  applying the registry option automatically after setup (it is a separate approved step on purpose); alerts reach only an administrator who is signed in; the vault resolver serves the one signed-in person per process.

## What is built (platform services: Gitea, the private store)

Settings → Dev cluster → **Platform services** (key `P`), the STORE → Settings "Private store" card and the K8S panel's **Services…** open one panel: pick a cluster profile (production-looking, read-only, hidden and unreachable ones are refused), a namespace (`iyakki-<service>`, created and labelled as a visible approved step, or an existing one already labelled `managed-by=iyakki`), review the exact plan (commands, pinned images, PVC, ports, the credentials created, what it exposes, the manifest), approve, watch the progress, and read the result. Installed services (`installed-services-<user>.json`) get Open, Stop/Start, Upgrade (forward along pinned versions, backup first), Backup, Remove (the data is kept unless asked, and the instance id is typed) and Details. Gitea is the first service (rootless, SQLite on a PVC, NodePort pair by default, an API token for IYAKKI in the vault); an existing Gitea or Forgejo in the cluster is detected and can be connected instead; a workspace can be connected to Gitea (private repository, `origin`, push after a second approval). The assistant has `platform_list`, `platform_status`, `git_service_status` (Safe) and `platform_deploy`, `platform_upgrade`, `platform_remove` (hard-ask). Design, security and the Gitea decision: `docs/PLATFORM-SERVICES.md`. Built and tested with fakes only; not run against a real cluster.

### Looking after the private store (repair, reset, remove)

The panel above also reads the private store's state before deploying it and offers Repair, Reset and reinstall, and Remove (`store/privatemanage.rs`, `docs/STORE.md` section 13.2).
It reads and writes only the chosen cluster and the one namespace (`iyakki-private-store` by default, and only when it carries `managed-by=iyakki`). With no `kubectl` on the machine the managed
cluster's `k3s kubectl` is used (the same rewrite as everywhere); with no kubectl at all, or with the cluster down, it says so and changes nothing (for the managed cluster it
offers to start it). Reset and Remove run as a detached job, `iyakki store-manage reset|remove --kubeconfig FILE [--context NAME] --namespace NS [--delete-data] [--wait SECONDS] [--force]`,
in the installer workspace, so the top bar shows it and JOBS has the log. The in-cluster registry (Quick install, Local image registry) is never touched unless you ask for it in the Remove choices.
