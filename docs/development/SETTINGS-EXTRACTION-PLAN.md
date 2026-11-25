# Settings extraction plan (alpha.10, lane ST1)

Goal: the Settings tile reaches the shell only through small port traits (APP-DEVELOPMENT-PROCEDURE.md section 10), so each page runs on a fake in
unit tests and the tile can later become an SDK app. Settings is the biggest tile: about 29,000 lines of code and 21,600 of tests in 147 files
(`crates/iyakki-apps/src/apps/settings.rs`, `settings_*.rs`, and `bluetooth_page.rs`, `devices.rs`, `model_picker.rs`, `browser_config.rs` that
pages share). `settings.rs` alone is 7,730 lines: 5,565 of code (the `SettingsApp` state, the dispatch, the modals, and the pages that were never
split out) and about 2,000 of inline tests (line 5566 on).

## 1. How the code is laid out today

- One `SettingsApp` struct. Every page is `impl SettingsApp` in a child module of `settings.rs` (`#[path = "settings_x.rs"] mod x_page;`), and
  each file starts `use super::*;`, so a page can see all of the parent's private state. A page is usually `handle_x(&mut self, k, cx: &AppCx)`,
  `render_x(&mut self, g, a, s, cx: &AppCx)`, a `x_buttons()` list and a form submit.
- 56 pages in 8 categories (`Tab::ORDER`, `Tab::group()`, `keywords()`); the left list is `settings_nav.rs` (pure, no shell context).
- The shell context reaches the pages as `AppCx`. What the pages name of it (counted in non-test code): `cx.session` (user, policy, admin check) and
  `cx.audit` in nearly every file; `cx.data_dir()`; `cx.settings()` (the settings file); then, in fewer files, `crate::agent()` (the agent helpers:
  host probe, power fan, scene, profile policy), `cx.services` (the vault, the voice/TTS engines, models, storage), the store API (`updates`,
  `developer`, `homelayout`, `overview`, `wizard`), `cx.host` (`settings_host.rs`: 17 uses), `cx.web` (`browserspeed`), `cx.os` / the core (`computer`,
  `crashlog`, `fleet`, `install`, `memory`, `time`), `cx.store` (`enchome`, `remote`, `sftp`).
- The migrated-tile ratchet (`crates/iyakki-core/tests/data/migrated_tiles.list`) lists a file with the most `AppCx` mentions it may have, and
  forbids `Services`, `.services`, `cx.host`, `cx.web`, `cx.chat`, `cx.store`, `cx.session`, `cx.audit` in its non-test code. A page that takes a
  port parameter must NOT call it `cx` (the text `cx.audit(` trips the check): call it `p`.

## 2. The port design (built in step 2, `settings_port.rs`)

Not one 500-method trait: a trait per kind of thing, each tiny, and a page names only the traits it uses.

| Trait | Methods | Used by |
|---|---|---|
| `PagePort` (done) | `is_admin`, `audit(action, subject)`, `data_dir`, `now_secs` | every page |
| `SettingsDataPort: PagePort` (done) | `settings_get() -> Settings` (copy), `settings_update(&mut dyn FnMut(&mut Settings)) -> Result<(), SettingsWriteError::{Busy, Refused}>` | the pages that only keep choices in the settings file (radio, battery, power, screensaver, notifications, ...) |
| `SecretsPort` (next) | vault locked/unlock state, read/write a named secret, provider aliases | Vault, SecretProviders, Models, Gemini, Alexa, Matrix, Lemonade, Agents, DevCluster |
| `ModelsPort` (next) | providers, the chat/embedding choice, model lists, pulls, usage (as `story_port::TalesPort` already does) | Models, Lemonade, ModelHost, ModelOpts, Router, Assistant, Rag |
| `DevicePort` (next) | the guarded hardware handle: sound, display, brightness, bluetooth, battery env, keyboard | Sound, Sounds, Display(s), Bluetooth, Keyboard, Battery reading, Power |
| `NetworkPort` (next) | wifi/ethernet state, firewall rules, tunnels, VPN profiles, domains routes, remote serve, shares, SFTP | the Network category |
| `SystemPort` (next) | time/NTP, memory/zram, storage and install, updates (store API), crash log, startup registry, restart | the System category |
| `AccountPort` (next) | users, roles, sign-in, encrypted home, backup, cloud tenants | Account, Backup, EncHome, CloudRoles |
| `BrowserSettingsPort` (next) | the person's browser config file (`browser_config::load/save`), the agent's profile policy, certificate and speed probes | the Browser pages |
| `DevPort` (next) | developer mode, dev cluster, git keys, USB-C link | Developer, DevCluster, GitKeys, Usbc |

