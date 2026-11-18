# Compile-speed pilot: the toolkit crate and what blocks the first tile (2026-11-07)

Pilot of Phase 2 of `COMPILE-SPEED-PLAN.md`. Result in one paragraph: the **toolkit moved out of `iyakki-shell` into a new crate
`iyakki-uikit`** (19 modules, about 9 300 lines, 120 unit tests, no `iyakki-*` dependency, re-exported under the old paths so no tile
file changed). **No tile crate was extracted**: every tile implements `App` and receives `&AppCx`, and `AppCx` holds the agent's
`Services`, so a tile cannot leave the shell before `App`/`AppCx` have a home in `iyakki-core-api`. The blockers per tile are below.

## Phase 0: findings

### (a) The dependency picture inside `iyakki-shell`

The toolkit is one tight cluster (gfx, fx, ui, theme, shaping, uisize, safeclamp, ctxmenu all use each other) with very few edges out:

| Edge out of the cluster | Where | What it was | How it was cut |
|---|---|---|---|
| `iyakki_storage::loss::reword` | `fx::status_line` | rewords a failure line when the boot drive is lost | `fx::set_failure_rewriter(fn)`, set once in `Shell::new` |
| `iyakki_web::svg::TextMask` | `Gfx::text_mask` (one caller, `apps/chatpic.rs`) | picture text as coverage | `gfx::TextMask` with the same fields; `chatpic.rs` converts |
| `crate::fnkeys::FnKey`, `crate::panes::LayoutKey` / `Side` | `ui::Event` variants | plain data enums | new `keys.rs` in the toolkit; `fnkeys.rs` / `panes.rs` `pub use` them |
| `iyakki_agent::keyboard::ScrollPrefs` | `impl From<..> for ScrollConfig` in `scrolleng.rs` | 3 call sites use `.into()` | `fnkeys::scroll_config(prefs)` function in the shell (orphan rule forbids the impl) |
| `#[cfg(test)]` hooks (`Gfx.text_log`, `layout_log`, `ui::legacy_*`, `controls::HEADINGS`, thread-local scroll settings) | used by shell tests | invisible once the code lives in another crate | cargo feature `testing`, enabled by the shell's dev-dependency |
| `include_bytes!("../assets/*.ttf")` | `gfx`, `shaping` | 3.5 MB of fonts | still `include_bytes!`, now by path `../../iyakki-shell/assets/` (see "Not done") |

Moved with it, with no edge at all: `links`, `mdtext`, `textsel`, `tooltip`, `listwindow`, `scrolleng`, `scrolllist`, and from `apps/`: `neon`, `card`,
`controls` (drawing helpers that several tiles share). Not moved because they still reach the shell or the agent: `apps/widgets.rs` (needs
`filepicker`, which uses the agent), `apps/multiline.rs` (needs `apps::blink`), `apps/component.rs` (`AppCx`), `filepicker`, `osk`, `input*`, `touch*`, `fnkeys` (the
key *map* and device writes use `iyakki_devctl` and the agent; only the `FnKey` type moved), `panes` (same).

Rest of the shell crate: `shell*.rs` (about 40 000 lines of frame, windows, login, overlays), drivers (`backend*`, `fb`, `display*`, `input*`, `touch*`), and `apps/` (the tiles).

### (b) The ten largest tiles

`agent` = occurrences of `iyakki_agent::`; `other crates` = `iyakki_{devctl,storage,usb,drivers,netscan,hwinfo,zram,web,js,webview,rfb,flow,data,edit,sec,fw,vpn,ledger,schema}::`;
`crate:: outside toolkit` = `crate::x` where `x` is not one of the toolkit modules (including `crate::apps`, `crate::shell`, `crate::filepicker`). Counts are over the tile's
files (`<tile>.rs`, `<tile>_*.rs`, folder), tests included.

