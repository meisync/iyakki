# Documentation index

**For:** anyone looking for the right document. **You will get:** every document in the repository, grouped by what you want to do.

## Start here

| Document | Read it for |
|---|---|
| [`../README.md`](../README.md) | What IYAKKI is, the tiles, the three ways to run, status |
| [`INSTALL.md`](INSTALL.md) | Window mode, USB stick, installing to a disk, VM, variants, upgrade, backup, boot problems |
| [`QUICKSTART.md`](QUICKSTART.md) | A 15-minute tour with expected results |
| [`ZRAM-AND-INSTALL.md`](ZRAM-AND-INSTALL.md) | Memory tuning (zram, swap file) and the on-device installer (`iyakki install`, Settings, System) |
| [`HARDWARE.md`](HARDWARE.md) | Supported and tested hardware, quirks |
| [`../KNOWN-ISSUES.md`](../KNOWN-ISSUES.md) | What is partial, broken or untested in this alpha |

## Use

| Document | Read it for |
|---|---|
| [`USER-GUIDE.md`](USER-GUIDE.md) | Every tile, its keys, tips and limits; global keys, approvals, inbox |
| [`FAQ.md`](FAQ.md) | Short answers |
| [`TROUBLESHOOTING.md`](TROUBLESHOOTING.md) | Problems we hit, with fixes, and what to send in a report |
| [`AI-IDE.md`](AI-IDE.md) | The AI IDE: workspaces, board, Browser tab, Kubernetes view, Autopilot |
| [`K8S-TILE.md`](K8S-TILE.md) | The K8S tile: pick a cluster and a namespace and explore it (YAML, events, related objects, pod logs); read-only unless an administrator allows changes |
| [`GIT-TILE.md`](GIT-TILE.md) | The GIT tile: browse the repositories of your Git connections and the clones on this computer (files, commits and diffs, branches, issues and pull requests, read-only), clone, open in the AI IDE, fetch, pull, push with their reviews; limits, retries and what is not done |
| [`DISKS.md`](DISKS.md) | The DISKS tile: see every drive, mount, unmount and safely eject, change partitions (administrators, with confirmations); safety rules, file system table, what is not built |
| [`AUTH.md`](AUTH.md) | The AUTH tile: one-time codes (TOTP, RFC 6238) in groups, kept with their names in the person's own vault; add by typing, an otpauth:// link or the camera's QR scan; copy as a secret; the clock check |
| [`CHECKUP.md`](CHECKUP.md) | The CHECK-UP tile: a guided hardware self-test (screen, touch, keyboard, pointer, speaker, microphone, camera, monitors, network, Wi-Fi, Bluetooth, battery), Pass / Fail / Skipped with advice, camera and microphone only on a click and nothing kept, the result through Share diagnostics; the manual list for a real computer |
| [`STUDY.md`](STUDY.md) | Study plans and quizzes: ask the chat for "learn X in 21 days" or "quiz me", the program does the dates, the checks and the score; the LEARN tile shows the same plans and quizzes; report a wrong question |
| [`LAB.md`](LAB.md) | The LAB tile: try picture-making and vision models, compare two on the same prompt or picture, sample a video into frames for a vision model; the log, the caps, what goes to a remote provider |
| [`REMOTE-PROTOCOL.md`](REMOTE-PROTOCOL.md) | The browser front end's WebSocket messages, including browser speech |
| [`DATA.md`](DATA.md) · [`DASHBOARD.md`](DASHBOARD.md) · [`FLOWS.md`](FLOWS.md) · [`FLOWS-QA.md`](FLOWS-QA.md) · [`STORE.md`](STORE.md) · [`MEDIA-SEARCH.md`](MEDIA-SEARCH.md) · [`MEDIA-PLAYER.md`](MEDIA-PLAYER.md) · [`KEYBOARD-AND-FN-KEYS.md`](KEYBOARD-AND-FN-KEYS.md) · [`ACCESSIBILITY.md`](ACCESSIBILITY.md) · [`BATTERY.md`](BATTERY.md) · [`CLOUD-MODE.md`](CLOUD-MODE.md) · [`MULTI-DISPLAY.md`](MULTI-DISPLAY.md) | The DATA, DASH, FLOWS, STORE (including the private store, several stores and the portal) and MEDIA tiles in depth |
| [`STORY.md`](STORY.md) · [`VOICE.md`](VOICE.md) | TALES as a conversation (panel, automatic pictures, character sheet, cartoon composer, versions) and reading aloud (engines found on the computer, Settings, Voice, the browser voice helper), the VOICE tile (read aloud, dictate) and the system sounds |
| [`APP-AUTHORING.md`](APP-AUTHORING.md) | Making a skill, MCP server or tool, testing it and publishing to a private store |
| [`DEVELOPER-MODE.md`](DEVELOPER-MODE.md) | Developer mode: pair a computer, then `iyakki dev install`, `push --watch`, `logs`, `run`, `screenshot` (like adb); the security model |
| [`UPDATES.md`](UPDATES.md) | Settings -> System update: install a newer IYAKKI from a folder or USB stick with the previous version kept, roll back, the checks, what is not built, how to try it |
| [`HOST-FILES.md`](HOST-FILES.md) | Window mode: drag and drop, shared folders, saving to the computer |
| [`BROWSER-AGENT.md`](BROWSER-AGENT.md) · [`WEB-RESEARCH.md`](WEB-RESEARCH.md) · [`FULL-BROWSER.md`](FULL-BROWSER.md) · [`BROWSER-ROADMAP.md`](BROWSER-ROADMAP.md) | The assistant in the browser, web research, the optional full web engine, what the browser cannot do |
| [`EXTERNAL-AGENTS.md`](EXTERNAL-AGENTS.md) | Claude Code, GitHub Copilot CLI and custom agents; terms notes |
| [`PARALLEL-TASKS.md`](PARALLEL-TASKS.md) · [`K3S-DEV.md`](K3S-DEV.md) | Parallel tasks in the IDE; building and deploying to k3s |
| [`GIT-AND-SSH.md`](GIT-AND-SSH.md) | Git in the AI IDE: clone several repositories into one workspace, SSH keys and tokens per site and per workspace, the key service in memory, known hosts, git in the image |
| [`SHARED-FOLDERS.md`](SHARED-FOLDERS.md) | Share a folder over HTTP on the local network (files or a website): modes, the security model, limits, sharing from the AI IDE, troubleshooting |
| [`FIREWALL.md`](FIREWALL.md) | The firewall: incoming connections blocked by default, rules by host name, IP, range or network, presets, the 60-second Keep, the blocked-connection list, how to get out of a lockout, the kernel mechanism and why it is our own code |
| [`CLI-SSH.md`](CLI-SSH.md) | The SSH client in the CLI tile: which client, host keys, vault keys, the Connections tab, what the assistant may do |
| [`QUICKSTART-BUILD.md`](QUICKSTART-BUILD.md) | One page: build an image, run it in a window or VM, write it to a disk, update a running machine, deploy to the cloud |
| [`JOBS.md`](JOBS.md) | Detached jobs: long commands that outlive the chat, the Jobs strip, `job_start` for the assistant and Claude Code |
| [`MACOS.md`](MACOS.md) · [`MAC-HARDWARE.md`](MAC-HARDWARE.md) | IYAKKI on a Mac: window, VM, Intel Mac hardware, driver packs |
| [`BLUETOOTH-INPUT.md`](BLUETOOTH-INPUT.md) | Bluetooth keyboards, mice and game pads (classic HID, no BlueZ): pairing, reconnect, Settings and chat, what is not done (audio, low energy), the test steps for a real keyboard |
| [`MAC-WIFI-BLUETOOTH-SETUP.md`](MAC-WIFI-BLUETOOTH-SETUP.md) | Wi-Fi and Bluetooth firmware on T2 Macs: script route, Linux-side route, a prompt for an AI agent, checks, rollback |
| [`YOUTUBE-ON-IYAKKI.md`](YOUTUBE-ON-IYAKKI.md) | YouTube per engine and image (window, web, macweb), Haswell graphics, RAM, test steps, diagnostics |

