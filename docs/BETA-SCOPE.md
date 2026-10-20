# Beta scope: 1.0.0-beta.1

Derived from `FEATURES.md` on 2026-10-19 (done 286 · partial 205 · planned 53 · blocked 3 · deprecated 1 · total 548). Every one of the 262 rows that is not `done` is classified below, once. The format follows [`ALPHA-SCOPE.md`](ALPHA-SCOPE.md). The plan that uses this list is [`BETA-PLAN.md`](BETA-PLAN.md).

The classification was made from each row's Feature and Notes text. Where a Notes text was long, the work sentence is a short reading of it and the row itself is the authority; whoever picks up a row reads its Notes first. Rows on the lab list as 'live model' run against a local model (qwen3:8b class) unless the row says otherwise.

**Beta definition (owner):** no blockers and all rows working. Section "Honest reading" says what that can and cannot mean.

## Summary

| Class | Meaning | Rows | partial | planned | blocked | deprecated |
|---|---|---|---|---|---|---|
| A | Close for beta in code (tests, no hardware) | 55 | 45 | 10 | 0 | 0 |
| B | Needs real-hardware or real-service verification (code exists) | 145 | 143 | 2 | 0 | 0 |
| C | Needs outside input or an owner decision | 15 | 8 | 5 | 2 | 0 |
| D | Post-beta (v1.x/v2), deferred with a reason | 33 | 6 | 27 | 0 | 0 |
| E | Remove, merge or deprecate | 14 | 3 | 9 | 1 | 1 |
| | **Total** | **262** | 205 | 53 | 3 | 1 |

Sizes: XS status only or under an hour, S under half a day, M about a day, L several days, XL more than a week (only in class D or C). For class B the size is lab time with the hardware, not coding. 'Conflict area' names the code area; rows in one area go to one agent at a time.

Hardware and service legend used in class B and C:

| Code | Meaning |
|---|---|
| H1 | Mac mini 2018 (Macmini8,1, i7, 32 GB) |
| H2 | 2019 MacBook Pro with Touch Bar (exact model id not yet known) |
| H3 | An Apple Silicon Mac (model not yet known; M1/M2 native, M3 limited, M4 and newer VM only) |
| H4 | One or more ordinary PCs (UEFI and BIOS; Intel and AMD Wi-Fi; not yet known what the owner has) |
| H5 | ARM64 VM (UTM or QEMU) |
| H6 | A real k3s cluster owned by the owner (a throwaway one; never a shared or production cluster) |
| H7 | The owner's own accounts for Claude Code, Copilot, Codex, Gemini |
| H8 | Real speakers/headphones and a microphone |
| H9 | USB webcam(s) and the FaceTime HD camera |

## A. Close in code (tests, no hardware)

