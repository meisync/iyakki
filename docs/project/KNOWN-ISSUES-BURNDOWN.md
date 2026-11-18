# Known issues burn-down plan

**Written:** 2026-10-28. **Owner request:** KNOWN-ISSUES.md is too big; "don't just clear, we have to FIX those".
**Docs only.** Nothing was built, run or committed for this page; no machine was reached. Evidence comes from reading the tree (much of it uncommitted
on 2026-10-28), `FEATURES.md`, `docs/project/HARDWARE-MATRIX.md`, `BETA-CHECKLIST.md`, and the coordinator's observations of 2026-10-26/28 on the T2 MacBook Pro
(image 1027e; observed through developer mode, **no saved `iyakki dev diag` file**, so those count as observations, not proof).

**Rule (owner, 2026-10-28):** no issue is deleted or hidden to shrink the file. An issue leaves `KNOWN-ISSUES.md` only when it is **fixed and there is evidence**
(a passing test name, a dated device check, or a code path that is commit-ready), recorded in the table of section 5. Nothing is removed in this pass.

## 1. Result of the check (66 handwritten bullets, ids KI-001 to KI-066)

Each bullet of `KNOWN-ISSUES.md` (lines 1 to 206) now starts with its stable id, `* KI-001 ...`; the text after the id is unchanged.

