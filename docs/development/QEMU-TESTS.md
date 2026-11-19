# QEMU session tests

**For:** anyone who wants a feature row proved against the real image instead of fakes. **Status:** opt-in; not part of the test layers (`test-core`, `test-apps`) or of `alpha-check`.

A QEMU boot of the built image is an honest real run for software behaviour (the real kernel, the real program, the real developer-mode channel). It is not a run on
real hardware: cameras, GPUs, Wi-Fi, touch and a real display stay with the hardware tests. A row is raised to Test 70 only for what a check below actually ran.

## What it does

`dist/qemu/session-checks.py` boots an existing image **once** (reusing `dist/devtest/qemu_guest.py`: a scratch account made by typing, developer mode switched on from Settings,
this computer paired with the code read off the guest's screen, the shell approval answered on the guest's own screen), then runs every check of
`dist/qemu/session-checks.list` and prints `PASS`, `FAIL` or `SKIP` per row id with the evidence. A check looks at the guest three ways: the serial console log, the
developer-mode API (`iyakki dev info|doctor|diag|status|state|...`) and commands run inside the guest (`iyakki dev shell`, as the restricted account).
Everything is on 127.0.0.1; the guest is deleted at the end; no real machine is touched.

## Profiles and screen driving

The list file groups checks by **profile**; one boot per profile serves all of its checks (`profile: ram`, `profile: data`, `profile: clock`).

| Profile | The guest | For |
|---|---|---|
| `ram` | the image, no data volume (the session lives in RAM; `/var/lib/iyakki` is readable by the guest shell, which runs as an unprivileged user) | most checks |
| `data` | an ext4 volume labelled `IYAKKI-DATA` on a virtio disk (made with `mkfs.ext4`, no `iyakki-usb` needed), so `/home/admin` exists | checks that save into the home folder (editor, DOCTOR share) |
| `clock` | the virtual hardware clock set to 2024-01-01 (`-rtc`) | the clock floor at boot |

A check can drive the guest's own screen the way a person does: `dist/qemu/session_ui.py` wraps `iyakki dev input key|text|click|tap` and `dev state` (open a tile through its group, open a Settings page through its search box, click by label, tap a drawn line by its place, wait for text). The guard rules of `dist/devtest/devui.py` apply: never a password, never Esc, q, o, p or r on Home, never an approval card by dev input. The pictures used as evidence come from `dev screenshot` (the shell's own picture) or, for what only the display shows (the screen saver, pane frames), from QEMU's `screendump` through the monitor. Text drawn by an engine (the editor, the Browser tile) is read with `tesseract` on a cropped, enlarged picture, or compared pixel by pixel. Pages for the Browser tile are served by the guest itself through Settings, Shared folders (website mode).

## Run it

```
./run.sh test-qemu --list                          # the checks and the rows each one proves
./run.sh test-qemu [BUILD_DIR]                     # one boot, every check
./run.sh test-qemu --only pkg_trust_levels,device_name BUILD_DIR   # one boot, some checks
```

`BUILD_DIR` (or `$IYAKKI_QEMU_BUILD`) holds `images/bzImage` and `images/rootfs.cpio.zst` (a lean-image output folder works; `iyakki-install-x86_64.img` and `iyakki-usb`
are used for a data disk only when present). `$IYAKKI_CLIENT` is a built `iyakki` (the runner builds nothing). Needs `qemu-system-x86_64` (KVM recommended),
python3 with Pillow and `tesseract`. Set `TMPDIR` to a folder on a disk with space (never `/tmp`). The boot and pairing take about a minute with KVM, the checks seconds.
Exit status: 0 all selected checks passed, 1 a check failed, 2 the guest could not be started or paired.

Evidence goes to `~/iyakki-work/qemu-session-runs/<date-time>/` (`latest` links to the newest): `results.json`, one `NAME.txt` per check, and `serial.log`.

## As Rust tests

`crates/iyakki-core/tests/qemu_session.rs` has one `#[ignore]`d test per check (same name), so a row's Tests column can cite `iyakki-core::qemu_session::NAME`.
`IYAKKI_QEMU_RESULTS=<run folder> cargo test -p iyakki-core --test qemu_session -- --ignored` reads the run's `results.json` and asserts each check passed: the tests
boot nothing, so one boot serves all. `run_all_in_qemu` starts the runner itself. Two tests that are not ignored keep the list, the Python functions and the Rust tests in step,
and check that the default layers do not run this.

## Add a check

1. Write `check_NAME(s)` in `dist/qemu/session_checks_defs.py` (serial, dev API, shell commands) or `dist/qemu/session_checks_ui_defs.py` (screen driving) (`s.serial()`, `s.dev(...)`, `s.sh("...")`; return the evidence, `raise Fail` or `Skip`).
2. Add a line to `dist/qemu/session-checks.list` under its `profile:` header: `NAME | F-ROW,F-ROW | what it looks at`.
3. Add `#[test] #[ignore] fn NAME() { status_of("NAME"); }` to `qemu_session.rs`.
4. Run it. Only a row whose check printed PASS goes from Test 50 to 70: cite the script and the Rust test in Tests and write in Notes what ran and what did not.