| ID | Work (one sentence) | Size | Conflict / dependency area | Who/what |
|---|---|---|---|---|
| F-DISP-02 | Audit every Settings/launcher page at 150%/200% on 800x480 and 1024x600; scroll or wrap overflow; one render test per page. Resolution half: add a mode list test. | M | shell UI | agent |
| F-VAULT-05 | Widen the chat credential guard (PEM variants, JWT, connection strings) with positive/negative tests. | S | agent guard | agent |
| F-ASSIST-21 | Pass the workspace test command into the chat and build the http check so done-claims trigger the checks. | M | agent chat/IDE | agent |
| F-ASSIST-22 | Settings fields for think_limit_secs/silence_limit_secs. | S | settings | agent |
| F-ASSIST-24 | Settings switch and Forget button for workspace history; forget_session on chat delete. | M | chat/IDE/settings | agent |
| F-ASSIST-25 | Add the missing click test for Mark done on a review card. | S | ide tests | agent |
| F-IDE-05 | Show a header total of elapsed time on the board. | S | board | agent |
| F-IDE-06 | Settings field for commands.coverage in project.json. | S | ide settings | agent |
| F-DATA-03 | Render data_query results as a table block with an Open in Data viewer button. | M | chat view | agent |
| F-EDIT-14 | Enter between brackets splits the pair; ignore brackets in strings/comments. | S | iyakki-edit | agent |
| F-EDIT-16 | Stop a running file and feed it input; parse cargo test / pytest output into a result list. | M | edit/ide | agent |
| F-IDE-01 | Implement Clone from a URL through the runner (test with a local bare repo). Keyboard/IME claims go to a lab check. | S | ide | agent |
| F-IDE-10 | Confirm before closing the IDE window while tasks run. | S | ide tasks | agent |
| F-EXTA-03 | Delete the vendor session-id file when a chat is deleted. | S | extagent | agent |
| F-EXTA-05 | Persist the Autopilot cost total across Resume; footer-text test. | S | autopilot | agent |
| F-APPS-02 | Install a skill package from a store index entry; Settings page (list, update, remove). | M | store/pkg | agent |
| F-APPS-03 | Count tool file writes against the run budget; update check; test with a Rust-built wasm32 module. | M | store/pkg | agent |
| F-APPS-40 | Notes list no missing work; confirm and mark done. | XS | status | agent |
| F-APPS-41 | Notes list no missing work; confirm and mark done. | XS | status | agent |
| F-APPS-43 | Notes list no missing work; confirm and mark done. | XS | status | agent |
| F-APPSTORE-10 | A CLI item opens the CLI tile on its entry. Bare-metal drawing check is a lab check. | S | store tile | agent |
| F-BROW-01 | Note is stale; add find-in-page and in-page anchors, then close. | S | browser | agent |
| F-BROW-05 | Animated WebP frames, GIF loop count, picture/srcset, lazy loading. | M | browser images | agent |
| F-BROW-07 | Cookies persist in the vault (F-BROW-31); fix the note, mark done. | XS | status | agent |
| F-BROW-14 | Style sheets are used (F-BROW-06); fix the note, mark done. | XS | status | agent |
| F-BROW-22 | Streamed download with progress, open from Ctrl+J list, cancel stops the fetch. | M | browser | agent |
| F-BROW-23 | Bookmark folders (create/move) and a history search box. | M | browser data | agent |
| F-BROW-24 | Remove a saved sign-in from the browser. Passkeys are D. | S | browser logins | agent |
| F-BROW-25 | Ctrl+scroll zoom and text-only size. | S | browser | agent |
| F-BROW-29 | Favicons and inline svg elements. Canvas 2D, AVIF, APNG: see D. | M | browser images | agent |
| F-BROW-34 | Certificate details view; mixed-content blocking for scripts/styles. | M | browser/proxy | agent |
| F-BROW-36 | ETag/Last-Modified revalidation in the disk cache. | M | iyakki-web cache | agent |
| F-BROW-37 | User-defined address-bar short words. | S | browser config | agent |
| F-BROW-06 | Finish the listed missing style-sheet features; keep real-page list green. | M | browser CSS | agent |
| F-BROW-08 | ES modules, click/typing events inside pages, script navigation (list in the row). | L | iyakki-js | agent |
| F-BROW-18 | Close the failing platform APIs found by probe_many on the 20-site list (3 of 20 still error). | L | iyakki-js | agent |
| F-BROW-31 | Lift the 64 KiB vault limit or document it as the limit; storage for a locked vault. | M | browser storage | agent |
| F-BROW-38 | Keyboard navigation across links/forms and screen-reader hooks of the reader browser. | M | browser | agent |
| F-BROW-30 | Drop-down popups, date pickers and other form controls in the reader browser (full engine already has them). | L | browser forms | agent |
| F-BROW-35 | Print to PDF and a PDF viewer (page images, search); in-house or permissive crate. | L | browser/PDF | agent |
| F-SHELL-09 | Headless smoke script for the egui window (compile + start under xvfb) so the row can be done. | S | gui | agent |
| F-SHELL-12 | Touchpad on bare metal: evdev tap/scroll/gestures in the fbdev shell, unit-tested with scripted events. | M | input | agent |
| F-APPS-04 | Signed index plus store CLI for self-hosted/LAN mirror; finish the missing parts in the row. | L | store | agent |
| F-APPS-05 | Ratings and comments per package version (1 to 5, anonymous or per account, moderation hooks, local cache, offline queue, spam limits); server part verified live in class B. Moved from D on 2026-10-20 (owner priority 3, BETA-PLAN workstream (j)). | L | store | agent |
| F-APPS-06 | Optional service apps: install on demand, off by default; write the model and tests. | M | store | agent |
| F-DEVMODE-08 | mDNS discovery of developer-mode devices on the LAN (in-house, tested on loopback). | M | net | agent |
| F-DRIVERS-05 | Re-verify the FaceTime HD checksum table against the upstream script (transcribed through a summariser). | S | drivers | agent |
| F-DRIVERS-14 | In-house fan controller for Macs (applesmc read/write), tested with fake sysfs. | M | drivers | agent |
| F-RADIO-03 | Ogg Vorbis/Opus (permissive decoder) and HLS; AAC only through the media-player fallback or a user-triggered codec pack (decided 2026-10-20, BETA-PLAN section 10). | M | radio | agent |
| F-MEDIA-07 | Sample media installed in every image: check each variant build lists them. | S | media | agent |
| F-FLOW-04 | Agent steps with tools and external agents through the executor seam. | M | flows | agent |
| F-FLOW-11 | Store screen lists and installs flow templates. | S | flows/store | agent |
| F-STORE-05 | Encrypt the data volume (dm-crypt/LUKS or in-house): key from login, tests in QEMU. See owner decision. | L | storage | agent + QEMU |
| F-PKG-11 | Compress kernel modules (zstd/xz decision), drop rarely used drivers; budget in BETA-PLAN. | M | build | agent + owner decision |
| F-BOOT-14 | Two-stage boot: small first image on FAT, rest from the data side. Owner priority 1 (BETA-PLAN workstream (i)). | L | boot | agent, then H1 |

