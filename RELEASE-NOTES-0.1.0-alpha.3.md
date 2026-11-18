# IYAKKI 0.1.0-alpha.3: release notes

*An OS for agents.* Third alpha, cut on 2026-11-14 (the notes were first written on 2026-11-12; the numbers and the section "The last two days" were brought up to the cut). Licence: Apache-2.0. The image names and the checksums are filled in when
`./run.sh release` has run ([docs/RELEASING.md](docs/RELEASING.md)). The notes for the earlier alphas are
[RELEASE-NOTES-0.1.0-alpha.1.md](RELEASE-NOTES-0.1.0-alpha.1.md) and [RELEASE-NOTES-0.1.0-alpha.2.md](RELEASE-NOTES-0.1.0-alpha.2.md).

**This alpha is still a preview for developers and testers.** Do not use it in production or on a machine that holds data you cannot lose: the installer
erases the drive you pick. Most of what is new below was checked by tests and fakes. The section "What was tried where" says which parts were also seen on a
real computer, with a real model or in a real browser, and "Known limits" says what is not proven. Where a line says "not on hardware", believe that.
[KNOWN-ISSUES.md](KNOWN-ISSUES.md) is the full list.

## The numbers

The feature matrix ([FEATURES.md](FEATURES.md)) has 1514 rows. `python3 dist/features/migrate-impl-test.py` (run on the cut tree) reports:

| State | Rows | Meaning |
|---|---|---|
| done | 431 | fully written (code implemented) and fully proven (fully tested): Impl 100, Test 100, and no open hand check |
| partial, fully written but not fully proven | 848 | the code matches the row (Impl 100); the proof is fakes, scripted models, or a hand check nobody has made yet |
| partial, not fully written | 141 | part of the row is still missing (Impl below 100) |
| planned (not started) | 70 (17 of them the alpha.4 fleet and model-router rows) | |
| deprecated | 24 | |

So 1279 rows are fully written (431 + 848), and 431 of them are also fully proven; the rows that moved most since the first draft of these notes are those at Test 70 (about 80 more rows were run against a real tool, kernel, browser or second implementation, not only fakes). At the alpha.2 commit the matrix counted done 370, partial 887, planned 77, deprecated 29 (1363 rows; those
were counted by status only, before the Impl/Test split). "Implemented" on a row does not mean "tried on a machine": the Manual column says `passed <date> <machine>` only where a
sentence of the row's notes reports a real run (28 rows today).

The words follow the owner's rule of 2026-11-12: *fully written* means the code is implemented, *fully proven* means fully tested, so verification that is still owed (a real device, a real service, a hand check) is shown in Test and Manual and never holds Impl below 100. On that day 43 rows whose notes name only verification as missing were raised to Impl 100 (list in docs/development/IMPL-RULE-2026-11-12.md), so the counts above moved; run the script for the current numbers. The browser and mobile client family (F-CORE-14, F-CLIENT-01 to 09) is experimental by owner decision and verified on the Chrome path only.

## What is new since alpha.2

About 360 commits, 2026-11-10 to the cut. The full list is in [CHANGELOG.md](CHANGELOG.md); the rows and tests behind each item are in [FEATURES.md](FEATURES.md).

### The last two days (2026-11-12 to the cut)

* **The core rule (owner, 2026-11-14): the app layer never touches Linux or a device; it asks the core, which asks permission and logs.** `cx.os` now has a guarded
  wrapper (`GuardedOs`): reads need `system.read`; changing things needs its own capability, and the hard ones ask a card every time that only the person's own press answers
  (`system.hostname.set`, `net.scan`, `system.crashlog.write`, radio, brightness, volume, memory). Moved through the core so far: System hardware page, drive-gone wording
  (Chat, Story, Camera), host name, kept crash logs, NET SCAN. The shell's direct-access list shrank to four files. **An app procedure for teams** is in
  [docs/development/APP-DEVELOPMENT-PROCEDURE.md](docs/development/APP-DEVELOPMENT-PROCEDURE.md) with a phone-size isolation check; MCP, MEDIA, SKILLS, DATA, AUTH, CAMERA, USERS and
  RADIO are built on the port pattern (RADIO is its own crate, `iyakki-tile-radio`). The built-in apps' migration is alpha.4 work.
