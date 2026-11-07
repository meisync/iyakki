# Cloud sandbox plan

Owner request 2026-11-04: "also sandbox (IYAKKI browser (optional) - CLIs etc)" for cloud mode. This page is the audit of what exists, the
threat model, the three isolation tiers (and the optional browser tier), the policy kind `sandbox.policy`, and what is built
(`crates/iyakki-agent/src/sandbox/`) and what is not. Nothing here was run against a cluster.

## 1. Audit: what exists today

Proven = read in the code and, where a test exists, named. Guessed = my reading, not checked by running anything.

| piece | finding | status |
|---|---|---|
| `iyakki-wasm` (`Sandbox`, `SandboxConfig`) | wasmtime with fuel, a memory cap (`StoreLimits`), an optional wall-time epoch, capability tokens (`iyakki-sec`) for files and hosts, an in-memory file system with a quota. `iyakki run` and `wtool.rs` (pkg tools) use it. | proven |
| `linuxapps_init.rs` (`Spec`) | a real Linux sandbox: user, mount, pid, uts, ipc and (optionally) net namespaces, cgroup v2 memory/cpu/pids, `pivot_root` into a folder, minimal `/proc` and `/dev`, capability drop, `no_new_privs`, seccomp deny-list. `Spec::to_args`/`parse`/`validate` are a pure round trip. `docs/LINUX-APPS.md` says it was tried on a development PC and **not on an IYAKKI device**. | proven (code), untried on a device |
| `cluster.rs` `sandbox_run` (desktop tool `sandbox_run`) | a Job per run in the local k3s: runAsNonRoot, drop ALL, memory limit, no service account token, a `no-network` NetworkPolicy label; code passed as one argument. A desktop-only helper (single user, local cluster); it has no tenant, no policy, no image allow-list, no per-run NetworkPolicy of its own. | proven; the sandbox layer replaces it in cloud mode, not on the desktop |
| `run_command` (`agent.rs`) | the person approves each call (`Risk::Ask`); `exec::check_with` allows `ssh aws gcloud` and installed CLIs, no shell; `hard_ask` always asks for ssh/destructive verbs/paths outside the workspace. **It runs the program on the machine, in the pod in cloud.** `cloudmode.rs` hides hardware apps only; nothing blocked `run_command` in a tenant pod. | proven. This is the gap this work closes |
| `tenantconfig` | a registry of kinds with typed fields, merge rules (tenant > team, a lower layer only narrows), safety rules with `block`/`warn`/`info`, `k3s_decide` for the team's cluster binding, one approvals router (`approvals.rs`: the admins of the requester's team, never the requester), `fairshare.rs` (per-person and per-team concurrency and a queue). | proven |
| egress proxy (`iyakki-proxy`, F-SEC-04) | host allow-list, key injection, response sanitising. Its rules come from its own configuration: **no per-run control interface**. | proven; so proxy-injected secrets for a run are refused until one exists (`NoInjector`) |
| cloud pod (`dist/k3s-web.yaml`, `cloudtenant.rs`) | the tenant pod itself is hardened (non-root 65532, read-only root, drop ALL, RuntimeDefault, `automountServiceAccountToken: false`), with an ingress NetworkPolicy. It sets `IYAKKI_TENANT_DATA_DIR`. It has **no cluster access by design**, so applying a Job needs the team's cluster binding (a kubeconfig in the vault). | proven |
| flows (`flows/host.rs`) | `cli` nodes run `local_exec` on the host or `k3s_exec` through a `deploy_run_job` (a Job, `flows-<user>` namespace; refuses secrets). `browser` nodes use the IDE or a headless session on the host. The trait is `iyakki_flow::host::Host` (`exec`, `browser`). The other agent's `cloudflows/` (an `Executor` trait whose `K3sJob` asks `k3s_decide`, and `job.rs`, a text-YAML builder: UID 65532, read-only root, drop ALL, RuntimeDefault, no token, but **no NetworkPolicy, no workspace volume, no size limits, no runtime class, no inputs**) appeared while this was written; `JobSpec` has `secret_env` as Kubernetes Secret references, which a sandbox spec (vault references only) cannot carry. | proven |
| browser | `iyakki browser-test SPEC.json [--out DIR] [--json] [--profile DIR]` runs the deterministic runner against the web engine helper; `allow_domains` bounds it; `IYAKKI_BROWSER_MEDIA=allow|deny`; the helper is `/usr/libexec/iyakki/webview-helper` (WPE) or the GTK host helper. A pod needs `iyakki` and the helper in its image, and a headless display path. | proven (CLI, env); **guessed**: that the WPE helper starts headless in a plain pod (not tried) |
| Kubernetes pods and user namespaces | pods usually need seccomp/AppArmor relaxed (and often `hostUsers: false` or a userns-capable runtime) for `unshare(CLONE_NEWUSER)` and `mount`; the default RuntimeDefault profile blocks them. | guessed (general knowledge, nothing tried here) |

