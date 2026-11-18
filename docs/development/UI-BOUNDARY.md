# The UI boundary: `iyakki-ui` and the core (alpha.4 item 0, stages c to e)

Status: design plus the build side, 2026-11-15. Branch `alpha.4/ui-boundary`. Owner condition for cutting alpha.4: the split is done, the build is
layered, an SDK for UI exists. This page says what is built, what is only drawn here, and the smallest honest next cut. Related:
ALPHA-4-PLAN.md item 0, [FAST-LOOP.md](FAST-LOOP.md), [LAYERED-ROOT-DESIGN.md](LAYERED-ROOT-DESIGN.md),
[COMPONENT-UPDATES-PLAN.md](COMPONENT-UPDATES-PLAN.md), [UI-OS-SPLIT-PLAN.md](UI-OS-SPLIT-PLAN.md), [CORE-CONTRACT.md](CORE-CONTRACT.md), [BOUNDARY.md](../../BOUNDARY.md).

## 1. Where things are today (measured by reading the code)

* One program, `/usr/bin/iyakki`, is PID 1. `start_shell_thread()` in `crates/iyakki-core/src/main.rs` runs the shell (and with it every app) as a **thread**
  of that program, kept alive by `keepalive::run_kept`. The shell and the agent share one in-memory `ShellEnv` / `Services` (`shell_env_with`, `SHARED_ENV`).
* `iyakki-apps` (stage a, done) holds the apps; the shell depends on it. It calls `iyakki_agent::` 3,589 times in 354 files: the apps use the agent's
  services directly, in the same address space. Only the migrated tiles (`tests/data/migrated_tiles.list`, `iyakki-tile-*` crates) go through a port trait.
* The layered pack has three layers (`dist/buildroot/layers.txt`): `base` (`provides base_abi=1`), `core` (`provides core_iface=1`, needs `base_abi 1..1`),
  `ui` (needs `core_iface 1..1`, `base_abi 1..1`). Today the `ui` layer holds fonts, icons, samples, theme and the web-view helper only. The program is in `core`,
  so **an apps change changes the `core` layer** (a rebuilt program is about 38 MB; LAYERED-ROOT-DESIGN.md) and the `ui` layer does not move.

## 2. What runs where, after the split

| Program | Layer | Holds | Does not hold |
|---|---|---|---|
| `iyakki` (core, PID 1) | `core` | boot, services, the agent, vault, approvals, the broker, device access, the developer-mode server, the core API server | the shell, any app, any drawing code |
| `iyakki-ui` | `ui` (`usr/bin/iyakki-ui`, `usr/libexec/iyakki-ui/**`) | the shell, the apps, the tile host, uikit, the in-house web engine's drawing side | `/proc`, `/sys`, `/dev` readers, process spawning, key material, direct service state |

`iyakki-ui` is started and supervised by the core (the way the shell thread is today, with the same crash policy: restart on the same session, rebuild after
repeated crashes). A restart of `iyakki-ui` does not restart the machine, the services or a running flow. The core stays up when the UI is replaced: that
is what makes `push-apps` seconds instead of a reboot.

## 3. The API surface the UI is allowed to use

Everything goes through the core (owner rule 2026-11-14): the UI asks, the core checks capabilities and approvals, logs, then acts.

| Surface | Number | Where | Rule |
|---|---|---|---|
| Layer interface core to UI | `core_iface` = 1 (in `layers.txt`, `provides` of `core`, `requires` of `ui` as `1..1`) | the layer manifest, checked by the first stage and by the update code before anything is written | raise it only when the UI-visible surface breaks; the UI layer then ships with the core layer |
| Libraries and ABI under both | `base_abi` = 1 | the same | a base library change that breaks a program |
| Typed core API | contract 0.2 (`CONTRACT_VERSION`, `crates/iyakki-core-api/src/contract/hello.rs`; minors only add; `OLDEST_MINOR` 1) | `iyakki-core-api`, `hello` and `negotiate` first | domains, calls, events, streams; transports `in_process` today, `dev_mode` and `relay` later, `local_socket` for `iyakki-ui` |
| Tile ABI | `iyakki-tile-abi` / `iyakki-tile-api` | tile apps, the SDK | a tile sees only `iyakki-uikit` plus the tile API |