| Tile | Files | Lines | `iyakki_agent::` | Other iyakki crates | `crate::` outside toolkit | Main sibling tiles it reaches through `super::` |
|---|---|---|---|---|---|---|
| story | 2 | 10 332 | 109 | 4 | 32 | ide_chrome, multiline, widgets, browser_full, neon |
| browser | 30 | 19 340 | 144 | 213 (web, js, webview, rfb) | 23 | browser_config, soundask, widgets, web_agent |
| settings | 94 | 33 277 | 544 | 332 | 181 | firewall_page, git_page, sshtun_page, many pages |
| chat | 15 | 11 704 | 318 | 38 | 47 | ide_chrome, jobs_ui, dropattach, chatpic |
| store | 16 | 13 777 | 269 | 19 | 100 | setup, kind_label, widgets |
| dash | 5 | 6 532 | 62 | 4 | 8 | widgets, component |
| media | 1 | 3 384 | 30 | 1 | 4 | mediaplay, neon |
| ide | 26 | 22 785 | 338 | 23 | 39 | ide_chrome (58), ide_k, board |
| edit | 2 | 3 144 | 29 | 9 | 4 | ide_chrome, edit_preview, widgets |
| board | 4 | 4 115 | 53 | 0 | 0 | ide_chrome, widgets |

A few small ones for comparison: neon 0/0 (a widget, moved), card 0/0 (moved), usage 11, calendar 13 (130 uses of `cx`), mesh 2, learn 24, lab 22, inbox 43, auth 6.

Reading: the **agent counts are large everywhere**, but they are domain types and services (`iyakki_agent::calendar::Alarm`, `featureboard::Board`, `story::Reader` ...), not the toolkit. The number that matters
for the cut is that **every tile, however small, has `use super::{App, AppCx, Flow}`**.

### (c) What can move with no agent dependency

Done in the pilot (table in (a)): the whole drawing and input toolkit. What cannot move without core-api work:

