# Changelog

All notable changes to IYAKKI are written here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and the project uses [Semantic Versioning](https://semver.org/). Every 0.x release can change anything; the alpha series
(`0.1.0-alpha.N`) additionally promises nothing about stability, data formats or upgrade paths (see
[SECURITY.md](SECURITY.md) and [KNOWN-ISSUES.md](KNOWN-ISSUES.md)).

The rows behind each item, and the tests that prove them, are in [FEATURES.md](FEATURES.md). What is only partly true is
listed in [KNOWN-ISSUES.md](KNOWN-ISSUES.md); an item below is not a promise that it was tried on real hardware.

## [Unreleased]

Nothing yet.

## [0.1.0-alpha.1] - 2026-10-19

First public alpha: a preview for developers and testers, not for production or for machines holding data you cannot lose. Everything is
new, so this is the list of what exists, grouped by area. "Tested" means automated tests and QEMU boots; the one real computer used is a
**Mac mini 2018 (Macmini8,1, T2), checked on 2026-10-18/19** (see ALPHA-GATE.md and KNOWN-ISSUES.md). Last full test run on 2026-10-18:
3,845 passed, 0 failed, 59 ignored.

### Added

**Hardware: Mac mini 2018 (the only real machine tried)**
- Boots the install image from a USB SSD through GRUB with a logo menu on UEFI; boot screen with logo, progress and messages; first-boot
  data volume format in 178 ms and a persistent `/home`. GRUB waits 20 to 30 s at the logo today (a side image on the boot partition is planned).
- HDMI audio and the built-in speaker and headphone jack through the out-of-tree `apple-bce` driver; software volume and mute.
- USB webcam through the `uvcvideo` quirk applied at boot; the CAMERA app takes photos, video and passport pictures.
- Not working yet: the Mac's Wi-Fi (BCM4364 needs Apple's firmware copied from macOS) and Bluetooth (needs `brcm/BCM.hcd`). T2 MacBook Pro 2019
  patches and Apple Silicon support are scaffolding only.

