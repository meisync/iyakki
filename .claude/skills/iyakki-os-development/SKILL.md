---
name: iyakki-os-development
description: Work on the IYAKKI operating system itself (this repository): find your way around the crates and docs, keep FEATURES.md honest, build and test on the right host, avoid the mistakes that broke real gates, build and boot-test an image, push it to real hardware, and record the result. Use for any change to IYAKKI code, images, tests, the feature matrix or a release.
---

# IYAKKI OS development

IYAKKI is an AI-first operating system: a Rust workspace (`crates/`), a Buildroot-based image (`dist/buildroot/`), an in-house UEFI boot loader (the separate `iyakki-boot` folder), and a feature matrix (`FEATURES.md`) that says what is built and how well it is proven. This skill is the working method. The project's own rules live in `README.md`, `CONTRIBUTING.md`, `BOUNDARY.md`, `ALPHA-GATE.md` and `docs/development/LANE-RULES.md`: read `LANE-RULES.md` before you change code.

## The core rule

Everything goes through the IYAKKI core (owner, 2026-11-14): the app layer never touches Linux or a device directly; it calls the core API, the core asks for the permission (capabilities, broker, approval card), logs the decision, then acts. See docs/development/LANE-RULES.md, "The core rule". Ratchets only shrink.

## 1. Principles (owner decisions)
- Build in-house; permissive licences only (`cargo deny` is part of the gate). Third-party tools (a coding agent, ffmpeg, a vendor firmware) are downloaded from the vendor at the person's click and are never shipped.
- Every user-facing feature has a chat tool, and anything that changes the machine asks for approval on the person's screen (hard asks: every time, never covered by a session grant).
- No personal names, home paths, IPs, usernames or secrets in the repository, in commit messages or in built images. Commits use the project's noreply identity and end with the `Co-Authored-By` line the tooling asks for.
- Never raise a number by reading names: a Test level goes up only for what was run. Never relabel a row to hide unwritten code.
- Tag, release and public pushes happen only on the owner's word. Do not push to the public remote from a working branch.

## 2. The feature matrix (`FEATURES.md`)
- One row per feature: ID, Feature, Phase, Status, Pri, Impl/Test, Manual, Tests, Notes. Impl means the code is implemented ("fully written"); Test is 0/30/50/70/100 (50 is the ceiling for fakes, 100 needs a real run); Manual is none, needed, `passed <date> <machine>` or `failed KI-nnn`.
- `done` needs Impl 100, Test 100 and Manual none or passed. Everything else with work done is `partial`; planned rows start at 0/0.
- After any edit run `python3 dist/features/migrate-impl-test.py` (rewrites the totals line) and `cargo test -p iyakki-core --test feature_matrix`.
- `docs/development/partial-ceiling.txt` may fall, never rise except for new rows listed in `partial-ceiling-raises.txt`. Bugs get a KNOWN-ISSUES entry (KI-nnn); a new feature gap gets a row, not a note.
- Splitting a row: the built part stays (and may reach done), the unbuilt part becomes a new planned row.

## 3. Where things are
- `crates/iyakki-core`: PID 1, boot, the contract tests (`tests/feature_matrix.rs`, `boundary.rs`, `dependency_ratchet.rs`). `iyakki-agent`: services, tools, settings, the assistant. `iyakki-shell`: the screen, tiles, Settings, input. `iyakki-uikit`, `iyakki-web` (in-house browser engine), `iyakki-js`, `iyakki-flow`, `iyakki-usb` (installer), `iyakki-storage`, `iyakki-vt`, `iyakki-remote`, `iyakki-link`, `iyakki-relcheck` (release checks).
- `dist/buildroot` (image, lean image, packs, QEMU tests), `dist/release` (gate scripts), `dist/hwtest` (hardware recipes), `dist/site` (the public progress page generator), `dist/skills` (skills that ship inside the OS).
- `docs/development/` holds plans and triage (start with `PARTIALS-TRIAGE-*.md`, `IMPL-RULE-*.md`, `LANE-RULES.md`).

## 4. Hosts and the one-build rule
- Build practice (hosts, `rcargo`/`mcargo` in dist/buildhost, cleanup after every run, `--retire` when a lane ends, the compressed build disk, the gate): docs/BUILD-PRACTICE.md.
- Compiling is heavy. Keep one build at a time on the main machine; use extra build hosts for parallel work: macOS hosts compile and run unit tests but cannot run Linux-only code, so a Linux host with KVM and QEMU is the faithful test runner (`dist/release/setup-linux-host.sh` prepares one if present).
- Move work between machines with `git bundle` (create on one side, `scp`, `git fetch bundle branch:branch` on the other). Branch per lane, merge in a scratch worktree, then fast-forward the main working tree.
- Cargo runs with `CARGO_INCREMENTAL=0` and `TMPDIR=<repo>/target/tmp` (`dist/release/env.sh`); builds refuse to start with under 40 GB free. Prune old targets after use.
- On macOS: `ulimit -n 10240`; use `perl -e 'alarm N'` for time limits; the `iyakki-shell` lib test binary crashes in `backend::` tests, so run it by module filter; accepted sockets inherit non-blocking mode.

