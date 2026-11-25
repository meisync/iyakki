# Lifecycle audit (lane LC1, alpha.10, 2026-11-24)

The owner found that sound kept playing after sign-out (lane SO1 fixes sound, capture and the end-of-session registry; the rule is
D-SESSION-END in ALPHA-8-DECISIONS.md). This audit asks the same question for every other moment in the life of a running computer:
what do the long-lived things do, what would a person expect (Linux, macOS and Windows desktops), is that what the code does.

Method: the code was read (paths are under `crates/`); nothing was run on hardware or in QEMU. Verdicts: **right** (matches the
expectation), **gap** (a specific failure, with its severity), **unknown** (the code was not followed to the end; it is a question,
not a finding). "Fixed" names the gap id of this lane (L-nn). Severity classes, highest first: **S1** sound or capture continuing when
it should not, **S2** data loss, **S3** silent failure, **S4** wrong state shown.

There is no fast user switching: the shell holds one `session` (`iyakki-shell/src/shell.rs`), a second person must sign in after the first
signed out (or sees the lock screen of the first). "User switch" below is therefore sign-out then sign-in, plus browser sessions
(`iyakki-remote/src/session.rs`), which are per person and independent of the console.

## 1. The table: moments by long-lived things

| Moment | Thing | What the code does today (cites) | Expected | Verdict |
|---|---|---|---|---|
| Lock / idle | Playback (radio, media, TTS) | Keeps playing; the idle count restarts while music or video plays (`iyakki-shell/src/shell_lock.rs` header, `lock_tick`) | Same on all three desktops | right |
| Lock / idle | Microphone, camera, screen capture, wake listening | `lock_screen` (`shell_lock.rs:52-95`) closes vault, encrypted home, browser data; it stops no capture. D-SESSION-END says capture stops on lock; the registry is lane SO1 | A locked machine must not keep recording on its own | gap S1 (owner SO1, P-03) |
| Lock / idle | Jobs, flows, forwards, VPN, downloads | Go on behind the lock, by design ("apps and runs go on") | Same | right |
| Lock / idle | Vault, encrypted home, browser data | Sealed at lock when chosen; unlock re-opens with the typed password (`shell_lock.rs:65-81, 207-233`) | Same | right |
| Lock / idle | Idle counter vs sleep | `Instant` based (`shell_saver.rs`), monotonic; sleep also locks | Same | right |
| Suspend / lid | Sleep entry | `sleep_now` locks first (guest: signs out), audits, then `host.sleep()` (`shell_fnkeys.rs:263`); lid and button go through `power::lid_step` (`shell_power.rs:49`); busy things (jobs, installs, update) keep it awake | Same | right |
| Suspend | Speakers | `boot::sleep` calls `quiet_sound()` (the shutdown routine: volume to 0, SIGTERM then SIGKILL every process with a playback stream open, `iyakki-agent/src/quiet.rs`). Nothing put the volume back and the softvol cached "level already set", so sound returned silent | Sound returns at the old volume | gap S4, **fixed L-01** |
| Suspend | Radio / media after wake | The players are killed by the quieting; the radio's retry loop (`radio.rs:520-560`, `radio_recover.rs`) probably reconnects or shows Failed; the media queue marks Failed | Resumes or shows paused | unknown (P-04) |
| Suspend | Mic / camera / wake word / TTS | No suspend hook anywhere (`boot.rs sleep()` only quiets, saves the clock floor, syncs). Streams are not closed first, USB devices may reset on wake | Capture ends or is re-opened cleanly | unknown (P-03) |
| Suspend | VPN | `vpn_mgr.rs:1091-1235`: stale handshake and `moved_or_resumed` rebuild the tunnel; status says "waiting for the server" | Same | right |
| Suspend | SSH tunnels | `ServerAliveInterval=15`, `CountMax=3`, restart with back-off 2 s to 300 s (`sshtunnel_mgr.rs:114, 394-460`) | Same | right |
| Suspend | Flows scheduler | Ticks missed while off are handled by the missed policy (`iyakki-flow/src/sched.rs:462-470`) | Same | right |
| Suspend | Clock | `clock_floor_early()` after wake moves the clock forward only (`iyakki-clockfloor/src/lib.rs`) | Same | right |
| Suspend | Browser (remote) sessions | Idle limit 8 h used `Instant`, which does not run during sleep, so a day of sleep did not end a session | A day of sleep is a day idle | gap S3, **fixed L-07** |
| Suspend | Battery countdown, NTP, Wi-Fi/DHCP, Bluetooth | Battery reads again at the next heartbeat (`shell_battery.rs`); `udhcpc` and `wpa_supplicant` retry by themselves; the NTP thread retries (`ntp.rs:238`) | Same | right (Bluetooth: unknown) |
| User switch | Radio, media, capture, terminals of the first person | Only `soundowner::signed_out` and a hand list in `Shell::sign_out` (`shell.rs:1644-1690`): flows, shared folders, tunnels, VPN, vault, assistant tab, browser data | Sign-out ends the person's interactive things | gap S1 (owner SO1; process kinds SO2) |
| User switch | Browser sessions | Per person, up to 32, idle 8 h, CSRF token; sound owned by whoever signed in last (`soundowner.rs`, experimental) | Same | right (sound: SO1) |
| Network loss | Link state shown | `appevent_facts::link_state` said `online` for any non-loopback interface with operstate up: a cable without a lease, an access-point or tether link, read online | Online means a way out | gap S4, **fixed L-04** |
| Network loss | Link, DHCP, Wi-Fi | `udhcpc -b -R`, `wpa_supplicant -B` retry by themselves (`net.rs:270`, `iyakki-devctl/src/mac.rs:464`) | Same | right |
| Network loss | Radio | Retries twice, then the next station, then Failed after a full pass (`radio_recover.rs`); it never starts again when the network returns | Players keep trying or resume | gap S3 (P-01) |
| Network loss | Media downloads | A failed or restart-interrupted download is Failed and needs a manual retry (`mediaqueue.rs:106-115, 222-244`) | Resume when the network returns | gap S3 (P-06) |
| Network loss | SSH tunnels | Dead in about 45 s, restarts with back-off up to 300 s, hard faults say what is needed | Same | right (back-off could reset on a route, P-11) |
| Network loss | VPN | Tick watches handshake age; interface loss tears down, kill switch kept if asked, retry with back-off; status honest | Same | right (small: roam with no uplink, P-12) |
| Network loss | Flows | Per node timeout and retry (`iyakki-flow/src/driver.rs:1115`); the HTTP health check used a plain `TcpStream::connect` (two-minute hang on a black-holed address, `healthcheck.rs`) | Bounded | gap S3, **fixed L-08** |
| Network loss | Cloud link / remote client | Pings 2 s, silent 20 s, reconnect with back-off, status Reconnecting; gives up for good after 120 s with "the tenant did not answer" (`iyakki-link/src/cloudclient.rs:154, 727`) | Keeps trying while it is the person's choice | gap S3 (P-02, owner decision) |
| Network loss | k3s dev cluster | Waits for a default route and for the clock, says so (`k3sdev.rs:73-160`) | Same | right |
| Network loss | Internet sharing | Stops with `UplinkLost`, state Failed, no auto-resume (`sharing/manager.rs:304-327`) | Resumes when the uplink returns | gap S3 (P-07, design choice) |
| Network loss | NTP | Retries 5, 10, 20, 40, 60 s until it works, then every 4 h; no sync on route return | Sync soon after the network returns | gap S4 (P-10) |
| Network loss | Streams without a body timeout | `UreqStreamer` bounds connect (10 s) and first byte; a silent mid-stream death waits for TCP (chat stream); downloads use `guard::Limits::DOWNLOAD` and radio its own 15 s sound guard | Bounded | unknown (P-13) |
| Low battery / power loss | Warnings and shutdown | `iyakki-battery` alerter: warnings, countdown, then the same safe power-off as the menu (`shell_battery.rs`); `steady_secs` is monotonic, so a clock change does not touch it | Same | right |
| Low battery | Things in flight at shutdown | `boot::power_off` quiets sound, saves the clock floor, syncs; flows and jobs are not asked to stop (flows recover as Interrupted at the next start, `iyakki-flow/src/engine.rs:924`) | Same | right |
| Disk full | Audit log | `AuditLog::append` swallowed every open and write failure: the hash chain silently lost lines (`iyakki-accounts/src/lib.rs`) | A lost security event must be visible | gap S3, **fixed L-09** |
| Disk full | Camera recording | Free-space floor stops recording with a visible state, resumes at 110 % of the floor (`camrec.rs:1119, 1166-1172`) | Same | right |
| Disk full | Atomic writers | `iyakki_schema::write_atomic` (tmp, fsync, rename, remove tmp on error): old file intact; the message is the OS text | Same | right (message thin) |
| Disk full | Chat/story saves and flow run records | Direct `std::fs::write` (story) and rename without fsync (flow run store): a full disk or a power cut left a truncated or empty file | Whole old file or whole new file | gap S2, **fixed L-10** (other writers P-14) |
| Disk full | Settings, downloads | Settings go through atomic stores; downloads use `.part` files (`remotefiles.rs:837-904`) | Same | right (other downloaders unknown) |
| Disk full | Is the person told | `app events` has `low_space`, Doctor has a disk finding; writers return their error to the caller | A warning before it is full | right (not traced to a toast) |
| Time jump | Cron and interval triggers | `next_ms` was absolute: after a clock set back a trigger stayed silent for as long as the clock was moved (`iyakki-flow/src/sched.rs:408-470`) | Next run in the new time | gap S3, **fixed L-05** |
| Time jump | Missed runs after a forward jump | Missed policy (skip, run once, catch up) | Same | right |
| Time jump | Backup schedule | `next_slot` from `last_run`; a `last_run` in the future (written under a wrong clock) held backups off until the clock caught up (`iyakki-shared/src/backup_schedule.rs`) | Backups keep running | gap S2, **fixed L-06** |
| Time jump | Audit timestamps | Wall clock; the hash chain stays valid but a line could look older than the one before it | Non-decreasing | gap S4, **fixed L-12** |
| Time jump | Sign-in rate limits, idle limits | `Instant` based (`iyakki-remote/src/session.rs`); `console.rs:106` uses `saturating_sub` | Same | right |
| Time jump | Device-key sign-in skew, firewall `expires` | u64 seconds from the wall clock (`iyakki-remote/src/server.rs:271`, `devconfig.rs:129`); a wrong server clock rejects every device-key login | Same on all desktops | unknown/minor (P-15) |
| Hot-unplug | USB drive with a file open or a copy running | Mount, unlock and eject refuse a drive that is gone with a sentence (`iyakki-core-local/src/disks.rs:558, 794`); the backup writes `name.part` and renames, the half file is removed on error (`iyakki-shared/src/backup.rs:611-688`); no "the drive was removed while X was open" message and no sweep of stale `media/<label>` folders | A clear message; no stale folder | gap S4 (P-08) |
| Hot-unplug | Sound card | The output is re-routed within seconds, the dead pin dropped (`audioout.rs:362`); a player that gets EPIPE ends the session (`audiosink_stream.rs:60-72`); whether the radio re-resolves the output between retries was not traced | Sound moves to the next device | right (radio retry unknown, P-16) |
| Hot-unplug | Camera | Recording stops with "the camera was removed", segment closed, Problem state visible (`camrec.rs:1163, 1385-1400`); preview not traced | Same | right (preview unknown) |
| Hot-unplug | Bluetooth headset | The route removes itself, speakers play again and the reason is kept (`btroute.rs:1-8, 192`) | macOS and Windows pause the music | gap S1 (P-05: music continues on the room speakers) |
| Hot-unplug | Display | Panes move to the remaining screens, a toast names what moved or closed, the layout is kept (`shell_displays.rs:112-216`) | Same | right |
| Crash / restart | One app panics | Only that app closes, crash card, audit line (F-SHELL-40) | Same | right |
| Crash / restart | Shell crash | The loop restarts; session restore (F-SHELL-65) is developer mode only (owner decision, KI-121): a normal person gets the sign-in screen and loses open apps, ptys and unsaved content, with no word saying why | A message "your session ended unexpectedly" | gap S3 (P-09) |
| Crash / restart | Flow runs | `recover` at the next start: idempotent steps back to pending, others `Interrupted` with a reason (`iyakki-flow/src/engine.rs:924`) | Same | right |
| Crash / restart | Jobs | `lost` state for a vanished process (`iyakki-agent/src/jobs.rs:11`) | Same | right |
| Crash / restart | Leftover temp files | `write_atomic` and the run store leave `*.tmp<pid>-<n>` after a crash; nothing removed them | Cleaned at start | gap S3, **fixed L-11** |
| Crash / restart | Linux windows (sandboxes), ptys after a core crash | Window dropped when its tile closes (`iyakki-tile-linuxwin/src/lib.rs:405`); processes after a core crash not checked | No orphans | unknown (P-17) |
| Crash / restart | Downloads | `.part` file, retry by hand | Resume | gap S3 (P-06) |

