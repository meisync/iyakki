# Compile-split pilot 2: pure feature logic out of `iyakki-agent` (2026-11-09, queue item Q22)

Continues `COMPILE-SPLIT-PILOT.md` (toolkit, usage and calendar tiles). This pilot moves **pure protocol and format code** out of the
agent into three small crates, with the same two tricks as before: `pub use` shims in the agent (so `crate::camdetect::motion`,
`iyakki_agent::ssh_server` and the like keep compiling) and a TSV plus a script to re-point the `FEATURES.md` test citations.
Also: `feature_matrix` went from 554 s to 0.7 s.

## What moved

| New crate | Moved from `iyakki-agent/src` (lines) | Tests (moved with it) | Depends on |
|---|---|---|---|
| `iyakki-sshfs` (SSH, SFTP, FTP protocol code) | `ssh_server.rs` 1085, `sftp_proto.rs` 381, `sftp_fs.rs` 482, `sftp_session.rs` 251, `rf_sftp.rs` 617, `rf_ssh.rs` 686, `rf_ftp.rs` 679, `sshkey.rs` 529; new small files `b64.rs` 58, `remotefs.rs` 43 (the `Entry` / `RemoteFs` / `clean_path` that `remotefiles.rs` used to hold), `testdir.rs` + `testworld.rs` (test helpers, feature `testing`) | 47: the SFTP codec, jail and session tests (first half of `sftp_tests.rs`, now `sftp_tests.rs` 541), the client codec and loopback SSH/SFTP/FTP/FTPS tests (first half of `remotefiles_tests.rs`, now `rf_tests.rs` 875), `sshkey_tests.rs` 119, the `ssh_server` inline tests, one new `b64` test | `iyakki-sec` + aes, ctr, ed25519-dalek, sha2, zeroize, rustls, x25519-dalek, libc (all already in the agent); `rcgen` as dev-dependency (FTPS test certificate) |
| `iyakki-camdetect` (camera motion, geometry, alerts, recording format) | `camdetect/motion.rs` 776, `geom.rs` 291, `alerts.rs` 308, `person.rs` 205, `settings.rs` 219, and `camrec_format.rs` 380 | 32: the inline tests of those files and `camrec_format_tests.rs` 205 | `iyakki-sec`, `iyakki-volcrypt`, zeroize, serde, serde_json |
| `iyakki-diskcrypt` (LUKS / BitLocker header parsers, typed secret, wrong-key limiter) | `disks_crypt.rs` 290 (now the crate's `lib.rs`) | 5: the parser, hostile-header, secret and limiter tests (taken out of `disks_unlock_tests.rs`, `tests.rs` 168) | std only |

Total about 7 900 lines of code and 1 700 of tests left the agent crate. Behaviour is unchanged: the code is the old code (git sees renames), with these edits only:

* `sshkey.rs`: `b64`/`unb64` call the crate's own `b64` (a copy of the agent's `story::base64_encode` and `models::b64_decode`; the agent keeps its originals), `Reader`, `b64`, `unb64` went from `pub(crate)` to `pub` (the SFTP key store, git tools and tests in the agent use them).
* `rf_ftp.rs`: the clock call became `SystemTime::now()` (what `clock::now_unix` is), `connect_tcp` is `pub`; `rf_sftp.rs` / `rf_ftp.rs` take `Entry`, `RemoteFs`, `clean_path` from `remotefs`, which `remotefiles.rs` re-exports (`pub use iyakki_sshfs::remotefs::{clean_path, Entry, RemoteFs}`), so the manager, the shell tile and the tests are untouched.
* `person.rs`: a private `base64_encode` (same code) instead of `crate::story::base64_encode`.
* Agent shims: `lib.rs` has `pub use iyakki_sshfs::{ssh_server, sftp_fs, sftp_proto, sftp_session, rf_ftp, rf_sftp, rf_ssh, sshkey}` (one line each), `pub use iyakki_camdetect::camrec_format`, `pub use iyakki_diskcrypt as disks_crypt`; `camdetect/mod.rs` has `pub use iyakki_camdetect::{alerts, geom, motion, person, settings}`.
* The account/firewall seams already existed as traits or closures (`ssh_server::Gate`, `sftp_fs::Rule`, the audit closure of `Session`), so **no new trait or signature was needed**.

## What stayed in the agent, and why

