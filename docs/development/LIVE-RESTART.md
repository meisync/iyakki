# Live restart: a new program without a device restart

Owner 2026-11-15: no device restart for an apps change or a core change. Only the **device layer** (kernel, modules, base image, boot files, first stage) may need a reboot. Status: the
parts that need no hardware are built and tested (`crates/iyakki-core/src/liverestart.rs`); the wiring into the running PID 1 and the on-device trigger are **not built**; the real-device
proof is **Manual needed**. It is **off by default**: nothing acts unless `/etc/iyakki/live-restart.enabled` exists, and no image ships that file.

## 1. Which layer decides what

| Changed | Verdict | Route |
|---|---|---|
| ui/apps crates (`iyakki-apps`, `iyakki-shell`, `iyakki-uikit`, tile crates, ...) | `live-ui` | the shell is a thread of the core program, so today this is the same as `live-core` (see 2) |
| core/base crates (`iyakki-core`, `iyakki-agent`, libraries below them), `Cargo.toml`/`Cargo.lock` | `live-core` | in-place re-exec of PID 1 (section 3) |
| kernel config, modules, boot files, first stage (`iyakki-stage1`), image recipe (`dist/buildroot` recipes, `layers.txt`), `iyakki-boot` | `reboot` | staged update (`quick-push.sh` / `nightly-push.sh`, `iyakki dev update`) and `iyakki dev reboot` |
| docs, scripts | `nothing` | |

`./run.sh fast` (`dist/scripts/fast-loop.py`) prints the verdict before it builds or sends anything and stops on `reboot`. `iyakki dev push-apps --layer VERDICT` says the same on its side and
refuses `reboot`. The rule is a function with a test (`dist/scripts/test-fast-loop.py`).

## 2. Tile programs: there is nothing to restart

Finding of this round: the `iyakki-tile-*` crates are **libraries** linked into the one `iyakki` program (`iyakki-shell` depends on `iyakki-tile-host`; the apps in `iyakki-apps` adapt the
tile crates). A tile is not a process, so "restart only that tile's process" does not exist today; it would exist only after the process split (`iyakki-ui`, F-SPLIT-03). The tile apps that
*are* separate (WASM tiles from the store, `iyakki dev push DIR --watch`) already reload without a restart. Therefore every change to a built-in tile goes through section 3.

## 3. The in-place re-exec of the core (PID 1)

Like `systemd daemon-reexec`. The kernel keeps the process id; the program image is replaced.

1. **Request** (built 2026-11-16, tested with a fake exec only): `iyakki live-restart PROGRAM` leaves a request file (refused unless the flag file exists and PROGRAM is an executable program file; good for 60 s); the running program looks every two seconds (a thread started only when the flag file exists) and carries out a fresh one with `perform`. Run it through `dev shell` (one approval card per command, shown on the device) or a chat tool with an approval card. Refused unless the flag file exists, the
   device is not in the "reboot needed" state, and no sign-in or vault operation is half done.
2. **Prepare** (`liverestart::prepare`, built): check the new file (regular, over 1 KB, ELF header); copy the *running* program to `/run/iyakki/live-restart/previous` (memory, 0755);
   list the core's children with pid, name and start tick (`/proc`); write `state.json` atomically (phase `exec`). No secret goes in it.
3. **Secrets** (`hand_over`/`take_over`, built): what must stay unlocked (the vault key of the signed-in sessions) goes into a `memfd`: RAM only, never a disk, close-on-exec switched off so the
   new program inherits it; only the descriptor number is in the state file. `take_over` does not close it, so a rollback can pass it on again; `mark_healthy` closes it.
4. **Exec** (`perform` + `RealExec`, built): `exec()` of the new program with the same arguments. Same pid, same parent (the kernel), same children, same open memory files. Rust's own
   descriptors (screen, input, sockets) are close-on-exec and are opened again by the new program.
5. **Start** (`liverestart::begin`, built; the call in `main` is not): first thing in the new program. It sees phase `exec`, writes `starting`, and `adopt` compares the listed children with
   `/proc`: same pid **and** same start tick **and** still our child = adopted; a pid reused by another process, or a child that ended, is reported as lost so its owner starts it again.
6. **Health** (`mark_healthy`, built; the call site is not): the new program reports healthy when the shell has drawn its first frame and the services answer. It does this where it confirms the
   start to the boot loader (`bootconfirm`).
7. **Watchdog and rollback** (`watchdog`, `rollback`, built): a thread of the new program waits `HEALTH_SECONDS` (20) for the report. Without it: write `reboot-needed`, set phase `rolled-back` and
   `exec()` the kept `previous` program. That program sees `rolled_back` in `begin`, adopts the children again, and refuses another live restart until the device restarts (`/run` is memory, a
   reboot clears the block). If even that exec fails the message says "restart the device".

## 4. What is not built, and why it is the hard part

* **Re-entry of the start sequence.** `serve` as PID 1 does one-time work (mount `/proc`, early boot, driver loading) and starts background threads (late boot, hotplug, storage watching, the
  web service, the supervisor threads). After an exec the one-time work must be skipped (the `/proc/self` check already treats it like a container start) and the **thread starts must be
  repeated**. That needs the start sequence split into "once per boot" and "once per process", a refactor of `main.rs`/`boot.rs` that is best done with a QEMU run at every step. Until then a
  re-exec would leave the new program without its watchers. This is why the module is not wired in.
