# Implementation remainder, alpha.7 (2026-11-19)

What is left between the matrix and Impl 100, row by row, and who can close it. Source: every non-deprecated row of FEATURES.md with Impl below 100 (137 rows, 2773 open points), each row's Notes, its commit history and a check of the code. Where an earlier Note was stale the code was followed. Written by an analysis lane; no code and no row was changed.

How to read the classes: **A** owner item (a key, pin, digest, address, account, or a decision of docs/development/ALPHA-7-DECISIONS.md), **B** hardware or a real device (**B\*** = needs a build or a QEMU run on the PC with KVM, no real device), **C** real browser or real sites, **D** C code in the web-engine helper (`dist/webview/wpe-helper`, needs WPE headers or a web image build), **E** live cluster, tenant or external service, **F** still writable as code here, **G** process. A row lists every class that applies; the class that blocks its LAST points comes first and is the one counted in the table.


## 1. Summary

| Class | Rows (first class) | Open points (first class) | Rows with an F part |
|---|---|---|---|
| A owner item | 27 | 544 | (41 rows touch it) |
| B hardware or real device (B* = build host/QEMU on the PC) | 37 | 601 | (67 rows touch it) |
| C real browser or real sites | 5 | 123 | (14 rows touch it) |
| D C code in the web-engine helper | 9 | 214 | (12 rows touch it) |
| E live cluster, tenant, external service | 10 | 107 | (21 rows touch it) |
| F still writable as code here | 47 | 1054 | 88 |
| G process | 2 | 130 | (2 rows touch it) |
| Total | 137 | 2773 | |

The F lanes of section 3 can close about 1202 of the 2773 points by code alone (a row whose last points are a device or a key is counted by its F part only). Rows that are only waiting for a person to look (B, C, E) are the testing phase of section 4. The biggest single blocks: F-BROX-15 (75 points, speed work), F-CORE-30 (70), F-INST-12 (60), F-BROW-33 (60, real sites), F-BROW-71 (60, helper), F-ARCH-06/07 (52 each), F-BROW-40 (50), F-ARCH-08 and F-BOOT-11 (100 each, owner decisions: both are deliberately left at 0).

Honesty note for the matrix owner (not applied here): the Notes of F-NET-26, F-NET-28, F-INST-30, F-INST-13, F-SETUP-06, F-SETUP-18, F-VPN-08, F-WEBSVC-14, F-FLOW-16, F-CLOUD-76, F-TALES-18 and F-BROW-100 say the code is written and only a real run or a launcher tile is left; if "Impl = code written" is the rule, these probably deserve Impl 100 (about 50 points) with Manual `needed` kept. Rows ARCH-08 and BOOT-11 sit at 0 only because of a decision whose conservative default is "not built" (D-TILE-CRATE, D-SB-MS-SHIM); with those defaults they can never reach 100, so the owner should either confirm a different option or decide how such rows count.

## 2. What I need from you (owner items)

Defaults are already coded; nothing below blocks the build. Each line: id, the ask, the default, what it unlocks.