| Status | Count | Which |
|---|---|---|
| still true | 47 | all others |
| partly fixed | 16 | 005, 008, 009, 013, 016, 017, 020, 022, 026, 041, 044, 049, 058, 064, 065, 066 |
| fixed | 0 | none has both a code path **and** a test or a saved device file; the closest are 026, 013, 022 (below) |
| unclear | 3 | 007 (image vs Mac mini), 029 (were Gitea/store deployed to the Mac's k3s?), 063 ("0 warnings" vs a run that saw 10) |

| Class | Count | Ids |
|---|---|---|
| needs real-hardware check (HW) | 28 | 003 005 007 009 010 012 013 014 017 018 021 024 026 027 029 030 033 041 044 049 050 053 054 055 057 061 065 066 |
| missing feature (MF) | 9 | 015 020 022 025 034 039 047 048 051 |
| needs owner decision (OD) | 8 | 001 035 037 040 043 045 046 056 |
| to waive (W) | 8 | 008 011 016 019 032 038 042 052 |
| external dependency (EXT) | 5 | 002 006 023 031 062 |
| code bug (CB) | 4 | 004 028 036 060 |
| stale or informational (INFO) | 4 | 058 059 063 064 |

Wave counts: Wave 1 = 12, Wave 2 = 28, Wave 3 = 14, Wave 4 = 12.

Why nothing is "fixed" yet. The tree has code for many items, but the rule needs a test or a dated device file for each. Close calls:

* **KI-026 developer mode "loopback only"**: the Mac T2 MacBook was driven over Wi-Fi on 2026-10-26/27. No diag file is saved, and `FEATURES.md` F-DEVMODE-01 still says loopback only. Save one `iyakki dev diag` file and the bullet can be rewritten.
* **KI-013 camera picker / touch mapping on a second display**: owner-side observation 2026-10-27/28 (USB camera, USB microphone, camera picker, touch mapping on the external monitor). F-CAM-13 and F-DISP-07 notes still say "fake devices only". The sentence "Fixes built and not yet verified" is stale; the built-in microphone being off is still true.
* **KI-022 "Radio plays MP3 only"**: now false in the tree. HLS with MP3 segments is in-house (`iyakki-agent::radio::tests::an_hls_station_with_mp3_segments_plays_without_the_add_on_and_follows_the_live_edge`), AAC goes through the user-downloaded ffmpeg add-on (`...an_aac_stream_goes_to_the_decoder_whatever_its_content_type_says_and_its_sound_is_played`, fake decoder only). Ogg/Opus is not built. The text needs a rewrite, which is a text change, so it waits for the owner.

## 2. The generated appendix, by area

Source: current `FEATURES.md` (604 rows not `done`: 517 partial, 74 planned, 13 deprecated). **The appendix in KNOWN-ISSUES.md is stale: it says 579 (575 rows present); regenerate with
`dist/release/known-issues-appendix.sh`.** New or grown since: UPG, WEBSVC, NAME (new areas), FLOW 23 to 28, VOICE 12 to 18 (6 deprecated by the Indic engine removal), SND, RADIO, K3S, STARTUP.

Classification of the partial rows is **keyword-based on each row's Notes** (not read one by one), so use it for proportions only:

* **UNV = built, never verified on real hardware or a real service** (Notes say fake, scripted, never run, QEMU only, loopback, made-up /sys, not tried): **358 of 517 partial (69 percent)**.
* **MISS = built, missing a named part** (Notes say not written, not done, not built, missing): **51**.
* **OTHER = no keyword hit** (mixed, or a long note that needs a person to read): **108**, of which a share is stale (below).
* **Stale note** cannot be counted by keyword. Found by cross-reading while verifying the bullets, each says "fakes only" for something that has since run on the T2 MacBook:
  F-DEVMODE-01 (loopback only), F-DISP-07 (touch: no real panel), F-CAM-13 (no real USB camera, picker), F-CAM-12 (built-in camera frames never read), F-K3S-27 and F-K3S-28 (not run on a device),
  F-STARTUP-01 and F-STARTUP-02 (not seen on a real boot or screen), F-HW-02 (never run against real devices), F-DEVMODE-15 (firewall: fake backend only). Eight to ten rows; each needs a diag file
  or a dated sentence to be promoted (Wave 2).

| Area | Rows | Partial | Planned | Deprecated | UNV | MISS | OTHER |
|---|---|---|---|---|---|---|---|
| BROW | 55 | 47 | 7 | 1 | 20 | 14 | 13 |
| DRIVERS | 33 | 30 | 1 | 2 | 22 | 2 | 6 |
| FLOW | 28 | 25 | 3 | 0 | 19 | 1 | 5 |
| SHELL | 27 | 23 | 4 | 0 | 18 | 2 | 3 |
| BOOT | 25 | 17 | 7 | 1 | 11 | 2 | 4 |
| K8S | 21 | 20 | 1 | 0 | 17 | 1 | 2 |
| VOICE | 18 | 10 | 2 | 6 | 5 | 0 | 5 |
| APPS | 18 | 17 | 1 | 0 | 8 | 4 | 5 |
| IDE | 17 | 17 | 0 | 0 | 11 | 0 | 6 |
| ASSIST | 17 | 17 | 0 | 0 | 14 | 2 | 1 |
| GIT | 16 | 16 | 0 | 0 | 16 | 0 | 0 |
| SND | 14 | 12 | 2 | 0 | 11 | 0 | 1 |
| KBD | 14 | 12 | 2 | 0 | 9 | 0 | 3 |
| INST | 13 | 10 | 3 | 0 | 7 | 1 | 2 |
| EXTA | 13 | 13 | 0 | 0 | 10 | 0 | 3 |
| LINAPP | 12 | 11 | 1 | 0 | 7 | 2 | 2 |
| SETUP | 11 | 11 | 0 | 0 | 10 | 0 | 1 |
| TALES | 10 | 10 | 0 | 0 | 7 | 0 | 3 |
| NET | 10 | 9 | 1 | 0 | 8 | 1 | 0 |
| MEDIA | 9 | 8 | 1 | 0 | 7 | 1 | 0 |
| FW | 9 | 8 | 1 | 0 | 5 | 0 | 3 |
| PKG | 8 | 3 | 5 | 0 | 2 | 1 | 0 |
| DISKS | 8 | 7 | 1 | 0 | 6 | 0 | 1 |
| DEVMODE | 8 | 7 | 1 | 0 | 5 | 0 | 2 |
| DEV | 8 | 8 | 0 | 0 | 3 | 0 | 5 |
| APPSTORE | 8 | 6 | 2 | 0 | 3 | 1 | 2 |
| UPG | 7 | 6 | 1 | 0 | 1 | 3 | 2 |
| RADIO | 7 | 7 | 0 | 0 | 4 | 1 | 2 |
| DASH | 7 | 7 | 0 | 0 | 7 | 0 | 0 |
| CAM | 7 | 3 | 4 | 0 | 2 | 0 | 1 |
| REL | 6 | 5 | 0 | 1 | 4 | 0 | 1 |
| LOSS | 6 | 5 | 1 | 0 | 4 | 1 | 0 |
| WEBSVC | 5 | 5 | 0 | 0 | 2 | 1 | 2 |
| WEB | 5 | 2 | 2 | 1 | 2 | 0 | 0 |
| SYS | 5 | 5 | 0 | 0 | 3 | 1 | 1 |
| STORE | 5 | 2 | 3 | 0 | 1 | 0 | 1 |
| SCRN | 5 | 4 | 1 | 0 | 3 | 1 | 0 |
| MSG | 5 | 5 | 0 | 0 | 2 | 1 | 2 |
| MODELHOST | 5 | 5 | 0 | 0 | 4 | 0 | 1 |
| GPU | 5 | 4 | 1 | 0 | 1 | 1 | 2 |
| DISP | 5 | 5 | 0 | 0 | 4 | 1 | 0 |
| CLUSTER | 5 | 5 | 0 | 0 | 3 | 1 | 1 |
| CLOUD | 5 | 5 | 0 | 0 | 4 | 0 | 1 |
| BATT | 5 | 5 | 0 | 0 | 2 | 0 | 3 |
| UPD | 4 | 0 | 4 | 0 | 0 | 0 | 0 |
| SDK | 4 | 2 | 2 | 0 | 2 | 0 | 0 |
| NAME | 4 | 4 | 0 | 0 | 4 | 0 | 0 |
| LINWIN | 4 | 4 | 0 | 0 | 4 | 0 | 0 |
| LAB | 4 | 4 | 0 | 0 | 4 | 0 | 0 |
| EDIT | 4 | 3 | 0 | 1 | 1 | 1 | 1 |
| BELL | 4 | 4 | 0 | 0 | 2 | 0 | 2 |
| AIHW | 4 | 4 | 0 | 0 | 2 | 0 | 2 |
| AI | 4 | 3 | 1 | 0 | 2 | 0 | 1 |
| TOK | 3 | 3 | 0 | 0 | 1 | 0 | 2 |
| DATA | 3 | 3 | 0 | 0 | 2 | 0 | 1 |
| CLI | 3 | 2 | 1 | 0 | 2 | 0 | 0 |
| ARCH | 3 | 3 | 0 | 0 | 2 | 0 | 1 |
| ZRAM | 2 | 2 | 0 | 0 | 2 | 0 | 0 |
| WASM | 2 | 0 | 2 | 0 | 0 | 0 | 0 |
| VAULT | 2 | 2 | 0 | 0 | 2 | 0 | 0 |
| STARTUP | 2 | 2 | 0 | 0 | 2 | 0 | 0 |
| SHARE | 2 | 1 | 1 | 0 | 1 | 0 | 0 |
| QA | 2 | 2 | 0 | 0 | 0 | 2 | 0 |
| K3S | 2 | 2 | 0 | 0 | 2 | 0 | 0 |
| HW | 2 | 2 | 0 | 0 | 2 | 0 | 0 |
| FILE | 2 | 0 | 2 | 0 | 0 | 0 | 0 |
| PWR | 1 | 0 | 1 | 0 | 0 | 0 | 0 |
| MEM | 1 | 0 | 1 | 0 | 0 | 0 | 0 |
| JOBS | 1 | 1 | 0 | 0 | 1 | 0 | 0 |
| CORE | 1 | 1 | 0 | 0 | 1 | 0 | 0 |
| CLOCK | 1 | 1 | 0 | 0 | 1 | 0 | 0 |
| CHAT | 1 | 1 | 0 | 0 | 1 | 0 | 0 |
| BUG | 1 | 1 | 0 | 0 | 1 | 0 | 0 |
| APPLE | 1 | 1 | 0 | 0 | 0 | 1 | 0 |

Areas not shown above have 1 row each (APPLE, BUG, CHAT, CLOCK, CORE, JOBS and similar). Totals: UNV 358, MISS 51, OTHER 108, planned 74, deprecated 13 = 604.

### Top blockers per area (the 16 largest areas)

| Area | What really blocks it |
|---|---|
| BROW (55) | 7 planned (F-BROW-28 layout sizes, 32 real-time web/WebRTC, 33 mail and office apps, 35 PDF viewer and printing, 38 accessibility, 40 optional package, 64 H.264/AAC libraries). Most partial rows are fake-helper tests; the real WPE helper, ad-block rules (F-BROW-54), H.264 bar (F-BROW-62) and speed (F-BROW-56, `WEB_IRIS=1`) were never run on a screen. |
| DRIVERS (33) | Mac/T2 rows: Wi-Fi firmware install never seen on the Mac mini (F-DRIVERS-24), FaceTime camera driver 30 percent (F-DRIVERS-28), CS8409 speaker (F-DRIVERS-27); `hw` image never booted; 2 deprecated Apple Silicon rows. |
| FLOW (28) | Thrift node (F-FLOW-14), webhook trigger (F-FLOW-15), remote runner (F-FLOW-16) planned; the rest is "fake runner, no real cluster/browser/camera". |
| SHELL (27) | DRM/KMS renderer (F-SHELL-10), touchpad on bare metal (F-SHELL-12), themes, fonts; panes and screens drawn on made-up screens only. |
| BOOT (25) | Secure Boot routes 2 and 3 (F-BOOT-11, 12), two-stage and critical-path boot (F-BOOT-14, 15), lazy drivers (F-BOOT-04), prefetch (F-BOOT-30); aarch64 never on a Mac (F-BOOT-08). |
| K8S (21) | 17 of 20 partial: tested with a fake `kubectl`; the real k3s on the Mac has run since 2026-10-27 (auto-start verified), so most need a dated sentence plus a diag file. |
| VOICE (18) | F-VOICE-04 and F-VOICE-20 (real microphone and speaker): the USB microphone worked on the T2 MacBook, so they can move once a recording test is run; 6 deprecated. |
| APPS (18) | Real store hosting, real publisher flows (F-APPS-40 yank); mostly loopback. |
| IDE (17), ASSIST (17), GIT (16), EXTA (13) | Scripted models and fake CLIs, no live vendor service or keys (KI-023, KI-024); GIT tile never against a real Git server. |
| SND (14), KBD (14) | Mixer (F-SND-09..11) is in the tree, off by default, never run on a card; F-SND-12 (browser and media through the mixer) and F-SND-13 (alarm) planned. Touch Bar (F-KBD-12..14) fakes only; brightness, DDC, sleep planned. |
| INST (13) | Partition planner and installer follow-ups (F-INST-04, 12, 13): no real disk; Windows/macOS hosts planned. |
| LINAPP (12), SETUP (11), TALES (10), NET (10), MEDIA (9), FW (9) | Real-device runs; Wi-Fi/Bluetooth (F-NET-02, 05, 07, 09, 10); media player on screen (F-MEDIA-08); firewall on a device (F-FW-04, 09). |
| PKG (8) | 5 planned: `hw` image, budgets (decided 2026-10-27 to drop 30 MB/100 ms, still to measure), USB writer for Windows and with a window, image shrink. |
| UPD / UPG | UPD-01..04 all planned (update bundle, boot health check and rollback, overlays, update check): there is **no update path other than `iyakki dev update`**. UPG-01..06 are built; only the QEMU and device upgrade runs are open. |
| REL / QA / STORE | REL-05 never run end to end, REL-09 no release image; QA rows missing a part; STORE-05/06/07 encryption, read-only image, filesystem choice. |

## 3. The plan, in waves

Sizes: S = under half a day of agent work, M = about a day, L = several days. "Agent" names the kind of agent that can do it; the coordinator picks the person or agent.
Every item states the test that must exist before the bullet may be called fixed.

### Wave 1: fixable now in code, with tests (12 items)

| # | KI | Item | Size | Test to add or run | Who | Depends on |
|---|---|---|---|---|---|---|
| 1.1 | 064 | Make the last full run green: the 3 failures of 2026-10-27 (`mixer::...ten_seconds_takes_far_less_than_real_time` load flake; `matrix_is_consistent` because F-REL-09/F-REL-10 name `iyakki-relcheck release-image-check`, which does not exist: add the subcommand or correct the rows; `shell::overlap_tests::single_pane_mode...` order dependency) | M | the three tests themselves, then one full `cargo test --workspace --no-fail-fast` | tests agent + licence agent + shell agent | no builds running |
| 1.2 | 063 | Confirm "0 warnings" with `./run.sh warnings` (RELEASE-READINESS saw 1 in agent, 9 in shell during concurrent edits) | S | `./run.sh warnings` exits 0 | any agent | other agents finished editing |
| 1.3 | 058 | Replace the stale test count (3,845 passed) by the newest full run (6746 passed, 3 failed, 79 ignored on 2026-10-27, `BETA-CHECKLIST.md` section 5) and the tag-time run | S | the run of 1.1 | docs agent | 1.1, owner OK to change text |
| 1.4 | 004, 028 | Registry one-click without a password on NodePort 30500: stop exposing it beyond the node (k3s `kube-proxy-arg nodeport-addresses=127.0.0.0/8`, or a firewall forward rule for 30500, or htpasswd with a stored secret) | M | extend `localreg_tests` (manifest and k3s arguments contain no externally reachable registry; refusal when the firewall is off and the address is not loopback) | K8S agent (built: localreg.rs loopback hostPort, converts old NodePort installs; stays listed until the Mac check) | Wave 2 check on the Mac's k3s that pods still pull `localhost:30500` |
| 1.5 | 036 | Audit log: two processes on one `audit.log` can race (advisory file lock around read-last-hash-and-append; a BROKEN chain gets a documented "start new log" command) | M | a test that spawns two writers (threads over separate handles and a child process) and verifies the chain; a test that a broken chain is detected and a new log starts | shell agent | none |
| 1.6 | 060 | Tests leak folders in `/tmp`: one shared temp-dir guard that removes itself on drop, used by the tests that leak | M | a script step in `./run.sh test` that counts `/tmp/iyakki-*` before and after a crate's run and fails on growth | tests agent | none |
| 1.7 | 034 | Yank does not disable installed copies: show a "withdrawn" mark on installed copies in the STORE tile and in `iyakki pkg list`, offer remove; revocation list stays the hard route | M | store test: yank then list shows installed copy as withdrawn; STORE tile test at four sizes | store agent | none |
| 1.8 | 025 | Claude Code's own foreground Bash: investigate generating its settings (deny rule for the shell tool plus a hook that points to `job_start`) for IYAKKI-managed sessions | M | test that the generated settings file has the deny/hook entries and is written only for IYAKKI-launched sessions | external-agents agent | confirm the vendor setting names on a real Claude Code (Wave 2) |
| 1.9 | 047 | Flows: webhook trigger (authenticated local HTTP endpoint that starts a flow, F-FLOW-15); remote runner (F-FLOW-16) and Thrift stay Wave 4 | L | trigger tests (valid signature starts the flow; bad or replayed token refused; rate limit); `validate.rs` warning removed | flows agent | web service auth (F-WEBSVC-02) |
| 1.10 | 015 | "Very large UI sizes on small screens can overflow some pages": find overflowing pages with the existing size matrix (800x480 to 2560x1600 with the largest text size) and fix them | M | shell layout test at each size with the largest scale for every page | shell agent | none |
| 1.11 | 022 | Rewrite "Radio plays MP3 only" to what is true (MP3 and HLS-MP3 in-house; AAC through the add-on, fake decoder only; Ogg/Opus missing) | S | tests named in section 1 | docs agent | owner OK to change text |
| 1.12 | 013, 026, 041, 044 | Promote stale rows: where the T2 MacBook observation exists, write the dated sentence into `FEATURES.md` and `HARDWARE-MATRIX.md` (F-DEVMODE-01, F-DISP-07, F-CAM-12/13, F-K3S-27/28, F-STARTUP-01/02, F-HW-02) **after** a diag file is saved | S | none new; the diag file is the evidence | coordinator | Wave 2 step 2.1 |

### Wave 2: needs a real machine check (28 items)

Each step is run by the owner (or the coordinator through developer mode) on the machine named in `docs/project/HARDWARE-MATRIX.md` (section 1 ids A to J, matrix in section 2,
proposed waivers in section 5, diag files in section 8). The planned **Check-up tile (BACKLOG P1)** is the way to make one signed-off result file per machine: each step below
becomes a Check-up step, so the result is saved in a file instead of a sentence. Until the tile exists, use `iyakki dev diag` plus `iyakki hwreport` (`docs/BOOT-TEST.md`).

| Step | KI | Machine | Exact owner test | Evidence to record |
|---|---|---|---|---|
| 2.1 | 026, 013, 044, 057 | A (T2 MacBook, image 1027e) | `iyakki dev diag > diag-1027e.txt`; open About (version is not `unknown`); Settings, Agents, GitHub Copilot: sign in and show the vault entry; camera picker with two cameras; touch on the external monitor | the diag file attached to `HARDWARE-MATRIX.md` section 8, date |
| 2.2 | 005, 007 | B (Mac mini 2018) | Boot the newest image; Mac drivers page, Download from Apple, then install; read `iw dev`, `ip addr` for an associated address; confirm no 5.3 s input scan stall (`dmesg`, boot timeline line); play radio and a web video at once | `hwreport`, dmesg excerpt |
| 2.3 | 009, 053, 055 | A | Install to internal disk (done 2026-10-24), then: reboot twice, `iyakki dev update` to a newer image, pull the power during an update, boot the older menu entry; fill memory to watch zram | dated sentence per case; F-UPG-06 and F-INST-03 move |
| 2.4 | 012, 014 | D (AMD + NVIDIA), E (Intel + NVIDIA); the `hw` image on any | `docs/project/PC-TEST-SHEET.md` in full; boot the `hw` image once | filled sheet, `hwreport` per PC |
| 2.5 | 003 | any PC with Secure Boot on, and A | enrol the owner key (MOK), boot signed image; boot unsigned image and see it refused | dated result per machine; needs decision KI-003 in Wave 3 |
| 2.6 | 017, 018, 021 | A or B with the web image | Install the ffmpeg add-on from Store; play an H.264 video (TikTok) and an AAC radio station; play a file in MEDIA; download the filter lists and open an ad-heavy site | a screenshot or a dated sentence each |
| 2.7 | 010, 011 | any Apple Silicon Mac | follow `docs/APPLE-SILICON.md` (VM + k3s + bridge) | dated result; ARM64 hardware stays out of 1.0.0 (Wave 4) |
| 2.8 | 024, 027, 029, 030 | A with k3s | deploy Gitea and the private store to the Mac's k3s; `tile_ship` to it; run the ignored live tests of `dash/live_tests.rs` against real Prometheus/Elasticsearch/ClickHouse containers; run Copilot with a tool call end to end | test names plus dated sentence |
| 2.9 | 041 | A | firewall ON with k3s running and developer mode paired from the LAN (F-DEVMODE-15, F-FW-09) | diag file |
| 2.10 | 049, 050 | A (battery), any laptop | image with `CONFIG_ACPI_BATTERY`: battery indicator, unplug and plug; screen saver and blank on the real panel | dated sentence |
| 2.11 | 054 | spare USB stick | DISKS tile: mount, eject, partition manager on a spare stick only | dated sentence |
| 2.12 | 033 | A | read three TALES stories made by a real model; owner judges quality (not a test, a judgement); the timing fix for highlight drift | owner note |
| 2.13 | 061, 065, 066 | A | look at every screen on the real displays; repack the web/macweb images with the prune step and run `legal-collect.sh`; install restricted firmware at a click | screenshots, legal-info listing, `fwextra` result |

### Wave 3: needs an owner decision (14 items)

| KI | Decision | Recommendation |
|---|---|---|
| 001, 043 | Encrypt the data volume before 1.0.0, or ship unencrypted with vaults only | Ship unencrypted for 1.0.0 (vaults are encrypted), decide the crypto stack in BACKLOG decision 9 first, make the warning part of the installer confirmation |
| 002 | Managed cluster support in 1.0.0 | Waive: document "single-node k3s only"; test later with an EKS/GKE account |
| 006 | Bluetooth on the Mac mini 2018 (no legal firmware) | Waive for 1.0.0 in writing (BACKLOG decision 13); USB adapter is the route; stack stays Wave 4 |
| 020 | Build the Intel GPU driver into the web images (`WEB_IRIS=1`, brotli, WOFF2) | Yes, once; it grows the web images by a few MB (allowed by the image-size rule); verify with the frame-drop clip |
| 023 | Who supplies keys for live checks of OpenAI, Anthropic, Gemini, OpenRouter | One short owner session with throwaway keys; otherwise waive and say "scripted only" |
| 031 | Picture generation: Ollama image models only on macOS | Waive, keep a one-line note (external dependency) |
| 035 | Should INBOX approvals and Parallel-tasks workers ever take session grants | No: keep "ask every time"; move the text to the user guide as a feature |
| 037, 046 | Generate the official root key and host the catalog | Generate it offline now (`iyakki pkg keygen iyakki-root`), pin before the beta tag, host `catalog.json` on the portal; without this the official store is empty |
| 040 | Linux users for IYAKKI accounts | Keep root-owned files for 1.0.0; document it as design |
| 045 | Payments | Defer past 1.0.0; keep the button disabled |
| 056 | Release key comparison | Owner prints and pins the public key in the README at tag time (a step, not a decision) |
| 062 | CI | Register one runner for the self-hosted Gitea and run `.gitea/workflows/ci.yml`; until then `./run.sh alpha-check` is the substitute and the bullet stays |

### Wave 4: waive or defer past 1.0.0 (12 items, only with the owner's written decision)

For each, the owner writes one sentence in `docs/project/BACKLOG.md` (owner decisions table) with date and name, and the same sentence is added to the bullet; then it may leave `KNOWN-ISSUES.md` for a "Not promised" list in the user guide.

| KI | What the sentence must say |
|---|---|
| 008 | "Boot speed accepted for 1.0.0 (F-PKG-05, 2026-10-27); side image on the boot partition deferred" |
| 011 | "ARM64 hardware, Raspberry Pi 3/5 and Jetson are not promised for 1.0.0" |
| 016 | "The 30 MB and 100 ms targets are dropped; budgets will be measured (F-PKG-05)" |
| 019 | "The small built-in browser is for local pages; CJK and emoji fonts are not shipped (English-first fonts)" |
| 032 | "Voice engines are not shipped by licence; the browser voice or a found engine is used" |
| 038 | "Message box limits are a feature" (then it is not an issue) |
| 039 | "English-only phrase tables are accepted for 1.0.0; the board-file bypass is documented" |
| 042 | "The root filesystem is in RAM by design until F-STORE-06 exists" |
| 048 | "Encryption (see Wave 3), read-only system image, copy-on-write snapshots, external vector store, Bluetooth userland, device broker, DRM/KMS are after 1.0.0" |
| 051 | "Native web views and the WASM editor are after 1.0.0" |
| 052 | "Windows and macOS window builds are after 1.0.0" |
| 059 | "QEMU scenarios list is information, not an issue" (remove, or move to the test guide) |

## 4. Tracking scheme

* Ids **KI-001 to KI-066** are stable and never reused. New handwritten bullets take the next number. The generated appendix keeps its `F-` ids.
* A bullet leaves `KNOWN-ISSUES.md` only when its row below says `FIXED`, with evidence. Evidence is one of: a passing test (full path as in `FEATURES.md`), a dated device check with a saved diag file, or (for text-only fixes) the commit.
* Fixed in the tree but not yet seen on the image: append `Fixed in the tree <date>: <evidence>; leaves this file when verified on the image` to the bullet (no bullet qualifies today).
* Whoever fixes an item updates, in the same change: the bullet's evidence line, the row below, the `FEATURES.md` row (status, progress, Notes) and `HARDWARE-MATRIX.md` if a cell moves.
* Review: the coordinator re-counts this table on every image check and in the `RELEASE-READINESS.md` gap table.

## 5. Table of the 66 issues

Status: T = still true, P = partly fixed, F = fixed, U = unclear. Class: HW, MF, OD, W, EXT, CB, INFO. Owner: A = agent, O = owner, C = coordinator.

| ID | Section | Short title | Status | Class | Wave | Evidence / test / next step | Owner |
|---|---|---|---|---|---|---|---|
| KI-001 | Do not use | Installer erases the drive; data volume not encrypted | T | OD | 3 | F-STORE-05 planned | O |
| KI-002 | Do not use | No managed cluster | T | EXT | 3 | fake kubectl and local k3s only | O |
| KI-003 | Do not use | Secure Boot on real hardware | T | HW | 2 | QEMU 17 checks; step 2.5; initrd unsigned, GRUB 2.12 | O |
| KI-004 | Do not use | Internet exposure, registry without password | T | CB | 1 | item 1.4; `localreg_tests.rs` NodePort 30500 | A |
| KI-005 | Hardware | Mac mini 2018 Wi-Fi | P | HW | 2 | worked on T2 MacBook 2026-10-26/27 (observed); step 2.2 | O |
| KI-006 | Hardware | Bluetooth BCM4364 | T | EXT | 3 | no legal firmware; F-NET-05 | O |
| KI-007 | Hardware | Newest image not confirmed on the Mac mini | U | HW | 2 | F-BOOT-26 QEMU only; step 2.2 | O |
| KI-008 | Hardware | Slow start at GRUB | P | W | 4 | boot speed accepted 2026-10-27 (F-PKG-05); side image planned | O |
| KI-009 | Hardware | T2 MacBook Pro open items | P | HW | 2 | internal disk booted 2026-10-24, 1027e 2026-10-27/28; Touch Bar, BT, upgrade, battery, sleep open | O |
| KI-010 | Hardware | Apple Silicon VM recipe never run | T | HW | 2 | decision 2026-10-27; F-BOOT-08; step 2.7 | O |
| KI-011 | Hardware | ARM64 in QEMU only | T | W | 4 | HARDWARE-MATRIX K, L out of 1.0.0 | O |
| KI-012 | Hardware | Bare-metal PCs untested | T | HW | 2 | PC-TEST-SHEET; step 2.4 | O |
| KI-013 | Hardware | Microphone and FaceTime camera | P | HW | 2 | USB camera, USB mic, picker, touch map observed 2026-10-27/28; built-in mic off by design | O |
| KI-014 | Hardware | `hw` image never booted | T | HW | 2 | F-PKG-04 planned; F-REL-09; step 2.4 | O |
| KI-015 | Hardware | fbdev, software rendering, big UI sizes overflow | T | MF | 1 | item 1.10; DRM/KMS F-SHELL-10 is Wave 4 | A |
| KI-016 | Hardware | 30 MB / 100 ms targets not met | P | W | 4 | targets dropped (F-PKG-05) | O |
| KI-017 | Browser | H.264/AAC does not play | P | HW | 2 | add-on route in tree (F-BROW-62, F-BROW-64 planned, commit 5258cdc); fake helper only | O |
| KI-018 | Browser | ffmpeg media player not confirmed on screen | T | HW | 2 | F-MEDIA-08, F-MEDIA-09; step 2.6 | O |
| KI-019 | Browser | Small browser limits, CJK boxes | T | W | 4 | F-BROW-35 planned; English-first fonts | O |
| KI-020 | Browser | Slow video/pages on Macs | P | MF | 3 | Smooth pages switch and mixer in tree (F-SND-09/10, F-BROW-56); `WEB_IRIS=1` not in any image | O |
| KI-021 | Browser | Ad and tracker blocking never on real WPE | T | HW | 2 | F-BROW-54; step 2.6 | O |
| KI-022 | Browser | Radio plays MP3 only | P | MF | 1 | HLS-MP3 and AAC-via-add-on tests (section 1); Ogg/Opus missing; F-RADIO-03; 2026-10-28: text rewritten in KNOWN-ISSUES.md to what is true (AAC via add-on with a fake decoder; Ogg/Opus and directory search missing); stays listed (P) | A |
| KI-023 | Assistant | Live vendor checks need keys | T | EXT | 3 | F-AI-02 | O |
| KI-024 | Assistant | Copilot tool use, Autopilot external agents | T | HW | 2 | F-JOBS-04; step 2.8 | O |
| KI-025 | Assistant | Claude Code foreground Bash not switchable | T | MF | 1 | item 1.8; F-JOBS-04 | A |
| KI-026 | Assistant | Developer mode tested on loopback only | P | HW | 2 | T2 MacBook over Wi-Fi 2026-10-26/27; `devmode/tests_fw.rs`, `tests_update_safety.rs` in tree; no saved diag | C |
| KI-027 | Assistant | `tile_ship` loopback only | T | HW | 2 | step 2.8 | O |
| KI-028 | Assistant | In-cluster registry fakes only, open port | T | CB | 1 | item 1.4; `localreg_tests.rs` | A |
| KI-029 | Assistant | Platform services never on real k3s | U | HW | 2 | k3s auto-start verified 2026-10-27/28; services deployed? unknown; step 2.8 | O |
| KI-030 | Assistant | Dashboards never against real servers | T | HW | 2 | `dash/live_tests.rs` ignored tests; step 2.8 | O |
| KI-031 | Assistant | Picture generation (Ollama images only macOS); text has a broken line ("none is shipped.") | T | EXT | 3 | fix the stray line with the owner's OK | O |
| KI-032 | Assistant | Voice engines not shipped | T | W | 4 | F-VOICE-05..07 | O |
| KI-033 | Assistant | TALES picture quality unjudged | T | HW | 2 | F-TALES-06,13,15..18; step 2.12 | O |
| KI-034 | Assistant | Yank does not disable installed copies | T | MF | 1 | item 1.7; F-APPS-40 | A |
| KI-035 | Assistant | Approvals: session grants only for chat | T | OD | 3 | F-ASSIST-40 | O |
| KI-036 | Assistant | Audit-log chain race, BROKEN stays BROKEN | T | CB | 1 | item 1.5; F-SHELL-01; `upgrade_tests.rs` covers only version compatibility; 2026-10-28: race fixed in tree (flock) with `accounts::tests::two_processes_and_two_threads_appending_at_once_keep_one_unbroken_chain` and `a_line_cut_short_by_a_power_loss_...` passing; BROKEN-stays-BROKEN and the 'start new log' command still open | A |
| KI-037 | Assistant | Official store address empty | T | OD | 3 | `OFFICIAL_STORE_URL = ""` in `store/catalog.rs` | O |
| KI-038 | Assistant | Message box limits | T | W | 4 | `chat_box_tests.rs` (20,000 characters) | O |
| KI-039 | Assistant | English-only phrase tables; board file bypass | T | MF | 4 | board-file hash would be a small Wave 1 item if the owner wants it | O |
| KI-040 | Accounts | Own accounts only, root-owned files | T | OD | 3 | design | O |
| KI-041 | Accounts | Firewall not on a device with k3s | P | HW | 2 | dev mode through firewall built (`tests_fw.rs`); F-FW-04, F-FW-09; step 2.9 | O |
| KI-042 | Accounts | Root filesystem in RAM | T | W | 4 | F-STORE-06 planned | O |
| KI-043 | Accounts | Data volume not encrypted, self-signed cert | T | OD | 3 | same as KI-001 | O |
| KI-044 | Accounts | Copilot token moved into the vault | P | HW | 2 | `COPILOT_VAULT` in `extagent/login.rs`; re-test step 2.1 | O |
| KI-045 | Not built | Payments | T | OD | 3 | F-APPSTORE-13 | O |
| KI-046 | Not built | Official root key and hosted catalog | T | OD | 3 | `official-roots.txt` has no key; F-APPSTORE-14 | O |
| KI-047 | Not built | Flows: Thrift, webhook, remote runner | T | MF | 1 | item 1.9 (webhook); F-FLOW-14, 15, 16; `iyakki-flow/src/kinds/cli.rs` | A |
| KI-048 | Not built | Encryption, RO image, CoW, vector store, BT userland, broker, DRM/KMS | T | MF | 4 | F-STORE-05/06, F-WASM-06, F-MEM-02, F-NET-05, F-BOOT-04, F-SHELL-10 | O |
| KI-049 | Not built | Laptop battery | P | HW | 2 | `CONFIG_ACPI_BATTERY=y` in `linux.fragment`; no image carries it yet; step 2.10 | O |
| KI-050 | Not built | Screen saver and screen off | T | HW | 2 | F-SCRN-01; step 2.10 | O |
| KI-051 | Not built | Native web views, WASM editor | T | MF | 4 | after 1.0.0 | O |
| KI-052 | Not built | Windows and macOS window builds | T | W | 4 | `docs/MACOS.md` | O |
| KI-053 | Install | Installers: real disks, upgrade, power cut | T | HW | 2 | F-INST-03; F-UPG-06; step 2.3 | O |
| KI-054 | Install | DISKS tile fakes only; kernel lacks NTFS/exFAT/Btrfs/XFS/HFS+ | T | HW | 2 | F-DISKS-01..08; step 2.11 | O |
| KI-055 | Install | zram under pressure, no hibernation | T | HW | 2 | step 2.3 | O |
| KI-056 | Install | Release key and download page are new | T | OD | 3 | compare printed key with README at tag time | O |
| KI-057 | Install | Image before version bump says `unknown` | T | HW | 2 | F-REL-01: `/etc/iyakki-release` not seen on a built image; step 2.1 | C |
| KI-058 | Tests | Stale test count | P | INFO | 1 | newest: 6746 passed, 3 failed, 79 ignored (BETA-CHECKLIST section 5); 2026-10-28 run per crate, see section 9; text in KNOWN-ISSUES.md updated; final number only after one green full run | A |
| KI-059 | Tests | QEMU scenarios list | T | INFO | 4 | information | O |
| KI-060 | Tests | Tests leak `/tmp` folders | T | CB | 1 | item 1.6; 2026-10-28: run folder + growth check in `./run.sh test`, self-test `dist/release/test-tmp-guard.sh` passes; fixed in tree, leaves the file after one seen green `./run.sh test` | A |
| KI-061 | Tests | Screens checked by eye, not on real displays | T | HW | 2 | Check-up tile P1; step 2.13 | O |
| KI-062 | Tests | No CI running | T | EXT | 3 | `.gitea/workflows/ci.yml` (untracked) and `.github/workflows/ci.yml`; no runner | O |
| KI-063 | Tests | Warnings 0 | U | INFO | 1 | item 1.2; 2026-10-28: `./run.sh warnings` = FAIL, 4 distinct warnings in files of work in progress (section 9); not confirmed | A |
| KI-064 | Tests | Test triage | P | INFO | 1 | 38 failures to 3 (2026-10-27); item 1.1; 2026-10-28: the 3 old failures are fixed (mixer, matrix, overlap) plus 11 stale or host-dependent tests; open ones in section 9 | A |
| KI-065 | Licence | GPL source offer, libre image | P | HW | 2 | `legal-collect.sh` run for real 2026-10-27 (F-REL-10); images need repack; step 2.13 | O |
| KI-066 | Licence | Restricted firmware by click | P | HW | 2 | `fwextra*.rs` and tests in tree, fakes only; F-REL-08; step 2.13 | O |

## 6. The top 10 to fix first

1. **KI-064 / KI-063 / KI-058, the green gate (M).** BACKLOG and RELEASE-READINESS require one full run with zero failures and zero warnings; there are 3 failures left and none of them need hardware.
2. **KI-004 / KI-028, registry open on the network (M).** The only security hole in the list that is a code fix: a service on all interfaces without a password. The firewall does not cover forwarded NodePort traffic.
3. **KI-036, audit-log race (M).** An audit chain that can break is an integrity failure that is silent until it reads BROKEN.
4. **KI-062, CI (decision plus one runner).** Every other fix is only as good as a gate that runs; today nothing runs it.
5. **KI-053 and the missing update path (F-UPD-01/02).** Install, second boot, upgrade and power cut on a real internal disk, plus update and rollback, are what makes a daily-use install safe. No update bundle or rollback exists yet.
6. **KI-026, save one `iyakki dev diag` file from the T2 MacBook (S, owner).** It turns eight or ten stale "fakes only" rows into verified ones and starts the evidence trail.
7. **KI-012 / KI-014, the two PCs and the `hw` image.** Gate item 4 of the hardware matrix; the only machines that test NVIDIA and generic Intel.
8. **KI-046 / KI-037, official root key.** Without it nothing from the network is Official and the portal has nothing to list; generating it offline is cheap and blocks the store story.
9. **KI-005 / KI-007, Mac mini 2018 repeat.** The T2 MacBook route worked, so a repeat on the mini probably closes Wi-Fi and the "newest image" doubt in one session.
10. **KI-017 / KI-018, H.264/AAC and the media player on a screen.** The code path was written this week; one session with the real ffmpeg add-on proves or refutes it and settles TikTok-class sites.

## 7. Keeping `KNOWN-ISSUES.md` readable (proposal only; nothing was changed)

Today lines 1 to 206 are handwritten and lines 207 to 791 are a generated table that copies `FEATURES.md` (and is already stale: 575 rows against 604).

**Option A: move the appendix into its own generated file** (for example `docs/KNOWN-ISSUES-APPENDIX.md`) and link it from the end of `KNOWN-ISSUES.md`.

| Pros | Cons |
|---|---|
| The page people read is about 200 lines, with ids | One more file to keep and to ship in the release set |
| The generated table cannot go stale inside a hand-edited file; the script can run in `./run.sh alpha-check` | The release copy (`dist/release/release.sh` line 97 copies `KNOWN-ISSUES.md` only) would lose the appendix unless the file is added |
| No more merge fights between agents editing the handwritten text and the script | A reader of the release sees a link instead of the full list |

**Option B: drop the appendix** and link `FEATURES.md` filtered by status. Smallest file; but the "every non-done row is listed" promise (`README`, `SUPPORT.md`) would need rewording.

**Option C: keep as is.** No work; the file stays at 791 lines.

What references the appendix or the file, and what would have to change for A:

* `dist/release/known-issues-appendix.sh`: takes the file path as argument and rewrites between `<!-- appendix:start -->` and `<!-- appendix:end -->`. Change: default target to the new file, write the markers there, and keep one marker pair in `KNOWN-ISSUES.md` with the link only (or remove the markers there).
* `crates/iyakki-core/tests/release_version.rs` line 26: requires `KNOWN-ISSUES.md` to exist in the release file list. If the new file ships, add it to that list (and to `alpha-check.sh` line 41, `release.sh` line 97, `commit-alpha.sh` line 40, which are already lists of file names).
* `crates/iyakki-core/tests/secure_boot.rs` line 117: asserts `KNOWN-ISSUES.md` contains `SECURE-BOOT.md`. That text is in the handwritten part, so no change.
* Docs that name `KNOWN-ISSUES.md` (`README.md`, `SUPPORT.md`, `SECURITY.md`, `docs/INDEX.md`, `docs/HARDWARE.md`, `docs/project/HARDWARE-MATRIX.md`, `RELEASE-READINESS.md`, `BACKLOG.md`): only need the new link sentence.
* Not referenced by any Rust test: the appendix text itself and the "N rows" count.

Recommendation (owner decides): Option A, plus run the generator at the start of each image check so the count is never stale. In either case regenerate it once now; the current appendix misses the UPG, WEBSVC and NAME areas.

## 8. What could not be verified

* Whether any listed test passes today: no cargo was run (builds were in progress); test names come from `FEATURES.md` and the test files.
* All observations on the T2 MacBook (2026-10-26/28) are the coordinator's and owner's words; no diag file exists. No machine was reached.
* Whether images were rebuilt with `CONFIG_ACPI_BATTERY` or the new sound and radio code; the Buildroot and image builds were still running.
* KI-029 (Gitea or the store deployed to the Mac's k3s), KI-007 (which image the Mac mini last ran), KI-063 (current warning count).
* Whether the vendor's Claude Code accepts the deny and hook settings proposed in item 1.8.
* The UNV, MISS and OTHER counts in section 2 are keyword-based on Notes; they show proportions, not an exact audit.
* Many files are uncommitted on the working tree (560 changed paths), so "in the tree" does not mean "in a commit".

## 9. Wave 1 progress, 2026-10-28 (tests agent)

Machine rule: 8 GB limit, 4 build jobs. One `cargo test --workspace` could not run under it: the shell crate's test process needed more than 8 GB (KI-067). Fixed later the same day: the cause was freed memory kept by glibc's per-thread arenas, not live shells; `testmem.rs` trims it, and `cargo test -p iyakki-shell --lib` now peaks at 2.2 GB (2,503 passed, 0 failed, 33 ignored). The per-crate batches below were run before this fix.
So every crate ran on its own, the shell crate in 26 batches of 100 tests. Results (passed / failed): agent 2,978 / 0 after the fix below (+ licence_gate 6), flow 187 / 0,
shell 2,486 / 17 before fixes (8 fixed and re-run green, 9 left, see below), core 210 / 3 (2 open, 1 fixed), remote 76 / 0, relcheck 50 / 0, web 226 / 0, js 96 / 0, drivers 132 / 0,
storage 68 / 0, fw 55 / 0, devctl 57 / 0, edit 50 / 0, netscan 37 / 0, usb all green, every other crate green.

Fixed in the tree (test and cause):

| Test | Cause | Fix |
|---|---|---|
| `radio::tests::an_hls_station_with_aac_segments_...` | 5 s poll limit under a loaded machine | `wait_for` limit 30 s |
| `mixer::...ten_seconds_takes_far_less_than_real_time` | wall time of 500 periods, one preemption fails it | the slowest 10 percent of periods are left out |
| `engine_tests::a_failed_node_follows_its_error_port...` | real race: the run read as finished before the failure notice was posted | `iyakki-flow/src/driver.rs`: notice first, then the final status |
| `settings::shares_tests::start_stop_and_stop_all_with_a_real_server` | fixed port in use | uses an ephemeral port; green in the batch run |
| `shell_overlap_tests::single_pane_mode_...` | fixed 30 beats for a background look | polls until the condition (limit 10 s) |
| `settings::install_tests::installing_needs_the_typed_name_...` | read an 8.2 GB sparse file whole (killed the 8 GB limit) | reads only the first part |
| `browser_full::tests::addresses_without_a_host_...` | `url_allowed` let `javascript:` through (real bug) | only `Pass` is allowed |
| `cli::tests::the_cli_tile_has_a_terminal_tab...` | 10 key presses no longer reach the end of a longer catalogue | presses until the entry shows |
| `store::setup::tests::{install_all_in_this_category..., the_category_bar...}`, `voice_tests::pressing_install...` | OpenSearch added, the Voices category now holds two items, voice review now says "integration code only" (owner decision 2026-10-27) | assertions updated |
| `monkey_tests::monkey_the_rest` | real bug: a demo screen (since removed) multiplied a negative width cast to usize in a 1x1 window | `.max(0)` |
| `upgrade_tests::what_the_stores_say_while_loading...` | the sentence queue is shared by all tests; an audit line is cut at 300 chars | retries with a fresh sentence; checks file name and "damaged" |
| `ide_notes_tests::a_read_only_workspace_...` | fixed 2 ticks | waits for the tick |
| `release_version::the_feature_matrix_totals_are_consistent` vs `feature_matrix::matrix_is_consistent` | two checkers printed a zero count differently (real bug of the checkers) | `relcheck features::summary` lists every status, as the core test does |
| licence gate: `legal-info buildroot-web` | stale legal-info lacked the licence text of `iyakki-webview-helper-1` | `legal-collect.sh web` (copy only) |

Left open, and why (all caused by the other two agents' work in progress, files not touched):

| Test | Cause | Owner |
|---|---|---|
| `ide_deploy::tests::{approve_all_runs_every_step..., each_step_waits_for_approval...}`, `ide_tests::{ide_k8s_registry_strip_dialog..., ide_the_deploy_form_runs_build...}`, `platform_services::tests::{build_now..., choosing_a_file..., the_wizard_pages_fit..., without_a_registry...}` | `k8stools::registry_up` now probes the real `127.0.0.1:5000` (netprobe, curl removal); this machine has a registry there, so the tests take the registry path. Needs an injectable probe in `K8sTools` | curl-removal agent |
| `shell::tests::tile_names_and_captions_fit_inside_their_tile` (DOCTOR caption at 1024x768) | Doctor tile | Doctor agent |
| `boundary::only_the_listed_files_touch_the_base_os` | `childgone_tests.rs`, `doctor.rs`, `core-api/childstate.rs` not listed in BOUNDARY.md | curl-removal and Doctor agents |
| `dependency_ratchet::tiles_do_not_gain_kernel_or_hardware_access...` | `doctor_facts.rs` reads the kernel directly (5 accesses) | Doctor agent |
| `./run.sh warnings` | `flows/camera.rs` `model_place` unused, `flows_tip.rs`/`flows_paint.rs` unused `s`, `k3sdev_tests.rs` needless `mut` | owners of those files |

Wave 1 follow-up 2026-10-28 (items 1.7 to 1.10; 1.4 turned out to be already built: localreg.rs makes the registry ClusterIP with a loopback hostPort and converts an older NodePort install, 18 tests; 1.12 is still not started and stays listed):

* **1.7 (KI-034): done in code, KI kept and reworded.** `Check::Withdrawn` in `store/lifecycle.rs` marks an installed copy whose exact name, version and hash is in the pinned store's cached signed `yanked.json`; shown as "Withdrawn" in the STORE tile and `store list`, removal offered. Test: `lifecycle_tests::a_copy_the_private_store_yanked_is_marked_withdrawn_and_listed_so_and_can_be_removed`. Still open: the copy is not switched off (that stays the root-signed revocation's job), and there is no four-size tile test for the new chip.
* **1.8 (KI-025): partly done.** With the MCP bridge on, `--disallowedTools` now also refuses common long foreground commands (`LONG_BASH`), so the agent is pushed to `job_start`. Test: `extagent::tests::long_foreground_commands_are_refused_only_when_the_job_tools_are_offered`. Not done: the settings-file hook (vendor setting names unconfirmed, Wave 2 on a real Claude Code); the shell tool still cannot be switched off.
* **1.9 (KI-047): partly done.** The webhook request gate (`flows/webhook.rs`: HMAC-SHA256 signature, 5-minute window, replay refusal, rate limit, idempotency key) has five unit tests. Not done: the HTTP route. The web service allows only four public routes by invariant (`webservice` tests), so a signed-request route class needs its own design and review; until then the `validate.rs` "planned" warning stays, because no webhook fires. Remote runner and Thrift stay Wave 4.
* **1.10 (KI-015): not done.** The monkey matrix checks for panics at every size but nothing measures overflow of the page content at the largest text size; that needs a generic overflow check over every page first.

Wave 1 pass of 2026-10-28 (evening; items 1.1 to 1.12 re-checked in order; development computer only):

* **1.1 (KI-064): the three named failures pass.** `feature_matrix` (`matrix_is_consistent`, `the_checker_catches_the_mistakes_it_exists_for`: `release-image-check` exists in `iyakki-relcheck`), `mixer::tests::mixing_four_sources_for_ten_seconds_takes_far_less_than_real_time` and `shell::overlap_tests::single_pane_mode_gives_the_app_the_whole_area_below_the_header` were each run alone. The one green `cargo test --workspace --no-fail-fast` is the owner's separate run (80 minutes) and was NOT run; KI-064/KI-058 stay as written.
* **1.2 (KI-063): `cargo check --workspace --all-targets` printed no warning and no error** at the end of this pass (other agents' edits included, the tree as it was then); the gate script `./run.sh warnings` itself was not run, and the claim needs a re-check after the other agents finish.
* **1.3 (KI-058): already reworded** by the 2026-10-28 per-crate run; the final number waits for the owner's full run.
* **1.4 (KI-004, KI-028): built earlier (localreg.rs), unchanged; the Mac check stays.**
* **1.5 (KI-036): done in code.** The flock race fix was already in the tree; this pass adds the "start new log" command (USERS, audit log, capital `N`; `AuditLog::start_new_log` keeps the broken file as `audit.log.broken-<time>`, refuses an intact chain). Tests: `accounts::tests::a_broken_chain_is_kept_aside_and_a_new_log_starts_that_names_it`, `users::tests::n_in_the_audit_view_starts_a_new_log_only_when_the_chain_is_broken_and_keeps_the_old_file`, plus the two flock tests. Not tried on a device. KI-036 text updated, not removed.
* **1.6 (KI-060): already in the tree** (`./run.sh test` temp-folder guard, `dist/release/test-tmp-guard.sh`); not run here.
* **1.7, 1.8, 1.9: unchanged from the follow-up above.** 1.7 lacks the four-size STORE tile test of the Withdrawn chip (needs a tile fixture with an installed, yanked copy); 1.8 waits for the vendor setting names on a real Claude Code; 1.9 needs the web service's route design and review (owner/architecture decision, the four-public-routes invariant).
* **1.10 (KI-015): not done**, as above: it needs a generic overflow measurement over every page first.
* **1.11 (KI-022): the text is already what is true** (MP3 and HLS-MP3 in-house; AAC through the add-on with a fake decoder only; Ogg/Opus not built); no change needed.
* **1.12: not done on purpose**: it needs a saved `iyakki dev diag` file from the T2 MacBook, which needs the real machine (not touched).
* **2026-10-29 re-check of 1.7, 1.8, 1.12: nothing more is code-fixable now.** 1.7 still lacks the four-size STORE tile test of the Withdrawn chip (needs a tile fixture with an installed, yanked copy; the chip is only drawn when the row has room, so it cannot overflow). 1.8 waits for the vendor setting names on a real Claude Code. 1.12 needs a saved diag file from the T2 MacBook. 1.10 (KI-015): the overflow lists are now keyed by page name; see KNOWN-ISSUES.md.
* **N3 (NEXT-PLAN item 3, not a wave-1 item): done in code**, see KNOWN-ISSUES.md KI-072.

### Owner decisions on wave 1 (2026-10-28)

* **1.9 (KI-047, webhook route): stays open.** The web service keeps its four public routes. A webhook is designed together with the future option to open a local service to other computers (mesh or incoming port mapping): one secret per webhook, size and rate limits, an allow-list of senders. Nothing to build until then.
* **1.10 (KI-015, overflow check): scheduled after Tier A, in one task with the universal list-window fix.** The harness opens every page at several window and text sizes and reports text that is cut off, overlaps or leaves its box; the list-window fix makes a click never move the rows under the pointer (Settings -> Sound, System sounds, Screen saver; the radio list already does).
* **1.10 (KI-015) and the list-window fix: done in code (2026-10-28).** `listwindow.rs` is the one rule for the first visible row (a click never moves the rows; Settings hardware pages, System sounds and Screen saver use it). The overflow harness runs every tile and Settings page at 1280x720 and 2560x1600 (Normal and Larger text) besides the three small sizes at Huge, fixed the simple findings and lists the rest as a table in KNOWN-ISSUES.md KI-015. KI-015 stays open: the table is the work list, and nothing was compared with a real screen.