## Layering

The runner is a third layer in waiting: `dist/release/test-qemu.sh` is the entry point a layer stamp can call later (the first two layers are `test-core` and `test-apps`,
`dist/scripts/crate-layers.txt`). Nothing in `test-layer.sh`, `alpha-check` or `run.sh`'s default paths calls it today.

## Not covered by this harness

A data volume (no `iyakki-usb` in the run, so the session lives in RAM), a second boot (reboots, updates, unclean power-off: see the single-purpose scripts in `dist/qemu/README.md`),
the dev cluster (not in the lean image), models, audio you can hear, anything that needs a camera, GPU, Wi-Fi or touch.

## Planned checks: app events (F-SDK-209)

For the QEMU agent to write (profile `ram` unless said). SAMPLE is the built-in app that holds the event scopes, so its Events pane is the thing to look at (`dev state` text, or the shell's own screenshot).

| Check name | Rows | What it does and looks at |
|---|---|---|
| `app_events_connectivity` | F-SDK-195, F-SDK-190 | Open SAMPLE, tab Events (`Network: online` or `not heard yet`). Take the guest's link down (QEMU monitor `set_link net0 off`), wait a few seconds of ticks, read `Network: offline` and a line `network offline` without `(late)`. Link up again: `online`. |
| `app_events_late_after_away` | F-SDK-188, F-SDK-190 | Open the Events pane, go Home (Esc is not allowed: use the dock/Home click), change the link while SAMPLE is closed, open SAMPLE again: the line says `(late)` and no note appears in the inbox. |
| `app_events_survive_restart` | F-SDK-190 | With SAMPLE closed, change the link (events wait), write `appevents/` file names with `iyakki dev shell` (`ls` of the data folder shows `<user>.sample.json`), restart the guest session (dev reboot, same data volume: profile `data`), open SAMPLE: the waiting event arrives `(late)` and the registration was kept. |
| `app_events_lock` | F-SDK-196 | Lock the screen (Super+L is a key chord: use the Power menu), unlock by typing the scratch account's password (allowed only for the scratch account made by the runner), read `Session: locked` then `Session: unlocked` in SAMPLE's Events pane (SAMPLE now registers for `session.changed`). |
| `app_events_settings_switch` | F-SDK-192 | Settings, App events: the row `sample: connectivity.changed` is there; turn it off by label (`Turn off`), change the link, SAMPLE shows nothing new; `app_events_list` through chat shows `switched off`. |
| `app_events_flow_and_settings` | F-SDK-199 | Change a setting by Settings (for example the time server) with SAMPLE open on a registered `settings.changed`; read `Last setting changed: ...` in SAMPLE's Events pane (SAMPLE now registers for `settings.changed`). |

Notes for whoever writes them: the pane polls once a tick and the sources look at most once a second, so wait two seconds of guest time after a change. The Events pane is drawn text, so read it with `dev state`. Never type a password except for the scratch account. A check passes a row from Test 50 to 70 only for what it printed PASS for.

More checks, written 2026-11-18 (F-SDK-209; they are in `dist/qemu/session-checks.list` and `dist/qemu/session_checks_ui_defs.py`, none has been run yet, so each stays Test as it was until a run says PASS):

| Check name | Rows | What it does and looks at |
|---|---|---|
| `app_events_foreground` | F-SDK-188, F-SDK-209 | Open SAMPLE's Events pane, Home, Settings, App events, and SAMPLE again: the Heard log has an `app <name> foreground` or `background` line (`app.lifecycle`, scope `events.apps`, which SAMPLE holds). |
| `app_events_volume` | F-SDK-197, F-SDK-209 | The host makes a small ext2 image; the QEMU monitor hot-plugs it (`drive_add`, `device_add virtio-blk-pci`); the guest mounts it through `dev shell` and unmounts it; the Heard log has `volume <mount point> mounted` then `unmounted` (`storage.volume`). A host without `mkfs.ext2` or a guest without the bus is a SKIP. |
| `app_events_usb` | F-SDK-203, F-SDK-209 | The monitor plugs a `usb-mouse` in (`device_add`) and takes it out (`device_del`); the Heard log has `usb added <product> (<ids>)` then `usb removed` (`usb.device`, scope `events.devices`, which SAMPLE now holds). Needs a USB controller in the guest: without one it is a SKIP. |

Not drivable from a QEMU guest, so they stay unit tests: the sign of sharing (`sharing.changed` needs a paired client device), the zone from the network (`location.changed` needs the person to let the network choose), an app's own storage (`storage.quota`), an app asking to be woken (`app.wake`), and the tool request an app makes (`app_actions_pending`; restart keeps it, covered by `iyakki-agent` tests).
