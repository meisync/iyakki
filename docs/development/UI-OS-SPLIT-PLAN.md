# Splitting the OS-facing layer from the UI: plan

Status: approved by the owner on 2026-10-25 (see "Decisions"). Steps 1-4 are built (see Migration); steps 5-9 are planned. It builds on
`docs/EMBEDDING-AND-BACKEND.md` (2026-10-14), whose `iyakki-api` / `iyakki-backend-*` names are replaced by the `iyakki-core-*` names below.

**Naming, to avoid confusion.** The existing crate `iyakki-core` is the PID-1 supervisor (boot, services, wiring) and keeps its name and
role. The new crates are `iyakki-core-api`, `iyakki-core-local` and `iyakki-core-fake`: "IYAKKI core" in the three-layer picture is the
whole layer (those three plus the services in `iyakki-agent` plus the supervisor `iyakki-core`), not one crate.

## Goal

1. Tile apps are tested and deployed faster.
2. A cloud deployment can change the UI (another web framework) and reach the local OS through a connector.

## The three layers (owner's wording)

```
IYAKKI apps (tiles, UI)  ->  IYAKKI core (our changes)  ->  OS kernel (Linux, not touched)
```

- **Apps** are the tiles and any future UI (the framebuffer shell, a web UI in the cloud). They never touch the kernel; they call core.
- **Core** is everything IYAKKI adds: `iyakki-core-api` (the contract), `iyakki-core-local` (the only code that talks to the kernel),
  the services in `iyakki-agent`, and `iyakki-core` (the supervisor, which keeps its name). The connector for a remote UI is a transport on this layer.
- **Kernel** is the stock Linux kernel and its drivers. IYAKKI reads and drives it through `/proc`, `/sys`, `/dev`, ALSA and netlink and
  does not patch it (module and firmware grafts for hardware are build steps, not part of this split).

The rule to test: nothing above core opens a kernel interface, and nothing below core knows about a tile or a UI.

## What is true today

- `iyakki-shell` (226k lines, 2338 tests, 220 tile files in `apps/`) and `iyakki-agent` (231k lines, 2647 tests) hold about 85 % of all tests.
  A change to one Rust tile recompiles the whole shell; every shell test links the agent and the hardware crates.
- Tiles reach the system in two ways: through `Services` (a god object with over 40 fields, about 520 accesses in the UI) and by
  reading `/proc`, `/sys`, `/dev` or running `amixer`, `aplay`, `ffmpeg` directly (`settings_install`, `settings_time`, `settings_memory`,
  `store_setup`, `browser_speed`, `disks_tile`, `soundask`, `mediaplay`, `media`, among others).
- The agent crate depends on the usb, drivers, storage, devctl and netscan crates and has 53 `Command::new` sites.
- `HostApi` has five methods; it is too small to be the boundary.
- Web-bundle tile apps already have the right boundary (`window.iyakki`, `store/tilesdk.rs`, every backend a trait with a fake, `tile_test` headless,
  `dev push --watch` hot reload). Rust tiles do not.

## Target layers

```
0  iyakki-core-api     traits + serde request/response/event types, ApiError, Capabilities, Identity. No IO.
1  iyakki-core-local   the only crate that touches /proc, /sys, /dev, ALSA, netlink, disks, bluetooth, ffmpeg, aplay
   iyakki-core-fake    in-memory implementation for tests (canned data, recorded calls)
2  services          iyakki-agent: chat, runs, store, flows, inbox, tilesdk (depends on core-api only)
3  UI                iyakki-shell and the web-bundle tile runtime
4  connector         iyakki-core-remote (client) and iyakki-connector (server on the device)
   wiring            iyakki-core (the supervisor) chooses Local, Remote or Fake
```

Display and input drivers (`fb.rs`, `kms.rs`, `input.rs`) stay in the shell in phase 1.

## Boundary and security

