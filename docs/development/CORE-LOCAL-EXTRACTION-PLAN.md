# Extracting the remaining kernel access into iyakki-core-local (F-ARCH-07)

Status: plan, lane P-B round 8, 2026-11-13. Nothing in this file is built beyond what the UI/OS split plan already lists as done.

## Where we are

`iyakki-core-api` holds the interface (traits and pure parsers), `iyakki-core-local` the one implementation that opens files and starts programs, `iyakki-core-fake` the in-memory one. Tiles ask `cx.os`. Moved so far: sysinfo, audio, disks (space), net (addresses), and in round 8 `memory()` (zram, swap, `system.conf`, data place).

## Count of direct kernel reads that remain (2026-11-13)

Counted with the patterns of `dependency_ratchet.rs` (a `/proc/`, `/sys/` or `/dev/` literal, `Command::new(`, a use of the devctl, storage, usb, drivers, netscan, hwinfo or zram crate) over non-test code. The tile ratchet is exact; the agent and shell-outside-tiles figures are a cruder lexical count (test modules cut at the first `#[cfg(test)] mod`), so read them as the size of the job, not as a gate.

| Place | Lines | Files | Exact? |
|---|---|---|---|
| Tiles (`iyakki-shell/src/apps`, the allow list) | 44 after round 8 (devices.rs 33, system.rs 4, scan 2, settings_install 2, store_drivers 2, and one each in camera, chat, settings_crashlog, settings_time, store, story) | 11 | yes |
| Shell outside the tiles (devpop, memgate, btmodel, touchbar_host, fnkeys, about, shell, hostname, display, fb) | about 130 | about 25 | approximate |
| `iyakki-agent` | about 507: `/proc /sys /dev` literals 234, `Command::new` 80, storage 68, devctl 51, drivers 37, zram 15, netscan 13, usb 6, hwinfo 5 | about 130 | approximate |

The biggest agent files are disks_fake.rs, disks_part.rs, disks.rs, linuxapps_init.rs, settingstools_memory.rs, startupdisk.rs, fwextra_cli.rs, doctor.rs, sysinfo.rs, btaudiotools.rs, touchrecover.rs and macfw.rs. Several are the real implementation behind a trait that already exists (the agent's `Disks`, the storage `Mounter`), which makes them moves, not designs.

## Order

1. **Finish the tiles that already have a trait shape.** devices.rs (links, rfkill, Bluetooth, mac, sound cards, brightness): one `Devices` trait with `links()`, `rfkill()`, `set_rfkill(ids, on)`, `bluetooth()`, `mac_infos()`, `set_brightness()`; the types of `iyakki-devctl` are pure data and are re-exported the way `iyakki_core_api::memory` re-exports zram's. 33 lines in one tile; do it in three commits (links and rfkill, then Bluetooth and mac, then sound cards through the existing `Audio`).
2. **Storage layout** (`iyakki-storage`: block devices, mounts, crash log, loss rewording): one `Storage` trait. `loss::reword` reads machine state today, so it must become a trait method before camera.rs and chat.rs can drop their line. settings_crashlog and settings_install follow.
3. **Net scan**: `iyakki-netscan` carries an HTTP client, so the core API must not depend on it. The trait (`scan(targets, options, cancel, sender)`) and its plain data types go into the core API; the scanner stays in core-local. This needs the `Event`, `HostInfo`, `Service` and `Options` types split from the scanner first.
4. **The agent**, by weight: move the files with the most hits whose behaviour is already behind a trait (disks*, startupdisk, doctor, sysinfo), then the program starters (`Command::new`: 80 places; each becomes a named operation in core-local, listed in BOUNDARY.md as today).
5. **Gates that follow**: the `kernel_single_reader` pairs and the agent's device-crate dependencies count down per move; the allow list shrinks with every tile; the core surface file records every new trait with a note.

## Rules that stay

- A move is not a rename: the I/O goes behind a trait, the type of the answer is data, a `FakeOs` test drives the tile.
- Pure data and pure rules may be re-exported by the core API; anything that opens a file, starts a program or talks to a device is a trait method.
- One tile per commit, the allow list lowered in the same commit, a gate that breaks stops the run.

## Not decided

- Whether `Os` stays one trait with many accessors or splits into a few (it has five accessors now; a sixth for devices and a seventh for storage make the fake grow). Proposal: keep one handle, add accessors, and revisit at ten.
- Whether re-exporting a heavy crate's types through the core API is acceptable (it is for zram and the storage crash-log formatter, which depend on `libc` only; it is not for netscan).