## Administer

| Document | Read it for |
|---|---|
| [`ADMIN-GUIDE.md`](ADMIN-GUIDE.md) | Accounts and roles, policy, vault, models, agents, audit log, backups, ports, hardening, k3s, store keys, recovery |
| [`OTHER-BASES.md`](OTHER-BASES.md) | Running on another Linux distribution, as a service, swapping the base OS |
| [`../SECURITY.md`](../SECURITY.md) | Reporting vulnerabilities; what the alpha does not promise |
| [`PLATFORM-SERVICES.md`](PLATFORM-SERVICES.md) | Deploying Gitea and the private store into a cluster you pick (Settings, Dev cluster, Platform services): plan, approval, backup, limits |
| [`OWNER-CHECKLIST.md`](OWNER-CHECKLIST.md) | What only the owner can do before tagging a release, in order |

## Build and extend

| Document | Read it for |
|---|---|
| [`../CONTRIBUTING.md`](../CONTRIBUTING.md) | Build, test, style, the feature matrix and boundary rules, adding tiles/tools/skills/nodes/connectors |
| [`../CODE_OF_CONDUCT.md`](../CODE_OF_CONDUCT.md) | How we work together |
| [`PROJECT-TEAM.md`](PROJECT-TEAM.md) | The idea and requirements team, sponsors and hardware testers |
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | One-page overview, crate map, boot flow, security model, where to extend |
| [`../BOUNDARY.md`](../BOUNDARY.md) | Every place our code touches the base OS; the rules |
| [`../FEATURES.md`](../FEATURES.md) | The feature matrix: status, progress and tests per feature |
| [`development/SDK-API.md`](development/SDK-API.md) | The developer SDK API (design): bridge, local HTTP, MCP, embedding, usage budgets, the remote debug loop |
| [`development/BROWSER-MEDIA-DEVICES.md`](development/BROWSER-MEDIA-DEVICES.md) | Why the full browser has no camera or microphone (engine built without media-stream, helper setting off, no capture plugins), the permission prompt and store that now exist, what a Buildroot rebuild must change, steps and owner decisions |
| [`development/TOKEN-BUDGET.md`](development/TOKEN-BUDGET.md) | Where the tokens of a chat request go (system, tools, history, outputs), the per-run log line and footer, measured numbers for plain, Flows and Browser chats before and after tool routing, how routing and `more_tools` work, the Settings switch |
| [`development/AUDIO-MIXER.md`](development/AUDIO-MIXER.md) | The in-house audio mixer (several sounds share the T2 speaker, ducking, priorities, `sound.mixer`); what reaches it and what does not |
| [`development/QEMU-SELFTEST.md`](development/QEMU-SELFTEST.md) | The QEMU boot test and the self-test lines it requires, including the "smallest call per path" checks (platform connect, registry, cluster tools, Git client, DOCTOR probes, CA bundle, image programs) |
| [`development/DATA-SCIENCE-ADDON-PLAN.md`](development/DATA-SCIENCE-ADDON-PLAN.md) | Plan: scikit-learn, NumPy, SciPy and OpenCV as a click-to-download Quick install item (never shipped): verified licence audit of the wheels, musl vs glibc, pinned download table, sandboxed `python_run`, security gate, steps |
| [`development/GRAPH-DB-CHOICE.md`](development/GRAPH-DB-CHOICE.md) | Graph database for the AI: verified comparison (licence, image digest, RAM, read-only, Cypher, vectors) of Neo4j, Memgraph, FalkorDB, Kuzu, AGE, ArangoDB, SurrealDB, Dgraph, Nebula, TypeDB, JanusGraph; the choice (Neo4j Community, GPL-3.0, separate pod) and the design of the code graph for the AI IDE |
| [`development/VPN-CONNECTOR-PLAN.md`](development/VPN-CONNECTOR-PLAN.md) | Plan: WireGuard (kernel module, in-house netlink) and OpenVPN (separate program) in Settings -> Network -> VPN; verified licences, privileged helper boundary, kill switch, profile sanitising, security gate, steps, owner decisions, proposed `LICENSING.md` note |
| [`development/MINIMAL-WEBSERVICE.md`](development/MINIMAL-WEBSERVICE.md) | Scoping: running a minimal set of IYAKKI as a web service; three readings, named profiles (`agent-api`, `flows-runner`, `data-api`, `tile-host`, `full-ui-remote`), API and auth (no new scheme), deployment forms, v1.0.0 security gate, relation to the UI/OS split, steps, owner decisions |
| [`development/DOWNLOADS-MANIFEST.md`](development/DOWNLOADS-MANIFEST.md) | Plan, format and threat model of the signed downloads manifest (versions and checksums of the download items updated without a new ISO; rules, signature, rollback protection, migration backlog). |