One definition, three transports: in-process trait call, Unix socket, HTTPS plus WebSocket. A call is `(domain, method, JSON) -> JSON | ApiError`
with an event stream per domain. Remote auth reuses developer mode's pairing, ed25519 signed requests, per-computer scopes and audit
(new per-domain scopes such as `sysinfo:read`, `audio`, `disks:read`, `net`); a browser UI uses the machine login with an `HttpOnly`,
`SameSite=Strict`, `Secure` cookie and a CSRF token as `iyakki-remote` does. No second auth scheme.

The connector never exposes: the vault or secret values, raw `/proc` `/sys` `/dev` paths or paths outside the jail, a general shell, kernel
modules, firmware, boot files or system update, provider addresses and keys, user or scope management.

## Migration (each step builds and keeps all tests green; no behaviour change before step 9)

Order: 1 -> 2 -> 3 -> 4 are independent of the rest and done (2026-10-25). 5 needs 2-3. 6 needs 5 (same pattern). 7 needs 5-6. 8 needs 7. 9 needs 7.
One commit per step (steps 5 and 6: one commit per slice). Sizes: S under 300 changed lines, M 300-1500, L over 1500.
Every step ends with: `cargo check --workspace` (the workspace excludes `iyakki-gui`), `cargo test -p iyakki-core --test boundary --test
feature_matrix --test dependency_ratchet`, and the filtered tile tests named in the step. Never the whole `iyakki-shell` lib suite
(`ide_tests::browser_tests` hangs); filter by module.

### Step 1 - dependency-rule ratchet (S, DONE)

- Files: `crates/iyakki-core/tests/dependency_ratchet.rs`, `crates/iyakki-core/tests/data/shell_kernel_access.allow`.
- Rule scanned in `crates/iyakki-shell/src/apps/**/*.rs` (non-test code only): the literals `"/proc/`, `"/sys/`, `"/dev/`, `Command::new(` /
  `process::Command`, and any use of `iyakki_devctl`, `iyakki_storage`, `iyakki_usb`, `iyakki_drivers`, `iyakki_netscan`, `iyakki_hwinfo`,
  `iyakki_zram`. It does not see indirect reads (`memgate::read_mem_now`, `sharedsite::statvfs_free`, `std::thread::available_parallelism`); those
  move in step 5 by hand.
- Today's list: 23 files, 89 lines (devices 40, settings_memory 14, system 5, store_setup 5, settings_install 3, then 18 files with 1-2 each).
- Proof: the test fails when a file gains a violation, when a new file has one, and when a number can be lowered (so the list only shrinks).
- Checklist: [x] test [x] allow-list generated from the tree [x] scanner self-test [ ] each later step lowers its numbers in the same commit.

### Step 2 - `iyakki-core-api` with `HostApi` (S, DONE)

- Files: new `crates/iyakki-core-api` (error, identity, host, sysinfo modules); `Cargo.toml` (member and workspace dependency);
  `crates/iyakki-agent/Cargo.toml`; `crates/iyakki-agent/src/lib.rs` (the trait is replaced by `pub use iyakki_core_api::HostApi`).
- Call sites that keep compiling unchanged through the re-export: `iyakki-core/src/host.rs`, `iyakki-agent/src/{cli,agent}.rs`,
  `iyakki-shell/src/{shell.rs,apps/mod.rs,shell_loginbar_tests.rs}`, `iyakki-remote/{examples,tests}/*`, `iyakki-agent/examples/assistant_bench.rs`.
- Contents: `ApiError` (stable codes `not_found`, `denied`, `unsupported`, `invalid_input`, `busy`, `io`), `Capabilities` (audio, display,
  camera, keyboard, battery, bluetooth), `Identity`, the `Sysinfo` trait and the pure `/proc` parsers.
- Proof: `cargo test -p iyakki-core-api`; `cargo check --workspace` (every `impl HostApi` still compiles).
- Checklist: [x] crate [x] re-export [x] unit tests [x] licence Apache-2.0, deps only `serde`, `thiserror`, `iyakki-sec` (already in the tree).

### Step 3 - `iyakki-core-fake` with `FakeOs` (S, DONE)

