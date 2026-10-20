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
| [`REMOTE-PROTOCOL.md`](REMOTE-PROTOCOL.md) | The browser front end's WebSocket messages, including browser speech |
| [`DATA.md`](DATA.md) · [`DASHBOARD.md`](DASHBOARD.md) · [`FLOWS.md`](FLOWS.md) · [`FLOWS-QA.md`](FLOWS-QA.md) · [`STORE.md`](STORE.md) · [`MEDIA-SEARCH.md`](MEDIA-SEARCH.md) · [`MEDIA-PLAYER.md`](MEDIA-PLAYER.md) | The DATA, DASH, FLOWS, STORE (including the private store, several stores and the portal) and MEDIA tiles in depth |
| [`STORY.md`](STORY.md) · [`VOICE.md`](VOICE.md) | TALES as a conversation (panel, automatic pictures, character sheet, cartoon composer, versions) and reading aloud (engines found on the computer, Settings, Voice, the browser voice helper) |
| [`APP-AUTHORING.md`](APP-AUTHORING.md) | Making a skill, MCP server or tool, testing it and publishing to a private store |
| [`DEVELOPER-MODE.md`](DEVELOPER-MODE.md) | Developer mode: pair a computer, then `iyakki dev install`, `push --watch`, `logs`, `run`, `screenshot` (like adb); the security model |
| [`HOST-FILES.md`](HOST-FILES.md) | Window mode: drag and drop, shared folders, saving to the computer |
| [`BROWSER-AGENT.md`](BROWSER-AGENT.md) · [`WEB-RESEARCH.md`](WEB-RESEARCH.md) · [`FULL-BROWSER.md`](FULL-BROWSER.md) · [`BROWSER-ROADMAP.md`](BROWSER-ROADMAP.md) | The assistant in the browser, web research, the optional full web engine, what the browser cannot do |
| [`EXTERNAL-AGENTS.md`](EXTERNAL-AGENTS.md) | Claude Code, GitHub Copilot CLI and custom agents; terms notes |
| [`PARALLEL-TASKS.md`](PARALLEL-TASKS.md) · [`K3S-DEV.md`](K3S-DEV.md) | Parallel tasks in the IDE; building and deploying to k3s |
| [`CLI-SSH.md`](CLI-SSH.md) | The SSH client in the CLI tile: which client, host keys, vault keys, the Connections tab, what the assistant may do |
| [`JOBS.md`](JOBS.md) | Detached jobs: long commands that outlive the chat, the Jobs strip, `job_start` for the assistant and Claude Code |
| [`MACOS.md`](MACOS.md) · [`MAC-HARDWARE.md`](MAC-HARDWARE.md) | IYAKKI on a Mac: window, VM, Intel Mac hardware, driver packs |
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
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | One-page overview, crate map, boot flow, security model, where to extend |
| [`../BOUNDARY.md`](../BOUNDARY.md) | Every place our code touches the base OS; the rules |
| [`../FEATURES.md`](../FEATURES.md) | The feature matrix: status, progress and tests per feature |

## Design and deep dives

[`DESIGN.md`](DESIGN.md) (visual language) · [`EMBEDDING-AND-BACKEND.md`](EMBEDDING-AND-BACKEND.md) (components and a remote backend) ·
[`ASSISTANT-ROBUSTNESS.md`](ASSISTANT-ROBUSTNESS.md) (tool use, thinking models, speed) · [`WEBVIEW-PROTOCOL.md`](WEBVIEW-PROTOCOL.md) (shell to web-engine
helper) · [`JS-PERFORMANCE.md`](JS-PERFORMANCE.md) · [`BOOT-TEST.md`](BOOT-TEST.md) and [`LOCAL-TEST.md`](LOCAL-TEST.md) (testing on this computer and on real
hardware) · [`../Agent OS Architecture Specification.md`](<../Agent OS Architecture Specification.md>) (the original design and roadmap).

## Release and legal

| Document | Read it for |
|---|---|
| [`../CHANGELOG.md`](../CHANGELOG.md) · [`../RELEASE-NOTES-0.1.0-alpha.1.md`](../RELEASE-NOTES-0.1.0-alpha.1.md) | What is in this release |
| [`BUILD-INCREMENTAL.md`](BUILD-INCREMENTAL.md) | Build only what changed: change detection, repack, layered macweb, shared caches |
| [`RELEASING.md`](RELEASING.md) | How a release is made, signed and checked |
| [`../SUPPORT.md`](../SUPPORT.md) | Getting help |
| [`../LICENSE`](../LICENSE) · [`../NOTICE`](../NOTICE) · [`../LICENSING.md`](../LICENSING.md) · [`LICENSES.md`](LICENSES.md) · [`../THIRD_PARTY_LICENSES.txt`](../THIRD_PARTY_LICENSES.txt) | Licences and third-party notices |
| [`BETA-SCOPE.md`](BETA-SCOPE.md) · [`BETA-PLAN.md`](BETA-PLAN.md) | Every non-done feature row classified for the beta; the plan, risks and owner decisions |
| [ALPHA-2-PLAN.md](ALPHA-2-PLAN.md) | What goes into 0.1.0-alpha.2: PC image and GPU, power settings, multi-repo clone, Wi-Fi/Bluetooth, fixes |
