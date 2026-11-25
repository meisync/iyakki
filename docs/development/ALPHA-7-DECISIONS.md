# Alpha.7 decisions the owner may overrule

Each entry: id, question, options, the default that was implemented (so Impl does not wait on an answer). Change the default by changing the code named in the row.

## D-BACKUP-04: where the credentials of a backup network share live

* Question: an SMB share needs a user and a password; where are they kept?
* Options: (a) a vault entry of the person (`backup-share/<name>`, two lines), (b) a settings file, (c) typed each time.
* Default implemented: (a). The password is never in settings, never on a command line; a 0600 credentials file lives inside a private folder for the length of the mount only (`iyakki_shared::backup_share`). NFS needs none. Rows: F-BACKUP-04.

## D-BACKUP-05: what a backup of data outside a person's own places holds

* Question: system settings, Git connections, VPN profiles, Gitea and the store hold keys and tokens: which may be written and how encrypted?
* Options: (a) a separate "system" area, administrators only, always encrypted, restorable only by an administrator; (b) fold it into the person's backup; (c) never.
* Default implemented: (a). Files: `settings.json`, `connections.json`, `git.json`, the `vpn` folder, `store/installed.json`, `packages/installed.json`. A system backup is refused without a passphrase; the person's own create and restore paths refuse the area by name. Gitea's repositories live in a cluster volume, not in a file of the data folder, so they are NOT covered (a volume snapshot is a later row). Rows: F-BACKUP-05.

# Alpha.7 decisions the owner still has to make
**For:** the owner. **You will get:** each open question from the Implementation 100 work, the options, and the default that is already coded (the most conservative one, configurable or off). Nothing here blocks Implementation 100; the default runs until you choose.
| ID | Question | Options | Default now in code |
|---|---|---|---|
| D-VPN-1 | Where does the OpenVPN program come from for the person's click (F-VPN-06)? | (a) the person types an address and a SHA-256 in the setting `vpn.openvpn_download`; (b) we publish a musl build ourselves; (c) leave OpenVPN unsupported | (a): the setting is empty, so the click explains the gap and fetches nothing; the manager already runs the program once the person has put it in the downloads folder |
| D-RAG-1 | Model-based name extraction for Graph RAG (F-RAG-18): which models may read the person's passages? | (a) only a chat model of the person's own Lemonade server, each run approved with the model and the amount of text on the card; (b) also cloud models the person has a key for, with a stronger card; (c) never | (a): the setting is off, the tools always ask, no cloud model is reachable from this code |
| D-SHARE-07 | Shared folders (F-SHARE-07): may the assistant share a folder in one step, and how do HTTPS and a `.local` name come? | (a) tool `share_folder` always asks, read only, link or this-computer-only access; HTTPS and `.local` only through the domain listener (certificates imported or trial-made there); (b) own certificate authority per device; (c) no tool, the assistant only explains | (a): `share_folder` is written (one card, makes and starts, no open/password/upload); HTTPS and the `.local` name are not given to a share on its own: a person adds a domain route to the share's port in Settings -> Domains |
| D-RAG-2 | RAG pages kept up to date by themselves (F-RAG-17) | (a) only on a person's or the assistant's call (refresh and crawl, each asks); (b) a timer reads the kept pages with the web access while nobody is present; (c) never | (a): crawl and refresh are written and ask every time; no timer |
| D-QA-1 | The downloads list and the image content scan (F-QA-05, F-QA-06, F-QA-11): who signs the downloads manifest, where is it published, which image set does the image gate scan, and which unpinned items get a pin? | (a) a key made offline by the owner, kept outside the repository (the public half is in `legal/downloads-manifest.pub`); the list is published at the address already in the code; the gate scans the current release image set; items without a vendor checksum stay `floating` with a warning; (b) a key on a signing service; (c) no fetched list at all | (a): everything is written and runs without a key (a fetched list is refused until a key exists); no digest is invented for an item whose vendor download was not made; the gate scans the images named on its command line |
## D-CLIENT-01: display-list mode and file exchange in the client page

* Question: the client family (EXPERIMENTAL, off by default) has two ideas beyond the picture: display-list mode (the machine sends drawing commands, not pixels) and file exchange through the page.
* Options: (a) picture mode only; files travel through the connector's own `serve --share DIR` folder (one shared folder, read-only unless `--write`); (b) write both into the page and the engine now.
* Default implemented: (a) first; on 2026-11-19 the owner approved the additive display-list pair (D-CLIENT-PROTO-DISPLAYLIST) and page file transfer, both written now (CLIENT-SPLIT-DESIGN 7c and 7d). The picture stays the default mode. Older text follows: The tile decoder and the picture are the only mode; the client family stays experimental. Display-list mode and a page file transfer are NOT written and stay listed as missing in F-CLIENT-01's Notes (Impl stays under 100). Phone-sized pictures are handled by the page's "Fill height" mode instead of a smaller machine screen. Rows: F-CLIENT-01.

## D-CLIENT-02: which systems the connector targets

* Question: the window program uses Unix file modes, termios and signals; should it also build for Windows?
* Options: (a) Linux and macOS only for 1.0, Windows later; (b) port now.
* Default implemented: (a). Packaging is `dist/connect/package.sh` (tar.gz, SHA256SUMS, detached `ssh-keygen -Y` signature when `IYAKKI_CONNECT_SIGN_KEY` names a key kept outside the repository). The operating system's key store is opt-in (`IYAKKI_CONNECT_KEYSTORE=os`: Secret Service on Linux with the secret on standard input, Keychain on macOS where the tool takes the secret as an argument for an instant, which is why it is not the default); the default stays the mode-600 file. Rows: F-CLIENT-02.

## D-CLIENT-03: one sound per machine, PCM only

* Question: should the client's sound be per person (needs the own mixer, docs plan T2) and compressed (Opus)?
* Options: (a) one sound per machine that belongs to the person signed in last, 16-bit PCM about 190 KB/s; (b) per-person mixer and Opus.
* Default implemented: (a) stays the default; the per-person mixer (option b without Opus) is written since 2026-11-19 and switched on by the person (chat tool `client_sound_mode`), see CLIENT-SPLIT-DESIGN 7e. Opus is not written: an in-house encoder is several thousand lines and the repository has no encoder to check it against. Lip-sync is written (the page holds each picture back by the sound's queue plus the browser's output delay, at most 150 ms). The per-person mixer waits for the own-mixer work; Opus is not written (the repository carries no encoder, and an in-house one is out of scope). Rows: F-CLIENT-03.

## D-CLIENT-04: the client clipboard carries text only

* Question: should images and rich text cross the client clipboard as the older browser-as-hardware path does?
* Options: (a) text only, at most 64 KB, one press per use; (b) add PNG and HTML.
* Default implemented: (a): a smaller surface for a feature that moves the person's data across a trust boundary. The older path keeps its images and HTML. Rows: F-CLIENT-04.

## D-CLIENT-TENANT: client sessions in a cloud tenant

* Question (CLIENT-SPLIT-DESIGN section 8, question 4): in a cloud instance, who makes the link for a device?
* Options: (a) each person makes links for their own devices, a device belongs to that person; (b) the tenant administrator makes them for the tenant.
* Default implemented: (a), one pod per tenant. `ClientEndpoint::with_tenant` (the shell gate sets it from `IYAKKI_TENANT`) names the tenant in every audit line and refuses another person's sign-in on a device (`role_denied`); links and devices live in the tenant's own data folder, so another tenant never knows them. A machine that is not a tenant keeps the old rule. Rows: F-CORE-30.

## D-ROUTER-13: how a router strategy plugin reaches the store

* Question: the strategy plugin of F-ROUTER-13 should be listed in the store. Does the store get a new package kind `strategy`?
* Options: (a) a new kind with its own install path and signature check; (b) the sample ships as a file in `dist/samples/router-strategies/` and as the built-in `sample` option of `router_strategy_add` (the card shows the full module text, the person approves each add); (c) nothing.
* Default implemented: (b). A new package kind touches the signature, install, lifecycle and store screens, and is the owner's to open. A WebAssembly strategy runs with no capability token at all, so what it can do is the same whichever way it arrives. Rows: F-ROUTER-13.

## D-FLEET-10: how fleet frames travel through the relay

* Question: the relay of F-CORE-17 holds sealed envelopes for a member that cannot be reached directly. Which fleet frames may go through it?
* Options: (a) every frame kind; (b) heartbeats, assignments and answers only, never a bundle chunk; (c) none.
* Default to implement: (b), the relay being a mailbox of small sealed messages. Rows: F-FLEET-10.
## D-FW-10: how outgoing rules with port lists and both protocols reach the kernel (lane sys-b)

* Question: the kernel writer has no set for port lists in these rules. Expand or use sets?
* Options: (a) one kernel rule per protocol and port range (simple, limited by the writer's 500 rules per chain), (b) anonymous port sets.
* Default implemented: (a) (`iyakki_fw::outbound::to_table_specs`); too many rules is refused in words. Rows: F-FW-10.

## D-HOTSPOT-22: password element for WPA3-SAE (lane sys-b2)

* Question: hunting-and-pecking (original, simple, timing side channel) or hash-to-element (SSWU, resists the Dragonblood timing leak, needs a second code path)?
* Options: (a) hunting-and-pecking only, always 40 rounds; (b) hash-to-element also (status 126).
* Default implemented: (a). A station that insists on hash-to-element is refused (the access point does not announce it in its RSN extension). The point arithmetic is not constant time, so a nearby attacker could learn a little about the password from response times; hash-to-element is the follow-up. Rows: F-HOTSPOT-22.
## D-SDK-16-01: any payment provider behind `iyakki.money`?

* Question: should a payment provider ever stand behind `iyakki.money.charge`?
* Options: (1) never: apps that sell things use their own web checkout in the Browser tile; (2) a link-out helper that opens the provider's own page; (3) a provider behind `money.charge` (contract, merchant of record, refunds, security gate).
* Default implemented: (1). No provider is built or called; limits and cards stay, charging answers `Unavailable`. Rows: F-SDK-16, F-SDK-13.

## D-SDK-16-02: where may an app's listening be recognised?

* Question: the recording an app asked for is turned into text by the voice engine; may that engine be on another machine or the internet?
* Options: (a) only an engine on this computer; (b) also the person's network; (c) any engine the person chose, after a card.
* Default implemented: (a). A network or cloud engine makes `speech.available` answer `Unavailable` with the fix "choose a local engine in Settings -> Voice". Rows: F-SDK-16.

## D-SDK-35-01: size limit of the client engine module

* Question: the display list version 3 reader and the path rasteriser added about 28 KB to the WASM module; raise the limit again?
* Options: (a) 640 KB (from 608); (b) keep 608 KB and drop path filling from the module; (c) page-side drawing only for paths.
* Default implemented: (a). `dist/client-engine/check.mjs` limit 640 KB, module 646,818 bytes with no imports. Rows: F-SDK-35.

## D-WASM-07: what "a snapshot in the middle of a guest call" means

* Question: WebAssembly's call stack is not readable or writable through Wasmtime. The row can mean (1) pause a call in this process and read the state it stopped in, or (2) an Asyncify-style rewrite of every tile module so the stack lives in linear memory and a half-finished call can be saved to a file or forked.
* Options: (1) pause and resume on the call's own thread, no build step, no run-time cost, but the paused call cannot be saved, copied or moved to another process; (2) the module rewriter, 30 to 50 percent run time on instrumented modules and a build step for every tile, thousands of lines.
* Default implemented: (1). `Session::call_pausable` runs the call on its own thread with an epoch deadline callback as the pause point; `PausableCall::pause` returns the memory, exported mutable globals and VFS of the moment (`MidCall`, a `Snapshot`) without ending the call; `resume` goes on from the same instruction, `cancel` ends it, `finish` gives the session back. A snapshot taken this way is for looking and for ledgers; it cannot start a fork or a restore at the middle of a call. Choose (2) only if a flow must save or fork a half-finished call. Rows: F-WASM-07 (Notes name what stays open).
## D-LAUNCH-09: which apps say something on the home screen, and where the day ends

* Question: the home screen's icon lines come from what an app's engine publishes. USAGE, VOICE, DOCTOR, LEARN publish a number now; AUTH (one-time codes) and CAMERA (opened on demand; CAMERAS says what records) have no state. LEARN's "due today" needs a day boundary.
* Options: (a) AUTH and CAMERA stay silent by design, the day ends at the computer's local midnight; (b) AUTH shows how many codes it holds, LEARN uses UTC; (c) every app must say something.
* Default implemented: (a). USAGE shows tokens in 24 h, VOICE shows that the wake listener has the microphone, DOCTOR shows the problems of the last report in this run, LEARN shows the sessions dated today or earlier that are not done (`iyakki_agent::appfacts`). An app that has nothing to say shows its static line. Rows: F-LAUNCH-09.

## D-LAUNCH-22: the picture shipped as a wallpaper

* Question: a photograph needs a licence the owner accepts and adds megabytes to every image. Which picture ships?
* Options: (a) our own drawing made from numbers (`dist/brand/make-wallpaper.py`, 28 KB, the repository's licence) drawn when `wallpaper = picture` is chosen and the person has put no picture of their own; (b) a licensed photograph; (c) none.
* Default implemented: (a). A photograph can replace the file `dist/brand/wallpaper-default.png` at any time with no code change. A picture the person puts in their themes folder always wins. Rows: F-LAUNCH-22.

## D-LAUNCH-25: what the Look's `network` and `apps` bar items open, and the Look's menu

* Question: the Look can list `network` and `apps` as top bar items and a list of menu entries. What do the buttons open, and does the default bar change?
* Options: (a) `network` opens Settings, `apps` goes to the tiles like the Home icon, both only when a Look lists them (the default Look no longer lists `network`, which drew nothing); (b) `network` opens a new network panel; (c) leave them out.
* Default implemented: (a) for the bar items. The Look's menu entries are not built (the IYAKKI menu has its own eight entries; applying a Look's order and hiding to them is open). Rows: F-LAUNCH-25.
## D-SYS-10: how per-process network bytes of host processes are counted (lane sys-b2)

* Question: eBPF programs or the kernel's connection-tracking counters?
* Options: (a) conntrack accounting (`nf_conntrack_acct`), matched to each process's sockets; (b) eBPF cgroup socket programs (needs a loader, a verifier-clean program and a kernel option set).
* Default implemented: (a): nothing new ships in the image, the counters are plain text in /proc, and the owner's rule of everything in-house is kept. Limits: NATted flows and unconnected UDP sockets are not counted. The core switches the counters on once, on first use. Rows: F-SYS-10.

## D-LINAPP-19: which Debian archive key is trusted (lane sys-b2)

* Question: the signed-list check needs the Debian archive signing key pinned in the repository; which key and who vouches for it?
* Options: (a) the owner fetches the current archive key, checks its fingerprint against two independent sources and pins it in `legal/debian-archive-pgp.asc`; (b) ship Debian's keyring package; (c) leave the registry digest as the only pin.
* Default implemented: (c) until the owner does (a): `pinned_key()` returns `None`, the registry route is unchanged and says "image signatures are not checked". The verification code is ready for the key. Rows: F-LINAPP-19.

## D-LINAPP-18: sound and graphics hardware for Linux sandboxes (lane sys-b2)

* Question: what may a sandbox be given when the person asks for sound or the GPU?
* Options: (a) playback PCM and control nodes only, and render nodes only (no microphone, no screen control), per sandbox, off by default, a card every time; (b) the whole `/dev/snd` and `/dev/dri`.
* Default implemented: (a). Capture is a separate grant that does not exist yet. A program that opens a sound card holds it for itself until the planned own mixer exists. Rows: F-LINAPP-18.

## D-KBD-22: Touch Bar strips by holding Fn (lane sys-b2)

* Question: the Apple keyboard may never report a bare Fn key; what does "hold Fn to cycle" do then?
* Options: (a) act on KEY_FN when it arrives (hold 0.6 s, then every 0.9 s), do nothing and keep the chip when it does not; (b) also read the Fn state from the HID report directly.
* Default implemented: (a). (b) needs the keyboard in hand and a HID parser for Apple's report. Rows: F-KBD-22.

## D-NET-29: Bluetooth AAC and aptX beside SBC (lane sys-b2)

* Question: AAC-LC needs an in-house decoder and encoder (large: MDCT, Huffman tables, window switching, noise handling); aptX is a licensed codec.
* Options: (a) SBC only for now, aptX never until the licence is known to be permissive, AAC written as its own piece of work; (b) ship a third-party codec library; (c) write AAC-LC now.
* Default implemented: (a): nothing new is written under this row in alpha.7, no codec is offered to the phone or the head except SBC, so no endpoint is advertised that cannot be decoded. aptX is excluded by the owner's rule (permissive licences, in-house code only). The row stays at Impl 0, planned. Rows: F-NET-29.
## D-TERM-1: what a whole terminal session is allowed to be (F-SDK-103)
Question: a real shell in an app is far more than the policy-checked command line of F-SDK-102. Options: (a) no whole sessions; (b) one yes for the whole session; (c) a yes for every program started. Default implemented: (b). A real `/bin/sh -i` on a pseudo-terminal as the person own system user, never root; only for an account whose terminal level is Full; asked once with a card (Allow, Deny); at most 2 per person; nothing typed or shown is recorded, the audit holds asked, approved, denied and close with the app name. Code: `iyakki-agent::terminalpty`, `iyakki-core-api::terminal`. The assistant (chat) cannot open one.

## D-EVT-1: how app events reach client devices (F-SDK-202)
Question: client devices need the events too. Options: (a) a new contract domain; (b) ride on an existing pushed call. Default implemented: (a), contract 0.3 domain `appevents` (`register`, `events`, scope `events:read`, pushable), a client listens as the app `client.<paired name>` and the person approves its scopes like any installed app. Tiles use tile ABI 3 (`Event::App`) and `appevents:<name>` manifest entries.

## D-EVT-2: sharing sign, storage allowance, wake limits (F-SDK-203)
Defaults implemented: an app own storage is told near full from 90 percent, full at 100, ok under 80 (allowance 16 MiB per app); `sharing.changed` fires when a client device starts seeing the screen or uses the clipboard (never content); an app may have 20 wake times waiting, none more than a year ahead; `location.changed` only when the person let the network choose the time zone.

## D-K3S-1: what an app namespace is (F-SDK-32)
Default implemented: namespace `app-<id>` on the managed k3s, ResourceQuota 10 pods, 2 CPU and 2 GiB requests, default-deny ingress, plan = the deploy lint, changes ask every time. Code: `iyakki-agent::store::sdkv2_k3s`.
## D-VPN-16-01: who may switch the privileged VPN helper, and from where

* Question: the helper only adds protection once the session runs without root; may the person switch it on from the VPN page now?
* Options: (a) page and chat switch it, off by default, with the page saying it adds no protection yet; (b) hide the switch until the session runs as a normal user; (c) image setting only.
* Default implemented: (a). Key `h` on Settings -> Network -> VPN and the chat tool `vpn_helper_set` (asks first). Rows: F-VPN-16.

## D-HOTSPOT-15-01: Bluetooth sharing defaults

* Question: when phones may connect to this computer's Bluetooth network port.
* Options: (a) off until the person switches sharing on, paired phones only, phones isolated from each other; (b) any phone in pairing mode; (c) on after the first pairing.
* Default implemented: (a): `NapService::set_on` starts the listener (link security level 2), `NapHub` answers "not allowed" while off, the bridge keeps phones apart. Rows: F-HOTSPOT-15.

## D-INBOX-09-01: push and quiet hours

* Question: what a person's push settings default to, and what passes quiet hours.
* Options: (a) on once a browser is saved, normal severity and above, no quiet hours, only critical passes quiet hours; (b) high and above; (c) everything always.
* Default implemented: (a), stored per person in the vault entry; opt-out and quiet hours by `webpush_prefs`. Rows: F-INBOX-09.

## D-SDK-34-01: the version bump for the tile ABI's pinch event

* Question: the pinch event changes the published tile ABI; when does the shell start sending it?
* Options: (a) prepare it, send it only to tiles that declare ABI 3, enable nothing until a tile does; (b) bump the ABI for every tile now.
* Default implemented: (a). A batch that holds a pinch is written as version 3 (shared with the app events of F-SDK-202), every other batch stays version 1, an older decoder refuses the new tags (11, 12), and the host drops pinches before a tile of an older ABI (`TileRunner::event`). No shipped tile declares ABI 2. Rows: F-SDK-34.

## D-SDK-30-01: what units the pane manager reports to an SDK app

* Question: the pane manager draws each pane at its own scale; which size does an SDK app hear?
* Options: (a) the pane's logical size (pixels at the pane's own scale), screen 0; (b) the physical size of the pane on the screen.
* Default implemented: (a), so an SDK app chooses its size class (compact, regular) the same way in a pane as on a screen. The pane's screen is 0 until panes know which screen they are on. Rows: F-SDK-30, F-SDK-34.

## D-SDK-14-01: what an app's embedded browser may reach and how it plays sound

* Question: which hosts may an embedded web page visit, and how does an app's sound reach the speaker?
* Options: (a) the hosts the app declared that the person's own host policy also allows, with a cookie profile of its own; sound through the mixer like every other program (the person's volume and mute apply, a busy speaker answers `Busy` with the holder); (b) the hosts the app declared only.
* Default implemented: (a). An empty intersection refuses the mount in words. Only home-folder media files are played by `media.*` (the names `media.library` gave); `audio.play` takes a .wav inside the app or sent by it (.mp3 waits for the shared decoder). Rows: F-SDK-14.

## D-RADIO-AAC (F-RADIO-12, F-RADIO-16): in-house AAC decoder

- Why: an AAC-LC and HE decoder needs the spectral Huffman codebooks, the scale-factor tables, the window and TNS tables and the SBR tables of the standard. They are long (thousands of numbers) and cannot be written from memory without errors; a decoder built on guessed tables would be wrong and look right.
- Options: (a) keep AAC through the person's FFmpeg audio and video add-on, installed at their click (what exists today); (b) the owner supplies a source of the tables that may be used (a spec copy or a permissively licensed reference with attribution), after which the decoder is written in `iyakki-audiodec`.
- Default: (a). The rows say so. The parts that need no tables are written: stream detection (ADTS and LATM headers), the choice between the in-house decoders and the add-on, and the message when neither can decode.

## D-MEDIA-YTDLP (F-MEDIA-12): vendor checksum of the video-site extractor