## B. Verify on real hardware or a real service

| ID | Work (one sentence) | Size | Conflict / dependency area | Hardware / service (legend above) |
|---|---|---|---|---|
| F-HW-02 | Run mic and camera capture on real devices. | M | audio/camera | H1, H8, H9 |
| F-HW-03 | Re-run the UVC quirk table on the Mac mini and a second webcam. | S | camera | H1, H9 |
| F-NET-02 | Associate with a real access point (WPA2, WPA3, open); on H1 after the firmware routes in docs/MAC-WIFI-BLUETOOTH-SETUP.md (owner priority 2). | M | wifi | H4 Wi-Fi PC, USB adapter, H1 |
| F-NET-07 | Settings → Network scan/connect with a real adapter. | M | wifi UI | H4, USB adapter |
| F-NET-09 | Private MAC on a real adapter; list drivers that refuse it. | S | wifi | H4 |
| F-NET-10 | Pair with a real controller (needs working HCI). | M | bluetooth | H4 or H1 after C-NET-11 |
| F-SND-02 | Output selection on the Mac mini (HDMI devices 3, 7, 8). | S | audio | H1, H8 |
| F-SND-03 | Software volume on outputs without hardware volume. | S | audio | H1 |
| F-PKG-14 | Run on other base OSes incl. framebuffer desktop and driver loading. | M | bases | H4, VMs |
| F-PKG-15 | Run repack on real full builds; compare with clean builds. | M | build | build PC |
| F-SHELL-13 | Measure pointer/redraw responsiveness on real hardware. | S | shell | H1, H2, H4 |
| F-AI-09 | Live Lemonade Server: models, chat, pictures, speech. | M | models | AMD box or Lemonade host |
| F-AI-10 | Same, by model type (Kokoro voices, image wait). | S | models | AMD box or Lemonade host |
| F-ASSIST-10 | Live model run of the robustness cases. | S | assistant | local model |
| F-ASSIST-11 | Speed bench on the lab machines. | S | assistant | H1, local model |
| F-ASSIST-14 | Live qwen3:8b stall run. | S | assistant | local model |
| F-ASSIST-15 | Live board planning run. | S | assistant | local model |
| F-ASSIST-17 | Live board-tracker run. | S | assistant | local model |
| F-ASSIST-18 | Autopilot on a real build and deploy. | M | autopilot | local model, H6 |
| F-ASSIST-19 | Auto mode on a real cluster. | S | autopilot | local model, H6 |
| F-ASSIST-20 | Live proof-before-done run. | S | autopilot | local model |
| F-ASSIST-23 | Token budget measured with live model. | S | autopilot | local model |
| F-ASSIST-26 | Advice detector against live wording. | S | autopilot | local model |
| F-ASSIST-27 | Pretty-printed JSON calls from live models. | S | assistant | local model |
| F-ASSIST-28 | Prompt phrase tables with live models. | S | assistant | local model |
| F-ASSIST-29 | No-question rule with live models. | S | autopilot | local model |
| F-BUG-01 | Bug evidence from a real kubectl. | S | board | H6 |
| F-IDE-08 | Stack guessing on 5 real sample projects. | S | ide | agent + sample repos |
| F-IDE-07 | Look at Bugs lane on a real screen. | XS | board | H1 |
| F-DASH-04 | Real DuckDB command (user-downloaded) end to end. | M | dash | network |
| F-DASH-06 | Bare-metal look with real mouse. | S | dash | H1 |
| F-DASH-08 | Real model writes queries. | S | dash | local model |
| F-VOICE-04 | Real microphone and speaker. | M | voice | H1, H8 |
| F-VOICE-05 | Read aloud with the engines found on a real image. | S | voice | H1, H4 |
| F-VOICE-06 | Browser voice helper in a real browser. | S | voice | H4 |
| F-VOICE-08 | Listen: word highlight against real sound. | S | voice | H8 |
| F-VOICE-14 | Build the indic-tts Dockerfile and deploy on a real cluster. | M | voice/k8s | H6, C-VOICE-09 |
| F-VOICE-15 | Word timing with a live Kokoro/Lemonade server. | S | voice | Lemonade host |
| F-VOICE-16 | Lemonade/Kokoros as reading voice, local and remote. | S | voice | Lemonade host |
| F-EDIT-15 | Inline completion with a real model. | S | edit | local model |
| F-IDE-09 | Browser tab with real WebKit/WPE on screen. | M | ide | H1 |
| F-IDE-13 | Shared context with real model and Claude Code. | S | ide | H7 |
| F-IDE-14 | Dependency graph looked at by a person. | XS | ide | owner |
| F-IDE-15 | Several real agents on the board. | M | ide | H7 |
| F-IDE-16 | Phrase changes with a real chat. | XS | ide | local model |
| F-IDE-17 | Task hand-off through the real shell. | S | ide | H1 |
| F-IDE-11 | Run opens app full screen; look on bare metal. | S | ide | H1 |
| F-IDE-12 | Target picking with real models. | S | ide | local model |
| F-IDE-02 | Cluster view on a real k3s (read-only). | S | ide/k8s | H6 |
| F-K8S-02 | Deploy flow build, load, apply, verify on a real k3s. | M | k8s | H6 |
| F-K8S-06 | Registry setup on a real cluster. | S | k8s | H6 |
| F-K8S-07 | Hidden kubeconfigs: try with a real second cluster file. | S | k8s | H6 |
| F-K8S-08 | Teardown on a real dev cluster. | S | k8s | H6 |
| F-K8S-10 | Container tests with real docker/podman and kubectl. | S | k8s | H6 |
| F-K8S-12 | Settings → Dev cluster against a real remote cluster (read-only, owner's own). | S | k8s | H6 remote, owner approval |
| F-K8S-13 | Bare-metal Set up a local cluster: real k3s download and start. | M | k8s | H1, network |
| F-K8S-14 | Cluster health with real kubelet (disk-stats errors seen). | S | k8s | H6 |
| F-K8S-15 | Cloud mode inside a real pod. | M | k8s | H6 pod |
| F-K8S-16 | Platform services engine on a real cluster. | M | k8s | H6 |
| F-K8S-17 | Gitea on a real cluster; git credential helper gap. | M | k8s | H6 |
| F-K8S-18 | Platform services UI on bare metal. | S | k8s | H1, H6 |
| F-CLI-03 | Container cluster on the machine itself: repeat on H1. | M | k8s | H1 |
| F-CLI-09 | Real downloads of Claude Code/Node etc. (vendor, at user request). | S | cli | network |
| F-CHAT-23 | Read-aloud sentence sync with real audio. | S | voice | H8 |
| F-EXTA-01 | Real Claude Code, Copilot, Codex, Gemini CLIs in bridged mode. | M | ext agents | H7 |
| F-EXTA-02 | Policy bridge: check _meta tool-use id from a live Claude Code. | S | ext agents | H7 |
| F-EXTA-04 | Settings → Agents with real installs. | S | ext agents | H7 |
| F-EXTA-06 | Real npm install of the CLIs from the catalogue. | S | ext agents | network |
| F-EXTA-07 | Login/logout chip with real accounts. | S | ext agents | H7 |
| F-EXTA-09 | Sign-in code relay from a phone. | S | ext agents | H7, phone |
| F-EXTA-10 | Copilot token in the vault with a real config.json. | S | ext agents | H7 |
| F-EXTA-11 | Copilot with tools/approvals live. | S | ext agents | H7 |
| F-EXTA-12 | Repeat device pass for all agents. | S | ext agents | H1, H7 |
| F-EXTA-13 | tile_ship against a k3s-deployed store. | S | ext agents | H6 |
| F-JOBS-04 | Jobs gate with a real Claude Code. | S | ext agents | H7 |
| F-FLOW-05 | Job runner against a real cluster. | S | flows | H6 |
| F-FLOW-09 | Look at FLOWS tile in a real window. | XS | flows | H1 |
| F-FLOW-17 | Browser QA nodes on the full engine. | M | flows | H4 |
| F-FLOW-18 | Consent list with the profile shared. | S | flows | H4 |
| F-FLOW-19 | Visual regression on the full engine. | S | flows | H4 |
| F-FLOW-20 | Vision model picture handling with a real model. | S | flows | local model |
| F-APPS-01 | Install a signed package on a device. | S | pkg | H1 |
| F-APPS-42 | Private store as platform service on a real cluster. | M | store | H6 |
| F-APPS-44 | Pull the pushed image with real containerd. | S | store | H6 |
| F-APPS-45 | Tile apps on bare metal. | S | tiles | H1 |
| F-APPS-46 | Custom apps in launcher on hardware. | S | tiles | H1 |
| F-APPS-47 | Publish from the IDE to a real cluster and remote store. | M | store | H6 |
| F-APPS-48 | Local registry in one click on a real cluster. | M | store | H6 |
| F-APPS-49 | containerd pull and Quick install job. | M | store | H6 |
| F-APPSTORE-23 | Update checks against a real catalog host and vendors. | M | store | C-portal |
| F-DRIVERS-03 | Firmware symlinks on a real Mac. | M | drivers | H1 |
| F-DRIVERS-07 | Drivers chip on bare metal. | XS | drivers | H1 |
| F-DRIVERS-09 | Build, boot the Mac image variant; run kconfig on fragments. | L | drivers | H1, build PC |
| F-DRIVERS-11 | apple-bce on Mac mini 2018 and T2 MacBook. | M | drivers | H1, H2 |
| F-DRIVERS-15 | Build and boot the macweb variant. | L | drivers | H1, build PC |
| F-DRIVERS-18 | T2 MacBook: keyboard, trackpad, Touch Bar, GPU, suspend. | L | drivers | H2 |
| F-BROW-04 | Full engine on bare metal (not just QEMU). | M | full engine | H1, H4 |
| F-BROW-26 | Review Indic/complex scripts with native readers. | M | browser | reviewers |
| F-BROW-39 | Assistant panel with a real provider. | S | browser | network |
| F-BROW-43 | WPE helper on real hardware. | M | full engine | H1, H4 |
| F-BROW-44 | Live supervised automation checks. | M | browser agent | H4, local model |
| F-BROW-45 | Test runner on real sites. | S | browser | H4 |
| F-BROW-50 | Playback figures on real GPUs (VA-API). | M | full engine | H1, H4 |
| F-MEDIA-01 | Media tile on bare metal with real audio/video. | M | media | H1, H8 |
| F-MEDIA-03 | Search on a large real library. | S | media | H1 |
| F-MEDIA-04 | Chat search/play with a real model. | S | media | local model |
| F-MEDIA-05 | Download from real addresses. | S | media | network |
| F-MEDIA-06 | Queue with real downloads. | S | media | network |
| F-MEDIA-08 | In-house player with real ffmpeg on hardware. | M | media | H1, H8 |
| F-MEDIA-09 | Get ffmpeg: real download, xz reader. | S | media | network |
| F-TALES-02 | Hear the story read aloud. | S | tales | H8 |
| F-TALES-09 | Web Speech path in real browser. | S | tales | H4 |
| F-TALES-11 | TALES with real Claude Code. | S | tales | H7 |
| F-TALES-13 | Pictures with a real image server. | S | tales | image server |
| F-TALES-15 | Pictures by writer: judge quality. | S | tales | H7 |
| F-TALES-16 | Cartoon composer with small models. | S | tales | local model |
| F-TALES-18 | Real image/video model. | M | tales | image server |
| F-TALES-19 | Word highlight timing on hardware. | XS | tales | H1, H8 |
| F-CAM-01 | LAN discovery with real ONVIF cameras. | M | camera | IP camera |
| F-BOOT-08 | aarch64 image in UTM on an Apple Silicon Mac and in QEMU. | M | boot | H3 or H5 |
| F-PKG-06 | USB writer on macOS and a Linux PC. | M | usb | macOS Mac, H4 |
| F-BOOT-12 | Enrol own key in PC firmware (db) on PCs that allow it. | M | boot | H4 |
| F-BOOT-13 | Boot screen on the Mac mini. | S | boot | H1 |
| F-BOOT-19 | Collect dmesg splash lines on the Mac mini; confirm cause. | S | boot | H1 |
| F-BOOT-20 | GRUB menu over logo on real firmware. | S | boot | H1, H4 |
| F-BOOT-21 | Monitor re-probe on the Mac mini HDMI converter case. | S | boot | H1 |
| F-BOOT-22 | Splash continues GRUB on real hardware. | S | boot | H1 |
| F-BOOT-23 | Compile the i915 patch and test flicker-free start. | M | boot | H1, build PC |
| F-BOOT-24 | Real-Mac check of post-GRUB splash. | S | boot | H1 |
| F-BOOT-25 | Measure GRUB wait with rebuilt image. | S | boot | H1 |
| F-BOOT-26 | First-boot speed on the Mac mini. | S | boot | H1 |
| F-BOOT-01 | Boot-test matrix on real PCs (UEFI/BIOS). | L | boot | H4 several, docs/BOOT-TEST.md |
| F-BOOT-02 | Secure Boot shim+MOK on real PCs and T2 Macs; signed UKI for initrd. | L | boot | H4, H2 |
| F-SYS-01 | SYSTEM tile on a real machine. | S | system | H1 |
| F-SYS-03 | SSH diagnostics with authorized keys on other hardware. | S | system | H4 |
| F-SHELL-20 | run.sh window on an Apple Silicon Mac. | M | shell | H3 |
| F-SHELL-28 | Quick install with real downloads. | M | shell | H1, network |
| F-WEB-06 | Headless multi-user mode: finish after real K3s trial. | M | web | H6 |
| F-STORE-04 | Install to disk incl. internal drives on a PC. | M | install | H4 |
| F-STORE-09 | Bind mount of the files folder on a Mac mini. | S | storage | H1 |
| F-INST-01 | Install from stick on a real PC and Mac. | M | install | H1, H4 |
| F-REL-01 | /etc/iyakki-release on a freshly built image. | S | release | build PC |
| F-REL-05 | Run ./run.sh release end to end. | M | release | build PC |
| F-ZRAM-01 | zram on H1 and a small-RAM PC. | S | zram | H1, H4 |
| F-ZRAM-02 | Swap file on the data volume. | S | zram | H1 |

## C. Needs outside input or an owner decision

| ID | Work (one sentence) | Size | Conflict / dependency area | Who/what is needed |
|---|---|---|---|---|
| F-BOOT-09 | Native Apple Silicon: needs owner's Mac model; M1/M2 only, M3 limited, M4+ never in beta. | L | boot | H3, owner decision |
| F-APPSTORE-02 | Generate the official root key and fill official-roots.txt. | M | store | owner key custody |
| F-APPSTORE-06 | Host a signed catalog at a real URL. | M | store | portal URL |
| F-APPSTORE-14 | Official root key and hosted official catalog. | M | store | owner |
| F-NET-05 | Decide BlueZ or in-house HID/A2DP, then build (owner priority 2; decision still open). | L | bluetooth | owner decision |
| F-NET-11 | BCM4364 on UART answers but asks for the patch file `brcm/BCM.hcd` and has no address: install it by the routes in docs/MAC-WIFI-BLUETOOTH-SETUP.md (owner priority 2). | L | bluetooth | owner extraction |
| F-PKG-04 | hw image with firmware blobs: licence review of each blob. | M | build | owner / legal |
| F-AI-02 | Live OpenAI/Claude/Gemini checks. | M | models | owner API keys |
| F-DRIVERS-10 | Mac mini 2014 BCM4360 via wl pack: no 2014 Mac in lab. | M | drivers | waive or borrow |
| F-DRIVERS-12 | Apple Silicon bare metal: needs M1/M2 Mac. | XL | drivers | H3 |
| F-DRIVERS-13 | Download Apple camera driver on device: legal review. | S | drivers | owner / legal |
| F-DRIVERS-16 | Broadcom wl pack: Broadcom licence; user-build only. | S | drivers | owner / legal |
| F-DRIVERS-19 | Asahi installer package: needs M1/M2 Mac to test. | M | drivers | H3 |
| F-VOICE-09 | Indic TTS weights licence decision. | S | voice | owner / legal |
| F-VOICE-10 | Native speakers check number tables. | S | voice | reviewers |

## D. Deferred after beta

| ID | Work (one sentence) | Size | Conflict / dependency area | Reason |
|---|---|---|---|---|
| F-FLOW-06 | External agent as flow healer. | M | flows | post-beta |
| F-BROW-41 | Console prompt, inspector, report button. | M | browser | post-beta |
| F-BROW-42 | Open list of standards the reader engine lacks. | L | browser | post-beta |
| F-FILE-01 | SFTP server. | M | files | post-beta |
| F-FILE-02 | SFTP/FTPS client app. | M | files | post-beta |
| F-MEDIA-02 | DLNA/UPnP and casting. | L | media | post-beta |
| F-CAM-02 | Live camera view and RTSP. | L | camera | post-beta |
| F-CAM-03 | Recording with retention and encryption. | L | camera | post-beta |
| F-CAM-04 | Motion/person detection. | L | camera | post-beta |
| F-CAM-05 | Agent alerts from a vision model. | M | camera | post-beta |
| F-PKG-07 | Windows USB writer; document Rufus/dd mode for beta. | L | usb | post-beta |
| F-PKG-08 | USB writer with a window. | L | usb | post-beta |
| F-BOOT-11 | Own shim signed by Microsoft: EV cert, review queue of months. | L | boot | post-beta |
| F-BOOT-15 | Critical-path boot ordering. | L | boot | post-beta |
| F-BOOT-04 | Device broker for lazy driver loading. | L | boot | post-beta |
| F-SYS-02 | Load/unload drivers (needs broker). | M | system | post-beta |
| F-APPSTORE-08 | Licence tokens for paid apps. | L | store | post-beta |
| F-APPSTORE-13 | Payments. | L | store | post-beta, business decision |
| F-APPSTORE-16 | Extension taxonomy beyond WASM tools/themes. | L | store | post-beta |
| F-SHELL-10 | Direct DRM/KMS+GBM renderer; fbdev stays. | XL | shell | post-beta |
| F-NET-04 | Swarm scheduler beyond first peer. | M | net | post-beta |
| F-WASM-06 | Copy-on-write snapshots. | XL | core | post-beta |
| F-WASM-07 | Snapshot mid-call. | XL | core | post-beta |
| F-MEM-02 | External vector store. | L | memory | post-beta |
| F-WEB-04 | Native web views for Chat, Edit, Runs, Settings. | XL | web | post-beta (web plan) |
| F-WEB-05 | iyakki-edit to wasm32 for the browser. | M | web | post-beta |
| F-STORE-06 | Read-only compressed system image (EROFS/overlay). | XL | storage | post-beta |
| F-STORE-07 | Evaluate F2FS. | M | storage | post-beta |
| F-INST-04 | Install next to another OS. | L | install | post-beta |
| F-CLI-08 | Interactive SSH sessions (pty). | L | cli | post-beta |
| F-FLOW-14 | Thrift RPC node. | M | flows | post-beta |
| F-FLOW-15 | Webhook trigger. | M | flows | post-beta |
| F-FLOW-16 | Remote runner. | M | flows | post-beta |


**Applied on 2026-10-20 (owner: "let us do remove or merge").** Removed (no code behind them): F-DEP-03, F-FILE-03, F-BROW-02, F-BROW-27, F-BOOT-16 (its idea moved into F-BOOT-02 as the signed unified kernel image follow-up). Merged and deprecated: F-WEB-07 (into F-WEB-03) and F-BROW-16 (into F-BROW-50 and F-BROW-43). Reworded to the full web engine: F-BROW-28, F-BROW-32, F-BROW-33; F-PKG-05 now means measured budgets. F-BROW-40 stays as the one optional-browser row. Still open: F-EDIT-17 (needs the legacy shim deleted in code) and F-REL-07 (deprecate right after the alpha tag; the script is still needed). FEATURES.md now reads done 286 · partial 203 · planned 48 · blocked 3 · deprecated 3 · total 543. The tables above are the 2026-10-19 snapshot.

## E. Remove, merge or deprecate

| ID | Work (one sentence) | Size | Conflict / dependency area | Decision by |
|---|---|---|---|---|
| F-EDIT-17 | Delete the legacy shim after iyakki-gui uses iyakki-edit; remove row in same change. | S | iyakki-ui | agent |
| F-DEP-03 | Drop: thiserror replacement is low value; remove the row. | XS | build | owner |
| F-WEB-07 | Merge into the pixel-streamed desktop row (CLI tile already works in the browser). | XS | web | owner |
| F-BROW-02 | Duplicate of F-BROW-40; merge. | XS | browser | owner |
| F-BROW-40 | Keep one row for the browser as an optional package, depends on F-APPS-06. | XS | browser | owner |
| F-BROW-16 | Superseded by F-BROW-50 (video plays in the full engine); merge. | XS | browser | owner |
| F-BROW-27 | Same: video and MSE are served by the full engine (F-BROW-43/50); merge. | XS | browser | owner |
| F-BROW-28 | Reword: layout sizes for scripts are the full engine's job; the reader browser stays a reader. | S | browser | owner |
| F-BROW-32 | Reword to the full engine (WebSocket, EventSource, fetch streams are native there). | S | browser | owner |
| F-BROW-33 | Reword to a per-release site list run on the full engine (harness exists). | S | browser | owner |
| F-PKG-05 | Replace 30 MB / 100 ms targets by measured budgets (BETA-PLAN, section 7). | S | build | owner |
| F-REL-07 | commit-alpha.sh is obsolete after the tag; deprecate. | XS | release | owner |
| F-BOOT-16 | Merge into the signed unified kernel image work (F-BOOT-02 follow-up). | S | boot | owner |
| F-FILE-03 | Plain FTP: drop (unencrypted). | XS | files | owner |

## Owner decisions of 2026-10-20 that change this list

* Beta priorities, in order: (1) boot problems, (2) Wi-Fi and Bluetooth, (3) the store update framework (updates, ratings, comments) and upgrade compatibility. Rows F-BOOT-14, F-BOOT-25/26, F-PKG-11, F-NET-02/05/07/10/11 and F-DRIVERS-03 are the first two; F-APPS-05 (ratings, moved from D to A), F-APPSTORE-23 and F-PKG-06 serve the third. The work that has no row yet (versioned settings schemas, compatibility matrix, migration wizard, `iyakki-usb --update`, update of all package kinds) is workstream (j) of [`BETA-PLAN.md`](BETA-PLAN.md); rows for it are added to FEATURES.md by whoever owns that file.
* H.264/AAC: no codec is shipped; see BETA-PLAN section 10 (F-RADIO-03, F-BROW-50).
* Private store delete, repair and reinstall went into the alpha; no row of this scope covers it, so no row moved (BETA-PLAN section 9).

## Honest reading

"All rows working" can mean three different things. Only the first two can be promised.

1. **Every row is either done, or verified, or honestly labelled.** Class A is finished in code and tests. Class B is run once by hand on the listed hardware and the evidence (date, image build id, model of machine, log or photo) is written into the beta gate file. Class E rows are removed or merged so they stop counting.
2. **Class C and D rows are not "working" at the beta tag.** They are written down as planned in the README and in the known-issues file. Class C rows are resolved by the owner (a key, a URL, a licence answer, a firmware file) or waived in writing; a waiver moves the row to D with the reason.
3. **All 548 rows done** cannot be reached by code alone and is not the recommended meaning. The reasons:
   * **Hardware.** Rows that talk to a Wi-Fi chip, Bluetooth controller, Touch Bar, camera or GPU only count as working when they ran on that chip. We have one Mac mini 2018 and (to be confirmed) one T2 MacBook and one Apple Silicon Mac.
   * **Apple Silicon M4 and newer** have no bare-metal Linux path for anyone today (see `APPLE-SILICON.md`); they are VM only. M3 is framebuffer-only at best. A row cannot be made to work by IYAKKI code alone.
   * **Vendor services.** OpenAI, Claude, Gemini, Copilot, Codex and a hosted official catalog change without notice; a row stays "verified on date X", and the check is repeated before each release.
   * **Firmware and licences.** The Mac mini 2018 Wi-Fi and Bluetooth need Apple/Broadcom files the project may not ship. Rows can work for a user who supplies the file; they cannot be done for a user who does not.
   * **Deferred features** (class D) are product choices, not defects.

A blocker, in this document, is: data loss, a boot failure on a listed machine, a security flaw that lets one user read another's data or run an unapproved action, a build that cannot be installed, or a class-B row on a listed machine that fails with no workaround.

## Recommended beta exit criteria

1. 100% of class A rows done, each with a test that fails without the change; FEATURES.md validated by the matrix test.
2. Every class B row verified on the hardware listed, with evidence written into the beta gate file; a failed row either fixed or moved to known issues by the owner with a workaround.
3. Every class C row resolved, or waived by the owner in writing (the waiver names the row and the reason). The five rows that matter most: F-NET-11, F-APPSTORE-02/14, F-AI-02, F-VOICE-09, F-BOOT-09.
4. Class D rows carry a "post-beta" label in README and known issues; class E changes are made in FEATURES.md.
5. No open Sev-1 or Sev-2 issue (Sev-1: data loss, boot failure, security; Sev-2: a core feature unusable with no workaround on a listed machine).
6. The full test suite green on CI for 14 consecutive days, including the QEMU boot scripts and `run.sh alpha-check` (renamed for beta).
7. A clean-room install from the published release files on every listed machine, followed by a 24-hour soak with no crash and no unbounded memory growth.
8. Signed release files whose signature a user can check; third-party licence file and SBOM generated from the tag.
9. Documentation complete: user guide, admin guide, hardware compatibility list, troubleshooting, security policy with a working contact.