The UI never sees `Services`. What it sees is the contract's domains (settings, vault, flows, consent, devices, audio, sysinfo, router, bundle, wizard ...) as
typed calls and event streams. The transport for the process boundary is the same contract over a Unix socket in `/run/iyakki/` owned by the core, with the
caller identity set by the core (the socket's peer credentials), never by the UI: a UI cannot make itself look like another caller.

## 4. What tests enforce today (and what they do not)

| Test | What it holds | State |
|---|---|---|
| `iyakki-core/tests/boundary.rs` (+ `BOUNDARY.md`) | process spawns and system-level libc calls only in listed files | green, applies to every crate |
| `iyakki-core/tests/dependency_ratchet.rs` | apps do not read `/proc`, `/sys`, `/dev` or use the hardware crates; list `shell_kernel_access.allow`: 5 uses in 3 files; tile crates depend on `iyakki-uikit` only (1 listed exception); migrated tiles may not name `Services` | green, only shrinks |
| `iyakki-core/tests/kernel_single_reader.rs` | one reader per kernel fact; 80 listed (path, file) pairs | green, only shrinks |
| `test-affected.py` + `crate-layers.txt` | which crates a change can break, by layer | green |
| NOT enforced yet | that `iyakki-apps` / `iyakki-shell` do not call `iyakki_agent::` (3,589 call sites); that no app links `iyakki-agent` | needs a ratchet over `iyakki_agent::` uses, shrinking per slice |

## 5. The smallest honest first cut

The process split cannot be done by moving a `main`: 354 files call the agent in-process and the shell shares one `Services` with the core. Options considered:

1. **`iyakki-ui` links the agent and builds its own `Services`.** Quick to compile, wrong: two owners of the settings file, the vault, the approval queue and the device
   handles. Rejected.
2. **Move the 3,589 call sites to the contract first, per slice, then cut the process.** Right, slow. Each slice (a domain: Settings, Store, Browser, IDE, Chat) ends green
   by itself and ratchets the `iyakki_agent::` count in the apps down. This is the CORE-ACCESS-MODEL slice list, now with a number to watch.
3. **Cut at the tile boundary first:** tiles that already speak only `tile-api` (the migrated list and the `iyakki-tile-*` crates) run in `iyakki-ui`; the rest of the
   apps stay in the core's shell thread until their domain is migrated. Needs the drawing path between the processes (display-list mode, CLIENT-SPLIT-DESIGN.md).

The cut of this round is therefore on the **build and deploy side** only (what the owner's goal is about), and the process split is staged as 2 then 3:

* Done now: layer rules reserve the `ui` layer for `usr/bin/iyakki-ui` and `usr/libexec/iyakki-ui/**` (a test builds two roots that differ only there and shows that only
  the `ui` layer changes; section 6); `iyakki dev push-apps` ships one program file as a binary delta and tells the truth about the restart.
* Not done: a program called `iyakki-ui`, the `local_socket` transport, the `iyakki_agent::` ratchet, a restart of the UI without the device.
* Until `iyakki-ui` exists, an apps change still produces a changed `core` layer (the one program), and applying it needs the device to restart. `push-apps` makes that
  path as short as it can be (delta of a few MB instead of the program, no image build, no layers rebuild) and says so in its output.

## 6. Build side

* `dist/buildroot/layers.txt`: two rules put `usr/bin/iyakki-ui` and `usr/libexec/iyakki-ui/**` in `ui`, before the `core` rules (first match wins).
* `dist/buildroot/test-ui-layer.sh`: splits two tiny roots with `layers.py` that differ only in `usr/bin/iyakki-ui`: the `ui` layer file name (its hash) changes, `base` and
  `core` are byte-identical; and a root that differs only in `usr/bin/iyakki` changes only `core`. Runs without Buildroot, QEMU or a device (needs `mksquashfs`).
* Pack: `iyakki dev update-pack --against PREV` already carries only the layers whose hash changed (`pack_layers.rs`); `quick-push.sh --layers --against` passes it. With
  the `ui` rule an apps-only change gives a pack of one layer. The signed full-pack path is not touched.
* `iyakki dev push-apps`: below.

## 7. `iyakki dev push-apps`

```
iyakki dev push-apps --bin FILE [--target /usr/bin/iyakki] [--against OLDFILE] [--shell]
```

Makes a hot-patch pack with one file (`FILE` at `--target`), as a binary delta against the program that was pushed last (kept in the developer home under `push-apps/`; the
first push, or `--against`, decides), sends it with the existing patch route, and applies it for the next start. It asks nothing of the PC's own: **the device shows the approval
card and the person at the device answers it** (the PC only waits, as for every other `dev` command). It never reboots. When the target is the core program it says that
the device must restart for the new program (the shell is a thread of it); when the target is `/usr/bin/iyakki-ui` (after the process split) it applies live and the core restarts the UI.
Measured numbers: [FAST-LOOP.md](FAST-LOOP.md), "Deploy path".

## 8. The SDK for UI

What exists: `iyakki-tile-abi` / `iyakki-tile-api` / `iyakki-tile-kit`, `iyakki-sdk-emulator` (a window that emulates the device), `iyakki dev push DIR --watch` for tile apps
(live reload, no image). What the owner's condition needs on top, in this order: (1) the contract client crate for a UI written outside the repository (`iyakki-core-api` plus a
transport), (2) a sample UI program that uses only that client and runs against `iyakki-core-fake`, (3) the `iyakki-ui` skeleton the core supervises. (1) and (2) are separate rows
in FEATURES.md and are not built.

## 9. The ratchet and the first slice (alpha.5, branch `alpha.5/ui-program`)

**Measured, non-test code only** (test files, `#[cfg(test)]` modules and comments left out; one use is one `iyakki_agent::path` or one item of a `{...}` group). The 3,589 above
counted tests and every line; the figure to watch is now **2,436 direct uses before the first slice, 2,433 after it** in `iyakki-apps` and `iyakki-shell` (the full table by domain, agent module and file is
[UI-AGENT-USES.txt](UI-AGENT-USES.txt), regenerated by `IYAKKI_UI_USES_WRITE=1 cargo test -p iyakki-core --test ui_agent_uses`). The test
`crates/iyakki-core/tests/ui_agent_uses.rs` fails when a domain or an agent module grows, when a module that is not listed appears, or when the numbers are lower than the allow list
[ui-agent-uses.allow](ui-agent-uses.allow) says (lower it then, so it cannot grow back).

**First slice done: the mixer** (`iyakki_agent::mixer`, 4 uses, now 0). The contract is `iyakki_core_api::Core` (`crates/iyakki-core-api/src/uicore.rs`) with the facet
`Core::mixer()`: `stats()`, `active_sources()`, `play_wav()`. The apps and the shell hold it as `cx.core` / `Shell::core`. Real implementation
`iyakki_agent::corehandle::AgentCore` (built once in `Shell::new`, the composition root; this is the one new direct use, counted under `other`), fake `iyakki_core_fake::FakeCore`.
The Settings Sound page, the volume bar's list of sounds and the check-up tone no longer name the agent's mixer. Existing tests run on the real handle (unchanged
behaviour); a new test drives the Sound page from the fake.

**What is next** (domain by domain, smallest contract first): the rest of `sound` (`audioout` settings types and `syssounds` cues, 54 uses; needs the sound settings as contract types), `input`
(22), `display` (95), then `cluster`, `files`, `store`, `settings`, `chat`; the large `other` group (about 850) needs sorting into domains first. After every domain has a contract
facet, the process cut over a local socket (stage 2/3 above). Nothing here starts a separate process yet.

## 10. The real core serves the transport (F-SPLIT-07, alpha.5, branch `alpha.5/q-followups`)

`iyakki_agent::contract_local::uiserve` serves the line transport of `iyakki-ui-client` from the REAL core (the adapters of `Core`: settings, vault, flows of one
account, the real catalog `hello`), to a UI program on the same machine over a **unix socket** (`/run/iyakki/ui/ui.sock`, folder 0700, socket 0600; no TCP port). It is
**off by default**: the switch and the grants are the file `ui-socket.json` in the data folder, changed only by an administrator through the chat tools `ui_socket_status` (free) and
`ui_socket_set` (asks every time; switching on is a hard ask that Auto mode never approves). The core's start-up (`start_devmode_watch`) runs `uiserve::watch`, which follows the file
every two seconds: on starts the listener, off or a changed grant closes it and removes the socket file.

The caller is the LISTENER's, never the request's: `CallContext::remote(account, Transport::LocalSocket, scopes)` (a new transport value, never local), so a local-only call
(`vault.unlock`, the answer to a consent question) is refused like for any remote caller and is not on the transport at all. The grantable scopes are those of developer mode minus
`consent:read`. The audit lines are the adapters' own ("account via uisocket: call [risk] outcome", no values). `iyakki_ui_client::server::serve_unix` and
`client::Connection::unix` are the two ends.

Not done (still F-SPLIT-07): the supervised `iyakki-ui` program (F-SPLIT-03 is the ratchet on the way there), pushed events (a connection gets `settings.watch` and `flows.events` as
calls, not as unsolicited lines), binary chunks (`streams` over this transport), a pairing step for the socket (the file mode is the only door today), and a Settings page switch
(the chat tools are the only way to turn it on).

## 11. The scripted split (alpha.5, lane q-split-core)

The `iyakki_agent::` ratchet is worked down by scripts under `dist/split/` instead of by hand:

* `dist/split/move_types.py MODULE...` (pass 1): for each flat agent module it finds the top-level `struct`, `enum`, `const`, `type` items and their `impl`
  blocks, keeps the ones the UI names plus what they need (a fixpoint: nothing may use a `crate::` path outside `iyakki-shared`, call a function of the
  file, or use an agent-only type), moves them into `crates/iyakki-shared/src/MODULE.rs` (fields and inherent methods become `pub`), leaves
  `pub use iyakki_shared::MODULE::{...};` in the agent so nothing else changes, and rewrites the UI paths. `--dry` shows the plan, `--undo` takes a module back.
* `dist/split/pass1.py MODULE...` runs it, compiles `iyakki-shared`, and for each error blames the moved item that holds the line: it goes into
  `dist/split/excluded.txt` (it stays in the agent) and its module is moved again without it, until the crate compiles. Set `RCARGO` to the cargo front end
  (`RCARGO="path/to/rcargo $PWD"`). Then `cargo check --workspace --tests`, regenerate `UI-AGENT-USES.txt` and lower `ui-agent-uses.allow`.
* Result of the first run: 500 items of 120+ modules, 1,972 -> 1,596 uses. What is left (about 880 function and method uses, 490 uses of stateful
  types such as `Tools`, `Services`, `Runner`, `History`, `Ledger`) needs facets or a facade (pass 2): the functions cannot move, the UI has to ask the core.

## 12. Sound and cameras (alpha.5, lane q-split-sound-cam)

Two more `Core` facets, written by hand (`iyakki-core-api::soundctl`): `Core::sound()` (`SoundControl`: the output selector's state, the person's sound settings, the
notices of the output watcher, the outputs found under a `/proc` tree, the card that starts slowly) and `Core::cues()` (`CuePlayer`: the sign-in, sign-out, notification
and job-done sounds). Reading only: a change of the volume or the output still goes through the settings. Real adapter `AgentCore`, fakes `FakeSound` and `FakeCues`
(`iyakki-core-fake::sound`), both with a "no sound" default so a core without sound answers quietly. The data types they speak (`SoundSettings`, `Output`, `Choice`,
`choose`, `Cue`, `Outcome`, `CueHandle`, `CueOut`, `Master`) live in `iyakki-core-api::soundtypes`; `iyakki_shared::audioout` and the agent's `audioout` and `syssounds`
re-export them, so the old paths still work. The Devices popover (`Probe::core`), the Sound and Keyboard pages, the volume keys, the top bar and the shell's cues use the
facets; a test that builds a made-up `/proc/asound` gives its fake core a scanner (`FakeSound::set_scanner`).

Cameras: the CAMERAS tile's source (the hub, the recorder and the motion watcher for one person) is now `iyakki_agent::camsource::HubSource` (it was `ShellCameras` in
the app), and the red sign's names, the click that stops it and the person-to-hub rights are in the same module. What is still in the app: building the hub (this
computer's USB cameras, the picture encoder, the audit sink), `clientmedia` and the assistant's camera tools.

