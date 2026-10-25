# Backlog (working list, owner and agents)

Kept by the owner and the agents; newest first inside each section. Status words: **in image** (built into the last image), **in tree** (code written and tested, not in an image yet), **design** (a document exists, nobody built it), **idea** (listed only), **hardware** (needs a look on the real machine). Dates are 2026-10-24 unless stated.

## Scope of v1.0.0 (owner decision, 2026-10-24)

**Every item in this file is part of v1.0.0 unless the owner moves it out.** Pre-releases (alpha.2, beta, release candidates, see `RELEASE-READINESS.md`) still happen on the way; they are checkpoints, not a smaller scope.

### The release gate: v1.0.0 does not ship until all of this is true

1. **No known security issue.** Every item of the security review is closed or accepted in writing by the owner; no open high or medium finding; an outside review is done before the release candidate; dependencies, images and packages are signed and listed in the SBOM; secrets are never in logs, images or exports; the default settings are the safe ones.
2. **No strange or untrustworthy warnings in normal use.** On every supported machine a normal session shows no false alarms (for example the 'drive was disconnected' banner on a healthy NVMe), no scary banners for things that are fine, no certificate or signature warnings for IYAKKI's own services, packages or images, no kernel `WARN`/`ERROR` lines in the boot and session logs that we have not explained and either fixed or filtered with a stated reason, no compiler warnings (10 today), and no 'unsigned', 'dev' or 'experimental' marks left on shipped parts. A warning that stays must be true, useful and actionable.
3. **Everything fixed with testing.** Each item's definition of done (below) is met. The whole workspace test run is green on a clean checkout with no environment excuses (the /tmp quota, flaky tests triaged and fixed or removed); there is a working CI that runs it.
4. **Hardware matrix passed.** The owner's checklist is run on every machine on the supported list (at least the 2020 MacBook Pro T2, an AMD PC, one Intel PC, the older Mac mini): install from the image to internal and USB disks, boot, sound, Wi-Fi, camera, Touch Bar keys, browser video, shutdown, upgrade from the previous release, recovery.
5. **Upgrade and data safety proven.** An install made by the previous release upgrades without losing data (settings are versioned, fixtures from alpha.1 and beta are kept and tested); install and update tools never touch what they were not asked to touch.
6. **Docs true.** README, guides, KNOWN-ISSUES, HARDWARE and FEATURES say what is real.
7. **No licence issue and no proprietary-software issue (owner decision, 2026-10-24).** Every part that ships in an image, package or download has a licence that allows what we do with it, and we meet its conditions (notices reproduced, corresponding source offered for GPL parts, `legal-info/` complete). **Nothing proprietary or of doubtful redistribution is shipped**: the restricted firmware packs, Apple- or vendor-extracted files (Mac Wi-Fi/camera firmware), vendor CLIs (Claude Code, Copilot CLI, ffmpeg builds) and anything else we are not sure of are downloaded or extracted by the person at their click and never placed in an image or an export for others (`docs/RESTRICTED-FIRMWARE.md`). Model weights (LLM, TTS, speech) and fonts have their licences checked and shown. Codecs: no patent-encumbered codec ships unless the owner has decided it may (today H.264/AAC are removed from the image). An automated licence gate (deny list, allow list, SBOM, notice check; `iyakki-relcheck`) fails the build on a new unknown licence, and the owner's legal-opinion decisions (firmware, codecs, TTS weights) are made and recorded. **Open for the owner:** redistributable firmware blobs that the hardware needs (Intel, AMD, Mac Broadcom Wi-Fi in `linux-firmware`) are proprietary binaries with clear redistribution licences; they stay, listed with their licence texts, unless the owner wants a stricter 'no proprietary binary at all' line (which would remove Wi-Fi and graphics firmware from the default images and move them to the install-it-yourself route).

### Definition of done for every item

* A written requirement and, for anything with a design document, the owner's approval of the open decisions.
* Automated tests that fail without the change (unit, plus a headless/QEMU test where it touches boot, install or the screen); the narrow tests and the crate's whole lib tests green.
* A check on the real machine by the owner (or by an agent over SSH with the owner present) for anything hardware-facing, recorded with date and machine.
* A FEATURES.md row with real test names, docs updated, an audit/log line where it changes something, and no new compiler warning.
* Security review of the change when it touches secrets, boot, install, network, packages or approvals.

### Pending items added to v1.0.0 from the readiness audit