| Id | Ask | Default already coded | Unlocks |
|---|---|---|---|
| D-GPU-02 (F-GPU-02) | Three ROCm image digests: `PLUGIN_IMAGE`, `SMOKE_IMAGE`, `OLLAMA_IMAGE` in `crates/iyakki-shared/src/gpu_rocm.rs` (all empty) | Step refuses unpinned, fails closed | +5 points, and the AMD run on a real card |
| D-LINAPP-15 (F-LINAPP-15) | One builder image digest for `BUILDER_IMAGE` (`iyakki-shared/src/imagebuildtools.rs`, empty) or say "administrator sets it" | (a) none shipped; `image_builder_set` by an administrator | +8 |
| D-LINAPP-19 (F-LINAPP-19) | The Debian archive signing key for `pinned_key()` (`iyakki-shared/src/linuxapps_debrelease.rs`) | None pinned: registry digest only, said in words | +8 |
| D-QA-1 (F-QA-05/06/11) | Who signs the downloads manifest (offline key) and where it is published; which of the 38 REVIEW rows get a pin; which image set the gate scans | Runs without a key, a fetched list is refused until a key exists, no digest invented | F-QA-06 +30, F-QA-11 +15 (with the F step) |
| D-MEDIA-YTDLP (F-MEDIA-12) | Add the `yt-dlp` row with the vendor SHA-256 and a matching code item, run the licence gate | Refuses while the row is missing | +15 (part) |
| D-DATASCI-PINS (F-FLOW-31) | Download each toolkit file, run sha256sum, compare with the vendor's published hash, write address + 64 hex into `crates/iyakki-shared/src/datasci.rs` | Click ends at "its download table is not complete" | +20 (part) |
| D-VPN-1 (F-VPN-06) | Where OpenVPN comes from: an address + SHA-256 for `vpn.openvpn_download`, or a musl build we publish | Setting empty, nothing fetched | +15 (part) |
| D-MODPACK-URL (F-PKG-11) | Download address of the optional kernel-module pack | None built in; the card refuses | +15 with the kernel rebuild |
| D-VOICE-43-CLIENT-ID (F-VOICE-43) | Your Alexa client id and product id (`alexa-avs.json`) | Fails closed with a sentence, nothing dialled | +25 (part, with the F lane) |
| D-RAG-17 / D-RAG-2 (F-RAG-17) | Allow a per-collection timer that refreshes kept pages while the person is signed in? | (a) never by itself; crawl/refresh ask every time | +20 |
| D-CLIENT-01 (F-CLIENT-01) | Display-list mode and page file transfer in the client page? | (a) picture mode only; files through `iyakki-connect serve --share` | +20 |
| D-CLIENT-03 (F-CLIENT-03) | Opus and a per-person mixer for client sound? | (a) PCM, one sound per machine (no in-house encoder) | +10 |
| D-NET-29 (F-NET-26/28) | AAC beside SBC for Bluetooth headphones (aptX is excluded by the permissive-licence rule; D-AAC-ADDON covers AAC through the person's ffmpeg) | (a) SBC only | up to +20 |
| D-SITEAPP-TILE (F-BROW-100) | A launcher tile per site app on the home screen? | (a) the Browser opens the site app as its own tab | +2 |
| D-TILE-CRATE (F-ARCH-08) | Separate tile crate for a 9-14 s gain on a tile change? | (a) stays planned at 0, reconsidered after F-ARCH-07 | +100 (a row at 0) |
| D-SB-MS-SHIM (F-BOOT-11) | Microsoft-signed shim: pursue or not | (a) not pursued; row stays planned at 0 | +100 (a row at 0), process in section 5 |
| D-WINMAC-INSTALLER (F-INST-12) | Installer on Windows and macOS hosts | (a) stays blocked | up to +60 |
| D-DRIVERS-15 (F-DRIVERS-15) | Keep or deprecate the `macweb` variant | (a) keep, on request only, crocus via WEB_CROCUS=1 | +15 with a first build |
| D-HOTSPOT-22 (F-HOTSPOT-18/22) | Hash-to-element for WPA3-SAE (the arithmetic is not constant time) | (a) plain SAE; H2E stations refused | +10 to +20 |
| D-SDK-34-01 / owner rule (F-SDK-34) | Migrate existing apps onto the SDK now, or keep "only new apps" | Only new apps; each old app moves when it is next rewritten | +20 |
| D-GEMVOICE-OPEN-STATE (F-GEMVOICE-40) | Should a voice session carry dashboard and task tools of the open tile | Project and flow tools only | +25 |
| D-4KN-BIOS (F-INST-29) | Pick the built-in 4Kn boot code as default (`--bios-4k-boot-code builtin` exists) | Planner refuses `--bios` on 4Kn without the flag | +15 after a SeaBIOS run |
| D-BRAILLE-PROTOCOL (F-A11Y-07) | Which braille display protocols to drive beyond USB HID | HID displays only | +15 (part) |
| D-PASSKEY-PAGE (F-BROX-03) | Who may use a passkey when a page asks | Asks the person with a card | +15 (part) |

Items with no decision id yet that an owner has to supply (a new id will be written when asked):

| Item | Ask | Rows |
|---|---|---|
| Relay service | Address (and TLS certificate) of your relay for the machine's outgoing connection; KI-725 | F-CORE-33 (+15), F-FLEET-10 (+10) |
| Official store | Root key (made offline, two copies), run `iyakki pkg root-pin`, host the folder, set `OFFICIAL_STORE_URL` (empty in `iyakki-store-model/src/store_catalog.rs`), review first Verified publishers | F-APPSTORE-14 (+15) |
| Payment provider | Choose from docs/STORE.md section 10 | F-APPSTORE-13 (+8) |
| Update channel | Official release key and channel address for update bundles | F-UPD-01 (+10) |
| Google AI key | One real key for a single call (KI-460) | F-ASSIST-52 (+30), F-GEMVOICE-08 |
| Test accounts | Mail and office web apps accounts (KI-791) | F-BROW-33 (+60) |
| Malware list | Source of a real malware list (owner source decision) | F-BROW-98 (+6, with the F step) |
| Voice model | FastPitch re-export with a durations output and its server | F-VOICE-45 (+15) |
| Asahi pins | Release choice and network pins in `dist/asahi/PINS` | F-APPLE-03 (+65 with an M1/M2 boot) |
| Bundle tool members | What runs a tool member (they are descriptions with no command) | F-CORE-26 (part) |
| Default public address for the sign-in check | Which address the network page probes | F-NET-07 (+5) |

## 3. Writable code still open (class F), six lanes

Ordered inside each lane by points per effort (S about a half day, M one to two days, L several days or a series). 'Pts' is what the code part can close, not the whole row. Each first step is meant to be given to a fresh agent as it stands. Run the usual piece checks of the lane brief.

### Lane 1: core split and kernel access (UI/OS split) (about 236 points)

| Row | Pts | Size | First concrete step |
|---|---|---|---|
| F-CORE-26 | 30 | M | Make the picker, template list, theme chooser and extension list of the UI crates call `store::bundle::members_text` / the member reader (KI-943); nested-tile start stays out (needs shell, KI-891). |
| F-CORE-23 | 30 | M | Add golden fixtures under tests/compat/ for the pattern-A files listed in KI-720 and a rustdoc-scanner test in `iyakki-core-api` compat tests (`compat_surfaces`). |
| F-ARCH-06 | 50 | L | Pick the audio/disks/net files of `iyakki-agent` that hold kernel reads (`kernel_single_reader` ratchet list), move one cluster at a time into `iyakki-core-local` as in the `touchrecover` move (git mv, `pub use`, BOUNDARY.md row). |
| F-ARCH-07 | 50 | L | Same recipe for the rest of the agent's kernel reads (docs/development/CORE-LOCAL-EXTRACTION-PLAN.md lists the order); tighten the ratchet file after each move. |
| F-SPLIT-06 | 5 | S | Add the test for a new program that fails before reading the handed-over key descriptor (iyakki-agent live-restart module; old program keeps its vaults). |
| F-SPLIT-03 | 31 | L | Next member store as a trait object per D-SPLIT-03-X1: secrets (then calendar and study, whose functions take the concrete `CalendarStore`); regenerate with the store-api generator; one commit per store. |
| F-SPLIT-07 | 30 | L | Plug a `BulkSource` into the real core (`Core::ui_socket` / `serve_unix_bulk` in `iyakki-ui-client::server`); then pushed events; then the pairing step. Needs a display-list producer first (see F-SDK-29). |
| F-ARCH-05 | 10 | M | Chat route for the install with the typed drive-name confirmation (reuse the F-INST-30 pattern in the installer facet), then the Drivers trait next to `iyakki-core-api` installer. |

### Lane 2: client family, voice, cameras, messaging (about 165 points)

| Row | Pts | Size | First concrete step |
|---|---|---|---|
| F-WEBDEV-01 | 25 | M | Let the editor and chat text fields insert pasted HTML (as text) and pictures from the clipboard history (`iyakki-remote::browser_media`, shell clipboard). |
| F-VOICE-43 | 20 | M | Wire `tls_dialer` with the trusted roots, act on downchannel directives, reuse the in-house `mp3_to_wav` for answers, then consent card, chat tool and Settings page (all fail closed with no client id). |
| F-CLIENT-05 | 5 | S | First consumer of `Leases::hold_local`: the Cameras tile takes an exclusive hold when it records (iyakki-tile-cameras / `cameras.rs:543` wrapper). |
| F-CLIENT-06 | 5 | S | First consumer of `VirtualSources::attach` (`SourceTap`): a shipped app that reads the newest client picture or last second of sound; `clientmedia.rs`. Also covers F-CLIENT-07. |
| F-SDK-167 | 5 | S | `https://` ONVIF control: use `camtls::connect_for` (already used by the RTSP client) in `onvif.rs`; absolute moves after. |
| F-GEMVOICE-08 | 5 | S | Settings page controls for the Gemini voice language and the level meter (iyakki-apps settings pages; chat tool exists). |
| F-VOICE-38 | 5 | S | The instance call that hands the connector the audio address and certificate (in `iyakki-audiolink` / voice service); unlocks VOICE-61. |
| F-VOICE-61 | 15 | M | `iyakki-connect voice` command asking the instance for the offer, plus the listener thread of the cloud machine inside the voice service; mic capture itself is device work. |
| F-CORE-30 | 40 | L | Settings -> Devices row calling `client_device_role`; then mount `uidisplay.js` on the `/connect` page and add the shared picture loop in `iyakki-remote`; window/phone forms last. |
| F-MSG-08 | 30 | L | Device verification by SAS first (`msg::e2ee_olm`), then key backup/export, key requests, `m.room.encrypted` in the sync mirror, add-on loader. |
| F-CAM-20 | 10 | M | G.711 sound in the Cameras tile, then the card for a waiting certificate; H.265 decode is L and optional. |

### Lane 3: SDK, shell, display, assistant surfaces (about 240 points)

| Row | Pts | Size | First concrete step |
|---|---|---|---|
| F-SHARE-07 | 15 | S | One-step 'publish this share under a name' shortcut: creates the domain route (F-CLOUD-77 code) for a share. |
| F-SDK-217 | 10 | S | A card/chat tool offering a time limit that writes `Approval::ends` (D-GRANT-END; `iyakki-core-api::appcaps`). |
| F-VIEW-02 | 30 | M | Single-instance app shown in two views; key rebinding beyond the set choice (`settingstools_views.rs`). |
| F-SDK-207 | 25 | M | Feed `available` updates, failed installs and downloads into the app event sources (`appevent_feed.rs`, `appevent_more.rs`, `store/install.rs`). |
| F-SDK-29 | 25 | M | Mount a display list on the `/connect` page itself (`uidisplay.js` `mountDisplayList`) and the machine's own web UI pages. |
| F-DISP-15 | 20 | M | Un-ignore the 18 entries of the full-matrix overflow test (`iyakki-shell/src/monkey_tests.rs`, `KNOWN_OVERFLOWS`) and fix each overflow. |
| F-SDK-30 | 20 | M | Draw a second screen's dock (shell_panes.rs, `render_one_pane`) and Look parts beyond colours. |
| F-K8S-26 | 20 | M | Apply button on the K8S tile, then port-forward; exec needs a pty (L). |
| F-SDK-34 | 5 | S | Send the pinch gesture from the shell to a tile of ABI 3 (D-SDK-34-01 a already bumps the version). |
| F-BACKUP-05 | 5 | S | Settings -> Backup: restore a machine-data backup from the page (calls the same function as `backup_system_restore`; passphrase typed twice). |
| F-BACKUP-06 | 5 | S | Settings -> Backup: restore a chain from the page (`backup_chain_restore` function). |
| F-LAUNCH-25 | 15 | M | Network panel of its own and the top-bar buttons while an app is open (`iyakki-shell`, D-LAUNCH-25-MENU). |
| F-DEV-17 | 15 | M | Make the pairing script an importable function; add scenarios that use the scrollbar detector and the wheel (`dist/devtest/`). |
| F-INBOX-09 | 10 | M | Service worker + `PushManager.subscribe` page side and the open-vault registry for the sender (public name is the owner's). |
| F-SDK-14 | 6 | M | Element boxes by id: make the page layout keep element ids so the bridge can answer. |
| F-A11Y-06 | 4 | M | Headings, images and landmarks as access-tree nodes (`iyakki-web/src/doc.rs` `tree_a11y`). |
| F-A11Y-07 | 10 | L | Grade 2 contracted braille in `iyakki-uikit::braille`, then serial/vendor protocols (D-BRAILLE-PROTOCOL). |

### Lane 4: browser (Rust side), web tiles, PDF, rendering (about 302 points)

| Row | Pts | Size | First concrete step |
|---|---|---|---|
| F-WEB-56 | 30 | M | Three more settings pages as tile pages beyond the switches (iyakki-tile-settings). |
| F-BROW-40 | 30 | M | Chat tool + Store page button for the first-use install and removal of the browser package (`browserpkg`). |
| F-BROX-22 | 8 | S | Settings -> Browser proxy fields feeding `BrowserFetch::with_h2_proxy` (and the plain HTTP/1.1 path). |
| F-BROX-15 | 60 | L | Profile style and layout with the table in the Notes (4000 blocks 47 ms on the amd mini), then fix the hottest path; measure on the PC (no change yet). |
| F-WEB-53 | 15 | M | Attach a picture and choose single tools by name in the Web Chat tile (rebuild tile.wasm). |
| F-PDF-17 | 40 | L | Cyrillic/Greek need a retrained recogniser; JBIG2 first (decoder in `iyakki-pdf`). |
| F-WEB-59 | 4 | S | Send the selection as context to the Web Edit assistant (tile code `iyakki-tile-edit`, rebuild tile.wasm). |
| F-BROX-03 | 10 | M | Shell `Presence` card and `Sites.passkeys` setting in the browser app (D-PASSKEY-PAGE). |
| F-PDF-18 | 25 | L | CID-keyed CFF and OpenType-wrapped CFF in the PDF font reader (KI-960). |
| F-PDF-16 | 25 | L | Shaping rules and fonts for Sinhala, Khmer, Lao, Myanmar, Tibetan (SIL OFL fonts only). |
| F-WEB-58 | 3 | S | A second web tile that uses the clipboard service domain (Web Edit paste/copy). |
| F-BJS-01 | 3 | S | Pointer events other than click in `browser_live`. |
| F-BROX-05 | 7 | M | SVG `image` and `feImage`, `feTile`, then lighting filters, compared with headless Chrome in `svg_chrome_tests`. |
| F-BROW-100 | 2 | S | Launcher tile for a site app only if the owner picks D-SITEAPP-TILE (b); otherwise none. |
| F-BROW-99 | 15 | L | Faster script start beyond the compiled cache: a measured plan for QuickJS in wasmtime. |
| F-BROW-98 | 5 | M | Hand the tab's own certificate out of the HTTPS client (`browser_certview.rs`). |
| F-BROX-08 | 12 | L | PDF signature fields and push buttons (KI-715), then more fonts in new appearances. |
| F-WEBSVC-12 | 8 | L | HTML web-bundle tile runtime and WebSocket bridge (KI-821). |

### Lane 5: installer, boot, packaging, store, updates (about 142 points)

| Row | Pts | Size | First concrete step |
|---|---|---|---|
| F-INST-29 | 5 | S | Chat tool for `--bios` with `--bios-4k-boot-code builtin` (the installer chat tools only shrink today; D-4KN-BIOS update 2). |
| F-UPD-01 | 5 | S | Settings button for the update bundle (chat tool exists); key and address are the owner's. |
| F-PKG-08 | 15 | M | Join the Win32 steps to the window program, in-house file picker, administrator prompt. |
| F-INST-12 | 30 | L | macOS writer on the Mac build hosts first (positional writes, APFS container handling beyond reading); Windows after PKG-07. |
| F-PKG-07 | 10 | M | Windows build of the writer program with the command switch (type-check with x86_64-pc-windows-gnu as before). |
| F-INST-27 | 10 | M | FAT32/exFAT shrink in-house (btrfs needs mounting; xfs impossible). |
| F-STORE-05 | 24 | L | Export step copying the encrypted payload behind `luks2_export::export_header` (`iyakki-volcrypt`, `iyakki-enchome`), chat tool with approval; whole-partition encryption later. |
| F-QA-11 | 8 | M | Move the download and checksum addresses into the fetched list (backlog step 2b). |
| F-BOOT-02 | 20 | L | Signed unified kernel image in `dist/buildroot/secureboot.sh`/`post-image.sh`, GRUB 2.12 fixes. |
| F-QA-05 | 5 | M | ext4 listing and extended/hybrid partition tables in the image content scan (`iyakki-relcheck`), `.zst` through our own decoder. |
| F-BOOT-25 | 10 | L | Install-time side image from docs/MAC-HARDWARE.md (big parts read by Linux's own USB driver). |

### Lane 6: network, hotspot, VPN, firewall, flows, media (about 117 points)

| Row | Pts | Size | First concrete step |
|---|---|---|---|
| F-MEDIA-12 | 10 | S | Store -> Setup card with licence text and registering the finished file in the media index (the yt-dlp row is the owner's). |
| F-CORE-33 | 10 | S | TLS to the relay in `devmode::relay_link` (HttpLink over TLS with pinned certificate); the relay address stays the owner's. |
| F-NET-07 | 5 | S | Button opening the sign-in address in the Browser, adapter picker for two adapters (Settings -> Network). |
| F-SYS-10 | 5 | S | Add the conntrack options to the kernel fragment; the rest is a real run. |
| F-FLOW-31 | 15 | M | Self-test in the no-network sandbox with the `python_run` runner, Verify/Upgrade buttons. |
| F-HOTSPOT-15 | 15 | M | Sharing manager treats `iyaknap0` as a downstream with an address and forwarding; sharing page kinds; pairing from the page; BNEP option in the fragment. |
| F-NET-14 | 10 | M | Firewall hook with the zone and DHCP on a direct link (`iyakki_agent::directlink`). |
| F-MEDIA-11 | 10 | M | Shares page buttons, seeking and cast progress (`cast_*` tools exist). |
| F-VPN-06 | 10 | M | HYBRID domain rules for OpenVPN. |
| F-HOTSPOT-22 | 15 | L | nl80211 external-auth frame path in `hotspot_radio.rs` calling `SaeAp`, against a fake netlink first; hash-to-element is D-HOTSPOT-22. |
| F-FW-10 | 4 | M | Concatenation sets (address . port pairs) in `iyakki-fw::outbound`. |
| F-HOTSPOT-18 | 8 | L | SAE on the computer (shares code with HOTSPOT-22) and netlink link set-up. |

## 4. Needs a device, a browser or a cluster (testing phase)

Grouped so that one session closes many rows. Everything is code-complete (or will be after section 3) unless a row says otherwise.

**Mac mini (2014 Haswell and 2018, USB-C, touch via screen)**: F-DRIVERS-15 (first macweb build and boot), F-BROX-21 (VA-API on Intel), F-DEV-17 (run the device test runner), F-BOOT-25 (time the GRUB wait), F-SETUP-06 (touch list scrolling), F-SHELL-65 (restart with developer mode), F-HOTSPOT-18/22 (re-run on brcmfmac after the interface fix), F-VIEW-02 (locked view, step keys), F-NET-14 (direct link with a second machine), F-LAUNCH-25, F-SDK-30/34 (second screen, pinch).

**MacBook Pro (14,1)**: F-NET-26/28 and F-LINAPP-18 (headphones, sound sandbox), F-VOICE-61 (microphone capture), F-GEMVOICE-08 (real mic), F-A11Y-07 only with a braille display.

**Lenovo (and HP)**: F-CHECKUP-04 (GPU and USB step wording, KI-850), F-GPU-02 only if the Lenovo has an AMD card, F-K8S-13 (bare-metal cluster after CONFIG_DUMMY), F-SYS-10 (network bytes per host process with real sockets), F-INST-13 and F-INST-30 and F-INST-27 (an internal disk or a legacy-BIOS PC: the HP is the likely one), F-BROX-21 on whichever has a VA driver.

**Apple Silicon Macs (the two build Macs)**: F-APPLE-02 (Lima VM with the aarch64 image), F-APPLE-03 (an M1/M2 only), F-BROX-08 (compare filled PDF forms in PDFKit), writer on macOS for F-INST-12.

**Windows PC with a spare stick**: F-PKG-07, F-PKG-08, F-INST-12.

**Phone, TV, cards, headphones, display, camera, touch screen**: F-HOTSPOT-15 (a phone for Bluetooth tethering), F-HOTSPOT-22 (a station and card for WPA3), F-MEDIA-11 (a DLNA TV), F-SDK-167 and F-CAM-20 and F-CLIENT-05/06/07 (a network camera), F-A11Y-07 (braille display), F-SETUP-06 and F-SDK-34 (touch screen), F-BROX-03 (security key), F-LINAPP-18 (sound card).

**Real browser (Chrome and the full engine on a machine)**: F-CLIENT-01/03/05/06/07, F-SDK-29, F-WEB-53/56/58/59, F-WEBSVC-12, F-WEBDEV-01, F-BROW-33 (real sites with accounts), F-BROW-99 and F-BROX-22 (real pages and a bench), F-TALES-18 (a real site fetch), F-BROW-100.

**Web image build with WPE headers (class D, one lane of C)**: F-BROX-16, F-BROX-17, F-BROX-18, F-BROX-20, F-BROW-71, F-BROW-90, F-A11Y-06 (full-engine pages), F-VAULT-09, F-BROX-03 (navigator.credentials), F-BROW-40 (helper out of the image), F-BROX-15 (tuning). One helper.c/websink.c lane closes about 210 points (the rows whose first blocker is D): write the five C pieces together (cache-size limit, media cap, codecs refusal, user scripts and styles, screen-share feeder), compile them in one web image build, then a browser session closes the Test side. Verified: `webkit_website_data_manager_clear` is already in `helper.c`.

**PC with KVM (QEMU, B\*)**: F-CORE-21 (needs a migration pair), F-PKG-05 (idle-RAM ceiling), F-INST-28 and F-INST-29 (aarch64 VM, SeaBIOS), F-STORE-05 (dm-crypt in QEMU, cryptsetup luksOpen), F-QA-05 (full image gate), F-PKG-11 (kernel rebuild with the compress fragment), F-REL-09 (release image boot), F-SPLIT-06 and F-SPLIT-07 (booted core restart), F-VPN-05/08/16 (session as non-root, real WireGuard, real kernel), F-FW-10 (`tablegen_kernel` as root), F-WEBSVC-15 (headless image size, one build at a time), F-UPG-07 (QEMU boot of the beta fixture), F-BSHARE-05, F-SHELL-65. One kernel-fragment change (CONFIG_DUMMY, BNEP, conntrack, compress) followed by one rebuild (about 9 minutes, targeted make) covers F-K8S-13, F-HOTSPOT-15, F-SYS-10 and F-PKG-11.

**Live cluster, tenant or external service (E)**: F-VAULT-11, F-CLOUD-15 (a transit KMS), F-CLOUD-76 and F-CLOUD-77 (a tenant pod, ssh/WireGuard), F-TCFG-11 (a real provider), F-CLUSTER-01 (a remote cluster), F-SBX-03 (a pod with sh/base64/tr), F-FLOW-15 (a git host or CI sender), F-LINAPP-15 (a real image build, KI-851), F-K8S-26, F-SETUP-18 (Lemonade), F-VOICE-38, F-MSG-08 (a Matrix homeserver), F-ASSIST-52 (Google), F-VOICE-43 (Amazon), F-INBOX-09 (a push service with a public HTTPS name), F-CORE-33 and F-FLEET-10 (the owner's relay), F-BROW-40 (publish the package to our k3s store).

## 5. Process items

- **F-BOOT-11, Secure Boot route 2**: a Microsoft-signed shim needs a Microsoft developer account with an extended-validation certificate and a public shim-review. Owner decision 2026-11-06 and D-SB-MS-SHIM: not pursued for 1.0, route 1 only (own key, distro-signed shim, one-time enrolment). Nothing to code; the row cannot reach Impl 100 unless the owner changes the decision.
- **F-UPG-07**: the beta fixture exists only after a beta image is cut; the shared upgrade check already runs over every earlier release folder. Process: capture the fixture on the beta cut, then the QEMU boot.
- **F-BOOT-02**: signing packs' modules is a publisher step that needs the Secure Boot key (kept outside the repo, two local copies; the same key must be used or the MOK re-enrolled).
- **F-APPSTORE-14, F-UPD-01, F-QA-11**: key ceremonies (offline machine, two copies, `root-pin`, hosting). They are owner items in section 2 but also a release process: do them once, together, before the beta image.
- **Legal questions in general**: any unknown legal question is routed around, not waited for (owner rule 2026-11-06).
- **GitHub export** of alpha.4 to alpha.7 and the switch to GitHub as the primary repository are process steps outside the matrix.

## 6. Row by row

| Row | Pts | Classes (last blocker first) | Blocker |
|---|---|---|---|
| F-ARCH-08 | 100 | A | D-TILE-CRATE: a separate tile crate for 9-14 s per tile change; default keeps it planned at 0 |
| F-BOOT-11 | 100 | G,A | Microsoft shim-review and developer account (D-SB-MS-SHIM: not pursued for 1.0) |
| F-BROX-15 | 75 | F,D | style and layout speed work (no change yet) and tuning of the full engine |
| F-CORE-30 | 70 | F,E | engine/page behind /connect, Settings row for the device role, shared picture loop, window and phone forms; cloud tenants |
| F-APPLE-03 | 65 | A,B,E | release pins (network + owner's choice); M1/M2 boot; a build of the apple variant |
| F-BROW-33 | 60 | C,A | run the release-sites script against the real sites; test accounts for the mail/office apps (KI-791) |
| F-BROW-71 | 60 | D,F | helper feeder, in-house portal service and PipeWire in the image (screen share) |
| F-INST-12 | 60 | B,F,A | Windows host (no Windows target here), macOS writer on a Mac; D-WINMAC-INSTALLER default keeps it blocked |
| F-ARCH-06 | 52 | F | move the kernel reads of Audio/Disks/Net slices out of the agent (about 120 files hold kernel reads) |
| F-ARCH-07 | 52 | F | extract iyakki-core-local: the rest of the agent's kernel reads (plan CORE-LOCAL-EXTRACTION-PLAN.md) |
| F-BROW-40 | 50 | F,D,E | chat tool + Store button for first-use install/removal; helper out of the image (Buildroot); build and publish the package; first-use run |
| F-CORE-26 | 40 | F,A | pickers/template list/extension list call the reader (KI-943); what runs a tool member is undecided (new decision D-BUNDLE-TOOL-RUN), nested tile needs the shell (KI-891) |
| F-PDF-17 | 40 | F | ogonek, Greek/Cyrillic (retrain the model), JBIG2, JPEG 2000, Group 3 mixed mode |
| F-BROW-99 | 35 | C,F | real-page bench with network; script run time itself (QuickJS in wasmtime) |
| F-VIEW-02 | 35 | F,B | single-instance app in two views; changing the keys beyond the set choice; locked view, step keys, other displays by hand |
| F-SPLIT-07 | 34 | F,B* | plug a BulkSource into the real core; pushed events; pairing step; booted-device run |
| F-SPLIT-03 | 31 | F | member stores one at a time (calendar, study, secrets, browser, screen, camera, devtools, ext status), chat.rs RunRequest, RemoteServicesHandle, iyakki-ui --socket |
| F-APPLE-02 | 30 | B,F | Apple Silicon Mac with Lima + aarch64 image; manager half; GPU option |
| F-ASSIST-52 | 30 | A,E | a Google AI key and one real call (KI-460); whether Google's endpoint accepts every tool schema is unknown |
| F-BOOT-25 | 30 | B,F | rebuilt image timed on a Mac; install-time side image (plan in MAC-HARDWARE.md) |
| F-BROW-90 | 30 | D | helper.c user scripts/styles/user-agent hooks and a web-process part for headers |
| F-BROX-18 | 30 | D | helper.c / websink.c do not read the media-policy message |
| F-BROX-20 | 30 | D,F | helper reading the codecs message; tenants page line; engine refusing decoders |
| F-BROX-21 | 30 | B,F | a machine with a VA driver (Mac mini 2014 or HP/Lenovo); Settings switch beside web-tuning |
| F-CORE-21 | 30 | B* | a build pair with a schema migration, booted under KVM on the PC (KI-750) |
| F-CORE-23 | 30 | F | open enums, pattern-A golden files, rustdoc scanner (KI-720) |
| F-INST-28 | 30 | B*,B | genimage never run; no aarch64 VM or board booted an installed disk |
| F-MSG-08 | 30 | F,E | device verification (SAS/QR/cross-signing), key backup, key requests, add-on loader, m.room.encrypted in the sync mirror; a real homeserver |
| F-QA-06 | 30 | A | 38 REVIEW rows need pins/decisions (D-QA-1: which unpinned items get a pin) |
| F-SDK-29 | 30 | F,C | /connect page mounting a display list; machine's own web UI pages; judge in a browser |
| F-UPG-07 | 30 | G,B* | the beta fixture exists only after a beta cut; QEMU boot of it |
| F-WEB-56 | 30 | F,C | more settings pages as tile pages (only switches exist) |
| F-WEBDEV-01 | 30 | F,B | an app inserting pasted HTML/image into a text field; real browser/screen |
| F-BOOT-02 | 25 | F,A | signed unified kernel image, GRUB 2.12 fixes, signing packs' modules (publisher step, owner's key) |
| F-GEMVOICE-40 | 25 | A,F | dashboard and task tools (D-GEMVOICE-OPEN-STATE: a voice session has no tile to name) |
| F-PDF-16 | 25 | F | Sinhala/Khmer/Lao/Myanmar/Tibetan shaping, designed bold, bigger Thai dictionary |
| F-PDF-18 | 25 | F | CID-keyed CFF, expert glyph names, OpenType CFF, Type 1 private order, multi-line cells, ruled tables (KI-960) |
| F-PKG-07 | 25 | B,F | a Windows machine with a spare stick; Windows build of the program with the command switch |
| F-REL-09 | 25 | B*,B | build and boot a release image; GPU guard on hardware |
| F-SDK-207 | 25 | F | feed `available` updates, failed install, downloads into app events |
| F-SDK-30 | 25 | F,B | second screen dock; Look parts beyond colours |
| F-SPLIT-06 | 25 | B*,F | real exec with the descriptor on a booted QEMU guest; the new program failing before reading it |
| F-VOICE-43 | 25 | A,F | owner's Alexa client id and product id (D-VOICE-43-CLIENT-ID); TLS dialer, directives, MP3 decode, consent card, chat tool, Settings page |
| F-STORE-05 | 24 | F,B* | export step copying the payload behind the LUKS2 header + chat tool; whole-partition encryption; QEMU/hardware proof; cryptsetup luksOpen check |
| F-BROX-16 | 20 | D,C | real WebKit run of the Elements/console scripts (helper.c) |
| F-BROX-17 | 20 | D,F | helper.c cache clear written, not compiled (no WPE headers); cache size setting to WebKit's limit |
| F-CHECKUP-04 | 20 | B,F | Lenovo with GPU and USB devices (KI-850); a USB cell in the hardware matrix |
| F-CLIENT-01 | 20 | A,C | display-list mode and page file transfer are not written by default (D-CLIENT-01 a); then a real browser |
| F-DEV-17 | 20 | F,B | scenarios using the scrollbar detector and wheel; pairing script as importable function; runs on the Mac minis |
| F-DISP-15 | 20 | F | the 18 entries of the ignored full-matrix overflow list |
| F-FLOW-31 | 20 | A,F | SHA-256 + address per toolkit download (D-DATASCI-PINS); no-network sandbox self-test and python_run runner; Verify/Upgrade buttons |
| F-GEMVOICE-08 | 20 | A,B,F | Gemini key and a real microphone; Settings controls for language and meter |
| F-HOTSPOT-15 | 20 | B,F | a phone; sharing manager treating iyaknap0 as a downstream, sharing page kinds, pairing from the page, BNEP kernel option |
| F-HOTSPOT-22 | 20 | B,F,A | a real card and station; nl80211 external-auth path in hotspot_radio.rs; hash-to-element (D-HOTSPOT-22) |
| F-K8S-26 | 20 | F,E | exec in a terminal (pty), apply button, port-forward; a real cluster |
| F-PKG-08 | 20 | F,B | join the Win32 steps, in-house file picker, administrator prompt; a screen with a real stick |
| F-RAG-17 | 20 | A | timer refresh while nobody is present (D-RAG-17 / D-RAG-2: default a, no timer) |
| F-SDK-34 | 20 | A,F,B | existing apps moving over (owner rule: only new apps use the SDK); pinch sent to a tile of ABI 3; touch run |
| F-TCFG-11 | 20 | E,F | a real provider; team keys used beyond the run start |
| F-VOICE-61 | 20 | B,F | microphone capture on the person's computer; a `voice` connector command + instance call + listener thread |
| F-WEBSVC-15 | 20 | B* | build the headless image and measure its size (one build at a time on the PC) |
| F-A11Y-07 | 15 | B,F,A | a real braille display; contracted braille and serial/vendor protocols (D-BRAILLE-PROTOCOL) |
| F-APPSTORE-14 | 15 | A | official root key + two copies, hosting, OFFICIAL_STORE_URL (empty), first Verified publishers |
| F-BROX-03 | 15 | D,F,B | full engine's own navigator.credentials; shell Presence card + Sites.passkeys; a security key |
| F-BSHARE-05 | 15 | E,B* | k3s store with TLS on a real device; no aarch64 image exists |
| F-CORE-33 | 15 | A,B,F | owner's relay address and service (D-FLEET-10, KI-725); then a run on two computers |
| F-DRIVERS-15 | 15 | B*,B,A | first macweb build and boot on the Mac mini 2014; D-DRIVERS-15 keep/deprecate; crocus pieces |
| F-FLOW-15 | 15 | E,B | a real third-party sender (git host, CI), cloud tenant pods, a Mac mini run |
| F-INST-27 | 15 | F,B | FAT32/exFAT/btrfs shrink; a run on a real disk (xfs cannot shrink) |
| F-INST-29 | 15 | F,B* | chat tool for --bios; boot the 4Kn code under SeaBIOS (QEMU on the PC) |
| F-LAUNCH-25 | 15 | F | a network panel of its own; the bar's buttons while an app is open |
| F-MEDIA-11 | 15 | B,F,A | a real TV and ffmpeg run; Shares page buttons, seeking, progress; H.264 target if the owner wants it |
| F-MEDIA-12 | 15 | A,F | yt-dlp row with vendor SHA-256 + code item (D-MEDIA-YTDLP); Store -> Setup card; register file in media index |
| F-NET-14 | 15 | B,F | two real machines on one cable; firewall hook with the zone and DHCP on the link |
| F-PKG-05 | 15 | B* | one measure-budgets run with KVM gives the idle-RAM ceiling |
| F-PKG-11 | 15 | B*,A | kernel rebuild with the compress fragment, image carrying the split, measured saving; download address (D-MODPACK-URL) |
| F-QA-11 | 15 | A,F | manifest signing key + publishing address (D-QA-1); fixed digests; download addresses from the fetched list (backlog step 2b) |
| F-SBX-03 | 15 | E | a cluster and a pod with sh, base64, tr in the image |
| F-SHARE-07 | 15 | F | one-step 'publish this share under a name' shortcut (D-SHARE-07 a gives HTTPS/.local through a domain route) |
| F-SYS-10 | 15 | B,F | a real Linux machine with real sockets; conntrack in the kernel fragment |
| F-VOICE-45 | 15 | A,E | FastPitch re-export with a durations output and its server (outside this repository) |
| F-VPN-05 | 15 | B*,B | an image starting the VPN session as a non-root user; aarch64 kernel verification; real hardware |
| F-VPN-06 | 15 | A,F,B* | OpenVPN download address and checksum (D-VPN-1); HYBRID domain rules for OpenVPN; a real program run |
| F-WEB-53 | 15 | F,C | attach a picture, choose single tools by name, voice (in-house recogniser, KI-766); real browser |
| F-BROX-08 | 12 | F,B | PDF signatures, push buttons, fonts beyond Helvetica, JavaScript, protected files (KI-715); check in PDFKit/poppler on a Mac |
| F-CAM-20 | 12 | F,B | sound in the Cameras tile, H.265 decode, card for the waiting certificate; real camera |
| F-INBOX-09 | 12 | A,F,E | public HTTPS name; service worker + PushManager.subscribe page side; open-vault registry for the sender |
| F-TALES-18 | 12 | C | no real site fetched; Impl 88 looks understated against the Notes |
| F-VPN-16 | 12 | B* | the session as another user on an image; a refused peer on the real kernel |
| F-ARCH-05 | 10 | F | chat route for the install (typed confirmation); the Drivers trait |
| F-BROX-22 | 10 | F,C | Settings -> Browser proxy fields feeding with_h2_proxy; real-site measurement |
| F-CLIENT-03 | 10 | A,C | Opus and per-person mixer not written (D-CLIENT-03 a: PCM, one sound per machine) |
| F-CLOUD-76 | 10 | E,B | a real tenant pod on a screen |
| F-CLUSTER-01 | 10 | E | a real remote cluster; Setup installers stay local by rule |
| F-FLEET-10 | 10 | A,E | owner's relay address/protocol (KI-725) |
| F-HOTSPOT-18 | 10 | B,F,A | re-run on the Mac mini after the interface fix; SAE on the computer; netlink link set-up (D-HOTSPOT-22 hash-to-element) |
| F-K8S-13 | 10 | B,F | CONFIG_DUMMY in the kernel fragment, a kernel rebuild, a run on a bare-metal machine |
| F-NET-26 | 10 | B,A | real headphones/speaker on a radio; AAC/aptX stay refused (D-NET-29 a); Impl probably understated |
| F-NET-28 | 10 | B,A | real headphones; Impl probably understated (all code written per Notes) |
| F-SDK-14 | 10 | F,D,B | box of an element by id; real engines inside an embed; first-use card screen |
| F-SDK-217 | 10 | F | a card or chat tool that writes a grant's `ends` time (D-GRANT-END) |
| F-UPD-01 | 10 | A,F | official release key and channel address; Settings button |
| F-APPSTORE-13 | 8 | A | a payment provider (docs/STORE.md section 10); then a real return page |
| F-CLOUD-77 | 8 | B,E | a real ssh tunnel and WireGuard interface |
| F-LINAPP-15 | 8 | A,E | pin a builder image digest (D-LINAPP-15); a real build on a cluster (KI-851) |
| F-LINAPP-18 | 8 | B | a real sound card and sandbox run; sandbox alsa-lib file plugin unproven |
| F-LINAPP-19 | 8 | A | pin the Debian archive key (D-LINAPP-19) |
| F-VAULT-11 | 8 | E | a real tenant pod and a second real machine (KI-772) |
| F-WEB-58 | 8 | C,F | a run in a real browser; a second tile using the clipboard |
| F-WEB-59 | 8 | C,F | a real browser; the selection as context |
| F-WEBSVC-12 | 8 | F,C | HTML web-bundle tile runtime and WebSocket bridge (KI-821); a real browser |
| F-BROX-05 | 7 | F | image, feImage, feTile, lighting filters, primitive subregions, markers, nested svg |
| F-BROW-98 | 6 | A,F | real malware list source (owner); the certificate the tab itself used |
| F-BACKUP-05 | 5 | F | restore a machine-data backup from the Settings page (chat tool exists) |
| F-BACKUP-06 | 5 | F | restore a chain from the Settings page (chat tool exists) |
| F-CLIENT-05 | 5 | F,B | no shipped app calls Leases::hold_local yet; real browser/device run |
| F-CLIENT-06 | 5 | F,B | no shipped app attaches a SourceTap yet; real browser/device run |
| F-CLIENT-07 | 5 | F,B | consumer of the virtual sources (same piece as CLIENT-06); real screen/sound run |
| F-FLOW-16 | 5 | B | a run between two real machines (Impl 95 is on-purpose; probably 100) |
| F-GPU-02 | 5 | A,B | three ROCm image digests (D-GPU-02); a run on an AMD card |
| F-INST-30 | 5 | B | a run on a real internal disk (Impl probably understated) |
| F-NET-07 | 5 | F,A | button that opens the sign-in address, adapter picker for two adapters; default public address to check (owner choice) |
| F-QA-05 | 5 | B*,F | full image gate over a complete current image set; ext4/hybrid tables/.zst through our decoder |
| F-SDK-167 | 5 | F,B | https control addresses for cameras (TLS exists in camtls); absolute moves; real camera |
| F-SETUP-06 | 5 | B | a real touch screen/pad (Impl probably understated) |
| F-SETUP-18 | 5 | E,B | a real cluster and a real Lemonade (Impl probably understated) |
| F-VAULT-09 | 5 | D,C | real WPE profile folder names for local storage / IndexedDB |
| F-VOICE-38 | 5 | E,F | the instance's call handing the connector the audio address and certificate; a real run |
| F-A11Y-06 | 4 | D,F | full web engine pages; headings/images/landmarks as tree nodes; drawn store review screen |
| F-CLOUD-15 | 4 | E | a real vault/transit KMS |
| F-FW-10 | 4 | F,B* | concatenation sets (address . port); real-kernel run of ignored tablegen_kernel (root on Linux) |
| F-BJS-01 | 3 | F,B | pointer events other than click; restart seen against a real service |
| F-SHELL-65 | 3 | B*,F | real shell restart (F-SPLIT-06); window layouts and workspace panes |
| F-BROW-100 | 2 | A,F | launcher tile of its own on the home screen (D-SITEAPP-TILE b) |
| F-VPN-08 | 2 | B* | a real WireGuard run (Impl 98) |
| F-WEBSVC-14 | 2 | B* | a reconnect against a really restarted service (Impl 98, probably 100) |
| F-INST-13 | 1 | B | a real legacy-BIOS PC and GPT disks |
