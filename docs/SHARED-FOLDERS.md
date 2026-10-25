# Shared folders

Settings, **Shared folders** (also: right-click a folder in the AI IDE tree, **Share this folder…**, or the EDIT context menu, **Share this file's
folder…**) shares a folder of yours over HTTP on the local network, so a phone, a laptop or a TV can browse and download it, optionally upload to
it, or open it as a small website. It is built into IYAKKI (no extra program is started, nothing is installed). Rows: `F-SHARE-*` in
[`../FEATURES.md`](../FEATURES.md).

## What it is, in plain words

* A **share** is a folder, a name, a way to open it and a port. Each share has its own port (8081 and up, chosen when the share is made and kept) and
  therefore its own address and its own "origin" in a browser. That is the reason for one port per share instead of one port with `/name/`
  prefixes: a shared *website* can run its own scripts, and those scripts can never touch another share, the portal or the shell's pages, because
  the browser keeps different ports apart.
* **Nothing is shared until you make a share and start it.** A new share is stopped. It is **read-only**; uploads are a separate choice. It stops
  when you sign out (unless you chose "it starts again when I sign in"), when its time is up, and when you press **Stop all sharing**.
* The folder must be one of yours: inside your home folder, one of your workspaces, or a folder you were given. `/`, `/etc`, `/proc`, `/sys`, `/dev`,
  `/boot`, other people's homes, the system's own data folder (vault, settings, the list of shares) are refused, also when an administrator could
  write there.

## Ways to open a share (who can open it)

| Mode | What a visitor needs | Notes |
|---|---|---|
| **Secret link** (default, recommended) | The address `http://<this computer>:<port>/s/<32 hex digits>/` | 128 random bits. Anyone with the link can open it; without it every address answers "not found". **New secret** (`t`) changes it at once and the old link stops working. |
| **Password** | One password per share (any user name) | HTTP Basic. On plain HTTP the password travels in clear text: someone on the same network could read it. The page says so. Stored only as an Argon2id hash. Ten wrong tries from one address in a minute: that address waits. |
| **Anyone on the local network** | Nothing | You must type `OPEN` in the form to confirm. |
| **This machine only** | Nothing | Listens on 127.0.0.1 only; refuses other addresses and other host names (a web page elsewhere cannot reach it through a name that points here). |

In every mode a visitor must come from a **private, link-local, VPN-shared (100.64.0.0/10) or loopback address**; a public address gets 403, so a router
port forward does not put a share on the internet. IPv6 is accepted from local addresses but the share listens on IPv4 only.

## Files or website

* **Files**: a clean, mobile-friendly list (folders first, sorted by name, sizes, dates), downloads with `Range` (video and audio can seek), `HEAD`,
  `ETag`, `Last-Modified` and `304`, correct content types. Pages and scripts (`.html`, `.svg`, `.js`, `.xml`, `.wasm`…) are served as **downloads**
  (`Content-Disposition: attachment`, `application/octet-stream`, a sandboxing Content-Security-Policy), so a shared folder cannot run script in
  anybody's browser.
* **Website**: static files; `index.html` is served for a folder; there is **no list** and no upload. Scripts run, but on the share's own port.
  Use it to look at a site you build in the AI IDE on another device: share the workspace folder (IDE tree, **Share this folder…**; a folder with an
  `index.html` is offered as a website) and open the address on a phone.

## Choices in the form

Name · folder (type the full path; the IDE and EDIT fill it in) · Files or Website · who can open it · password · visitors may (read only; upload new
files; upload and make folders; upload, make folders and replace files with the same name) · largest upload (1 to 512 MB, default 25) · hidden files
(`.name`) stay hidden or are shown (never in a website) · stops after 1 hour, 1 day, 7 days or never (counted from each start) · listen on all
interfaces or one address · port (empty: the next free one) · after sign-out (stops, or starts again at the next sign-in).

The page lists every share (name and port, folder, mode, who can open it, time left, number of different visitors), with **Start/Stop**, **Copy
link**, **Show QR** (scan it with a phone), **Edit**, **New secret**, **Delete** and **Stop all sharing**. Every address of this computer is shown;
the `.local` name is not offered because the images have no mDNS responder. The top bar's Devices list shows a **Sharing** row with a **Stop**
button while anything is shared.

## Limits and protections

32 connections, 16 KB of request headers, 15 seconds to send them (slow clients get 408), 15 seconds of silence while an upload arrives, two uploads at
a time (an upload is kept in memory while it arrives, up to the share's limit), no keep-alive, 300 requests per 10 seconds per address. An upload
never overwrites a file unless the share says so, is refused when less than 16 MB of free space would be left, is written with mode 0644 and never
through a symbolic link. File names from forms keep only their last part; names starting with `.` are refused.

Paths: split on `/`, each part percent-decoded once, then `..`, backslashes, NUL, control characters and over-long names are refused; the result is
made real (`canonicalize`) and must still be inside the folder, so a symbolic link that points out is a 404 and is not listed; dotfiles (and a link
into a hidden folder) are hidden; the data folder of the system is never served. Responses carry `nosniff`, `Referrer-Policy: no-referrer`, a strict
Content-Security-Policy and `X-Frame-Options` for listings, and never a cookie.

The audit log gets `share.create/edit/delete/start/stop/stop_all/expired` and one line per request: share name, client address, method, path (without
the secret part of a link) and status. Passwords and secret links are never written there.

## Firewall

The images have no firewall of their own (only the cluster image carries `iptables` for Kubernetes). A share in "all interfaces" mode therefore opens its
port on every network interface of the computer, to the local networks listed above. Choose **This machine only** or one address in **Listen on**
to narrow it.

## Troubleshooting

* *"port … is in use"*: edit the share and give it another port.
* *A phone cannot open the link*: both must be on the same network; check the address in the list (a computer can have several); a guest Wi-Fi often
  isolates devices from each other.
* *403 "not available from this address"*: the visitor's address is public or, for "This machine only", not this computer.
* *404 on everything with a secret link*: the link changed (**New secret**) or is mistyped.
* *410*: the share's time ran out; start it again.
* *The share stopped by itself*: you signed out, its time ran out, or **Stop all sharing** was used.

## Not done yet (planned) and not tried yet

* The assistant has **no tool to share a folder** (on purpose, for safety); it can only describe how to do it.
* Not offered: a zip of a whole folder (the tree has no zip writer), encryption (HTTPS), a `.local` name, IPv6 listening.
* **Unverified on hardware.** The tests drive the server on 127.0.0.1 only (many requests, hostile paths, limits, timeouts) and the page at four
  screen sizes. It has not been tried from real phones, TVs or media players, over Wi-Fi, with large videos, or on the images; a browser's own
  handling of `Content-Security-Policy: sandbox` for downloads, Basic-auth prompts and QR scanning are untested.