| Item | Notes |
|---|---|
| Working CI for the Gitea remote (the tracked workflow targets GitHub) | Runs the whole workspace tests, the feature matrix, the boundary check, the licence/SBOM check and the image smoke tests (QEMU). |
| Triage of the ~31 failing tests from the last full run | Mostly /tmp quota and ordering flakes; fix the tests to use per-test temp dirs under the build tree, remove real flakes. |
| Compiler warnings to zero (10 today) and a warnings gate | |
| Upgrade path: versioned settings, alpha.1 and beta fixtures, upgrade tests | |
| Wi-Fi and Bluetooth on the Macs | Wi-Fi works on the T2 MacBook once the firmware is installed; Bluetooth does not; Mac mini 2018 Bluetooth needs a decision (waive or fix). |
| Two-stage (fast) boot as the normal boot | The 'One IYAKKI entry' design; F-BOOT-14. |
| Install and update proven on real hardware (internal disk, USB, `iyakki-usb --update`) | Internal-disk install of the T2 MacBook: first boot worked 2026-10-24; remaining: the drive-lost fix, the menu, Wi-Fi, a second boot, upgrade. |
| Outside security review and a written threat model | |
| Secure Boot: in or out of the promise; signed images if in | |
| Official store key and URL; package signing end to end | |
| Legal opinion on firmware, codecs and TTS weights | The firmware rule today is the owner's working rule, not legal advice. |
| Automated licence and proprietary-software gate in CI (deny/allow lists, SBOM, NOTICE and `legal-info/` completeness, a scan of every image for files not in the allow list) | Extends `iyakki-relcheck` and `LICENSING.md`; also scans the image contents (not only Cargo.lock) so a stray vendor binary cannot slip in. |
| Licence review of model weights (LLM, TTS, speech), fonts, icons, samples, the store catalogue entries | Show the licence in the Setup tile before a download. |
| Codec and patent decision (H.264/AAC/others: ship, download-on-request, or leave out) | Today removed from the image (`WEB_H264=1` keeps them in a self-built image). |
| Supported machines list | |
| Commit slicing of the ~600 uncommitted paths into reviewable commits, and release notes | Only when the owner says. |
| Fix the inconsistent rows: F-INST-03 'done' vs internal disks 'never tried'; F-PWR-01 'planned' but in the alpha.2 scope; KNOWN-ISSUES and HARDWARE saying the MacBook and PCs were never booted | |
| Hardware video decode (or a clear, honest statement of CPU decode limits) | YouTube is smooth at 720p on the T2 MacBook with CPU decode; decide the target. |
| IYAKKI-drawn Touch Bar strip on the T2 Mac (the USB configuration switch falls back) | Keys row works; strip is a separate bring-up. |

## Added by the owner (not started)