## 2. Threat model

Who: a tenant member, a flow, or an AI agent acting for them, running arbitrary commands, scripts or browser sessions on behalf of a tenant.
Assume the code is hostile or tricked (prompt injection in a page or a file).

Goals, each with the mechanism that makes it true:

| goal | mechanism |
|---|---|
| one run cannot read another tenant's or another run's data | a pod and a workspace `emptyDir` per run; no shared volume; a fixed high UID; labels per tenant, team, person and run; NetworkPolicy has no ingress; the tenant pod's volume is never mounted |
| cannot reach the cluster API, the metadata service or internal hosts | `automountServiceAccountToken: false`; no service links; a per-run NetworkPolicy that allows only DNS and the egress proxy pods (or nothing); the proxy has the allow-list (no `ipBlock`, no `0.0.0.0/0` anywhere, golden test) |
| cannot exhaust the node | CPU/memory requests and limits, ephemeral-storage, `sizeLimit` on every `emptyDir`, `activeDeadlineSeconds`, `ttlSecondsAfterFinished`, per-person and per-team concurrency with a queue, output caps |
| cannot persist outside its workspace | `readOnlyRootFilesystem`, `emptyDir` workspace deleted with the pod, no `hostPath`, no PVC (a named browser profile is the single, opt-in exception) |
| secrets only by reference or injection | a spec holds `secret_ref` (`vault:ns/name`) only; delivery is a per-run Secret mounted 0400 on a volume that dies with the run, or (planned) injected by the proxy so the run never holds it; env values that look like credentials are refused; output is scrubbed of every value the run held |
| everything audited | `sandbox.start`, `sandbox.end`, `sandbox.refused`, `sandbox.needs_approval` lines (run id, tenant, team, person, tier, program name only, argument count, image registry, limits, network mode, approver); never argument or environment values; plus `tenant.k3s.deploy` from the cluster binding |

Out of scope here, said plainly: a kernel or runtime escape (that is what `runtimeClassName` gVisor/Kata is for, a WARN when missing on a
shared node); the supply chain of an allowed image (digest pinning is available, `require_digest`); a malicious but allowed proxy host.

## 3. The tiers