## 2. Ranked gaps

Fixed in this lane (each has a unit test; rows F-LIFE-01 to F-LIFE-10):

1. **L-01 (S4)** volume stayed at 0 after sleep. Fix: `Softvol::forget_level`, `Selector::after_resume`, `quiet::restore_after_resume`, called by `boot::sleep` after wake.
2. **L-10 (S2)** story, chat and flow run files could be truncated or empty after a full disk or a power cut. Fix: `write_atomic` in `iyakki-shared::story`, `iyakki-agent::story`, `iyakki-flow::runstore`.
3. **L-06 (S2)** a backup time in the future (wrong clock) paused scheduled backups. Fix: it counts as never run.
4. **L-09 (S3)** audit lines lost on a full disk, silently. Fix: a count per log file and an `audit.gap` line with the number at the next write.
5. **L-05 (S3)** a clock set back silenced cron and interval triggers. Fix: re-anchor to the first tick after now.
6. **L-07 (S3)** a browser session survived a long sleep. Fix: idle by the wall clock too (a clock set back counts as no time).
7. **L-11 (S3)** leftover temp files after a crash. Fix: `iyakki_schema::sweep_stale_tmp`, run at shell start, audited as `data.tmp.swept`.
8. **L-08 (S3)** the health check could hang two minutes. Fix: `connect_bounded`.
9. **L-04 (S4)** "online" with no way out. Fix: `link_state_routed` (needs a default route through an up interface), used by the `link` app event.
10. **L-12 (S4)** audit time could go back. Fix: a line is never older than the line before it.