## 13. Bucket b0 of the module split (alpha.5, branch `alpha.5/q-split-b0`)

Twenty modules with four or more UI uses were cleared or cut down by hand, with no seam file (a module that only re-exports the agent for the UI does not count as cleared):

* **Moved to `iyakki-shared`, the old path re-exported by the agent:** `providers` (`Kind`, `ProviderCfg`, the name checks), `target` (`Target`, `ChangePlan`, read and write of the project's target), `gpu` (the whole detection, report and test-prompt parser; only `save_last_test` stays), `quiet` (`Env`, `ASOUND`), `tzdetect` (`apply`, `due`), `pkg` (`parse`, `Review`, `Package`, `Manifest`, the signature checks), `fwpack` and `fwextra` (the licence, outcome and plan logic; only the download and the real environment stay), `media` (`MediaStore`, the folder scan and the queue state), `mcp` (`McpClient`, the config file), `inputprofile` (all of it), `checkup` (all steps; only `Inventory::read`, now `read_inventory`, the tone and `start_mic` stay), `quickinstall` (`Item`, `Group`, `Latest`, `Facts`, `order`, `resources`, `ollama_cluster` choices, the uninstall plan), `idebrowser::OpenRequest`, `ocipush::RegistryHttp`.
* **Facets of `iyakki-agent-api::facets_b0`** (`cx.agent.b0()` for calls that need only their arguments, `cx.services.b0()` for calls over the person's services; real implementation `iyakki-agent/src/facets_b0.rs`, fakes are the trait defaults and `NoB0`): `providers` (thinking sink), `quiet` (the system environment), `target` (plan and apply a change), `idebrowser`, `ocipush` (the registry connection), `linuxapps` (the workspace sandbox), `media` (the shared player as `MediaPlay`), `gemini` (the live voice settings), `mcp`, `checkup`, `quickinstall` (jobs, presence, start, upgrades, facts, uninstall, firmware and test jobs, ollama host), `sharedfolder` (the tools and the restore), `disksformattools` (the tools).
* **Still counted here:** `gitkeys` (`GitKeys` needs `sshkey` and the vault), `disksformattools` (`Req`, `to_op`: `disks` types), `linuxapps` (`LinuxApps`), `gemini` (the settings page), `sharedfolder::Shares`, `checkup::start_mic`, `quickinstall::{ide_git offer, platform_services}`.
