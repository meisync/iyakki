# IYAKKI architecture in one page

**For:** developers and reviewers who want the shape of the system before reading code.
**You will get:** a diagram, the crate map, the boot flow, the process model, the security model, how an assistant run flows,
the seams, how images are built, and where to extend.

IYAKKI is one static Rust program (`iyakki`) plus a set of apps (tiles). It is not a Linux fork: it runs on a stock kernel and a few
ordinary programs through four listed interfaces ([`../BOUNDARY.md`](../BOUNDARY.md)). Deep dives: [`DESIGN.md`](DESIGN.md),
[`EMBEDDING-AND-BACKEND.md`](EMBEDDING-AND-BACKEND.md), and the files in [`INDEX.md`](INDEX.md).

```
  person (keyboard, mouse, touch, voice)
        |
        v
  +-------------------- front ends: draw pixels, send events ----------------------+
  |  framebuffer (iyakki shell)   desktop window (iyakki-gui)   browser (iyakki-remote, HTTPS+WS)  |
  +---------------------------------+-----------------------------------------------+
                                    v
  iyakki-shell   Shell: login, launcher, 24 tiles (apps/), Gfx drawing, input
        |  AppCx / Services (shared handles)
        v
  iyakki-agent   models, tools, approvals, RAG, MCP, skills, store, flows, dash, k8s, browser agent, voice, vault
        |                |                     |                    |
        v                v                     v                    v
  iyakki-sec       iyakki-wasm           iyakki-proxy       iyakki-web / -js / -webview
  roles, policy,   wasmtime sandbox:     egress allow-list,  built-in browser engine,
  capability       fuel + memory caps    key injection,      QuickJS in WASM, helper
  tokens           capability host calls response cleaning   process for full WebKit
        |
  iyakki-core   PID 1: mounts, driver coldplug, network, supervisor, CLI (`iyakki ...`)
  ================ boundary: kernel ABI / named programs / cmdline / our files ================
  Linux + BusyBox + GRUB (Buildroot)       or any Linux distribution, or a container
```

## Crate map

| Crate | One line |
|---|---|
| `iyakki-core` | The `iyakki` binary: PID 1 boot, supervisor, CLI, browser-test runner |
| `iyakki-shell` | Login, launcher, every tile, drawing (`Gfx`, `fx.rs` design system), input; renders to a framebuffer, a window or PNG |
| `iyakki-agent` | Everything behind the tiles: providers, agent loop and tools, approvals, RAG, MCP, skills, store, flows host, dashboards, k8s/deploy, browser agent, voice, vault, settings |
| `iyakki-sec` | Roles and policy, revocable capability tokens, entropy, Wi-Fi key derivation |
| `iyakki-wasm` | wasmtime sandbox with fuel and memory caps, snapshots, speculative branches, replay recording |
| `iyakki-ledger` | Hash-chained execution ledger with time travel |
| `iyakki-vfs` | In-memory filesystem with quota, fork and diff |
| `iyakki-mem` | Tiered memory (registers, scratchpad, vector store) with paging |
| `iyakki-hw` | Microphone and camera bridges into WASM guests |
| `iyakki-edit` | Editor engine (buffers, syntax, diff), no dependencies |
| `iyakki-ui` | Headless editor model, with an optional egui editor binary |
| `iyakki-web` | Browser engine: HTML, CSS, layout, images, fetch |
| `iyakki-js` | JavaScript platform for pages (QuickJS-ng in WebAssembly) |
| `iyakki-webview` | Shell side of the optional full web engine helper (socket protocol, shared-memory frames) |
| `iyakki-proxy` | Egress proxy: host allow-list, credential injection, response sanitising, HTTPS transport |
| `iyakki-remote` | The desktop in a browser: HTTPS + WebSocket server, sessions, streamed canvas |
| `iyakki-netscan` | Local network scanner (models, cameras, media, devices) |
| `iyakki-storage` | Finds the IYAKKI-DATA volume, lists disks, makes a disk persistent, virtual disks for window mode |
| `iyakki-devctl` | Settings controls: links, Wi-Fi, sound, brightness, radios |
| `iyakki-hwinfo` | Hardware report for the boot hardware check |
| `iyakki-swarm` | libp2p + mDNS peers that run WASM tasks with no capabilities |
| `iyakki-data` | Table service for DATA: CSV/TSV/JSONL, pipelines, built-in and Polars engines |
| `iyakki-flow` | Workflow engine for FLOWS: model, scheduler, retries, healing, versions; no UI or network |
| `iyakki-drivers` | Driver packs: detection, known-driver table, firmware placement, kernel-module checks |
| `iyakki-zram` | Memory tuning: zram swap, optional swap file, figures for SYSTEM ([`ZRAM-AND-INSTALL.md`](ZRAM-AND-INSTALL.md)) |
| `iyakki-relcheck` | Release checks (links, secrets, version, features, SBOM, licences) run by `./run.sh alpha-check` |
| `iyakki-usb` | Host tool: writes an image to a USB stick safely (Linux, macOS) |
| `iyakki-gui` | Desktop window `iyakki-desktop` (eframe); a separate Cargo workspace |