- Files: new `crates/iyakki-core-fake` (`FakeOs`, `Canned`): implements `Sysinfo` and `HostApi` from canned data and records every call.
- Proof: `cargo test -p iyakki-core-fake`. Each later slice adds its trait to `FakeOs` in the same commit as the trait.
- Checklist: [x] crate [x] tests [ ] used by `cx_with_*` helpers (step 5).

### Step 4 - headless `TileHarness` for Rust tiles (S-M, DONE)

- Files: `crates/iyakki-shell/src/apps/tile_harness.rs` (test-only, registered in `apps/mod.rs`); a test-only `text_log` in `Gfx`
  (`gfx.rs`, `#[cfg(test)]`, records the strings drawn); `docs/development/TILE-TESTING.md`.
- Migrated as proof: `apps/radio.rs::tests::an_aac_station_without_the_add_on_offers_to_install_it` (15 -> 15 lines, but it clicks the
  button by its label instead of the magic `Event::Tab(310)`), `apps/calendar.rs::tests::moving_around_the_month_selects_days_and_changes_months_correctly`
  (30 -> 28 lines; the four sizes are now the standard four).
- Proof: `cargo test -p iyakki-shell --lib -- apps::tile_harness apps::radio::tests apps::calendar::tests`.
- Checklist: [x] harness [x] 2 tests migrated [x] doc [ ] migrate a tile test whenever it is touched (rule, not a task) [ ] `AppCx` from
  `FakeOs` after step 5 (the harness takes any `AppCx` today).

### Step 5 - Sysinfo slice (M)

- Add `trait Os { fn sysinfo(&self) -> &dyn Sysinfo; }` to `iyakki-core-api` (later slices add accessors) and implement it on `FakeOs`.
- New crate `iyakki-core-local` with `LocalOs` implementing `Sysinfo` over `/proc` and `/sys/class/dmi` (reuse the parsers in `core-api`; paths are a
  field so tests use a fake tree). Dependencies: `core-api` only. This is the first code of the crate step 7 fills.
- `AppCx` gets `os: Arc<dyn Os>`. The two literal constructions are `apps/mod.rs::tests::cx_with_admin_store` (give it a `FakeOs`) and
  `shell.rs` (~line 804; give it `LocalOs`; `shell.rs` and the files below belong to the home-screen/settings work, so do this when that is merged
  or with its author). Every other test builds its `AppCx` through helpers.
