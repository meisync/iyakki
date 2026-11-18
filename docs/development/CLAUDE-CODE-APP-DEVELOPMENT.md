# Building IYAKKI apps with Claude Code

**For:** Claude Code (or any coding agent) that writes apps, skills, tools and services for IYAKKI, either on a PC with the `iyakki` program
or inside IYAKKI's own IDE.
**You will get:** which kind of app to build, the loop for each kind, the rules every kind follows, how a service goes into k3s today,
and the design for an app that **ships its own k3s deployment and installs through the wizard with its dependencies** (not built yet; section 6).
The SDK that apps will use to reach OS features (sound, markdown, speech, LLM, dashboards, flows, k3s, embedding, usage, remote debug loop) is specified in `docs/development/SDK-API.md` (design, not built).
Reference: `docs/APP-AUTHORING.md` (the authoritative loop and bridge), `docs/STORE.md` (packages), `docs/DEVELOPER-MODE.md` (installing on a device),
`docs/PLATFORM-SERVICES.md` (the deploy wizard), `docs/K3S-DEV.md`, `dist/skills/k3s-*.md` and `dist/skills/iyakki-*.md` (the skills the in-OS assistant uses).

## 1. Choose the kind first

| You are asked for | Kind | Runs as | k3s? |
|---|---|---|---|
| "an app for IYAKKI", a calculator, notes, a form, a small tool with a screen | **tile** | HTML/CSS/JS in the launcher on IYAKKI's page engine, sandboxed, its own origin | **no** (no server, no Dockerfile) |
| teach the assistant a method | **skill** | Markdown (body at most 3950 characters, ASCII, at most 12 triggers) | no |
| give the assistant a tool that talks to a program or API | **mcp** (python3 or node, standard library only) or **tool** (WASM, sandboxed with budgets) | process / sandbox on the device | no |
| a web service, API, database, worker | **service** (IDE target `k3s-service`) | container in the person's k3s | **yes** |
| extend DASH, TALES, FLOWS, the IDE or chat; look and feel | extension, template, theme, agent | JSON the target tile reads | no |

Words like *k3s, kubernetes, docker, deploy, service, api* mean a service; *tile, app in the OS, launcher* mean a tile; *mcp, skill, wasm* mean those.
Both families of words, or neither: ask **one** question. An IDE workspace keeps its choice in `.iyakki/project.json` (`target`); a tile workspace refuses deploy tools and the
other way round. Do not mix: a service is not a tile, but a project may have **both** (a service plus a tile that is its front end, section 5).

## 2. Rules every kind follows

* Never embed secrets (read them from the vault or the environment). Least permissions, declared in plain words. Validate every input against a schema. No shell, no `eval`.
  A time limit and a size limit on every call. Only protocol messages on an MCP server's stdout. A test for every tool and every refusal.
* Licence from the permissive list only. Versions only go up. Add a `CHANGELOG.md` entry per release. Never commit signing keys.
* Everything that changes something outside the app (an install, a deploy, a delete) is a **plan the person reads and approves**. Never skip that step, never do it silently.
* Be honest in the README and in your report about what was tested and what was not.

## 3. Tile apps

1. Scaffold: IDE `⋯` -> New app (Ctrl+Shift+N) or the assistant's `tile_scaffold {name}`. Files: `index.html` (required), `app.js`, `style.css`, `app.json`, `iyakki-app.json`,
   `README.md`, `tests/app.test.js`. At most 64 files, 512 KB each, 1.5 MB in all.
2. `app.json`: `id`, `name`, `version`, `icon`, `group`, `capabilities` (`storage`, `network: [hosts]`, `clipboard`, `notifications`, `files`, `agent`). Everything is off unless declared; `vault` and `js-eval` are refused.
3. The page engine draws text, headings, lists, tables, boxes, CSS colours and sizes, simple flex rows and real form controls. **No canvas; plain elements take no clicks: use `<button>`, `<input>`,
   `<select>`, `<textarea>`, checkboxes, `<a>`.** `localStorage` only lives while the app is open: keep data with `iyakki.storage`.
4. The bridge `window.iyakki`: `app.info()`, `storage.get/set/remove/list`, `ui.toast/title`, `host.openFile/saveFile`, `clipboard.read/write`, `notify.show`, `agent.ask` (asks every time). Calls without the capability throw.
5. Test headless on the real engine: `tile_test {path}` (files `tests/*.js` with `T.test`, `T.click`, `T.type`, `T.text`, `T.exists`, `T.eq`, `T.ok`). Lint: `tile_validate {path}`.
6. Try on a device from a PC: `iyakki dev push <folder> --watch --open` (pack, install as an unsigned dev app, reload on change); `iyakki dev logs <id> --follow` shows the page console; `iyakki dev screenshot -o shot.png` to look.
   From the IDE: Run (F5), Add tile / `tile_install_dev`.
