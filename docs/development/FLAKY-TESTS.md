# Flaky tests: the cure for the class (owner order 2026-11-18)

**For:** anyone who writes or fixes a test.
**You will get:** the rules, the `iyakki-testkit` helpers that make them easy, and how to find the next flake before the gate does.

Every flake the gate showed was a race in the TEST. They share a few causes, and each has one cure in `crates/iyakki-testkit` (a dev-dependency, never shipped):

| Cause | Do not | Do |
|---|---|---|
| Two tests touch one process-wide thing (input state, environment, current folder, the cluster, the audio mixer, a fixed port, a clock override) | hope they never overlap | `let _g = iyakki_testkit::lock(iyakki_testkit::names::INPUT);` around the critical section only. Works across threads, test binaries and parallel cargo runs (`flock` under `/tmp/iyakki-testkit-<uid>/`); times out after 120 s and names the holder. `EnvGuard::set(&[("K", Some("v"))])` sets variables under the env lock and restores them. |
| A port number is chosen, freed, then used | `bind(":0")`, drop, reuse; a literal port | `let p = iyakki_testkit::port();` holds the number until `p.release()` / `p.into_listener()`. For "nothing listens": `closed_port()`. |
| Wait for a background effect | `sleep(50ms)`; `for _ in 0..600`; "after 3 beats" | `wait_until(BUDGET, \|\| cond)` (30 s wall clock, polls every few ms, names the last state), `wait_for_state`, `wait_for_file`, `wait_for_line`. |
| A stub child must "run for a while" | `sleep 2` stubs | `GatedChild::start()` lives until `release()`; `ChildGuard` kills and reaps on drop; `wait_gate_snippet(gate)` for stub scripts. |
| A test asserts speed | `elapsed < 50ms` | assert the behaviour; where speed is the point inject `ManualClock` and `advance` it. |

Never weaken an assertion that checks behaviour, never add a test to `dist/release/known-flaky.txt` (the mechanism stays, the list is empty), never use `#[ignore]` to hide a flake.

## Finding them
* `python3 dist/scripts/flaky-patterns.py` scans all test code for the risky patterns and writes `docs/development/FLAKY-AUDIT.md` (counts per crate).
* The ratchet test `flaky_pattern_ratchet` (iyakki-core tests) fails when a crate has more risky hits than `docs/development/flaky-ratchet.txt` allows; the numbers only shrink.
* `dist/scripts/flake-hunt.sh` runs a test filter N times in parallel under CPU load and lists the tests that failed at least once (see its header).

## The flake hunter
`dist/scripts/flake-hunt.sh -p CRATE [-n RUNS] [-j PARALLEL] [-T TEST_THREADS] [-l BURNERS] [-f FILTER]` builds the crate's unit-test binary, then runs it RUNS times (PARALLEL at a time, TEST_THREADS threads each, far above the core count) next to BURNERS busy-loop processes, and lists every test that failed at least once with the number of runs it failed in. Examples (on a build host, in the worktree copy, `cargo` on the PATH):

    ulimit -n 10240
    bash dist/scripts/flake-hunt.sh -p iyakki-shared -n 10 -j 4 -l 8
    bash dist/scripts/flake-hunt.sh -p iyakki-apps -n 3 -j 3 -T 32 -l 6 -f jobs_screens

* A test that fails in EVERY run on a Mac is usually a Linux-only test (see LINUX-ONLY-TESTS.md): compare with one quiet run (`-n 1 -j 1 -T 8 -l 0`) first; only the difference is a load flake.
* A test that fails in some runs is a race in the test: find the pattern in the table above and apply its cure.
* Do not run it on the host that is running the release gate; on Linux run it only when the gate is idle. The burners and test processes are always stopped when the script ends. Logs stay in `HUNT_OUT` when something failed.

## What the audit counts, and what stays risky
`dist/scripts/flaky-patterns.py` is a heuristic scanner, not a proof: a hit is `ok` when its surroundings show the guard (a wall-clock deadline, a condition-polling `while`, a lock, a claimed port) and `risky` otherwise. What stays risky after the conversion, and why:

* real-time audio (the mixer thread, `embedmedia_tests`, `mixer/stress_tests`, `syssounds_tests`): the sound is produced in real time by a thread; the cure is a mixer driven by a manual clock (own mixer plan), until then they use `mixer::retried_under_load` and the AUDIO lock;
* product time semantics that read the wall clock (jobs "quiet" detection, IDE once-a-second refresh, ...): they need a clock injected into the product code first;
* tests of "nothing happens" (a negative check needs a quiet window, not a wait);
* JavaScript and Chrome tests (`crates/iyakki-remote/tests/*chrome*.rs`): the sleeps are in the page script, not in Rust;
* fixed delays inside fake servers (`Site::new(40)`): they model latency, nobody waits for them;
* `set_var` of a variable only that test reads (unique names): harmless, listed until it carries `// flaky-ok`.

## What the first hunts found (2026-11-18, two Apple Silicon Macs under load)
Real races in the tests, now cured; each one is a member of the classes above.
* sshfs `testdir::alive` judged every other process dead without `/proc` (macOS) and swept the folders of a test run that was still going: `sftp_tests::*`, `extagent` and other "NotFound" failures. The agent's copy had the guard already.
* The `childgone` tests took a child's exit status "from behind" while a `sleep 1.5` stub lived: a loaded machine started the thief too late. The stub now waits for a gate the thief opens when it has found the child; a scenario in which the code under test reaped the child first is run again.
* `k3sdev` alert, `domains` listener answer, extagent login cancel (child id file), IDE redraw counting, `preload_tests` (14 fixed sleeps), jobs log cap, `flows` browser retry: fixed sleeps or attempt counts replaced by waits on the thing itself.
* Model calls held open by the test (board stop, autopilot switch-off) instead of "takes 300 ms, so the switch lands inside".
* Fixed ports behind the `FIXED_PORT` lock (browsertools); camera refusal "at once" asserted by what was not called, not by a stopwatch; clientlink lock test asserts order instead of elapsed time.
* About 180 polling loops `for _ in 0..N { ...; sleep }` became wall-clock loops (30 s or more); small `wait_for(secs)` helpers have a floor of 30 s.

Seen under load and NOT fixed (reason): `sshfs ssh_server::...never_opens_its_window` (the client sends 2000 packets as fast as it can; the bound depends on how fast the server thread runs: needs a paced client or a counter in the server), `proxy guard::..swallows_packets` (partly fixed: the accept queue of macOS), `extagent::login` ("could not open a terminal": the Mac runs out of ptys when 8 runs open them together; use `-j 2` for that crate), the `iyakki-netscan` mDNSResponder test (needs a real network), and the Linux-only `jobs`/`gitjobs` tests (they read `/proc`; run the hunt for them on Linux when the gate is idle). Hunting the `iyakki-apps` crate as a whole took more than 30 minutes per run on a shared Mac, so it was hunted by filter (the tests changed here); the whole-crate hunt belongs on Linux.
