# IYAKKI administrator guide

**For:** whoever sets up and looks after an IYAKKI machine (window, USB/installed, VM or container).
**You will get:** how accounts, policy, secrets, models and agents are controlled, how to verify the audit log, where
everything lives, which ports are open, a hardening checklist and the recovery steps.

Contents: [Accounts, roles and policy](#accounts-roles-and-policy) · [Vault and credentials](#the-vault-and-credentials) ·
[Models and providers](#models-and-providers) · [External agents](#external-agents-claude-code-copilot) ·
[Audit log](#the-audit-log-and-its-hash-chain) · [Data locations, backup](#data-locations-backup-and-restore) · [Updating](#updating) ·
[Network exposure](#network-exposure) · [Hardening](#hardening-checklist) · [k3s](#running-in-k3s-and-the-dev-cluster) ·
[Store keys](#store-trust-keys) · [Logs](#logs-and-diagnostics) · [Forgotten admin password](#forgotten-admin-password)

## Accounts, roles and policy

The first start asks for the first administrator (name: lowercase letters, digits, `-`, `_`; password 8+ characters).
Passwords are hashed with Argon2. After 3 wrong tries for a name, sign-in is delayed (1, 2, 4... up to 60 seconds).
Without a screen (containers) create accounts on the command line:

```
echo 'a long password' | iyakki user add alice --role operator --password-stdin
iyakki user add admin --role admin --if-missing --password-file /secret/password
iyakki user list
```

Passwords are never accepted as arguments. `IYAKKI_USERS=/path/users.json` points the command at another account file;
the default is `/var/lib/iyakki/users.json`.

**Roles** (templates in `crates/iyakki-sec/src/policy.rs`):

| Role | Tiles | Terminal | Files read / write | Network hosts | Agent budget |
|---|---|---|---|---|---|
| admin | all (USERS, SETTINGS...) | full | `/` | any | large |
| operator | all except USERS | full | `{home}`, `/shared` | any | large |
| user | no USERS, MESH, NET SCAN | restricted | read `{home}`, `/shared`; write `{home}` | any | standard |
| guest | CHAT, SRE PILOT, INBOX | none | none | none | tiny |

Only admins manage users and system settings; an override can never give a non-admin that power. Budgets are presets (tiny 20k
tokens, small 60k, standard 200k, large 1M, unlimited).

**USERS tile** (admins): select a person, then keys: `n` new, `d` delete (the last admin cannot be removed), `r` cycle role,
`p` reset password (resets their personal vault too), `1-4` toggle CHAT, RAG, MCP, CLI, `a` agent, `v` voice, `k` skills,
`b` budget preset, `w` browser agent (off, read-only, interactive), `f` fuel, `m` memory, `q` disk quota, `h` allowed network
hosts (comma list, `*` = all), `s` allowed MCP servers, `l` audit log. Every change is audited.

**File scopes** are part of the policy (`fs_read` / `fs_write`; `{home}` is `/home/<user>`). The USERS tile does not edit
them in this build; to narrow or widen them for one person, stop IYAKKI and add `"overrides": {"fs_read": [...],
"fs_write": [...]}` to that account in `users.json` (the file is mode 0600). Overrides refine the role template and cannot
escalate. Window mode confines host files to the host home folder ([`HOST-FILES.md`](HOST-FILES.md)).

The assistant always runs with its person's policy; risky tools ask first ([`USER-GUIDE.md`](USER-GUIDE.md#approvals-ask-first-auto-approve-safe-autopilot)).

## The vault and credentials

* Secrets live in encrypted vault files: the **system vault** (`<data>/vault/system.afv`, shared by admins, holds provider API keys)
  and one **personal vault** per account (`<data>/vault/users/<name>.afv`). Encryption is XChaCha20-Poly1305 with a random data
  key wrapped per key slot with Argon2id; a personal vault opens with the login password, so resetting someone's password
  resets their personal vault (the admin cannot read it). The `.afv` extension is a historical file name.
* Settings and tools hold only **references**: `vault:NAME`, `uvault:NAME`, `env:NAME`, `file:/abs/path`, or an external
  provider (`ext:ALIAS/path#field`, HashiCorp Vault/OpenBao, 1Password Connect: Settings -> Secret providers).
* The assistant never asks for a secret in the chat: it calls `vault_request`, which opens a masked form that stores the
  value in your vault and tells the assistant only the reference. A pasted key-like string is not sent to the model.
* If sign-in cannot open the vault the person is told (and an INBOX warning is added); model keys stay unavailable until an
  admin with a key slot signs in.
* Rules: never put keys in `ai.conf` or `mcp.json` literally; use references. Keep a backup of the vault files separate from
  the passwords. Lost password = lost vault (there is no recovery key).

## Models and providers

Settings -> Voice (`s v c m p t y`): the engines found on the computer with their voices and why an engine cannot be used, the engine, voice and speed choosers, a Test button, Re-check, and the **browser voice helper** for the window ([`VOICE.md`](VOICE.md)). Reading aloud needs no setup when an engine is already installed; the speech engines are not shipped in the images (GPL).

Settings -> Models (`a` add, `e` edit, `d` delete, `t` test, `m` chat model, `b` embeddings model, `k` key, `x` on/off,
`c` test chat). Kinds: Ollama, OpenAI, OpenRouter, Anthropic Claude, Google Gemini, OpenAI-compatible (any server that
speaks the OpenAI chat API; address required). Cloud kinds need a key (stored in the vault). NET SCAN adds an Ollama it
finds in one step. Older `ai.conf` `chat.url` / `chat.model` lines are migrated to a provider named `legacy`. Token usage is charted in
USAGE (the provider's own counts when reported, otherwise about four characters per token). Ollama can be installed and
started from the CLI tile (listens on 127.0.0.1 only, models under the data folder).

## External agents (Claude Code, Copilot)

Settings -> Agents (`d` detect, `i` install, `l` sign in, `k` API key, `m` model, `a` Auto, `u` use for new chats, `t` test,
`n` custom command). IYAKKI starts the vendor's own program, never touches the vendor's login, passes an API key only as one
environment variable read from the vault, and routes the agent's tool calls through the same approvals via a per-run socket.
Claude Code can drive Autopilot; Copilot and custom commands run as read-only advisors. Auto for an agent is off until you allow
it. **Read the terms-of-service notes before offering this to other people:**
[`EXTERNAL-AGENTS.md`](EXTERNAL-AGENTS.md#terms-of-service-decision-for-the-owner) (do not bundle the vendor binary in an image;
subscription limits assume individual use). No live run against the real vendor programs has been made yet.

## The audit log and its hash chain

Every sign-in, app open, denied action, account change, tool decision, screenshot and installs is appended to `audit.log`
(`/var/lib/iyakki/audit.log`; window mode `~/.local/share/iyakki-dev/audit.log`; mode 0600). Each line is JSON with `ts`, `user`,
`event`, `detail`, `prev` and `hash`, where `hash` is the SHA-256 of `ts|user|event|detail|prev` and `prev` is the previous line's
hash (64 zeros for the first). Every append takes an exclusive file lock (`flock`) on `audit.log`, re-reads the last line under it, writes the line and flushes it to disk before letting go, so the shell, the core and the command line can write at the same time without two lines sharing a `prev`; a line cut short by a power loss is skipped and the next line chains to the last whole one. **Verify in the app:** USERS -> `l` shows "Audit log - chain intact (N entries)" or "BROKEN at entry I".
**Verify outside IYAKKI** (any machine with Python 3):

```python
import hashlib, json, sys
prev, n = "0" * 64, 0
for i, line in enumerate(open(sys.argv[1], encoding="utf-8")):
    if line.strip():
        e = json.loads(line)
        h = hashlib.sha256(f'{e["ts"]}|{e["user"]}|{e["event"]}|{e["detail"]}|{e["prev"]}'.encode()).hexdigest()
        if e["prev"] != prev or h != e["hash"]:
            sys.exit(f"BROKEN at entry {i}")
        prev = e["hash"]; n += 1
print(f"intact ({n} entries)")
```

A clock that is corrected while the system runs (the network time arrives about 30 s after boot on a computer with no usable hardware clock, or the clock floor lifts it at the start) does not break the chain: `ts` is part of the hashed text, not an ordering the check relies on, so two neighbouring entries may show a big step (or, after a manual set-back, a step backwards) in `ts` and still verify. Entries before the first correction carry the floor time (`docs/K3S-DEV.md`, "The clock at boot").

### What a BROKEN report means now

* **Fixed for one process.** Inside one running IYAKKI (the window, the console shell or `iyakki web`), every write takes a lock: it reads the
  last hash and appends in one step, so two events in the same second can no longer share a `prev`. The old race ("BROKEN at entry N
  but nobody touched it") should not appear in a log that was written only by fixed versions.
* **Old chains.** A chain that already contains the race stays BROKEN at that entry for good: the code never rewrites or repairs
  `audit.log` (that would defeat the point); you start a new one (below). Several IYAKKI processes on one data folder (a window and
  `iyakki web`, a script and a shell) now share one exclusive file lock (`flock`) on `audit.log`, so they no longer collide.
* **How to read the report.** "BROKEN at entry I" counts from 0. Look at lines I-1 and I of `audit.log`. If both have the **same `prev`** and
  timestamps within a second or two, it is the old race (or two processes); the entries themselves are fine. If a line was edited, removed or
  reordered, `hash` or `prev` no longer matches and nothing explains it by timing: treat that as tampering, keep a copy of the file, and
  check who had access to the data folder. The chain proves order and integrity, not who wrote it from outside IYAKKI.
* **Starting a new chain.** In USERS, open the audit log (`l`) and press a capital `N` while it reads BROKEN: the old file is kept beside it as
  `audit.log.broken-<time>` (never deleted) and the new chain begins with an `audit.restart` line that names it. By hand, the same is:
  sign everyone out and stop IYAKKI, then **move** `audit.log` aside (do not delete it; it is your evidence) with
  `mv /var/lib/iyakki/audit.log /var/lib/iyakki/audit.log.broken-$(date +%F)` (window mode:
  `~/.local/share/iyakki-dev/audit.log`), and start IYAKKI. With no file the next event begins a fresh chain (the first line has 64 zeros as
  `prev`) and USERS -> `l` shows "chain intact" again. Keep the old file off the machine with your backups; it still verifies up to the
  broken entry with the script above. The first entry of the new log is a normal sign-in, so note in your own records why the log was
  restarted.

Run records (RUNS) use a separate ledger and `iyakki ledger-verify FILE`.

## Models and providers

Settings -> Models (`a` add, `e` edit, `d` delete, `t` test, `m` chat model, `b` embeddings model, `k` key, `x` on/off,
`c` test chat). Kinds: Ollama, OpenAI, OpenRouter, Anthropic Claude, Google Gemini, OpenAI-compatible (any server that
speaks the OpenAI chat API; address required). Cloud kinds need a key (stored in the vault). NET SCAN adds an Ollama it
finds in one step. Older `ai.conf` `chat.url` / `chat.model` lines are migrated to a provider named `legacy`. Token usage is charted in
USAGE (the provider's own counts when reported, otherwise about four characters per token). Ollama can be installed and
started from the CLI tile (listens on 127.0.0.1 only, models under the data folder).

### Lemonade (AMD Ryzen AI)

[Lemonade Server](https://github.com/lemonade-sdk/lemonade) is AMD's local OpenAI-compatible server for Ryzen AI machines (NPU,
iGPU, CPU). It serves chat models and also picture models (Stable Diffusion: SD-Turbo, SDXL), whisper (speech-to-text) and Kokoro
(text-to-speech). IYAKKI has a provider type **Lemonade** for it.

* **Finding it.** At an administrator's sign-in, and with **Detect** (`f`) on Settings -> Models, IYAKKI asks this machine's
  Lemonade ports (13305, the default since Lemonade v10.1.0, then 8000, the default before) for `GET /api/v1/health`, with short
  timeouts, together with Ollama's 11434. A Lemonade that answers is added as **lemonade-local** with its address
  (`http://127.0.0.1:13305/api/v1`) and the page says "Lemonade found at ... (version ...)". A provider that already points at that
  server is never changed, and neither is one named lemonade-local that points elsewhere (the new one is then
  `lemonade-local-<port>`). NET SCAN recognises Lemonade on the network by the same health answer (the same administrator consent
  as Ollama) and adds it as a Lemonade provider.
* **Add form.** The Type choice has **Lemonade** (default address `http://localhost:13305/api/v1`) and **Auto-detect (Ollama,
  Lemonade or OpenAI-compatible)**: give the address of any server and IYAKKI asks it (GET requests only) what it is, then saves it
  with the right type and address. A key typed in the form is used for the questions and stored in the vault (Lemonade only needs
  one when it was started with `LEMONADE_API_KEY`).
* **Models by type.** Lemonade's model list (`/api/v1/models?show_all=true`) labels each model; IYAKKI sorts them into Chat,
  Vision, Embeddings, Reranking, Image, Speech-to-text, Text-to-speech and **Video: none (Lemonade has no video models; no video
  endpoint exists in Lemonade today)**. Choose the Lemonade provider on Settings -> Models to see the sections, with each model's
  state (loaded, available, or not downloaded with its size). Only downloaded chat and vision models appear in the chat model
  pickers; vision models are also marked as able to see pictures (the FLOWS screenshot-to-model step accepts them).
* **Use for...** Click a model (or `l` for the full list) and choose what fits its type: *Use as chat model*, *Use for embeddings
  (RAG)*, *Use as TALES picture model* (your own TALES preference), *Use as reading voice...* (a Kokoro voice; TALES and chat
  replies are then read with it), *Use as speech-to-text*.
* **Load and Pull.** Lemonade loads a model by itself on its first use. *Load into memory...* and *Pull (download ...)* ask first on
  a card naming the model, its size and the server; nothing is downloaded silently. A pull shows its progress on the page.
* **Pictures.** `generate_image` and TALES call Lemonade's `/api/v1/images/generations` at the model's own size and steps
  (SD-Turbo 512x512 in 4 steps, SDXL 1024x1024 in 20) and keep the PNG only after the usual magic-byte and decode checks. SDXL on a
  CPU can take minutes; the request waits up to the generation timeout (5 minutes).

## External agents (Claude Code, Copilot)

Settings -> Agents (`d` detect, `i` install, `l` sign in, `k` API key, `m` model, `a` Auto, `u` use for new chats, `t` test,
`n` custom command). IYAKKI starts the vendor's own program, never touches the vendor's login, passes an API key only as one
environment variable read from the vault, and routes the agent's tool calls through the same approvals via a per-run socket.
Claude Code can drive Autopilot; Copilot and custom commands run as read-only advisors. Auto for an agent is off until you allow
it. **Read the terms-of-service notes before offering this to other people:**
[`EXTERNAL-AGENTS.md`](EXTERNAL-AGENTS.md#terms-of-service-decision-for-the-owner) (do not bundle the vendor binary in an image;
subscription limits assume individual use). No live run against the real vendor programs has been made yet.

## The audit log and its hash chain

Every sign-in, app open, denied action, account change, tool decision, screenshot and installs is appended to `audit.log`
(`/var/lib/iyakki/audit.log`; window mode `~/.local/share/iyakki-dev/audit.log`; mode 0600). Each line is JSON with `ts`, `user`,
`event`, `detail`, `prev` and `hash`, where `hash` is the SHA-256 of `ts|user|event|detail|prev` and `prev` is the previous line's
hash (64 zeros for the first). **Verify in the app:** USERS -> `l` shows "Audit log - chain intact (N entries)" or "BROKEN at entry I".
**Verify outside IYAKKI** (any machine with Python 3):

```python
import hashlib, json, sys
prev, n = "0" * 64, 0
for i, line in enumerate(open(sys.argv[1], encoding="utf-8")):
    if line.strip():
        e = json.loads(line)
        h = hashlib.sha256(f'{e["ts"]}|{e["user"]}|{e["event"]}|{e["detail"]}|{e["prev"]}'.encode()).hexdigest()
        if e["prev"] != prev or h != e["hash"]:
            sys.exit(f"BROKEN at entry {i}")
        prev = e["hash"]; n += 1
print(f"intact ({n} entries)")
```

> **Known alpha bug.** When two events are written in the same instant from different threads, two lines can end up with the
> same `prev`, and the verifier then reports "BROKEN" although nothing was tampered with (seen on the development machine's own
> log, at an entry written in the same second as its neighbour). A BROKEN report therefore needs a look at the lines around the
> reported entry: two lines with the same `prev` and consecutive timestamps are this race, not an attack. Run records (RUNS) use
> a separate ledger and `iyakki ledger-verify FILE`.

## Data locations, backup and restore

| What | Installed image | Window mode |
|---|---|---|
| Data folder | `/var/lib/iyakki` (the IYAKKI-DATA volume) | `~/.local/share/iyakki-dev` (`IYAKKI_DEV_DIR`) |
| Accounts | `users.json` | same |
| Audit log | `audit.log` | same |
| Vaults | `vault/system.afv`, `vault/users/` | same |
| Settings, chats, runs, documents, skills | `settings.json`, `sessions/`, `runs/`, `docs/`, `skills/`, `kb/` ... | same |
| Read-only config | `/etc/iyakki/` (`ai.conf`, `mcp.json`, `net.conf`, `authorized_keys`) | `ai.conf` in the data folder |
| Browser front end certificate | `tls/` in the data folder | n/a |
| Crash log | `crash.log` in the data folder (window) | |
| Dev cluster | `k3s-dev/` in the data folder | |
| People's home folders (`/home/<name>`: Pictures, Videos, Stories, Workspaces, tool logins such as `~/.claude`) | `home/` in the data folder, bind-mounted onto `/home` at boot (log line `home: /home is now on the data volume`); each folder is made on first use, owned by the account, mode 0700. Without a disk data volume `/home` stays in RAM and is lost at power-off. `/root` (the image's SSH keys) is not on the volume | `/home/<name>` or `$HOME` |

**Backup:** stop IYAKKI (or at least sign everyone out), copy the whole data folder (mount the IYAKKI-DATA ext4 partition on
another Linux machine), keep the copy private (only the vaults are encrypted). **Restore:** copy it back with the same owner and
modes (`users.json` and `audit.log` are 0600). Containers: back up the `iyakki-data` volume/PVC.

## Updating

**Apps, skills, MCP servers, command-line tools and drivers** (not IYAKKI itself) are updated from STORE: *Check* looks for news, *Update*
and *Update all* apply it, *Go back* returns to the previous version (one is kept), *Pin* stops updates for one item. The new version
is tested after installing (an MCP server must start and answer within 10 seconds) and the old one is put back if it fails; an update
that asks for more permissions is never applied without a person confirming it; an update from a different publisher is refused. Every
change is in the audit log (`store.update`, `store.rollback`, `store.remove`) and in `store/lifecycle.log`. Terminal: `store list
--outdated`, `store update <id>|--all`, `store rollback <id>`, `store remove <id>`. The assistant's `store_update`, `store_remove` and
`store_rollback` always ask. Platform services keep their own plan-and-approve flow. Details: [`STORE.md`](STORE.md) section 15.

IYAKKI itself: there is no in-place updater in the alpha. Window mode: pull and rebuild. Installed stick/disk: back up the data folder, write the
new install image with `iyakki-usb` (erases the drive), restore the data folder ([`INSTALL.md`](INSTALL.md#7-upgrade-back-up-uninstall-recover)).
Containers: roll the image. Verify downloads with `iyakki release verify DIR --pubkey HEX` (checks `SHA256SUMS` and the ed25519
signature). Release process: [`RELEASING.md`](RELEASING.md).

## Private MAC address

Each Ethernet and Wi-Fi adapter has a mode: `off` (hardware address), `per-network` (a stable made-up address per Wi-Fi network, or per
interface for Ethernet) or `random` (a new one every time). Defaults: Ethernet `off`, Wi-Fi `per-network`. Settings -> Network (`m`, `a`)
and `iyakki net mac` change it; everyone can read it (`iyakki net mac`, `iyakki net`, the assistant's network tool, the System tile).

```
iyakki net mac                                  # show: mode, hardware address, address in use, locally administered
iyakki net mac off --ethernet                   # root: choose a mode for Ethernet / Wi-Fi / one interface
iyakki net mac random --wifi
iyakki net mac per-network --iface wlan1
iyakki net mac apply --iface eno1 [--yes]       # root: change it now; --yes is needed when that adapter carries your SSH session
```

`/etc/iyakki/net.conf` keys (old files without them keep working; a bad `mac.` line is skipped, never a reason for the network not to
come up). When `/etc/iyakki` is read-only the same lines are kept in `/var/lib/iyakki/net-mac.conf`, which wins:

```
mac.ethernet = off              # off | per-network | random
mac.wifi = per-network
mac.iface.wlan1 = random        # one adapter (beats the kind)
mac.ssid.Work Net = off         # one Wi-Fi network (beats the adapter)
```

How it works. At start-up, before the link comes up and before the supplicant and DHCP client start, the address is set with
`SIOCSIFHWADDR` (link down; checked first: unicast, not all zeros; read back; the hardware address is put back if the change fails).
"Per network" is `HMAC-SHA1(device secret, kind + hardware address + SSID or interface)` with the first byte forced to
locally administered, unicast; the device secret is 32 random bytes in `/var/lib/iyakki/mac-secret` (owner-only), so the address is
stable per network and device, differs between networks and between devices, and is not linkable without the secret (without a
data volume the secret lasts one boot). For Wi-Fi a private mode also turns on the supplicant's random addresses for scans
(`preassoc_mac_addr=1`, `gas_rand_mac_addr=1`); drivers that cannot do that carry on. The address of the Wi-Fi adapter at start-up is
made for the newest saved network; joining another network from Settings sets that network's address first. If the machine roams to
another saved network by itself it keeps the address it has until the next join, restart or `iyakki net mac apply`.

DHCP: the client (`udhcpc -b -R -i IFACE`) never sends a host name (`-x hostname`, `-F`, `-H` are not used in any mode) and its client
identifier is the adapter's address at that moment, which is the private one, because the address is set before the client starts
and the client is restarted after a change. So nothing hands the router the hardware address when the mode is not `off`.
DHCP reservations and router rules written for the hardware address need `off` (or the stable address shown for that network).

Limits. Drivers can refuse (reported as "not supported by this driver"). The address is only changed with the link down, so a cable
being plugged in does not give a new "random" address; start-up, joining, switching the adapter on in Settings and `apply` do.
In the desktop window the host computer owns the addresses and this is read-only.

## Network exposure

| Port | Who | When | Notes |
|---|---|---|---|
| none | the shell on screen / window | always | no listener |
| 8443/tcp HTTPS (+WebSocket) | `iyakki web`, `iyakki serve --web`, `IYAKKI_WEB=1`, boot option `iyakki.web=1` | only if you start it | the desktop in a browser; people sign in with this machine's accounts; `http:8080` form speaks plain HTTP for a TLS proxy |
| 7979/tcp TLS | developer mode (Settings → Developer mode) | only while an administrator has it on (automatic switch-off, 4 h by default) | paired computers only: one-time pairing code, then every request signed by the computer's ed25519 key; installs are labelled Developer (local); revoke keys and read the activity log on the same page ([`DEVELOPER-MODE.md`](DEVELOPER-MODE.md)) |
| 22/tcp SSH (dropbear) | boot option `iyakki.ssh=1` | only on that boot entry | keys only, no passwords; needs `/etc/iyakki/authorized_keys` baked in at build (`IYAKKI_SSH_AUTHORIZED_KEYS=<file>`) |
| 11434 (loopback) | Ollama started from the CLI tile | when started | 127.0.0.1 only |
| 6443/tcp, 10250, 8472/udp, NodePorts 30000-32767 | the dev cluster IYAKKI installs (bare metal) | when installed | API server on every interface with TLS and a token |
| mDNS + libp2p | `iyakki serve --swarm` | only if you start it | LAN peers; runs WebAssembly tasks with no capabilities |

**Remote front end (`iyakki-remote`):** certificate is made on first start (self-signed; the browser warns once; the SHA-256
fingerprint is shown on the warning page). Put your own `cert.pem` and `key.pem` in the `tls/` folder of the data folder to avoid
that. Sessions: random cookie plus CSRF token, 8 hours idle limit, at most 32 sessions, 128 connections, 10 sign-in tries
per address (rate-limited). Features that need hardware act on the machine running IYAKKI, not the browser's. Serve it only on
networks you trust or behind a TLS-terminating proxy with its own access control.

## Hardening checklist

1. Strong, unique admin password; create everyday accounts as `user` or `operator`, not `admin`.
2. Leave Secure Boot expectations in mind: images are not signed, so on machines you must protect, rely on physical control.
3. Boot entries: remove or never use the SSH entry unless you built keys in; do not leave `iyakki.web=1` on untrusted networks.
4. Use your own certificate for the browser front end; or bind it to a proxy (`http:PORT`) with authentication.
5. Narrow `net_hosts` per person (USERS -> `h`) instead of `*`; narrow MCP servers (`s`); keep the browser agent read-only unless needed (`w`).
6. Keep Auto-approve safe off for agents you do not fully trust; remember the hard-ask list always asks.
7. Keep model API keys in the vault; use `uvault:` for personal keys.
8. Verify the audit chain periodically (see above) and back up `audit.log` off the machine.
9. Do not install store packages below Trusted unless you read the review; add only publisher keys you control.
9a. Leave developer mode off unless someone is developing on this device; keep its automatic switch-off; revoke computers you no longer use.
10. Back up the data folder; keep vault backups separate from passwords.
11. Containers: keep the manifest's non-root, read-only-root, all-capabilities-dropped settings.
12. Read [`../SECURITY.md`](../SECURITY.md) for how to report vulnerabilities.

## Running in k3s and the dev cluster

**IYAKKI in k3s (browser front end):** `dist/k3s-web.yaml` is a Deployment (one replica, `Recreate`, non-root 65532, read-only
root, all capabilities dropped, seccomp), a 2 Gi PVC for `/var/lib/iyakki`, an init container that adds the first account from a
Secret, and a Service on 8443. It names the image `iyakki:0.1.0`; build with `docker build -f dist/Dockerfile -t iyakki:0.1.0 .`
and load or push it:

```
kubectl create namespace iyakki-web
kubectl -n iyakki-web create secret generic iyakki-admin --from-literal=password='a long password'
kubectl apply -f dist/k3s-web.yaml
kubectl -n iyakki-web port-forward svc/iyakki 8443:8443     # then https://localhost:8443
```

Behind an Ingress that does TLS, set `IYAKKI_WEB` to `http:8080`. `dist/k3s-manifest.yaml` is the headless supervisor DaemonSet
(swarm). Tried on a local K3s, not on a managed cloud.

**Dev cluster (AI IDE deploys):** Settings -> Dev cluster (administrators) manages cluster profiles (this machine's k3s,
`$KUBECONFIG`, `~/.kube/config`, the pod's service account, vault-held kubeconfigs), the registry, and on bare metal "Set up
a local cluster" (shows a plan with URL, checksum, ports; you approve). IYAKKI never starts a second cluster. The workspace's
cluster is the one chosen in the IDE, not `$KUBECONFIG` by inheritance. Full detail and the real-hardware checklist:
[`K3S-DEV.md`](K3S-DEV.md).

**Platform services (Gitea, the private store).** Settings -> Dev cluster -> **Platform services** (button, key `P`; also the STORE card
"Private store" and the AI IDE's K8S panel, *Services...*) deploys a service into a cluster *you choose*, never silently. The wizard lists
every cluster profile with badges (reachable, k3s, this machine or remote, production-looking, read-only); production-looking, read-only
and unreachable ones are refused with the reason. You read the exact plan (commands, images, storage, what it exposes, the credentials it
creates by name, an example NetworkPolicy that is not applied), approve it, watch progress, and get the addresses and a masked one-time
admin password (Show, Copy, Done). Afterwards the list shows live state and Open, Stop/Start, Upgrade, Backup, Details, Connect to existing,
Connect workspace and Remove (the typed id is asked). The assistant has `platform_list`, `platform_status`, `git_service_status` and
`platform_deploy`, `platform_upgrade`, `platform_remove`; the last three always ask, in every mode, and name the server. Tokens and
admin passwords live in the vault. Tested with fakes only, never on a real cluster: [`PLATFORM-SERVICES.md`](PLATFORM-SERVICES.md).

**Private store.** `iyakki store-server` (or the `private-store` platform service) serves a signed catalog; publishers get per-publisher
bearer tokens (kept hashed), and every package is verified on upload. You give people the catalog key fingerprint; they add the store
(STORE -> Settings -> Add a store, or the Private store card) and press "They match" to pin it. A yank hides a package but does not disable
copies already installed. Steps and limits: [`STORE.md`](STORE.md) sections 13 and 14, making packages: [`APP-AUTHORING.md`](APP-AUTHORING.md).

## Store trust keys

Packages get a trust level from keys on the device: Official (a pinned IYAKKI root; none is pinned yet, so only built-in items are
Official), Verified (certified by a root), **Trusted** (a key you add), Community, Unsigned, Revoked. As administrator:
**STORE -> Settings**: `a` add a store (a portal page ending in `/` or a `catalog.json` link; its name, signer fingerprint and trust are shown and you confirm before it is enabled; up to 8; `w` the official store, "coming soon" until its address is published), `f` add a folder or USB stick, `o` daily check (off by default, no telemetry), `i` install
from a file, `t` trust a publisher key. SKILLS -> `k` lists trusted publishers. Set up: generate a key with `iyakki pkg keygen NAME`
(publish the `.pub`, guard the `.key`), sign packages with `iyakki pkg build ... --key` or `iyakki pkg sign`, and add the public key
on each machine. Installing for everyone needs `manage_skills`; MCP servers and unsigned packages need an admin. Driver packs are
administrator-only and whole-machine ([`STORE.md`](STORE.md)).

## Logs and diagnostics

* Boot log: press `L` on the boot screen, use the "show boot messages" entry, `dmesg`, or the serial console (VM:
  `/tmp/iyakki-vm-serial.log`). Hardware check entry or `iyakki hwreport` for devices.
* `IYAKKI_LOG=info|debug` sets the log level of the program; in containers read `kubectl logs`.
* SYSTEM tile (load, processes, agents, hardware); `iyakki top`; `iyakki storage` (where data lives, which disks could hold it).
* Tool calls are logged for three days (`tools.jsonl`); runs are in RUNS.
* **Diagnostics over SSH:** build the image with your public key (`IYAKKI_SSH_AUTHORIZED_KEYS=~/.ssh/id_ed25519.pub
  dist/buildroot/build.sh libre`), boot the "remote diagnostics" entry (`iyakki.ssh=1`), the address is shown on the sign-in screen,
  then `ssh root@ADDRESS` with the key. In QEMU: `./run.sh vm-direct --ssh` forwards 127.0.0.1:2222 (the example key path in `./run.sh help` is `~/.ssh/iyakki_diag`; use whichever key you built in).
* Window mode crashes: `crash.log` in the data folder (what to send: the last entry and the output of `iyakki-desktop` run from a terminal).

## Forgotten admin password

There is no recovery key and no reset of your own password without the old one, but anyone with access to the machine's data
folder can add another administrator from the command line (verified in `iyakki user add`):

1. Reach a shell on the machine: SSH diagnostics, a window-mode terminal, or boot the data volume on another Linux machine.
2. `echo 'new long password' | iyakki user add rescue --role admin --password-stdin` (with `IYAKKI_USERS=/path/to/users.json`
   when the file is not at the default location; the command refuses an existing name).
3. Sign in as `rescue`, open USERS, select the old admin and press `p` to set a new password. This also resets that admin's
   **personal vault** (its contents are lost; the system vault is unaffected if another admin still has a key slot).
4. Delete `rescue` if you no longer want it.

If nobody has command-line access (live ISO, nothing persisted), reboot: a fresh live session asks for a new administrator.