Not fixed (by severity; each is a parked row and a KNOWN-ISSUES draft in section 3):

| Id | Sev | Gap | Why not here |
|---|---|---|---|
| P-03 | S1 | Mic, camera, screen capture and wake listening are not stopped by lock, idle or before suspend | The end-of-session registry and the lock/idle stop are lane SO1 (D-SESSION-END); suspend needs the same registry (an on-suspend kind) |
| P-05 | S1 | A Bluetooth headset leaving moves the music to the room speakers | Needs the media player's pause from the route-lost event; touches the media facet (core-api) |
| P-01 | S3 | The radio gives up for good after an outage longer than its retries | A state machine change in `radio_recover.rs` with an owner choice (keep trying, or stop and say so) |
| P-06 | S3 | Failed and interrupted media downloads need a manual retry; no resume | Needs a route-return event and Range support |
| P-09 | S3 | After a shell crash a normal person gets no word that the session ended; open work is lost | F-SHELL-65 is dev-only by owner decision |
| P-02 | S3 | The cloud client gives up after 120 s | Owner decision (it says so in words today) |
| P-07 | S3 | Internet sharing does not resume after the uplink returns | Design choice; needs a Waiting state |
| P-04 | S3 | Players are killed by the shutdown-style quieting at suspend; state after wake unknown | Needs a measurement on hardware |
| P-08 | S4 | Unplugging a USB drive with files open: no message, stale folder | Needs the disk snapshot diff in `disks.rs` |
| P-10 | S4 | NTP waits up to 4 h after the network returns | Needs a route-return event |
| P-11/12 | S4 | SSH back-off up to 300 s after an outage; VPN roam with an empty interface name | Small, unmeasured |
| P-13 | S4 | Chat stream body has no idle timeout; yt-dlp and the Pi firmware download have no retry | ureq's body timeout is total, not idle: needs an idle-timeout reader |
| P-14 | S2 | Other plain `std::fs::write` of persistent state: `bundle_members.rs:72,197`, `fleet_context.rs:76`, `localreg.rs:254,265`, `embedbrowser.rs:72`, `quickinstall_ollama.rs:47`, `lockscreen.rs:76`; tmp and rename without fsync: `media.rs:178`, `browserplugins.rs:171`, `datamigrate.rs:92`, `webaccess.rs:99` | One mechanical pass, left for the next lane to keep this one reviewable |
| P-15 | S4 | Device-key skew and firewall expiry on a wrong server clock | Needs a policy (accept skew? warn?) |
| P-16/17 | S4 | Radio output re-resolve after a card loss; sandboxes and ptys after a core crash | Unknown; needs a run |

