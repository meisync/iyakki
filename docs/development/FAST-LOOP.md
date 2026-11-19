# The fast loop: build and test only what changed

Owner request 2026-11-14: development and testing must be faster. This page is the daily loop; the full gate (`./run.sh alpha-check`, about 50 minutes) is only the release check.

## Measured on the dev PC (16 cores, warm cache, 2026-11-14)

| What you do | Time |
|---|---|
| Change one line in a tile crate, build and test it (`cargo test -p iyakki-tile-radio`) | **2 s** |
| Change one line in an app file inside the shell crate, rebuild the shell's tests (`--no-run`) | **12 s** |
| Run only that app's tests in the shell crate (`-- apps::radio`, 14 tests) | **6 s** |
| Same edit, build and run in one go | **18 s** |
| The first build after a version bump or in a fresh clone (every workspace crate recompiles) | **2 to 10 minutes** |
| All shell tests (about 3,500) | **about 25 minutes** |

So the loop is already fast when the cache is warm and the tests are filtered. The slow cases are cold builds and full test runs.

## The loop

```
dist/scripts/test-affected.py --quick --run     # only the crates you changed; an edit under crates/iyakki-apps/src/apps/<app> runs only that app's tests
dist/scripts/test-affected.py --list            # what a change can break: the crates that depend on it, by layer (base, core, agent, ui, apps, tools)
dist/scripts/test-affected.py --run             # the changed crates and everything that depends on them, plus the contract tests when a base or core crate changed
./run.sh test-core                              # the core layers' tests (crates of the layers base, core, agent, tools)
./run.sh test-apps                              # the apps and ui layers' tests (the tile crates, iyakki-apps, the shell, the UI SDK, the sample app)
./run.sh test-affected --list                   # the same as dist/scripts/test-affected.py
./run.sh alpha-check                            # the full gate, before a release
```

`--base REV` compares with another commit (default: the one before HEAD; uncommitted and new files always count). The layer of each crate is in
`dist/scripts/crate-layers.txt`.

## Keeping the build warm (what to do, what not to do)

- **Do not change the version while developing.** `0.1.0-alpha.4-dev` stays until the next release: a version change recompiles every workspace crate.
- **Keep one warm target folder per machine** (`CARGO_TARGET_DIR`), shared by the jobs of one person; do not delete it after each task. A cold build of the shell and agent crates is minutes, a warm one seconds.
- **Do not run the same crate with different flags in the same folder** (a different `RUSTFLAGS`, `--release` against dev): each change throws the cache away.
- Gates and agents on shared machines: one cargo at a time, but never an empty target folder to start with.

## The apps split (alpha.4, phase 1): measured before and after

The apps (`apps/`, 394 files) and the device helpers they use (input, display, camera, touch, hostname, agent login ...) moved from `iyakki-shell` into the new crate `iyakki-apps`. The shell depends on it and re-exports every module under its old path. An app change builds and tests `iyakki-apps` only; a shell change no longer rebuilds or tests the apps.

Measured on amd-mini (16 cores, 59 GB, shared; `CARGO_BUILD_JOBS=8`; one cargo at a time unless noted; dev test profile; before = release/alpha.4 at 5952b400, after = branch `alpha.4/apps-split`; each in its own target folder, warm unless "cold"):

| What | Before (everything in `iyakki-shell`) | After |
|---|---|---|
| (a) one-line edit in `apps/radio.rs`, build the tests (`--no-run`) | 11.8 s (`-p iyakki-shell`) | 10.8 s (`-p iyakki-apps`) |
| (b) the same, then run that app's tests (`-- apps::radio`) | 11.8 + 5.2 = 17.0 s | 10.8 + 5.4 = 16.2 s |
| (b2) the same edit, build the shell's tests too (`-p iyakki-shell --no-run`) | (same crate as a) | 7.5 s (apps library again, shell tests relinked) |
| (c) one-line edit in `shell.rs`, build the shell's tests | 12.0 s (all apps' tests too) | 7.3 s; `iyakki-apps` stays at 0.2 s (nothing to build) |
| (d) full test pass, one crate at a time | shell lib: 1655 s (3,512 tests) | apps lib 907 s (2,789 tests) + shell lib 763 s (727 tests) = 1,670 s |
| (d2) both crates at the same time | not possible | 1,204 s wall (apps 1,203 s, shell 864 s), 27 % less than before |
| (e) cold build of the tests (empty target folder, deps included) | shell lib 155 s | shell lib 130 s + apps lib tests 54 s |