## Boot flow (bare metal or VM)

1. GRUB loads the kernel and the initramfs (`dist/buildroot/board/iyakki/grub.cfg`); the kernel starts `iyakki` as PID 1.
2. `iyakki-core/src/boot.rs`: boot screen, mount `/proc`, `/sys`, `/dev`, device nodes (`mdev`), core modules, **driver coldplug**
   by modalias, storage wait, mount the **IYAKKI-DATA** volume (`mount_data_volume`; formats the marked partition on first boot).
3. `late_boot`: driver packs' firmware is linked back, remaining drivers load, network comes up from `/etc/iyakki/net.conf`, the
   optional SSH server starts when `iyakki.ssh=1`.
4. The supervisor starts; the shell takes the framebuffer (`iyakki shell`) and shows setup or sign-in; with `iyakki.web=1` the browser
   front end also listens.
5. Sign-in unlocks the person's vault, starts their schedules, seeds samples (admins) and shows the launcher.

Power-on to the sign-in screen is about 1.6 s in QEMU (`dist/buildroot/boot-time.py`). In window mode and containers only the
supervisor/shell parts run; anything that needs a real boot happens only when IYAKKI is PID 1.

## Process model

One process runs the supervisor, the shell, the agent and the tools. Threads: UI (30 Hz, never blocks), a thread per assistant run,
loaders and workers that talk to the UI over channels. Separate processes are only started through the listed seams, with argument
arrays and no shell: base programs (`ip`, `modprobe`, `mke2fs`...), user tools (`kubectl`, `podman`, `git`, vendor CLIs), the
web-engine helper (one per tab), Ollama and k3s when you start them, MCP servers, external coding agents (own process group,
cut-down environment). WASM guests run inside the process under wasmtime.

## Security model (summary)

* **Policy:** role template plus per-user overrides that can only refine ([`ADMIN-GUIDE.md`](ADMIN-GUIDE.md)). Checked for tiles, files
  (scopes, symlinks resolved), network hosts, MCP servers, tools and budgets, on every call, whoever initiates it.
* **Approvals:** reading is free; risky tools pause (Approve / Always / Deny); a short `hard_ask` list always asks, even in Auto and
  Autopilot (`iyakki-agent/src/agent.rs`). Policy refusals (guards) are results, not prompts.
* **Sandboxes:** untrusted code runs in wasmtime with fuel, memory and wall-time caps and capability tokens, or in a limited non-root
  container; the workspace jail confines the AI IDE; flows never use a shell.
* **Vault:** XChaCha20-Poly1305 with Argon2id key slots; settings hold references; credentials are injected by the egress proxy or an
  environment variable and are redacted from logs and from what the model sees.
* **Audit:** hash-chained `audit.log` for people and decisions, separate ledgers for runs, 3-day tool log.
* **Supply chain:** store packages are signed (ed25519), hashed and shown for review; only permissive dependencies, enforced by `cargo deny`.