Rules (same as `chat_port.rs`, `ide_port.rs`, `story_port.rs`): the real implementation is `impl X for AppCx` in `settings_port.rs` (or, for a
group's own trait, in that group's `settings_<group>_port.rs`), the only place that reads the session or the services; the fake lives in the same
file under `#[cfg(test)]` with in-memory stores and `iyakki_testkit::ManualClock`; old tests that hand a page an `AppCx` keep running through the
real impl; new tests drive the page through the fake. Threads started by a page get a small `Worker` trait object cloned from the port (see
`TalesWorker`), never the port itself.

A page entry point is `handle_x(&mut self, k: &Key, p: &dyn SettingsDataPort)` (or the group's trait). `settings.rs` passes its `&AppCx`, which
coerces. `SettingsApp::admin` already takes `&dyn PagePort` (done), so every page can use it from a port.

## 3. The order and the disjoint file sets for four lanes

Order: (S0) one mechanical step by one agent, then (S1) the four lanes in parallel, then (S2) the shared shrink of `settings.rs`.

S0, before the lanes start (one agent, one commit per page, no behaviour change): move the pages still written inside `settings.rs` into their
own files so the lanes stop sharing it. Slices of `settings.rs` today (line numbers at a7c3b3bd): Voice 2348-2922 (+ `settings_voicewake.rs`),
Storage 2923-3287, hardware/Display/Accessibility `handle_hw`/`render_hw` 3288-3477, Browser body 3478-3637, Skills 3638-3732, Models 3733-3889 and
5013-5149, Rag 3890-3996 and 5150-5227, SecretProviders 3997-4017 and 5302-5380, Vault 4018-4056 and 5228-5301, Account 4057-4695 and 5428-5560,
theme/UI size cards 5381-5427 and 5526-5565; the inline tests 5566-7550 go to `settings_<page>_tests.rs` next to their page. After S0 the file
keeps only the struct, `Tab`, the dispatch (`render_page`, `handle`, `open_item`, buttons) and the modal plumbing.

Dispatch registry (part of S0): the three big `match tab` blocks in `settings.rs` (handle 4526 area, render 4734 area, buttons 4862 area) become
one line per page calling a function of that page's own file, grouped by lane in `settings_pages.rs`, so a lane edits only its own rows of that
table. The `mod` and `#[path]` lines at the top of `settings.rs` are also grouped by lane with a comment banner per lane; a lane adds or edits only
lines inside its own banner.

Lanes (file sets are disjoint; `*` means the page's `_tests.rs` file and its new port file go with it). Sizes are code / test lines of the
`settings_*.rs` files only, before S0.

| Lane | Pages | Files | Code / tests |
|---|---|---|---|
| ST-A Account, people, system | Overview, Account, Vault, SecretProviders, Notifications (+Matrix), Backup, EncHome, Time, Battery, Power, Computer, HostName, SessionLog, Startup, Memory, Storage, Install, Updates (+Wizard), Cloud, CloudRoles, Radio | `settings_{overview,vaultstate,notifs,matrix,backup,enchome,cloud,cloudroles,time,battery,power,computer,crashlog,startup,memory,install,updates,wizard,radio}.rs*`, new `settings_{account,vault,secretproviders,storage}.rs`, `settings_system_port.rs`, `settings_account_port.rs` | 4,985 / 4,707 + S0 slices |
| ST-B Apps and AI | Models, Agents, Skills, Rag, Assistant, ModelRouter, GeminiVoice, Alexa, Voice, HostAccess, AppAccess, AppEvents | `settings_{agents,assistant,router,gemini,geminivoice,alexa,lemonade,modelhost,modelopts,ragorder,voicewake,appaccess,appevents,host}.rs*`, new `settings_{models,skills,rag,voice}.rs`, `model_picker.rs` (shared with the Story tile: edits only additive), `settings_models_port.rs`, `settings_secrets_port.rs` | 3,899 / 3,088 + S0 slices |
| ST-C Browser, Network and sharing | Browser (16 sub-pages), Network, SshTunnels, Vpn, Remote (+Connect, Machines, SFTP), Domains, Firewall (+simple view), Shares (+Share internet), Fleet, Announce | `settings_{browser*,sshtunnels,vpn,remote,connect,machines,sftp,domains,firewall,fwsimple,shares,share,fleet,announce,link}.rs*`, `browser_config.rs`, new `settings_browser.rs`, `settings_network.rs`, `settings_network_port.rs`, `settings_browser_port.rs` | 6,085 / 5,412 + S0 slices |
| ST-D Display, sound, devices, developer | Display, Displays, ScreenSaver, HomeLayout, Views, Sound, SystemSounds, Appearance, Accessibility, Bluetooth, Keyboard, Developer, DevCluster, GitKeys (+USB-C) | `settings_{access,displays,screensaver,homelayout,views,sound,sounds,appearance,pagescroll,keyboard,developer,devcluster,git,usbc}.rs*`, `bluetooth_page.rs`, `devices.rs`, new `settings_display.rs`, `settings_device_port.rs`, `settings_dev_port.rs`, `settings_bt_tests.rs`, `settings_mac_tests.rs` | 5,850 / 4,774 + S0 slices (Display/hardware 3288-3477) |

Shared files every lane may touch, with a strict rule (one added or changed line inside the lane's own banner, nothing else): `settings.rs`'s
`mod` banners and dispatch rows, `migrated_tiles.list` (append lines for the lane's own files), `FEATURES.md` rows. Nobody edits another lane's
file. `settings_port.rs` (the two shared traits) is frozen after this step; a lane that needs a new shared method asks for it in its report and
adds it to its own group trait instead.

Cross-page tests that are not a page's own: `settings_ia_tests.rs`, `settings_choices_tests.rs`, `settings_responsive_tests.rs`,
`settings_aihw_tests.rs`, `settings_storageloss_tests.rs` stay with S2 (they walk every page and must keep passing after each lane merges).

Per page, the lane does: (1) a group trait with the calls the page makes (look at `cx.` in the file), the real impl over `AppCx` and the fake;
(2) change the page's entry points to take the port; (3) add the file to `migrated_tiles.list` with its allowed `AppCx` count (0 when its entry
points take the port); (4) fake-port tests (a change saves, is audited, a non-admin is refused, busy/refused answers show, the page draws at
several sizes), no sleeps, locks from `iyakki_testkit` for shared resources; (5) every old test stays green.

## 4. The first slice (done in this step)

The smallest page group with the clearest boundary: Radio and Battery (`settings_radio.rs` 81 lines, `settings_battery.rs` 158). Both keep only
choices in the settings file, audit each change, and let administrators change them. Their only shell context was `cx.settings()`,
`cx.session`, `cx.audit`: all three are now `SettingsDataPort`/`PagePort` calls, both files are listed in `migrated_tiles.list` with 0 `AppCx`
mentions, and `settings_{radio,battery}_tests.rs` gained fake-port tests (saves and audits, non-admin refused and audited, busy answer, drawing).
Power is the next-smallest (226 lines) and needs the device trait for the fan card (`crate::agent().host_probe()`), so it moves with `DevicePort`.

## 4b. S0 (done, mechanical, no behaviour change)

- `settings.rs` went from 7,730 to about 3,160 lines. The page code that was inside it now lives in `settings_{account,vault,secretproviders,storage}.rs`
  (lane ST-A), `settings_{models,skills,rag,voice}.rs` (ST-B), `settings_{browser,network}.rs` (ST-C: `network` holds the MAC mode and Wi-Fi
  helpers) and `settings_{hw,cards}.rs` (ST-D: `hw` is the shared Network/Display/Sound page (`handle_hw`/`render_hw`), whose Network half ST-C and
  whose Display/Sound half ST-D must coordinate on; `cards` is the theme and UI-size cards of Display and Account). Moved methods became `pub(super)`;
  paths `super::x` became `super::super::x`. Nothing else changed.
- The dispatch registry is `settings_pages.rs`: `page_key`, `render_page` and `page_buttons`, one row per page, grouped under a `lane ST-x` banner.
- The `mod` lines of the moved pages sit under lane banners in `settings.rs`.
- The inline tests moved out: `settings_tests.rs` (the shared helpers, now `pub(super)`, and the tests that belong to no one page: row tooltip, every tab
  renders, nav, mouse reach) and `settings_lane_{a,b,c,d}_tests.rs` (52 tests sorted by lane; they `use super::tests::*`). A lane may later split its
  file per page. A file name ending `_tests.rs` is skipped by the kernel-access and flaky ratchets, so keep that ending.
- `settings.rs` still holds: the struct and `Tab`, the data helpers, the jobs, the three big form/pick/confirm matches (`submit_form`, `apply_pick`,
  `apply_confirm`, about 700 lines that name every page's `FormKind`/`PickKind`/`ConfirmKind`), `open_item`, `handle`, `render`, the button bar and the
  voice free helpers (`vo_engine_lines`, `vo_flow`). Those are S2 (they move only after the lanes have finished with their pages).
- The kernel-access ratchet had never counted the moved code (the file was cut at its first test-module line); `settings_storage.rs` is listed with 1
  (a `"/dev/<name>"` display string). The flaky-pattern table went down from 140 to 138 sleeps because moved test files now end in `_tests.rs`.

## 5. Left after this step

- The four lanes, then S2.
- The page list and the left navigation (`Tab`, `ORDER`, `group()`, `keywords()`, `settings_nav.rs`) contain no shell context: `settings_nav.rs` is
  pure and already testable. Moving `Tab` metadata into a page table is part of S0's dispatch registry, not of the port.
- Hardware-only proofs stay Manual in each row; real-service pages (VPN, Gemini, Alexa, Matrix) are written against their trait with a fake and keep
  the real-service run in Manual.