**Browser and media (web and macweb images)**
- The full web engine (WPE WebKit) opens Google and YouTube; typing in pages works; YouTube video plays (VP9 + Opus, software rendered).
  H.264/AAC is deliberately not shipped. A media player built on `ffmpeg` (downloaded by Quick install at the user's request), a radio (MP3),
  and the camera app. The media player is not yet confirmed on a real screen.

**Quick install and the dev cluster (tried on the Mac mini)**
- A k3s dev cluster (node Ready), Elasticsearch, DuckDB (Python wheel), Node.js, Python 3.13 (standalone) and `ffmpeg`, all downloaded from
  their vendors on request and never shipped. Claude Code and GitHub Copilot CLI install and sign in; Lemonade on the LAN works as a model.

**Apps, store, private store and Developer mode**
- Tile apps with one-step publishing (`tile_ship`, loopback only), a private store with an in-cluster registry one-click (fakes only), and
  Developer mode (`iyakki dev ...`, loopback only).

**Secure Boot**
- shim + MOK with signed GRUB, kernel and modules; verified in QEMU with OVMF (17 checks: signed boots, unsigned is refused). Not tried on real hardware.

**Added in the last days before the alpha**
- **Private MAC address** (Settings -> Network `m`/`a`, `iyakki net mac`, `net.conf` `mac.*`): Off, Per network or Random each connection for Ethernet and Wi-Fi; Ethernet defaults to Off (DHCP reservations keep working), Wi-Fi to Per network, so a Wi-Fi network now sees a made-up, stable address instead of the hardware one unless set to Off. Shown on the System tile; row F-NET-09, `docs/ADMIN-GUIDE.md`.
- **Browser (QA) nodes in FLOWS** (`docs/FLOWS-QA.md`): twenty nodes (open, screenshot, get DOM, check, click, type, fill form, submit, key, hover, scroll,
  select, wait, extract, run JavaScript, frame, viewport, cookie/storage, download, compare to a baseline) on one private browser session per flow run,
  closed when the run ends or is stopped; asked once per run unless you marked the flow trusted; host list, budgets, `vault:NAME` secrets that never
  reach a file, a QA report (per step status, checks, pictures, console errors) in the RUN view with HTML and JSON export, three QA templates, and an agent
  step that can be shown a screenshot by a model that can look at pictures. Verified with the in-house JavaScript engine and, in an ignored test, with the real
  WebKitGTK helper; not on the WPE helper of the image. (F-FLOW-17..20)
- **Jobs** (`docs/JOBS.md`): long commands that run detached from the chat, in their own session, with a log and a status file,
  surviving the chat, the session and a restart of the shell. The assistant (and Claude Code through the policy bridge) gets
  `job_start`, `job_status`, `job_tail`, `job_list`, `job_wait` and `job_stop`; a **Jobs strip** in the chat and the AI IDE shows each
  job with a spinner or check, elapsed time, a sliding or percentage bar, the last output line and Log / Stop buttons; RUNS has a
  Jobs list; a finished job is announced to the assistant once and leaves an inbox note; Autopilot waits for a running job instead
  of calling the step a stall. Not tried with a real Android SDK install or a real Claude Code session. (F-JOBS-01..06)
- **Private store and authoring** (`docs/STORE.md` section 13, `docs/APP-AUTHORING.md`): `iyakki store-server` serves a signed catalog with
  per-publisher tokens and full package verification; a device adds it with its key fingerprint confirmed and pinned; `iyakki pkg validate`,
  `pack` and `publish`; scaffolds for a skill, a Python MCP server, a Node MCP server and a WASM tool; three built-in authoring skills;
  assistant tools `app_scaffold`, `store_validate`, `store_publish` (always asks). (F-APPS-40..43)
- **The private store server as its own static program** (`crates/iyakki-store-server`, about 1 MB; `iyakki store-server` stays as an alias).
  The deploy no longer takes the running program (the desktop window can never run in a `FROM scratch` image): it finds a static file of the
  cluster's CPU (shipped with the install, the cache, the source tree), judging by the ELF headers; when none exists the result is a card with
  **Build now** (a detached job with a moving bar; the deploy continues by itself), **Choose a binary file...** and **Open docs**, and nothing is
  created in the cluster before the file is found. Releases carry `iyakki-store-server-<arch>`, the images ship it under
  `/usr/share/iyakki/store-server/`. (F-APPS-42, F-APPS-44)
- **Platform services** (`docs/PLATFORM-SERVICES.md`): Gitea and the private store deployed into a cluster you pick, with a plan, an exact
  review, your approval and a capacity pre-check; Settings, Dev cluster, Platform services (also from the STORE card and the AI IDE's K8S
  panel); assistant tools `platform_list`, `platform_status`, `git_service_status`, `platform_deploy`, `platform_upgrade`, `platform_remove`
  (the last three always ask). Tested with fakes, never on a real cluster. (F-K8S-16..18)
- **Store switcher and add-a-store:** a "Store: All / name" chip in Discover (key `s`), remembered per user, install always from the chosen
  store; STORE, Settings, Add a store takes a portal or `catalog.json` link and shows name, signer fingerprint and trust before anything is
  enabled; the official-store button says "coming soon" until the address is set. (F-APPSTORE-18, 19)
- **TALES as a conversation** (`docs/STORY.md`): a full-height chat panel beside the story (Plan, Write, Draw and Keep steps); later messages
  correct the story (*make it shorter*, *change the goat's name*, *translate it to Tamil*) and each correction is a saved version; automatic
  pictures per scene (real images when a picture model exists, otherwise SVG, otherwise a **built-in cartoon composer** the model only
  directs with small JSON); a **character sheet** keeps characters looking the same; the picture line says what drew each picture;
  Claude Code can write and draw; a model chooser, an agent chooser and one Options popover. (F-TALES-03, 06, 12..18)
- **Reading aloud without setup** (`docs/VOICE.md`): speech engines already on the computer (piper, espeak-ng, speech-dispatcher, festival,
  pico2wave, `say`, Windows) are found once in the background and used when nothing is set; Settings, Voice shows each engine, its voices,
  whether it can save audio and why not; a **browser voice helper** lets the desktop window speak through the default browser's voice
  (one-time link, 127.0.0.1, one tab); the browser front end speaks through WebSocket messages (`docs/REMOTE-PROTOCOL.md`). (F-VOICE-05..07, F-WEB-08)
- **Approvals that stick:** Allow once / **Allow for this session** (`Ctrl+U`) / Deny; session grants are scoped (program and subcommand,
  cluster and namespace, host, workspace file) and shared by every run of the chat; calls that always ask say "Always asks: reason";
  "Approved this session: N" lists grants with Revoke and Reset. The modes are now named **Ask first / Auto-approve safe / Autopilot**
  (saved choices carry over). (F-ASSIST-40)
- **Taller message boxes:** one shared multi-line input (three lines, growing to ten or twelve, then scrolling) with a 20,000-character
  limit in chat and the IDE chat (was 512) and 4000 in TALES (was 300), with a visible notice when text is cut. (F-CHAT-22, F-TALES-14)
- **Compressed memory and the installer:** zram swap set up by IYAKKI at boot (no systemd), optional swap file and zswap, SETTINGS, System,
  Memory; `iyakki install` and SETTINGS, System, Install to disk copy the booted stick to the computer's disk with a typed confirmation;
  seen working in QEMU only. (F-ZRAM-01..03, F-INST-01, 02; `docs/ZRAM-AND-INSTALL.md`, `docs/HARDWARE.md`)
- **Release tooling:** `./run.sh alpha-check`, `./run.sh release`, signed `release.json`, `iyakki-usb` manifest check, SBOM, commit plan
  script, `docs/OWNER-CHECKLIST.md`. (F-REL-01..07)

**Core, sandbox and security**
- A supervisor that runs WebAssembly guests in a sandbox with explicit grants (files, network hosts), instruction budgets (fuel),
  memory caps and exit codes (`iyakki run`); microphone and camera bridges for guests.
- Unforgeable, revocable capability tokens; roles, apps, limits and per-user overrides that cannot escalate; an egress proxy with a
  host allow-list, key injection and response sanitising.
- A local encrypted vault (XChaCha20-Poly1305, Argon2id key slots) with a per-user lifecycle, external secret providers
  (HashiCorp/OpenBao KV2, 1Password Connect), and credentials that stay in vaults end to end (the assistant learns names, never values).
- A hash-chained audit log and a replay ledger (`iyakki ledger-verify`).

**Shell and tiles**
- A framebuffer desktop of app tiles that follow the person's role: first-run setup, sign-in with throttling, USERS (accounts, roles,
  limits, scopes, budgets, audit), SETTINGS (network, Wi-Fi, Bluetooth, display, sound, storage, models, vault, agents, dev cluster),
  SYSTEM (processor, memory, load, disks, network, heat, processes, agents, hardware; `iyakki top`), INBOX (notes, tasks, approvals),
  CALENDAR with alarms, USAGE, SKILLS, CLI, NET SCAN, MESH (Consul), RADIO, MEDIA, TALES (story generator), EDIT, RUNS.
- Everything clickable and keyboard driven, a clipboard with history, screenshots, F1 help, a context menu, UI sizes from 85% to 200%,
  a common design system, touch screens, smooth drawing up to 4K.
- A desktop window mode (`./run.sh window`, a Linux x86_64 tarball) that shows virtual disks only, and a browser front end
  (`iyakki web`, pixel streaming over HTTPS and WebSocket with the machine's own accounts) with a container image and a K3s manifest.
- SYSTEM shows the version and, on an image, `/etc/iyakki-release` (build date, git describe, variant, kernel); `iyakki --version`.

**Assistant**
- Chat with saved sessions, session memory, skills, an agent mode with approvals (approve / always / deny), per-run budgets, run
  records, Markdown, SVG and image answers, thinking models, streaming, clickable links, questions answered by a click.
- Model providers: Ollama, OpenAI, OpenRouter, Anthropic, Gemini and compatible servers, native tool-calling for all three API styles,
  exact token counts, model download and picture generation from the chat; voice in and out.
- Tools: files with real diffs, commands with approval, web search and page reading, HTTP requests, calendar, radio and media, screen
  capture, data tables, dashboards, board and deploy tools; MCP client and tile; knowledge base (BM25 + vector, namespaces, external
  RAG, federated search).
- Robustness measures for small local models (a live tool reminder, retry on a refusal that contradicts a tool, stall and thinking
  watchdogs, hard per-step limits) and an Autopilot that works through a feature board with proof required for "done".

**Browser**
- A browser tile of our own: HTML/CSS layout, forms and POST, cookies, pictures (PNG, JPEG, GIF), tabs, find in page, bookmarks and
  downloads, a QuickJS-based script engine with a web platform of our own, ES modules, search from the address bar, consent
  prompts for the assistant's browsing, a browser agent and test specs (`iyakki browser-test`).
- Optional full web engine in the large `web` image (WPE WebKit, Mesa, GStreamer) for modern sites and video.

**AI IDE and editor**
- An editor with highlighting for many languages, find/replace, diffs per hunk, assistant proposals with review, inline completion,
  running the open file; the AI IDE tile with an isolated workspace per product, a feature board, bugs with evidence, a browser tab,
  parallel tasks, test and coverage parsers, and external coding agents (Claude Code, Copilot, custom) through a policy bridge.

**K3s and deploy**
- Connect to an existing cluster or let IYAKKI set up a local K3s; build, load/push, apply and verify flows with a manifest and
  Dockerfile linter, pull-failure and crash diagnosis, registry choices, teardown, cluster health in SYSTEM, a cloud mode inside a pod.
- Platform services deployed into a chosen cluster with an approved plan (Gitea, the private store) and a dev-cluster settings page.

**Data and dashboard**
- DATA tile and the assistant's table tools (CSV/TSV/JSON lines, schema, query); DASH tile with panels on a grid, connectors for
  Prometheus, Elasticsearch/OpenSearch, ClickHouse, DuckDB (through the command), mixed queries, vault-held credentials.

**Flows**
- A flow editor and runner (canvas, properties, scheduler) where a flow can call the assistant, the browser, deploy and data steps.

**Store and drivers**
- Signed packages and catalogs (ed25519; trust levels Official/Verified/Trusted/Community/Unsigned), revocation lists, offline use from a
  folder or stick, licence tokens for paid apps (no payment code), `iyakki pkg`, the STORE tile with Installed, Discover and Publishers.
- Driver packs: detection from `/sys` and `/proc`, firmware on the data volume with a boot hook, FaceTime HD camera firmware extracted
  from a file the person supplies, an optional Mac image variant. A private store server can run in a cluster.

**Installer and storage**
- A hybrid BIOS+UEFI ISO, an install image written by `iyakki-usb` (partitioned system plus an IYAKKI-DATA volume that fills the stick),
  persistence tested across reboots in QEMU, `iyakki storage` / `iyakki persist`, an ARM64 UEFI image for VMs, a hardware check and test
  guide, optional zram swap and installer improvements.
- `./run.sh release` and `./run.sh alpha-check`: signed release folders with `release.json`, a download page, and a pre-release gate;
  `iyakki-usb` checks an image against `release.json` or a `.sha256` file before it writes.

**Security and supply chain**
- Own implementations instead of dependencies where practical; a licence policy enforced by `cargo deny` (permissive only), the third-party
  texts in `THIRD_PARTY_LICENSES.txt`, an SBOM (`SBOM.json`), and the images' GPL/LGPL source information through Buildroot `legal-info`.

### Fixed
- **Audit log:** concurrent writers in one process could share the same `prev` hash and make the chain read BROKEN. Writes now take a lock.
  An existing broken chain is not repaired (start a new log, `docs/ADMIN-GUIDE.md`), and two separate processes on one file can still race.
- Typing in web pages, YouTube autoplay, the shared sound mixer (in the newest image, not yet confirmed on the Mac) and a 5.3 s input-device scan stall at boot (same).

### Changed
- `crates/iyakki-core/tests/feature_matrix.rs`: the id check now accepts digits in the area part of a feature id (for example `F-K8S-01`),
  and `B-` ids for bugs on workspace boards. Nothing else about the matrix rules changed.

### Known problems
See [KNOWN-ISSUES.md](KNOWN-ISSUES.md). The short version: real hardware was tried on one Mac mini 2018 only; its Wi-Fi and Bluetooth do not
work yet; H.264/AAC video does not play; Secure Boot is verified in QEMU only; payments, the official store root key, the remote runner and
the webhook trigger do not exist yet.

[Unreleased]: https://example.invalid/iyakki/compare/v0.1.0-alpha.1...HEAD
[0.1.0-alpha.1]: https://example.invalid/iyakki/releases/tag/v0.1.0-alpha.1