7. Ship: `iyakki pkg validate .`, `iyakki pkg keygen me` (once; keep `me.key` private), `iyakki pkg pack . --key me.key -o app.iyp`, `iyakki pkg verify app.iyp`, then `iyakki pkg publish --server URL --token-env STORE_TOKEN app.iyp`
   (or `store_publish`, which always asks). Samples to copy from: `dist/samples/tiles/` (calculator, notes, converter).

## 4. Skills, MCP servers, WASM tools

* `app_scaffold {kind, name, dir}` writes a working project with its own test (`tests/test_skill.py`, `test_server.py` / `test_server.js`, `check_tool.py`). Edit small, run the test, `store_validate`, pack, install from a file
  (STORE -> Settings -> Install from a file) or `iyakki dev install <folder>`.
* An MCP package carries the server source and works on any device that has the runtime; keep dependencies to the standard library.
* `iyakki-app.json` (project root) names `kind`, `name`, `version`, `license`, `title`, `summary` (140 characters), `permissions` (`read:/abs/path`, `write:/abs/path`, `net:host`, `vault:name`, `run:program`; wide ones like `net:*` or `read:/` are refused),
  `entry`, and `runtime` for an MCP server.

## 5. Services on k3s (what works today)

The cluster is already connected in an IDE workspace (the dev cluster `docs/K3S-DEV.md`, or one the person picked). Never start, install or stop a cluster, never `kubectl delete namespace`. The in-OS skill
`dist/skills/k3s-dev-best-practices.md` is the checklist; the short form:

1. **Look first** (`k8s_status`, `k8s_get`, `k8s_describe`, events) and say which cluster (local or remote). Track the work as rows on the feature board.
2. **Layout:** one folder with the Dockerfile and app files, manifests in `deploy/`, tests in `tests/`.
3. **Dockerfile multi-stage:** pinned small base (never `:latest`), dependencies first, `.dockerignore`, a `test` stage that runs the unit tests (the build fails if they fail), a minimal non-root runtime stage.
4. **Unit tests in a container before anything ships:** `deploy_build target=test`; record counts and coverage; fix until green.
5. **App:** listen on `0.0.0.0`, port from an env var, logs to stdout, `GET /healthz`, handle SIGTERM, expose `/metrics` if it has numbers worth graphing.
6. **Build, load, deploy:** `deploy_build` (tag = content hash it returns), `deploy_load` (use exactly the image name it returns), then a Deployment plus Service with `requests`/`limits`, readiness and liveness probes,
   `runAsNonRoot`, no privileged, no `hostPath`, no `hostNetwork`, no added capabilities. No `Namespace` manifest: the workspace namespace (`<workspace>-dev`) exists. `k8s_apply`; fix linter findings yourself.
7. **Integration test after rollout:** build `tests/integration` as its own image and run `deploy_run_job`; verify `deploy_status` (rollout complete, Ready, restarts 0) and a real request to the "reachable at" address (a ClusterIP is internal only).
8. **Failures:** `bug_report`, then events; change one thing, new tag, retry; stop after three failed attempts and report.
9. **Metrics:** the local k3s runs metrics-server (current CPU and memory in the Kubernetes tile). For history, deploy Prometheus as a normal app in the workspace namespace (NodePort 9090,
   a ConfigMap with `scrape_configs`) and add it in DASH as a `prometheus` source at `http://<machine>:<nodePort>`.
10. **Teardown only when told:** `k8s_stop_env`, `k8s_teardown`, `k8s_registry_cleanup`.

A service can have a **front end tile** in the same project: the tile declares `network: ["<service host>"]` and talks to the service over the cluster address the deploy reports. Package them as two artefacts
today (the tile as a store package, the service as the manifests plus README and a deploy plan).

## 6. Design: an app that ships its k3s deployment and installs through the wizard (NOT BUILT)

**What exists.** The *platform services* engine (`crates/iyakki-agent/src/platform/`) already gives the wizard the owner wants, for services written in Rust (`ServiceDef`: Gitea, the private store):
1 choose the cluster (badges, refused ones listed with the reason), 2 options made from the service's schema (namespace, version, storage, exposure, admin user, image mirror, own options),
3 review (the exact commands, images, storage, exposure, credentials it creates by name, warnings, the full manifest, an example NetworkPolicy that is *not* applied), 4 approve, 5 live progress,
6 result with addresses and a masked one-time reveal of generated secrets. Capacity precheck, a guard against production-looking clusters, secrets in the vault via `SecretSpec`, labels `iyakki.dev/service` and `iyakki.dev/instance`.
Adding a service today means implementing `platform::ServiceDef`, `platform::register(...)` and tests (`docs/PLATFORM-SERVICES.md`, "Adding a service").
What does **not** exist: a store package kind that carries manifests, so an app author cannot ship a deployment as data.