## 3. KNOWN-ISSUES drafts (triage rule: one entry per defect, cause, evidence, where it is fixed or parked)

Numbers assigned 2026-11-24: KI-LC-A to H are KI-1022 to KI-1029 in KNOWN-ISSUES.md.

* **KI-LC-A (P-03)** Microphone, camera and screen capture keep running while the screen is locked and when the machine goes to sleep. Cause: `lock_screen` and `sleep_now` call no stop; D-SESSION-END makes it a registry hook. Parked: row F-LIFE-13 (with SO1).
* **KI-LC-B (P-05)** Music moves to the room speakers when a Bluetooth headset disconnects. Cause: `btroute` falls back to the speakers by design. Parked: F-LIFE-14.
* **KI-LC-C (P-01)** The radio stays stopped after a long network outage. Cause: `radio_recover.rs` stops after one pass of failing stations. Parked: F-LIFE-11.
* **KI-LC-D (P-06)** Media downloads interrupted by a lost network or a restart stay Failed until retried by hand. Parked: F-LIFE-15.
* **KI-LC-E (P-09)** After a shell crash a normal person is not told what happened. Parked: F-LIFE-16 (message only; restore stays dev-only).
* **KI-LC-F (P-02, P-07)** The cloud client and internet sharing do not come back by themselves after a long outage. Parked: F-LIFE-17.
* **KI-LC-G (P-04, P-08, P-10 to P-13, P-15 to P-17)** Unmeasured or small lifecycle items listed in section 2. Parked: F-LIFE-18, F-LIFE-19.
* **KI-LC-H (P-14)** Persistent state written without the atomic writer. Parked: F-LIFE-20.

## 4. What is not yet known (needs a run, not code)

Whether the radio and the media player come back after a real suspend on the T2 Macs and the HP; whether a USB webcam survives a
suspend (the driver may reset the device); what the wake word does on resume. These are owner round items (Manual `needed`).
