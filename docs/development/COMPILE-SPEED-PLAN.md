# Compile speed: fixing or changing one app must take under a minute (plan, not built)

Status: plan written 2026-11-07 after the owner asked that a one-app change compile in less than a minute. Nothing here is built yet. It complements [`UI-OS-SPLIT-PLAN.md`](UI-OS-SPLIT-PLAN.md), which splits the code by *layer* (apps, core, kernel) for testing and a cloud UI; this plan splits it by *compile unit* and builds the quick loop. The two share most of the moves, so do them together.

## The goal, as numbers

| Loop | Today (measured 2026-11-07 unless marked) | Budget |
|---|---|---|
| Change one line in one tile, `cargo check` of that tile | not measured | **under 10 s** |
| The same, run that tile's tests | not measured (the shell test binary links the whole agent; the full shell run takes about 32 minutes) | **under 30 s** |
| The same, a runnable binary on the dev PC | not measured | **under 45 s** |
| The same, on the device (build + push + the tile restarts) | **15 min** (8 m 50 s release compile of the changed crates, 3 m 49 s a second compile of proxy and agent, about 2 min repack and signing, 16 s lean image, then a 420 MB upload) | **under 60 s** |

The first three are about compile units. The last needs a different way of deploying too (see "Phase 4").

## Why it is slow, from the logs and the sources

* **The compile unit is the crate.** `iyakki-agent` is 729 files and 350 000 lines; `iyakki-shell` is 484 files and 276 000 lines, of which `apps/` (the tiles) is about 300 files and 162 000 lines (`story.rs` 10 000, `browser.rs` 8 400, `settings.rs` 6 600, `chat.rs` 5 400, `store.rs` 4 900). Touching one line of one tile recompiles the whole shell crate, and because the shell depends on the agent, whatever the agent needs first.
* **The release profile cannot be incremental.** `[profile.release]` is fat LTO with one code-generation unit: one thread, the whole crate, every time. That is the 8 m 50 s. (Only 5 of our own crates compiled in that log; the third-party crates and the Buildroot base came from the cache. The artifacts are kept: a build with no change ends in 0.11 s.)
* **A duplicate compile.** `dist/buildroot/repack.sh` runs `cargo build` twice (the main program, then the store server). The two builds unify some shared third-party features differently, so proxy and agent compile again: 3 m 49 s of pure waste.
* **The test binary is the product.** Shell tests link the agent and the hardware crates, so even a test of one tile links about 600 000 lines.
* **The deploy unit is the whole system.** The binary is 80 MB; `dev patch` refuses a file over 64 MB, so every change ships a full update pack.

## Two audiences, one idea (owner's clarification, 2026-11-07)

The "app" in this plan means **two things**, and WASM is the right answer for the first one first:

1. **Apps that people develop on IYAKKI** (the AI IDE's tile apps and extensions, the thing a developer or the assistant writes and tests all day). Target: **WASM guests with the display-list interface**. A developer's change compiles in seconds to a small `.wasm`, is pushed in kilobytes by `dev push --watch` and reloads in place; no OS image is built at all. The same module renders on the device's screen **or in a browser**, with the OS (services, vault, models, k3s) on the device or in the cloud behind the connector. One renderer (`iyakki-ui` on the host side, a thin one in the browser) draws the display list, so the look and feel is the same everywhere. This is also the second goal of `UI-OS-SPLIT-PLAN.md` (cloud UI plus connector).
2. **IYAKKI's own tiles** (story, browser chrome, settings, chat ...). Split into crates first (Phase 2) for the compile speed; move to the same WASM interface later, tile by tile, only where the checks in Phase 4 pass. The native tiles that need hardware stay native with their screens in WASM.

The one interface (events in, display list out, services through `iyakki-core-api` host imports) is built once, against a small app, and serves both.

## End to end: every stage of the loop, with a target

The goal is not only the compiler. A change travels through six stages; the loop is as slow as its slowest one.

| Stage | Today (1107e build, 2026-11-07) | Target for an app change | How |
|---|---|---|---|
| 1. Compile | 8 m 50 s + 3 m 49 s (release, fat LTO, two cargo calls) | **under 10 s** for a WASM app; **under 45 s** for the native binary | WASM app crate; `device` profile with incremental; one cargo call; lld (Phases 1, 2, 4) |
| 2. Repack the system | about 2 m 25 s (unpacks the base, prunes 3 233 firmware files = 1 280 MiB **again every run**, applies overlays, packs, signs) | **0 s** for a WASM app (no repack); **under 20 s** when the binary changes | Keep the pruned, signed base as a cache and only swap the changed files (the binary, the app) into it; prune once, not per build |
| 3. Lean image | 16 s | **0 s** (not needed for a push) | Skip it for pushes; it is for writing sticks |
| 4. Pack | seconds, 420 MB | **under 5 s**, a few MB | Only what changed: a binary delta (Phase 4, option 1) or just the `.wasm` |
| 5. Upload and apply | minutes over the network, 420 MB, then boot-file replacement | **seconds** | Same: few MB; apply to a new file and swap; no boot-file rewrite for an app |
| 6. Restart and see it | a reboot (about 1 to 2 min) | **no restart for an app** (hot reload); a service restart for the binary | Apps reload in place; the shell restarts itself, not the machine |

So an app change goes from about **15 minutes to under 30 seconds** and an OS-binary change to **about 1 to 2 minutes**, with the full image build kept for releases and for checking a whole image in QEMU before the first push of a day (the boot-test rule stays: every new image is booted in QEMU before it goes to a device; an app push does not change the boot files, so it is not an image).

Two pipeline fixes are independent of any code change and are in Phase 1: stop re-pruning the firmware (stage 2) and skip the lean image and full pack for a push (stages 3 and 4).

## Phase 0: measure (half a day, no code change)

1. On the dev PC, with nothing else building: touch one line in `apps/story.rs` and time `cargo check -p iyakki-shell`, `cargo build -p iyakki-shell` (dev), and `cargo test -p iyakki-shell --no-run`. Repeat for a small tile (a few hundred lines). Record cold and warm numbers in this file.
2. `cargo build --release --timings` once for the main program: which crates and which codegen steps dominate; how long the link takes.
3. Count what depends on what: `cargo tree -p iyakki-shell -e normal --depth 1`, and for the 10 biggest tiles, which `iyakki_agent::` items each one uses (a one-line grep per tile). That list is the real boundary the split has to cut.

Exit: a table of baseline numbers and a ranked list of tiles by (lines x how often they change).

## Phase 1: quick wins that need no restructuring (one day)

These alone are expected to bring a dev-PC run of one tile to well under the budget, and the device build far below 15 minutes. Each is measured against Phase 0.

1. **One cargo call in `repack.sh`** for both programs (`-p iyakki-core -p iyakki-store-server`): removes the duplicate compile. Compare the two binaries' feature sets before and after; ship only if the main binary is unchanged in behaviour.
2. **A device-test profile** (`[profile.device]`, inherits release): `lto = false`, `codegen-units = 16`, `incremental = true`, `opt-level = 2`, `debug = false`, `strip = true`. Incremental compilation reuses everything but the changed code-generation units. Test images and `dev update` pushes use it; releases stay on `profile.release`. The existing `fast` profile (thin LTO, 16 units, no incremental) is the starting point; the difference to add is `incremental`.
3. **A faster linker** for dev, test and device profiles: `lld` (or `mold`) through `.cargo/config.toml` (`-C link-arg=-fuse-ld=lld`). Linking an 80 MB binary is seconds with it, tens of seconds without. Check the licence (both are permissive) and that the build machine and the image pipeline have it.
4. **Test binaries without the world**: `cargo test -p iyakki-shell` builds one big binary. Until the split, keep the tile unit tests that do not need the agent running through `--lib -- <filter>` with `profile.test` incremental (already `debug = "line-tables-only"`), and measure.
5. **Repack without re-pruning**: cache the pruned and signed base (the 1 280 MiB firmware prune and the signing happen once per base, not per build) and put only the changed files into it; skip the lean image and the full pack when pushing (see the end-to-end table).
6. **Cargo caches**: `sccache` is optional; the target directory is already kept. Do not delete it; do not change `RUSTFLAGS` between runs (it invalidates every crate).

## Phase 2: split the shell into a toolkit and tile crates (the structural fix; 1 to 2 weeks)

```
iyakki-ui           gfx, fx, widgets, text fields, events, fonts: no tile, no agent
iyakki-core-api     traits and types the tiles call (UI-OS-SPLIT-PLAN.md), no I/O
iyakki-tile-story   one crate per big tile, depends on iyakki-ui + iyakki-core-api only
iyakki-tile-browser
iyakki-tile-settings ...
iyakki-shell        the frame (home, windows, input, drivers) + a registry of the tiles; depends on the tile crates
```

* **Order**: the ranking from Phase 0. A likely start: `story`, `browser`, `settings`, `chat`, `store`, `dash`, `media`, `ide`, `edit`, `board`. The rest of the small tiles stay in a `iyakki-tiles-small` crate until one of them is edited often enough to deserve its own.
* **The cut**: a tile may use `iyakki-ui`, `iyakki-core-api` and a narrow facade of the services it needs (a trait per domain instead of the 40-field `Services`; the traits are the ones `UI-OS-SPLIT-PLAN.md` already names). Where a tile today calls `iyakki_agent::...` directly, that call moves behind a trait in `iyakki-core-api` (with a fake in `iyakki-core-fake` for the tile's tests). Where a tile reads `/proc`, `/sys` or runs a program, it moves into the shared kernel-access module first (the owner's rule: no per-tile copies; `kernel_single_reader` and `dependency_ratchet` already hold the line).
* **Gate**: extend `crates/iyakki-core/tests/dependency_ratchet.rs` so a tile crate may depend only on `iyakki-ui` and `iyakki-core-api`; the list of exceptions may only shrink.
* **What gets easier**: a tile's tests link only its own crate, the toolkit and the fake core: seconds, not 32 minutes for the whole shell run. A tile compiles in parallel with the others.
* **Cost and risk**: heavy use of `pub(super)` and of child test modules sees private fields today; the move needs visibility changes (`pub(crate)`) and the test files move with the tile. `FEATURES.md` cites tests by path (`iyakki-shell::apps::...`): a script renames the paths in one pass and the matrix test confirms it. Moves are mechanical; do one tile per commit, with the whole suite green, so a break is easy to find. The owner is the only person editing the repository, so there are no merge conflicts to manage.

## Phase 3: split the agent by domain (1 to 2 weeks, after Phase 2 has started)

`iyakki-agent` (350 000 lines) is the other half of every rebuild. Cut it along the seams that already exist as modules: `chat/assistant`, `store`, `flows`, `cloud` (tenants, storage, routing), `sound` (audioout, mixer, micdefaults), `net` (wifi, hotspot, bluetooth, vpn), `devmode`, `quickinstall`, `k3s/gpu`. Each becomes a crate behind a trait in `iyakki-core-api`; `Services` becomes a thin registry of those. The order follows how often each is edited and how many tiles use it. A change in `quickinstall` then rebuilds `quickinstall` and the binary, not the chat engine.

## Phase 4: deploy only what changed (decision for the owner)

Even with a 30-second compile, a 420 MB pack is not a one-minute push. Three ways, in order of effort:

1. **A binary delta in `dev patch`**: send the difference between the running binary and the new one (our own small implementation of a binary diff with zstd; no new dependency), apply on the device to a new file, swap on restart. A tile change is a few MB; the 64 MB per-file limit stops mattering. This needs no change to how tiles are built and is the recommended first step.
2. **Tiles as separately loaded units**: a tile crate built as a `cdylib` behind a small, versioned C interface (the toolkit's drawing and event calls passed in as a table), pushed alone (1 to 3 MB) and loaded at the next open. Rust has no stable ABI, so the interface is explicit and tested; a version mismatch refuses to load. Biggest win for push time; real work, and it must keep the "tiles never touch the kernel" rule (the table is the only thing they get).
3. **Tiles as WebAssembly guests (the owner's preference, 2026-11-07: "since it is wasm and rust it should be super fast")**: a Rust tile compiled to `wasm32-unknown-unknown` as a small crate builds in seconds (incremental, no LTO, no link of an 80 MB binary), the module is 100 KB to 1 MB so a push is instant, it is sandboxed (fuel, memory, time, declared capabilities) and it can be replaced while the shell runs. The same module can later draw in a browser UI in the cloud, which is the second goal of `UI-OS-SPLIT-PLAN.md`.
   * **What exists and what does not (checked 2026-11-07).** `iyakki-wasm` (931 lines) is a sandbox for **tools** (fuel, memory, a virtual file system, net through a backend trait, snapshots). It has **no drawing or input interface**. Today's Rust tiles are **native code inside the shell crate**, not WASM; web-bundle tile apps (`window.iyakki`, `dev push --watch`) are the only tiles that hot-reload now. So the speed comes from the split and a new tile ABI, not from WASM by itself: a tile that stays inside the 276 000-line shell crate compiles just as slowly whatever it is compiled to.
   * **The tile ABI to design** (small, versioned, the only thing a guest gets): per frame the guest receives events (pointer, key, text, tick, size, theme) and returns a **display list** (rectangles, rounded rectangles, text runs with a face and size, images by handle, clip, hit regions that map to the events the shell sends back). The host (`iyakki-ui`) rasterises it, so no pixel buffer crosses the boundary and the guest needs no font or graphics code. Calls to services go through the `iyakki-core-api` traits as host imports (JSON in, JSON out, the same scopes and approvals as the other transports). Storage, network and every kernel fact stay behind those imports; a tile cannot open anything itself.
   * **Checks before committing to it**: (a) a display-list frame of the heaviest tile (`story`, `browser` chrome, `settings`) crosses the boundary in under 2 ms; (b) text editing and IME work through events alone; (c) the tiles that need native code (the browser engine, camera, sound capture) stay native and are not forced into WASM; they keep a thin native shell and put their screens in WASM.
   * **Order**: build the ABI and one small tile (a few hundred lines) end to end first, then the first big tile, then decide how many follow. Keep the native tiles working throughout; a tile is native or WASM, never both.

Recommendation: ship (1) first (independent, about 2 days), do Phase 2 (the split) because every option needs it, then build (3) as the long-term answer; (2) is the fallback if the display-list cost in (3) is too high.

## Order and effort

1. Phase 0 (measure) and Phase 1 (quick wins): do first, about 1.5 days. It also gives the numbers that decide whether Phase 2 needs to cover all tiles or only the biggest ten.
2. Phase 4, step 1 (binary delta in `dev patch`): about 2 days.
3. Phase 2, tile by tile, starting with the two or three heaviest.
4. Phase 3, in parallel when a domain is next to be edited.

## Acceptance (what "done" means)

* The table at the top, filled with measured numbers, meets the budgets for the tiles that were split.
* A test (or a documented script) that touches a tile file and checks the incremental time on the dev PC stays under budget; it is advisory, not a CI failure, because machines differ.
* `dependency_ratchet` rejects a tile crate that depends on the agent or on another tile.
* All existing tests still pass after each move, the feature matrix passes, and an image built from the split tree boots in QEMU and behaves the same on the Lenovo.
* The owner's rules still hold: kernel facts only through the shared modules, everything a tile does is reachable by chat tools, no personal names or paths in the repository.

## Measured results (Phase 0/1)

Measured 2026-11-07 on the dev PC (16 cores, 28 GB), cargo under `systemd-run --user --scope -p MemoryMax=9G -p MemorySwapMax=6G`, `CARGO_BUILD_JOBS=4`, `TMPDIR=$HOME/.cache/iyakki-tmp`. **Caveat for every number below:** another change set (the UI toolkit and tile ABI work) was being edited in the same workspace and was compiling at times, so some "touched" runs also rebuilt that work, and the machine was not idle. Read them as orders of magnitude, not as benchmarks. The probe was one added line (`#[allow(dead_code)] const IYAKKI_COMPILE_SPEED_PROBE: u32 = N;`) at the end of the file, removed again after each run (`git status` shows the files clean). A comment-only change was not used: it would let incremental compilation skip nearly everything and flatter the result.

### Phase 0: one line in a tile, dev PC

| Command | `apps/story.rs` (10 000 lines) | `apps/lab_pic.rs` (160 lines) |
|---|---|---|
| `cargo check -p iyakki-shell`, warm, nothing changed | 1.1 s | 3.8 s (included a change of the other work) |
| `cargo check -p iyakki-shell`, one line touched | 3.3 s, 3.2 s | 3.1 s, 9.4 s |
| `cargo check -p iyakki-shell`, **cold** (empty target directory, all dependencies) | 92 s (whole crate; nothing changed second time: 0 s) | same |
| `cargo build -p iyakki-shell` (dev), one line touched | 47 s, 40 s | 5.1 s, 11.7 s |
| `cargo test -p iyakki-shell --no-run`, one line touched | 12.0 s, 11.5 s | 19.8 s, 12.5 s |

Findings: with incremental compilation the dev profile is already near the budgets for **check (under 10 s: met)** and **test build (under 30 s: met)**; the dev **build** of the shell binary is 40 to 47 s for the biggest tile (budget 45 s: borderline) and 5 to 12 s for a small one. The fat-LTO release build (8 m 50 s) is the real cost, not the dev loop. Not measured: `cargo build --release --timings` (Phase 0 item 2) and the dependency count per tile (item 3); the first needs a 9-minute release build and was left out on purpose.

### Phase 1 item 2: `[profile.device]` (done)

`Cargo.toml` has `[profile.device]` (inherits release; `lto = false`, `codegen-units = 16`, `incremental = true`, `opt-level = 2`, `debug = false`, `strip = true`); `[profile.release]` is unchanged. `IYAKKI_PROFILE=device dist/buildroot/repack.sh ...` (and `build.sh`) already used the `PROFILE`/`PFLAG` mechanism and the path `target/x86_64-unknown-linux-musl/$PROFILE/`, so `repack.sh` needed only documentation and a "cargo profile" line in its plan. Measured with `cargo build --profile device --target x86_64-unknown-linux-musl -p iyakki-core -p iyakki-store-server` (one call):

| Step | Time |
|---|---|
| First build of the profile (every dependency, no cache; beside a cold shell check) | 312 s |
| Build after the other work's edits had landed (not a pure no-change run) | 161 s |
| One line touched in `iyakki-agent/src/clock.rs` (the agent, the shell and the core relink) | **20.8 s, 20.5 s** |
| One line touched in `iyakki-shell/src/apps/story.rs` | **22.1 s, 9.8 s** |
| Back to the unchanged source | 9.7 s |

So a one-line change builds a **runnable musl image binary in 10 to 22 s** instead of 8 m 50 s + 3 m 49 s. The device binary is 90.8 MB (release: 80.6 MB), the store server 1.9 MB (release 1.3 MB). It is slower at run time (opt-level 2, no LTO): for test images and `dev update` pushes only. Not verified: that an image built from the device profile boots (the boot-test rule applies as for any image) and its run-time speed.

### Phase 1 item 1: one cargo call (done, with a check)

`repack.sh --cargo` now runs `cargo build ... -p iyakki-core -p iyakki-store-server` once; if the joint build fails it falls back to the old two steps (the store server stays optional, as before). Feature check, `cargo tree --target x86_64-unknown-linux-musl -e normal,build --prefix none --format '{p} [{f}]'`, sorted and compared:

* `-p iyakki-core` alone against `-p iyakki-core -p iyakki-store-server`: **identical for every package** (the only added line is the store server itself). The main binary gets no extra feature, so it is unchanged in behaviour and the merge is made.
* `-p iyakki-store-server` alone against the joint set: 13 third-party crates gain features for the store server (`blake2 std`, `bytemuck extern_crate_alloc`, `digest alloc,mac,oid`, `fnv default,std`, `libc extra_traits`, `once_cell critical-section,portable-atomic`, `ring std`, `sha2 alloc,default,oid`, `smallvec const_generics`, `subtle std`, `x25519-dalek default,precomputed-tables`). All are additive (they enable code the main binary already compiles in); none removes anything. That is exactly the unification that made cargo build `iyakki-proxy` and `iyakki-agent` twice. The store server may grow by a few KB; that was not measured on a release build.

Not measured here: the saving itself (3 m 49 s in the 2026-11-07 log) needs a release build; the device-profile call above compiled each crate once.

### Phase 1 item 3: faster linker (not installed, off)

`which ld.lld mold lld` finds none on the dev PC. Nothing was installed. `.cargo/config.toml` holds the exact block for the host target (dev and test builds), commented out, and the command line for a device build of the musl target (cargo cannot scope rustflags to one profile, and the musl target is shared with release, so it is an environment value, not a config line). To enable: `sudo apt install lld` (or `mold`), remove the `# ` of the block. Not measured.

### Phase 1 item 5 and stage 2: repack without re-pruning (done)

New `dist/buildroot/repack-cache.sh` (sourced by `repack.sh`) and a small change in `board/iyakki/post-build.sh`. The pruned and signed `lib/modules` + `lib/firmware` of a repack are saved as a zstd tar (190 MB for the pc variant) in `target/repack-cache` (`IYAKKI_REPACK_CACHE=DIR` moves it, `=0` turns it off; never `/tmp`; the newest two per variant are kept). The key is a hash of: variant, the kernel donor's `rootfs.cpio` and out-of-tree modules (size and mtime), the linux-firmware `WHENCE` and `copy-firmware.sh`, the prune and keep scripts and `post-build.sh`, the Secure Boot certificate and key, and the signing tool. A cold cache runs exactly as before and saves at the prune point; a hit restores the tree and sets `IYAKKI_FW_PRUNED=1` so `post-build.sh` skips the prune (the restricted-firmware strip, licence texts and release stamps still run). Release images and overlays that ship their own firmware or modules do not use the cache.

Measured with the owner's pc recipe (signed, `--base target/buildroot-web --kernel-from target/buildroot-mac --graft git`, output to a scratch folder under `~/iyakki-work/cache`, nothing written to a device or a stick, no cargo):

| Run | Total |
|---|---|
| Old behaviour / cold cache (copies the firmware, signs the modules, prunes 3 233 files = 1 280 MiB, saves the cache), zstd -19 | **154 s** |
| Warm cache, zstd -19 | **92 s** |
| Warm cache, `--fast-zstd` (zstd -3; push mode) | **21 s** |
| Cache off, `--fast-zstd` (same as the line above without the cache) | 95 s |

The cache saves about 60 to 75 s; with zstd -19 the compression of the 1.2 GB archive (about 70 s) is now the largest remaining step, which is why push mode uses zstd -3. Equality check: the `lib/modules` and `lib/firmware` of the warm-cache image and the cache-off image have the same 4 937 entries with the same types, modes, link targets, sizes and SHA-256 sums, and the complete cpio file lists of the two are identical. Not compared: the whole cpio byte for byte (file times of files written at repack time differ by definition) and the boot of the result.

Test: `dist/buildroot/test-repack-cache.sh` (23 checks on fake folders: stable key, key changes with the keep list, prune script, donor, out-of-tree module, firmware tree, signing key and certificate; no cache for release, with `IYAKKI_REPACK_CACHE=0`, or with an overlay that ships firmware; trim; a prune-snapshot-restore round trip identical in names, modes, links, sizes and content; the flag in `post-build.sh`; one cargo call in `repack.sh`). A full `repack.sh --dry-run` prints "mod+fw cache hit/miss" with the file name.

### Push mode (stages 3 and 4; script written, not run end to end)

`dist/buildroot/quick-push.sh -o PACKDIR [--cargo] [--bin FILE] [--sign KEYFILE] [--version V] [-- repack args]`: optional one-call `cargo build --profile device`, `repack.sh pc --fast-zstd` into a separate `target/buildroot-pc-push` (the finished builds are left alone; `IYAKKI_REPACK_BIN` carries the program), then `iyakki dev update-pack` with the program just built. It skips the lean image and writes no drive. It prints the `iyakki dev update PACKDIR` command for the person to run; the script never contacts a device. Expected loop on the dev PC: about 10 to 22 s compile + about 21 s repack + the pack, instead of 15 minutes. What this does **not** shrink: the pack is still the full initial RAM disk (about 420 MB), so stages 4 and 5 stay as listed in the table until the binary delta (Phase 4, option 1) exists. Not verified: the script was only syntax-checked and its `--help` run; the repack it calls was verified above, and `update-pack` was not run.

### Summary against the end-to-end table

| Stage | Before | Now (this machine, this day) |
|---|---|---|
| 1. Compile (device profile, one line) | 8 m 50 s + 3 m 49 s | 10 to 22 s warm (5 m 12 s once, cold) |
| 2. Repack | about 2 m 25 s | 92 s with zstd -19, **21 s** with push mode |
| 3. Lean image | 16 s | skipped in push mode |