- Call sites to move (each becomes `cx.os.sysinfo().x()`; the ratchet number drops with it):
  - `apps/settings_memory.rs:42` `/proc/cmdline` (and `Env::real()` for zram figures, step 6 with Disks/Memory).
  - `apps/settings_time.rs:53` `/proc/cmdline` (hostname panel).
  - `apps/store_setup.rs:99` `/proc/meminfo`, `:104` `/sys/class/dmi/id/product_name`, `:676` `/proc/uptime`.
  - `apps/browser_speed.rs:25` `memgate::read_mem_now()`, `statvfs_free`, `available_parallelism` (`machine()`), also `apps/settings_displays.rs:61`
    and `shell_panes.rs:125` (`memgate::read_mem_now`).
  - `splash.rs:102` `/proc/uptime` (shell, outside `apps/`, not counted by the ratchet).
  - Agent side (step 7 moves them): `audioout.rs:1039`, `clusters.rs:855`, `cloudmode.rs:266`, `k3sdev.rs:84`, `sysinfo.rs` (the SYSTEM tile's source).
- Test that proves no behaviour change: for each moved site, a test with a fake tree (`LocalOs` over a temporary `/proc`) giving the same
  value the old code gave, plus the tile's existing tests unchanged: `apps::store_setup_tests`, `apps::settings_time_tests`, `apps::settings_memory_tests`,
  `apps::settings_browserspeed_tests`, `apps::browser_speed_tests`.
- Checklist: [ ] `Os` trait [ ] `iyakki-core-local` [ ] `AppCx.os` [ ] 4 tiles moved [ ] allow-list lowered by 10 lines [ ] FEATURES row updated.

### Step 6 - Audio, Disks, Net slices (L, one commit each)

Order: Audio, Disks, Net (owner's order; Audio first because the mixer already has the one-entry design in `docs/development/AUDIO-MIXER.md`).
Each slice: trait in `core-api` (serde types, no IO), `FakeOs` implementation, `LocalOs` implementation that wraps the existing crate, tiles moved,
allow-list lowered, a test per tile proving the same output from the fake as before from the fixture.

- Audio (`Audio` trait: cards, volume get/set/change, mute, default sink; events later): `apps/devices.rs` (`iyakki_devctl::{cards, Volume,
  Control}`, ~12 sites), `apps/settings_sound*.rs` (through the agent), `apps/radio.rs` and `apps/browser_sound.rs` (via `iyakki_agent::mixer`/`quiet`,
  not counted today), `apps/soundask.rs` (`/proc/asound`, signals), `apps/mediaplay.rs` (shell `aplay` pipelines in `mediaplay.rs:267-278`).
- Disks (`Disks`: list, usage, mount, format, install target): `apps/disks_tile.rs` (`:166` uid/gid from `/proc/self`, `:348` `iyakki_storage::human`),
  `apps/disks_tile_ui.rs:76`, `apps/settings_install.rs` (`iyakki_usb::install`, `InstallEnv { "/sys", "/dev", "/proc/mounts" }`, 3 lines),
  `apps/settings_memory.rs` (14 lines, `iyakki_zram`, `iyakki_storage::data_location`), `apps/settings_crashlog.rs`, `apps/settings_overview.rs:55-58`,
  `apps/settings_devcluster.rs:1086`, `apps/system.rs:479-505` (zram, `iyakki_storage::space`), pure helpers `iyakki_storage::human` and
  `loss::reword` (used by `camera.rs:314`, `chat.rs`, `story.rs`): move these into `iyakki-core-api` as plain functions, no IO.
- Net/Devices (`Net`, `Bluetooth`, `Screen`, `Rfkill`): `apps/devices.rs` (links, rfkill, bluetooth, brightness, mac; 40 lines in all),
  `apps/scan.rs:15,139` (`iyakki_netscan`), `apps/settings_developer.rs:226` (`ipv4_addresses`), `apps/system.rs:127,704` (`devctl::mac`, `hwinfo::collect`),
  `apps/store_drivers.rs`, `apps/store.rs:2587` (`iyakki_drivers::known`), `apps/settings_keyboard.rs:21` (`"/sys"`), `apps/voice.rs:389` and
  `apps/voice_ui.rs:239` and `apps/browser_full.rs:436` and `apps/linuxwin.rs:207` (a `"/proc"` root passed to agent code: the agent takes the
  root from `cx.os` instead).
- Not to move: the `Command::new` calls in `apps/media.rs:3212`, `browser.rs:6468`, `web_agent.rs:369`, `system.rs:862` are inside test code the ratchet skips.
- Test that proves no behaviour change per slice: the tile's fixture-based tests run again against `FakeOs` seeded with the same fixture
  (`apps::devices_tests`/`settings_bt_tests`/`settings_usbc_tests`, `apps::disks_tile_tests`, `apps::scan` tests, `apps::system_cluster_tests`,
  `apps::settings_install_tests`, `apps::settings_memory_tests`, `apps::settings_sound_tests`), and `LocalOs` is tested against the same fake
  `/proc` `/sys` trees these tiles used.
- Checklist per slice: [ ] trait + serde types [ ] FakeOs [ ] LocalOs [ ] tiles moved [ ] allow-list numbers lowered/removed [ ] FEATURES row.

### Step 7 - extract `iyakki-core-local` (L)

- Move the code that touches the kernel out of `iyakki-agent` and `iyakki-shell` into `iyakki-core-local`: the dependencies of
  `iyakki-agent/Cargo.toml` on `iyakki-devctl`, `iyakki-usb`, `iyakki-drivers`, `iyakki-storage`, `iyakki-netscan` become dependencies of
  `iyakki-core-local`; the agent uses `iyakki-core-api` traits.
  Today's agent files by crate (non-test): devctl: `sharedfolder.rs`, `devmode/debug.rs`, `directlink/{mod,real}.rs`, `macfw.rs`; storage: `disks*.rs`,
  `scope.rs`, `services.rs`, `extagent/mod.rs`; usb: `devmode/cli_adb.rs`, `disks.rs`; drivers: `macrecovery.rs`, `audioout.rs`, `fwpack.rs`, `macfw.rs`,
  `store/{publish,drivers}.rs`, `camfw.rs`, `voicedesk.rs`; netscan: `cli.rs`, `modelhost.rs`. The 53 `Command::new` sites in the agent (top: `ssh.rs` 7,
  `webengine.rs`, `tasks.rs`, `player.rs`, `jobs.rs` 3 each) move or stay case by case: user-program runners (jobs, tasks, ssh, git) are services and stay;
  `aplay`, `amixer`, `ffmpeg`, `mount` wrappers move.
- `iyakki-core` (supervisor) builds `LocalOs` and hands it to the shell and the agent (the only place `Local`/`Fake` is chosen).
- Test that proves no behaviour change: the existing agent tests (`cargo test -p iyakki-agent` by module: `disks`, `audioout`, `quiet`, `mixer`,
  `macfw`, `directlink`), unchanged, plus `LocalOs` tests moved with the code; a new boundary test (extend `crates/iyakki-core/tests/boundary.rs` /
  `BOUNDARY.md`) that only `iyakki-core-local` and `iyakki-core` name these crates.
- Checklist: [ ] one crate at a time (usb, drivers, netscan, devctl, storage) [ ] BOUNDARY.md rows move to `core-local` paths [ ] agent `Cargo.toml`
  loses the dependencies [ ] image build unchanged (not run here).

### Step 8 - split `Services` (M-L, per tile on demand)

- `crates/iyakki-agent/src/services.rs` (over 40 fields, about 520 accesses in the UI): add small facades (`RadioSvc`, `CalendarSvc`, ...) that a tile asks
  for in `new`; no big-bang. Convert a tile when it is touched for another reason; each conversion makes its test build the facade from a fake.
- Test: the tile's existing tests, unchanged; a count of `cx.services.` accesses in the tile file goes down (tracked in the PR text, optionally a
  second ratchet file `services_access.allow` in the same style).
- Checklist: [ ] first facade as a pattern [ ] ratchet for `cx.services.` [ ] decision 6 (split the shell crate) taken after this step.

### Step 9 - opt-in transports and hot reload (L, later, out of scope for the first steps)

- Unix-socket transport of the `(domain, method, JSON) -> JSON | ApiError` call and an event stream per domain (`iyakki-core-api` gets the
  dispatcher; `iyakki-core-local` is served by it); then `iyakki-core-remote` (client implementing the traits) and `iyakki-connector` on the device,
  behind a cargo feature, off in release until the security review; per-domain scopes in developer-mode pairing; more hot reload for tiles.
- Test: a round-trip test per domain over a socket pair against `FakeOs`; a refusal test per route (missing scope, over-long body, rate limit); the
  existing remote and developer-mode tests unchanged.
- Checklist: [ ] dispatcher [ ] socket [ ] scopes and audit [ ] refusal tests [ ] security review before enabling.

Fastest wins: steps 1-4 (done) and making web-bundle tiles the default for new tiles (decision 5; `docs/development/TILE-TESTING.md`). A separate build
unit for Rust tiles (`iyakki-tiles` on `core-api` and an `iyakki-gfx` crate) only becomes possible after steps 7-8.

## Risks

Shell size (ratchet test, one slice per commit, no deleted tests); blocking hardware calls inside `render` once remote (poll model with cached
values); image-only boot behaviour stays in `iyakki-core`; secrets stay on the owning device; every remote route is attack surface and needs a
scope, audit, rate and body limits and a refusal test before v1.0.0; only crates already in the tree (check `deny.toml`).

## Out of scope

Pixel streaming (`iyakki-remote` stays); the React UI itself; moving `iyakki-js`, `iyakki-web`, `iyakki-webview` behind the API; the `Component`
embedding work (B0-B6); moving display and input drivers in phase 1; new hardware.

## Decisions

Approved by the owner on 2026-10-25: 1 (names `iyakki-core-*`), 3 (the ratchet first), 5 (new tiles are web-bundle tiles by default; Rust tiles only where
`Gfx` is needed), 9 (a baseline timing run first), and "plan all steps, start with the first ones". Decided as recommended (not separately confirmed):
2 (`HostApi` seeds `core-api`; display and input drivers stay in the shell in phase 1), 4 (slice order after Sysinfo: Audio, Disks, Net).

Still open: 6 (split `iyakki-shell` into `iyakki-gfx` plus a tiles crate: after step 8), 7 (wire format JSON over HTTPS and WebSocket, remote auth
reusing developer-mode pairing and scopes: before step 9), 8 (remote transport behind a feature flag, shipped off until the security review: before
step 9).

1. Crate names `iyakki-core-api`, `iyakki-core-local`, `iyakki-core-fake` (not `iyakki-os-*`); the supervisor `iyakki-core` keeps its name and role. APPROVED.
2. `HostApi` as the seed of `core-api`; display and input drivers stay in the shell for phase 1. As recommended.
3. Step 1 (the ratchet test) as the first commit. APPROVED.
4. Slice order after Sysinfo: Audio, Disks, Net. As recommended.
5. New tiles are web-bundle tiles by default; Rust tiles only where `Gfx` is needed. APPROVED.
6. Whether to split `iyakki-shell` into `iyakki-gfx` plus a tiles crate (large; after step 8). OPEN.
7. Wire format JSON over HTTPS and WebSocket, with remote auth reusing developer-mode pairing and scopes. OPEN (as recommended, confirm before step 9).
8. Remote transport: build behind a feature flag, ship off, enable only after the security review. OPEN (as recommended, confirm before step 9).
9. A baseline timing run first (shell test build, one-line edit, repack) so the speed-up can be shown in numbers. APPROVED; the run itself is not done yet
   (step 4's harness gives the "after" side).

## Addendum 2026-10-28: the forward-looking contract (owner: "yes please go ahead, define a futuristic contract")

The steps above move kernel-facing code behind small traits. This addendum widens the target to every domain that is today reachable only
through `Services`, and fixes the rules a cloud or web UI will rely on. The full contract is `docs/development/CORE-CONTRACT.md`; in code it lives in
`iyakki-core-api::contract` (types, traits for settings, vault, flows and consent, the call catalogue) and `iyakki-core-fake::contract` (fakes).
It changes no tile's behaviour. **Done 2026-10-28 (migration steps 1 to 3):** a real local implementation of settings, vault, flows and consent over the existing services (`iyakki-agent::contract_local`), one conformance suite run against both the fake and the real adapter, one screen path per domain on the contract (Settings > Time, Settings > Vault, the FLOWS next-runs list), and an opt-in, off-by-default carriage over the developer-mode TLS channel (`devmode::contract`) with a loopback test. No relay. Details and the open owner questions: `docs/development/CORE-CONTRACT.md` sections 5 and 6.

What it adds to "Boundary and security" above: three transports behind one typed API (in-process now, the developer-mode TLS channel, a cloud relay
the machine dials out to); `hello` with version and capability discovery; events with sequence numbers and resume; chunked, resumable streams
for large data; scopes, a risk class and an audit line for every call; consent calls only the local screen can answer; idempotency keys and
expiring queued requests for a machine that is off; one error type with stable codes, message keys and hints; the vault never returns a secret.

Slice order for the Services domains (each slice follows the pattern of steps 5-6: trait, fake, local implementation, first tile, ratchet):

1. settings + accounts, 2. vault (and consent), 3. flows + jobs, 4. chat/agent + models, 5. store + apps + updates + downloads manifest,
6. files, sharing, backup, cluster, Git, 7. network + firewall, 8. browser + media, 9. doctor + diagnostics, 10. inbox + notifications,
11. power + time + display.

Slices 1-3 have their trait and fake already (accounts is the exception: not yet defined). They come before the Sysinfo-style kernel slices of steps 7+
only where a tile needs them; the two orders do not block each other. Decision 7 (wire format and remote auth) is refined, not settled: the
contract fixes the shapes; the choice of relay protocol stays open until step 9.