* `sftp.rs` (548), `sftptools.rs` (315): the manager that binds the server to accounts, the vault, the firewall and the chat tools; `RealGate` and `jail_for` implement the crate's `Gate` and `Rule`.
* `remotefiles.rs` (917) and `remotefiles_tests.rs` (the second half: connection manager, vault passwords, transfers, chat tools): they use `Services`, the vault and the job runner. `NetConnector` there connects the crate's clients.
* The manager halves of `sftp_tests.rs` (471) and `remotefiles_tests.rs` (560); they use the crate's `testworld` (dev-dependency with feature `testing`).
* `camdetect/events.rs` (the encrypted event store) uses `crate::clock::{Local, Zone}`; `watch.rs` / `watch_tests.rs` use the camera hub, inbox, vault and clock. They move once `clock` has a home (it is the first row of the candidate list).
* `iyakki-camdetect` carries `camrec_format`, so it depends on `iyakki-volcrypt` (the XTS and HMAC primitives). I judged that cleaner than a fourth crate; `events.rs` still reaches it through the agent shim.
* The detect-the-header lines of two `disks_unlock_tests` tests use the agent's `detect()` and the made-up disk fixtures; they stayed as one new agent test, `the_probe_shows_encrypted_volumes_as_luks_or_bitlocker_and_plain_ones_as_not_encrypted`. The moved tests use three small local copies of the header fixtures (`sb_luks`, `sb_luks1`, `sb_bitlocker`, about 30 lines).
* Duplicated on purpose: base64 (also existing in 4 other places in the agent), the test-folder helper `testdir` (copied into `iyakki-sshfs`; the camdetect format test makes its own folder).

## Measurements

Dev profile, warm cache, one cargo at a time under `flock`, `CARGO_BUILD_JOBS=3`, 9 GB cap, a machine shared with other sessions. The edit is one appended comment line in the named file (then removed); the first two rows were also measured before the move for the moved file itself (the agent rows). Each value is one of two runs. "Old way" = `cargo test -p iyakki-agent --lib -- <filter>` (the agent test binary is 4 870 tests). The listed time includes compiling, linking and running the filtered tests; the test run itself is the last column.

| Extraction | Old way, edit in the agent (s) | New way, edit in the new crate (s) | Test run inside that |
|---|---|---|---|
| A, SSH/SFTP: edit `sftp.rs` (a stand-in for `sftp_session.rs`, same crate) | `-- sftp ssh_server`: 23.6, 23.2 | `cargo test -p iyakki-sshfs` (47 tests): 5.1, 4.9 | 4.5 s (loopback tests); compile only: `--no-run` 0.42 s, 0.42 s against 19.7 s, 19.8 s for `-p iyakki-agent --lib --no-run` |
| B, camera: edit `camdetect/motion.rs` | `-- camdetect::motion` (10 tests): 23.0, 23.1 | `cargo test -p iyakki-camdetect` (32 tests): 3.6, 3.6; `-- motion` (10 tests): 3.5, 3.5 | 3.2 s (the motion tests themselves) |
| C, disks: edit `disks_crypt.rs` / `iyakki-diskcrypt/src/lib.rs` | `-- disks_crypt`: 19.7, 19.6 (0 tests then: they were in `disks_unlock_tests`) | `cargo test -p iyakki-diskcrypt` (5 tests): 0.26, 0.22 | 0.01 s |

Reading the numbers honestly:

* The agent figure is mostly **compile + link of the agent test binary** (about 19.7 s even when nothing is run), so it does not depend on which agent file is touched; the new-crate figure is the compile of a crate that has no agent in it (0.2 to 0.4 s) plus whatever the tests themselves cost. For A and B the new way is **about 5 to 6 times faster end to end**, and the remaining time is the tests (loopback sockets, motion tests), not the compiler. For C it is about 100 times faster (0.25 s against 19.7 s).
* The edit was a comment line, so rustc's incremental pass has little to redo in either case. A code change inside a function costs more in both ways; the agent side grows with how much of the crate depends on the changed module, the new crate side stays small. I did not measure a code edit.
* Cold builds: the new crates add three compile units that build in parallel with the rest; the first build of `iyakki-sshfs` was 1.6 s after its dependencies existed. I did not measure a cold build of the workspace.

### `feature_matrix`

| | Time |
|---|---|
| `cargo test -p iyakki-core --test feature_matrix` before | 554 s (test run only, build excluded; the three tests passed, `the_checker_catches_the_mistakes_it_exists_for` took nearly all of it) |
| after | **0.68 s** test run (0.86 s wall with a warm build) |

