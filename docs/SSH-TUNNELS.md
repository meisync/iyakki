# SSH tunnels: see a web page behind an SSH host

Settings -> Network -> SSH tunnels (administrators). For a page that only the inside of a network can reach, and an SSH host (a
"bastion") that is reachable from outside. You bring your own host and key; nothing is built in. Part of the same family as
[Git and SSH keys](GIT-AND-SSH.md): same key list, same known hosts, same safety rules.

## What a tunnel is

| Kind | What it does |
|---|---|
| One page (local forward) | `127.0.0.1:PORT` on this device reaches `HOST:PORT` as the SSH host sees it. Open `http://127.0.0.1:PORT/` in the Browser. |
| Any site (SOCKS proxy) | `127.0.0.1:PORT` is a SOCKS5 proxy to everything the SSH host's network reaches (`socks5h://127.0.0.1:PORT`: names are looked up on the far side). |

A tunnel has a name, the SSH host and port, the user, an optional chain of jump hosts before it (at most three), the name of a key and
an **enabled** flag. The list is one JSON file per person (`<data>/ssh/<user>/tunnels.json`, mode 0600). **No secret is in it**: the key
is a name from Settings -> Git and SSH keys (vault entry `ssh.<name>`).

## The page

A list with one row per tunnel and its own state: `connected 5 min`, `connecting`, `retrying`, `needs you`, `disabled`.
Buttons: **Add**, **Edit**, **Enable / Disable**, **Delete** (asks first), **Test**, **Retry**, **Copy address**, **Copy public key**.
Any number of tunnels can exist; each can be enabled or disabled on its own.

* **Add** starts disabled. Choose "Make a new key for this tunnel" (a key `tunnel-<name>` is made; its public half is copied so you can
  put it in the user's `authorized_keys` on the host) or pick one of your keys. Leave the local port empty for a free one.
* **Test** connects to every host of the route and shows its fingerprint. A host not yet trusted opens a form with the fingerprint and
  the answer **No** selected; **Yes, it matches** records exactly the key with that fingerprint. Then Test connects for real, waits for
  the local port to open and stops again.
* **Enable** starts the tunnel now and with the device (after sign-in: the key is in the vault). It reconnects after a drop with a pause
  of 2, 4, 8 ... 300 seconds. **Disable** stops it and keeps it.
* **Delete** stops it, removes the key made for it (a key you chose yourself stays) and removes the trusted-host line of its hosts
  unless another tunnel uses them. (A host that git also uses loses its trusted line too; git shows its fingerprint again next time.)

## Rules that cannot be switched off

* The program is the OpenSSH client the image carries for git (`/usr/libexec/iyakki/git-ssh`), started with a **fixed argument shape**
  (`sshtunnel::build_args`, checked again by `check_args`): `-N -T`, `-o` options from an allow-list, `-p`, `-J user@host:port,...`,
  exactly one of `-D 127.0.0.1:PORT` or `-L 127.0.0.1:PORT:HOST:PORT`, then `--` and `user@host`. Never `ProxyCommand`, `LocalCommand`,
  `Include`, `-F`, `-S`, `-R`, `-W`, `-i`, a file path typed by the person, a password, a bind to anything but 127.0.0.1, or a remote
  forward. Host names and users are letters, digits, `.`, `-`, `_`: no space, quote, control character, `-` first.
* `BatchMode=yes`, `StrictHostKeyChecking=yes`, `UserKnownHostsFile` = the person's own file, `ExitOnForwardFailure=yes`,
  keep-alives (15 s x 3), `ConnectTimeout=15`. The client runs with a cut-down environment; `HOME` is the private SSH home, and the
  tunnel refuses to start if a `.ssh/config` exists there.
* **The key never leaves memory**: the private half is read from the vault and served to the client by IYAKKI's in-memory ssh-agent
  (`SSH_AUTH_SOCK`), the same as git. It is on no command line and in no file.
* **Host keys** as in git: a host not in the known hosts is refused ("trust it first") with its fingerprint to confirm; a changed key
  is a hard refusal that Trust does not override (forget the old entry on purpose first).
* **Local ports**: 1024 and above, 127.0.0.1 only. A port another tunnel owns (enabled or not) or another program holds is reported, never
  reused silently. An empty port asks the operating system for a free one, and it is remembered.
* Errors of the client become plain words (host key changed, permission denied, connection refused, timed out, no route, unknown name,
  address in use, dropped). The raw text is not shown. A **hard** fault (not trusted, key changed, key refused, port in use, key
  unavailable) is not retried by itself; the person fixes it and presses Retry or toggles Enable.
* Audit lines (`sshtunnel.*`) carry names, fingerprints and states, never a value.

## The assistant

`ssh_tunnel_list` reads without a card. `ssh_tunnel_add`, `_edit`, `_enable`, `_disable` and `_delete` each ask on a card that names the
tunnel, host and port, user, kind, local port, jump chain and the vault entry that holds the key (delete says it removes the key made
for it and the trusted-host line). They are on the hard-ask list (`agent::hard_ask`): they ask every time, also in Auto mode, and "Allow
for this session" does not remember them. The assistant refers to a key by name only; text that looks like key material is refused and not
repeated; a missing key returns "ask the person to add it in Settings -> Git and SSH keys". A new host key is never trusted by a tool: the
result shows the fingerprint and says the person must confirm it here. Offered to administrators only (`iyakki-agent::sshtunneltools`).

## Sending a site through a tunnel (the seam; not wired yet)

`sshtunnel::SiteRouter::tunnel_for(url) -> Option<tunnel id>` is the one question the Browser will ask; `Manager::route(router, url)`
answers with the tunnel's address only when it is enabled and connected. The planned router is the VPN's hybrid filter (domain name,
URL, CIDR; `crates/iyakki-vpn` `FilterSet`) with a tunnel named per rule. Today `NoSiteRouter` says "direct" for everything: open the
address shown on the page.

## What is verified

With a fake client, key store, host-key probe and ports (no network, no server, no root): the profile and file rules, the exact
argument line and its allow-list against hostile profiles, the error-text mapping, ports, enable/disable/edit/delete (key and trusted-host
removal), reconnect with back-off, hard faults, trust and changed-key refusal, Test, the Browser seam, the audit log, the Settings page
(including four screen sizes) and the assistant tools (card text, hard-ask, no secret in any result, card or log).

**Only a real SSH host can show**: that OpenSSH 10.2 accepts exactly these arguments; that `-J` works on the images (OpenSSH starts the
jump hop with the `ssh` of its compiled-in path, which on an image may be Dropbear's `ssh`; see KNOWN-ISSUES); that the error texts are the
ones mapped; that the in-memory agent signs for a login to a real server (the git path already proves this with `ssh-add`/`ssh-keygen`);
a real drop and reconnect; start at sign-in on a device.
