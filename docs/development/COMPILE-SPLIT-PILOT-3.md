# Compile-split pilot 3: clock, camera events, word timing, QR, archives, delta, lints, battery (2026-11-09, queue item Q22)

Continues `COMPILE-SPLIT-PILOT-2.md` (same recipe: move code and its tests, `pub use` shim in the agent, TSV plus `compile-split-rename.py`
for the `FEATURES.md` citations). No behaviour change; no new external dependency (the new crates use crates the agent already had).

## What moved

| New crate | From `iyakki-agent/src` (lines) | Tests | Depends on |
|---|---|---|---|
| `iyakki-clock` | `clock.rs` 649 (the crate's `lib.rs`) | 8 | std |
| `iyakki-camdetect` (existing) | `camdetect/events.rs` 437 (encrypted event store) | +5 (37 in all) | + `iyakki-clock` |
| `iyakki-wordtime` | `wordtime.rs` 1 659 | 38 | std |
| `iyakki-qr` | `qr.rs` 720, `qr_decode.rs` 829, `qr_decode_tests.rs` 574 | 31 (1 ignored) | std |
| `iyakki-archive` | `archive.rs` 621, `xz.rs` 1 032 | 13 (1 ignored) | miniz_oxide, ruzstd |
| `iyakki-bindelta` | `bindelta.rs` 636, `bindelta_tests.rs` 187, `bindelta_stream_tests.rs` 211 | 13 (1 ignored) | sha2, ruzstd |
| `iyakki-lint` | `deploylint.rs` 545, `dockerlint.rs` 406 | 10 | serde_json |
| `iyakki-battery` | `battery.rs` 841 incl. its inline part (the crate's `lib.rs`), `battery_tests.rs` 632 | 33 | serde (dev: serde_json) |

About 9 900 lines. Shims in the agent `lib.rs`: `pub use iyakki_clock as clock;`, `iyakki_wordtime as wordtime`, `iyakki_battery as battery`,
`pub use iyakki_qr::{qr, qr_decode}`, `iyakki_archive::xz`, `iyakki_bindelta::bindelta`, `iyakki_lint::{deploylint, dockerlint}`; `archive` is
`pub mod archive { pub use iyakki_archive::archive::*; }` (see below); `camdetect/mod.rs` re-exports `events`. All 112 `crate::clock::...` users compile unchanged.

Edits to the moved code, besides imports:

* `events.rs`: `crate::clock::{Local, Zone}` became `iyakki_clock::`; the test folder helper (`crate::testdir::unique`) became a 6-line local copy.
* `xz.rs`: `make_xz_stored` (a test fixture the agent's `clis.rs` and `linuxapps_tests.rs` call) lost its `#[cfg(test)]` and is `pub` + `#[doc(hidden)]`,
  because a `cfg(test)` item of another crate is invisible to the agent's tests.
* `archive.rs`: the `tests` module (fixtures `tmp`, `make_tar`, `gzip`, `make_zip`, used by 8 agent test files as `crate::archive::tests::...`) is
  compiled under `cfg(any(test, feature = "testing"))`; the agent has `iyakki-archive` as dev-dependency with `testing`, and the glob re-export brings
  `archive::tests` back. Normal builds do not see the module.

## What stayed, and why

* `clockfloor.rs` (292): calls `crate::ntp::set_clock` (two places); moves with `ntp` or behind a closure later. Not needed for `clock`.
* `camdetect/watch.rs` (823) and `watch_tests.rs` (683): use the camera hub (`FrameTap`, `Hub`, `Spec`, `Who`, `can_view`, `shrink`), `Services`
  (inbox notices), `camrec` and the vault. That is five edges to behaviour, not one clean seam; skipped on purpose (needs a `Hub` trait first).
* `battery.rs` "pure parts": the whole file was already pure, so all of it moved.

## Measurements

Dev profile, one cargo at a time, timed inside `flock` (waiting for other sessions is not counted), edit = one appended comment line, two runs each.
"Old way" = `cargo test -p iyakki-agent --lib -- clock` after touching an agent file (the agent test binary is about 4 700 tests): **21.8 s, 20.0 s**
(compile + link + run of the filtered tests). New way, same edit in the new crate, `cargo test -p <crate>` (compile + link + all of the crate's tests):

| Crate | Seconds (2 runs) | Faster than 20 s by |
|---|---|---|
| iyakki-clock | 0.3, 0.2 | about 80x |
| iyakki-wordtime | 0.3, 0.3 | about 70x |
| iyakki-lint | 0.3, 0.2 | about 80x |
| iyakki-battery | 0.3, 0.3 | about 70x |
| iyakki-archive | 0.8, 0.7 | about 27x |
| iyakki-qr | 1.0, 0.9 | about 21x |
| iyakki-bindelta | 2.0, 1.9 (streaming tests) | about 10x |
| iyakki-camdetect | 3.7, 3.7 (the motion tests dominate) | about 5x |

Honest caveats: comment-only edits (as in Pilot 2); the old way is a floor too, since the agent unit has to be recompiled for any real edit (see below).
An agent edit after a rebuild of its dependencies costs more (a first run in this session took 74 s and 121 s while other sessions were building).

### Where the agent spends its compile time

`cargo build -p iyakki-agent --lib --timings` in the dev profile, cold, into its own target folder
(`~/iyakki-work/timings/target/cargo-timings/`, log `~/iyakki-work/timings/build.log`): the whole build was 1 min 51 s; the **agent is one unit of
63.5 s (frontend 47.4 s, codegen 16.1 s)**, followed by cranelift-codegen 15.4 s, iyakki-flow 6.3 s, wasmtime-environ 5.8 s. Cargo's report is per crate, not per
module, and per-module numbers need nightly `-Zself-profile` plus a decoder we do not have, so the ranking of modules below is a **proxy**
(lines, `json!` calls, serde derives, `format!` count), not a measurement:

1. `agent.rs` (3 643 lines, 325 `json!`), 2. `k8stools.rs` (2 760, 160 `json!`), 3. `providers.rs` (1 861, 83 `json!`, serde), 4. `webservice/openapi.rs`
(173 lines but 92 `json!`: large generated literals), 5. `story.rs` (4 982, the largest file). The `json!` macro is the usual cause of a slow
frontend; replacing big `json!` literals by builder helpers or `const` JSON strings in those files is the cheap experiment. The real answer is a
`-Zself-profile` run on a nightly toolchain (not installed here). The 47 s frontend share also says: splitting more out of the agent pays off by
**shrinking this single unit**, so prefer moving the largest independent files (below) over small ones.

## Gates and tests

* `cargo test -p` iyakki-clock 8, iyakki-camdetect 37, iyakki-wordtime 38, iyakki-qr 30 + 1 ignored, iyakki-archive 12 + 1 ignored, iyakki-bindelta 12 + 1 ignored, iyakki-lint 10, iyakki-battery 33: all pass.
* `cargo test -p iyakki-agent --lib -- clock wordtime qr archive xz bindelta deploylint dockerlint battery camdetect` (through the shims): 57 passed.
  Run once after all extractions (not after each: a full agent test build is 1 to 2 minutes), each crate was tested alone after its own move.
* `cargo check --workspace --all-targets`: clean (the `iyakki-remote` test-build failure of Pilot 2 is gone, another session fixed it).
* `cargo test -p iyakki-core --test feature_matrix --test kernel_single_reader --test dependency_ratchet`: 4 + 2 + 7 passed. The ratchet
  `the_crates_split_out_of_the_agent_depend_on_small_leaf_crates_only` now lists all ten split crates (camdetect may also use `iyakki-clock`; the
  new ones depend on no `iyakki-*` crate).
* Citations: `COMPILE-SPLIT-PILOT-3-RENAMES-{clock,events,wordtime,qr,archive,bindelta,lint,battery}.tsv` (8, 4, 35, 21, 10, 7, 10, 16 = 111 citations).
  `iyakki-battery` tests are cited as `iyakki-battery::tests::...` (the TSV had `battery_tests::`, corrected in both places).

## Next candidates, ranked (updated)

| Rank | Module | Lines | Notes |
|---|---|---|---|
| 1 | agent `clockfloor.rs` | 292 | next to `ntp::set_clock`; a closure parameter removes the edge |
| 2 | agent `camdetect/watch.rs` + `watch_tests.rs` | 823 + 683 | needs a hub/inbox trait seam (`FrameTap`, `Hub`, `Services::notify`); worth it, riskier |
| 3 | agent `macrecovery.rs` | 529 (577 tests) | `iyakki_drivers`, `iyakki_proxy` edges |
| 4 | agent `vpn_dns.rs`, `regex.rs`, `openpgp.rs`, `consul.rs` | 502, 437, 415, 376 | scan each for `crate::` edges first |
| 5 | agent `story.rs` (4 982), `agent.rs` (3 643), `k8stools.rs` (2 760) | | the biggest frontend cost (proxy), but tied to `Services`; need the Services seam |
| 6 | shell `btmodel.rs` 932, `splash.rs` 824, `accounts.rs` 712, `osk.rs` 519, `panes.rs` 576, `touchpad.rs`+`touchmap.rs` | | as in Pilot 2 (Bluetooth files are being edited by another session now) |
| 7 | agent `browserjobs.rs` | 685 | 12 users, after the Services seam |

Done from the Pilot 2 list: ranks 1 to 7 (clock, wordtime, qr, archive+xz, battery, bindelta, lint).

## Not done

* Per-module compile times (needs nightly self-profile); cold or release timings of the new crates; measuring a code edit instead of a comment.
* `watch.rs`/`watch_tests.rs`, `clockfloor.rs` (see above).
* Nothing was committed, no image was built, no device or k3s was touched.