Cause: `test_exists` built, **per `check_matrix` call**, one string of a crate's whole source (the agent is about 35 MB) and ran `contains("fn <name>(")` over it for every cited test. The meta-test calls `check_matrix` about 22 times, so the workspace was read and scanned 22 times for every cited test. Fix (`crates/iyakki-core/tests/feature_matrix.rs`): `crate_fn_names` reads each crate's `src` and `tests` once per process into a set of the names for which the text `fn <name>(` occurs (the identical match, now done once per file), kept in a process-wide `OnceLock<Mutex<..>>` and shared by all checks. Nothing about what is checked changed: the same missing-test, bad-crate, status/progress, duplicate, totals and 8-column checks run, and `the_checker_catches_the_mistakes_it_exists_for` still passes. New test `the_function_index_matches_the_text_search_it_replaced` pins the matching rule (`fn gamma (` and `fn delta<T>(` do not match, exactly as the string search did). I did not add a cache file under `target/` keyed on mtimes: after this change the scan is well under a second, and a cache file is one more thing that can go stale.

## FEATURES.md citations

`docs/development/compile-split-rename.py OLD NEW [--apply] [--tsv FILE]` rewrites every `OLD::...::name` citation whose `fn name(` is gone from `OLD` and defined in `NEW`, computing the module path from the file (including `x_tests.rs` pulled in as `x::tests` through `#[path]`). It was **applied** (not left for the owner as in the earlier steps): `COMPILE-SPLIT-PILOT-2-RENAMES-A.tsv` (45 citations), `-B.tsv` (28), `-C.tsv` (2), 75 citations in all. Every cited test still exists (`feature_matrix` passes). The old citations that were not moved (the manager tests in `sftp_tests` / `remotefiles_tests`) are unchanged. One test name that was cited also kept an agent half: none (the leftover assertions in C were put into a differently named test so the script moves the cited name).

## Gates and tests (end of run)