## 5. Checks (run before you hand work over)
- `sh dist/release/check-linux-cfg.sh` (type-checks the Linux targets from any host, 0 warnings) and `bash dist/release/compat-check.sh` (surface, core API, fixtures, test hygiene).
- `cargo test -p iyakki-core --test feature_matrix --test boundary --test dependency_ratchet` and `cargo test -p iyakki-shell --test poison_scan --test safeclamp_scan`; plus the tests of the crates you touched.
- The full gate is `./run.sh alpha-check` (about 1.5 h on a Linux machine with KVM: builds, zero warnings, workspace tests with a rerun of failures, compat gates, window build, macOS cross-check, wasm engine, licences, SBOM, three QEMU tests). A test that fails in parallel but passes alone is a flaky test and fails the gate unless it is listed in `dist/release/known-flaky.txt`: fix the cause instead.
- Compare any failing test with an unmodified checkout before calling it a flake.

## 6. Mistakes that failed real gates (do not repeat)
1. Temp folders: the gate puts `TMPDIR` inside the repo, so "a folder with no Cargo.toml/.git above" is not true by default. Use `testdir::outside_project`.
2. Unix socket paths over about 100 bytes fail on Linux (SUN_LEN): use a short `/tmp` folder.
3. Tests that depend on the wall clock between two draws, on a countdown, or on a background thread finishing: freeze the clock, join or poll with a bound.
4. Font-dependent layout expectations: Linux and macOS draw different widths; scroll the page (generic body scroll bar) instead of asserting positions.
5. `.clamp(` with computed bounds in the shell crate panics when min > max: use `.clamp_safe(`.
6. `.lock().unwrap()` in non-test code: use `.lock().unwrap_or_else(std::sync::PoisonError::into_inner)`.
7. `iyakki-shell` tile code must not read `/proc`, `/sys`, `/dev` or use `iyakki_devctl` directly (dependency_ratchet): go through a wrapper or the agent crate.
8. New files that spawn processes or touch the base OS need a `BOUNDARY.md` row; new public consts/types may need a `compat-surfaces.txt` row; a core API addition needs `IYAKKI_CORE_SURFACE_WRITE=1` to regenerate the snapshot.
9. Tests must not read the real HOME without a `// home-ok: why` comment.
10. A fix to a security tightening must not lock a person out: when home folders were made private, accounts with no passwd entry lost access to their own home. Check both sides of an ownership change.

## 7. Building, testing and pushing an image
1. Signed images need the Secure Boot key and certificate in `IYAKKI_SB_KEY` and `IYAKKI_SB_CERT` (kept outside the repo); an unsigned build is refused by a device that expects signed ones, so set them on every repack and lean-image build.
2. `dist/buildroot/nightly-push.sh -o PACKDIR --version V [--layers]` builds the program, repacks, makes the lean image and an update pack, boots it in QEMU through the real chain and prints the update commands. It never sends anything to a device. The version label must sort above the one on the device (a plain `1113a` looks older than `0.1.0-alpha.2`); the first push to a device must be an ordinary pack, not a layered one.
3. A lean boot volume keeps the RAM disk as `boot/initrd-lean` (stick layout) or `boot/initrd` (single-root layout); the loader's `boot.cfg` must name the file that exists. Check the pack's `boot.cfg` before sending it.
4. Images must not carry a builder's account name: `dist/buildroot/build.sh` sets neutral `KBUILD_BUILD_USER/HOST`; check a kernel image with `dist/release/image-identity-scan.sh`.
5. The in-house boot loader is a separate folder (`iyakki-boot`) with its own host and QEMU tests; changes to it are patches until its tests pass.

## 8. Real hardware (developer mode)
- On the device: Settings, Developer mode, On (off again after 4 hours; extend it). Pair once with `iyakki dev pair ADDRESS` and the one-time code and fingerprint the device shows (check that they match). The scopes a computer holds are chosen on the device.
- Look first, read-only: `iyakki dev info|status|ps|state|screenshot|logs --system|dmesg --grep X|kube get ...`. Drive the screen with `dev input click "LABEL"` (read `dev state` first; never type a secret). `dev shell` runs one command per approval, as a restricted user.
- `iyakki dev update PACKDIR` writes the new boot files and keeps "previous version" in the boot menu; `iyakki dev reboot` restarts. Always boot-test the image in QEMU first. Never write to an internal disk, and ask before restarting a machine somebody is using.
- A laptop with another system first in its boot order restarts into that system; the stick needs to be first in the firmware order (`iyakki startup-disk status|register|first|next|revert`), or picked in the firmware boot menu.
- `dist/hwtest/hwcheck.py --device NAME run ID...` runs the read-only recipes and writes PASS / FAIL / NEEDS-HUMAN with evidence outside the repository. A PASS lists what it did not cover: do not flip a whole row on a narrow observation.
- Record results honestly: a manual pass sets Manual to `passed <date> <machine>`; a failure becomes a KI entry. Hardware that cannot be reached stays "needed".

## 9. Working in parallel (lanes)
- One lane = one branch, one scope (a list of rows), one bundle. A lane implements, tests, updates its rows and KIs, runs the checks in section 5, commits and bundles; the integrator merges. Merge conflicts in `FEATURES.md` are resolved row by row (keep the receiving side, apply the incoming column changes and new rows), then the totals are regenerated and the ceiling file set to the actual partial count.
- Keep context small: do not read huge files whole; search, then read the part you need.
- Report exactly what was run, what passed, what was not verified, and which failures also occur on an unmodified checkout.

## 10. Quick start for a change
1. Read the row(s) in `FEATURES.md` and the plan docs named in their Notes. 2. Write the failing test, then the code. 3. Add the chat tool and its approval, reachability and routing entries. 4. Update the row, run the matrix script, add or fix the KI. 5. Run the checks in section 5. 6. Commit, bundle or push to the working remote, and say what you could not verify.