- Why: the extractor is fetched from its vendor at the person's click, and its SHA-256 must be pinned in `legal/downloads.txt`. The pin cannot be obtained or verified offline, and a licence row without a code item fails `tests/licence_gate.rs`.
- Options: (a) fail closed until a human adds the row `yt-dlp | Unlicense | download-only | digest | sha256:<hex> | yt-dlp <version> ... | <date>` and the matching code item; (b) a floating pin (refused: nothing could prove the download is the vendor's).
- Default: (a). `ytdlp::pin_from` refuses while the row is missing, empty or not a SHA-256; nothing is fetched and nothing runs. Human step: add that row and its Store entry, then run the licence gate.

## D-GEMVOICE-OPEN-STATE (F-GEMVOICE-40): board, dashboard, flow and task tools in a voice session

- Why: these tools act on "the open" project, dashboard, flow or task list. A chat has that state; a voice session has none, so a spoken request has to say which one it means.
- Options: (a) not offered by voice (today); (b) offered on the last project, dashboard or flow the person had open in its tile, said back before acting; (c) offered by name only ("the flow called nightly backup"), each change asking on a card as in the chat.
- Default: (a). The job tools and the pending-work ledger do not depend on open state and are offered (`tool_chain::session_state_tools`).

## D-WAKE-SHARED-MIC (F-VOICE-60): the wake phrase while a page or the Voice tile has the microphone

- Why: the wake listener reads this machine's microphone; a page (`getUserMedia`) and the Voice tile's recorder open it too, and a sound card opened twice fails or gives one of them silence.
- Options: (a) the wake listener gives way while another holds the microphone and says so (default; written: `iyakki_shared::wakeshare`); (b) the helper's shared microphone plugin and the Voice tile publish their sound to the listener too, so the phrase is heard during a call or a recording (needs a tap in the C plugin and a streaming recorder in the Voice tile).
- Default: (a). The phrase is not heard while a page or the tile has the microphone, and Settings says why (`wakeshare::why_paused`).

## D-CAM-TRUST (F-CAM-20): the certificate of an encrypted camera

- Why: cameras carry self-signed certificates, so the public authorities never vouch for them; the row said the trust rule was an owner decision.
- Options: (a) the person sees the certificate's fingerprint on a card and approves it once; it is then the only certificate that camera may show (a pin); (b) trust on first use without a card; (c) refuse every certificate the public authorities do not know.
- Default: (a), written: `iyakki_rtsp::trust::judge` never accepts an unknown certificate on its own (it asks, with the fingerprint) and refuses a pinned camera that shows another one; the chat tools `camera_trust_status` and `camera_trust_set` ask every time, and only the fingerprint the camera showed can be approved.

## D-SITEAPP-TILE (F-BROW-100): where a site app lives on the screen

- Why: the shell's launcher tiles are a fixed set; a site app per site would need the launcher to take tiles from a list the person grows.
- Options: (a) a site app is a tab of the Browser tile with its own engine, profile folder and origin scope, listed on the Browser start page (written); (b) a launcher tile per site app, with its own window and entry in the task list.
- Default: (a). Everything a tile of its own would need (profile, scope, add-on card, plain message about protected video, audit) is already per site app, so (b) is the launcher work only.

## D-SECURITY-COUNTS (F-SDK-205): whose count a security alert is

- Why: a failed sign-in is written to the audit log with no person (the name tried may not exist), so it cannot be counted per person.
- Options: (a) tell every signed-in person the machine-wide count of failed sign-ins and newly paired devices, counts only, never names or addresses (written: `appevent_feed::audit_totals`); (b) tell only administrators; (c) count only failures for existing accounts.
- Default: (a). The payload carries a number and a kind word, nothing a person could use against another; an app still needs the `events.security` scope the person approved.

## D-BRAILLE-PROTOCOL (F-A11Y-07): which braille displays are driven

- Why: braille displays speak many protocols; writing each one without a device to test is guessing.
- Options: (a) only displays that follow the USB HID braille standard (usage page 0x41), read from the display's own report descriptor (written: `iyakki_devctl::braillehid`); (b) also a serial or vendor protocol per make; (c) a third-party daemon.
- Default: (a). Standard displays work with no table per make; vendor protocols are added one at a time when a display is in hand, and no third-party daemon is shipped.

## D-DATASCI-PINS (F-FLOW-31): who fills in the toolkit's download table

- Why: the toolkit's install checks every downloaded file against a pinned address and SHA-256 (`iyakki_shared::datasci::PINS`), and a hash is never guessed. The plan holds only the first sixteen hex characters of each hash and cut-off addresses, so the full table has to be made from the vendors' files.
- Options: (a) the table ships empty and the item fails closed with a plain sentence and no download until the owner fills it (written); (b) the install looks the hashes up on PyPI at the click (the hash then comes from the same place as the file, which proves nothing); (c) ship the wheels in the image (refused: licences, see the plan).
- Default: (a). The owner step is written in the header of `crates/iyakki-shared/src/datasci.rs`: download each file, run `sha256sum`, compare with the vendor's published hash, write the address and the 64 hex characters into the row. Until then Store, Setup shows the item and its click ends at "its download table is not complete".

## D-DATASCI-ONE-ITEM (F-FLOW-31): one click or two for the toolkit

- Why: the plan names two items (scikit-learn/NumPy/SciPy and OpenCV, the biggest download with the notices that matter most).
- Options: (a) one item "Data science toolkit" that installs both and shows all the licence notices at the one click (written); (b) two items as the plan says.
- Default: (a), because the two sample flows (Scikit-learn and OpenCV) both need it and one missing half would stop a flow at its check step. Splitting later only moves a few rows of the pin table into a second item.

## D-BRAILLE-DEFAULT-OFF (F-A11Y-07): braille output without the person asking

- Why: a display on a USB port is found by its descriptor, so the shell could start using it by itself.
- Options: (a) off until the person turns it on (Settings, Accessibility, key B, or the assistant's `accessibility_set`) and the reader is on too (written); (b) on whenever a display is present.
- Default: (a). Nothing is written to a device the person did not choose to use; a blind person finds the switch next to the screen reader's.

## D-SHELL-PARALLEL-DRAW (F-SHELL-64): splitting the drawing across cores

- Why: the profile of 2026-11-16 showed the cost was repeated painting, fixed by F-SHELL-66 (28.7 ms to 7.4 ms on the AMD host); a recording layer for `Gfx` would touch every draw call.
- Options: (a) not built; look again only if the Mac mini, measured with the same harness, is still over 16.7 ms after F-SHELL-66 (default); (b) build the recording layer now.
- Default: (a). The row stays at Impl 0 and says why; nothing is deprecated.
- 2026-11-18 (lane d1): the lane brief asked for (b), so it is built, default OFF (Look key `parallel_draw`). Row F-SHELL-64 is Impl 100; the Mac mini measurement stays open. Owner can still pick (a) in effect by leaving the key off.

## D-A11Y-LINT-STRICT (F-A11Y-06): when the store refuses a tile with unnamed parts

- Why: the access lint finds clickable parts no node names; refusing such tiles before the tiles the store hosts have caught up would block people.
- Options: (a) warnings only, with the review showing the findings and no date set (`iyakki_tile_host::lint::STRICT_FROM` is `None`; written); (b) a fixed date after which the install is refused.
- Default: (a). The review answer carries `strict_from` so an author sees the date as soon as the owner sets one.
## D-LOCALE-SOURCE (F-SDK-206): which language `locale.changed` reports

- Why: this release has no per-person language setting (English first); the event needs one source of truth.
- Options: (a) the machine's one language: a `locale` file the owner puts in the data folder, else `LC_ALL`, else `LANG` (written: `appevent_more::machine_locale`); (b) a language each person chooses in Settings (needs a new setting, UI and migration); (c) the browser's language per client.
- Default: (a). Every person hears the same value; when a per-person setting exists the source is replaced and the payload stays the same.

## D-GRANT-END (F-SDK-217): where a time-limited grant keeps its end

- Why: an app's grant had no end time; the event word `expiry` needs one, and the broker must agree with the event.
- Options: (a) the approval's own extra field `ends` (capability name to Unix seconds), read by `Approval::ends` and enforced by `Approval::holds_at` in the broker (written; no card or chat tool sets one yet); (b) a separate file of end times (two files to keep in step); (c) no time-limited grants.
- Default: (a). A value that is not a whole number is ignored; an older release keeps the field and writes it back.

## D-FW-OUT-ADMIN (F-FW-10): where outgoing rules are managed, and what a change costs

- Why: the outgoing, forwarding and port-forward rules can cut this computer off, and the System group has no FIREWALL tile yet (a tile needs a new entry in the policy's app table and a shell tile).
- Options: (a) Settings -> Firewall -> Advanced (the Outgoing form) and the chat tools `firewall_outgoing_status` / `firewall_outgoing_set`, every change a tightening with the 60-second Keep/Undo (written); (b) a FIREWALL tile of the System group as well.
- Default: (a). The tile is only another view of the same engine and waits for the launcher work.

## D-VPN-POLICY-APPLY (F-VPN-08): when policy routing for HYBRID filters goes in

- Why: policy routing marks packets by destination or account and changes where a person's traffic leaves; a wrong plan can send everything into a tunnel that is down.
- Options: (a) the plan is applied by the root helper only when the tunnel is up and its mark and table are the defaults, and taken out with the tunnel (written: `InstallPolicy` / `RemovePolicy`; the manager's call is the next step); (b) applied at boot for every saved profile.
- Default: (a). Excluded destinations (the LAN, the VPN server) are always listed first, account 0 is never marked, and a fetch the filters send into the tunnel that cannot be marked fails with a sentence instead of leaving outside the tunnel (`tunnel_connect::connect_for`).
## D-SB-DIRECT-ENROL (F-BOOT-12): who may write Secure Boot keys into the firmware

- Why: enrolling our key as PK, KEK and db makes the key's owner the owner of the computer's Secure Boot; a wrong write can leave a machine that starts nothing.
- Options: (a) only in firmware setup mode, never replacing an existing platform key, the write asking on a card every time and naming the machine; user mode is sent to the firmware's own screen or to the shim route; (b) also clear the keys from the running system (not offered: not every firmware allows it and it cannot be undone from inside).
- Default: (a). `iyakki_usb::sbenroll::judge` opens only setup mode; the other states return the words that send the person to the firmware screen. Nothing is written by the formats module.

## D-HW-IMAGE (F-PKG-04): an image that carries firmware blobs

- Why: the decision of 2026-11-06 (F-REL-08) is that no firmware ships in the release image; a `hw` image with blobs would contradict it.
- Options: (a) no such image; the person downloads firmware by a click (today); (b) a private, never published image for the owner's own machines.
- Default: (a). The row stays blocked at Impl 0 and nothing is built; the owner may choose (b) by saying so.

## D-SB-MS-SHIM (F-BOOT-11): our own Microsoft-signed shim

- Why: needs a Microsoft developer account, an extended-validation certificate and a public review; the owner decision of 2026-11-06 is route 1 only.
- Options: (a) not pursued for 1.0, labelled unsupported (decided); (b) start the shim-review application.
- Default: (a). Nothing is built; the row stays planned at 0.

## D-TILE-CRATE (F-ARCH-08): a separate tile crate for faster builds

- Why: the measured gain is 9 to 14 s on a tile change, and it adds a maintained interface between two crates.
- Options: (a) do not split (the gain is small); (b) split after the core split of F-ARCH-07 is finished, when the interface exists anyway.
- Default: (a). The row stays planned at 0; the split is reconsidered after F-ARCH-07.

## D-WINMAC-INSTALLER (F-INST-12): the installer on Windows and macOS hosts

- Why: the Win32 calls cannot be compile-checked on the build hosts (no Windows target or linker) and the macOS planner needs a macOS disk reader proven on a Mac.
- Options: (a) the stick writer first (F-PKG-07/08 pure parts, written), the planner on those hosts later; (b) a full planner on both before alpha.8.
- Default: (a). The row stays blocked at 0.

## D-4KN-BIOS (F-INST-29): legacy BIOS boot on 4096-byte-sector disks

- Why: GRUB's boot sector assumes 512-byte sectors; a patched boot sector belongs to our own loader plan and needs a real 4Kn disk with a BIOS that boots it.
- Options: (a) refuse `--bios` on 4Kn with a plain sentence and keep UEFI only (today); (b) wait for the own loader's BIOS sector.
- Default: (a). The planner refuses; the row stays blocked at 0.

## D-APPLE-VM (F-APPLE-02) and D-ASAHI (F-APPLE-03): Apple Silicon routes

- Why: route 3 (a light Linux VM for k3s on macOS) and the native Asahi variant both need an Apple Silicon machine to prove anything, and Asahi does not cover M3 and later.
- Options: (a) the VM route first, documented in docs/APP-MODE-AND-APPLE-SILICON.md, no code until an Apple Silicon test machine is in the lane; (b) Asahi first.
- Default: (a) for the VM, Asahi later; both rows stay planned at 0.
## D-CLIP-BRIDGE (F-WEB-58): how text moves between a web tile and the browser's clipboard

- Why: the tile's copy slot lives on the service; the browser's clipboard needs the person's own key press and the browser's permission.
- Options: (a) the page carries the text on the tile events call only after the person's own paste and writes the tile's copies to `navigator.clipboard` when the browser lets it (written); (b) a separate clipboard route the page polls; (c) no bridge.
- Default: (a). No text moves without the person's own paste or copy key; the text is never in an audit line and a refusal by the browser is said on the tile.

## D-SHARED-CAM (F-CAM-19): one connection for the recorder and the live view

- Why: cameras that allow one connection could not be recorded and watched together.
- Options: (a) the recorder holds the connection and the live view looks through it (written); (b) a broker that owns the connection for both in either order; (c) record only while nobody watches (before).
- Default: (a). When the live view was first, the recorder still opens its own connection; (b) is the later step.

## D-KERNEL-OWNER (F-ARCH-07): the one owner of kernel access in the single-reader ratchet

- Why: a kernel reader that moves into `iyakki-core-local` would otherwise count as a new copy in a new place, and the ratchet could never go down by moving files.
- Options: (a) `iyakki-core-local` is the owner: its files are not counted as copies, and the pairs of files that moved there leave the allow list; (b) keep counting it and add each moved file to the allow list (the list would then not shrink).
- Default: (a). Only lines are removed from `data/kernel_readers.allow`; a file outside `iyakki-core-local` that reads a kernel path still has to be on the list.

## D-UNSIGNED-STICK-UPDATE (F-UPG-05): an update folder with no signature written to a stick

- Why: `iyakki-usb --update` replaces boot files without mounting the stick; a pack nobody signed could be anything.
- Options: (a) a wrong signature is always refused; a missing one is refused unless `--allow-unsigned` is given; (b) refuse every unsigned pack (a person's own pack could not be written); (c) never check (the old behaviour).
- Default: (a). The release key is the one that signs the downloads manifest; the update centre on the device keeps its own rule (it asks the person).

## D-WIN-WRITER-UNRUN (F-PKG-07): Win32 code written without a Windows machine

- Why: the build hosts have no Windows machine; the Rust compiler can still type-check the Windows file for the Windows target.
- Options: (a) write the calls, type-check them for `x86_64-pc-windows-gnu` in a scratch crate, keep the Manual row `needed`, and do not wire a command switch (the crate still builds for Unix only); (b) write nothing until a Windows machine exists.
- Default: (a). Nothing runs on real hardware; the file says so in its header.
## D-BACKUP-04b: where the share password is read from by the chat tool (F-BACKUP-04)

- Why: the agent's chat tools have no unlocked vault at hand and the password must never go through the chat.
- Options: (a) a credentials file the person made inside their home (user on line 1, password on line 2), the same way a passphrase file works; (b) the vault entry `backup-share/<name>` read through the vault facet; (c) typed in Settings only.
- Default: (a) for the chat tool today. The mount still gets the password only through a 0600 file inside a private folder for the length of the mount. (b) stays the Settings route and is wired when the Settings field is built.
## D-PASSKEY-PAGE (F-BROX-03): who may use a passkey when a web page asks

- Why: the page-facing `navigator.credentials` reaches the same vault the assistant's passkey tools use; a page must not be able to sign in or make a passkey on its own.
- Options: (a) a card that names the origin, the site and the account on EVERY use, never covered by a standing grant, and a build with no card refuses everything (written); (b) ask once per site and remember; (c) no page access, assistant tools only.
- Default: (a). A page that names a site other than its own host or a parent of it is refused before the person is asked; conditional mediation, security keys and user verification are not offered; the shell's own card and the setting of `Sites.passkeys` are the next step, so until then pages in the shell see no `navigator.credentials`.

## D-PDF-TABLES (F-PDF-18): how tables found on a PDF page reach the assistant

- Why: table analysis finds rows and columns from the positions of the words; a wrong guess in prose would change what the assistant reads.
- Options: (a) tables are written as Markdown tables only when the caller asks (`pdf_text` with `tables: true`), the plain text stays the default (written); (b) always; (c) never.
- Default: (a). A block needs three rows, two columns and most rows in two or more columns; anything less stays text.

## D-BROWSER-PACKAGE-LOOKUP (F-BROW-40): where the engine of an installed browser package is found

- Why: the browser engine is to leave the base image and be installed from the store on the person's say-so.
- Options: (a) the image's helper first, then the installed package under the system data folder, then (desktop mode only) the checkout's script; a person's own override still wins (written); (b) the package first.
- Default: (a), so an image that still carries the helper behaves exactly as before; the package runs in the same small private environment as the image's helper.
## D-VPN-MARK-RIGHTS (F-VPN-08): what a marked connection does when the session may not mark sockets

- Why: marking a socket (`SO_MARK`) needs `CAP_NET_ADMIN`; the target is a session without administrator rights. A fetch the filters send into the tunnel must never leave outside it.
- Options: (a) the reader browser and HTTP client ask the marked connection and, when it cannot be made, the fetch fails with a sentence (written: `tunnel_connect::dial_hook` through `iyakki_proxy::dial`); (b) fall back to a plain connection and warn; (c) the root helper opens the connection and passes the descriptor to the session.
- Default: (a). (b) would leak what the person asked to protect; (c) is the planned end state and needs the helper to carry descriptors, so it waits for the non-root session work of F-VPN-05.

## D-VPN-ROAM (F-VPN-05): what a live tunnel does when the computer changes network or wakes up

- Why: the route to the VPN server and the server's idea of this computer's address go stale when the Wi-Fi changes or the computer sleeps.
- Options: (a) rebuild the tunnel at once (all pieces taken away, the guard of a kill-switch or fail-closed profile kept, then the normal connect) when the default interface or gateway differs from the one the tunnel was built on, or when two looks at the tunnel are more than 120 seconds apart (written: `Manager::roamed_or_resumed`); (b) only wait for the handshake to go stale (180 seconds).
- Default: (a). The cost of a needless rebuild is a second of reconnecting; the cost of waiting is minutes without the tunnel.

## D-BROTLI-DATA (F-BROX-19): where the Brotli static dictionary and its transform tables come from

- Why: the decoder for WOFF2 web fonts needs the 122,784-byte static dictionary, the 121 word transforms and the context look-up table of RFC 7932. They are data of the standard that cannot be derived from a formula and are not worth typing from memory (one wrong byte breaks the dictionary words of real fonts).
- Options: (a) read them out of the system's Brotli library (MIT licence, Copyright the Brotli Authors) with a script that is in the repository, keep the data files with a licence row and the MIT text; (b) wait for a person to enter them from the RFC's appendices; (c) leave WOFF2 out and keep the common WOFF 1.0.
- Default: (a). MIT is permissive, no code of the library is copied (the decoder is written here), the dictionary file is checked in the tests against the hash of the standard's, and `legal/assets.txt`, the notices and `legal/extra-licences/brotli-reference/LICENSE` carry the notice.
## D-DRIVERS-15 (F-DRIVERS-15): keep or deprecate the optional `macweb` image variant

- Why: the variant is written (`build.sh macweb`, fragment, kernel gate) but never built or booted; the web image may already serve the Intel Macs. Rows are never deprecated by an agent.
- Options: (a) keep it as an optional variant, built only on request, never part of the default release, Haswell on software rendering (llvmpipe, said in words), Mesa's crocus only by the explicit `WEB_CROCUS=1`; (b) deprecate it in favour of the web image; (c) make it a default release image.
- Default: (a). Nothing changes for the default images; the row counts the code that exists and the first build and boot stay Test/Manual. The owner may choose (b) later; the code is then removed in one commit.

## D-MODPACK-URL (F-PKG-11): where the optional kernel-module pack is downloaded from

- Why: the optional modules leave the image and come back by the person's click; the address is the owner's to publish.
- Options: (a) no address in the code; the person (or the owner's channel setting) names it and the manifest in the image checks every file by size and SHA-256 (`iyakki_drivers::modpack::install`); (b) a fixed official address.
- Default: (a). Nothing is fetched until an address is set; a file that does not match the manifest is never written.
## D-RAG-17: refreshing kept web pages by itself (lane w5)

* Question: may the device read the web again on a timer, while nobody is typing, to refresh the pages a person kept?
* Options: (a) never by itself: only the person's or the assistant's call (today); (b) a switch per collection, off by default, runs only while the person is signed in, at most once a day, never more than the 20 pages and the account's request budget of one manual refresh, one audit line, a card that says so when the switch is turned on; (c) always on.
* Default chosen: (a) stays in force. (b) is the design to build when the owner agrees; it needs a facet call for the refresh and a shell tick like the backup schedule. Not built in alpha.7. Rows: F-RAG-17.

## D-BACKUP-SCHED: the schedule may write to a share and run incremental chains (lane w5)

* Question: how does the scheduled backup use a network share and incremental files?
* Options: (a) the schedule names the share address and a credentials FILE the person made (the password is never in the settings or the mount line), and `full_every` N: a full file, then links until the chain holds N files, then a new full file; old chains are removed whole beyond `keep`; (b) a vault entry for the credentials (D-BACKUP-04's first default).
* Default implemented: (a). Rows: F-BACKUP-04, F-BACKUP-06.

## D-LAUNCH-25-MENU (F-LAUNCH-25, lane d4): how a Look's menu list applies to the IYAKKI menu

* Question: the Look's menu list names Home, Apps, Files, Chat, Settings, Lock, Sign out, Power; the shell's menu has other entries (About, Quick install, Media, Linux apps, Mac drivers, Developer mode, Cloud).
* Options: (a) a Look with a menu of its own adds the seven actions beside the machine's entries, ordered by the Look, Settings in the Look's place; the default list changes nothing; (b) replace the machine's entries by the Look's; (c) never apply.
* Default implemented: (a). The machine's own entries are never hidden by a Look; the essentials stay in every list.

## D-SDK-204-CAPTIVE (F-SDK-204): captive portal events

* Question: may the event tick probe the network for a sign-in page?
* Options: (a) no traffic of its own: tell it only from the check the person chose to run (a check address in Settings -> Network); (b) probe on a timer.
* Default implemented: (a). `connectivity.changed state=captive_portal` is told only on machines where that check runs.

## D-SDK-205-PLACE (F-SDK-205): what "a new place" is for a vault unlock

* Question: the vault has no client address at unlock.
* Options: (a) the place is the network the machine is on (default route interface and gateway), kept hashed per person; (b) the paired device that asked; (c) a GPS or Wi-Fi name.
* Default implemented: (a): counts only, 32 newest places kept, first unlock only records.

## D-GEMVOICE-40-NAMING (F-GEMVOICE-40, lane d4): how a spoken request names an open tile

* Question: a voice session has no open project, flow, dashboard or task list; how does a spoken request name the thing it means?
* Options: (a) the project opened last in the IDE and the flow open last in FLOWS (the rule the CHAT tile already follows for a flow), only for accounts that may run commands; the dashboard and task tools stay out; (b) the person says the name and a tool binds it for the session; (c) none by voice.
* Default implemented: (a). Same grants and approval cards as the chat; every change still asks.

## D-GEMVOICE-08-HOLD (F-GEMVOICE-08, lane d4): press-to-talk release

* Question: what does letting go of the start key mean?
* Options: (a) a key kept down about half a second after it started the session is press-to-talk: the release hands the turn over; a tap starts a session that stays open; (b) every release hands over; (c) no release event.
* Default implemented: (a).

## D-VOICE-43-CLIENT-ID (F-VOICE-43, lane d4): where the Alexa client id lives

* Question: the AVS client id and product id belong to the owner's registered product (Amazon no longer opens AVS to new products).
* Options: (a) a file `alexa-avs.json` in the data folder, written by the owner on the machine; no file, no Alexa; (b) built into the image; (c) a vault entry.
* Default implemented: (a), failing closed with a sentence (`avs_h2::NO_CLIENT_ID`). No id is in the code and nothing is dialled without one.

## D-VOICE-61-SEAM (F-VOICE-61, lane d4): the shared crate between the agent and the connector

* Question: the connector crate must not depend on the agent, yet both run the device end of the cloud sound stream.
* Options: (a) a small crate `iyakki-audiolink` (frames, pinned TLS link, consent gate, uplink, device loop, certificate offer, `Mic` trait) that both use; (b) copy the code into the connector; (c) make the connector depend on the agent.
* Default implemented: (a). `iyakki-shared::wakelisten::Mic` is a re-export of the crate's trait.
## D-4KN-BIOS update (lane d3, 2026-11-18)

* The default (a) stays: without the 4096-byte-sector boot code file the planner refuses `--bios` on 4Kn disks in plain words. The byte handling for the day the file exists is written (`iyakki_usb::bios4kn`); the contract for the boot code is its end marker `IYAKKI-4K-BOOT\x01\0`. Rows: F-INST-29.
## D-AAC-ADDON: AAC through the person's ffmpeg add-on (F-RADIO-12, F-RADIO-16, F-NET-29)

* Question: owner direction 2026-11-19 on D-RADIO-AAC.
* Chosen: option (a). AAC (radio and Bluetooth) is decoded and encoded by the ffmpeg of the audio and video add-on, which the person installs at their click (LICENSING.md items 10 and 11). No in-house AAC decoder or encoder, no codebook tables typed in, no aptX (patents and the licence rule).
* In-house only what needs no tables: stream detection (ADTS, LATM), the choice between in-house decoders and the add-on, the plain message when neither can decode, the A2DP AAC capability bytes, LATM and ADTS framing.
* Bluetooth: the sink offers an AAC endpoint only when the add-on is installed; the source picks AAC only when the add-on is installed and the headphones offer it, else SBC. Without the add-on the Settings line says so. The ffmpeg process starts with fixed arguments and no shell, only when a stream is started (every start asks as before, nothing starts by itself).
* Rows: F-RADIO-12, F-RADIO-16, F-NET-29 re-scoped (ids kept, history in Notes).

## D-BROW-PLUGIN-CAP (F-BROW-90): when the person is asked about a browser plug-in

- Why: the extensions plan marks `web.content.script` as a hard ask (a card at every use). A person's own script on a site they named has no use at which a card could appear: it runs while the page loads.
- Options: (a) the card is shown when the plug-in is added and again when it is switched on (both are chat tools that ask), and the catalogue entry is an ordinary one held by the signed built-in `browser-plugins` through the table; (b) a card at every page load that matches.
- Default: (a). Plug-ins are off when added, none exist by default, and every change is audited. A store-installed plug-in package from another author is a different case and would be a hard ask when that path is built.

## D-GPU-02: the ROCm images are the owner's to pin (lane d5)

* Question: which images does the `gpu-rocm` Quick install entry pull (AMD's device plugin, a ROCm test image, optionally the ROCm build of Ollama)?
* Options: (a) none in the code: the entry stops with a plain message and changes nothing until the owner puts digests in `iyakki_shared::gpu_rocm` after a test on a real AMD card; (b) float on tags (refused: a tag can be moved).
* Default implemented: (a). The step, the patches, the uninstall and the page are written; only the three constants are empty. Rows: F-GPU-02.

## D-LINAPP-15: the builder image of the on-device image builder is the owner's to pin (lane d5)

* Question: which builder image (buildah or similar) does `image_build` run, and which Linux capabilities does its job get?
* Options: (a) none shipped: `BUILDER_IMAGE` stays empty and an administrator sets one by digest with `image_builder_set` (asks every time); builds refuse in plain words until then; (b) ship a default pin chosen by the owner after a test on a real cluster.
* Default implemented: (a). The capability list in the job (drop ALL, add CHOWN, DAC_OVERRIDE, FOWNER, SETUID, SETGID, SETFCAP, KILL) is still a guess that a real cluster has to confirm (KI-851). Rows: F-LINAPP-15.

## D-TCFG-10: the cloud profile offers k8s_apply_yaml through the team's deploy rules (lane d5)

* Question: may a cloud caller change a cluster with `k8s_apply_yaml`, and under whose rules?
* Options: (a) not at all in the cloud profile (the tool stays on the desktop); (b) offered, asks on a card every time, a cluster bound to a team environment goes through `PerTeamGate` (approval request, one-apply spend, audit) judged by the caller's own role in that team, and a cluster with no binding is left to the card.
* Default implemented: (b). The reviewed tool list of the cloud profile gains exactly this one name. Rows: F-TCFG-10.

## D-VPN-1b (F-VPN-06): where the setting `vpn.openvpn_download` lives (x6, 2026-11-19)
Question: Settings has no key-value store; where does the address and checksum of D-VPN-1 (a) go? Options: (a) a new field in `settings.json`; (b) a small file `cli/sources.json` next to the programs it names, changed by an administrator-only chat tool that asks each time. Default: (b): `<https address> <sha256>`, https only, the download stays on that host, a mismatch deletes the file, empty or damaged means no download. Implemented; the owner may still supply the real address and checksum.

## D-NET-07-ADAPTER (F-NET-07): where the chosen WiFi adapter is kept (x6, 2026-11-19)
Question: with two WiFi adapters, who chooses and where is it kept? Options: (a) always the first; (b) a setting in `settings.json`; (c) a line in `wifi-adapter.txt` beside the saved networks, read by the page and the chat tools alike. Default: (c): the Network page key `v` and the tool `wifi_adapter_set` (asks) write it; a name that is gone falls back to the first adapter; the hotspot's own interface is never offered. The sign-in page opens only on key `o` (or the person's own request) and only for an http(s) address the sign-in check found; the check address itself stays the person's choice (`wifi-probe.txt`, no default, nothing sent without it).
## D-SPLIT-03-X1: member stores reach the shell as trait objects one at a time; the zone travels as data (lane x1)

* Question: how does the shell stop holding concrete stores so it can run over the socket, and how do zone-taking store methods cross it?
* Options: (a) one store per commit, `ServicesHandle::<store>()` returns `Arc<dyn XApi>`, the generator keeps the zone-taking methods and `iyakki_clock::Zone` derives serde; (b) leave the zone-taking methods inherent (the shell then keeps the concrete store); (c) change `CalendarStore` parameters of the study functions at once.
* Default implemented: (a) for usage, secret requests, device watch (with the inbox). Calendar and study wait for (c): the study functions take `&CalendarStore`, so they move together. Rows: F-SPLIT-03.
## D-4KN-BIOS update 2 (lane x5, 2026-11-19)

* The default (a) stays fail-closed: `--bios` on a 4096-byte-sector disk is refused unless the person passes `--bios-4k-boot-code`. The boot code now exists in the repository (`dist/boot4k/boot4k.S`, built by LLVM's assembler, permissive licence) and `--bios-4k-boot-code builtin` selects it; no default picks it. Whether it starts a real machine is unproven until a BIOS (CSM) boots it on a 4096-byte-sector disk. Rows: F-INST-29.
## D-CLIENT-05-A (lane l2, 2026-11-19)

* Question: the row asks the Cameras tile to take an EXCLUSIVE hold when it records. The recorder reads network cameras only, and its one capture is shared with every viewer.
* Options: (a) exclusive hold (shuts remote clients out of a recorded camera); (b) shared local hold named Recorder (shows who looks, keeps viewing).
* Default implemented: (b), in `camrec::run_worker`. Exclusive stays for programs that need the device alone (`Hub::hold_local`). Rows: F-CLIENT-05.
## D-VOICE-38-A (lane l2, 2026-11-19)

* Question: how does the cloud machine tell a connector where its audio listener is?
* Options: (a) a settings field; (b) the environment variable `IYAKKI_VOICE_AUDIO_ADDR` (`host:port`), read when the first offer is asked; (c) derive it from the web address.
* Default implemented: (b). With no address, or with the switch off, there is no offer and nothing listens (fail closed). The certificate is made for this link alone and offered only inside the signed-in call. Rows: F-VOICE-38, F-VOICE-61.

## D-BROXY-AUTH: where a browser proxy's name and password live (lane 4)

* Question: the browser proxy setting (F-BROX-22) can reach an HTTP proxy; a proxy may want a sign-in. Where is that secret kept?
* Options: (a) not stored: the setting is `host:port` only and a text with a sign-in is refused in words; (b) in the person's vault, read by the shell when the browser starts; (c) in a 0600 file next to the setting.
* Default implemented: (a). A secret is never typed into the chat and never kept in a plain file; the proxy client code (`iyakki-proxy`) already sends a Basic sign-in when given one, so (b) is a settings-screen and vault change only. Rows: F-BROX-22.

## D-BPKG-SOURCE: where the optional browser package is fetched from (lane 4)

* Question: from which address does `browser_package_install` download the engine, now that the official store address is empty?
* Options: (a) only from a store source the person configured with a pinned key (https), at `<store>/browser/<arch>/manifest.json` and `webview-helper`; (b) a built-in address; (c) any address the person types.
* Default implemented: (a). With no pinned store the tool says so in words and installs nothing; the signed description, size and checksum are checked as before. Rows: F-BROW-40.

## D-BADSITES-SOURCE: where the known-bad site list comes from (lane 4)

* Question: which malware list does the browser check, and who updates it?
* Options: (a) none shipped; the person's own list, with `browser_badsites_download` fetching the suggested URLhaus host list or any https address only when the person asks (already written); (b) ship a list in the image; (c) the project hosts a list and the browser fetches it on a schedule.
* Default implemented: (a). Nothing is shipped or fetched in the background. Rows: F-BROW-98.

## D-QA-05-ZSTD (F-QA-05, lane l5, 2026-11-19): the image scan opens a `.zst` by itself

- Why: the gate depended on the build machine's `zstd` program, and `iyakki-relcheck` had no dependency at all. The owner's rule is in-house code or permissive licences.
- Options: (a) add the pure-Rust `ruzstd` (MIT, already a dependency of the archive, update and delta code) to `iyakki-relcheck`; (b) write a zstd decoder in this crate (about 1,500 lines of Huffman and FSE code that would repeat what ruzstd already does and is tested for); (c) keep the build host's `zstd`.
- Default: (a). `iyakki-relcheck` now has exactly one dependency; the lock file records it; the gate script uses `iyakki-relcheck unzstd` for its home-folder check, so no zstd program is needed. Nothing is written to disk while scanning.

## D-WIN-WRITER-SWITCH (F-PKG-07, F-PKG-08, F-INST-12, lane l5, 2026-11-19): the Windows writer program is wired, fail closed

- Why: D-WIN-WRITER-UNRUN (default a) left the Windows calls unjoined because the crate did not build for Windows. The owner's lane brief asks for the portability pass; the standard `x86_64-pc-windows-gnu` Rust target is installed on the development PC only (official toolchain, `rustup target add`).
- Options: (a) cfg-gate the Unix-only parts (`cfg(unix)` with small portable stand-ins) so the planner and writer crates compile for Windows, and join the existing Win32 steps to the program and the window; (b) leave the crate Unix-only.
- Default: (a), with these fail-closed rules: Windows writes only a ready image (ISO or disk image) to a drive named with `--erase`; the install layout (partitions) and applying a plan file are refused on Windows until the planner's Windows writer exists (F-INST-12); "am I root" is false there, the Win32 steps check elevation themselves and the program starts itself again through the UAC prompt once. Nothing ran on a Windows machine; Manual stays `needed`.

## D-MACOS-PLANNER-WRITE (F-INST-12, lane l5, 2026-11-19): how the planner writes on a Mac

- Why: a Mac's raw device `/dev/rdiskN` accepts only whole-sector transfers; the planner writes smaller pieces. A Mac also keeps APFS volumes in synthesized containers that stay mounted when only the physical disk is unmounted.
- Options: (a) write through the buffered block device `/dev/diskN` and unmount the APFS containers of the target first (never forced); (b) a sector-aligned adapter over the raw device; (c) keep the planner Linux-only.
- Default: (a). Only external physical disks are listed (the internal drive and virtual disks are never targets); a busy volume stops the install before anything is written. Not run on a Mac with a spare external disk.

## D-UKI-STUB (F-BOOT-02, lane l5, 2026-11-19): the stub of the unified kernel image

- Why: a unified kernel image needs a stub (the program that starts the kernel from its own sections). Choosing and building it is a boot-path decision and nothing here may boot a real machine.
- Options: (a) write the section writer and the signing step in-house and take the stub as an input (`IYAKKI_UKI_STUB`), building a unified image only when one is named; (b) pick systemd-stub (LGPL) as the default; (c) make our own loader the stub now.
- Default: (a). GRUB stays the default way in; without `IYAKKI_UKI_STUB` the images are built exactly as before. The stub choice and a QEMU/OVMF boot of the result are open.

## D-LUKS2-EXPORT-PASSWORD (F-STORE-05, lane l5, 2026-11-19): who types what for a LUKS2 export

- Why: the volume key lives only inside `iyakki-enchome` and must never reach the model, a log or an argument; the export also needs a password for the new LUKS2 header.
- Options: (a) the new LUKS2 passphrase is the person's own home password, typed into the existing masked form on the person's screen (the chat tool only asks for the form); the recovery key and the administrator slot are not carried over; (b) a separate export password typed twice; (c) a chat argument (refused: secrets never pass through the model).
- Default: (a). The export is only made from a LOCKED home (an open home's blocks are changing), never overwrites a file, is mode 0600, and is read back (the header must give the same key, every payload byte must match) before it counts; a failed export deletes its partial file.

## D-BIOS-CHAT (F-INST-13, F-INST-29, lane l5, 2026-11-19): the chat tools for a legacy BIOS install

- Why: the installer chat tools only shrank partitions; `--bios` with `--bios-4k-boot-code builtin` had no chat route.
- Options: (a) `install_bios_plan` (free, administrators, writes nothing) and `install_bios` (a hard ask: the plan on the card every time, the drive's name typed again as `confirm`, no standing allow, never Auto), both through the installer's own planner and writer, free-space mode only, the old boot code saved first; (b) a tool that picks the disk itself.
- Default: (a). The disk is always named by the person; the 4096-byte-sector boot code is passed only as `builtin` and is used only when the disk's sectors are 4096 bytes (the planner refuses otherwise or without a BIOS boot partition).

## D-UPD-OFFICIAL-KEY (F-UPD-01, lane l5, 2026-11-19): the official update key and channel address

- Why: the official release key and channel address are the owner's (key ceremony, hosting).
- Options: (a) no official address and no key compiled in: the update centre uses the release key the build has (the same one that signs the downloads manifest); with none, an update file or address is refused; the address is typed by the administrator (Settings -> System update -> Download update, or `updates_bundle_fetch`) and asks every time; (b) a fixed official address.
- Default: (a) (this is how the code behaves today). Nothing is fetched until the owner publishes a key and an address.

## D-SIDE-VS-LEAN (F-BOOT-25, lane l5, 2026-11-19): the side image and the fast start both shorten the wait at the logo

- Why: the plan of a side image (big parts on the boot volume read by Linux's own USB driver) and the existing fast start (`boot/initrd-lean`, `boot/root.sfs`, docs/BOOT-SPEED.md) attack the same 20-second wait. Having both doubles the image steps (two cpios).
- Options: (a) keep the fast start as the way and the early side-image mount as an opt-in device side (`iyakki.side=1`, nothing sets it), build nothing more until a Mac measurement shows the fast start is not enough; (b) finish the side image (image split, GRUB switch, kernel squashfs/zstd, installer size) and retire the fast start; (c) delete the side-image code.
- Default: (a). The code is behind a command-line option no entry sets, so no image changes. The owner decides between (b) and (c) after the first measured Mac boot with the fast start.

## D-HOTSPOT-15: Bluetooth sharing is served in user space; no kernel BNEP, and pairing is a short visible window (lane l6)

* Question: does letting phones connect over Bluetooth need the kernel's BNEP (`CONFIG_BT_BNEP`, bridge) and a pairing screen on the sharing page?
* Options: (a) no kernel option: the server is our own code (L2CAP listener on PSM 15, the BNEP set-up and a learning bridge in `iyakki-devctl::nap_server`, a TAP interface `iyaknap0` handed to the sharing manager as one downstream port), and pairing is phone-initiated: when sharing starts the controller is made connectable with the "no input, no output" capability and visible for 180 seconds, then it is hidden again; (b) the kernel's BNEP with a bridge and `BNEPCONNADD`.
* Default implemented: (a). It needs no kernel change (TUN is already there for the client side), keeps phones isolated from each other in our own bridge (and by the firewall table), and the visible window closes by itself. Rows: F-HOTSPOT-15.

## D-HOTSPOT-22 default (a) taken: plain SAE, no hash-to-element (lane l6)

* Question: does the computer's SAE exchange offer the hash-to-element variant (status 126, a different password-element derivation)?
* Options: (a) plain SAE only (the hunting-and-pecking element, 40 fixed rounds, group 19): the beacon announces SAE without the H2E selector, a station that insists on H2E is refused by the exchange itself; (b) add hash-to-element beside it.
* Default implemented: (a). Every phone and laptop that does WPA3 also does plain SAE. Rows: F-HOTSPOT-22, F-HOTSPOT-18.

## D-MEDIA-YTDLP update (lane l6): the Setup card is data until the owner adds the pin row

* Question: how does the Store -> Setup card for the video-site downloader exist before the owner's pin row does? A Quick install item with no row in `legal/downloads.txt` fails `tests/licence_gate.rs`.
* Options: (a) the card is built by `ytdlp::card` (licence, licence text, vendor address, pinned version, short checksum, state) and shown wherever the person is asked (the approval card of `ytdlp_install`, `ytdlp_status`), with no button and a plain "not available yet" while there is no pin; the Setup list entry is added by the human step together with the row; (b) a Setup entry now, with a licence row without a checksum (refused: the gate and the rule that nothing floats).
* Default implemented: (a). Finished downloads go to the person's video folder and into the media index (`ytdlp::Library`). Rows: F-MEDIA-12.

## D-DATASCI-RUN-ARGV: the toolkit's Python runs isolated and finds its packages by a start-up line (lane l6)

* Question: the plan's run line uses `python3 -I` with `PYTHONPATH`, but isolated mode ignores `PYTHONPATH`. Which wins?
* Options: (a) keep `-I -B` and put the packages folder on the path with a fixed `-c` start-up (`sys.path.insert(0, '/opt/datasci/site')`, then `runpy.run_path` of the script in the work folder); (b) drop `-I` for `-s -B` and use `PYTHONPATH`.
* Default implemented: (a), so a stray environment variable or a user site folder can never change what the toolkit imports. No network (`--net off`), the toolkit read-only, one writable scratch folder, 1024 MB, 200 % of a processor, 64 processes, 512 MB of temporary space, the system-call filter on. Rows: F-FLOW-31.

## Pins filled by the owner's directive (2026-11-19, lane r2)

The owner said "address them"; each value below was read from the vendor's official HTTPS source on 2026-11-19 and nothing was invented.

* D-GPU-02: three ROCm digests pinned in `iyakki_shared::gpu_rocm` (Docker Hub registry v2 manifests API; the body hash was recomputed). Tags: `rocm/k8s-device-plugin:1.31.0.12`, `rocm/rocm-terminal:6.4`, `ollama/ollama:0.40.16-rocm`.
* D-MEDIA-YTDLP: row `yt-dlp` added (release 2026.10.26, SHA-256 from the GPG-signed SHA2-256SUMS, signature verified).
* D-LINAPP-19: Debian 12 and 13 archive keys pinned in `legal/debian-archive-pgp.asc` (two official sources, byte-identical). Release image digests are still not pinned.
* D-DATASCI-PINS: table filled for x86_64 glibc (python-build-standalone SHA256SUMS; PyPI JSON digests). aarch64 still empty.
* D-VPN-1: NOT filled. The five projects publish source tarballs only; the downloader needs a runnable program, so the setting stays the pin.
* Signed downloads manifest items (aws-cli, gcloud, esptool, ...): NOT filled; editing the manifest needs the owner's signing key (`dlmanifest sign`).

## D-SPLIT-03-SECRETS: the plain value of a secret never crosses the core socket (lane 1, 2026-11-19)

* Question: the `Secrets` store has `resolve(reference, user_vault)` that returns the plain value. May that be part of the trait object (`SecretsApi`) that the shell holds and that a remote UI would call over the core socket?
* Options: (a) put `resolve` in the trait (a remote UI could then read every key the core can; with the socket's `rpc` switch on that is a plain-text secret on a socket); (b) the trait carries only what does not hand out a value (`has_ref`, `test_provider`, `set_providers`, the unlocked flags); the screens that need the value (a key sent in a request header, a token for a mesh client) run in the core's process, not in a remote UI; (c) never move the secrets store.
* Default implemented: (b). `Secrets::has_ref` is new; `ServicesHandle::secrets_api()` returns `Arc<dyn SecretsApi>` and the key-present checks, the provider test and the provider list use it; `ServicesHandle::secrets()` (the concrete store with `resolve`) stays for the in-process value callers (Gemini calls, story, agents, mesh, model host) until they become core-side jobs, and a test keeps `resolve`, `Zeroizing` and any `String` return out of `SecretsApi`. Rows: F-SPLIT-03.

## D-SHARE-07-PUBLISH (F-SHARE-07): publishing a share under a name

* Question: what does the one-step "publish this share under a name" do, and for whom?
* Options: (a) administrators only: one card adds a domain route `NAME.local -> share port` (plain http, route access public because the share asks for its own link or password); a share for this computer only is refused; (b) any person, through their own routes; (c) also allow this-computer-only shares behind the route's member sign-in.
* Default implemented: (a), as the chat tool `share_publish`. HTTPS and the certificate stay with the domain listener (Settings -> Domains). Rows: F-SHARE-07.

## D-SDK-217-LIMIT (F-SDK-217): who may give an app grant a time limit

* Question: who sets a time limit on an app's grant and how long may it be?
* Options: (a) administrators, 1 hour to 366 days, only shortening what was approved (never granting), written into the approval's `ends` map; (b) any person for their own apps.
* Default implemented: (a): `apps_access_limit` (asks) and Settings -> Apps access `e`. Rows: F-SDK-217.

## D-SDK-34-02 (F-SDK-34): how the shell sends a pinch

* Question: does the pinch replace the Ctrl+plus/minus keys of the two-finger gesture?
* Options: (a) beside them: the keys stay for apps (PDF viewer), the pinch event goes to a WASM tile of ABI 3 only; (b) replace the keys for every app.
* Default implemented: (a); older tiles hear nothing. Rows: F-SDK-34.

## D-WEB-56-SECRETS (F-WEB-56): secrets in the Web Settings tile

* Question: may a person type a password, key or passphrase into a form of the Settings tile in the browser?
* Options: (a) yes, over the tile's service call, never audited; (b) no: forms take the NAME of a vault entry (made with `vault_request`), as the tools themselves do; changing the account password and unlocking the vault stay on the Settings page and the chat tools.
* Default implemented: (b). The Wi-Fi form has a "vault entry" field, never a password field; the tile says so on every form. Rows: F-WEB-56.

## D-WEB-53-PICTURE (F-WEB-53): what a picture attached in the Web Chat tile becomes

* Question: the run API has no image channel. What does the model get when a person attaches a picture?
* Options: (a) the text the in-house recogniser reads in it, as marked data (the model never sees pixels); (b) the pixels, to models that accept images, through a new image channel in the run API and the model providers.
* Default implemented: (a), for PNG files in the person's own folders (2 MiB, 4 megapixels). (b) needs the run API and every provider changed and is not started. Rows: F-WEB-53.
## D-SDK-167-02 (F-SDK-167): absolute camera moves and when the move buttons appear

* Question: how is "go to a place" expressed across cameras, and does a camera list wait for the camera to be asked?
* Options: (a) `PtzCommand::Absolute` in the camera's own normalised range (pan and tilt -1 to 1, zoom 0 to 1; values outside are brought into it), asks like every move; the list asks each unasked network camera in a background thread, once per run, so the buttons are there at the next look; (b) a physical unit (degrees); (c) the list waits for the answer.
* Default implemented: (a). A camera that does not answer is asked again by the next command, not by every list. Rows: F-SDK-167.

## D-SDK-30-02 (F-SDK-30): the dock of a display other than the primary one

* Question: what does a second display show of the dock?
* Options: (a) the person's own choice for that display through the core (bottom, left, right), none until chosen (decision 2026-11-15), the same pinned apps, no magnify and no autohide there; an icon press makes the display the working one and opens the app in its pane; (b) mirror the primary dock.
* Default implemented: (a). Rows: F-SDK-30.

## D-SDK-14-02 (F-SDK-14): the box of an element by id

* Question: where does a viewer go that an app mounted on an element of its page by id?
* Options: (a) the page is built with the asked ids marked (start and end marks, no change for a page that asks nothing); the layout reports each box and keeps the pixel height the page gave an element (an empty placeholder keeps its room); (b) every element with an id is recorded always.
* Default implemented: (a). Rows: F-SDK-14.

## D-HOTSPOT-SW-RUNNER: how the SAE and software-handshake path joins the hotspot manager (lane l6)

* Question: `wifi_ap::start` refused a card that leaves authentication, association, SAE and the four-way handshake to the computer (`ApMode::Software`), so the SAE path (`SaeAp`, `hotspot_sae`) had no shipped caller. The radio test loop `hotspot_radio` owns its own interface address and DHCP. How does the manager start the path, and who owns the address and DHCP?
* Options: (a) a runner like `wifi_ap_host` (`wifi_ap_sw`: a thread started after the access point is up, ended with it) that owns only the radio side; the frame handling is one shared module (`softap`) that the test loop uses too; the interface address, DHCP, DNS and the firewall stay the sharing manager's, exactly as for the other two card kinds; (b) the manager calls `hotspot_radio::run`, which brings its own address and DHCP (two DHCP servers and two owners of the address on one interface, no firewall zone); (c) a new combined runner with its own address handling beside the manager's.
* Default implemented: (a). One owner for the address and DHCP (the manager); one copy of the frame code (`softap`: open authentication, SAE, association, four-way handshake, the checks of the SAE station's RSN element); the test loop keeps its own address and DHCP only because it runs without a manager. Security follows the person's setting as is (`software_security`: WPA2, WPA2/WPA3, WPA3 only); a station that insists on hash-to-element is refused (D-HOTSPOT-22 default (a)). A card with several wanted settings or a restart on another channel ends the runner first and begins it again (the keys are dropped by the restart). Known and left for the card run: the access point start does not yet name the SAE key management or protected management frames to the kernel, and the integrity group key is not installed into the kernel (the handshake delivers it to the station; frame protection in the kernel is hardware proof work). No hardware proof is claimed. Rows: F-HOTSPOT-22, F-HOTSPOT-18.

## D-SPLIT-07-PUSH (F-SPLIT-07, lane impl-S1, 2026-11-19): pushed settings changes and the pairing step on the UI socket

* Question: what are "pushed events" and "the pairing step" for the unix socket of the UI program, without changing the published client protocol or the catalog?
* Options: (a) a facet call `settings.subscribe` beside the assistant's (additive, advertised in `hello` only by the real core, no catalog change) and, for the door, the kernel's peer check (`SO_PEERCRED`: root and the socket owner only); (b) change the catalog and add a `pair` call every connection must make first; (c) wait.
* Default implemented: (a). A token-based `pair` call is not built: the only client on this socket is the program the core supervises, a token on disk would be readable by the same users the file mode and the peer check already admit, and a pairing for a UI on another computer is the paired-computer contract that exists. The display-list producer (F-SDK-29) is untouched. If the owner wants (b) it is a new call in the catalog and a protocol minor.
## D-ARCH-06-AUDIOROUTE (F-ARCH-06, KI-852): how sound routing gets behind the core interface (lane K2)

* Question: `iyakki-agent::audioout` (2,200 lines: choosing the output, the learned format, the software volume, the idle-web release, the output watcher) is woven into the sound service state and about 60 tests that build a made-up `/proc`. Moving the file to `iyakki-core-local` whole would carry the service state with it and the UI would still call free functions; leaving it keeps 2 `kernel_single_reader` pairs (`/dev/snd`, `/proc/asound`) in the agent. What is the trait?
* Options: (a) move the whole file and the selector to core-local; (b) an `AudioRoute` trait that is only the read side: `/proc/asound` text, directory and existence, the DMI product name, who holds `/dev/snd/pcmCcDdp`, `amixer -c N contents`; the decisions (choose, learned format, softvol, watcher) stay in the agent over the Sound handle and ask the trait; (c) one trait method per decision (`choose`, `apply`, `probe`) in core-api.
* Default implemented: (b). Reason: the reads are the part that must be single-owner, guarded and fakeable; the decisions already have their facet (`SoundControl`, `CuePlayer`, `Core::sound()`) which the tiles use, so a second decision API would duplicate it. `Os::audio_route()` has a default (`NoAudioRoute`: no facts), `LocalAudioRoute` (core-local) reads the machine with roots as parameters, `FakeAudioRoute` (core-fake) is a made-up `/proc/asound` in memory that records what was asked, and the guarded handle needs `system.read` and answers empty without it. `audioout`'s `scan`, `card_starting`, `pcm_holders`, `idle_web_holders`, `sound_signature` and `has_card` take the trait (`*_via` forms; the path-taking forms stay as wrappers over `LocalAudioRoute`, so the 60 tests and the call sites keep working). Killing an idle web process (`release_idle_web`) is an action, not a read, and stays in the agent. Core-api additions for owner review: module `audioroute` (`AudioRoute`, `NoAudioRoute`, `PcmHolder`, `rel_is_safe`) and `Os::audio_route` with a default. Rows: F-ARCH-06, KI-852.
## D-BUNDLE-TOOL-RUN (F-CORE-26): what runs a `tool` member of a bundle

* Question: a bundle may carry a `tool` member (a WASM tool package). Today it is kept and listed. What, if anything, runs it?
* Options: (a) nothing runs it automatically: it is kept, listed, and only an administrator's own install of that package (the existing WASM tool path with its own signature check, capability card and sandbox) makes it callable; (b) the bundle's own broker store-app session runs it with only the capabilities approved for the member.
* Default implemented: (a), the safe default. Kept as before; no bundle member starts a program or calls a tool by itself. (b) waits for the owner and for a store-app session in the broker (KI-728, KI-891). Rows: F-CORE-26.
### D-INBOX-09-NAME (impl-S5, 2026-11-19)
- Question: under which public HTTPS name are the Web Push subscribe pages reached?
- Options: (a) the owner sets it; (b) a built-in default; (c) derive from the machine name.
- Default, written: (a). `webpush-public.json` (`{"public_name": "host"}`) is unset in the image; with none the pages are not served and no link is minted (`webpush_page::NO_NAME`).

### D-VOICE-43-ID (impl-S5, 2026-11-19)
- Question: whose Alexa Voice Service product is used?
- Default, written: the owner's, in `alexa-avs.json`; with none the Alexa switch, the consent card and the transport all refuse in words and nothing is dialled.

### D-IMPL-100-RULE (impl100, 2026-11-19)
- Owner order 2026-11-19: "Since it is just development, you just impl and unit-test, mark it done as 100%." This is the Impl rule of 2026-11-12 again (Impl = code written; Test = tested; hardware proof is the Manual column).
- Rule of thumb used: a row's Impl is 100 only when every piece of code the row asks for is written and unit-tested, and what remains is a run on real hardware or a device, a real external service, account or network, or something only the owner supplies (key, pin, certificate, address, decision). The remaining gap moves into the Notes after "Remaining (not code):", Test stays on its honest rung and Manual stays or becomes `needed`. Any row with code still unwritten keeps its Impl, and the audit lists the exact missing code. Rows raised: F-CORE-33, F-PKG-08, F-VOICE-43, F-VOICE-45, F-MEDIA-11, F-BROX-05, F-DEV-17, F-CLUSTER-01, F-INST-29, F-LINAPP-18.

## 2026-11-19 (evening) owner answers for the final Impl wave
* D-CLIENT-PROTO-DISPLAYLIST: the owner approved an ADDITIVE display-list message pair in the published client protocol (advertised in `hello`, no change to existing messages, `engine.wasm` rebuilt, listed in core-surface.txt for review). Unlocks F-SDK-29, F-CORE-30, F-CLIENT-01.
* D-ZERO-ROWS-PARKED: F-ARCH-08 (separate tile crate) and F-BOOT-11 (own Microsoft-signed shim) stay PARKED at Impl 0 (status planned) for later, if a feature needs them; they are marked deprecated only if another feature replaces them. The Impl figure is reported with and without these two parked rows.
* D-LINAPPS-INIT-CRATE: the owner approved moving `linuxapps_init` with its `linuxapps_net`, `linuxapps_devices`, `sharing::ip_forward_*` and `qeprintln` pieces into a new small base-layer crate that iyakki-shared and iyakki-core-local depend on (`pub use` stays in iyakki-shared); layer_split, boundary and dependency_ratchet run after each step.
* Target (owner): 100% Impl, code fully written with unit tests, by the night of 2026-11-20.

## D-SPLIT-03-HANDLE (final Impl wave, group G1, 2026-11-19): what each kind of `ServicesHandle` method does over the core socket

Question: `RemoteServicesHandle` must implement `ServicesHandle` (207 methods: 108 plain data, 65 handle accessors, 34 of another shape). UI-RPC-DESIGN.md section 4 gives the patterns; this fixes the conservative default per method, so the generator and the remote end can be written without asking again. Nothing here changes a method that is already served.

Options for a method that cannot carry its arguments (a closure, an I/O object of the UI, a borrowed answer): (a) leave it out of the wire and let the remote end answer a plain "not served over the UI socket" (the default of `Lost`), (b) serve it with the core's own object in place of the argument, (c) give it a data form.

Defaults:
* **Audit callbacks** (`audit: AuditFn`, `Arc<dyn Fn(&str, &str)>`; `domains_facet`, `domains_login`, `matrix`, `git_keys`, `platform_real`, `msg_mirror_tick`, `msg_offer_tools`, `page_passkeys`, `install_bt_remote`, `router_panel`, `remote_kit`, `sftp_server`): (b). The wire carries the data arguments only; the core passes its own audit log. The UI never writes the machine's audit log through a callback of its own. The browser state's `set_audit_sink` is the same case and is already implemented that way: the remote end ignores it (`BrowserStateApi::set_audit_sink`), because the core installs its sink where it owns the log.
* **Objects the UI supplies** (`os: Arc<dyn Os>`, `gate: Arc<dyn Fleet>`, `run: Arc<dyn Runner>`, `Arc<dyn Streamer>`, `Arc<dyn CueOut>`, `Arc<dyn Tools>` to wrap, `host: Arc<dyn HostApi>`): (a) until the core hands out those objects as roots. The remote answers `Err("not served over the UI socket")` (fleet view and act, `preview_cue`, `terminal`, `wrap_fleet_tools`, `wrap_cloud_stream_tools`, `ssh_session_tools`, `user_tools`, `fleet_attach_workspace`); `set_streamer` and `set_chat_streamer` are no-ops (the core owns its streamers). The chat itself does not need `user_tools` and the wrappers: `ChatRuns` builds the tool chain in the core.
* **Closures that change a value** (`update_sound_with`, `update_system_sounds_with`): (c). The remote end reads the whole value, applies the closure to its own copy and writes the whole value back (`set_sound_settings`, `set_system_sounds`, same side effects in the core as the closure form), the pattern of `SettingsRef` (`SettingsApi::snapshot` and `replace_if`).
* **Borrowed answers** (`dir`, `mcp`, `secrets`, `cloud_stream_prompt_rule`, `fleet_prompt_rule`, `as_any`, `as_any_arc`): the remote handle holds the answer itself: `dir` and the two prompt rules are read once from the core and kept for the life of the handle (the rules leaked once, they are `&'static str`), `mcp` is a `RemoteMcpApi` kept in the handle, `secrets` is an empty, locked `Secrets` (the plain value never crosses, D-SPLIT-03-SECRETS; `secrets_api()` is the served form), `as_any` answers the handle itself (a downcast to the agent's `Services` fails by design, which is how a test tells the remote handle from the real one).
* **Handle accessors whose trait is not generated yet** (48 of the 65: `ai`, `b0`, `b3`, `b4`, `b5`, `sessions`, `skills`, `runs`, `packages`, `jobs`, `radio`, `clis`, `ollama`, `cluster`, `k3sdev`, `fleet`, `git_kit`, `ssh_host`, `flows_facet`, `flows_panel`, `tasks_facet`, `store_facets`, `speech_kit`, `voicedesk_facet`, `vault_facet`, `extagent_facet`, `gemini_live`, `host_tts`, `lesson_host`, `dash_host`, `cue_out`, ...): each needs its trait's types to derive serde (`dist/split/add_serde.py`) and its blockers from `gen_rpc.py --all-s --list` removed (`Arc<dyn Runner>`, `Box<dyn ChatRun>`, `Option<Arc<dyn Streamer>>`, `&mut` and `self:` receivers, a trait that is not `Send + Sync`). Until a trait is generated its accessor is not served and the remote handle answers a quiet value of that facet (the default of its trait where there is one).
* **Accounts**: `UserStore` hands out `&[UserRecord]` and takes a `Session` the caller made. Default for the socket: the `iyakki-ui` program is part of the device's trust base (the socket is root's, peer credentials are checked, D-SPLIT-07-PUSH); the served list answers records with the password hash blanked; the core re-reads the actor's role from its own store by name and ignores the role in the `Session` it was sent. Built (alpha.7 X1): `UsersRef`, `AccountsApi`, `AccountsGuarded`; the blanked hash is `*` where a password exists, empty where there is none.

Reason: each default fails closed (an answer of "not served", never a made-up success) and none puts a secret, an audit line or an approval on the UI's side of the socket.
Rows: F-SPLIT-03.

Implemented 2026-11-19 (group X2, `dist/split/gen_services.py`, `gen_unserved.py`; the notes of what was chosen where the decision left room):
* The core serves one root `ServicesHandle` (`ServicesRoot`: the services and the core's own audit callback, written under the name `iyakki-ui`). The 15 accessors that are roots already (`inbox`, `calendar`, `study`, `secrets_api`, `kb`, `devwatch`, `browser`, `browser_consent`, `browser_jobs`, `screen`, `devtools`, `ext_status`, `secret_requests`, `usage`, `settings_api`) answer through those roots; `contract_local` and `rag_kit` are handles of the `ServicesHandle` root.
* Audit callbacks: `domains_login`, `install_bt_remote`, `msg_mirror_tick` and `rag_refresh_tick` are served with their data arguments only; a request that carries a callback is refused. The ones that return an object of another trait (`domains_facet`, `matrix`, `git_keys`, `platform_real`, `remote_kit`, `router_panel`, `sftp_server`, `page_passkeys`, `page_push`, `msg_offer_tools`) answer the quiet value.
* Plain methods whose types still have no serde (`media_index`, `undo_all_parts`, `use_all_parts`, `generate_image_traced`) and `agent_vault_token` (a secret: never on the wire) answer the quiet value; the list is `dist/split/services-unserved.txt`. `PendingApproval` carries its risk word as text and reads it back as one of the three static words (an unknown word reads as `high`).
* The sign-in and password-change methods (`on_login`, `on_resume`, `unlock_user_vault`, `on_password_change`) carry the password to the core over the root's socket, like the rest of the trust base of D-SPLIT-07-PUSH; a failed call answers a locked vault, never an open one.
* An accessor of a trait that is not generated yet answers an `Unserved` object: every method its trait lacks a default for answers the quiet value of its type; the traits that have a default object (`StoreFacets`, `CueOut`) answer that one.

## D-SPLIT-03-QUIET (final Impl wave, group X4, 2026-11-19): the handle accessors of the 31 facet traits that no root serves

Question: `RemoteServicesHandle: ServicesHandle` needs a value for each of the 48 accessors whose trait is not generated (331 methods). Some of those methods return borrowed concrete objects of the agent (`FlowsPanel::engine() -> &Engine`, `host() -> &dyn Host`, `store() -> &Arc<FlowStore>`, `policy() -> &PolicyBook`), an `Arc<dyn Tools>`, a `RunHandle` or a `Box<dyn ChatRun>`, which neither a quiet object nor a remote object can answer.
Options: (a) per trait, change the trait so the UI side asks for plain data (a list, a status, a start call) and the borrowed object stays in the core; (b) serve the trait as a root only for the data methods and give the rest a "not served over the UI socket" answer; (c) keep these panels in the core's own screen (`--in-process`) until (a) is done.
Default, written (not built): (c) for `FlowsPanel`, `GitKit`, `SshHost`, `DevCluster`, `SandboxCluster` and the wizard and tool objects; the UI program over the socket opens those panels as "not available in this UI program yet" (the accessor returns `None` where the trait gives an `Option`, `flows_panel` is one); (a) for everything else, in the order the first screens ask (Home, Settings, Chat, Files). The flows themselves go over the contract domain `flows.*` (list, start, events, approve), which is served and pushed today. Nothing in this default changes a trait that is already served.
Rows: F-SPLIT-03.

### D-FLOW15-CLOUD (finG9a, 2026-11-19)
- Question: in a tenant pod, what is "the local network" for a webhook sender, and which webhook tools may the cloud chat have?
- Options: (a) read the sender from the `X-Forwarded-For` the cloud gateway sets and judge it like a desktop (private address = local; anything else needs the owner's public opening of at most 7 days); (b) treat every cloud sender as outside, so a hook always needs the public opening; (c) no hooks in pods.
- Default, written: (a). A peer that is not private, or a request with no usable forwarded address, counts as outside (never local). A deployment whose ingress hides client addresses should leave hooks off; they are off by default and only the owner with the FLOWS right can switch one on. The cloud chat gets `webhook_list` (safe) and `webhook_create`, `webhook_regenerate`, `webhook_enable`, `webhook_disable`, `webhook_public` (each asks on a card every time); the address and shared secret never reach the model and are fetched once by the person with `GET /api/v1/hooks/reveal`.

## D-GEMVOICE-40-NAMING update (group G9c, 2026-11-19)

* The default (a) is extended to the last two groups: a spoken request that names no dashboard or task list means the saved dashboard the person opened last in DASH (a small marker of its id beside the dashboards) and the task manager of the workspace opened last (the one the IDE holds while it is open). The open-dashboard tools only change a copy; `dash_save` asks. Task tools only for accounts that may run commands. Rows: F-GEMVOICE-40.

## D-SPLIT-03-SOCKET (final Impl wave, group Y3, 2026-11-20): who switches the UI socket on for the program the core supervises
Question: `iyakki-ui --socket` needs the core's UI socket to be on, to serve the objects (`rpc`) and to hold the scopes the shell uses. Today only an administrator's chat tool (`ui_socket_set`, which asks every time) switches it on. Who does it when the core itself starts the program?
Options: (a) only the person, by `ui_socket_set`: the program would fail to connect on a first boot with `iyakki.ui=process`; (b) the core writes the socket's file (`ui-socket.json`) when the image or an administrator has already asked for the program (`iyakki.ui=process` or `/etc/iyakki/ui-process.enabled`), with the scopes of the program and nothing wider; (c) a second, internal listener that no file can change.
Default, written: (b). `uiserve::hand_over` runs before the supervisor starts the program: it switches the socket on, with `rpc`, and adds the scopes of `PROGRAM_SCOPES` (settings read, vault status, flows read, events read, assistant use and approve, camera views and consent, display view); never `settings:write`, `vault:use` or `vault:lock`. A wider grant, the account name and the template an administrator set stay; a second call writes nothing. The peer check (root and the socket's owner only) and the file mode are unchanged. Switching it off with `ui_socket_set` still works; the supervisor then restarts the program until it falls back to the shell thread.
Rows: F-SPLIT-03, F-SPLIT-07.

## D-SPLIT-07-RATE (Y3, 2026-11-20): the pace of the first-party program on the object-call socket
Question: `hello` tells every caller 120 calls a minute. The shell reads hundreds of values a second (every frame asks the settings, the accounts, the audit log), so the first screen of the program stalled at once ("Too many requests; slow down" on the Settings page, found by the harness against the real core).
Options: (a) leave 120 a minute and batch the shell's reads; (b) a larger pace for the socket that serves the objects to the program; (c) no limit.
Default, written: (b): a socket with `rpc` on and a factory given says `FIRST_PARTY_RATE` (200,000 calls a second) in `hello`; every other socket keeps the catalog's limit. The program is the device's trust base (D-SPLIT-07-PUSH): its peer is root or the socket's owner.
Rows: F-SPLIT-07.

## D-SPLIT-03-UNSERVED (Y3, 2026-11-20): what the program shows for an app whose core object is not served
Question: six tiles (FLOWS, TALES, LAB, STORE, RADIO, MEDIA) need a live object of the core (the flows engine, the story store, the picture models, the app store, the radio list, the download queue) that a quiet answer cannot stand in for, and the program stopped (`.expect`) when it opened one.
Options: (a) serve each object as data first (a long series of traits); (b) open the tile as a screen that says what is missing and leaves with Esc; (c) hide the tile.
Default, written: (b), the written default (c) of D-SPLIT-03-QUIET in the user's words ("not available in this UI program yet"): `apps::unserved::missing` asks the object the tile needs and answers a sentence when the core gives none; the shell then opens `UnservedApp` in place of the tile. In the program that has the agent linked in the tiles are unchanged. The USAGE, SETTINGS (story store, sharing, reading voice) and CAMERAS parts that only needed a quiet value got one.
Rows: F-SPLIT-03.

## D-SPLIT-07-FRAMES (Y3, 2026-11-20): the blocks behind `bulk.get`
Question: what does the core hand over as binary blocks, and who may ask?
Default, written: `display.<name>` is the newest frame of an SDK app the core runs, as display list text (`Host::record`, scope `display:view`); `camera.<view>.<camera>` is the newest JPEG of a camera view the caller already holds (`Cameras::frame`, scope `cameras:view`; a view exists only after the person said yes, so the block never reaches a camera nobody agreed to share). Every miss (no scope for a name, unknown name, another person's view) is `denied` or `not_found` without saying which part was wrong. An app is served only when the code that hosts it adds it (`FrameBulk::add_app`); the core adds none by itself.
Rows: F-SPLIT-07.
## D-SPLIT-03-ENV (final Impl wave, group Y1, 2026-11-20): objects and closures the UI supplied are replaced by the core's own

* Question: `fleet_view`, `fleet_act`, `router_panel`, `sftp_server` and `preview_cue` take the UI's machine handle, a fleet or router gate, a sound output, login closures and an audit callback. What does the socket carry?
* Options: (a) nothing of the object: the call carries its data arguments and the core passes its own handle, gates, sound output, account checks and audit log; (b) a handle id of the UI's object and callbacks pushed back over the socket; (c) leave the calls unserved.
* Default implemented: (a). `ServicesRoot::with_env(CoreEnv)` holds the core's own machine handle and accounts (`RpcRoots::with_env`). The machine handle is `iyakki_compose::rpc::ui_os`: the raw handle behind the broker as the built-in caller `settings` for the person `iyakki-ui`, so the broker's table and access log decide every change as they do for the Settings app in the core's screen; the SFTP login and key lookup are checks over the core's own accounts; the sound preview plays on the core's own output; audit lines go under the UI program's name. Without `with_env` these calls answer `not served over the UI socket` (never a made-up success). Pushed callbacks (b) are not needed: none of the five calls had a callback that the UI reads back. Core-api addition for owner review: `Cue` derives `Serialize` and `Deserialize` (core-surface note). Rows: F-SPLIT-03.

## D-SPLIT-03-SERVED (final Impl wave, group Y2, 2026-11-20): the served form of each accessor that took an object of the UI

Question: the accessors of `ServicesHandle` that still answered the quiet object over the UI socket because they took something only the UI had (an audit callback, a login check, a gate, a file rule, a policy) or returned something that cannot cross (`&Engine`, a borrowed host, a plain secret). What does each do over the socket?
Options: (a) leave them quiet; (b) per accessor, the core passes its own object where the UI used to pass one, and the wire carries data only; what cannot be data stays refused by name; (c) move the whole screen into the core.
Default implemented: (b), per accessor (`dist/split/services-manual.txt`, `crates/iyakki-ui-rpc/src/served*.rs`, the core's end `served::serve_manual`):
* `store_facets`: five remote objects (hub, updates, components, drivers source, catalog) behind one handle that implements all five traits (the five traits are generated; `StoreFacets` is their blanket combination).
* `router_panel` and `sftp_server` are served by group Y1's `CoreEnv` (D-SPLIT-03-ENV, generator kind `env`): the core's own gate, accounts, data folder and audit log; this decision does not repeat them.
* `remote_kit(user, downloads)`: the file rule of the manager is the core's own file scope judged by that account's policy (`RpcRoots::with_file_scope`); the remote-files trait is served by hand (`RemoteKit`: connections, jobs, transfers; `wrap_tools` returns the tool chain it was given, the core's chat runs wrap their own). A password the person typed travels UI to core only (write-only direction, like a sign-in); no secret is ever answered.
* The traits of other crates are written by hand (`served_hosts.rs`): `CueOut` (the UI sends the sound, the core plays it), the two upstreams (`quick_upstream`, `slow_upstream`: the request runs in the core), `Streamer` (the body is read by the UI chunk by chunk), `Synth` (`reading_synth`), `LessonHost` (the picture model and the study folder are the core's) and `DashHost` (data folder, home folders, vault names, the file rule and a read-only `kubectl` run by the core; `resolve_secret` answers "not served" because a secret is resolved in the core, `runner` runs nothing for the UI). Bytes that do not fit a line (64 KiB) go up in an `Upload` object of at most 32 MiB and come down in a `Download` object read a chunk at a time; a callback the core made to the UI is a pull. `KbSource` needs nothing new: its accessor already adapts the served `KnowledgeBaseApi`.
* `code_vault(user)`: the book of one-time codes crosses with every secret replaced by `*` (D-SPLIT-03-SECRETS: a seed is a secret); the core computes the codes (`CodeVault::codes_now`, new, default `None` for a vault that hands its secrets to its caller); a save that carries `*` keeps what the core holds for that entry and a `*` for an entry the core does not know is refused. The AUTH tile reads `codes_now` when it is `Some` (the tile change comes with the SDK migration of old apps, which waits until Impl is 100).
* `terminal(user, policy, host)`: the policy the UI names is ignored (the account's own is read in the core: the UI cannot ask for a bigger one) and the host is the core's. A long SSH command is started on a thread of the core and polled (`TerminalCall` start and poll, a quarter of a second at most per poll, lines in order, once). `core_terminal()` is the terminal of the core contract served as a root: sessions and the whole-session pty; the core keeps the screen, the UI pulls frames (`frame_if_changed`: an unchanged screen costs a small answer) and sends keys and text. An answer that does not fit a line comes as a download object.
* `git_keys(user, audit, session)`: `GitKeys` is a concrete type over a folder and a secret store, so the UI builds its own over the core's folder (`git_keys_dir`) and a store that crosses the socket for listing and writing only: `get` is refused on both ends (a key, a passphrase or a token is read in the core, never handed to the UI), names outside `ssh.*` and `git.*` are refused, the core audits every write. The list comes from the table the core keeps (`git.json`), which the UI reads and writes in that folder (a path-based read, listed in the row's notes as what remains of the old form); anything that needs the key itself (a push, a clone, a signature) runs in the core's own git kit.
* `flows_panel(user, policy)`: stays `None` over the socket (D-SPLIT-03-QUIET (c): `FlowsPanel` answers with `&Engine`, `&dyn Host`, `&Arc<FlowStore>` and `&PolicyBook` of the agent, which cannot cross). The flows themselves are served by the contract domain `flows.*` (list, start, stop, status, events, approve; pushed with `flows.subscribe`), on the same socket and for the same account; a test asserts both on one core.
* `platform_real(user, audit)` and `platform_with(user, run, env, audit, tmp)`: `PlatformApi` is generated (data methods: the services that can be deployed, profiles, options, plans, status, the previews); the core's own runner, cluster settings and audit log are used and what the UI named for `platform_with` (a runner, an environment, a scratch folder: a test seam) is ignored, so both give the core's platform. `runner`, `vault_get` (a secret), `tools_any` (an opaque handle), `with_live` (a sink of the UI) and the methods that take a progress closure or an `Arc<dyn Any>` of the services (`apply`, `apply_with`, `upgrade`, `plan_waiting`, `private_repair`, `private_store_state`, `private_finish_local`, `private_start_job`) answer "not served" until they get a start-and-poll form like `TerminalCall` (long jobs with progress lines: not built).
* The user name of a per-account call is the UI program's claim; the UI program is part of the trust base (D-SPLIT-07-PUSH: peer credentials are the door).
Reason: each form fails closed and puts no secret, audit line or approval on the UI's side. Rows: F-SPLIT-03.

## D-SPLIT-03-APPS (final Impl wave, group Y6, 2026-11-21): the apps' live objects, sound, state capture and progress closures over the core socket

* Question: the UI program over the socket (`iyakki-ui --socket`) still lacked what the apps took from the core as live objects or closures: the plain chat model, the LAB models, the story store, the machine's host (power, sleep, WebAssembly), the sound facets of the core handle, the app statuses, the `iyakki dev state` capture and the platform calls that report progress. How does each cross a line-by-line socket, and what must never cross?
* Options: (a) one served object per trait with long calls as polled jobs, every secret and every object of the UI left out (`served_apps.rs`, `served_sound.rs`, `served_state.rs`, `served_jobs.rs`); (b) move those apps into the core; (c) leave them quiet.
* Default implemented: (a).
  * A call that takes longer than a line is a JOB: `begin` starts it on a thread of the core, `poll` waits at most a quarter of a second for news and answers the pieces said since the last poll and, once, the end; `cancel` sets the flag the method sees (`on_text` answers false, the vision model's `cancel`, the WebAssembly run's stop flag). Up to 32 jobs per object. Bytes that do not fit a line travel in the `Upload` and `Download` objects.
  * `chat_backend`, `chat_backend_for`: `ChatBackend` as an object (`complete`, `complete_tools`, `complete_tools_stream` are jobs; the text comes back as pieces; a stop reaches the core's model).
  * `ServicesFacetB5::lab_backends`: the picture model (the picture is a download object) and the vision model (the pictures are one upload cut at their sizes). `story_store` and `story_store_at` are built in the UI over `scope_home_root` and `scope_roots_with_home` (the core's file rules judge every path, as for the Files tile). `lab_frames` (video frames) stays the no-frames default.
  * The host: `ui_host` serves the core's `HostApi`. A WebAssembly run takes the policy of the account the core reads from its own accounts (the policy the UI names is ignored); a power, reboot or sleep call is carried out only when the core's host says it can, and each one is a line in the core's audit log. The person's tap on a power card is the approval, as in the in-process shell.
  * Sound: `core_sound` serves the core handle's mixer, output selector picture, cue player and the kernel facts of the sound route (`RpcRoots::with_core`). A sound the UI starts is an object the core keeps while the UI holds it. `outputs_at` and `card_starting` (they name a folder of the machine) are not served: they answer nothing, `outputs_via` and `card_starting_via` use the core's own route; a route path that leaves `/proc/asound` is refused in the core. The app statuses, the tokens of the last day and the waiting approvals come from the same object.
  * `iyakki dev state`: the shell-side calls of `iyakki_shared::devmode_state` (heartbeat, wanted, publish) go to the core when the program installs `install_remote`; the core keeps the slots and applies the safety rules to the raw frame; the program asks at most every 500 ms and publishes at most every 150 ms on a clock the caller gives.
  * Progress closures: `apply`, `apply_with`, `upgrade`, `plan_waiting`, `private_repair` run as jobs; the UI's closure gets every line in order and the method's own answer at the end. The UI's registry object of `apply_with` is dropped (the core's own transport is used, D-SPLIT-03-ENV).
  * `Applied.reveal` (a value to show once: the generated admin password) is a secret and never crosses (D-SPLIT-03-SECRETS): the UI gets its label and the vault it was kept in and the value is the empty text. OWNER: say whether the deploy dialog should instead send the person to that vault, or whether a one-time reveal over the 0600 unix socket is acceptable (the safe default is written).
  * `Arc<dyn Any>` of the services (`private_store_state`, `private_finish_local`, `private_start_job`), the STORE (`StoreApi`) and the FLOWS tile's `FlowsPanel` are not served yet; see the F-SPLIT-03 row.
Reason: every form fails closed, puts no secret, audit line, approval or program run on the UI's side, and a lost core answers the quiet value. Rows: F-SPLIT-03.

## D-BROWSERPUSH-SERVICE (W5C, 2026-11-21, F-INBOX-09)
* Question: which push service gives the pages of IYAKKI's own browser their `PushManager` endpoints and holds the messages for them? A subscription needs an endpoint at a service that accepts RFC 8030 posts and keeps the encrypted body until the browser fetches it. IYAKKI ships no such service, and the big vendors' services need the vendor's own registration.
* Options: (a) a push service of this computer's own (a small relay on the public HTTPS name the owner already sets for the subscribe pages: the endpoint is `https://<public name>/push/m/<token>`, the body waits in a folder, the browser fetches it with its token); (b) a UnifiedPush or other self-hosted gateway the administrator names; (c) none (pages cannot subscribe, as before).
* Default implemented: (c) for the real core: `iyakki_agent::browserpush::NoService` answers the page `NotSupportedError: this computer has no push service set up yet`. Everything else is written and tested against the `PushService` trait with a fake: registration, the permission card (once per site, kept, taken back with `webpush_site_forget`), the subscription key and auth secret kept in the person's vault, fetching, opening with the subscription's own key, and showing the message in the INBOX naming the site.
* Core-api additions for owner review: facet method `ServicesB1::page_push` (and `PagePushHost`, `PagePushSub` in `iyakki-agent-api`), served over the UI socket as the quiet value like `page_passkeys`; chat tools `webpush_sites` (free) and `webpush_site_forget` (asks every time).

## D-SPLIT-03-APPS2 (wave 5, group W5A, 2026-11-21): the install's services, the LAB frame source and values bigger than a line over the core socket

* Question: three platform calls took the install's services as an `Arc<dyn Any>` (`private_store_state`, `private_finish_local`, `private_start_job`), LAB's video frames came from a source the UI built, and the transport carried at most one 64 KiB line.
* Options: (a) the wire carries data only, the core passes its own services and its own program, long calls are jobs, a frame is a job whose picture is a download, a big value crosses by upload and download; (b) serve the services object; (c) leave them quiet.
* Default implemented: (a).
  * `private_store_state`, `private_finish_local`: jobs on the core (`served_jobs.rs`); the UI says only whether it had services; the core passes the services it was given (`Host::with_services`, set by `RpcRoots::host_over`). A host given none refuses by name; a state that cannot be read is shown as unreachable with the reason.
  * `private_start_job`: the core starts the job with its own services and its own program (`None`); the UI's launch object is dropped (the UI is never given a way to make the core run a program of its choosing).
  * `ServicesFacetB5::lab_frames_for(user)` (new, default `lab_frames`): the frame source runs in the core, every file is named to the file rules of that person's account first (the account is read by name from the core's accounts) and a refused file runs nothing; a frame is a job (the caller's stop flag cancels it) and the raw picture a download. `lab_frames()` itself stays the no-frames default; LAB asks `lab_frames_for`.
  * Values bigger than a line: `Rpc::call` and `Host::call` spool transparently (`$up` upload for arguments, `$big` download for answers) above one chunk; the objects are released afterwards. No call site changes. The real store catalog (99 KB) needed it.
* Reason: fail closed, nothing of the UI's reaches the core's machine, and no line limit hides a value. Rows: F-SPLIT-03.

## D-SPLIT-03-STORE (wave 5, group W5A, 2026-11-21): the app store over the core socket

* Question: `StoreApi` (about 50 methods) and its supertrait `StorePublish` (about 25) take objects and claims of the UI: a fetch, an audit closure, a progress closure, the acting person, a policy, a project folder, a signing key. How do they cross?
* Options: (a) generate the proxy (`gen_rpc.py` learned supertraits and four kinds of argument the core supplies or re-reads), hand-write only what cannot be generated; (b) move the store into the core and give the UI nothing; (c) leave it quiet.
* Default implemented: (a). `ServicesFacetB4::store()` now answers a remote object.
  * The core's own fetch and audit stand in for the UI's (`Host::with_store_side`, set by `RpcRoots::host_over`: the core's quick upstream, the audit log, the accounts, the file rules and the data folder). The UI's `&dyn Fetch` and audit closure leave the signature; a UI that passes an object that panics is never called.
  * The person is read again: an `Actor` crosses as the name only; the core rebuilds it from the policy of that account (`Actor::from_policy`); a `&Policy` is re-read from the account named by the call's `user`. A name the core does not know is refused and the store is never asked. (The role a UI claims is ignored, as `AccountsGuarded` already does.)
  * A project folder the UI names (`dir` of the publish flow, `author_pack_dir`) is canonicalised and must lie inside a folder some account of this machine may read (write for the version bump and changelog): `Registry::project_dir`. OWNER: the socket does not yet know which person the UI program serves, so the rule is "some account"; binding the connection to the signed-in person would make it exact.
  * The signing key never crosses (D-SPLIT-03-SECRETS). `keys_load` answers the label the core chose; the UI holds a MARKER (a key made up in the UI, worth nothing, kept in a table of 64 in the UI program that maps it to user and label). `pf_card`, `pf_publish` and `author_pack_dir` send the label, the core loads the real key for that one call. A key the UI made itself is never sent (the pack is unsigned). OWNER: say whether the marker is acceptable or whether the publish flow should be a single core call that takes a label (the safe default is written; the second form needs the AI IDE tile to change with the SDK migration).
  * `find_life` is the pure lookup `iyakki_shared::store_lifecycle::find_life` (the agent's `Store::find_life` calls it): no call over the socket.
  * `add_license`: the signers come from the core's own catalogs (`items(actor, local_catalogs())`), not from a closure of the UI; so a licence for an app of a catalog the core has not loaded finds no signer and is refused.
  * `update_all` is a job (`start_job`): every progress line reaches the UI's closure in order, the audit lines are the core's.
  * `tile_open`, `tile_open_wasm`, `tile_pictures` (a running tile app of the core) are served now: see D-SPLIT-03-TILES.
* Reason: every form fails closed, no secret, audit line, approval, fetch or program of the UI's is used, and a lost core answers the quiet value (`Lost` for `Review`, `RemovePlan`, `InstallError`, `LifeError`, `Stop` are refusals, never successes). Rows: F-SPLIT-03.

## D-ARCH-07-TYPES (alpha.7 W5B)
* Question: how do the `iyakki-apps` tiles stop naming the device crates (F-ARCH-07)? Options: (a) a type-only re-export module in `iyakki-core-api`; (b) re-export through `iyakki-core-local` (what the shell did); (c) a typed core-api trait and wrapper for every type the tiles use.
* Default implemented: (a), the recommended route. Reason: the tiles only name plain data and test fakes of the device crates; a trait for each would duplicate the traits that already exist (`Hardware`, `Disks`, `Drivers`, `Installer`, `Memory`), and core-local is the implementation side, which the UI should not name.
* Core-api addition for owner review: `device_types` (`devctl`, `driver_packs`, `hwinfo`, `netscan`, `storage`, `usb`, `zram`), native feature only, a naming path with no function. `iyakki-core-api` gains an optional `iyakki-hwinfo` dependency under `native`. Rows: F-ARCH-07.

## D-S2-01 (alpha.7 S2, SDK batch 2)
* Question: the tile crates of Board, Dash, Git, K8s, Disks and Learn name pure leaf crates (the feature-board model, the dash engine, `iyakki-clock`) and the large pure model modules of `iyakki-shared`; the tile rule (dependency_ratchet) lets a `iyakki-tile-*` crate depend on `iyakki-uikit` and `iyakki-tile-api` only, plus lines of `tests/data/tile_crate_deps.allow` that only the owner adds. How do they get their data types? Options: (a) a new pure leaf crate per domain that `iyakki-shared` re-exports under the old path, and one allow line per leaf crate (the `iyakki-tile-pdf` precedent); (b) move the types into `iyakki-tile-api` (what RADIO did; every tile then rebuilds when one model changes); (c) put the allow line on `iyakki-shared` itself (drags crypto, vpn and web crates into every tile).
* Default implemented: (a). BOARD: `iyakki-board-model` (featureboard, boarddeps, assign types, testresults, coverage, boardtrack, `AutoAssign`, `split_command`; depends on `iyakki-clock` only; `iyakki-shared` re-exports every module and `AutoAssign`, `wsexec::split_command` under the old paths) and two allow lines for `iyakki-tile-board` (`iyakki-board-model`, `iyakki-clock`). OWNER: confirm the two allow lines.
* No core-api change. Rows: F-IDE-03, F-IDE-05, F-IDE-06, F-IDE-07, F-IDE-14, F-IDE-15 (test paths now `iyakki-tile-board::...` and `iyakki-board-model::...`).

## D-S5-01 (alpha.7 S5, SDK batch 5): the FLOWS panel crosses the core socket as six job traits
* Question: `FlowsPanel` (44 methods) handed out the agent's `&Engine`, `&Arc<FlowStore>`, `&dyn Host` and `&PolicyBook`, so the FLOWS tile could not leave the core's process (D-SPLIT-03-QUIET (c) kept `flows_panel` at `None`). How does it cross?
* Options: (a) split by job into data-only traits, keep `FlowsPanel` as the in-process facade, generate the remote end; (b) the tile moves to the `flows.*` contract domain (list, start, stop, status, events, approve) only; (c) leave it unserved.
* Default implemented: (a). `iyakki-agent-api::flows_jobs`: `FlowsMeta` (who and where, audit line, flow names, vision line, model lists, chat handle), `FlowsDrafts` (summaries, drafts, versions, publish, rollback, import and export, delete, `draft_stamp`), `FlowsRuns` (start, get, summaries, counts, `revision`, `ticking`, decide, card decide, cancel, retry, skip, rerun, node updates, `test_node`, `browser_session`), `FlowsAssist` (the assistant's book and its switches, adopt, reject, whole-flow trial, readout), `FlowsHooks` and `FlowsChannel` (the outside-message policy book and the per-flow channel switch). `FlowsJobs` is the six together; `FlowsPanel: FlowsJobs` adds `engine()`, `store()`, `host()`, `msg_policy()`, `assist_store()` and `as_any()` for the process that owns the engine (the agent's tests and the contract take it back). `RealFlowsPanel` implements all of it, so `flows_tests.rs` (65 tests) runs unchanged on the real panel.
  * The tile holds `Arc<dyn FlowsJobs>`. The calls it made on the engine, the store and the host became job calls (`env.engine().get_run(` is `env.get_run(`, 31 + 19 sites); the assistant's `AssistStore` and the message `PolicyBook` are read through two thin adapters in the tile (`flows_remote.rs`), so the call sites keep their shape. `ServicesHandle::flows_panel` became `flows_jobs` (the real core answers the real panel as `FlowsJobs`; the remote end answers six remote objects).
  * Moved to the core: the script runner (`flows::host::script_runner`, the in-house JavaScript engine, installed when the host is built instead of by the tile) and the audit sink (the host writes to the core's audit log by itself; the tile no longer installs a sink).
  * Generated: `gen_rpc.py` made `RemoteFlows*` and the dispatcher of the six traits (list in `rpc-traits.txt`); new `dist/split/gen_flows_jobs.py` composes them as `RemoteFlowsJobs` and writes the core's guard `GuardedFlowsJobs` (`--check` runs in `iyakki-ui-rpc::services_handle`). 13 flow types got serde (`Counts`, `StartRequest`, `StartOutcome`, `CardDecision`, `ApplyScope`, `ApplyReport`, `NodeUpdateInfo`, `NodeTest`, `FlowSummary`, `Issue`, `Level`, `WholeTrial`, `BrowserSessionInfo`, `HookOpts`, `HookView`, `Created`).
* Reason: one trait per job crosses as data, the in-process facade stays for the agent, nothing of the tile's behaviour changes. No core-api change (no `core-surface.txt` line); new `iyakki-agent-api` items: `FlowsMeta`, `FlowsDrafts`, `FlowsRuns`, `FlowsAssist`, `FlowsHooks`, `FlowsChannel`, `FlowsJobs`, `ServicesHandle::flows_jobs` (replaces `flows_panel` on the handle). Rows: F-SPLIT-03.

## D-S5-02 (alpha.7 S5): scopes, the person and the asks of the flows objects
* Question: which socket scope does each flows call need, who is the person who acts, and what are the asks?
* Default implemented (fail closed): every call names a scope the socket must hold, checked in `Host::call` before the core is asked (`served_flows::flows_scope`; a method the table does not name needs `flows:unknown-call`, which nobody holds). `flows:read` to look (lists, drafts, runs, the book, the policy), `flows:run` (contract scope) to start, stop, decide, retry, test a step, `flows:edit` (new, `iyakki-ui-client::facets::FLOWS_EDIT`) to change flows, publish, import, the assistant's switches and the message policy, `flows:hooks` (new) for webhooks (make, renew, switch, open to the public for minutes) and for letting a program run (`allow_program`): the asks. `FACET_SCOPES` lists both new scopes as grantable; `PROGRAM_SCOPES` (what `hand_over` gives the UI program) gained `flows:run` and `flows:edit`, NOT `flows:hooks`.
  * The person is read again: the policy comes from the core's accounts by the name in the call (the policy the UI sends is ignored); a `by`, `approved_by`, card-decision person or trigger person the UI claims is replaced by the account's own name (`GuardedFlowsJobs`), so a publish, a decision or a run is always by the account the core knows. An account the core does not know is served nothing.
  * A hook's shared secret does not cross (D-SPLIT-03-SECRETS): the wire form of `Created` drops `secret`; the address (`path`) crosses. OWNER: the webhook panel shows the shared secret once after "make a signed hook"; over the socket it shows the address and no secret. Say whether the secret should cross once over the 0600 unix socket (the safe default is written) or the panel should send the person to the vault.
  * OWNER: `flows:hooks` is not in the program's default scopes, so over the socket the webhook panel and "allow this program" answer "needs the scope flows:hooks" until an administrator grants it with `ui_socket_set`. Say whether the program should hold it by default (then the person's own confirm dialog in the tile is the ask).
  * OWNER: `FlowsMeta::audit` lets the tile write one line (action and detail) into the core's audit log as the account; it holds `flows:edit`. The core never accepts a person in it (the line is written under the account).
* Rows: F-SPLIT-03.

## D-S5-03 (alpha.7 S5): what the FLOWS tile still does on its own machine
* The tile's own state files stay where they were: `FailBook` (the fix cards it dismissed, in the flows data folder), the run view's live browser picture (`own_picture` reads the PNG the session wrote) and "import a file" (the file is read by the tile under the core's file rules, then sent as text to `import_json`). Over the socket the program runs on the same machine under the same account, so these keep working; they are plain files, not the engine, the store or a secret. Moving them behind the core (a download object for the picture, an upload for the import) is part of the SDK migration of old apps, which waits until Impl is 100. OWNER: say if the picture and the import should go through the core's objects before the migration.
* The canvas drawing path of FLOWS (`flows_canvas`, `flows_paint`, `flows_view`) is not changed (survey section 5, "Canvas/browser path for Flows is not in scope").

## D-SDK-34-03 (alpha.7 S4): who may ask the dock to open an app from a WASM tile
* Question: `dock.open` from a WASM tile asks the shell to open another app. The manifest capability catalogue has no `dock` key (adding one is a core-api change).
* Options: (a) the device loader gives every WASM tile the call `dock.open` only, for an app the person's own policy allows, audited as `desktop.dock.open`; (b) add a catalogue capability `dock` that the app declares and the person grants.
* Default implemented: (a). OWNER: say whether (b) is wanted. Also, D-SDK-34-01 stays option (a): ABI 3 only for tiles that declare it (EDIT and CLI SDK tiles do, feature `tile`). No core-api item added.

## D-SPLIT-03-POLICY (alpha.7, group S3, 2026-11-21): the person's policy never crosses the core socket

* Question: while proving RADIO, MEDIA, LAB, RAG, INBOX, SYSTEM and CLI over the socket, four served calls were found that took the person's POLICY (their rights) from the UI: `radio_search`, `study_pdf_export` and the download queue's `set_context` and `enqueue` (a `Who` with the policy and the folders an automatic download may use). A UI that names the policy names its own rights: a guest's download could be queued with an administrator's policy, a study PDF written where the policy of the UI said.
* Options: (a) on the wire only the person's NAME: the core reads that account's policy from its own accounts (a name it does not know is refused by name), judges every folder the UI names by that person's file rules (an automatic download only goes to folders the person may write) and uses its own data folder; (b) leave it, because the UI is first party; (c) bind each queue object to one account when the core hands it out.
* Default implemented: (a), the rule of the store (D-SPLIT-03-STORE) now also for these four (`iyakki-ui-rpc/src/served_policy.rs`, `dist/split/rpc-manual.txt`). A core with no accounts refuses these calls by name. Tests: `iyakki-compose::remote_tiles` (real core, real socket: a guest named with an administrator's policy gets the guest's answer; an unknown name is refused; the station, download, study and LAB objects are the core's own).
* Not done, for the owner: `AccountsApi` calls take a `Session` (user, role and policy) from the UI (`create_user`, `delete_user`, `set_role`, `set_overrides`, `set_password`) and the story store and flows panel are built from a UI policy; the accounts calls are the next place to apply (a) (the core would read the acting account from the socket's own login). OWNER: say whether the acting person should be bound to the socket login instead of named per call (option (c) for every object).
* No core-api change. Rows: F-SPLIT-03, F-RADIO-05, F-MEDIA-01, F-LAB-01.

## D-SEC1-VOUCH (alpha.7, group SEC1, 2026-11-21): every right claimed over the UI socket is replaced by the core's own reading

* Question: D-SPLIT-03-POLICY fixed four calls that took the person's policy from the UI and listed the rest as not done: the accounts calls that take a `Session`, the story store, the flows panel. An audit of every served method found the same shape in 25 more: `scope_check_read`, `scope_check_write`, `scope_roots`, `scope_roots_with_home`, `rag_refresh_tick`, `FlowsFacet::open`/`answer_permission`/`start_for_login`, `StoreHub::tiles`/`add_dev_tile`, `StoreUpdates::update_check_run`, `DashHost::read_ok` (a Policy); `ClusterFacet::switch`, `FleetApi::start`/`stop` (a Role); `on_login`, `on_resume`, `on_logout`, `sharing_notice_tick` (an is-administrator flag); `computer_announce` (the list of administrators); the five `AccountsApi` calls (a Session); `SkillApi::add`/`remove`, `PackageApi::install`/`remove` (the can-share and can-own flags).
* Options: (a) one table of claims and one step before dispatch that overwrites each claim from the core's accounts by the account's NAME and refuses an unknown name; (b) change every method and its generator output one by one (the S3 pattern); (c) bind the acting person to the connection.
* Default implemented: (a) (`iyakki-ui-rpc/src/served_auth.rs`, called from `Host::call`). It keeps the wire shapes, so no generator, trait or core-api change, and a new method is caught by `served_auth_audit` until it has a rule. The story store holds the core's reading of the policy (a new `ServicesHandle::policy_of`, an agent wire call, not core-api). A core with no accounts refuses these calls by name. Tests: `iyakki-ui-rpc/tests/served_auth.rs`, `iyakki-compose::remote_services` (real services).
* Core-api: none. New served wire call (iyakki-ui-rpc, not core-api): `ServicesHandle::policy_of(user) -> Policy`.

## D-SEC1-SHARED (alpha.7, SEC1): the shared-skill right cannot be read from an account

* Question: `SkillApi::add`/`remove` and `PackageApi::install`/`remove` take `can_shared` and `can_own`. For a personal scope the scope names the owner, and the core now reads `can_own` from that account. The shared scope names nobody, so a UI that claims `can_shared` is believed.
* Options: (a) bind the acting person to the connection (the core records who signed in through `on_login`/`on_resume` and reads the right from it); (b) add the person's name to those four calls (a trait change in `iyakki-agent-api`, every caller changes); (c) accept it, because the UI is the trust base.
* Closed in alpha.8 by D-SEC3-BIND (option (a)). Was: (c) for now, recorded as an `Open` entry in `served_auth::ALLOWED` and in `docs/UI-SOCKET-SECURITY.md`. OWNER: choose (a) or (b); (a) also closes D-SEC1-APPROVED.

## D-SEC1-APPROVED (alpha.7, SEC1): the approval flag is the UI's word

* Question: the platform calls (`apply`, `apply_with`, `backup`, `connect`, `connect_existing`, `create_pull_request`, `power`, `push`, `remove`, `upgrade`) and `PackageApi::install` carry `approved: bool`, set by the card the person tapped in the UI. A UI program can send `true`.
* Options: (a) the core issues a one-time approval ticket when it shows its own plan, and the call carries the ticket; (b) the UI is the approval surface by design, the core re-checks the plan; (c) approvals move to the broker.
* Closed in alpha.8 by D-SEC3-TICKET (option (a)). Was: (b), as before. The core still builds the plan and the broker's table and log decide each change. OWNER: say whether (a) is wanted.

## W5E: graphics acceleration of the browser (2026-11-21, owner request)

- D-ACCEL-VIDEO-DEFAULT. Question: hardware video decoding (VA-API, F-BROX-21) was an administrator's tuning word; the owner asked for it on wherever the driver is usable. Options: (a) keep it a tuning word, (b) on by itself when a real card, its driver, the VA driver and GStreamer's VA plug-in are installed. Default taken: (b), implemented in `iyakki_shared::webaccel::plan` and added to the engine's start by `webengine::helper_env`; the person's switch, the administrator's switch and the per-site list turn it off; the old `vaapi` word still works. Owner confirmed 2026-11-21: on by default, Settings can turn it off.
- D-ACCEL-ABSTRACTION. `iyakki-uikit::accel` (no dependencies on other IYAKKI crates, so the shell, the tiles and the in-house engine share it) holds the probe, the fallback chain (card, wide processor path, plain processor path) and the golden harness. New public items: none in core-api (nothing crosses the core interface yet; the shell does not use it until F-BROX-26). New chat tools: `browser_accel_get` (free) and `browser_accel_set` (asks), no core-api change (agent tool wrapper like the graphics tools).
- D-ACCEL-STATE. Switches live in `<data>/web-profile/web-accel.json` (person, site list, administrator override); a damaged file means everything on (the safe default is the working card path, and the graphics mode and the freeze fall back still apply).

## D-ARCH-07-DISKTOOLS (alpha.7 group F2, 2026-11-21): typed core-api handles for the install, firmware and driver-store files
* Question: the agent files startupdisk, settingstools_install, devmode cli_adb, quickinstall(+uninstall), modpackcard, macfw and store/drivers mixed a kernel or disk call with agent types. How do they stop naming the device crates?
* Default implemented: a handle per kind of call, each with a fake and a real one. `installer::DiskTools` (BIOS plan and write, the look at a partition, the boot files of an image) is a new trait; `StartupDisk` gains `firmware_present` and `own_entry_first`; `Drivers` gains `pack_files` and `remove_pack` (the uninstall hook is the only caller and it has its own card and approval); the pure driver-pack modules are re-exported by `iyakki_core_api::drivers` (type-only, as D-ARCH-07-TYPES). Core-api items for owner review are in `docs/development/core-surface.txt` (the `drivers` re-export line was widened, so its old line became a `removed` line with a note).
* `kernel_single_reader`: 16 pairs of the core's boot, main and display and the shell's backend and touchbar_host moved to `iyakki_core_local::kernelfacts`. The other 25 stay: the owner crates (storage, devctl, usb, fw, netscan, linuxinit) and iyakki-shared are below core-local in the dependency order, stage1 must stay small, and the UI crates may not name core-local. OWNER: say whether to fold the shared and owner crates' readers into core-local by a restructure (a cycle breaker), or keep them as the named owners.


## D-SPLIT-03-TILES (final wave, group F3, 2026-11-21): the store's running tile apps over the core socket

* Question: `StoreApi::tile_open` (an HTML app run by the core's JavaScript page engine, driven tick by tick as `TileRt`), `tile_open_wasm` (a WebAssembly tile in the core's sandbox, a `TileRunner`) and `tile_pictures` (the pictures of a WebAssembly tile) answered "not served over the UI socket" and D-SPLIT-03-STORE said they needed the additive display-list message pair of the client protocol. How do they cross?
* Options: (a) the UI socket (the program's own object-call socket, not the client protocol of the browser page) serves them as remote objects: the page model and the tile's display list are the "display lists" the UI draws, and the client protocol is untouched; (b) add the display-list pair to the client protocol and route the tiles through it; (c) leave them unserved.
* Default implemented: (a). The pair of D-CLIENT-PROTO-DISPLAYLIST is not needed for the UI socket (it carries browser clients; the program already has an object-call transport with big values and handles), so nothing in `iyakki-core-api`, `core-surface.txt`, the client protocol or `engine.wasm` changed (no rebuild, the 655360 byte limit is untouched). The core runs the app; the program only draws.
  * `TileRt` is a remote object (`served_tiles.rs`, `RemoteTileRt`): the page model `iyakki_web::Doc` (serde derives added to the page-model types of `iyakki-web`, which now depends on `serde`) comes over when the page changed, `TileTake`, `TileEmbed`, `TileEmbedData` and `Request` have serde. A click or a key names a control of the page the core itself last handed out (`doc()`): the page the UI passes back is ignored.
  * The running WebAssembly tile is a new trait `WasmTileRt` (`iyakki-agent-api`; `TileRunner` implements it; `tile_open_wasm` answers `Box<dyn WasmTileRt>`). Events go to the core and the display list comes back in the tile ABI's own binary form (`EventBatch::encode` and `DisplayList::encode`, base64 inside the call); the program paints it with the host's renderer. `tile_pictures` crosses as base64 RGBA and the table's caps are applied again on the UI side.
  * The web object of the UI leaves the signature: the core runs the page with its own (`Host::with_web_fetch`, set by `iyakki_compose::rpc`: the browser's HTTP client behind the connection limits, the person's proxy setting, the ad and tracker filter and the known-bad site list; no page cache). The person's policy is read again from the core's accounts by name; a name the core does not know is refused.
  * The entry that runs is the core's own: a new `StorePublish::tile_listed(policy, user, key)` (default none; the agent lists the installed and dev apps) gives it and the UI's description of the entry is not used. An app that is not listed but names a folder (the IDE's Run preview) is accepted only when the folder lies where some account may read it (`Registry::project_dir`) and then runs as a developer app (`dev` and the developer trust label are set by the core, a name with a slash or `..` is refused). OWNER: as for the store's project folders, the socket does not yet know which person the program serves, so the rule is "some account".
  * A refusal of the core (the switch for WebAssembly tiles is off, the account may not run tile apps, the app is no longer installed) crosses as the core's own sentence; a lost core answers the quiet value (an empty page, nothing waiting) and the program ends with status 5 through `CoreWatch` as for every other object.
* Reason: no secret, program or closure of the UI is used by the core; every handle is dropped with the app (`rpc.release`). Rows: F-SPLIT-03.
* Core-api additions for owner review: none (no change to `iyakki-core-api`, the client protocol or `core-surface.txt`). Added to `iyakki-agent-api` (not a published surface): `WasmTileRt`, `StorePublish::tile_listed`, serde on `TileTake`, `TileEmbed`, `TileEmbedData`. Added to `iyakki-web`: serde on the page-model types (`Doc` and what it holds, the reading tree).

## H1: Manual tests tab (owner review)

* D-H1-1 Evidence `by` field. Question: the runner's migrator accepts a human result only with `by: owner`. Default: the tab writes `owner` for administrator accounts and `person` for everybody else; options: a team-member allow-list in the migrator, or `owner` for all. Needs A1 and the owner.
* D-H1-2 Answers imported from another machine are kept in `manual-tests/human/<machine>/` so they never replace this machine's files (the file name has no machine). Default: the migrator reads the flat folder only for its own machine; a per-machine folder read is a small change on A1's side.
* D-H1-3 Chat tools added (not core-api): `manual_tests_list`, `manual_test_open`, `manual_test_record` (asks), `manual_tests_export` (asks). New `DoctorPort` methods (default bodies, no core-api change). No core-api items added.
* D-H1-4 Checklist format: in-house `.chk` (`ScenarioParser` trait). When A1 publishes the scenario format, a second parser reads its NEEDS-HUMAN scenarios.

## Alpha.8 E1: the Doctor "Manual tests" tab joined with the autotest runner (owner review)
* D-E1-1 Resolves D-H1-1, D-H1-2 and D-H1-4. The tab now writes the runner's file names `human/<ROW>.<clause>.<machine>.json` (own machine in `human/`, imports in `human/<machine>/`; the runner's enforcer reads both) and exactly the runner's fields (no `who`, `scenario`, `when`, `result`); the first layout (`iyakki.human-evidence.1`) is still read, never written. A second `ScenarioParser`, `RunnerParser`, reads `dist/autotest/checklists/<ROW>.json`.
* D-E1-2 `by` is a role: `owner`, `admin` or `person` (allow-list `BY_ROLES`, in `humanschema.py` and in Rust). Default: the tab writes `admin` for an administrator account and `person` otherwise; it never writes `owner` because a device cannot tell the product owner from another administrator. The enforcer counts all three as a human pass and records a deterministic 10 percent sample of the passes of `admin` and `person` for the owner to look at again (`owner-review.json`); it never blocks a raise. Question for the owner: should the owner be able to mark an account as the owner (then `owner` is written)?
* D-E1-3 The machine id in an answer is the runner's kind (`lenovo`, `mac-mini-2014`, `mac-mini-2018`, `macbook-pro`, `hp`, `qemu`), found from the hardware text; the host name when none matches. The mapping of "Mac mini" without a year to 2018 is NOT made (such a machine files under its host name). Question: should the person pick the kind once in the tab?
* D-E1-4 An answer needs a known image commit (7 to 40 hex digits: the `g<hex>` of the release description). A machine that did not start from a release image gets a sentence and no file (the runner's schema requires it).
* D-E1-5 Chat tool added (not core-api): `manual_tests_import` (Risk::Ask, never pre-approved, never approved always). New `DoctorPort` methods with default bodies: `manual_checklists`, `manual_shot`. No core-api item added, no core-surface change.
* D-E1-6 The checklists and the screenshots they name are installed by post-build.sh into `/usr/share/iyakki/manual/checklists/`; the runner's PNGs live outside the repository (the runner's results folder), so none ship yet: the canvas draws a PNG only when the file is there and shows its name otherwise.

## D-E2-01 (alpha.8 E2, SDK batch: DISKS): the drive model is a leaf crate, the tile is a crate, no new core handle
* Question: the owner's brief asked for a typed core handle for unlock, format and resize behind approvals and a `DisksPort`. What is missing in the core?
* Finding: nothing. `iyakki-core-api::diskmodel::DiskEngine` already carries `plan_unlock`, `unlock`, `allow_writing`, `lock`, `plan_partition`, `apply_partition` and `take_format_request` (administrators only, typed drive name and a second confirmation checked again in the engine, audit line with the drive's name, size and serial, the key only to the helper); the chat tools (`disksunlocktools`, `disksformattools`) ask the person through the tile. So the work was the extraction, not a new handle.
* Default implemented (D-S2-01 option a): the module `iyakki-core-api::diskmodel` moved unchanged to the pure leaf crate `iyakki-disks-model` (depends on `iyakki-sec`, `iyakki-diskcrypt`, `iyakki-storage`); `iyakki_core_api::diskmodel` is now `pub use iyakki_disks_model::*`, so every path and shape stays. `iyakki_shared::disks_part::CANCEL_WINDOW_SECS` re-exports the constant now defined in the leaf. The tile is `iyakki-tile-disks` (state, keys, drawing, the `DisksPort` trait with five calls: engine, who, audit sink, the assistant's unlock request and format request); one allow line `iyakki-tile-disks iyakki-disks-model` in `tile_crate_deps.allow`. The shell's `disks_tile.rs` holds the real port and the `App` adapter (it also tells the engine where the person-installed programs are).
* Core-api for owner review: no new item. `core-surface.txt`: the 258 `crate::diskmodel` lines that the glob re-export hides from the scanner are recorded as `removed` lines with the note (the items still exist at the same paths); the same run added 8 lines for audio-route items that were already in the code (alpha.7 drift, additions only).
* OWNER: confirm the one allow line.

## D-E2-02 (alpha.8 E2, SDK batch: K8S): the cluster explorer's model is a leaf crate
* Question: the K8S tile names the model modules of `iyakki-shared` (`k8s`, `k8sexp`, `k8scr`, `k8sretry`, `k8sexptools`, `b64`, and `wsexec::Out`). How does a tile crate get them (D-S2-01)?
* Default implemented: option (a). New pure leaf crate `iyakki-k8s-model` (serde, serde_json, log only; no process started) holds those modules with their tests (46 tests); `iyakki-shared` re-exports every module under the old path (`pub use iyakki_k8s_model::k8s` ...). What stayed in `iyakki-shared`: the real `Kubectl` runner (it starts a program through `wsexec`), the runner `wsexec` itself (`Out` and `Sink` moved, re-exported), and the test that runs the real `kubectl` in a throwaway namespace (`k8sexp_real_tests.rs`). Test paths in FEATURES.md changed from `iyakki-shared::k8s*::tests::` to `iyakki-k8s-model::k8s*::tests::`. The flaky ratchet moved 6 `sleep` entries from `iyakki-shared` to the new crate (13 to 7 and 0 to 6, total unchanged).
* No core-api change. Allow line for the tile crate: not yet (the tile is not extracted, see the K8S rows' Notes for what blocks it).

## D-E3-01 (alpha.8 E3, SDK small apps): LEARN and DASH become tile crates on pure leaf crates
* Question: how do the LEARN (1.5k lines) and DASH (5k lines) tiles leave `iyakki-apps` while keeping the dependency rule of D-S2-01?
* Default implemented: option (a) of D-S2-01 for both.
  * LEARN: new pure leaf crate `iyakki-study-model` (the study data types and pure rules: scoring, dates, spacing, quiz check, lesson parse and clean; depends on `iyakki-clock` and `iyakki-sec` only); `iyakki-shared::{study, studylesson, studypdf}` re-export it under the old paths and keep the store (the person's file, the calendar link, saving a lesson), so no other crate changes. New tile crate `iyakki-tile-learn` asks one trait `LearnPort` plus two small objects (`LessonWriter`: the assistant writes and answers, the study files keep the lesson; `Pictures`: the shell's decoders draw a lesson's pictures); the voice (the TALES reader, engines and speaker) is behind `LearnPort::read_aloud`. The real port is `apps/learn_port.rs`. `study_pdf_export` stays the core's call with the policy never crossing (D-SPLIT-03-POLICY).
  * DASH: `iyakki-tile-dash` holds the screen, the charts, the query box and the repair banner. It asks two traits, `DashPort` (read an import and write an export under the person's file rules; `shell()` hands the shell's own context back to the chat pane) and `ChatPane` (the assistant's pane; the whole chat app is the shell's `ChatApp`). The engine's environment (`iyakki_dash::tools::Env` with the host, the audit sink and the accounts of this machine for sharing: `StorePeople`) is built by the shell and handed to `DashApp::new`. `iyakki-dash` gained `Env::for_test` (testing feature only) so the tile's tests run without the shell.
* OWNER: confirm the allow lines added to `crates/iyakki-core/tests/data/tile_crate_deps.allow`: `iyakki-tile-learn -> iyakki-study-model`, `iyakki-tile-dash -> iyakki-dash`, `iyakki-tile-dash -> iyakki-edit`. No core-api change (no `core-surface.txt` line). Rows: F-STUDY-01, 02, 04, 05, 06, 08, F-DASH-05, 06, 08 (tests added; Impl was already 100, Test unchanged until the rows' own proof exists).

## E4 (alpha.8, tile crates of the port-migrated apps): D-S1-E4-01 and D-S1-E4-02

* D-S1-E4-01 How a port-migrated app becomes a tile crate when its data types are `iyakki-shared` types with logic. Question: the tile rule (dependency_ratchet) lets a `iyakki-tile-*` crate depend on `iyakki-uikit` and `iyakki-tile-api` only. CHECK-UP and MESH use types of `iyakki_shared` (the step and result types and their verdict rules; the Consul answers) that cannot move into a tile crate without dragging the shared crate in. Options: (a) a pure leaf model crate re-exported by the old path with one allow line per tile (the BOARD precedent, D-S2-01); (b) plain mirrors in `iyakki-tile-api` and the verdict rules behind the port; (c) an allow line on `iyakki-shared`. Default implemented: (b), as the owner's brief for this lane asks, because `iyakki_shared::checkup` reaches `voicedesk`, `gpu` and `doctor`, and `iyakki-netscan` (the NET SCAN types) is a scanner with an HTTP stack, so neither is a pure leaf that (a) could name. What was written: `iyakki-tile-api::checkup` (outcome, step ids and titles, result, fix, what was heard, the `Check` the tile asks a verdict on, `MicCapture`, `Shot`, the sign bits, `CheckupPort`) and `iyakki-tile-api::mesh` (the Consul types, `MeshClient`, `MeshPort`); the shell converts both ways in `apps/checkup_port.rs` and `apps/mesh_port.rs` (a `Bridge` over the old shell port, so the old fake-port tests still run), and a mirror test per module keeps the two sides in step (names, titles, labels, order, sign bits, round trips). No new allow line, no core-api change. OWNER: say whether (b) stays the pattern for the remaining apps of the lane (Doctor, Runs, Scan, LinuxWin, DevTools) or whether a leaf crate per domain (a) is wanted where the types are pure.
* D-S1-E4-02 The ratchet that forbids a migrated tile to name `Services` matched the word anywhere, so MESH (a tab called Services) and NET SCAN (a field of that name) could not be listed. Default implemented: the check is word-aware (`names_the_services_type` in `iyakki-core/tests/dependency_ratchet.rs`): `Services` counts only as a whole word, outside string literals, where a type or a path to it stands (after `:`, `<`, `&`, `->`, `dyn`, `impl`, `as`, `use`, before `::`, or at the end of a module path such as `iyakki_agent::Services`); a variant of the tile's own enum (`Tab::Services`, `enum Tab { Services }`) and a label ("Services") are not the handle. The other forbidden names are specific text and are unchanged. Test: `the_word_services_is_the_type_only_as_a_whole_word_outside_literals_and_own_enum_paths`. OWNER: confirm the rule; it only narrows false positives, every real use of the handle in the tests of the checker is still found.
* Tile-api surface added (not core-api, no `core-surface.txt` line): `iyakki-tile-api::checkup::*`, `iyakki-tile-api::mesh::*`. New crates `iyakki-tile-checkup`, `iyakki-tile-mesh` (workspace members; `iyakki-tile-*` is already an `apps` layer glob). `apps/mesh.rs` joined `migrated_tiles.list` (4 entry points); `apps/checkup.rs` stays at 6.

## E5 (alpha.8, tile crates of DOCTOR and RUNS): D-S1-E5-01
* D-S1-E5-01 Same question as D-S1-E4-01 for Doctor and Runs. Default implemented: option (b) again. `iyakki-tile-api::doctor` holds plain mirrors of `Finding`, `Fix`, `Action` (as `FixAction`) and `Severity` plus `DoctorPort` (findings, a slow job and its finish, the text of the bundle preview, the places to save, save, audit, developer-mode extension). The brief also named `Facts`: the tile never reads facts (it asks the port for the judged findings), so the facts stay in the shell's `Bridge` like CHECK-UP's inventory, and no mirror of the large `Facts` struct was written. The `diagshare::Bundle` stays in the shell, the tile gets its preview text. The Manual tests tab is not mirrored: the shell wrapper owns `ManualTab` and the tile hands the tab to it through the small `ManualPane` trait of the tile crate (`is_open`, `open`, `handle`, `draw`, `wheel_px`, `take_dirty`). New crate `iyakki-tile-doctor`. No allow line, no core-api change. OWNER: confirm that (b) stays the pattern.
* D-S1-E5-01 (Runs part). The native RUNS tile is the new crate `iyakki-tile-runlog` (the name `iyakki-tile-runs` already belongs to the web page's WASM tile F-WEB-55). It holds the tile and the Jobs strip and list (`JobsUi`, 775 lines, shared with the chat, the AI IDE chat pane and the install indicator of the shell). Mirrors in `iyakki-tile-api::runs`: `RunMeta` (only the fields the screens read), `RunLog`, `RunEvent` (a tool call's arguments as one line of JSON text; the events the screen never draws are one `Hidden`), `Integrity`, `Job` (with its command line, which the shell's install indicator tells install jobs apart by), `JobState`; ports `RunsPort` and `JobsPort`; the tile takes both as one `RunsHost`. The chat keeps working through the same port: `jobs_ui::JobsCx` (apps) wraps the shell context. The strip's poll pacing now reads a clock the strip is given (real by default), so its tests run on a manual clock. Small moves that came with it: `ScrollLog` to `iyakki-uikit::scrolllog` (old path re-exported; its `lh` is now public so struct-update syntax in other apps still works), `iyakki_uikit::fx::spinner` (a third copy of the 10-line spinner of the dash tile and the browser chrome, which are untouched), `iyakki_shared::quickinstall::item_of_command` (the old `item_of` calls it), and the shell's `installpop.rs` and one line of `shell.rs` follow the plain `Job`. No allow line, no core-api change. OWNER: confirm the crate name `iyakki-tile-runlog` and that the chat depends on a tile crate for `JobsUi` (the only way to share it without a new crate).

## E6 (alpha.8, Scan and LinuxWin onto tile crates): D-S1-E6-01

* D-S1-E6-01 Scan follows D-S1-E4-01 option (b). `iyakki-netscan` (the scanner with its HTTP stack) stays out of the tile; `iyakki-tile-api::scan` holds plain mirrors (Kind, Confidence, Service, HostInfo, Subnet, Event, the device-watch Device and Neighbor), the `ScanFeed` the tile polls (no channel, no thread), the `DevWatch` view of the watch and the `ScanPort`. `hosts_of`, `parse_targets` and `now` are asked of the port, so the tile has no address math and no clock of its own. The shell converts in `apps/scan_port.rs` (mirror tests: labels, order, round trips, cidr); `Confidence` is reached through `iyakki_core_api::device_types::netscan` (no core-api change). Adding a found model server (Settings and vault writes, Lemonade's typed list) is `apps/scan_models.rs` behind `ModelsPort`, with the rules tested on a fake. No new allow line. OWNER: confirm (b) for Scan.
* D-S1-E6-02 LinuxWin follows option (b) too. `iyakki-tile-api::linuxwin` holds the plain mirrors (Program, Usage and its cpu figure, the keysyms), the `Screen` and `Window` traits (the shell's are the RFB link and the sandbox's display process), the `WinRig` (the sandbox's state and network switch, a blocking start that returns the window and its screen, the process-table usage, the apt install of a test program) and the `LinuxWinPort` (policy facts, the install job, the Command-chord flag of the input profile, a millisecond clock). The tile never builds a sandbox command or opens a socket; `iyakki_shared::linuxwin` (command, folders, namespaces, network switch) and its tests are untouched. The shell bridges in `apps/linuxwin_port.rs` (`ShellRig`, `ShellScreen`, `ShellWindow`, `Bridge`); `Screen` sends the picture whole when it is new or has another size and by the changed part otherwise, so the tile keeps only RGBA bytes and does not need `tiny-skia`. The shell keeps the `Drop` that clears the Alt pass-through. One small difference: the apt install's "no sandbox manager" error now reaches the person through the install's answer instead of before the thread starts. Mirror tests: constants and text, keysyms (all of them), programs, usage and cpu figure, framebuffer rectangles. OWNER: confirm (b) for LinuxWin.

## D-E7-01 (alpha.8 E7, SDK batch: K8S): the cluster registry and the cloud plan are a leaf crate; the tile is a crate
* Question: what blocked `iyakki-tile-k8s` (D-E2-02): `ClusterHost` (shared with the AI IDE), `k8s_cloud` (the agent's deploy kit and the `cloudlaunch`/`cloudroute` types), the cluster registry types and the shell widgets. How do they get behind ports or into leaf crates (D-S2-01 option a)?
* Default implemented: (a). New pure leaf crate `iyakki-cluster-model` (serde, serde_json, `iyakki-sec`, `iyakki-clock`; no process, no network): `clusters` (entries, `Registry`, rules, confirmation texts), `cloudlaunch`, `cloudroute` (without the certificate reader, which stays in `iyakki-shared` because it needs the TLS stack), `cloudnames`, `profiles` (the vault kubeconfig profiles file) and `choice` (the last-choice file and the managed kubeconfig note), `ids` (the two name rules that `k3sdev::valid_id` and `vault::valid_secret_name` now call). `iyakki-shared` re-exports every module under the old path; `ClusterEntryExt` (needs the managed k3s layout and the vault) stays in `iyakki-shared::clusters`. The tile `iyakki-tile-k8s` holds `ClusterSwitch` and `ClusterHost` (the AI IDE, Store setup and Settings reach them as `apps::cluster_switch` / `apps::cluster_host`; the shell constructor is `cluster_host::new(base, cx, workspace)`), asks `K8sPort` (new calls with defaults: `cloud_kit`, `host_save_enabled`, `has_ide_workspaces`, `instant`) and `ClusterHostPort` (local and remote status, the GPU look and summary, the role-checked switch, audit), and `CloudKit` (the five deploy calls; the real one forwards to the agent's `DeployKit`). `Rig` gained `local_kubeconfig`. The shell keeps the `App` adapter and the 107 whole-screen tests (they drive the real context); the crate has 11 tests over fake ports.
* Allow lines added to `tile_crate_deps.allow`: `iyakki-tile-k8s -> iyakki-k8s-model`, `-> iyakki-cluster-model`, `-> iyakki-sec` (the role). `migrated_tiles.list`: the lines of the two deleted shell files (`k8s_tile_ui.rs`, `k8s_cloud.rs`) were removed (the list shrinks). No core-api change.
* OWNER: confirm the three allow lines.

## D-E7-02 (alpha.8 E7, SDK batch: DEV TOOLS): MultiLine moves to the toolkit; the screen is a tile crate
* Question: DEV TOOLS needed `MultiLine`, the file read and clipboard behind a port, and the ICMP and port-check tools stay in the shell (D-S1-6).
* Default implemented: `MultiLine` (the multi-line text box, only toolkit dependencies) moved to `iyakki-uikit::multiline` (`apps::multiline` re-exports it, so Chat, Voice, Lab, Story and the IDE are unchanged). New tile crate `iyakki-tile-devtools` (toolkit, `iyakki-devtools-engine`, `iyakki-qr`) with one trait `DevToolsPort` (read a text file under the person's rules, home folder, save a picture, PNG writer, time stamp, audit, start ping, start port check, clipboard with the toolkit's clipboard as default) and `NetJob` (the running check). `apps/devtools/net.rs` keeps the sockets and implements `NetJob`; the random source of UUIDs is installed by the shell constructor. The shell's 26 tests run unchanged; the crate has 9 tests over a fake port.
* Allow lines: `iyakki-tile-devtools -> iyakki-devtools-engine`, `-> iyakki-qr`. No core-api change.
* OWNER: confirm the two allow lines.

## D-SEC3-BIND and D-SEC3-TICKET (alpha.8, lane SEC3, 2026-11-21): the acting person of a UI connection and the approval ticket

* Question: close D-SEC1-SHARED and D-SEC1-APPROVED and the "a name is believed" limit (owner delegated to the model, ALPHA-8-DECISIONS item 1).
* Taken: (a) of both. The core binds the account that signed in (verified password, or resume) to the connection; rules that name an account refuse another name; the shared right comes from the account; `approved` needs a one-time ticket (call, account, connection, argument hash, single use, 60 s, audited). Wire shapes of the served methods are unchanged: the ticket is one trailing argument that the core removes; `Approval::issue` is a new served wire call (iyakki-ui-rpc, not core-api).
* Choices to review: an administrator may act for another account only on `ADMIN_ACTS` (`on_logout`); `resume_session` and `on_resume` bind without a password and may not move a bound connection to another person (a fast user switch must sign out first); `on_login` verifies the password with the accounts (a second check, only while nobody is bound).
* Core-api: none. agent-api: none. New wire items (iyakki-ui-rpc): the `Approval` interface, method `issue(iface, method, args) -> ticket`; `Host::with_login_binding`, `with_ticket_clock`, `acting`, `open_tickets`; `TICKET_TTL_MS`.
* Rows: F-SPLIT-03 (notes), F-SPLIT-09 (new).

* D-N2-01 How Rag, Inbox, Lab and System were moved (alpha.8 lane N2). Question: which of the two steps of APP-DEVELOPMENT-PROCEDURE.md section 10 each app gets, when its data types are `iyakki_shared`/`iyakki_agent_api` types. Default implemented: RAG is extracted (step 4): `iyakki-tile-rag` depends on `iyakki-uikit` and `iyakki-tile-api` only; the hit, document and research-run types are plain mirrors in `iyakki-tile-api::rag` (D-S1-E4-01 option (b)), the worker threads, the audit lines and the kept runs are behind `RagPort`. INBOX, LAB and SYSTEM are moved onto a port (step 3, the pattern of RUNS, DOCTOR, SCAN): `InboxPort`, `LabPort`, `SystemPort` in `apps/*_port.rs` carry every use of the shell context, the three tiles join `migrated_tiles.list`, and each has fake-port tests in a child module; the crate extraction waits because their types (`iyakki_shared::inbox`, `iyakki_shared::lab`, `iyakki_core_api::sysinfo`, `iyakki_shared::{sysprocs, runs, k3sdev}`) are not yet plain mirrors or leaf model crates (D-S2-01 option (a) would make `iyakki_shared::inbox` and `iyakki_shared::lab` leaf crates re-exported by the old path). TALES (`story.rs`, one file of 10,466 lines with about 190 uses of the shell context across 100 functions) was only inventoried. OWNER: say whether (a) leaf model crates (inbox, lab) or (b) mirrors in tile-api is the way for the extraction of the three. No core-api change, no new allow line, no `core-surface.txt` line.
* New core-api items of lane N2: none. Tile-api surface added: `iyakki-tile-api::rag::*`. New crate `iyakki-tile-rag` (workspace member; `iyakki-tile-*` is already an `apps` layer glob).

## D-N1-01 (alpha.8 N1, for alpha.9): FLOWS becomes the tile crate `iyakki-tile-flows`
* Question: how does the FLOWS tile (7.3k lines, 14 files) leave `iyakki-apps` while keeping D-S2-01 and without breaking the generated socket code that reads the job traits of `iyakki-agent-api`?
* Default implemented:
  * New pure leaf crate `iyakki-flows-model` (serde, `iyakki-flow`, `iyakki-sec`, `iyakki-credguard`, sha2, log): the failure book (`failfix`), the webhook store and view (`hookstore`, `hookview`), the flow spec builder (`spec`), the program allow list (`programs`), the outside-message policy types (`channel`: `Policy`, `Held`, `Watch`) and the tile's jobs as traits (`port`: `FlowsMetaPort` ... `FlowsChannelPort`, `FlowsPort`). `iyakki-flowsx` and `iyakki-shared::flows_channel_policy` re-export the old paths (flowsx drops its shared, sec, sha2 and log dependencies). The `PolicyBook` file logic stays in `iyakki-shared`; `Policy::rules()` became the free function `flows_channel_policy::rules_of` because the type moved out of the crate that owns `Rules`.
  * New pure leaf crate `iyakki-credguard` (the credential detector, std only; `iyakki_shared::credguard` re-exports it) and `iyakki-querybox` (the query box of DASH and FLOWS: uikit plus `iyakki-edit`; `iyakki_tile_dash::query` re-exports it). `ScrollLog` moved into `iyakki-uikit` (`scrolllog`; `apps::ScrollLog` re-exports it).
  * The job traits of `iyakki-agent-api` (`FlowsMeta` ... `FlowsChannel`) are NOT moved: `dist/split/gen_flows_jobs.py` and the generated socket code read that file. The model crate has the same traits (data only, same types, minus `chat_handle`, which returns the agent API's chat handle) under the names `Flows*Port`; the shell's `flows_port.rs` (`JobsPort`) forwards every call (79 methods). A job method added later must be added in both places.
  * The tile asks the shell for three things only: `ChatPane` (the assistant beside the flow), `FlowsShell` (is the assistant allowed, read one file a person named for import) and `PictureFiles` (the program's own picture files for the run view and the report). The plain files of the owner's list are all behind ports: the live browser picture and the report's picture bytes (`PictureFiles`), the file import (`FlowsShell::read_import`) and the failure cards' book (`FailStore` in the model crate; the shell gives the tile a `FailBook` opened on the account's flows folder, the tests an in-memory one).
  * Tests: the 61 screen tests of `flows_tests.rs` and `flows_jobs_tests.rs` stay in `iyakki-apps` and pass unchanged (tile reached through `Deref`); 15 fake-port tile tests and the canvas geometry tests run in the crate.
* Allow lines added to `tile_crate_deps.allow`: `iyakki-tile-flows` -> `iyakki-flow`, `iyakki-flows-model`, `iyakki-querybox`, `iyakki-clock`; `iyakki-tile-dash` -> `iyakki-querybox` replaces `iyakki-tile-dash` -> `iyakki-edit`. OWNER: confirm.
* No core-api change (no `core-surface.txt` line).
* Remaining: nothing in code for the extraction. The SDK migration of FLOWS itself (the tile on `iyakki-ui-sdk` widgets, D-S1/D-S2) is the later step; it is not part of this lane.

## D-N1-02 (alpha.8 N1, for alpha.9): GIT becomes the tile crate `iyakki-tile-git` on the leaf crate `iyakki-git-model`
* Question: how does the GIT tile (3.9k lines in four files; also drawn by the AI IDE's Git tab) leave `iyakki-apps` while keeping D-S2-01?
* Default implemented:
  * New pure leaf crate `iyakki-git-model`: `gitsite` (addresses, `RepoEntry`), `gitconn` (connections, `Conns`, the host API client and its fake), `gitbrowse` (the read-only repository browser), `gitjobs` (the plain data of the jobs: `Op`, `Phase`, `ItemState`, `CloneSpec`, `RepoStatus`, `Tools`, `Probe`, `HostPrompt`, `Item`, and the new `Batch` trait), `hostkey` (`HostKey`), `keys` (`Auth`, `Who`, `AuditFn`, `SecretStore`) and `isotime`. `iyakki-shared` re-exports every module under the old path. What stayed in shared: `GitKeys` and the host-key store (`gitkeys`, `ssh`), the job engine context (`gitjobs::Ctx`, `CtxFor`, which hold the keys) and `gitdiscover` (cluster Gitea).
  * Moves it needed: `Runner`, `Call` and `FakeRunner` to `iyakki-k8s-model::wsexec` (next to `Out` and `Sink`; the process-starting `System` runner stays in shared), `redact` to `iyakki-credguard`, `Job`, `Line`, `TextView`, `ListState` and `draw_fit` from the K8S tile and `lab_pic` to `iyakki-uikit::tilekit` (K8S and the tile both use them).
  * The tile asks the shell for one trait, `GitPort` (user, data folder, default Repositories folder, write and network rules, audit, the connections, the AI IDE workspaces, the three job plans, picture size and thumbnail). `GitBatch` of the agent API is mirrored by the model's `Batch` (the generated socket code reads the agent API's trait, so it is not moved); `git_port.rs` wraps one in the other (`ShellBatch`) and implements `GitPort` for the shell context (`AppCx`).
  * The wizard (`git_wizard.rs`, the Settings Git connection wizard) is not part of the tile: it uses the keys, the cluster probe and quick install, which are Settings' business; it shares only the connection dot (`iyakki_tile_git::git_tile_ui::dot_of`).
  * Tests: the 82 existing screen and wizard tests (`git_tile_tests.rs`, `git_wizard_tests.rs`, `ide_gitview_tests.rs`) stay in `iyakki-apps` and pass; 10 fake-port tile tests run in the crate.
* Allow lines added: `iyakki-tile-git` -> `iyakki-git-model`, `iyakki-k8s-model`. OWNER: confirm. No core-api change.
* Remaining (not code of this lane): none for the extraction. `FEATURES.md` rows keep their numbers (Impl was already 100, nothing new is claimed).

## D-P5-01 (alpha.9 P5, for alpha.10): TALES model leaf crates and the LAB tile crate `iyakki-tile-lab`
* Question: how do TALES and LAB leave `iyakki-apps` under D-S2-01 and D-P1-01?
* Default implemented:
  * New pure leaf crates, each re-exported by `iyakki_shared` under the old path (no caller changed): `iyakki-story-model` (the story options and enums, the prompt text, `parse_story`, sentences and speech pieces, the `Synth`/`Playing`/`Sink` traits, `Status`/`Snapshot`, and the picture options `Coverage`/`Look`/`Quality`/`PictureOpts`; `ModelChoice` lives here and `iyakki_shared::settings` re-exports it), `iyakki-speechtime`, `iyakki-provider-kind` (`Kind`, `ImageTrace`, `ImageDefaults`), `iyakki-serde-static`, `iyakki-frames` (`png`, `fit`), `iyakki-lab-model` (the whole of `iyakki_shared::lab`).
  * `iyakki-tile-lab`: the LAB tile (app, ui, pictures) on `LabPort` and a `Rig`; the picture decoders reach it as a `PictureCodec` the rig carries (the shell's real codec is `ShellCodec` in `lab_port.rs`). `resize_box` and `clean_name` moved to `iyakki_uikit::tilekit` (chat keeps its old paths). The 954-line `lab_tests.rs` stays in `iyakki-apps` over the real services; 6 fake-port tests run in the crate.
* Allow lines added: `iyakki-tile-lab` -> `iyakki-lab-model`, `iyakki-clock`, and `iyakki-testkit` (dev-dependency). OWNER: confirm. No core-api change.
* Remaining (not code of this lane): the TALES tile crate itself and the INBOX tile crate (see the P5 report); both need the mirrors listed in D-P1-01.

## D-P5-02 (alpha.9 P5, for alpha.10): INBOX becomes the tile crate `iyakki-tile-inbox` on plain mirrors in `iyakki-tile-api`
* Question: how does INBOX (notifications, tasks, approvals; the chat also shows its browser questions) leave `iyakki-apps` under D-P1-01?
* Default implemented:
  * `iyakki-tile-inbox`: the tile, `InboxPort` (the person, the admin flag, the audit line, the 17 calls of the inbox store, the calendar as a `CalendarSource` of the existing calendar mirror, the secret requests and the vault answer, the browser questions and their answer, the device trust, the pending approvals and their answer, the thumbnail decoder), and the browser-consent dialog (`consent.rs`, drawn once).
  * New mirrors in `iyakki-tile-api::inbox`: `PendingApproval`, `SecretRequest`, `ConsentRequest`, `ConsentKind`, `ConsentChoice`, `Answer` (field for field the agent's shapes). The shell converts in `inbox_port.rs` and `browser_consent.rs`; tests round-trip every kind, answer and risk word. The agent's `ConsentDialog` of the chat is now a thin wrapper that converts and delegates drawing and keys to the tile's dialog (nothing is drawn twice).
  * `OwnedCalendar` (in `apps/calendar.rs`) is the calendar adapter over an owned store and zone, for ports that hand a calendar out.
* Allow lines added: `iyakki-tile-inbox` -> `iyakki-inbox-model`, `iyakki-clock`. OWNER: confirm. No core-api change.
* Remaining (not code of this lane): none for the extraction.

* D-P3-01 Voice moved onto a port (alpha.9 lane P3). Question: how the VOICE tile (seven files, 65 uses of the shell context) reaches the system. Default implemented: step 3 of APP-DEVELOPMENT-PROCEDURE.md section 10 (the pattern of INBOX, LAB, SYSTEM): `VoicePort` in `apps/voice_port.rs` carries the signed-in person, the voice right, the administrator right, the audit line, the voice settings (read and one `update_voice`), the dictation desk facet, the wording corrector (a `Fixer` closure a worker thread runs), the revision kit, the wake phrase helpers (experimental, off by default: session request, the person's counts, the cloud-stream card), My IYAKKI's waker and worker, the person's folders and the read check. The real port `CxPort` is the only place that names `Services`, the session, the audit log, the agent and the core; `VoiceApp::with_port` takes any port. The seven files join `migrated_tiles.list` (voice.rs keeps 5 entry points: the import, `new`, `with_rig`, `handle`, `render`). 9 fake-port tests in `voice_fake_tests.rs`; the 99 existing voice tests run through the real port, only their direct calls of inner methods now pass `&CxPort(&cx)`. NOT extracted to a tile crate: the tile draws with the shell's `Gfx`, keeps `MultiLine`, `TextField`, the path picker, the clipboard and `Rig` (recorders, sink) in `iyakki-apps`, and takes `iyakki_shared` types (voice settings, `dictfix`, `revise`, `story`, `voicedesk`) and `iyakki_agent_api` facets that `iyakki-tile-*` may not name; the extraction needs the pure leaf model crate of D-S2-01 option (a) (voice settings, dictfix, revise, voicedesk, story, wake counts) plus plain mirrors of the facets, which is a lane of its own. New core-api items: none.
* D-P3-02 Store moved onto a port in steps (alpha.9 lane P3). Question: how the STORE tile (8,300 lines, 107 mentions of the shell context) reaches the system. Default implemented: `StorePort` in `apps/store_port.rs` (the pattern of INBOX, LAB, SYSTEM, VOICE; the port hands out the already-served handles: the quick-install facet `b0`, the second facet `b4`, the agent facade, the core, the settings, the development cluster, the quick-install network reader, plus the person, the role name, the two rights, the audit line, the data folder and the read check). Step 1 moves the SETUP section: `store_setup.rs`, `store_setup_view.rs` and `store_setup_remove.rs` take `&dyn StorePort` and join `migrated_tiles.list` with 0 mentions of `AppCx`; `store.rs` hands its real port (`CxPort`) to the Setup at the eight places it calls it. 6 fake-port tests in `store_setup_fake_tests.rs` (rights, machine read, restart, show-all-AI switch, removal card); the 200 store tests that existed run through the real port (only their calls of Setup methods pass `&CxPort(&cx)`). The port hands out the agent facade and the core as handles (both have fakes: `FakeAgentApi`, `FakeCore`), which is the step the owner approved for apps that cannot use a pure leaf crate; narrowing them to one method each is parked with the extraction. Step 2 moves `store_manage.rs` and `store_rating.rs` the same way (the port adds the policy, a `recorder` handle for worker threads that record when done, the narrow store facets and the machine's driver reader; `store.rs` keeps `reload(&AppCx)` as a one-line bridge to `reload_port(&dyn StorePort)`, `actor` takes the port); 2 more fake-port tests (`rating_tests.rs`). Not yet moved: `store.rs` itself (about 55 mentions in its 49 methods and its background jobs that capture the services), `store_drivers.rs` (it names no shell context already). Not extracted to a tile crate: the section draws with `Gfx` and takes `iyakki_shared` quick-install types and `iyakki_agent_api` facets that a tile crate may not name. New core-api items: none.

## D-P6-01 (alpha.9 P6, for alpha.10): TALES leaf crates for chat, scene, picture log and the private-host rule
* Question: which pure parts of the TALES code leave `iyakki-shared` so the tile crate can name them (D-S2-01 option (a), D-P1-01)?
* Default implemented: new pure leaf crates, each re-exported by `iyakki_shared` under the old path (no caller changed): `iyakki-chat-model` (`Message`, `Role`, `ToolCall`, `ToolSpec`, `ModelTurn`, the `ChatBackend` trait, `Unconfigured`, `trim_history`; the OpenAI-compatible client stays in shared), `iyakki-scene` (the whole of `iyakki_shared::scene`: scene JSON, vector art, character sheet, rich SVG prompt and reply checks, plus `svg_messages` and `image_prompt_with` that moved here from `story.rs`), `iyakki-piclog` (the picture log, on `iyakki-provider-kind::ImageTrace`), `iyakki-hostrules` (`is_private_host`, one function). Layers: base. 5 + 4 + 2 new direct tests; the existing tests run through the re-exports.
* No core-api change, no tile allow line yet (the tile crate that would need them is not written, see D-P6-03).

## D-P6-02 (alpha.9 P6, for alpha.10): SYSTEM becomes the tile crate `iyakki-tile-system` on a leaf model crate and plain mirrors
* Question: how does SYSTEM (1,502 lines, three test files) leave `iyakki-apps` under D-S2-01 and D-P1-01?
* Default implemented:
  * New pure leaf crate `iyakki-sysinfo-model` (std, serde_json, `iyakki-credguard`): the page's figures and rules (`Memory`, `human_bytes`, `human_duration`, `SysRoots`, `CpuTimes`, `cpu_percent`, `NetCounters`, `DiskCounters`, `Proc`, `NetUse`, `DiskUse`, `proc_cpu`, `protected`), the `Kubectl` trait and the whole of `sysprocs` (pod table, cache, rows, groups, redaction). `iyakki_core_api::sysinfo`, `iyakki_shared::kubectl` and `iyakki_shared::sysprocs` re-export every item (the core-surface list is unchanged). The owner's list asked for plain mirrors of these in `iyakki-tile-api::system`; a leaf crate re-exported by the old path was taken instead because it needs no conversion code and no second copy (D-S2-01 default), the tile names it through an allow line.
  * Plain mirrors in `iyakki-tile-api::system` (the data that has no pure home): `LiveRun`, `WasmTask`, `ClusterHealth` (with the one line the page prints), `MemStats` and `Zram`. `OwnedProc` is the tile crate's own type (it carries `Proc`, `NetUse`, `DiskUse` of the leaf crate, which `iyakki-tile-api` may not name). `SysRead` (the tile-side read trait in place of `HostFacts`, 11 calls) is in the tile crate; the shell's `HostRead` wraps the core's host facts. The graphics cards (`gpu_lines`), the machine name (`hostname`) and the about line (`machine_about`) come through the port, so the tile no longer reads `iyakki_shared::{gpu, relinfo}` or `crate::hostname`.
  * `fmt_tokens` moved to `iyakki_uikit::tilekit` (the Usage tile re-exports it).
  * Tests: 9 fake-port tests in the crate (the 6 old ones moved, 3 new), 6 in the leaf crate, 4 conversion tests in `system_port.rs`; the 18 screen tests over the real services stay in `iyakki-apps`. The tile's state fields are `pub` so those tests (reached through `Deref`) keep reading them.
  * `text_report` (`iyakki top`) stays in the shell file: it reads the core's host facts directly and is called by `iyakki-core`.
* Allow lines added: `iyakki-tile-system` -> `iyakki-sysinfo-model`, `iyakki-clock`. OWNER: confirm. Core-api: no new item and no shape change; `core-surface.txt` records the moved sysinfo items as `removed` lines with a note and one `pub use iyakki_sysinfo_model::{...}` line (the same way as `diskmodel`), for owner review.
* Remaining (not code of this lane): none for the extraction.

## D-P6-03 (alpha.9 P6, for alpha.10): TALES tile crate not extracted; the exact remainder
* Question: can `iyakki-tile-tales` leave `iyakki-apps` now?
* Answer: not in this lane. What the tile (about 6,200 lines in `apps/story/`, 4,252 lines of screen tests, 467 of fake-port tests) still names outside the toolkit and the leaf crates, with what each needs:
  * `ChatView`, `NoPics`, `Ctx`, `set_mono_boost`, `TOGGLE_BASE` (`chat_view.rs`, 2,348 lines) and `PicCache` (`chatpic.rs`, 668 lines): they need the toolkit, `iyakki-chat-model` and `iyakki-web` only, so they can become one UI-layer crate (like `iyakki-querybox`); `chatpic::save_file` takes the policy and stays in the shell.
  * the model chooser (`model_picker.rs`, 1,338 lines: `ModelList`, `Pick`, `Current`, `TopRow`, `key`, `Out`, `draw`): `build`, `remember_custom` and the list of external agents read the shell's context; the state and the keys can join the UI-layer crate, the three shell calls stay behind `TalesPort` (the brief's mirrors would be copies of this state, so the move is cleaner).
  * speech: `SpeechEngine`, `SpeakOpts`, `VoiceSettings`, `Highlight` (`voice.rs`, which names `gemini`, `hostengines`, `providers`, `audioout`), `hostvoice` (`lang_tag`, `BrowserSynth`, `BrowserSink`), `hostengines` (`Env`, `Report`): a leaf `iyakki-voice-model` that takes those four modules' pure parts, or mirrors.
  * `iyakki_shared::story` still holds about 1,000 lines the tile calls (`StoryStore` with its file scopes, `render_audio`, `assemble_audio_timed`, `export_html`, `follow_agent_run`, `PrebuiltSynth`, `StoryChat`, `Prefs`) and `storyillus` (1,074 lines: `Look`, `Cand`, `Readiness`, `Resolved`, `job_estimate`, `check_caps`, `reason`); `StoryStore` needs a trait over the library files, the rest are pure once `voice` and `scene` are leaf crates.
  * port types: `AgentsCfg`, `PullStatus`, `lemonade::Model`, `StoryPdf`, `ImageParams`, `QuickEngine`, the external-agent run (`ExtRunSpec`, `RunHandle`, `RunLimits`, `NoTools`, `ExternalAgent`), `HostTtsApi`, `DirectVoice`, `LinkedPicture`, `Os`: plain mirrors in `iyakki-tile-api` (about 25) with conversions in `story_port.rs`.
* Default taken: stop at the clean boundary (everything that moved is green and re-exported). The order for the next lane: UI-layer chat view crate, then `iyakki-voice-model` and the `story`/`storyillus` leftovers, then the mirrors, then the crate move (`git mv story/` and one `Cargo.toml`; the 9 fake-port tests move with it, the 4,252-line screen tests stay and reach the tile through `Deref` like INBOX and SYSTEM).
* No core-api change, no allow line.

## Q4 (alpha.10, Store and Voice extraction steps): D-Q4-01
* Question: how far do the STORE and VOICE tiles get toward tile crates (D-S2-01 option a)?
* Default implemented: STORE step 3 is finished: `store.rs` now keeps six mentions of `AppCx` (the import, `store_of` that other tiles call, `new`, `open_item`, `handle`, `render`, which wrap the context in `CxPort` once); every other method takes `&dyn StorePort` and the audit line, the person, the policy, the file-scope check, the pending-licence app data (through `core()`), the update-check setting (through `settings()`), the private store status and stick stores (through `facets()`), the folder package and the Settings rows go through the port. One port method was added, `trusted_keys` (the publisher keys the person trusts); the fake in `store_setup_fake_tests.rs` answers it. `store.rs` is in `migrated_tiles.list` with 6; the inline tests run through the real port. VOICE: new pure leaf crate `iyakki-voice-model` (layer base; depends on `iyakki-story-model` and `iyakki-chat-model`) holds `dictfix` and `revise`; `iyakki_shared::dictfix` and `iyakki_shared::revise` re-export it. The crate has 12 unit tests (the two modules had none of their own). No allow line yet (no tile crate names it), no core-api change.
* Not done, and why: `iyakki-tile-store` and `iyakki-tile-voice` are not written. The Store tile still names `iyakki_shared::{quickinstall, store_*, skills, store_private}` and the agent's `store::drivers::Drivers`; the quick-install model (`Item` and its text) reaches `avaddon`, `datasci`, `fwpack`, `fwextra`, `regsetup`, `gitconn`, `jobs`, `localreg`, so it is not a pure leaf yet. VOICE settings (`voice.rs`) and `voicedesk` reach `gemini`, `hostengines`, `providers`, `audioout`, `micpcm`, `speechtime`, `iyakki_drivers::cs8409`, `iyakki_storage`; the wake counts are in `wakeshare` and `wakelisten`. Each needs its own leaf split first (OWNER: say whether the quick-install `Item` model should become a leaf with those hooks behind a small trait, or plain mirrors in `iyakki-tile-api`).

## D-Q3-01 (alpha.10 Q3): the chat transcript view and picture cache move to the UI-layer crate `iyakki-chatview`
* Question: first step of the D-P6-03 remainder (ChatView, PicCache).
* Done: `iyakki-chatview` (layer ui; depends on uikit, `iyakki-chat-model`, `iyakki-web`, `iyakki-wordtime`, tiny-skia only) holds `chat_view`, `chatpic` (without `save_file`) and `pixmap` (`to_pixmap`, `bitmap_pixmap`, `shrink`, moved from `browser_images`, which re-exports them). `iyakki-apps` keeps the old paths (`apps::chat_view`, `apps::chatpic` re-export; `save_file` stays in the shell because it asks the policy). The pure card words and rules (`split_card`, `split_summary`, `ALWAYS_ASKS`, `SESSION_COVERS`, `ASK_NOTE`, `takes_pictures`, `SPOKEN_MARK`) moved to `iyakki_chat_model::cardtext`, re-exported by `iyakki_shared::{grants, camtools, voiceassist}`; the view calls `takes_pictures` instead of the agent facet. No allow line (not a tile crate), no core-api change. Tests moved with the code (19 in the crate, 2 new in `cardtext`).
* Left of D-P6-03: the model chooser, `iyakki-voice-model`, the `story`/`storyillus` leftovers, the mirrors and `iyakki-tile-tales` itself.

## D-Q3-02 (alpha.10 Q3): the model chooser's state, keys and drawing move into `iyakki-chatview`
* Done: `iyakki_chatview::model_picker` holds `ModelList`, `MRow`, `Current`, `TopRow`, `PGroup`, `Pick`, `Out`, `key`, the drawing, and a pure `build(Inputs)`. The router's zone is mirrored by a three-value `Zone` (same order); no `iyakki_shared` type moved (the shell fills `Inputs`: the filtered models, `ProviderInput` per provider with label, zone and prefix, `AgentInput` per external agent with its models, the unusable pairs and the recent list). The three shell calls stay in `apps/model_picker.rs`: `build(cx, ..)` (gathers `Inputs`, refreshes the agents' status), `remember_custom` (the settings write behind the manage-settings right) and `draw(.., cx, ..)` (passes the agent rows and the login-tab lookup). Old paths unchanged. Adds `iyakki-extagentx` (a base leaf) to the crate's dependencies. 3 new tests in the crate; the 124 picker, chat_ext and story tests stay green.

## D-Q3-03 (alpha.10 Q3): `iyakki-voice-model`, the speech leaf crate
* Done: new base leaf `iyakki-voice-model` (serde, serde_json, `iyakki-sec`, `iyakki-wordtime`, `iyakki-story-model`) holds the whole of `voice` (`SpeechEngine`, `SpeakOpts`, `VoiceSettings`, `Highlight`, WAV and command helpers), `hostvoice` (`lang_tag`, `BrowserSynth`, `BrowserSink`, the host voice list framing), `hostengines` (`Env`, `Report`, `Kind`, the validators), the PCM converter `pcm::to_pcm16_wav` (from `audioout`), and the three small items the speech types needed from elsewhere: `Tier` and `KEY_REF` (from `gemini`), `valid_model_name` (from `providers`) and `which` (from `story`). `iyakki_shared::{voice, hostvoice, hostengines}` re-export the modules; `gemini`, `providers`, `story` and `audioout` re-export the moved items, so no caller changed. The `MAGIC` row of compat-surfaces.txt names the new file (same bytes, no wire change). No core-api change, no allow line. 2 new tests in the crate; 508 shared and 109 voice screen tests stay green.

## D-Q3-04 (alpha.10 Q3): `iyakki-story-kit`, the TALES helpers that are not data
* Done: new base crate `iyakki-story-kit` (story-model, voice-model, chat-model, k8s-model for `b64`) holds `messages`, the audio helpers (`envelope`, `speech_edges`, `assemble_audio`, `assemble_audio_timed`, `split_audio`, `PrebuiltSynth`, `render_audio`) and the HTML export (`export_html`, `export_html_with`, `html_escape`, `mime_of`). `title_from_topic` moved into `iyakki-story-model`. `iyakki_shared::story` re-exports every item. 3 new tests in the crate (the prebuilt synth, rendering with cancel and a missing piece, the empty level and title). No core-api change, no allow line.
* Still in `iyakki_shared::story` (about 1,150 lines): `StoryStore` (file scopes and the policy: needs a trait over the library files), `StoryChat`, `Prefs`, `plan_model`, the hint parsers (`parse_hints*`, pure, movable next), `follow_agent_run` (names the agent's run handle: needs the run-type mirrors), `quick_engines`/`tts_status*` (name providers and lemonade); and the whole of `storyillus` (1,074 lines, not started).

## D-Q3-05 (alpha.10 Q3): `storyillus` and Lemonade's model types leave `iyakki-shared`
* Done: `iyakki_shared::storyillus` (1,074 lines: `Readiness`, `Cand`, `Resolved`, `resolve*`, `compose`, `job_estimate`, `check_caps`, `reason`, the timing words) is now `iyakki_story_kit::storyillus`, re-exported by the old path; its 21 tests moved with it (FEATURES.md cites the new path). `lemonade::Model` and `ModelType` (with their words) moved to `iyakki_provider_kind::lemonade` (re-exported by `iyakki_shared::lemonade`); `iyakki-story-kit` now also depends on `iyakki-provider-kind`, `iyakki-scene`, `iyakki-sysinfo-model` and serde (all base leaves). No core-api change, no allow line.

## D-Q4-02 (alpha.10 AP2): STORE mirror group 1, the quick-install item, its facts and its job state
* Question: first step of D-Q4-01 (plain mirrors in `iyakki-tile-api`, conversions in `apps/store_port.rs`).
* Done: `iyakki_tile_api::store_qi` holds plain copies of `Item` (39 variants, same order), `Group`, `Presence`, `Latest`, `Job` and `JobState`, `Launch`, `HwFirmwareOpts` and `Facts`, plus `ItemInfo`, a table row that carries what the real item answers (id, title, about, licence, group, needs, download size, time, RAM, service/hardware/job/npm/pip flags, CLI id). The words are not copied into the tile crate: `store_port::item_table()` asks the real item once, so a changed text has one home. Conversions live in `store_port.rs` (`item_to_mirror`/`item_from_mirror` and the same pairs for each type); the matches have no catch-all arm, so a new real variant does not compile until it is mapped. The runtime ids of `Facts` are `&'static str` in the real type and come back through `serde_static::intern`. 7 mirror tests in `store_port.rs` (every item round-trips and keeps its place, every group, the table equals the real answers for all 39 items, presence, latest, jobs and states, launch and options, facts) and 2 in the tile-api module.
* Not done in this step: the Store files still call `B0Services` and name the real `Item`; they move onto the mirrors when `StorePort` gets mirror-typed methods (`qi_gather_facts`, `qi_install_jobs`, `qi_start*`, `qi_check_upgrades`, `qi_uninstall_plan`) in the next step. `Item::fw_def` (the `fwpack::Def`, 20 uses) waits for the fwpack mirror (group 3). `qi::resources`, `qi::ollama_cluster`, `qi::order` and `qi::order_text` stay behind the port as calls (pure but they take the real `Item`).
* No core-api change, no allow line (nothing names the mirrors yet; `iyakki-tile-store` does not exist).

## D-Q4-03 (alpha.10 AP2): the Store files run on the mirrors of group 1; one table, one trait
* Done: `store_setup.rs`, `store_setup_view.rs`, `store_setup_remove.rs` and the Setup parts of `store.rs` now use `iyakki_tile_api::store_qi::{Item, Presence, Facts, Latest, Job, JobState, Launch, HwFirmwareOpts, FwOutcome, UnPlan, UnOpts}` and `store_qi::ollama_cluster` (plain mirrors of the Ollama move types and words). `StorePort::qi()` returns `Arc<dyn QuickInstall>`, a mirror-typed trait (facts, jobs, upgrades, the starts, removal plan and removal, dependency lines, the install order's runtime titles, the memory warning, the Ollama host decision, the firmware pack calls). The real one is `RealQi` in `store_port.rs` (b0 facet and agent facade underneath, conversions here); the fake port uses it over `NoB0`.
* The item's words are not copied: `ItemInfo` rows carry them, held in one process-wide table (`store_qi::set_table`, first call wins, the shell calls `store_port::install_table()` from `StoreApp::new`, `CxPort::qi` and the tests). Reason: the tile calls `item.title()`, `.id()`, `.licence()`, `.needs()` about 140 times, some from the drawing code that has no port; a per-call port question or a table field in every struct would have changed all of them. The table is the same for every caller, so a first-wins `OnceLock` carries no state of its own. If the owner prefers an explicit table handed to `StoreApp`, only `Item::info` changes. A tile crate test must call `set_table` with a fixture (the group 5 step).
* Pure helpers that took the real item are duplicated on the mirrors with parity tests against the real ones for every item and several present-sets: `order`, `order_text`, `titles_text`, `items_of`, `failed_items`, `item_of`, `item_of_command`, `failure_reason`, `Presence::word`/`present`, `FwOutcome::reboot_pending`, and the Ollama words. The removal plan arrives as both reviews (data kept, data deleted) computed by the real plan, so the review logic is not copied.
* Still on real types in the Store files: `iyakki_agent_api` (`agent()`, `b0()`, `facets()`), the camera and Mac firmware outcomes, `HwView`, `avbuild`, `datasci`, the private-store and catalog types (groups 2 to 4). No core-api change, no allow line.

## D-Q4-04 (alpha.10 AP2): groups 2 and 3 become a leaf crate, not mirrors: `iyakki-store-model`
* Question: the Store files name the pure STORE modules of `iyakki-shared` (catalog, trust, install review, installed list, lifecycle, organize, ratings, licence, update check; about 5,000 lines with their rules and tests). D-Q4-01 chose plain mirrors for the quick-install item, whose hooks reach `avaddon`, `datasci`, `fwpack` and the agent. Do these follow?
* Default implemented: no. The modules start no process and touch no agent (they need `ed25519-dalek`, `sha2`, serde, `iyakki-vfs`, `iyakki-clock`, `iyakki-credguard`, `iyakki-schema`, `iyakki-proxy::Upstream` for `HttpFetch` and three small `iyakki-core-api` items for the install review); mirrors would copy the trust rules, the filters and the review logic (about 2,500 lines of logic with 37 tests) and the copies would drift. So D-S2-01 option (a) applies: the new base crate `iyakki-store-model` holds `skills`, `pkg`, `wtool`, `store_manifest`, `store_trust`, `store_revocation`, `store_catalog`, `store_core`, `store_bundle`, `store_install`, `store_installed`, `store_lifecycle`, `store_organize`, `store_ratings`, `store_updatecheck`, `store_license`, `store_licencelink`, `store_private` (git mv, history kept) and `iyakki-shared` re-exports each under the old path, so no caller changed. The crate has a `testing` feature that exposes the catalog's test helpers and `pkg::new_signing_key` to the tests of `iyakki-shared` (`store_hosting`). `iyakki_shared::skills::Scope`, `store_core::*`, `store_organize::item_key`, `store_trust::Level`, `store_updatecheck::{Mode, Kind}`, `store_revocation::Freshness`, `store_bundle::MemberState`, `store_ratings`, `store_licencelink`, `store_private::status_text`, `pkg` (`MAX_EXTENSION_PACKAGE_BYTES`, `public_key_hex`) are therefore all the same types the tile will name from the leaf. The FEATURES.md test paths of these modules now read `iyakki-store-model::...`; `compat-surfaces.txt` lines for `pkg.rs`, `wtool.rs`, `store_ratings.rs` name the new files (same bytes, no wire change).
* OWNER: the allow line for the tile crate (`iyakki-tile-store -> iyakki-store-model`) is added with the crate move; confirm it then. No core-api change.