1. **`App`, `AppCx`, `Flow`** (`apps/mod.rs`). `AppCx` has `session`, `host: Arc<dyn HostApi>`, `cfg: AiConfig`, `chat`, `audit`, `store`, `mcp_servers`, `services: Arc<Services>`, `web`. `Services` is the agent's 40-field registry. A tile crate that implements `App` needs the trait, and the trait needs `AppCx`, so the tile crate would depend on the agent. That is the single blocker for all tiles; it is exactly the `iyakki-core-api` work of `UI-OS-SPLIT-PLAN.md` (step 5: `cx.os`).
2. **Shared tile widgets** that the tiles reach through `super::`: `widgets` (Form, Picker; needs `filepicker`), `multiline` (needs `blink`), `component`, `ide_chrome` (used by story, chat, ide, edit, board). Each is a small move once `AppCx` is out of the way; `ide_chrome` first, because five of the ten tiles use it.
3. **Per-tile direct uses of agent domain modules.** Each becomes a trait in `iyakki-core-api` with a fake (for the tile's tests) or the agent type moves into a leaf crate (`iyakki-calendar` ...). Phase 3 of the plan.

Tests that build an `AppCx` (`apps::tests::cx_with_admin_store`, `TileHarness`) link the whole agent; that is why the shell test binary is slow, and it stays that way until (1) is done.

## Decision: pilot step 2 (extract one tile)

Not feasible without touching `iyakki-core-api` (blocker 1), so **no tile was moved** (instruction: "if there is no tile that can move without core-api work, move none").
Per-tile blockers, smallest first:

| Tile | Blockers besides `App`/`AppCx` |
|---|---|
| usage (395 lines) | 11 agent uses (`usage::{Range, Series}`, `clock`, `Metered` in tests); `Services` in tests |
| calendar (790) | `calendar` service via `cx.services.calendar`; `widgets::Form`; `blink` |
| mesh (853) | `consul` client types; `widgets::Form`; `blink` |
| auth (3 files) | `cloudflows` / identity services |
| learn (2 424) | `study`, `story::{Synth, Sink}`, `chatpic::PicCache`, `scrolllist` (now moved) |
| board (4 115) | `featureboard`, `tasks`, `testresults`, `coverage`; `ide_chrome`; `widgets` |
| dash (6 532) | `widgets`, `component`, 62 agent uses |

Recommended next step: move `App`/`AppCx`/`Flow` behind an `iyakki-core-api` facade (a `TileCx` trait with `session()`, `host()`, `os()` and the narrow service traits), then `ide_chrome` and `widgets` into `iyakki-uikit`, then `usage` as the first tile crate (smallest, one service).

## What the pilot changed

* New crate `crates/iyakki-uikit` (workspace member added to the root `Cargo.toml`). Dependencies: `tiny-skia`, `fontdue`, `rustybuzz`, `unicode-bidi`, `serde_json`. Feature `testing`.
* `git mv` into it: `gfx fx ui theme shaping uisize safeclamp ctxmenu links mdtext textsel tooltip listwindow scrolleng scrolllist` (+ `scrolleng_tests.rs`, `tooltip_tests.rs`) and `apps/{neon,card,controls}`. `iyakki-shell/src/lib.rs` and `apps/mod.rs` re-export each with `pub use iyakki_uikit::<module>;`, so `crate::gfx::Gfx`, `super::neon::...` and `iyakki_shell::ui::Event` (examples, `iyakki-gui`, `iyakki-remote`) compile unchanged.
* Small edits: `fnkeys.rs` and `panes.rs` re-export the key types; `fnkeys::scroll_config`; call sites in `settings_keyboard.rs`, `shell_fnkeys.rs`, `inputprofile.rs`; `chatpic.rs` converts `TextMask`; `Shell::new` registers the failure rewriter.
* `crates/iyakki-shell/tests/safeclamp_scan.rs` also scans `iyakki-uikit/src` (otherwise moving the files would have silently dropped them from the `.clamp(` scan).
* `crates/iyakki-core/tests/dependency_ratchet.rs`: `the_toolkit_crate_depends_on_no_iyakki_crate`; `tile_crates_depend_only_on_the_toolkit_and_the_list_only_shrinks` with `tests/data/tile_crate_deps.allow` (empty); the WASM interface crates `iyakki-tile-abi/-host/-example` are listed in `TILE_INFRA` and are not tile crates. The existing kernel-access list is unchanged (the moved `neon/card/controls` had no entries).
* `docs/development/COMPILE-SPLIT-PILOT-RENAMES.tsv`: 76 cited tests, old path to new path, for the owner to apply to `FEATURES.md`. **Until it is applied, `feature_matrix` fails** (it looks the cited tests up in `iyakki-shell`).

## Not done, and why

* **Crate name.** The task said `iyakki-ui`; that name already exists (the older headless editor model used by `crates/iyakki-gui`, row F-EDIT-17 plans to delete its shim). Renaming it touches the separate `iyakki-gui` workspace, so the new crate is `iyakki-uikit`. When the old crate is deleted, rename with `git mv` and change `UI_CRATE` in the ratchet.
* **Fonts not moved.** `include_bytes!` reaches `../../iyakki-shell/assets/*.ttf`. Moving the files would change about 25 references: `legal/assets.txt`, `legal/image-rules.txt`, `legal/sbom.cdx.json`, `NOTICE`, `LICENSING.md`, and the `dist/buildroot` scripts (`post-image.sh`, `changed.sh`, `qemu-*-test.py`, `web-post-build.sh`) which another change was editing. Nothing is duplicated. Do it as its own change with the legal files.
* **No tile crate**, so the "touch a line in the moved tile" measurement does not exist.

## Measurements (dev profile, warm, shared machine, so noisy)

One appended line, `cargo build -p iyakki-shell`, three runs each:

| What was edited | Rebuilt | Time |
|---|---|---|
| a tile inside the shell (`apps/usage.rs`) | shell only | 7.0 s, 19.0 s, 8.5 s (other builds were running; 50 s once for a `check` that had to catch up) |
| the toolkit (`iyakki-uikit/src/neon.rs`) | toolkit, then shell | 5.1 s, 5.1 s, 5.2 s |

Test binaries: after one line in the toolkit, `cargo test -p iyakki-uikit --no-run` takes 1.8 s; after one line in a tile, `cargo test -p iyakki-shell --lib --no-run` takes 18.5 s. Running the toolkit's 120 tests takes 42 s on their own (the largest are the script-shaping and drawing tests), and they no longer link the agent.
These numbers say little about the final win: the dev profile already has incremental compilation, so a one-line change is cheap either way. The gain of the split shows in (1) the test binary of the toolkit (no agent, no 32-minute shell run), (2) release builds with LTO, where a tile crate recompiles alone, and (3) the point in time when a tile crate exists. A cold build of the shell is unchanged or slightly better (the toolkit now compiles in parallel with the agent's last crates).

## Verification run

* `cargo check -p iyakki-uikit`, `cargo build -p iyakki-shell`, `cargo test -p iyakki-shell --no-run`: clean (warnings were there before).
* `cargo test -p iyakki-uikit`: 120 passed.
* `cargo test -p iyakki-shell --lib -- gfx fx widgets design_tests apps::usage apps::calendar apps::mesh shell_loss settings_storageloss scroll`: 85 passed, 1 failed (`apps::story::tests::the_options_popover_*`, the known unrelated failure).
* `cargo test -p iyakki-shell --test safeclamp_scan`; `cargo test -p iyakki-core --test dependency_ratchet`: pass (5 tests).
* `cargo test -p iyakki-core --test feature_matrix`: fails on the 76 renamed paths until the TSV is applied (see above).
* Not run: the whole shell suite (as instructed), anything on a device or in QEMU. The shell's `iyakki-gui` and `iyakki-remote` users of `iyakki_shell::gfx/ui` were not rebuilt (`iyakki-gui` is a separate workspace); the paths they use still exist.

## Second step: the first tile crate, `usage` (2026-11-07)

**What moved.** The USAGE tile (`apps/usage.rs`, 395 lines) is now the crate `crates/iyakki-tile-usage` (`git mv` to its `src/lib.rs`; drawing, key handling, `fmt_tokens`, `nice_max` are byte-for-byte the old code except the imports and the context). Its dependency tree is `iyakki-tile-api -> iyakki-uikit` and nothing else: `cargo tree -p iyakki-tile-usage -e normal | grep -c iyakki-agent` prints 0 (also 0 with `-e all`). The shell registers it exactly where it was (`shell.rs`: `PApp::Usage => Box::new(apps::usage::UsageApp::new(&cx))`; `system.rs` still calls `apps::usage::fmt_tokens`, now a re-export).

**The facade, `crates/iyakki-tile-api`** (depends on `iyakki-uikit` only):

| Piece | What it is |
|---|---|
| `App` | the tile contract: `title icon accent hint help handle render` (the same names and meaning as the shell's `App`; the other 20 methods of the shell trait stay in the shell until a tile needs them) |
| `Flow` | `Stay` / `Back` |
| `TileCx<'a>` | `user: &str`, `manage_settings: bool`, `usage: &dyn UsageSource` |
| `usage::{Range, Bucket, Series}` | the chart data (plain data, the same fields as the agent's) |
| `usage::UsageSource` | **2 methods**: `now_unix()`, `series(range, now, Option<user>)` |

So the usage tile needed 2 trait methods and 2 plain fields, well under the limit of ten. The shell side (`apps/usage.rs`, now 150 lines): `ShellUsage(&Services)` implements `UsageSource` with the agent's `UsageLog::series` in `Zone::system()` and converts the agent's `Range/Series` to the facade's (field copy); `UsageApp(iyakki_tile_usage::UsageApp)` implements the shell's `App` by building a `TileCx` from the `AppCx` on each call. Nothing else in the shell changed: `App`, `AppCx`, `Flow` and the 299 other tiles are untouched.

**Tests.** In the tile crate, against a fake `UsageSource` (an in-test log with hours-ago, user, model, tokens): `numbers_and_axes_are_rounded_for_reading`, `the_ranges_switch_and_show_my_usage_or_everyones_for_administrators`, `a_person_without_the_settings_permission_only_ever_sees_their_own_usage` (same assertions as before, same call counts) and the ignored `usage_picture`. Left in the shell because they test the agent: `what_goes_through_the_chat_backend_is_counted_for_the_person_and_the_model` (same path as before), and one new test, `the_shell_adapter_feeds_the_real_usage_log_to_the_tile_crate`, which drives the tile through the shell `App` trait and checks the converted series equals the agent's. The fake is a simplified bucketer; the agent's own `usage::tests` still cover the real bucketing. Renames for `FEATURES.md`: `COMPILE-SPLIT-STEP2-RENAMES.tsv` (3 tests; not applied, so `feature_matrix` fails until it is).

**Ratchet** (`dependency_ratchet.rs`): a tile crate may depend on `iyakki-uikit` and `iyakki-tile-api` only; `iyakki-tile-api` is in `TILE_INFRA` and has its own test (`the_tile_facade_depends_on_the_toolkit_only`: `iyakki-uikit` and nothing else). `tile_crate_deps.allow` stays empty.

**Numbers** (dev profile, warm, one cargo command at a time, `CARGO_BUILD_JOBS=3`, a shared machine where another agent was editing the agent crate, so noisy). The edit is one colour constant changed (a comment-only edit is cheaper still: 4 s and 10 s in the shell, 0.1 s in the tile):

| After a one-line code change in | Command | Seconds |
|---|---|---|
| the tile crate `iyakki-tile-usage` | `cargo check -p iyakki-tile-usage` | 0.2, 0.1 |
| the tile crate | `cargo test -p iyakki-tile-usage --no-run` | 0.3, 0.2 |
| a tile still in the shell (`apps/mesh.rs`) | `cargo build -p iyakki-shell` | 37.7, 29.7 |
| a tile still in the shell | `cargo test -p iyakki-shell --lib --no-run` | 30.0, 29.6 |

Running the tile's tests: 3 tests in 0.55 s. First build of the three new crates (uikit already built): about 1.4 s. The goal "fix one app and compile in under a minute" is met for this tile by two orders of magnitude; for a tile still in the shell it is about 30 s of dev build (release/LTO builds were not measured). Not measured: the cold cost, release profile, the whole shell suite.

**Verified:** `cargo test -p iyakki-tile-usage` (3 passed, 1 ignored); `cargo build -p iyakki-shell`; `cargo test -p iyakki-shell --lib -- apps::usage` (2 passed). **Not verified:** a run on a device or in QEMU (not allowed in this step), the screenshot test, the shell's other tests.

### Next tiles, ranked by (services used, lines), with what each needs

Counts are `cx.services.<field>` uses in the tile's non-test code.

| Rank | Tile | Lines | Services | Facade it needs | Other blockers |
|---|---|---|---|---|---|
| 1 | calendar | 790 | `calendar` (+ `on_login` in tests) | `CalendarSource` (alarms: list/add/remove/ring state, about 6 methods) and the `Alarm` data types moved to the facade | `widgets::Form` (needs `filepicker`), `blink`: move those two into `iyakki-uikit` first |
| 2 | learn | 575 + 536 + 446 | `calendar`, `study`, `host_tts`, `voice_settings` | `StudySource`, `Speaker`, `CalendarSource` (more than ten methods in total: split the tile first) | `chatpic::PicCache`, `story::{Synth, Sink}` |
| 3 | mesh | 853 | `secrets`, `quick_upstream`, `system_set`, `dir` | `MeshSource` (consul client calls) | `widgets::Form`, `blink` |
| 4 | lab | 1 671 (+ 3 files) | `settings`, `dir` | image-generation source | `multiline`, `widgets`, `ide_chrome`, `chatpic`, `browser_images` |
| 5 | inbox | 1 628 | 13 services | too many; later | `widgets`, `browser_consent` |

Recommended order: first move `widgets` (with `filepicker`'s picker part) and `blink` behind the facade or into `iyakki-uikit`, which unlocks calendar, mesh and lab; then calendar.

## Third step: `widgets`, `blink`, `filepicker`, `ide_chrome` into the toolkit; the `calendar` tile crate (2026-11-07)

Owner decisions applied: `widgets`, `blink` and the picker go into `iyakki-uikit`; `TileCx` stays one struct (one `&dyn` field per tile family) until more than five tiles are extracted, then it becomes a trait.

**What moved into `iyakki-uikit`** (all with `git mv`; the shell re-exports each under its old path, so no tile file changed):

| Module | Old path | Notes |
|---|---|---|
| `widgets` (Form, confirm, pickers, 822 lines, its tests) | `apps/widgets.rs` | `pub use iyakki_uikit::widgets;` in `apps/mod.rs` |
| `ide_chrome` (383 lines, its tests) | `apps/ide_chrome.rs` | not on the owner's list, but `widgets` calls it for its density metrics and it is pure toolkit code (gfx, fx, ui only), so it moved too |
| `blink` (`blink`, `caret_allowed`, `with_caret`) | `apps/mod.rs` | `pub use iyakki_uikit::blink::{blink, caret_allowed, with_caret};` |
| `filepicker` (1 153 lines: the picker modal, the path chip, drops) | `filepicker.rs` | split as below |

**The filepicker split.** The picker's drawing, listing and keys are pure, but it asked the agent for three things: the account's policy and file roots (`Scope` held a `Policy` and `FileRoots`), `explain_io`, and the signed-in session. Now `iyakki-uikit::filepicker` has a trait `ScopeRules` (`home`, `can_read`, `can_write`, `expand`, `explain_io`) and `Scope` wraps an `Rc<dyn ScopeRules>`; `current_scope()` reads a per-thread override (tests) or a per-thread maker function (`set_session_scope`), else `Scope::nothing()`. What stayed in the shell is `crates/iyakki-shell/src/filepicker.rs` (54 lines): `AccountRules` (the real policy and `FileRoots`), `scope_for(user, &policy, roots)` (was `Scope::new(user, &policy, roots)`) and `set_session(...)` (same signature, `shell.rs` unchanged); the rest is `pub use iyakki_uikit::filepicker::*`. The 405 lines of `filepicker_tests.rs` stay in the shell, because they build real accounts (agent `FileRoots`, security `Role`); the test helper `scope_for` there was renamed `account_scope` and two lines of `apps/disks_tile_tests.rs` call `scope_for` instead of `Scope::new`. `iyakki-uikit` still depends on no `iyakki-*` crate (`dependency_ratchet` passes).

**The calendar tile crate, `crates/iyakki-tile-calendar`** (`git mv` of `apps/calendar.rs` to its `src/lib.rs`; the drawing and key code is the old code with `cx.services.calendar.x(&cx.session.user, ..., &self.zone)` turned into `cx.calendar.x(cx.user, ...)`). Dependencies: `iyakki-tile-api` and `iyakki-uikit` only (`cargo tree -p iyakki-tile-calendar -e all | grep -c iyakki-agent` prints 0).

Facade additions (`iyakki-tile-api`): `TileCx` gains `calendar: &dyn CalendarSource` and `audit: &dyn AuditSink`; module `calendar` with the plain data (`Repeat`, `Alarm`, `Occurrence`, `AlarmOccurrence`, `Local`, `Added`), the pure helpers the tile needs (`civil`, `days_from_civil`, `parse_days`, `parse_hm`, `parse_local`, `now_unix`), and `NoCalendar` / `NoAudit` for contexts of tiles that do not use them (the usage tile's tests and the shell's usage adapter).

**Size of `CalendarSource`: 11 methods, over the ~6 to 10 asked.** `local`, `local_to_unix` (the machine's zone, so the tile does not need the agent's `Zone`), `events_in`, `alarms_in`, `add_event`, `remove_event`, `alarms`, `add_alarm`, `remove_alarm`, `set_alarm_enabled`, `next_alarm`. Two of them are the zone. I did not force it down further: merging add/remove would hide meaning. The audit log is a separate one-method trait.

**One cost, said plainly: duplicated pure code.** The tile crate cannot call the agent's `parse_days`, `parse_hm`, `parse_local`, `civil`, `days_from_civil`, and the facade holds copies (about 90 lines), as `Repeat`/`Alarm` shapes are copied. The shell test `the_tile_crates_parsers_agree_with_the_agents` compares the copies with the agent's on tables (times, day lists, dates including leap days and daylight-saving gaps, 11 civil values, repeat and alarm labels), so they cannot drift silently. The cleaner end state is to move those pure helpers into a small crate both depend on; not done now.

**Shell side** (`apps/calendar.rs`, 248 lines): `ShellCalendar { store, zone }` implements `CalendarSource` (field-copy conversions), `ShellAudit` implements `AuditSink`, and `CalendarApp` wraps the tile crate's app plus the `Zone::system()` read once at open, as before. `shell.rs` is unchanged: `crate::apps::calendar::CalendarApp::new(&cx)` still works.

**Tests.** In the tile crate (fake `CalendarSource` in UTC with in-memory events and alarms, fake audit): `moving_around_the_month_selects_days_and_changes_months_correctly`, `an_event_is_added_with_the_form_shows_in_the_plan_repeats_and_is_deleted_after_asking`, `the_calendar_can_show_or_hide_the_alarm_schedule_and_alarms_are_set_switched_and_deleted_on_the_clock_tab` (same steps and assertions as before, except that "stored" is read from the fake and the test no longer drives a shell `TileHarness`; it draws at four sizes instead of the harness's standard sizes), and the ignored `calendar_picture`. In the shell: `the_shell_adapter_connects_the_tile_to_the_real_calendar_and_audit_log` and the parity test above. The fake does not reproduce the real store's monthly/yearly repeat or daylight saving; the agent's own `calendar::tests` still cover that. The 3 FEATURES.md-cited calendar tests and the 3 cited `ide_chrome` tests moved: `COMPILE-SPLIT-STEP3-RENAMES.tsv` (6 lines, not applied, so `feature_matrix` fails until it is). The cited `filepicker_tests::*` paths did not change.

**Numbers** (dev profile unless stated, warm, one cargo command at a time, `CARGO_BUILD_JOBS=3`, under a 9 GB cap; the edit is one colour constant; a machine shared with another agent, so noisy; two runs each where cheap):

| After a one-line code change in | Command | Seconds |
|---|---|---|
| the tile crate `iyakki-tile-calendar` | `cargo check -p iyakki-tile-calendar` | 0.8, 0.2 |
| the tile crate | `cargo test -p iyakki-tile-calendar --no-run` | 0.3, 0.3 |
| a tile still in the shell (`apps/mesh.rs`) | `cargo build -p iyakki-shell` | 5.0, 4.4 |
| a tile still in the shell | `cargo test -p iyakki-shell --lib --no-run` | 10.8, 9.8 |
| the tile crate `iyakki-tile-calendar` | `cargo build --profile device -p iyakki-core` | 5.0 (one run) |
| a tile still in the shell (`apps/mesh.rs`) | `cargo build --profile device -p iyakki-core` | 34.6 (one run) |

For the real push loop (the `device` profile), a change inside the calendar tile crate costs 5.0 s against 34.6 s for the same change in a tile that is still in the shell: the split helps the loop, about 7 times here. The calendar edit was measured first, then the mesh one; both single runs. The earlier shell figure of about 30 s (Second step) was a dev build before this step; the dev figures here (4-5 s) are much lower, which I cannot fully explain (the warm incremental cache after the long device build, less contention); do not read the dev rows as a gain from this step. Not measured: cold builds (the first `device` build after the move took 349 s), release/LTO, other tiles.

**Verified:** `cargo test -p iyakki-uikit` (128 passed, includes the moved widgets and ide_chrome tests); `cargo test -p iyakki-tile-calendar` (3 passed, 1 ignored); `cargo test -p iyakki-shell --lib -- apps::calendar widgets blink filepicker apps::usage` (18 passed); `cargo build -p iyakki-shell`; `cargo build --profile device -p iyakki-core`; the ratchet results are in the final report. **Not verified:** a run on a device or QEMU (not allowed), the screenshot test, the whole shell suite, a hand look at the calendar on a screen.

### Next tiles after this step

| Rank | Tile | What it needs now |
|---|---|---|
| 1 | mesh (853 lines) | `MeshSource` (consul client calls; `secrets`, `quick_upstream`, `system_set`, `dir`). `widgets` and `blink` are unblocked. |
| 2 | learn (575 + 536 + 446) | `CalendarSource` exists; still `StudySource`, `Speaker`, `chatpic::PicCache`, `story::{Synth, Sink}`. |
| 3 | lab (1 671 + 3 files) | image-generation source; `multiline`, `chatpic`, `browser_images`. `widgets`, `ide_chrome` and the picker are unblocked. |
| 4 | inbox | 13 services; later. |

With calendar the extracted tiles are two; at more than five, `TileCx` becomes a trait (owner decision).