* `cargo test -p iyakki-core --test feature_matrix --test kernel_single_reader --test dependency_ratchet`: 4 + 2 + 7 passed. New ratchet test `the_crates_split_out_of_the_agent_depend_on_small_leaf_crates_only` (sshfs: `iyakki-sec`; camdetect: `iyakki-sec`, `iyakki-volcrypt`; diskcrypt: none).
* `cargo test -p iyakki-sshfs`: 47 passed. `-p iyakki-camdetect`: 32 passed. `-p iyakki-diskcrypt`: 5 passed.
* `cargo test -p iyakki-agent --lib -- sftp ssh_server camdetect camrec disks`: 129 passed, 1 ignored (through the shims; the moved modules' own tests now run in the new crates, so the agent count is lower than before the move). Also `-- remotefiles sshkey rf_`: all passed.
* `cargo check --workspace --all-targets` is clean for everything except `iyakki-remote`'s test build (`devname.rs`: a test `impl Net for Broken` misses `set_extra_names`; not touched here).
* `cargo test -p iyakki-shell --lib -- remotefiles cameras`: 15 passed, 1 failed: `apps::remotefiles::tests::the_shell_source_connects_pins_browses_and_downloads_inside_the_persons_file_rules` fails because `~/Downloads/hello.txt` (dated 8 Nov, left by an earlier run, the test writes into the real Downloads folder) already exists. Environment, not this change; not removed.

## Recipe for the next extraction

1. Count the module's edges: `grep -n "crate::\|super::\|iyakki_" file.rs` (and its `_tests.rs`). Edges to `std`, third-party crates already in the agent and small leaf `iyakki-*` crates are free. Each edge to another agent module is one of: (a) a **tiny helper** (base64, `now_unix`, path cleaning): copy it, 10 lines, and say so in the doc; (b) a **type shared with the glue** (`Entry`, `RemoteFs`): move it into the new crate and re-export from the old module (`pub use`); (c) **real behaviour** (vault, accounts, clock zone): a small trait in the new crate, implemented in the agent, or leave that file behind.
2. `git mv` the files into `crates/<new>/src/`, add the crate to the root `[workspace] members` and `[workspace.dependencies]`, add it to the agent's `[dependencies]`.
3. In the agent: `pub use iyakki_<new>::<module>;` in `lib.rs` (or the parent `mod.rs`) for each moved module; fix `pub(crate)` items that the glue uses (make them `pub`).
4. Split each `_tests.rs` file at the point where it starts needing accounts or `Services`. The pure half moves; helper fixtures that both halves need go into a `testworld`-style module behind a `testing` feature (agent: `[dev-dependencies] iyakki-<new> = { path = .., features = ["testing"] }`), not copied twice.
5. `cargo test -p iyakki-<new>` and `cargo test -p iyakki-agent --lib -- <module names>`; then `cargo check --workspace --all-targets`.
6. `python3 docs/development/compile-split-rename.py iyakki-agent iyakki-<new> --tsv docs/development/COMPILE-SPLIT-<step>-RENAMES.tsv --apply`, then the three gates. Add the crate to `the_crates_split_out_of_the_agent_depend_on_small_leaf_crates_only` in `dependency_ratchet.rs`.
7. Measure: append a comment to one source file, time `cargo test -p iyakki-<new>` against `cargo test -p iyakki-agent --lib -- <filter>`.

## Next candidates, ranked

Lines are non-test lines (test lines in brackets). "Edges" counts `crate::`/`super::` uses of other modules of the same crate found by a scan (so it can miss `use crate::{a, b}` groups: check by hand); "reverse" is how many agent files use the module. Ranked by independence first, then size, and by what the module unlocks.

| Rank | Module | Lines | Edges out | Reverse uses | Notes |
|---|---|---|---|---|---|
| 1 | agent `clock.rs` (+ `clockfloor.rs` 292) | 649 | none | 112 agent files, 51 uses in other crates | Foundation: `iyakki-clock` would let `camdetect/events.rs` (432), `watch.rs` (823) and most time-bound modules follow. Pure std. Its shim is the biggest, so do it by itself. |
| 2 | agent `wordtime.rs` | 1 659 | none | 4 | Largest independent module; no `iyakki_*` use. Easy. |
| 3 | agent `qr.rs` + `qr_decode.rs` | 720 + 829 (574 tests) | `qr_decode` -> `qr` only | 2 | A matching pair; `iyakki-qr`; other crates already cite them (10 uses): re-export. |
| 4 | agent `archive.rs` + `xz.rs` | 618 + 1 031 | `xz` -> `archive` (2) | 16 + 4 | `iyakki-archive`; many readers in the agent, pure shim. |
| 5 | agent `battery.rs` | 841 (632 tests) | none | 3 | Pure; 7 uses in other crates. |
| 6 | agent `bindelta.rs` | 636 (398 tests) | none | 4 | Binary delta; used by the component updates. |
| 7 | agent `deploylint.rs` (+ `dockerlint.rs` 406) | 545 + 406 | none | 7 | Lint rules; pure text. |
| 8 | shell `btmodel.rs` | 932 | `iyakki_agent`, `iyakki_devctl` | 4 | The Bluetooth page model; needs the two agent types behind a trait first. |
| 9 | shell `splash.rs` | 824 | `gfx`, `ui` (toolkit) and `iyakki_storage` | 4 | Can sit on `iyakki-uikit` once its two edges are re-pointed; first shell split that helps the whole shell build. |
| 10 | agent `browserjobs.rs` | 685 | none in the crate; `iyakki_web`, `iyakki_webview`, `iyakki_sec` | 12 | Larger interface (12 users); do after the `Services` seam is clearer. |

Also near the top of the scan and worth a look: agent `macrecovery.rs` 529 (577 tests, `iyakki_drivers`, `iyakki_proxy`), `vpn_dns.rs` 502, `regex.rs` 437, `openpgp.rs` 415, `consul.rs` 376, shell `accounts.rs` 712, `osk.rs` 519, `panes.rs` 576, `touchpad.rs` 302 + `touchmap.rs` 343. The shell's `apps/` folder is 164 000 lines; its tiles follow the tile-crate route of `COMPILE-SPLIT-PILOT.md`, not this one.

## Not done

* Cold-build and release/LTO timings; measuring a real code edit instead of a comment; the `iyakki-remote` test build (an unrelated failure noted above).
* `events.rs`, `watch.rs`, `sftp.rs`, `sftptools.rs`, `remotefiles.rs`: need clock, services and the vault (see above).
* Nothing was committed, no image was built, no device or k3s was touched.

## Continued in Pilot 3

See `COMPILE-SPLIT-PILOT-3.md`: clock, camera events, wordtime, QR, archive/xz, bindelta, lints and battery moved; updated candidate list there.
