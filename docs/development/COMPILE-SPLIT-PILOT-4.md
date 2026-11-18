# Compile-split pilot 4: clock floor, macOS Recovery client, regex, OpenPGP (2026-11-09)

Continues `COMPILE-SPLIT-PILOT-3.md` (same recipe: move code and tests, `pub use` shim in the agent, TSV plus `compile-split-rename.py`
for the `FEATURES.md` citations). No behaviour change; no new external dependency.

## What moved

| New crate | From `iyakki-agent/src` (lines) | Tests | Depends on |
|---|---|---|---|
| `iyakki-clockfloor` | `clockfloor.rs` 292 (the crate's `lib.rs`) | 6 | `iyakki-clock`, log |
| `iyakki-macrecovery` | `macrecovery.rs` 529, `macrecovery_tests.rs` 577 | 26 (2 ignored) | `iyakki-proxy`, `iyakki-drivers`, sha2 |
| `iyakki-regex` | `regex.rs` 437 | 4 | std |
| `iyakki-openpgp` | `openpgp.rs` 415, `openpgp_tests.rs` 73 | 8 | sha2 |

About 2 300 lines. Shims in the agent `lib.rs`: `pub use iyakki_macrecovery as macrecovery;`, `iyakki_regex as regex`, `iyakki_openpgp as openpgp`,
and `pub mod clockfloor` (below). All callers (`macfw`, `camfw`, `fwpack`, `textcmd`, `browserheaders`, `clis`, `ntp`, `authcodes`, `startup_probe`,
`k3sdev`, and `iyakki-core`'s `boot.rs`) compile unchanged.

Edits to the moved code, besides imports:

* `clockfloor.rs` (rank 1, the closure seam): the two boot steps called `crate::ntp::set_clock`. In the crate, `apply_early(set)` and
  `apply_after_data(on_disk, set)` take the setter (`&mut dyn FnMut(i64) -> Result<(), String>`, the type `apply` already used).
  The agent's `pub mod clockfloor { pub use iyakki_clockfloor::*; ... }` keeps the old zero-setter signatures as two small wrappers that pass
  `|t| ntp::set_clock(t as f64)`, so `boot.rs` did not change. `crate::clock::now_unix` became `iyakki_clock::now_unix`.
* `openpgp`: the test fixtures (`tests/fixtures/openpgp/`, 5 small files) are copied into the new crate because `clis.rs` in the agent still
  includes them for its AWS CLI signature test. Two copies of 32 KB of throw-away test keys and signatures.
* Nothing else changed in the code. The dependency ratchet allows `iyakki-macrecovery` the two crates it already used (`iyakki-proxy` for the
  `Streamer` trait and the real HTTPS client in its tests, `iyakki-drivers` for `macboard::valid_board_id`); neither depends on the agent.

## What stayed, and why

* Rank 2, `camdetect/watch.rs` and `watch_tests.rs` (823 + 683): needs the hub/inbox trait seam; not attempted (riskier, see Pilot 3).
* Rank 4 remainder: `vpn_dns.rs` (502 + 239 tests, only an `iyakki_vpn::dnsroute` edge) and `consul.rs` (376, `iyakki_proxy::Upstream`) have no
  `crate::` edges and are the next easy moves. Not done here (the task asked for 3 to 4 entries).
* Rank 3 `macrecovery` is done; ranks 5 to 7 (`story`, `agent`, `k8stools`, shell files, `browserjobs`) still need the Services seam.

## Measurements

Dev profile, one cargo at a time inside `flock` (waiting for other sessions not counted), edit = one appended comment line, two runs.
"Old way" = `cargo test -p iyakki-agent --lib -- clockfloor macrecovery regex:: openpgp` after touching an agent file: **21.1 s** (compile +
link + run of the 44 filtered tests; the agent test binary is about 4 700 tests). "New way" = same edit in the new crate, `cargo test -p <crate>`
(compile + link + all the crate's tests, including the integration test of the macrecovery real-client test against a local socket):

| Crate | Seconds (2 runs) | Faster than 21 s by |
|---|---|---|
| iyakki-clockfloor | 0.20, 0.21 | about 100x |
| iyakki-regex | 0.24, 0.23 | about 90x |
| iyakki-openpgp | 0.31, 0.30 | about 70x |
| iyakki-macrecovery | 0.56, 0.56 | about 38x |

The same caveats as Pilot 3 apply (comment-only edits; the old way is a floor since a real agent edit recompiles the whole unit; the agent's single
compile unit shrinks by 2 300 lines only, a few percent).

## Gates and tests

* `cargo test -p` iyakki-clockfloor 6, iyakki-macrecovery 26 + 2 ignored, iyakki-openpgp 8, iyakki-regex 4: all pass.
* Ratchet `the_crates_split_out_of_the_agent_depend_on_small_leaf_crates_only` now lists 14 crates (the four new ones with their allowed deps).
* Citations: `COMPILE-SPLIT-PILOT-4-RENAMES-{clockfloor,macrecovery,regex,openpgp}.tsv` (5, 15, 4, 6 = 30 `FEATURES.md` citations). The macrecovery and
  openpgp test modules are `tests` (pulled in by `#[path]`), so they are cited as `iyakki-<crate>::tests::...` (the script wrote
  `..._tests::`; corrected in `FEATURES.md` and both TSVs).
* `cargo test -p iyakki-agent --lib -- clockfloor macrecovery regex openpgp macfw camfw fwpack textcmd browserheaders authcodes clis::` (through the
  shims): 133 passed, 0 failed, 1 ignored.
* `cargo test -p iyakki-core --test feature_matrix --test dependency_ratchet`: 4 + 7 passed. `kernel_single_reader`: its second test passes; the first
  failed with one new line that is not from this pilot (`iyakki-shell/src/camera/v4l2.rs now reads /sys/devices`, another session's file). None of
  the four moved files adds or removes an allowed kernel path, and no allowance was raised.
* `cargo check --workspace --all-targets`: the only errors are in `iyakki-shell` (lib test), `iyakki_wake` unresolved (another session's wake-phrase
  work in progress); every crate touched here compiles.

## Not done

* `watch.rs`, `vpn_dns.rs`, `consul.rs`; cold or release timings; per-module compile profile (needs nightly self-profile).
* Nothing was committed, no device or k3s touched.