## Data flow of an assistant run

1. The CHAT (or an IDE/DASH/FLOWS pane) builds the request: system prompt with the tools that exist now, skills that match,
   memory by relevance, project state in a workspace.
2. `ChatBackend` (a provider: Ollama, OpenAI-compatible, Anthropic, Gemini; or an external agent through `extagent`) streams tokens back.
3. A tool call goes to `Tools` (`UserTools`, wrapped by `ProjectTools` in a workspace): repair of malformed calls, policy check,
   `hard_ask`/risk check, approval card in the chat and the INBOX.
4. The tool runs: in process (files, data, calendar), through the proxy (HTTP), in WASM (packages), as a named program (`wsexec`,
   `exec`), or through the browser engine.
5. The result returns to the model (secrets redacted); everything is recorded in the audit log, tool log, ledger and USAGE.

## Seams

* **Engine / host:** `iyakki-flow` and `iyakki-data` are UI-free libraries with serde request/response types; `Host`, `DataService`,
  `ChatBackend`, `Kube`, `Runner`, `Fetch`, `HostApi`, `Tools`, `Upstream` are the traits through which the world is reached and
  faked in tests.
* **Service layer (design):** the shell still calls `iyakki-agent` directly; the plan for a remote backend and embeddable
  components is [`EMBEDDING-AND-BACKEND.md`](EMBEDDING-AND-BACKEND.md) (partly built, not yet a stable API).
* **Front ends:** every front end calls `Shell::render` and sends `Event`s, so every tile works in the window, on the framebuffer and
  in the browser.

## How images are built

`./run.sh build` calls `dist/buildroot/build.sh libre`: it downloads Buildroot 2026.02.3, builds the static musl `iyakki` binary
(`cargo build --release --target x86_64-unknown-linux-musl -p iyakki-core`), then Buildroot makes the kernel (stock 6.12 plus
`linux.fragment`), BusyBox and friends, the initramfs, the BIOS+UEFI hybrid ISO and the install image (`genimage-install.cfg`),
and `legal-info/` (sources and licences for GPL components). Variants: `hw` (firmware), `mac`, `web` (WPE WebKit), `ARCH=aarch64`.
`dist/Dockerfile` makes the scratch container image. Tests boot the images in QEMU (`dist/buildroot/qemu-*.sh`).

## Where to extend

| You want to add | Start here |
|---|---|
| A **tile** | `App` enum in `iyakki-sec/src/policy.rs` (id, label, role templates), a module in `iyakki-shell/src/apps/` implementing `App`, the `TILES` entry and the `open` match in `shell.rs`, the group in `launcher.rs`, a row in `FEATURES.md` |
| An assistant **tool** | `iyakki-agent/src/agent.rs` (`Tools`, `hard_ask`), a tools module like `datatools.rs` or `k8stools.rs`, the policy check, tests with a fake backend |
| A **skill** | Markdown with `name`, `description`, `triggers`; ship it in `dist/skills/` and `iyakki-shell/src/samples.rs` (`BUILTIN_SKILLS`), or as a signed package (`iyakki pkg`) |
| An **MCP** server | no code: add it in the MCP tile; code in `iyakki-agent/src/mcp.rs` |
| A **store extension** | `iyakki-agent/src/store/` (`manifest.rs` kinds, `install.rs`), format in [`STORE.md`](STORE.md) |
| A **flow node** | implement `NodeKind` in `iyakki-flow/src/kinds/` and register it in `kinds::builtin()` ([`FLOWS.md`](FLOWS.md#node-sdk-adding-a-kind)) |
| A **dashboard connector** | `iyakki-agent/src/dash/connector.rs` seam, register in `registry.rs` ([`DASHBOARD.md`](DASHBOARD.md#how-to-add-a-connector)) |
| A **base-OS touchpoint** | a row in `BOUNDARY.md` in the same change (enforced by `crates/iyakki-core/tests/boundary.rs`) |
| A **driver pack** | [`MAC-HARDWARE.md`](MAC-HARDWARE.md) section 9, `iyakki-drivers` |