**Proposal (for the owner to approve before anyone builds it).** A package kind `service` (a bundle, `.iyp`, signed like the others) with these parts:

```json
{"kind":"service","name":"shop","version":"1.2.0","license":"Apache-2.0","title":"Shop","summary":"...",
 "permissions":["net:registry.example.com"],
 "deploy":{
   "manifests":["deploy/app.yaml","deploy/db.yaml"],            // plain Kubernetes YAML, namespaced objects, no Namespace/ClusterRole/CRD unless declared
   "options":{"storage_gi":{"type":"int","default":5,"min":1,"max":200},"expose":{"type":"enum","values":["ClusterIP","NodePort"],"default":"ClusterIP"}},
   "images":[{"name":"shop","source":"build","context":"image/"},{"name":"postgres:16.4","source":"pull","digest":"sha256:..."}],
   "secrets":[{"name":"db-password","generate":"random-24","mount":"DB_PASSWORD"}],
   "requires":[{"service":"postgres","version":">=15"},{"package":"tile:shop-ui","optional":true}],
   "ports":[{"name":"http","port":8080,"expose":true}],
   "health":{"path":"/healthz","port":"http"},
   "storage":[{"name":"data","gi":"{{storage_gi}}","backup":true}],
   "hooks":{"upgrade":"deploy/upgrade.yaml","backup":"deploy/backup.yaml"}}}
```

How the install wizard would treat it (reuse the platform engine's wizard, plan and approval; do not invent a second one):

1. **Resolve dependencies** into an ordered graph. A `requires` entry is satisfied by an installed platform service or package, a service the wizard can deploy (offered in the same plan), or fails with a plain sentence.
   Cycles are refused; versions compared; the plan lists every item that will be installed, in order, with the total storage and memory (capacity precheck).
2. **Review one combined plan:** cluster, namespace per instance (`<service>-<instance>`), every image (source, digest, size), every object (kind/name), exposure, storage, generated secrets (names only), NetworkPolicy suggestion, what each dependency adds.
3. **Static guard before approval** (refuse, do not warn): privileged containers, `hostPath`, `hostNetwork`, added capabilities, cluster-scoped objects not declared by a trusted publisher, `:latest`, images without digest from non-built sources,
   Secrets with literal values, a `Namespace` object, anything that mounts the container runtime socket. The same lint the IDE's deploy tools use.
4. **Apply in order with readiness gates** (dependency ready before the dependent starts), progress streamed, rollback of this install if a gate fails; nothing outside the plan is touched.
5. **Lifecycle:** upgrade (same wizard, shows the diff, keeps data), backup/restore hooks, stop/start, remove (asks whether to keep the volumes), and the installed instance appears in Settings -> Dev cluster -> Platform services and in the Kubernetes tile.
6. **A tile front end** is just a `requires` with `package: tile:...`; it is installed in the same plan and gets the service address.
7. **Trust:** a service package from a verified publisher can be offered with fewer prompts; an unsigned dev bundle is marked and always shows the full manifest. Everything is audited with the cluster name.

Open decisions for the owner: where the manifest guard's allow-list lives; whether images may be pulled from the internet at install (mirror option exists in the platform engine) or must be built on the device; whether
dependency services other than the built-ins (a Postgres, a Redis) are shipped as store packages of this same kind (recommended: yes, then "requires" is uniform).

**Until it is built,** a deployable app is delivered as: the service folder (Dockerfile, `deploy/`, tests), a README with the plan in words, and, if a one-click experience is needed now, a Rust `ServiceDef` registered in the
platform engine (reviewed code, not data). Say which route you took and why.

## 7. Working agreements

1. Start from the nearest sample or scaffold; keep the app small; least permissions.
2. Test before you claim: tile tests on the engine, MCP tests over pipes, container unit tests, integration job in the cluster. Report counts, not adjectives.
3. Use `iyakki dev` for the device loop (`push --watch`, `logs --follow`, `screenshot`); ask the person before anything that changes the device beyond the app.
4. Update `README.md`, `CHANGELOG.md` and the version; run `iyakki pkg validate`.
5. When a request is ambiguous about tile versus service, ask one question; when the person wants a one-click multi-part install, explain section 6 honestly (what works today, what is proposed).