* **Security fix: developer mode's `dev input` can no longer answer an approval or power card** (a paired computer holding `input` could press the approve key of its own
  shell approval, or restart the device with `p` then `r`). Fixed and tried in a real guest.
* **Product defects found by real runs and fixed:** a power cut shortly after boot was not noticed at the next boot (the session record is now synced to disk); `iyakki cloud
  listen` left its firewall rule after SIGTERM; the cloud profile's service touched sound cards; the FAT32 reader took small volumes for FAT16 and listed nothing; GRUB's `loadenv`
  module was missing; Chat hid the Model chip while an error was shown; the System tile overflowed with long host names. The new boot self-test stages and these image-side fixes
  reach a device only with a rebuilt image.
* **Windows too small or too narrow:** the shared app header drops icons at narrow widths instead of running over the title; most tiles fit at 360 pixels and at the largest text size
  (the remaining ones are listed in KNOWN_OVERFLOWS and KI-015).
* **Web UI:** Chat answers are shown formatted; the Web Edit tile handles the pointer; Settings has two more switches that ask first; `iyakki serve --profile tile-host` (first slice);
  runs started on the desktop are announced to the web service at once (F-WEB-51); real-Chrome and node page runs passed (561 checks).
* **Real tools, not only fakes** (each row's note says exactly what ran): real `sfdisk`, `sgdisk` and `mkfs.*` disks, Ghostscript and `pdftotext` for PDFs, real WireGuard, `ip` and `nft`
  for VPN routing, a real WebSocket client, a 3 GB encrypted backup, a real `sshd`, a real paired QEMU guest for developer mode, the shell's pages on a virtual screen, real
  Ubuntu and Debian checksums, real `curl` and `openssl` against the webhook listener.
* **Still open on purpose:** a session kept across a whole-process crash needs an owner decision (F-SHELL-40); pinned downloads need your decisions (`docs/project/PIN-DECISIONS.md`);
  nothing in the last two days was seen on the Mac mini, MacBook or HP unless a row's note says so.

### Updates and the image

* **Layered root and delta updates.** The system can be split into layers (a layer manifest read by the first boot stage). An update pack of **format 2** carries each
  layer as keep, delta or full, so a small change ships as a small download; `quick-push.sh --layers` and `nightly-push.sh --layers` make such packs
  ([docs/UPDATES.md](docs/UPDATES.md), [docs/development/LAYERED-ROOT-DESIGN.md](docs/development/LAYERED-ROOT-DESIGN.md)). Booted in QEMU, including a data volume that
  survives the change. **Not yet confirmed as an update on a real computer.** See "Upgrading from alpha.2".
* **Lean release image.** `./run.sh release --lean-image` releases the lean PC image as `iyakki-x86_64.img`. The kernel (6.12.112 has a pinned checksum file in the
  Buildroot patches) and the downloads the build needs are pinned by checksum.
* **Installing:** `iyakki-usb --install` and the installer can write a legacy-BIOS boot for a shared disk (`--bios`, with a save file and a read-back). The
  planner lists MBR logical partitions and keeps them out of the primary slots. The BIOS write is proven in QEMU only.
* `users.json` carries a schema stamp, a newer file is kept as it is, and override settings survive an upgrade.

### Keyboard, sign-out, the screen and the idle machine

* **Mac profile: Ctrl+Cmd+Q signs out (after a confirm card) and Ctrl+Cmd+L locks.** A keyboard sign-out always asks first. The desktop window takes the same
  Ctrl+Alt layout chords as the machine.
* **Cameras:** the computer's own cameras are closed the moment you leave the Cameras tile by any route, and a stream that is no longer drawn is closed at once. A
  camera whose picture stays black now says why (a closed shutter).
* **An idle Home no longer redraws about 7 times a second**; it redraws every 500 ms (KI-092).
* **Screen saver:** two new styles, a starfield and a photo slideshow of the signed-in person's own Pictures folder, and a key chord that starts the saver at once
  (Ctrl+Windows+S, Ctrl+Command+S in the Mac profile). Not looked at on a real screen ([docs/SCREEN-SAVER.md](docs/SCREEN-SAVER.md)).
* **Desktop views, first slice:** named views per person with their own apps and panes, jump and step keys, a strip, a Settings page, templates (your own too) and a
  locked view.
* An administrator can set the default Home layout for new accounts, from the page or by chat. Short word buttons, file pickers for more path fields, and smaller
  windows fixed for notifications and empty states.
* **Compressed memory (experimental, off by default):** one setting on Settings, Memory (zswap plus a small swap file when there is a data disk, zram otherwise).
* **Boot options:** `iyakki.bootrecord=1` records which files the first 90 seconds open; `iyakki.prefetch=on` reads them ahead on later boots. Off by default, **not
  measured** ([docs/BOOT-SPEED.md](docs/BOOT-SPEED.md)).

### Network, Bluetooth, downloads and files

* **Wi-Fi on Macs with a Broadcom BCM4360 card (2013 to 2015 Macs, for example the Mac mini 2014):** no driver ships; Settings, Network and the join message say so
  in plain words, and `b43` and its relatives no longer probe the card (this needs the new image). See "The Wi-Fi note" below.
* **Downloads:** a broken download continues where it stopped (Range with If-Range), a short body counts as a network error and a retry resumes. With no network a
  driver or install download now fails within seconds with a plain message (KI-083). The xz reader verifies block headers and the CRC32, CRC64 and SHA-256 block checks.
* **Bluetooth:** a phone tether reconnects after the phone went away, with growing pauses. Classic HID devices take vendor and product from the Device ID record. A
  working controller with a valid address is no longer shown as "firmware missing" (KI-084, not on hardware).
* **Hotspot:** group-key handshake and rotation in the host access point (tests only).
* **Firewall:** per-rule counters on the page and by chat, nf_tables sets for long address lists (the kernel part was loaded once in a private namespace, no traffic),
  the networks of a connected VPN can count as the local network, and the page says when the VPN kill switch or fail-closed guard is active.
* **SSH:** the SSH server takes RSA and ECDSA P-256 login keys, offers an ECDSA P-256 host key and the chacha20-poly1305 and aes256-gcm ciphers, and re-keys after 1 GB;
  Settings has server controls, a card before a start, keys and password login. `iyakki-connect` can sign in with a key an ssh-agent holds. The CLI tile does an
  interactive SSH login on the in-house terminal.
* **Shared folders:** a shared folder can be downloaded as one zip; an **UPnP/DLNA media server** shows the audio and video of your shared folders to TVs and players on the
  local network (off until you switch it on, by chat `media_server_set`, which asks) ([docs/SHARED-FOLDERS.md](docs/SHARED-FOLDERS.md)). Tried with a test client, not a real player.
* **Backup:** `iyakki backup areas|preview|create|inspect|restore` works on the same functions as the Settings page; the backup function reports progress over both
  passes and can be stopped without leaving a file (the page and the command do not show a bar or a stop button yet) ([docs/BACKUP.md](docs/BACKUP.md)).
* **Developer mode:** `iyakki dev shell --tty` gives a real terminal (resize, raw mode, the same grant rules); a standing restart grant per paired computer, which ends
  with the developer connection; **web service API scopes** (`api:chat`, `api:flows`, `api:files` and so on, narrowable to reading): programs sign in with a paired
  developer key and may call only the route families an administrator granted to that computer ([docs/DEVELOPER-MODE.md](docs/DEVELOPER-MODE.md)).
* **Messaging:** an inbound Matrix mirror into the INBOX (every send still asks). **Flows:** a Thrift RPC node over HTTP (JSON protocol, in-house codec). **Cluster
  work:** a swarm scheduler with least-loaded, round-robin and fastest policies.

### Linux apps

* **Outbound-only sandbox network:** the Network button of a Linux window cycles off, outbound only (own network namespace joined to the computer, with an allow-list of hosts
  and ports), and shared; chat tool `linux_network_set`. Written and proven in QEMU guest tests; **not run on a real Linux machine** ([docs/LINUX-APPS.md](docs/LINUX-APPS.md)).

### Browser

* **The built-in browser:** pictures by `srcset`/`sizes`, `<picture>` sources and lazy pictures; animated WebP; the GIF loop count (a GIF without one plays once);
  streamed downloads in ranged pieces with a progress bar and file upload fields; **bookmark folders** (made, filled, renamed and deleted from the address bar), history
  search; **text-only zoom** and Ctrl with + / - / 0 and the wheel; small pictures of the sites you visit most on the start page (switchable, never for sign-in pages);
  name and address filling from the vault; saved files open from the Downloads list in the tile that suits them; a problem report key (`!`); a console prompt, an
  Elements page, reader logs for the agent and a Network panel that names the custom header rules active on the page; a script that sends the page on is followed for
  at most three steps.
* **Script engine:** scripts now get element sizes, real media queries, working observers and a 2D canvas context that takes drawing calls and draws nothing; cache storage in a visit; nodes a script made and dropped are
  given back (the Rust book no longer runs out of elements); a much shorter start-up on heavy pages (BBC News from 1.09 s to 0.40 s in a release build on a Mac mini).
  Position, floats and `box-sizing` are laid out ([docs/development/BROWSER-COMPAT.md](docs/development/BROWSER-COMPAT.md)).
* **Blocking:** a uBlock-filters button in Settings, Browser, and chat tools to see status, download, update, remove and allow sites (nothing is downloaded until a person
  asks) ([docs/CONTENT-BLOCKING.md](docs/CONTENT-BLOCKING.md)). **Your own short words** for the address bar, in Settings and by chat.
* **Custom request headers** in the full web engine (the first browser extension). Written, **not proven**: it was never compiled against the real engine headers (KI-086).
* **Chat tools** for bookmarks, history, zoom, saved details, the firewall VPN switch and API scopes.

### PDF

* **Select text** with the mouse or a finger and copy it; pinch, swipe and long-press; multi-column pages are read column after column with headings in place; PDFs from
  Downloads open in the viewer.
* **Writing PDFs:** the writer compresses page text and pictures with an in-house encoder and embeds DejaVu subsets for Greek, Cyrillic and symbols. The chat tool
  `pdf_create` (and the Markdown-to-PDF blocks) writes a PDF; **TALES** has Save PDF (`Stories/<title>/story.pdf` with its pictures) and **LEARN** exports plans, quizzes
  (with an optional answer key page) and lessons. Scripts that need shaping (Indic, Arabic, Hebrew, Thai) and Chinese, Japanese and Korean are refused with the script named
  ([docs/PDF.md](docs/PDF.md)).

### Chat, flows and the assistant

* **More of the system is reachable by chat.** Settings pages that had no tool now have one (views, notifications, screen saver, Apps access, the client switch,
  keyboard, time, browser headers, Home layout, system sounds, battery, power, cloud mode, startup, the Assistant page, compressed memory, backup, the check-up result).
  Reads are free; every change asks every time and is audited. A test fails when a Settings page has neither a tool nor a written exemption
  ([docs/development/CHAT-REACHABILITY.md](docs/development/CHAT-REACHABILITY.md)).
* A correction line is appended when a reply says a board row is done while the board says otherwise; plain wording for a model that is missing or not loaded on a LAN
  server; a live tool-use harness for a small real model, and small models are offered fewer tools by routing on the words.
* **Flows:** a kill switch for assisted flows on the flows list and a one-way chat stop; a failed run reaches the inbox once; the failure-help offer can be turned off per flow.
* **Last session:** a Settings button copies the last session log to a private file in your home folder (chat tools `last_session_status`, `last_session_log_copy`).
* **`iyakki doctor [--detail]`** prints the DOCTOR tile's checks as text and exits 1 when something needs attention; it reads files from the data folder and does not see the running
  system's own state ([docs/DOCTOR.md](docs/DOCTOR.md)). **`iyakki checkup-result FILE`** checks the signature of a CHECK-UP result file.
* **SDK 1.2 (app bridge v2):** discovery (`iyakki.has`, `iyakki.capabilities`), Markdown, embed, dash, flows, k3s, media, audio, `speech.listen` and a per-app money limit
  (default 0; no payment provider is called), each through scope and approval, with additive versioning rules ([docs/development/SDK-BRIDGE-V2.md](docs/development/SDK-BRIDGE-V2.md),
  [docs/development/SDK-API.md](docs/development/SDK-API.md) section 14). The SDK emulator has fake services for them ([docs/SDK-EMULATOR.md](docs/SDK-EMULATOR.md)). The IDE has a docked
  Preview beside the editor on Run. The host side of several namespaces (viewer, embed painting, cluster, flow engine, player, microphone) is not driven end to end.
* **Developer tools in the IDE and cluster work:** open a pull request from a connected workspace branch on the cluster's Gitea (with a chat tool); test counts are read from
  mocha, vitest, Maven, .NET, RSpec and PHPUnit output.

### Cloud

* An **audit trail** for the tenant: `GET /api/v1/tenant/audit`, an Audit trail card on the Cloud access page, and a chat tool; person layers of tenant configuration;
  `cloud_storage_usage` shows folder and tenant use against the limits. Cloud features are tested with fakes and a local cluster, never a managed cloud.

### Security

* **Home folders are made 0700 at every start, and diagnostics, downloads and screenshots are created 0600** (KI-095: before this, a developer-mode shell could list and read a
  person's home). Fixed in code; the hardware re-check is pending.
* The chat input guard also catches a netrc entry (machine, login, password). The desktop in a browser (remote screen) denies a page's camera and microphone unless the setting says allow.
* The dashboard limits itself to 4 `kubectl` runs at once and its error cards name the cause (KI-090); Platform services asks the cluster instead of only its own install
  records, and the Registry row no longer spins (KI-091).

### Experimental, off by default

* **Connect a client:** a browser page behind a one-time link and a small window program (`iyakki-connect window`) show an instance's screen and send input back, with sound,
  a text clipboard and the client's own camera and microphone as opt-in extras, each decided per device in Settings. Labelled EXPERIMENTAL everywhere. Tried end to end in one
  headless Chrome against fakes; not over a real network, not on macOS or Windows (KI-087 to KI-089).
* **Core access model:** a catalogue of capabilities, an `app.json` capability list with an install card, and a broker with the camera capability and an access log. Tests with
  fakes; not seen on a screen.
* **macOS:** the desktop window compiles and runs natively on a Mac with Apple silicon and the mixer has a native AudioQueue sink (KI-078). A few tiles are limited there.
* **Compressed memory, boot record and prefetch** (above). **The wake phrase** is still experimental and off by default.

### Not user-visible but worth knowing

* The matrix has Impl/Test and Manual columns, so a row cannot say "done" on fakes alone. The partial count may not exceed a ceiling file that a test reads.
* `dist/hwtest` drives a real machine through `iyakki dev` and records what it saw (read-only recipes, and some that change state and clean up after themselves).
* `dist/browsertest` is a small real-browser harness (Chrome over DevTools, Python standard library only; not part of the default gate).
* A static demo site with a generated progress page (which says "not started" for planned rows) was added for GitHub Pages.

## What was tried where

"Tried" means a person or the check scripts ran it on the machine; everything else is tests and fakes. Evidence is in the Manual and Notes columns of FEATURES.md and in
[docs/HARDWARE.md](docs/HARDWARE.md).

### On real hardware (builds between alpha.2 and alpha.3)

| Machine | What was seen | Not tried or failing |
|---|---|---|
| Mac mini 2018 (Macmini8,1, T2) | Hardware-check batches ran read-only page and state checks (the commit that records the sixth batch says 19 passed on what was observed, and KI-095 failed for the home-folder row); Ctrl+Cmd+Q and Ctrl+Cmd+L both act on a USB keyboard; the desktop view buttons; the dev cluster's clock wait, cluster health in SYSTEM, the Store tile, `iyakki dev info/ps/dmesg`; zram swap; AI-tool profile rows; a Debian sandbox created and removed; key repeat; a tile app install and a flow from a template | Apple's own keyboard; Bluetooth (the chip needs a patch file macOS does not carry); the idle-redraw figure after the fix is to be re-measured (KI-092) |
| Mac mini 2014 (Macmini7,1, no T2) | Boots from the lean image over a cable; the owner reports Bluetooth and sound work; the commit that records its sixth batch says 39 recipes passed on what was observed and 3 need a person | **Wi-Fi does not work** (see below) |
| MacBook Pro 14,1 | Booted on the alpha.2 final image (owner). Bluetooth pair, phone sound and tether with internet (alpha.2 line, 2026-11-08) | The camera release fix and the black-picture message are not confirmed |
| HP Pavilion 16 | Booted on the alpha.2 final image (owner); its camera worked (alpha.2) | The camera fixes (KI-082, KI-093) are not confirmed |
| Lenovo Legion Pro 5 | Booted on the alpha.2 final image (owner); no-network download failure in seconds (KI-083); a flash drive written with `iyakki-usb`; lock keys (Manual: passed 2026-11-11) | The camera light fix (KI-080, KI-093) is not confirmed |
| MacBookPro16,2 (T2, 2020) | Nothing new since alpha.2 | |
| A Mac with Apple silicon | The desktop window runs natively and finds a real Ollama (hand test) | The VM path for IYAKKI itself has never been run |
| QEMU and containers | Layered root boot with a data volume; the usual boot, install and update tests; opt-in guest proofs for the outbound-only network, boot prefetch, compressed memory and the BIOS shared disk | |

Secure Boot is verified in QEMU only on most machines (on the HP, the log of the loader says it accepted the signed chain on 2026-11-07).

### With a real model, a real server or a real browser (no hardware involved)

* **A real model on a Lemonade server on the LAN (qwen3-8b, plus other models for comparison, a speech and picture model):** live checks for the assistant, IDE, flow and
  provider rows, speech, transcription, pictures and the TALES tile, `pdf_create`, TALES Save PDF and LEARN PDFs. Findings and what the runs did not cover:
  KI-099 to KI-101. Only the Lemonade (OpenAI-compatible) path was used live; OpenAI, Anthropic and Gemini were not.
* **Real Chrome driving the web pages and the client page** (`dist/browsertest`): the desktop-in-a-browser pages, the tile renderer, the
  client engine (a real AudioWorklet, clipboard and camera list against a fake machine). It found and fixed defects: touch taps reaching the machine as NaN, the skip link,
  unnamed icons, empty ids, an upload that did not fit a phone, and a committed client engine module that was older than its source (KI-096, re-check pending).
* **The built-in browser engine against about 30 live sites** (a probe that ranks the web APIs real sites use and the engine lacks): findings and fixes are in
  [docs/development/BROWSER-COMPAT.md](docs/development/BROWSER-COMPAT.md). A site that needs a real engine still needs the full engine.
* **Real tools on the build Mac:** a real OpenSSH `sshd` for the tunnel tests, a real `diskutil`, a real mDNSResponder, and the real flow engine behind the web routes.

### Only with fakes or scripted models

Everything not named above: for example the UPnP/DLNA server (a test client), the BIOS write of a shared disk, the SDK 1.2 host side, Hotspot rekeying, the cloud audit trail
and tenant layers, the Matrix mirror, swarm scheduling, the Thrift node, the custom request headers extension, VPN (KI-076), Linux sandboxes on an image (KI-075), and the encrypted
home beyond the single Mac mini run recorded for alpha.2.

## Upgrading from alpha.2

* **Layered root and update pack format 2.** A device that reads only format 1 (every alpha.2 device) **refuses a format 2 pack** with "update format 2 is not understood" before
  anything is staged, so it is never applied half. The **first push to each existing device must be an ordinary (format 1) pack**; format 1 packs apply as before. A single-root update
  after a layered one retires the layer pointers so the newer root starts. A fresh install from a layered image boots from its seed set. How a device that already runs a single root becomes
  layered by an update is not written down yet and has not been tried: treat layered packs as for fresh layered installs and for QEMU until a device has been shown to take one.
* **Signatures.** An update pack carries a detached ed25519 signature (`update.json.sig`): a wrong signature is always refused, a missing one asks the person first, and a pack that
  changes the Secure Boot chain is refused ([docs/UPDATES.md](docs/UPDATES.md)). An image is Secure Boot signed only when it was built with the maintainer's key.
* **The new image is needed** for the `b43` blacklist, the 0700 home folders at every start, the Wi-Fi message and the other changes that live in the base layer.
* **Data:** `users.json` gets a schema stamp; a file written by a newer IYAKKI is kept as it is. The data volume is not encrypted (vaults and the optional encrypted home are).
* **Hot patches** (`iyakki dev patch`) made for a specific root id are skipped on a layered root; patches for any root work.

## The Wi-Fi note for the Mac mini 2014 (and other 2013 to 2015 Macs)

These Macs have a Broadcom BCM4360 card. The kernel's old `b43` driver finds it and fails (`FOUND UNSUPPORTED PHY`), and `brcmfmac` does not support the chip. The usual fix on other
systems is Broadcom's `wl` driver, which is closed source and not something this project can include or ship, so **there is no built-in Wi-Fi on these machines**. What works: the
network cable, a USB Wi-Fi adapter (MediaTek and Atheros chips with open drivers), and a phone's tether over USB or Bluetooth. From this alpha, Settings, Network says this instead of showing
nothing, and the noisy `b43` error at boot goes away with the new image. A person can build the `wl` pack themselves ([docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md)); that was never done on a
Macmini7,1 and is not covered by this alpha. This is a known limit (KI-094), not a bug waiting for a fix.

## Known limits

The honest list is [KNOWN-ISSUES.md](KNOWN-ISSUES.md). Those you will meet first:

* **Not for production or irreplaceable data.** The installer erases the drive you pick; the data volume is not encrypted (vaults and the optional encrypted home are).
* **Most feature rows are still `partial`** (989 of 1514; see "The numbers"). "Implemented" on a row does not mean "tried on a machine".
* **Wi-Fi on BCM4360 Macs** (KI-094), above. **Bluetooth on the Mac mini 2018** needs a patch file macOS does not provide; use a USB adapter.
* **Camera light:** the fix for the light staying on (KI-080, KI-093) and the message for a covered camera (KI-082) are tested with fakes only. Until confirmed, check the light after
  closing the Cameras tile, and sign out if it stays on.
* **Home folder permissions (KI-095), the idle redraw figure (KI-092), the dashboard and Platform services fixes (KI-090, KI-091)** are fixed in code and wait for a re-check on the Mac mini.
* **Encrypted home** is a first version with one real-hardware run; do not keep the only copy of anything in it. Whole-disk encryption and TPM sealing are not built.
* **Browser sound** with the mixer on: the lag fix (KI-085) is tested with a fake sink and not on any machine. **Custom request headers** (KI-086) and **Connect a client** (KI-087 to
  KI-089) are experimental and tried in narrow setups only.
* **Compressed memory, boot record and prefetch, the outbound-only sandbox network, the UPnP media server and the BIOS write of a shared disk** are new and have no real-machine result.
* **The wake phrase** has no real-voice measurement. Leave it off.
* **Secure Boot** is verified in QEMU only on most machines; **VPN, Linux sandboxes on an image, managed clusters and every cloud provider except a local Lemonade server** are untried
  (KI-002, KI-023, KI-075, KI-076).
* **Tests that fail now and then** under a full parallel run are listed by name (KI-079); the release check reruns them alone.

If something is not listed and it does not work, please report it: [SUPPORT.md](SUPPORT.md).

## Getting it

The images, the window build and the checksums are made by `./run.sh release` ([docs/RELEASING.md](docs/RELEASING.md)); `SHA256SUMS` and the signed `release.json` come with them. Verify
before you use them. Secure Boot needs the maintainer's key and enrolment; unsigned images need Secure Boot off.