* **Signed-in sessions.** The shell holds sessions in memory. Handing over the vault key is built (3); serialising the sessions (who is signed in, which app is open) into the state file is not.
* **Zombies and signals.** The new PID 1 must reap children that ended during the exec (the first `waitpid(-1)` loop does); the signal mask and dispositions are inherited and must be reset at start.
* **A crash of the new program** before the watchdog thread exists is a kernel panic (PID 1 died). Covered only by the existing guard (`patches/boot-count`, the loader's try counter): the device
  reboots and the stored patch is switched off after repeated failures. A live restart therefore never leaves a device that cannot start by itself, but a crash costs one reboot.
* **A change of on-disk or socket formats between the old and the new program.** Both programs must read the state file (`format`), the data folder (the existing migrations are two-phase) and
  the socket protocols they share with the children (k3s, audio helpers). A change that breaks that is a `reboot` change by hand; the loop cannot tell.

## 5. Test status

| Part | Test | Proof |
|---|---|---|
| state file, refusal when off, ELF check, no state left after a refusal | unit tests in `liverestart.rs` | run here |
| adoption with a reused pid, lost children | unit tests with a fake `/proc` | run here |
| health in time, no health: rollback exec, block until reboot, failed rollback exec | unit tests with a fake `Exec` and clock | run here |
| secret in a memory file (not in the state file, readable twice, closed when healthy) | unit test | run here |
| real `exec()`: same pid, running child still alive, secret and state read by the new program | unit test with a child process and `/bin/sh` as the "new program" | run here |
| the whole in a booted guest (PID 1 re-exec, shell drawn again, k3s kept) | QEMU: `dist/buildroot/qemu-test.sh` boots a real image, but the wiring in 4 must exist first | **not run** |
| on a real device (screen, input, audio, k3s, signed-in session) | Manual needed | **not run** |

## 6. Safe fallback order

1. Refused before anything changes (flag off, bad file, reboot-needed state): nothing happened.
2. exec fails: the old program keeps running, state removed.
3. New program does not report healthy in 20 s: the old program is started again, live restart is blocked until a reboot.
4. Old program cannot be started: "restart the device"; the boot loader's own try counter and the stored patch guard protect the next start.

## Vault key hand-over (2026-11-16: built, tested without a guest)

Built as designed, with two differences: the descriptor number travels in the environment variable `IYAKKI_LIVE_KEYS_FD` (set by a `before_exec` hook, read once by the new program's `build_shell_env`) and not in the state file, and the keys are packed by `iyakki-agent::livekeys` (JSON, hex). `LocalVault::unlock_material` and `unlock_with_material` are in `iyakki-agent::vault`; `Services::export_session_keys` and `import_session_keys` in `services.rs`; `on_resume` reports a vault that was handed over as open. Tests: a vault exported and imported into a second `Services` over the same files opens without the password, a wrong key opens nothing, an invalid name or kind is ignored, and no key is found in any file of the data folder. Not run: the exec with the descriptor surviving on a booted guest.

1. **What is handed over.** For each signed-in person, the unlocked key material of the personal vault and, for an administrator, of the system vault: 32 bytes each, never the password. New in `iyakki-storage::vault` (the vault module): `LocalVault::unlock_material() -> Option<Zeroizing<[u8; 32]>>` (only while unlocked) and `LocalVault::open_unlocked(path, slot, material)` (checks the material against the vault's own check value, so a wrong or stale key is refused and the vault stays locked).
2. **Who packs it.** `Services::export_session_keys() -> Vec<SessionKey { user, kind, material }>` and `Services::import_session_keys(...)`. Neither writes to disk or to the log; the audit line says only how many were handed over.
3. **The channel.** `perform` is given the memory file from `hand_over("session-keys", json)` as `secret_fds` (close-on-exec off for it only). The state file names the descriptor number, never the content. The new program calls `take_over(fd)` first thing in `main`, before the shell exists, imports the keys, and closes the descriptor when it reports healthy (the existing release rule).
4. **Rules.** Only a live restart hands keys over: a crash resume (F-SHELL-65) never does. Only people who are signed in at that moment. If the new program fails to start, `perform` closes the descriptor and the old program still holds its own keys. The memory file is sealed against writes after it is filled and has mode 0600.
5. **Tests that can run without a guest.** A vault unlocked with a password, exported, imported into a second `Services` over the same files, opens without the password; a wrong key is refused; nothing readable lands in the data folder or in `state.json`; a failed exec leaves the old vault open.
6. **What cannot be shown here.** The exec itself with the descriptor surviving (`/bin/sh` as the new program is already the pattern of the existing unit test) and a real signed-in session on a booted guest.

## What the new program reports, and what the old one does before the exec

* The new program reports healthy from the shell thread right where the first frame (the sign-in screen) is about to be drawn (`report_healthy_soon`), after two seconds. Before this round it was a timer in `main`.
* Before the exec the running program runs the registered hooks (`before_exec`): the shell loop is told to leave so the screen is let go, then the disks are synced. Threads that are still running die with the exec; children are adopted by pid and start tick.