## Design and deep dives

[`DESIGN.md`](DESIGN.md) (visual language) · [`EMBEDDING-AND-BACKEND.md`](EMBEDDING-AND-BACKEND.md) (components and a remote backend) ·
[`ASSISTANT-ROBUSTNESS.md`](ASSISTANT-ROBUSTNESS.md) (tool use, thinking models, speed) · [`AGENTIC-RAG.md`](AGENTIC-RAG.md) (the assistant's own retrieval: plan, search, grade, rewrite, cite) · [`WEBVIEW-PROTOCOL.md`](WEBVIEW-PROTOCOL.md) (shell to web-engine
helper) · [`JS-PERFORMANCE.md`](JS-PERFORMANCE.md) · [`BOOT-TEST.md`](BOOT-TEST.md) and [`LOCAL-TEST.md`](LOCAL-TEST.md) (testing on this computer and on real
hardware) · [`../Agent OS Architecture Specification.md`](<../Agent OS Architecture Specification.md>) (the original design and roadmap).

## Release and legal

| Document | Read it for |
|---|---|
| [`../CHANGELOG.md`](../CHANGELOG.md) · [`../RELEASE-NOTES-0.1.0-alpha.1.md`](../RELEASE-NOTES-0.1.0-alpha.1.md) | What is in this release |
| [`BUILD-INCREMENTAL.md`](BUILD-INCREMENTAL.md) | Build only what changed: change detection, repack, layered macweb, shared caches |
| [`LINUX-APPS.md`](LINUX-APPS.md) · [`BUILD-YOUR-OWN-IMAGE-APP.md`](BUILD-YOUR-OWN-IMAGE-APP.md) | Linux apps: a Debian or Ubuntu sandbox (downloaded, checked, namespaced; apt, folders, network switch, terminal, AI IDE), image apps, building your own; phase 2 (graphical) design |
| [`GPU-K3S.md`](GPU-K3S.md) | GPU for local models in k3s: Vulkan on any GPU, the NVIDIA driver, the PC image, the RTX 4090 checklist |
| [`FIRMWARE-LICENCES.md`](FIRMWARE-LICENCES.md) | Which firmware and GPU driver parts ship, which are downloaded, the licence conditions and the open legal questions |
| [`RESTRICTED-FIRMWARE.md`](RESTRICTED-FIRMWARE.md) | Firmware that is not in the images (Qualcomm Bluetooth, Realtek and Broadcom Ethernet, Atheros AR9170): what degrades per chip, installing it at a click or from a stick, building an image that includes it, what you accept |
| [`MAC-MODEL-HOST.md`](MAC-MODEL-HOST.md) | Model host on your network: use a Mac's GPU (Metal) or another GPU computer from IYAKKI and from k3s pods: setup, security, test, scan, memory table |
| [`APP-MODE-AND-APPLE-SILICON.md`](APP-MODE-AND-APPLE-SILICON.md) | Plan and feasibility: Debian/Ubuntu apps, Windows .exe via Wine, Apple store apps (not possible), Apple Silicon models via a macOS bridge |
| [`RELEASING.md`](RELEASING.md) | How a release is made, signed and checked |
| [`../SUPPORT.md`](../SUPPORT.md) | Getting help |
| [`../LICENSE`](../LICENSE) · [`../NOTICE`](../NOTICE) · [`../LICENSING.md`](../LICENSING.md) · [`LICENSES.md`](LICENSES.md) · [`../THIRD_PARTY_LICENSES.txt`](../THIRD_PARTY_LICENSES.txt) | Licences and third-party notices |
| [`BETA-SCOPE.md`](BETA-SCOPE.md) · [`BETA-PLAN.md`](BETA-PLAN.md) | Every non-done feature row classified for the beta; the plan, risks and owner decisions |
| [ALPHA-2-PLAN.md](ALPHA-2-PLAN.md) | What goes into 0.1.0-alpha.2: PC image and GPU, power settings, multi-repo clone, Wi-Fi/Bluetooth, fixes |
| [`project/AUDIENCES-AND-SELLING-POINTS.md`](project/AUDIENCES-AND-SELLING-POINTS.md) | Who IYAKKI is for (students, developers, ops, office, home, home automation, internet automation), what each can do today, the gaps, what not to claim, and a video plan |
| [`development/UI-OS-SPLIT-PLAN.md`](development/UI-OS-SPLIT-PLAN.md) | Approved plan: apps -> IYAKKI core -> kernel layering, `iyakki-core-api` / `-local` / `-fake`, the nine migration steps in detail (1-4 built), and the open decisions |
| [`development/TILE-TESTING.md`](development/TILE-TESTING.md) | How to test a tile: web-bundle tile by default (scaffold, `tile_test`, `dev push --watch`); the headless `TileHarness` for Rust tiles; the dependency ratchet |