| tier | id | what | when | status |
|---|---|---|---|---|
| T0 | `wasm` | `iyakki-wasm`: fuel, memory, wall time; no files, no network, no secrets | tools and scripts that compile to wasm | built (`WasmExecutor`) |
| T1 | `namespace` | `linuxapps_init` inside the pod: user/mount/pid/uts/ipc/net namespaces, cgroup limits, capability drop, seccomp; **no network and no secrets** (it would share the pod's network) | only when the tenant admin lists it and sets `userns_ack`: the pod must allow user namespaces, which usually means relaxing the pod's seccomp/AppArmor and so weakens the pod itself. Never chosen automatically | built (`LocalNsExecutor`, launcher behind a trait); real clone not tried |
| T2 | `job` | one hardened Job and NetworkPolicy per run | **the default in cloud** | built (`JobExecutor`), goldens; never run on a cluster |
| T3 | `browser` | the IYAKKI browser in a Job of its own (optional) | off unless `sandbox.policy.browser: true` and `browser` is in `tiers` | built (manifest and admission); the runner inside the pod is `iyakki browser-test`, not tried in a pod |

T2 hardening, every field written out in `job.rs` and pinned by a golden test: `runAsNonRoot` with UID/GID 10001, `readOnlyRootFilesystem`,
`allowPrivilegeEscalation: false`, `privileged: false`, `capabilities.drop: [ALL]`, `seccompProfile: RuntimeDefault` (pod and container),
`automountServiceAccountToken: false`, `hostNetwork/hostPID/hostIPC/shareProcessNamespace: false`, no `hostPath`, an `emptyDir` workspace and
`/tmp` with `sizeLimit`, requests and limits for cpu, memory and ephemeral storage, `backoffLimit: 0`, `activeDeadlineSeconds`,
`ttlSecondsAfterFinished: 600`, `enableServiceLinks: false`, optional `runtimeClassName`, image from the tenant's allowed registries only
(digest pinning required when `require_digest`). A per-run NetworkPolicy (policyTypes Ingress and Egress, no ingress): `none` means no egress;
`proxy` means DNS (kube-dns) and the egress proxy pods on their port only. `HTTP(S)_PROXY` is set by the builder; a spec cannot set it.

T3 browser: a Job with the browser image; command `iyakki browser-test /inputs/spec.json --out /workspace/out --json --profile /profile`; the
spec JSON is made from the start addresses, the steps given and a final screenshot, `allow_domains` = the requested domains, every one of which
must be on `sandbox.policy.browser_domains`; `allow_payment` and `allow_destructive` false; `IYAKKI_BROWSER_MEDIA=deny` (clipboard, camera, mic,
sound); the profile is an in-memory `emptyDir` (no cookies survive) unless the tenant sets `browser_profile: named`; a downloads `emptyDir` with
`browser_download_mib`; network only through the proxy; the session time is the smaller of the run limit and `browser_session_secs`. Screenshots
return as artifacts under the same size caps as any output.

## 4. Policy: tenant configuration kind `sandbox.policy`

A single value per tenant, narrowable per team (a lower layer can only narrow, the registry's merge rules). **Off unless switched on**; absent
or unreadable is "everything refused" (`SandboxPolicy::off`). Fields (schema.rs `SANDBOX`): `tiers` (default wasm, job), `browser` (false),
`userns_ack` (false), `allowed_registries` (none), `require_digest`, `max_cpu_m/mem_mib/pids/seconds/workspace_mib`, `network_modes` (none,
proxy), `runtime_class`, `cli_role` (editor), `approval_patterns`, `per_person_concurrency` (2), `per_team_concurrency` (8),
`queue_timeout_secs`, `audit_retention_days` (90), `k3s_environment` (dev), `browser_domains`, `browser_profile` (tmpfs),
`browser_download_mib`, `browser_session_secs`, `browser_image`, `cli_image`, and five switches that exist only so a safety rule can refuse
them: `allow_privileged`, `allow_host_network`, `allow_host_path`, `allow_root`, `allow_cap_add`.

Safety rules (safety.rs): BLOCK `sandbox.privileged` (any of the five switches true), `sandbox.net.direct` (direct internet in `network_modes`),
`sandbox.net.unknown`, `sandbox.tier.unknown`, `sandbox.userns.ack` (namespace tier without the acknowledgement), `sandbox.browser.off`
(browser tier listed without the switch). WARN `sandbox.runtime_class.missing` (job or browser tier without a runtime class on shared nodes),
`sandbox.registry.any`, `sandbox.registry.none`, `sandbox.browser.domains`, `sandbox.browser.profile`.

Approval: built-in patterns (sudo, docker and runtime CLIs, ssh/scp/sftp/nc, `kubectl|helm|oc` apply/delete/replace/patch/drain/uninstall/exec,
`terraform|tofu` apply/destroy, `rm -r`/`mv`/`find -delete` leaving the workspace, a download piped into a shell, `curl | sh` and `<(curl)`)
plus the tenant's `approval_patterns` (`program word word`: the words must appear among the arguments). A match returns `NeedsApproval`; the
caller files it with the approvals router (`kind sandbox.policy`, item `cmd-<hash of the exact argv>`, so an approval covers that command once);
the team's admin (never the requester) approves; `take_approval` gives the approver back and the second try passes `approved_by`.
The cluster binding is asked as well (`k3s_decide` through `K3sGate`, environment `k3s_environment`).

## 5. What is built

`crates/iyakki-agent/src/sandbox/`: `spec.rs` (SandboxSpec: argv array, env, secrets by reference, workdir, inputs, limits, network, tier, labels,
and the escalation requests that exist only to be refused; SandboxResult), `policy.rs`, `admit.rs` (`choose_tier`, `admit`, approval patterns;
`Admitted` can only be made by `admit`, and every executor and manifest builder takes one, so a refused request has no manifest), `job.rs`
(Job, NetworkPolicy, ConfigMap of inputs, Secret object, browser spec: pure), `kube.rs` (`KubeApi` trait, `Kubectl` over stdin), `exec.rs`
(`Executor`, `JobExecutor`, `LocalNsExecutor`, `WasmExecutor`, `ApplyGate`/`K3sGate`, `SecretResolver`, `Injector`), `limiter.rs`
(fairshare), `output.rs` (credential filter `bugs::redact`, held values, caps), `engine.rs` (admit, queue, run, redact, audit),
`wiring.rs` (`run_command` route, `TenantRunner`, flow adapters). In `agent.rs`, `run_command` asks `wiring::route()` first.
Tests: `sandbox/tests.rs`; policy and rule tests included there.

## 6. Seams and gaps (read before trusting any of this)

* **Cluster access.** The tenant pod has no service account token. `JobExecutor` needs a `KubeApi` for the team's cluster; `K3sGate` checks
  the binding, but turning the binding's vault credential into a kubeconfig for `Kubectl` is **not built**. Until the pod start installs a
  `CloudRunner` (`wiring::install`), `run_command` in a tenant pod is refused (fail closed), never run on the pod.
* **Results from a Job.** Logs are stdout and stderr together (the cluster does not separate them); exit code and OOM come from the pod
  status. **Artifacts from a finished pod are not retrievable** with plain kubectl (the container is gone): the real `KubeApi::artifacts` returns
  none. Planned: the run wrapper prints framed artifacts on its log, or an object-store hand-over.
* **Proxy injection per run** needs a run-scoped rule interface in `iyakki-proxy`; refused until then. File delivery works (Secret, mode 0400).
  "Read once" is a convention of the program inside, not enforceable by Kubernetes.
* **pids.** Kubernetes has no per-pod pid limit in a manifest (kubelet `podPidsLimit` is node-wide); the limit is checked against policy and
  used by T1, not written to a Job.
* **Flows.** `cloudflows/` (its `Executor` trait and `job::JobSpec`) appeared while this was written; `wiring::spec_from_flow_job` turns a `JobSpec` into a sandbox request (secret_env refused), and the scheduler is not switched over. The adapters use `iyakki_flow::host::{ExecRequest, ExecResult, BrowserRequest,
  BrowserResult}`; `sandbox::job::job_for` / `bundle` is the Job builder to reuse. See the report for the exact seam.
* **Browser pod** needs `iyakki` and the WPE helper in the image and a headless path; not tried.
* **Namespace tier** was never run: Kubernetes seccomp/AppArmor, `hostUsers`, and `linuxapps_init` itself are untried on a device.
