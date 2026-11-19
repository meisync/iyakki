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
* Default implemented: (a). The tile decoder and the picture are the only mode; the client family stays experimental. Display-list mode and a page file transfer are NOT written and stay listed as missing in F-CLIENT-01's Notes (Impl stays under 100). Phone-sized pictures are handled by the page's "Fill height" mode instead of a smaller machine screen. Rows: F-CLIENT-01.

## D-CLIENT-02: which systems the connector targets

* Question: the window program uses Unix file modes, termios and signals; should it also build for Windows?
* Options: (a) Linux and macOS only for 1.0, Windows later; (b) port now.
* Default implemented: (a). Packaging is `dist/connect/package.sh` (tar.gz, SHA256SUMS, detached `ssh-keygen -Y` signature when `IYAKKI_CONNECT_SIGN_KEY` names a key kept outside the repository). The operating system's key store is opt-in (`IYAKKI_CONNECT_KEYSTORE=os`: Secret Service on Linux with the secret on standard input, Keychain on macOS where the tool takes the secret as an argument for an instant, which is why it is not the default); the default stays the mode-600 file. Rows: F-CLIENT-02.

## D-CLIENT-03: one sound per machine, PCM only

* Question: should the client's sound be per person (needs the own mixer, docs plan T2) and compressed (Opus)?
* Options: (a) one sound per machine that belongs to the person signed in last, 16-bit PCM about 190 KB/s; (b) per-person mixer and Opus.
* Default implemented: (a). Lip-sync is written (the page holds each picture back by the sound's queue plus the browser's output delay, at most 150 ms). The per-person mixer waits for the own-mixer work; Opus is not written (the repository carries no encoder, and an in-house one is out of scope). Rows: F-CLIENT-03.

## D-CLIENT-04: the client clipboard carries text only

* Question: should images and rich text cross the client clipboard as the older browser-as-hardware path does?
* Options: (a) text only, at most 64 KB, one press per use; (b) add PNG and HTML.
* Default implemented: (a): a smaller surface for a feature that moves the person's data across a trust boundary. The older path keeps its images and HTML. Rows: F-CLIENT-04.

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
