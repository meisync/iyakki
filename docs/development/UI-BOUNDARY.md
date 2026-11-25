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

## 14. The UI as its own program (F-SPLIT-03, alpha.7, branch `alpha.7/impl-split`)

Decision: the door is a **unix domain socket**, not D-Bus (no bus daemon to ship, the kernel names the peer, a descriptor can travel with a line).

* `crates/iyakki-ui-bin` is the program `iyakki-ui` (`--socket PATH` or `--in-process`). `--in-process` (feature `in-process`) runs the shell with the real services linked in, as `iyakki shell` does: the build and test option for the QEMU and desktop window. `--socket` connects, negotiates the contract and reports; the shell's services are NOT yet served over the socket (the shell still needs a `ServicesHandle` with hundreds of methods), so it ends with status 3 and says so.
* `iyakki_ui_client::ipc`: `peer_of` reads `SO_PEERCRED`, `peer_allowed` lets root and the one UI user in (`server::serve_unix_for` closes any other peer), `make_bulk` / `send_with_fd` / `recv_with_fd` / `read_bulk` hand a block over as a sealed memory file (`memfd_create`, `SCM_RIGHTS`, at most 64 MB) so display lists, audio blocks and camera frames do not travel through the line transport.
* `iyakki_core::uiprocess`: when `/usr/bin/iyakki-ui` exists and `iyakki.ui=process` or `/etc/iyakki/ui-process.enabled` asks for it (the kernel parameter `iyakki.ui=thread` forces the old way), `main` starts it under the helper supervisor (restart with a growing pause, give up after six quick crashes, then the shell runs as a thread of the core so the person is never left without a screen). Off by default. The restart passes `--restore`; the shell's own sealed token still decides whether anything is restored (developer mode only, F-SHELL-65).
* The layered image: `post-build.sh` installs `IYAKKI_UI_BIN` as `/usr/bin/iyakki-ui` when a build made one; `dist/buildroot/layers.txt` already puts it in the ui layer.
* Benchmark: `crates/iyakki-ui-client/tests/bench.rs` (run with `--nocapture`, or `IYAKKI_BENCH_OUT=file`): 2,000 `settings.get` round trips over a real unix socket, and a 32 MB block handed over as a memory file against the same bytes through a plain stream. It asserts correctness only, no time limit. First numbers (Linux build host, 2026-11-18): round trip p50 17 us, p99 24 us; 32 MB block 26 ms by memory file (including making the sealed file and reading it back) and 15 ms by stream copy.

Not done: the shell's services over the contract (so the program can run the shell without the agent linked in and the ui layer shrinks to a few MB), the pushed events and binary stream calls on the transport, a QEMU boot with `iyakki.ui=process`.