| Item | Status | Notes |
|---|---|---|
| **Lock screen** | idea, design asked | There is no lock screen today (the screen saver signs in again). Needed by two-step sign-in (unlock with a code), by 'lock when I leave', and by suspend. See `docs/development/LOCK-AND-ENCRYPTION.md`. |
| **Option to encrypt the data partition** | idea, design asked | Off by default, chosen in the installer or later in Settings; the unlock happens at boot (password or recovery key). Protects a stolen drive, which two-step sign-in does not. |
| **Option for a separate encrypted vault partition** | idea, design asked | A small partition only for secrets (vault, keys, tokens), encrypted on its own, so the big data volume can stay unencrypted for speed (or be encrypted separately). |
| **Clipboard: copy text from a QR code seen by the camera** (owner idea 2026-10-24: get an API key or any text onto the Mac without a remote door) | in progress | Reuses the in-house QR decoder and camera scan of the Claude Code sign-in; camera permission and 'camera is on' indicator; scanned text can be marked secret (kept out of the history, cleared after paste or 60 s). Candidate later: 'send text to the clipboard' in developer mode with the `input` scope, an approval card and auto-clear. |
| **TALES: option to use the selected picture/image model** (illustrations: none / cover / every chapter / ask; off by default; disabled with a plain reason when no picture model is selected) | in progress | Uses the existing Lemonade image backend (`lemonade_image`); stored with the story; caps, cancel, regenerate, honest timing. |
| **Omni collections: 'Use all parts'** (owner question 2026-10-24: what is LMX-Omni-5.5B-Lite and how to use it; **owner decision: worth trying, test in the next batch**; he will pull SD-Turbo again himself) | in progress (next batch) | Lemonade models with recipe `collection.omni` are bundles: `LMX-Omni-5.5B-Lite` = Qwen3.5-4B-MTP-GGUF (chat+vision) + SD-Turbo (pictures) + Whisper-Tiny (speech in) + kokoro-v1 (speech out); the list's `components` field names the parts; the Models page shows it only as 'chat'. Build: show the parts, a 'Use all parts' button that sets chat/vision, picture, STT and TTS engines in one step (checks each part is installed, Install for the missing ones), a TALES 'story kit' and a hands-free chat preset; look at `RPG-HaloTales-V1/V2` (collections, 40/56 GB, not installed) for TALES. Note: removing a component model (the owner removed SD-Turbo) breaks the collection's picture part. |
| **Connect Git or Gitea from Settings and the AI IDE** (owner priority 2026-10-24: 'in AI IDE, and settings need option connect git or gitea') | in progress, TOP priority | Today: Settings -> Git (hosts/keys), the IDE git panel, and the Gitea platform service reached only via IDE K8S -> Git. New: a 'Connections' list and add wizard (GitHub, GitLab, Gitea/Forgejo on the cluster or existing, Other), token or SSH key, Test, commit identity, default; IDE: 'Connect Git / Gitea' action, create repo, set origin, push, connect to an existing repo. Tokens only in the vault. |
| Authenticator (TOTP) two-step sign-in, switched on by each person in Settings | design | `docs/development/TOTP-2FA.md`; 12 open decisions. |
| Own mixer for the T2 speaker (ducks other programs, plays notification tones over them) | design | Owner's SDK answers; `docs/development/SDK-API.md`; memory note on dmix. |
| Per-install identity (a stick and an internal install must never mount each other's data) | design | `docs/INSTALLER.md`, section 'Per-install identity'. Waits for the stage1 patch work. |
| One 'IYAKKI' entry: full first boot, then fast start by itself; import settings and firmware from another drive | design | `docs/development/SELF-PREPARING-BOOT.md`, `IMPORT-FROM-DRIVE.md`. |
| Installer on the framebuffer (bigger text, mouse and touch) | question to the owner | The console installer stops at 2x. |

## Bugs and fixes from the Mac tests

| Item | Status | Notes |
|---|---|---|
| Lemonade downloads (Whisper and every other model) report 'the server stopped answering before the download finished' although they finish: the shell did not recognise `event: complete`, and its data line (zero bytes fields) read as one more progress step | in tree | Fixed in `models.rs::pull_line` with a test of the real stream; patchable only by the next image (it lives in the main program). Found 2026-10-24 on the internal T2 install. |
| Models page shows 'connection refused' if Lemonade is still starting; should wait/retry and say 'Lemonade is still starting' | idea | Seen as 'connection refused' in the first minutes after a cluster restart. |
| **T2 built-in microphone is off (driver `capture=0`): dictation says 'not supported yet' (opening capture while sound plays has hung or reset a MacBookPro16,2)** | hardware, 1.0.0 item | Today's warning is true. To be actionable and not strange: say 'use a USB microphone or headset' and list detected capture devices; investigate the hang in apple-bce `aaudio` capture (read-only first: which stream, which command, whether a mic alone without playback is safe) and either make it safe or keep it off with a Settings switch and a risk text (SDK-API decision 8 is open); a phone microphone relay (record on the phone, send to the Mac) is another way. Never enabled on the owner's Mac without his say. |
| **Browser has no sound in the first session, works after closing and reopening the browser** (seen twice by the owner on the T2 MacBook: 2026-10-24 right after boot, and again later) | open, evidence needed | Likely cause: the speaker has one owner at a time (no mixer); some other program (the radio, an earlier browser session, a cue, the agent's volume-control prime) or an idle open stream holds it, and the browser's sink then plays nothing without telling anyone. Evidence still missing: read `/proc/asound/card0/pcm0p/sub0/status` (owner_pid, state) and the web process's open sound descriptors **while it is silent**. Candidate fixes: (A) show 'Sound is in use by <program>' in the browser tile (the SDK `Busy` idea: `quiet.rs` already finds the owners of open streams); (B) the owner-agreed own mixer (ducks others, plays tones over them); (C) retry opening the sink when the engine reports an unreadable device. |
| **K8S tile shows a failure banner at times, Refresh fixes it** (owner, 2026-10-24, after the `-A` bug was fixed) | in progress | Transient `kubectl` failures (killed or slow under load, API busy right after boot, 8 s limit, connection refused while k3s restarts, aggregated metrics API 503) replace the view with a banner and are never retried. Fix: classify transient/persistent, retry with backoff in the shared fetch, keep the last good data on screen marked stale, show the banner only after about 25 s with last-worked time and the real error, a Cluster log with Copy for evidence. |
| **Developer-mode system update: variant name mismatch** (found 2026-10-24 on the real Mac): `iyakki dev update-pack --variant macweb` makes a payload the device refuses ('the variant is not valid' for `macweb-x86_64`; 'this update is for the macweb image and this device runs the macweb-x86_64 image' for `macweb`), so the update needed `--force`; and `update-pack` of the install image needs `--version` because the image has no `boot/iyakki-release`. Fix: one variant vocabulary on both sides (accept `macweb` and `macweb-x86_64` as the same), and write `boot/iyakki-release` into the install image. The update itself worked (2 files replaced, 7 unchanged, previous version kept in the boot menu). | open | `docs/DEVELOPER-MODE.md` section 7 |
| YouTube sound (muted preview players hold the speaker) | in image (autoplay deny when the speaker cannot be shared) | Verified by the owner: sound works. First attempt right after boot had no sound: cause unknown; retest. |
| Sound stutter, dmix spinning a core on the T2 | in image (plughw, patched driver with real periods) | Radio verified by the owner. |
| Touch Bar icons dark after a minute (kernel auto-dim) | in image (auto-dim off, backlight relit) | Verified by the owner. IYAKKI-drawn strip still never opens on this Mac (USB config switch falls back). |
| Page fullscreen in the browser tile | in image | Verified by the owner: works. |
| K8S tile 'kubectl failed' (kubectl 1.37 refuses `-A` before the verb) | in tree; live wrapper on the stick | Needs the next image. |
| 'The IYAKKI drive was disconnected' on an internal NVMe (watcher read controller state `live` as offline) | in tree; live workaround on the internal install | Needs the next image. |
| Sound keeps playing while powering off or restarting | in tree (OS quiet sequence + apple-bce patch 0004) | Hardware check needed. |
| GRUB loading text letter gaps (font name mismatch) | in tree | Menu and loading text fixed in the post-image step; real screen check needed. |
| Installer: copy of the fast-start menu entries onto a disk with no tree | in tree | Internal disk menu already fixed by hand. |
| Installer: bigger text; plain confirmation instead of typing | in tree | Erasing plans still need a second deliberate step. |
| Tooltip for device lists that hide the IP | in progress | |
| Claude Code sign-in: bigger QR, scan a QR from the phone with the Mac camera | in progress | In-house QR decoder. |
| YouTube had no sound on the very first attempt after a boot | hardware | Owner to retest in the first minute after boot; then read the speaker state. |

## Open decisions for the owner (collected 2026-10-24)

Answered ones are marked in their own documents. Where a document holds the full list, only the pointer and the headline are here. **Recommended** is the agents' suggestion, not a decision.

| # | Question | Where | Recommended |
|---|---|---|---|
| 1 | Draw the boot installer on the framebuffer (bigger text, mouse and touch) instead of the text console, which stops at 2x? | installer agent report; `docs/INSTALLER.md` | Yes (medium effort) |
| 2 | Does 'reuse over a blank partition' count as non-erasing (one confirmation instead of two)? | `docs/INSTALLER.md` rule 2 | Yes |
| 3 | Build per-install identity (each install mounts only its own data volume)? | `docs/INSTALLER.md` 'Per-install identity' | Yes, after the stage1 patch work is verified |
| 4 | Should the older `iyakki install` and stick-writer paths also strip the fast-start files? | installer agent report | Yes |
| 5 | May `/tmp` be cleaned (old `iyakki-*` folders, `*.log`, `*.out` older than an hour), or set `CLAUDE_CODE_TMPDIR` to another disk? | tool limit seen in this session | Point the tools at a bigger disk |
| 6 | Strictness of the 'no proprietary software' rule: may redistributable firmware blobs (Intel, AMD, Mac Broadcom Wi-Fi) stay in default images, listed with their licences, or must the default image carry none? | release gate point 7 above | Keep them, listed openly |
| 7 | T2 built-in microphone: which option(s)? | 'Options to decide' below | A and E now, B as an experiment |
| 8 | Two-step sign-in (TOTP): 12 decisions (build order, vault vs file for the secret, hash for recovery codes, reset by console only or also boot menu, lockout numbers, step-up list, lock screen, SSH coverage, grace period, suggest after first account). Off by default, switched on by the person in Settings is decided. | `docs/development/TOTP-2FA.md` | Build order: enrolment + sign-in, then recovery, then step-up |
| 9 | Lock screen and encryption: 13 decisions (disk password = account password? crypto stack, kernel options, `boot-prev/`, vault container or partition first, lock screen on by default?) | `docs/development/LOCK-AND-ENCRYPTION.md` | Lock screen first; dm-crypt LUKS2 + Argon2id; vault container before partition |
| 10 | SDK API: open items 3, 5, 7, 8, 9, 11, 12, 14, 15 (default model or pick, markdown links, service-token lifetime, microphone, external-agent usage, QEMU auto-pair, `screen_text` scope, element-box lookup, where the API description lives) | `docs/development/SDK-API.md` | See the document |
| 11 | Self-preparing boot: D1 (unpack the image or copy the live root), D5 (smallest data volume that gets fast start), D7 (which menu entries stay forced full), D8 (marker names) | `docs/development/SELF-PREPARING-BOOT.md` | Unpack the image; see the document |
| 12 | Import from another drive: D3 (may a password hash move), D4 (Apple firmware between drives of the same Mac model), D5 (Qualcomm firmware copy), D7 (browser profile) | `docs/development/IMPORT-FROM-DRIVE.md` | Re-ask for passwords; same model only |
| 13 | Release plan: beta name and bar and a scope freeze, Bluetooth waiver for the Mac mini 2018, Secure Boot in or out, a CI runner for Gitea, official store key and URL, legal opinion (firmware, codecs, TTS weights), commit slicing of the ~600 uncommitted paths, supported machines list | `docs/project/RELEASE-READINESS.md` section 8 | See the document |
| 14 | Order of the next image: when the QR work compiles, merge all rows, rerun tests, one image for the stick and the internal disk | this session | Do it as soon as the tree builds |

## Options to decide: T2 built-in microphone (owner asked to list them, 2026-10-24)

The built-in microphone of a T2 MacBook is off (`apple-bce` parameter `capture=0`), because opening a capture stream while sound plays has hung or reset a MacBookPro16,2. Choose one or combine; all four are 1.0.0 candidates.

| Option | What it is | Pros | Cons / risks | Effort |
|---|---|---|---|---|
| **A. Keep it off, say it clearly** | Dictation and the voice tile say 'The built-in microphone is not supported on this Mac yet: plug in a USB microphone or headset', list the capture devices found, and link to Settings -> Sound. | Safe; true; no hang. | No built-in mic. | S |
| **B. Settings switch: 'Use the built-in microphone (experimental, may freeze this Mac)'** | A per-machine switch (administrators) that sets `capture=1`, with a risk text, a restart-safe default off, and an audit line. | Lets the owner try and gather evidence. | A hang loses unsaved work; must never be on by default. | S |
| **C. Find the cause and make it safe** | Read-only investigation first: which command or stream hangs (capture alone vs capture plus playback), the T2 reply logs, `aaudio` capture buffers; then a driver patch (serialise capture/playback start, timeouts, never open the loopback). | A real fix, no warning needed. | Needs hardware sessions with the owner present; a hang is possible during tests. | L |
| **D. Phone microphone relay** | The phone records in its browser and sends audio to the Mac over the existing remote/relay link; dictation reads it like a recording. Also useful on any machine without a microphone. | No T2 risk; works today on any phone. | Needs the phone and a network; privacy text and a visible 'phone is recording' indicator. | M |
| **E. USB microphone or headset (already works)** | Documented path; detect and select it in Settings -> Sound/Voice. | Works now. | Needs hardware the person owns. | S (docs and detection) |

Recommended order: A and E now, B as an owner-controlled experiment, C when a hardware session is scheduled, D as a feature.

## Platform work

| Item | Status | Notes |
|---|---|---|
| SDK phase 1a (llm, models.list, speech.say, per-app usage) | in tree | Not built: embed, markdown, audio, media, dash, flows, k3s, speech.listen, money caps. |
| SDK phase 1b (`iyakki sdk init/doctor/docs`, d.ts, templates) | in tree | Emulator and `iyakki dev mcp` not built. |
| Remote hot patch (`iyakki dev patch`) | in tree | Stage1 overlay never run on real boot: QEMU first. |
| Service package kind (apps that ship a k3s deployment, install wizard lists permissions) | approved, design | `docs/development/CLAUDE-CODE-APP-DEVELOPMENT.md` section 6. |
| Release readiness | audit done | `docs/project/RELEASE-READINESS.md`: beta about 12 to 16 weeks, 1.0.0 6 to 12 months; 10 owner decisions. |

## Housekeeping

* Merge the FEATURES.md rows reported by the agents; fix `BOUNDARY.md` for `devmode/patch.rs`, `quiet.rs` and the apple-bce 0004 patch.
* The /tmp quota is full (a RAM disk with old test folders); clean old `iyakki-*` folders or set `CLAUDE_CODE_TMPDIR`.
* 598 uncommitted paths: slice into commits when the owner says.
* Docs reorganisation into `docs/<topic>/` (one pass).
