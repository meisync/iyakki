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