`bulk.get` (alpha.7): `iyakki_ui_client::server::serve_unix_bulk` answers it with a `BulkSource`; the block arrives as a sealed memory file passed beside the answer line (`Connection::bulk_get`). The real core serves it (alpha.7, impl-S1): `uiserve::CoreBulk` is the `BulkSource` of its socket, with the block `settings.all` (every setting the listener's scopes may read, through the same adapter as `settings.get`) and an extra source in `Facets::bulk` for a producer to come; a display-list or frame producer is not there (F-SDK-29, which waits for an owner decision on the display-list protocol). The same listener now closes a peer that is neither root nor the socket's owner (`SO_PEERCRED`), and `settings.subscribe` pushes `{"push":"settings","events":[..],"gap":bool}` lines (the point to continue from is taken before the answer, so no change is missed; the machine side looks every 30 ms, the client does not).

Pushed flows and app events (alpha.7, lane implW1, D-SPLIT-07-PUSH): `flows.subscribe` and `appevents.subscribe` start the same kind of push as `settings.subscribe`: `{"push":"flows","events":[..],"gap":bool}` and `{"push":"appevents",...}` lines, through the same guarded read the client could call itself (`flows.events` needs `flows:read`, `appevents.events` needs `events:read`; the starting point is taken before the answer). One subscription per kind per connection, counted in the same limit of 4. Additive: the catalog does not change; `hello` lists `subscribe` for `settings`, `flows` and `appevents` only where the core serves them. Reading app events hands them out, so a subscribed connection is their reader: a second reader of the same person's feed would miss what the push took.

## 14b. `iyakki-ui --socket` runs the whole shell (F-SPLIT-03, F-SPLIT-07, alpha.7 final, group Y3)

`cargo build -p iyakki-ui-bin --features socket-shell` is the program of the ui layer: the launcher and every app, with no agent, no core and no composition root in its dependency tree (`tests/no_agent.rs` checks it; `iyakki-core-local`, the machine facts the UI reads itself, is the one crate of that list that comes with the shell). `socket_shell::environment` connects (waiting for the core's socket for a while, because the core opens it as it starts), checks that the core serves the object calls, and installs what `iyakki_compose::wire()` installs for the in-process program, built from the socket: `RemoteServicesHandle` for the shell's services, `UsersSrc` over the served `AccountsApi` for the accounts, the `AuditApi` root for the audit log, `RemoteAgentApi` for the process-wide agent facade (the three traits the core serves are remote objects, every other trait answers its own quiet default), `SocketCore` for the core handle (settings, vault status, flows, assistant, cameras, and the file scope asked of the core with `ServicesFileScope`), `UiHost` for the host (it names the machine and cannot power it off), and the core's chat runs (`install_chat_runs`: the chat sends a `RunRequest` as data and reads the events of the core's own run through `core_run::remote_run`; the tool chain, the approval rules and the budget are the core's).

What the program does not get, and says so: FLOWS, TALES, LAB, STORE, RADIO and MEDIA need a live object of the core that no quiet answer can stand in for, so the tile opens as `UnservedApp` ("not available in this UI program yet", D-SPLIT-03-UNSERVED); the rest of the services the core does not serve answer their quiet value (D-SPLIT-03-QUIET). The program ends with status 5 when the core stops answering (`CoreWatch`: five misses in a row, two seconds apart) and the supervisor starts it again.

Supervision: `uiprocess::mode` decides (kernel parameter or flag file), `uiserve::hand_over` switches the UI socket on for the program with `PROGRAM_SCOPES` and the objects before it is started (D-SPLIT-03-SOCKET), the supervisor restarts it with a growing pause and gives up after six quick crashes (then the shell runs as a thread of the core), and a program of the first-party kind is told a pace of `FIRST_PARTY_RATE` instead of the catalog's 120 calls a minute (D-SPLIT-07-RATE). Blocks: `bulk.get` also serves `display.<name>` (the display list of an SDK app the core runs, `Host::record`) and `camera.<view>.<camera>` (the newest JPEG of a view the person agreed to), see `iyakki-compose::frames` and D-SPLIT-07-FRAMES.

Proof: `crates/iyakki-ui-bin/tests/socket_shell_fake.rs` (a reference server with one host of remote objects over a throwaway device) and `socket_shell_real.rs` (the real core's `uiserve`): the login screen draws, a wrong password is refused by the core's accounts, the person signs in, the launcher draws, thirty apps open and draw, a setting changed on the Cloud mode page of Settings lands in the core's own settings and its audit line is in the core's log, a chat message becomes a run of the core and its answer is on the screen. Not run on a booted device (Manual).

## 15. The assistant and the camera views over the socket (F-SDK-85, F-SDK-166, alpha.7, branch `alpha.7/impl-sdk-c`)

`iyakki-ui-client/src/facets.rs` carries two more domains on the same line transport: `assistant.*` (`models`, `roles`, `open {app, spec}`, `set_context`, `choose`, `send`, `poll`, `subscribe`, `decide`, `answer`, `cancel`, `history`, `close`) and `cameras.*` (`list`, `open {app, spec}`, `decide`, `state`, `set_visible`, `frame`, `overlay`, `snapshot`, `ptz`, `events`, `close`). `SocketCore::assistant()` and `SocketCore::cameras()` give a UI program the same traits the shell has. The person is the listener's (`Served::ctx`), never the request's; a connection reaches only the sessions and views it opened, and the views and sessions of a connection are closed when it ends.

* **Scopes** (granted with `ui_socket_set`, listed by `ui_socket_status`): `assistant:use`, `assistant:approve`, `cameras:view`, `cameras:consent`. The `approve` and `consent` scopes are what let a connection say yes to a tool card, a camera consent, a snapshot or a move; a no needs none. They are scopes of this socket only (`uiserve::FACET_SCOPES`), not of the paired-computer contract. The FLOWS objects add `flows:edit` (change flows, the assistant's book, the message policy) and `flows:hooks` (webhooks and "allow this program": the asks), beside the contract's `flows:read` and `flows:run`; the program's default scopes (`PROGRAM_SCOPES`) hold `flows:read`, `flows:run` and `flows:edit`, not `flows:hooks` (D-S5-02). Every call of the six objects names its scope, and the person is always the account the core knows.
* **Events are pushed.** After `assistant.subscribe {session}` the machine writes `{"push":"assistant","session":..,"events":[..]}` lines between answers, from a thread of the connection (so it ends with it); at most 4 subscriptions per connection. `SocketAssistant::wait_events` blocks until some arrive; `poll` hands out what already arrived. The core's own facet is pull-only, so the machine side looks every 30 ms; the UI does not.
* **Pictures** are cut into 40 KB chunks (a line is 64 KiB); the client puts them together and starts again if the picture moves on under it.
* **Not built:** the daemon serves them only when it was started with an assistant and cameras for the account (`uiserve::Facets`); a UI program outside the repository has not been run against a real machine.

## 16. Booted proof (F-SPLIT-03, alpha.10, lane B1, 2026-11-24)

The first QEMU boot of `iyakki.ui=process`. Image: the alpha.8 base root and kernel with the alpha.10 core (`iyakki`, device profile, musl) and `iyakki-ui` (`--features socket-shell`) laid over it as an extra cpio (the autotest recipe). Evidence: `~/iyakki-work/autotest/b1-*` (outside the repository).

**Does the image ship it?** No. `post-build.sh` installed `IYAKKI_UI_BIN` when a build set it, but nothing in `build.sh` ever built the program. Now `dist/buildroot/ui-program.sh` builds it (`cargo build -p iyakki-ui-bin --features socket-shell`, musl), `IYAKKI_WITH_UI_PROGRAM=1` makes `build.sh` do so and `lean-image.sh` takes `IYAKKI_UI_BIN=` for a lean image. The mode is off unless `iyakki.ui=process` is on the kernel command line (GRUB `e`) or `IYAKKI_UI_PROCESS=1` wrote `/etc/iyakki/ui-process.enabled` into a test image. `dist/buildroot/test-ui-program.sh` checks the recipe (run by `iyakki-relcheck::release_image`).

**What the boot found (all fixed, with tests):**

1. The boot screen was never told to let go of the display in process mode, so the splash and the program drew on the same framebuffer (torn screen, the splash over the launcher). The steps the shell thread did when it started (end the boot screen, tell the boot loader the start worked, live-restart healthy report, data migrations, display probe) are now one function, `desktop_start_steps`, run by the thread and, before the program starts, by `start_ui` (test `ui_start_tests`). Before this fix process mode also never confirmed a start to the boot loader.
2. The reaper of PID 1 (`waitpid(-1)`) took the program's exit status: the audit said "gone (No child process)". Supervised children are now skipped (`keep_supervised`/`release_supervised`, `reap_children` peeks with `WNOWAIT`; test `iyakki-core::reap_supervised`). The audit now reads `service.exit ui: signal: 9 (SIGKILL)` and `exit status: 5`.
3. The core closed a connection after 100,000 calls (`MAX_CALLS_PER_CONNECTION`). The program makes several hundred calls a second on its one connection, so after about four minutes it lost the core, ended with status 5 (`CoreWatch`) and the person was back at the sign-in screen, again every five minutes (booted: exit 5 at 219.9 s and 522.5 s). `call_cap` leaves the cap for the catalog's pace and lifts it for the first-party pace (`FIRST_PARTY_RATE`); tests `server::cap_tests`. The exit status 5 of `CoreWatch` was therefore seen on a booted machine (that way), and with the fix it no longer happens by itself.

**What the checks prove** (`dist/qemu/session-checks.list`, profile `ui_process`; scenarios `F-SPLIT-03`, `b` to `e`; run `session-checks.py IMAGE --only ui_process_tree,...` with `IYAKKI_CLIENT` and `IYAKKI_QEMU_ROOT_KEY`, or `./run.sh autotest --profile qemu --ui-process --rows ...` for any scenarios):

| Check | Result |
|---|---|
| `ui_process_tree` | process 1 is `/sbin/init` (the core), `/usr/bin/iyakki-ui --socket /run/iyakki/ui/ui.sock --fb /dev/fb0 --restore` is its child (user root), the core has no `iyakki-shell` thread, the audit has `service.start ui pid N (start 1)`, `dev state` reads the program's screen through the core |
| `ui_process_screen` | Settings and a tile draw through the socket, a theme change on its screen is measured, FLOWS says it is not available in this program (D-SPLIT-03-UNSERVED) |
| `ui_process_socket` | folder `drwx------` and socket `srw-------`, uid 0 both, the program's user; the dev shell's unprivileged account is refused at the folder (`Permission denied`); with the modes opened by root for the test, `iyakki-ui --socket` as that account gets `Connection reset by peer` from the core (the `SO_PEERCRED` lock), modes restored |
| `ui_process_restart` | `kill -9` of the program: audit `service.exit ui: signal: 9 (SIGKILL)`, `service.start ui pid N (start 2)`, parent still the core; the screen returns at the sign-in screen (the program's session is gone; it signs in again by typing the scratch password) |
| `ui_process_stays_up` | 420 s of uptime, no exit of the program by itself |

How the dev API works in process mode: the Settings page that switches developer mode on and shows the pairing code is NOT served over the socket (it says "unavailable: this core does not serve this call"). So the guest is paired in the usual mode on a data volume and restarted with `iyakki.ui=process` on the same volume (pairing, grant and developer mode are kept); after that `dev state`, `dev input` and `dev shell` work through the core. A root door (`iyakki.ssh=1`, a throw-away key whose public half is the image's `/etc/iyakki/authorized_keys`) is used for what the dev shell's unprivileged account cannot do (kill, file modes).

**Not proven on a booted machine:** the core dying (PID 1 cannot be stopped, a live restart of the core is off by default) beyond what is above; the peer check for a non-root peer that passes the file mode was proven only with the modes opened by root; real hardware (a real display, touch, GPU) stays Manual. The program's stderr goes to `/dev/null` under the supervisor, so the reason for an exit (only its status is audited) is lost.

**Sizes** (`lean-image.sh --layers`, squashfs zstd level 3, device-profile programs, alpha.8 base root): base 417.1 MB, core 63.7 MB, ui 46.4 MB. The core layer still holds the whole `iyakki` program (the shell thread is still inside it), so the ui layer is an addition of the second copy of the shell, not a split of the first; the ui layer is the one a UI-only change would replace.

**Startup** (3 boots per mode, `~/iyakki-work/autotest/b1-measure`): kernel clock at the boot screen's end 3.4 to 3.6 s in both modes; in process mode the supervisor starts the program 0.05 to 0.1 s after that. The first frame the shell draws (the create-administrator screen, found by screenshot and OCR) was seen at 7.2 to 7.3 s of host time in both modes, the resolution of the polling (about 2 s), so no difference is measurable at this resolution. These are counts and logs, not a stopwatch assertion.

Same suite in both modes (`./run.sh autotest --profile qemu --rows ...` without and with `--ui-process`, 9 screen-driving scenarios that apply to the qemu target, evidence `~/iyakki-work/autotest/b1-compare/{thread,process}`): F-AI-17, F-APPSTORE-08, F-DEVT-05 and F-RAG-08 give the same PASS in both modes; F-A11Y-05 and F-NET-06 PASS in the usual mode and FAIL in process mode with "timed out waiting for app=Settings (saw Home)", the key sent by `dev input` never reaching the program (the first KNOWN-ISSUES draft above); NEEDS-HUMAN and NOT-APPLICABLE rows are the same in both runs.