What this says, honestly: the one-line app edit (a, b) is not faster, because the test binary of the apps is nearly all of the old shell test binary (the app files were most of the crate). What changed is the other side: a shell edit builds 40 % faster and runs 727 tests instead of 3,512 (a shell-only change is about 13 minutes of tests instead of 28), an app change never runs the shell's tests, and the two suites can run on two machines or in two terminals at once (wall time is the slower of the two, about 15 minutes alone). The sum of both suites is unchanged. The next step to make (a) fast is to split the apps crate itself (the biggest apps into crates of their own, like `iyakki-tile-radio`).

Notes: 4 tests were added (`apps_split_tests.rs`: tile table and group checks, capture sign, device popover wiring, chat mouse drag) because those checks need the shell. Under heavy parallel load two tests flake that pass alone: `shell::perm_tests::a_client_devices_question...` (20 s wait while 16 tests draw) and a browser test with the fake engine helper (`apps::browser::tests::...`); the same load pattern was seen in the notes tests the task mentioned.

## Deploy path: edit to running (2026-11-15, `iyakki dev push-apps`, [UI-BOUNDARY.md](UI-BOUNDARY.md))

Goal (owner): an app-only change runs on the Lenovo in under a minute. **Not reached, and not measured end to end**: no device was available in this round and no pack was built.

What was measured (dev PC, `cargo test -p iyakki-agent --lib cli_push_apps -- --ignored --nocapture`, dev test profile, so slower than a release `iyakki`):

| Step | Result | Basis |
|---|---|---|
| Pack of a 40 MB program with an inserted 48 bytes and 40 changed bytes, as a binary delta against the old program | 6 KB, 6.2 s | synthetic bytes, not a real recompiled `iyakki` (real binaries shift more: the delta size on a real rebuild is unmeasured) |
| Same step with the whole file | not allowed: a patch file is at most 64 MB, so the first push needs the old program (`--against`) | patch limits |
| Warm one-line change, device-profile build of the core program (no LTO) | about 20 s | quoted from F-BOOT-39 (COMPILE-SPEED-PLAN.md), not re-run here |
| Apply on the device | not measured | needs a device; the approval card waits for the person there |
| Restart of the device | 1 to 2 minutes (alpha.4 plan, 2026-11-14) | measured earlier on hardware |

So today the path is: build (about 20 s) + delta (seconds) + send + the approval on the device + a **device restart** (1 to 2 minutes), because the shell is a thread of the core program (PID 1). Under a minute needs the process split (`iyakki-ui`, restarted by the core without the device); until then the best case is about 2 to 3 minutes, against 5 to 6 for a full program push (2026-11-14). The `ui` layer rules are ready for it; see UI-BOUNDARY.md section 5 for what remains.

## `./run.sh fast`: the one-command loop (2026-11-15, alpha.5, F-SPLIT-05)

```
flock ~/iyakki-work/.cargo.lock env CARGO_TARGET_DIR=... ./run.sh fast [--device NAME] [--no-push] [--no-test] [--dry] [--base REV]
```

Steps, each timed in the table it prints, stopping at the first failure: **detect** (changed files, the layer verdict) -> **build** (`cargo build --profile device --target x86_64-unknown-linux-musl -p iyakki-core --bin iyakki`) ->
**test** (`test-affected.py --quick --run`) -> **push** (`iyakki dev push-apps --layer VERDICT`, the approval card on the device). The verdict is printed before anything is built or sent:

| Verdict | Meaning | What the loop does |
|---|---|---|
| `live-ui` | only ui/apps crates changed | builds, tests, pushes; no device restart once live apply is on (see LIVE-RESTART.md) |
| `live-core` | core/base crates, Cargo files | the same; the core program is swapped by the in-place re-exec |
| `reboot` | kernel, modules, boot files, first stage, image recipe (`DEVICE_LAYER` in `fast-loop.py`) | stops, names `quick-push.sh` / `nightly-push.sh`, `iyakki dev update`, `iyakki dev reboot` |
| `nothing` | docs and scripts | nothing |

Measured on the dev PC, 2026-11-15, one-line edit in `crates/iyakki-apps/src/apps/radio.rs`, `--no-push` (no device access in this round), other agents working on the same machine:

