# Frequently asked questions

**For:** people deciding whether and how to use IYAKKI. Problems with a fix are in [`TROUBLESHOOTING.md`](TROUBLESHOOTING.md).
**You will get:** short, honest answers.

**What is IYAKKI?** An operating environment written in Rust and WebAssembly around an AI assistant that is allowed to *do* things
(read files, call APIs, run commands, deploy to Kubernetes) safely: every action passes one permission model, risky ones ask
first, untrusted code runs in a sandbox, and everything is audited. See [`../README.md`](../README.md).

**Is it an operating system or an app?** Both. The same program runs as PID 1 on a USB-booted PC or VM, as a desktop window on Linux,
and as a container. It is not a Linux fork: it sits on a stock kernel and a few ordinary programs ([`../BOUNDARY.md`](../BOUNDARY.md)).

**Is it ready to use?** It is an **alpha** (0.1.0-alpha.1). The window mode and QEMU paths work; nothing has been booted on real
hardware yet; many features are partial ([`../FEATURES.md`](../FEATURES.md), [`../KNOWN-ISSUES.md`](../KNOWN-ISSUES.md)). Do not put
data on it that you cannot lose, and keep backups.

**Does it send my data anywhere?** Only to the models and services you configure (Ollama on your network, or a cloud provider with
your key). No telemetry. The optional store catalog check is off by default and sends a plain GET with no identifiers.

**Which models work?** Ollama models, any OpenAI-compatible server, OpenAI, OpenRouter, Anthropic Claude and Google Gemini. Live calls
to the cloud providers have not been exercised in testing (they need keys). Claude Code and GitHub Copilot CLI can also answer the chat
if you install and sign in to them yourself ([`EXTERNAL-AGENTS.md`](EXTERNAL-AGENTS.md)).

**Can I use it without a GPU or internet?** Yes for most tiles (the built-in catalog, RADIO excepted). Models need a server you can
reach; small models run on CPU in Ollama.

**What can the assistant do without asking?** Read-only things: read files you may read, search your documents, look things up on
the web, weather and prices, read calendars. Anything that writes, runs, installs, deletes or spends asks first. Some actions ask
even in Auto mode ([`USER-GUIDE.md`](USER-GUIDE.md#approvals-ask-first-auto-approve-safe-autopilot)).

**Can the assistant see my passwords?** No. It never asks for them in chat; it requests a masked form that stores the value in your
encrypted vault and returns only a reference. Pasted keys are not sent to the model.

**What is the difference between the three ways to run?** Window mode: fastest, uses your real network and files, shows virtual
disks only. USB/installed: the real OS, with a data partition for accounts and files. VM: the same without touching hardware
([`INSTALL.md`](INSTALL.md)).

**Does it run on a Mac?** Window mode is written for macOS but has not been tried on a Mac ([`MACOS.md`](MACOS.md)). Intel Macs can boot the USB image like a PC (the
Option key; untested). Apple silicon Macs cannot boot it; use a VM and run models in Ollama on the Mac ([`MAC-HARDWARE.md`](MAC-HARDWARE.md)).

**Does it run on Windows?** There is no Windows build. A VM (QEMU or others, untested) works in principle.

**Why is Wi-Fi not working?** The default image has no proprietary firmware. Use Ethernet or a supported USB adapter
([`TROUBLESHOOTING.md`](TROUBLESHOOTING.md#hardware-boot-and-network)).

**Where is my data?** `/var/lib/iyakki` on an installed system or `~/.local/share/iyakki-dev` in window mode. Back it up
([`ADMIN-GUIDE.md`](ADMIN-GUIDE.md#data-locations-backup-and-restore)).

**How do I add more people?** USERS tile (administrators) or `iyakki user add`. Roles: admin, operator, user, guest ([`ADMIN-GUIDE.md`](ADMIN-GUIDE.md)).

**Can I run it in the cloud?** The container image and `dist/k3s-web.yaml` serve the desktop to a browser. Tried in Docker and a
local K3s; not on a managed cloud.

**What is the license?** Apache-2.0 for our code; dependencies must be permissive (enforced by `cargo deny`). Images contain GPL
components as separate programs with source in `legal-info/`. Tools you install from the CLI tile keep their own licenses
([`../LICENSING.md`](../LICENSING.md)).

**How do I report a bug or a vulnerability?** [`../CONTRIBUTING.md`](../CONTRIBUTING.md#reporting-issues) and [`../SECURITY.md`](../SECURITY.md).

**Why the name?** IYAKKI is the project's name (renamed from an earlier working title). Some internal file names, such as the
vault's `.afv` extension, still carry the old one.
