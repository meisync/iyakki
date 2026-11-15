# Rules for implementation lanes (alpha.3)

Read this first. These rules come from real gate failures and owner decisions.

## What counts
- Impl means the code is implemented (owner, 2026-11-12). Impl 100 when everything the row promises is written, even if only verification (a device, a live service, a screen) is left: then write the dated sentence used by earlier lanes ("Impl 100 by the owner's rule of 2026-11-12: written; what remains is verification (...)").
- Test only for what you ran. Never raise a Test level by reading test names. Test 100 needs a real run.
- Never relabel a row to hide unwritten code; never invent hashes, keys or data.
- Skip rows that need real hardware to design, a live service, or an owner decision; say why in your report.

## The core rule (owner, 2026-11-14)

Everything goes through the IYAKKI core. The app layer (tiles, the shell's apps, the web UI, anything a person or an assistant can reach) never talks directly to Linux or to a device: no `/proc`, `/sys`, `/dev`, no spawned program, no raw socket or file outside what the core hands it. It asks the core through the core API (`iyakki-core-api`: `cx.os` and the other handles), the core asks for the permission (capability catalogue, broker, per-use approval card where the catalogue says so) and writes the decision to the access log, and only then does the device or the system get touched. New code follows this from the first line: a new read or change of the machine is a method on a core API trait with a `FakeOs` test, a capability name and an access-log line, not a direct call. The ratchets (`kernel_readers.allow`, `shell_kernel_access.allow`, `BOUNDARY.md`, the dependency-rule ratchet) may only shrink.

## Every new feature
- In-house code, permissive licences only. Every user-facing feature has a chat tool with approvals (see `chat_settings*.rs`, `settingstools_*.rs`, `CHAT-REACHABILITY.md`, the reachability tests, toolblock routing, the tool-count and tool-list assertions in agent tests, the hard-ask list in `agent.rs`).
- Update the FEATURES.md row honestly; run `python3 dist/features/migrate-impl-test.py`.
- Never raise `docs/development/partial-ceiling.txt`; new partial rows go in `docs/development/partial-ceiling-raises.txt`.
- No `tokenbudget.rs` changes. No personal names, home paths, IPs, usernames or secrets anywhere in the repo.

- A set of tools that wraps another (an `Arc<dyn Tools>` field) implements `inner_tools` (`Some(&*self.inner)`) and only the methods it decides itself: every other method of `agent::Tools` (card text, refusal, grant rules, open work, notes, running jobs) then reaches the tools underneath. A wrapper that implements only some of them hides the rest (the screen-capture approval was lost that way). `crates/iyakki-agent/tests/tools_wrappers_forward.rs` fails for one that forgets.

## Hazards that failed real Linux gates (avoid them in every new test)
- Temp folders: the gate sets TMPDIR to `<repo>/target/tmp`, inside the repo. A test that needs a folder with no `Cargo.toml` or `.git` above it uses `testdir::outside_project`.
- Unix socket paths must stay under 100 bytes (use a short `/tmp` folder).
- Never depend on the wall clock between two draws, nor on racing background threads: freeze the clock, or join/poll with a bound.
- Layout expectations must not depend on a font (Linux and macOS differ); a page taller than the window scrolls (use the generic body scroll bar).
- No `.clamp(` with computed bounds in the shell crate: use `.clamp_safe(`.
- Locks in non-test code: `.lock().unwrap_or_else(std::sync::PoisonError::into_inner)` (the shell crate's `tests/poison_scan.rs` enforces it).
- No direct `/proc`, `/sys`, `/dev` or `iyakki_devctl` use in `iyakki-shell` tile code (dependency_ratchet): go through `crate::display`-style wrappers or the agent crate.
- New base-OS touching files (also test files that spawn processes) need a BOUNDARY.md row. New pub consts/types the surface scan lists need a `compat-surfaces.txt` row; core-surface additions only via `IYAKKI_CORE_SURFACE_WRITE=1`. Tests must not read the real HOME without `// home-ok: why`. Zero warnings.

## Checks before every bundle
- `python3 dist/features/migrate-impl-test.py`
- `cargo test -p iyakki-core --test feature_matrix --test boundary --test dependency_ratchet`
- `cargo test -p iyakki-shell --test poison_scan --test safeclamp_scan`
- tests of the crates you touched, by module filter (the `iyakki-shell` lib test binary segfaults on macOS in `backend::` tests)
- `sh dist/release/check-linux-cfg.sh` (both targets, 0 warnings) and `bash dist/release/compat-check.sh`
- Compare a failing test with an unmodified worktree before calling it a macOS flake.

## Git
- Commit as akash-meisync (the noreply email already in `git log`), message ends `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- Use `git add -A -- <paths>`; never commit the copied `iyakki-boot` folder.
- Bundle after every row or two: `git bundle create ~/iyakki-work/<LANE>.bundle <base>..<branch>`. Bundle what is finished before you run low on room, and keep your own context small (do not read huge files whole).