| Run | detect | build (musl, device profile) | test (radio's 14 tests) | total before the push |
|---|---|---|---|---|
| first run in this worktree (cold: the workspace crates of this path had not been built) | 0.4 s | 168 s | 129 s | 298 s |
| second edit, warm | 0.5 s | **11.2 s** | **17.6 s** | **29.3 s** |

Push step: **not measured** (needs a device). From the earlier numbers: a binary delta of a real program is seconds (6.2 s for the synthetic 40 MB one, a real rebuild unmeasured), the approval card waits for a
person, apply and the restart of the program are not measured. With the live apply built and switched on, the target is under 30 s from the push; today's run shows the part before the push alone takes
29 s on a warm cache, so under 30 s from the edit is **not** reached, under 30 s from the push is unproven. The QEMU part of the plan (boot a real image, push, re-exec) was not run: the re-exec is not wired
into PID 1 yet ([LIVE-RESTART.md](LIVE-RESTART.md), section 4).

## Still to measure (not done yet)

- A faster linker (`lld` is installed on the dev PC) for the dev and test profiles: the test binaries are large, so linking may dominate. Needs a measured before and after in a separate target folder.

## `iyakki dev loop`: watch, test, push, reopen (2026-11-16, F-SPLIT-08)

```
iyakki dev loop [--repo DIR] [--device NAME] [--reopen APP] [--base REV] [--no-test] [--apply] [--fetch CMD] [--once]
```

It watches the repository (not `target/` or `.git`), waits 0.4 s for a burst of saves to end, then runs one pass of `dist/scripts/fast-loop.py` with the options you gave
(detect the layer, build the program, run the quick tests of the change beside the build, push with `push-apps --layer`, and with `--apply` ask the running program to
replace itself in place through `dev shell -- iyakki live-restart`), opens the app on the device again (`--reopen APP`, or the app that the changed files name:
`apps/radio.rs` is `radio`, `iyakki-tile-pdf` is `pdf`) and prints one line with the time **from the edit** (the newest modified time of the changed files) to the end:

```
loop: OK edit -> done 41.2 s (settle and detect 2.3 s, steps 38.9 s)
```

On a build host behind a `cargo` wrapper (this lab: the AMD mini through `rcargo`) put the wrapper first on `PATH`, set the remote incremental switch and name the copy-back command:

```
PATH=~/iyakki-work/ag-q-layers-shim:$PATH IYAKKI_REMOTE_INCREMENTAL=1 CARGO_TARGET_DIR=~/iyakki-work/loop-lenovo/tgt \
  iyakki dev loop --device lenovo-t7 --fetch 'scp -q HOST:PATH/x86_64-unknown-linux-musl/device/iyakki "$FAST_BIN"'
```

(`CARGO_INCREMENTAL=0`, which `rcargo` sets by default, switches the device profile's incremental build off; the 18 s warm build above needs it on.)

### What was tried on the Lenovo, and what stops the no-restart path (2026-11-16)

Device `lenovo-t7` (paired, signed in, Home screen, image alpha.5-dev built 2026-11-16, uptime 5.5 h). The program of that image was taken out of
`packs/alpha5c/boot/ROOT.SFS` (sha256 0815e59d..., 119,415,688 bytes) and the new program built from this tree (123 MB, device profile, musl) was sent with
`dev push-apps --against` (client side: 24 MB delta in 32 s of CPU).

1. **The image on the device has no live restart.** The text `live-restart.enabled` is not in its `/usr/bin/iyakki`; `dev files stat /etc/iyakki/live-restart.enabled` says it does not exist. The wiring is in this tree (`main.rs` calls `liverestart::begin`, `serve_once`, `watchdog`, the vault key hand-over), so the **first** program that carries it has to be installed the old way (stored for the next start, one restart). Only after that can a pushed program replace the running one.
2. **The push needs a person.** `push-apps` waits for the administrator to approve patching the system on the device's screen; nobody did within the two minutes (`nobody approved it in time`). The approval card is not answered by the model (the device test runner `dist/devtest/run.py` states the same rule; a person answers the card), so the measurement of apply and re-exec on real hardware is **not done**.
3. **The flag file is off by default and nothing creates it.** `/etc/iyakki/live-restart.enabled` has to exist (`dev files` cannot write `/etc`; `dev shell -- touch ...` asks the person again). Until something sets it for a device in developer mode, `iyakki live-restart` refuses ("live restart is off on this device").
4. **Each `live-restart` is one more approval** (it goes through `dev shell`). That is by design, but it means the 1 to 2 minute target holds only while a person is at the device.
5. **QEMU.** No QEMU test of a real live restart exists. The lean images in `~/iyakki-work/images/` (pc-lean-*) predate the wiring, and a guest would need the same dev approval; building one means repacking a lean image with the new program (a 400 MB squashfs), a flag file and a console trigger. Not built.

What can run today without a device, measured above: edit to a finished, fetched program in 22 s (core) to 35 s (apps, with the tests). What is still to be measured with a person at the Lenovo: push (about 24 MB delta), apply, re-exec, first frame.

## Layered tests and green stamps (alpha.5, item 0b)

Gates 71 to 75 each failed on one test in one layer, and every fix cost a full 55-minute rerun. The workspace tests now run as two layers,
and a layer that already passed on the same inputs is not run again.

**The layers.** `core` = the crates of the layers base, core, agent and tools; `apps` = the crates of the layers apps and ui (the tile crates, `iyakki-apps`, the shell, the UI SDK, the sample app). The lists are not kept anywhere
else: they are read from `dist/scripts/crate-layers.txt` by `dist/scripts/testlayers.py` (`./run.sh test-core` prints the list it is testing).
The file has no catch-all line: a workspace crate without a layer fails `python3 dist/scripts/testlayers.py check` and the test
`cargo test -p iyakki-core --test layer_split`, so a new crate must be given its layer when it is added. Together the two layers are exactly the
workspace.

**The stamp.** After a complete, passing run of a layer, `dist/release/test-layer.sh` writes `target/alpha-check/stamps/<layer>`: the input hash
and the commit. The input hash is a SHA-256 over git's own content hashes (`git ls-files -s`, never mtimes) of
the source trees of the layer's crates and of every crate in the layers below it (apps sit on top of core), `Cargo.toml`, `Cargo.lock`, the toolchain and
cargo config files and `crate-layers.txt`; for `core` also every other tracked file except `site/`, `.github/` and `.impeccable/` (the contract tests read
FEATURES.md, BOUNDARY.md, docs and the gate scripts). The next run of the same layer prints `SKIP stamp <commit>` when the hash is the same.

A stamp is never written by:
- a failed run, or a run with a flaky test (exit 3: the test passed only alone),
- a partial run (`./run.sh test-apps -- some_filter`, which runs plain `cargo test` for the layer with your arguments),
- a tree with uncommitted or untracked files in the layer's inputs (commit first; the stamp names a commit),
- a run during which the inputs changed (the hash is taken before the tests start and compared when they end).

```
./run.sh test-core                     # run, or SKIP stamp <commit>
./run.sh test-apps --no-stamps         # run even when a stamp matches
./run.sh alpha-check --layer apps      # rerun ONLY the layer that failed in the gate (no builds, no licence checks, only that layer's tests)
./run.sh alpha-check --no-stamps       # the whole gate, every layer really run
./run.sh release-check --status        # read the stamps: is every layer green at this tree? (exit 1 if not)
./run.sh release-check                 # the release-tag gate: alpha-check --release
```

**The release tag.** `./run.sh release-check` is `alpha-check --release`: it refuses `--dry`, `--no-tests` and `--layer`, runs every layer
that has no valid stamp, and ends with the row `every test layer green at this tree`, which lists each layer as `GREEN stamp <commit>` (by
a run now or an earlier run on the same inputs) or `NOT GREEN`. `./run.sh release` (dist/release/release.sh) refuses to collect a release unless the
same status is green (`IYAKKI_RELEASE_SKIP_LAYER_CHECK=1` is for emergencies and should be said out loud). Because a stamp needs a clean, committed tree,
a release is always tested at the tagged tree.

**Strict stamps for a release.** A stamp holds two hashes. The loop and the gate use the plain one (the layer and the layers below it, as the plan says). It is not sound for the
real dependency graph: `iyakki-core` links the shell and the apps, so the plain core hash ignores an apps edit that the core tests do exercise. `IYAKKI_STAMP_STRICT=1`, which
`./run.sh release-check`, `release.sh` and `release-check --status` set, hashes every crate a layer depends on, transitively (Cargo.toml dependencies of all kinds): for the real workspace
that is 91 of 93 crates for core and all 93 for apps, so a release needs a run (or a stamp) on exactly this tree, and any edit to any crate invalidates both layers.

**Not covered.** The build, warnings, licence, QEMU and cross-check rows of the gate are not stamped: they always run (they are the fast part, or need the
images). A stamp proves "the tests passed on these inputs", not that the machine state (for example the hardware tests, or the build host) is
the same. The tests of `crates/iyakki-gui` (a separate workspace) are not part of either layer.

The stamp logic and the runner are tested on fake trees with a fake cargo: `python3 -I dist/scripts/test_testlayers.py` (also run by
`cargo test -p iyakki-core --test layer_split`).

### Measured on the build host (2026-11-16)

Measured 2026-11-16 on the build host (the AMD mini, 16 cores, 59 GB, **shared with the other agents: load average 7 to 24 during these runs**, so every
number is an upper bound), through `rcargo` (source copied there, build and tests run there, dev test profile, `CARGO_INCREMENTAL=0`), tree `alpha.5/q-layers` merged with
`release/alpha.5` at 3d18e1ef. One file touched (a comment) so that the change is real; the target folder of the worktree was warm unless "first".

| What you run | Wall time | Result |
|---|---|---|
| `./run.sh test-core --no-stamps` as measured (76 crates; the shell and the other ui crates were still in this group) | **2032 s (33.9 min)** | red: 6 real failures that are already on `release/alpha.5` (see below); no stamp |
| `./run.sh test-apps --no-stamps` as measured (17 crates, apps only; apps lib 2,913 tests took 1,086 s alone) | **1345 s (22.4 min)** | red: 2 real failures already on `release/alpha.5`; no stamp |
| *derived, not run:* with the ui layer moved to the apps group as the plan says (68 and 25 crates), the test binaries of those runs add up to about 600 s for core and about 2,350 s for apps (shell lib alone 1,132 s), plus compiling | |
| the same two in a row = the full workspace test (what `alpha-check` ran as one block before) | 3377 s (56.3 min) | |
| second `./run.sh test-core` or `test-apps` on an unchanged, committed tree with a green stamp | about 1 s (`SKIP stamp <commit>`) | only unit-tested on fake trees: no real green run exists yet |
| `test-affected --quick --run`, one line in `apps/radio.rs` (14 radio tests) | **72 s first, 60 s warm** | green |
| `test-affected --run` (no `--quick`), the same edit: iyakki-apps, iyakki-shell, iyakki-core, iyakki-remote | **2933 s (48.9 min)** | apps lib 1172 s + shell lib 1386 s + the rest; the two apps failures again. Not cheaper than the two layers: **the shell and the core depend on the apps crate**, so an apps edit makes nearly everything a dependent |
| `test-affected --run`, one line in `iyakki-core/src/liverestart.rs` (nothing depends on iyakki-core) | **130 s** | iyakki-core only plus the four contract tests; red on a failure that is already on `release/alpha.5` |
| `fast-loop.py`, one line in `apps/radio.rs`, device-profile musl build + copy back from the build host + radio tests, tests beside the build | **35 s wall** (build 18 s, copy 3 s, tests 33 s alongside); 57 s when the tests run after the build (`--serial`) | green; `--no-push`: **no device was reached, see below** |
| `fast-loop.py --no-test`, one line in `iyakki-core/src/liverestart.rs` | **22 s** (build 16.5 s, copy 3 s, detect 2 s) | |
| cold device-profile build of the program on the host (first time in a fresh target folder) | 365 s | |

The overhead of `rcargo` itself is 1.4 s a call (rsync of the source), `cargo metadata` 1.5 s, `testlayers.py hash` under 1 s.

**What this says.** The goal "only the tests of the changed layer plus dependents" works as designed for a *core* change (130 s) and for the loop (`--quick`: the
changed crate only, 60 s). It does **not** shrink an apps change in the pre-push check: the apps crate sits below the shell and the core in the dependency
graph, so a dependent run is 49 minutes. The honest loop is: `--quick` while editing, `./run.sh test-apps` (22 min, stamped) before pushing, the full gate for a release.
Making the shell and core tests independent of the apps crate (the F-SPLIT-03 process cut) is what would make an apps edit cheap.

**The layers are red on the tree they were measured on, so no real stamp was written.** These tests fail on `release/alpha.5` itself (they belong to other lanes, not to this change):
`audiosink::tests::only_the_entry_point_knows_about_the_mixer_setting` (embedmedia.rs calls `global_if_enabled(`), `webservice::tests::the_cloud_profile_offers_nothing_that_needs_local_hardware`
(`/api/v1/tiles/install mentions install`), `iyakki-core --test release_version` (`RELEASE-NOTES-0.1.0-alpha.5-dev.md` is missing), `iyakki-shell --test safeclamp_scan`
(`shell_board.rs:333`, `blur.rs:125`), `iyakki-sdk-emulator tests::a_tile_built_for_another_abi_is_refused_with_the_devices_sentence`, and in apps
`browser::tests::the_built_in_reader_lists_its_own_requests_...` and `browser::tests::the_full_engine_has_no_prompt_and_the_panel_says_so`. Also fixed here: `Cargo.lock`
of `release/alpha.5` lacked five crates, so `cargo --locked` (which the gate uses) refused to run.
