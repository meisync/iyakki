# Manual test checklist for the weekend of 2026-11-14, version 2

Written 2026-11-13 for the owner's own manual tests on the **Lenovo (T7 stick)**, the **Mac mini 2018** and the **MacBookPro14,1**. It replaces version 1 as the thing to work from: version 1's long list is still below, in the last part, for the rows that no check here covers.

**What is new in this version.** The checks are ranked by how many feature rows one check can finish, each check has one to three steps in plain words, an expected result and the exact row ids to report, and the things built today (live scripts in the browser, HTTP/2, the bad-site list, PDF passwords and forms, the web UI tiles, themes, browser data protection, Gemini, the echo canceller) have their own part. Only rows whose code is written (Impl 100) are in the ranked checks: a row with Impl open cannot reach done by looking at it, so it stays out of them. Of the 272 rows with Impl 100 that need a person (154 are covered by the 43 checks below), the rest are listed at the end.

## How to report

Send one line per row id, in exactly these words:

- `F-XXX-NN: pass`
- `F-XXX-NN: fail + what you saw` (the words on the screen, the machine, the time; a photo is welcome)
- `F-XXX-NN: skipped + why` (no phone, no cable, no time): nothing changes for it

One check names several rows: report every row of the check, because a check can pass for one row and fail for another (say which step it was).

## What each answer changes

- **pass**: the row's Manual column becomes `passed <date> <machine>`. The row becomes **done** (Impl 100 and Test 100) when its Notes say the manual try was the only gap left; the check headings below say how many rows a pass can finish. A row whose Notes list other gaps stays partial with its Manual passed.
- **fail**: a new KI number is made for what you saw and the Manual column becomes `failed KI-nnn`. The row stays partial.
- **skipped**: no change.

## Safety

- Never run anything that writes to a machine's internal disk. Run from the T7 stick or the install already in place. Rows that would write the internal disk (F-INST-03, F-CLOUD-53) or the boot partition are not in this list.
- The hard asks need the owner's Yes first: downloads of over a gigabyte (F-CLI-04), the built-in microphone switch, which may freeze the T2 Mac (F-SND-20, be ready to power-cycle), vendor downloads (F-CLI-02's kubectl, F-NET-11's Bluetooth patch).
- On every Mac run `iyakki dev state` before sending keys. Esc or q on the Home screen signs the person out.
- Do not type passwords or keys into anything except the sign-in screen and the forms a check names. The Gemini check names one key; keep it in a file with mode 600 outside the repo and never in a chat.
- Use test accounts and test sites only. Nothing here needs a real personal account.

## The three machines

| Short name | What | Notes |
|---|---|---|
| Lenovo (T7 stick) | the Lenovo, booted from the Samsung T7 install | boot, Wi-Fi, GPUs, touch if the screen has it |
| Mac mini 2018 | Macmini8,1 with a 4K monitor | most checks; sound over HDMI; pair by name `iyakki-30ed15.local` and check `iyakki dev info` first (the same T7 install also boots the other machines) |
| MacBookPro14,1 | the 2017 laptop | battery, Bluetooth, speakers, keyboard, speed numbers |
| Any machine | a phone, the Linux PC, or any of the above | marked so |

## The top 40 checks, best value first

Ranked by the number of rows one check can finish (most first), then by the minutes it takes (fewest first). "Rows" counts rows with Impl 100 and Manual `needed`. A `◇` after the rows means that fewer than half of its rows had steps written in version 1: the steps then come from the rows' own text, so if a button is not where the step says, report what you found there.

### 1. The AI IDE on Autopilot: the board, the proof rule and the agents

**Machine:** Mac mini 2018. **About 30 min.** **Moves 12 rows.** ◇

1. Open the AI IDE (needs a chat model), make a workspace for a small tile app, and switch the approval chip to Autopilot (Ctrl+Q).
2. Give it a goal with several steps, for example: make a tile with a counter, test it and tell me when it is done. Keep the Board tab and the Agents strip in view.
3. When it ends read its summary and look at the board and the Agents strip. Write down every time it asked you to do something by hand (it should never).

**Expect:** The assistant adds one planned row per step itself, moves rows only after a real check (a refused done says what is missing), is never told to ask you to edit a file by hand, shows its name on the cards and in the Agents strip, and ends with a summary of what really ran. A request that says tile gets the tile tools, not the cluster ones.

**Report:** `F-ASSIST-15: pass`, `F-ASSIST-17: pass`, `F-ASSIST-18: pass`, `F-ASSIST-20: pass`, `F-ASSIST-21: pass`, `F-ASSIST-23: pass`, `F-ASSIST-25: pass`, `F-ASSIST-26: pass`, `F-ASSIST-28: pass`, `F-IDE-08: pass`, `F-IDE-12: pass`, `F-IDE-15: pass` (or `fail + what you saw`).

Rows: F-ASSIST-15 (The assistant plans on the feature board in IDE workspace chats); F-ASSIST-17 (Board status tracker); F-ASSIST-18 (Autopilot for the AI IDE chat); F-ASSIST-20 (Done needs proof); F-ASSIST-21 (Claims of done trigger checks, and pending rows); F-ASSIST-23 (Autopilot context compaction and token budget); F-ASSIST-25 (Hard limits per Autopilot step); F-ASSIST-26 (Autopilot fixes instead of advising, and reports only what ran); F-ASSIST-28 (The assistant does it itself); F-IDE-08 (Every workspace run starts from the project's state); F-IDE-12 (The assistant picks the target); F-IDE-15 (Agents on the board).

### 2. The tenant settings page of the web UI

**Machine:** Any machine. **About 20 min.** **Moves 9 rows.** ◇

1. On a machine in cloud mode (or running the web service) open the web UI as a tenant administrator and open Tenant settings.
2. In each part (provider catalog and test, safety rules, media policy, local model hosts, cluster bindings for a team, a coding agent switch) change one value, save, and reload the page.
3. Type a secret in a secret field, save, reload.

**Expect:** What you saved is shown after the reload; every change is kept with its version; secrets are never shown again; a bad value is refused with a reason; the connection test says what it found.

**Report:** `F-TCFG-15: pass`, `F-TCFG-03: pass`, `F-TCFG-04: pass`, `F-TCFG-06: pass`, `F-TCFG-07: pass`, `F-TCFG-08: pass`, `F-TCFG-12: pass`, `F-TCFG-13: pass`, `F-TCFG-14: pass` (or `fail + what you saw`).

Rows: F-TCFG-15 (Tenant settings page in the web UI (`ui/tenantcfg.js`, tenant and team); F-TCFG-03 (Config plus vault); F-TCFG-04 (Versioned, file-backed tenant configuration (`tenantconfig::store`)); F-TCFG-06 (Shared coding agent switch at run start); F-TCFG-07 (Browser media policy from the tenant configuration (`media.policy`)); F-TCFG-08 (k3s cluster bindings per team and environment (dev, preprod, prod or c); F-TCFG-12 (Provider catalog and connection test (`tenantconfig::catalog`)); F-TCFG-13 (Safety rules and warnings (`tenantconfig::safety`)); F-TCFG-14 (Local model host integration (`models.local`)).

### 3. Domain names for services

**Machine:** Mac mini 2018. **About 15 min.** **Moves 6 rows.** ◇

1. Settings, Network, Domains (administrator): add a route whose name ends in .local, edit it, turn the listener on; open the name from a phone on the same network.
2. Remove the route and try the name again. Turn the listener off.
3. Ask the assistant in the chat to list the domain routes, then to add one (it asks first).

**Expect:** The page is readable and every step works; the phone reaches the device by the name and stops reaching it after the removal; the assistant lists routes without a card and asks before it changes one; a certificate warning is explained in words.

**Report:** `F-DOMAIN-01: pass`, `F-DOMAIN-02: pass`, `F-DOMAIN-03: pass`, `F-DOMAIN-04: pass`, `F-DOMAIN-05: pass`, `F-DOMAIN-06: pass` (or `fail + what you saw`).

Rows: F-DOMAIN-01 (Domain names for services, the manager (iyakki-agent domains.rs, docs/); F-DOMAIN-02 (Domain listener as a service); F-DOMAIN-03 (Local names); F-DOMAIN-04 (Certificates for domain routes); F-DOMAIN-05 (Assistant tools `domains_list` (no card), `domain_route_add`, `domain_); F-DOMAIN-06 (Settings, Network, Domains page (administrators)).

### 4. Battery of a laptop

**Machine:** MacBookPro14,1. **About 15 min.** **Moves 5 rows.**

1. Open Settings, System, Battery and look at the top bar left of the clock.
2. Unplug the charger and plug it in again; then (only if you can wait) let it fall below 20 percent once.
3. Read the bar, the status, the time left and the health.

**Expect:** The percent and charging or discharging match the laptop; the top-bar icon changes colour (green, amber under 20, red under 10, a bolt while charging, a plug when full); one message at 20 and one at 10 percent while discharging.

**Report:** `F-BATT-01: pass`, `F-BATT-02: pass`, `F-BATT-03: pass`, `F-BATT-04: pass`, `F-BATT-05: pass` (or `fail + what you saw`).

Rows: F-BATT-01 (Laptop battery reading from `/sys/class/power_supply` (pure engine ove); F-BATT-02 (Battery indicator in the top bar (launcher, apps, panes bar; left of t); F-BATT-03 (Low-battery handling); F-BATT-04 (Settings, System, Battery (laptops only)); F-BATT-05 (Kernel options for the laptop battery).

### 5. Read aloud: a story, the chat and the VOICE tile

**Machine:** Mac mini 2018. **About 20 min.** **Moves 5 rows.**

1. Open Settings, Voice with nothing set and press the read-aloud test: a voice already on the computer speaks.
2. Open TALES, open a story and have it read aloud with the mixer off; watch the highlighted word. Then in the chat ask a short question with Read aloud on.
3. Open the VOICE tile, READ ALOUD, and read a short text.

**Expect:** Speech starts with no setup; a whole story is read with no gaps inside sentences; the word being said is lit at about the right time in TALES and in the chat message; the VOICE tile reads the text.

**Report:** `F-VOICE-05: pass`, `F-SND-14: pass`, `F-VOICE-08: pass`, `F-CHAT-23: pass`, `F-VOICE-17: pass` (or `fail + what you saw`).

Rows: F-VOICE-05 (Reading aloud with no setup); F-SND-14 (One long-lived `aplay` for a whole TALES reading with the mixer off (`); F-VOICE-08 (The word being said); F-CHAT-23 (A reply read aloud is read sentence by sentence with the TALES reader ); F-VOICE-17 (VOICE tile, READ ALOUD (Ask & Create group; all roles but guest; hidde).

### 6. Boot on the Lenovo from the T7 stick

**Machine:** Lenovo (T7 stick). **About 20 min.** **Moves 5 rows.** ◇

1. Boot the Lenovo from the T7 and press any key while the logo shows: the logo moves up and a menu with a countdown and More options appears. Use keys, and touch if the screen has it.
2. Sign in, then About: read the computer's name and the boot line. Move the stick to the Mac mini and look again.
3. In the log folder of the boot volume (or Settings, System, Logs) look for the loader's boot log.

**Expect:** The menu works; the same install recognises each computer by itself (its name differs on the two); the loader's log is on the boot volume; the file list is read ahead after boot.

**Report:** `F-BOOT-01: pass`, `F-LDR-04: pass`, `F-LDR-09: pass`, `F-BOOT-30: pass`, `F-HW-04: pass` (or `fail + what you saw`).

Rows: F-BOOT-01 (Boot on real PCs); F-LDR-04 (Boot loader menu under the logo); F-LDR-09 (Boot log on the boot volume); F-BOOT-30 (Background prefetch of the boot-order file list into the page cache af); F-HW-04 (One install, several computers).

### 7. Turn cloud mode on and look around

**Machine:** Mac mini 2018. **About 25 min.** **Moves 5 rows.**

1. Settings (as administrator), the machine settings: set Cloud mode to On and restart. Look at the home screen, then press the volume, brightness and media keys.
2. Open the sign-in screen: look for two chips, This machine and Cloud, and choose Cloud (an address box for a tenant shows).
3. Sign in as administrator, open Settings, Cloud access, add one member and remove it again.

**Expect:** MEDIA, RADIO, TALES and CAMERA tiles are hidden and the others stay; the Fn, volume, brightness and media keys do nothing (power and sleep still work); the two chips are there; Cloud access lists members and namespaces and shows refusals in words.

**Report:** `F-CLOUD-01: pass`, `F-CLOUD-02: pass`, `F-CLOUD-05: pass`, `F-CLOUD-22: pass`, `F-CLOUD-20: pass` (or `fail + what you saw`).

Rows: F-CLOUD-01 (Cloud mode decision); F-CLOUD-02 (Cloud mode on the home screen); F-CLOUD-05 (Cloud mode turns off the Fn, volume, brightness and media keys with th); F-CLOUD-22 (Cloud sign-in mode and remembered tenants (`cloudtenants.rs`, `shell_t); F-CLOUD-20 (Settings -> Cloud access (`settings_cloudroles.rs`)).

### 8. Cameras: the tile, the app, the assistant, the sources

**Machine:** Mac mini 2018. **About 25 min.** **Moves 5 rows.**

1. Plug a webcam in: the CAMERA tile is enabled; unplug it: it is grey with Connect a camera. Plug it in again, open the CAMERA app, look at the preview and take a photo.
2. Open the CAMERAS tile: try the 1, 2, 4 and 6 views, add a source if you have a network camera, close the tile.
3. In the chat ask the assistant to take a photo with the camera and approve.

**Expect:** A smooth preview and a saved photo; the camera light goes out the moment the tile closes; the assistant asks first, then the photo shows in the chat; sources and who may see them behave as the page says.

**Report:** `F-CAM-06: pass`, `F-CAM-07: pass`, `F-CAM-02: pass`, `F-CAM-11: pass`, `F-CAM-16: pass` (or `fail + what you saw`).

Rows: F-CAM-06 (CAMERA tile); F-CAM-07 (Camera app); F-CAM-02 (Live camera view); F-CAM-11 (Camera for the assistant); F-CAM-16 (Camera sources, vault and per-person access (`iyakki-agent::cameras`)).

### 9. Pair a second computer and use developer mode

**Machine:** Mac mini 2018. **About 25 min.** **Moves 5 rows.** ◇

1. On the device turn Developer mode on (Settings, Developer mode), pair your PC, then from the PC run the update-pack and update commands for a test image (iyakki dev update-pack, iyakki dev update) and USB check (iyakki dev usb check).
2. Open a real terminal with iyakki dev shell --tty and run a program that needs one (top or vi).
3. In the AI IDE open Publish: This device (developer mode) is listed; in Settings, Developer mode change one family to reading only and try to write from the PC.

**Expect:** The update is offered and applied with its checks; the terminal behaves like a real one (arrow keys, resize); the paired computer appears in Publish; a family set to reading only refuses writes from the PC.

**Report:** `F-DEV-09: pass`, `F-DEV-10: pass`, `F-DEV-16: pass`, `F-DEVMODE-06: pass`, `F-WEBSVC-06: pass` (or `fail + what you saw`).

Rows: F-DEV-09 (`iyakki dev update-pack IMAGE` and `iyakki dev update PAYLOAD` (scope ); F-DEV-10 (`iyakki dev usb` (check, authorize, pair, speed, link, gadget)); F-DEV-16 (A real terminal for `iyakki dev shell --tty`); F-DEVMODE-06 (The IDE's Publish dialog lists **This device (developer mode)** while ); F-WEBSVC-06 (Programs pair with the web service through developer-mode pairing and ).

### 10. Bluetooth on the MacBook

**Machine:** MacBookPro14,1. **About 30 min.** **Moves 5 rows.**

1. Open Settings, Bluetooth: one switch at the top; switch it off and on. If it says firmware is needed press Firmware under Store, Setup, Hardware.
2. Open pairing (two minutes), pair a phone, play music on it.
3. Turn on the phone's Bluetooth tethering and connect to it from Settings, Bluetooth; turn tethering off and on.

**Expect:** The adapter is found and the page says why when it is not; music plays from the computer; a web page opens through the phone and it reconnects by itself.

**Report:** `F-NET-08: pass`, `F-NET-10: pass`, `F-NET-13: pass`, `F-NET-25: pass`, `F-NET-20: pass` (or `fail + what you saw`).

Rows: F-NET-08 (Settings → Bluetooth); F-NET-10 (In-house Bluetooth management client (`iyakki-devctl::btmgmt`, no Blue); F-NET-13 (Bluetooth honesty on Macs); F-NET-25 (Using Bluetooth audio); F-NET-20 (Bluetooth tethering, client side (use a phone's internet) without Blue).

### 11. Sound: outputs, volume and system sounds

**Machine:** Any machine. **About 10 min.** **Moves 4 rows.**

1. Open Settings, Sound: the HDMI output (or the speakers) is listed with its name; press its test sound; move the master volume with the keys, then with the speaker icon in the top bar.
2. Open Settings, System sounds; sign out and in again and listen for the login and logout notes; trigger a notification.

**Expect:** You hear the test sound from the right output; the icon shows muted, low, medium or high and the same volume shows in Settings; the cues are short and quiet.

**Report:** `F-SND-02: pass`, `F-SND-04: pass`, `F-SHELL-53: pass`, `F-VOICE-19: pass` (or `fail + what you saw`).

Rows: F-SND-02 (Sound output selection); F-SND-04 (One master volume (keys, mute, Settings, popover, radio bars; applied ); F-SHELL-53 (Speaker icon in the home screen's top bar on IYAKKI's one master volum); F-VOICE-19 (System sounds).

### 12. Settings, Assistant and the assistant's settings tools

**Machine:** Mac mini 2018. **About 15 min.** **Moves 4 rows.**

1. Open Settings, Assistant: check the layout, the history switch and the workspace list; set think seconds for all models and for one provider; press the Forget key twice on a chat.
2. Open the IDE's project settings (gear or Ctrl+R) and choose the assistant memory option at the end.
3. In the chat ask the assistant to change a setting (for example keyboard speed).

**Expect:** The stall limits change the next chat; one Forget press deletes nothing and the second does; the memory choice is applied and audited; the assistant asks every time and changes the setting only after you approve.

**Report:** `F-ASSIST-46: pass`, `F-ASSIST-47: pass`, `F-ASSIST-48: pass`, `F-CHAT-02: pass` (or `fail + what you saw`).

Rows: F-ASSIST-46 (Stall limits per provider (Settings -> Assistant); F-ASSIST-47 (Settings -> Assistant (all accounts)); F-ASSIST-48 (The IDE's project settings (the gear, Ctrl+R) end with the assistant's); F-CHAT-02 (Settings tools for the assistant (`iyakki-agent::settingstools`)).

### 13. The Store: Setup tiles, lists and drag and drop

**Machine:** Mac mini 2018. **About 15 min.** **Moves 4 rows.**

1. Open Store, Setup: tiles grouped by category, each with an icon in its colour, a name, one line and a tick or an Update pill.
2. Open Installed: drag one item onto a group heading, another before or after an item, and a group heading to reorder; do the same with the keyboard only.
3. Look at the lifecycle line of one installed item (update, remove).

**Expect:** It reads well; every drag works and every drag has a keyboard way; the lifecycle actions are offered for the kinds of item that have them.

**Report:** `F-SETUP-01: pass`, `F-SETUP-07: pass`, `F-APPSTORE-26: pass`, `F-APPSTORE-20: pass` (or `fail + what you saw`).

Rows: F-SETUP-01 (Store → Setup tab (replaces the Quick install page of the IYAKKI menu;); F-SETUP-07 (Store → Setup as a page of tiles (an app-store look)); F-APPSTORE-26 (Drag and drop in the Installed and Discover lists with a keyboard path); F-APPSTORE-20 (Lifecycle layer for everything installed (`store::lifecycle`, UI indep).

### 14. How the window feels: selecting, tooltips, scrolling, pointer speed

**Machine:** Mac mini 2018. **About 15 min.** **Moves 4 rows.**

1. Click into a text field and drag to select, double click a word, triple click a line, try Shift+arrows. Hover a cut-off row in Settings and in the Devices popover (F8) and keep still.
2. Scroll a long list with the wheel (and the trackpad if there is one).
3. Settings, Keyboard, Pointer speed: try 25, 300 and back to 100 percent.

**Expect:** The selection follows; a tooltip with the full text appears after a short delay; scrolling eases in and out with no overshoot; the pointer speed changes clearly both ways and returns at 100.

**Report:** `F-SHELL-11: pass`, `F-SHELL-48: pass`, `F-SHELL-50: pass`, `F-KBD-20: pass` (or `fail + what you saw`).

Rows: F-SHELL-11 (Selecting text in text fields with the mouse); F-SHELL-48 (Tooltips and kept addresses in cut device rows); F-SHELL-50 (Smooth scrolling); F-KBD-20 (Pointer speed (Settings -> Keyboard, row "Pointer speed"; asked for on).

### 15. PDF: Greek and Cyrillic, and the PDF buttons of LEARN and TALES

**Machine:** Mac mini 2018. **About 15 min.** **Moves 4 rows.**

1. In the chat ask the assistant to make a PDF with Greek and Cyrillic words and an arrow and a math symbol, then open it in the PDF viewer. In Settings, Models flip the native PDF output switch for one provider and ask for a PDF again.
2. In LEARN open a plan, a quiz and a lesson and press Export PDF (p on a lesson, k for the answer key of a quiz).
3. In TALES press d (Save PDF) on a story and open Stories, the title folder, story.pdf.

**Expect:** No letter is a box; the page and the chip read well and the tool used follows the switch; each export names its file and opens; the story file has the text and the first picture as cover.

**Report:** `F-PDF-11: pass`, `F-PDF-14: pass`, `F-STUDY-08: pass`, `F-TALES-22: pass` (or `fail + what you saw`).

Rows: F-PDF-11 (PDF writer); F-PDF-14 (Capability rule for `pdf_create`); F-STUDY-08 (LEARN tile **Export PDF** on a plan (open or chosen in the list), a qu); F-TALES-22 (TALES **Save PDF**).

### 16. Wi-Fi, network page and the hotspot

**Machine:** Lenovo (T7 stick). **About 20 min.** **Moves 4 rows.** ◇

1. Open Settings, Network: Ethernet and Wi-Fi rows with state, address and speed; join a Wi-Fi network.
2. Open the startup disk status line (once per boot, in the log or Settings) and read it.
3. Turn the hotspot on while sharing from a cable and join it from a phone.

**Expect:** The rows match the machine; the Wi-Fi joins; the startup disk line names the own disk first; the phone joins the hotspot and has internet.

**Report:** `F-NET-02: pass`, `F-NET-06: pass`, `F-NET-17: pass`, `F-HOTSPOT-19: pass` (or `fail + what you saw`).

Rows: F-NET-02 (WiFi configuration (wpa_supplicant planner)); F-NET-06 (Settings → Network); F-NET-17 (Startup disk status and own-disk-first); F-HOTSPOT-19 (Wi-Fi hotspot while sharing from a cable on a one-radio card).

### 17. A WireGuard VPN of your own

**Machine:** Mac mini 2018. **About 20 min.** **Moves 4 rows.**

1. Settings, VPN: import your own WireGuard .conf (needs a provider): it is listed and the key is not shown.
2. Connect it; set a mode (everything through it, or only chosen sites).
3. Open Settings, Firewall and tick the VPN's networks as the local network.

**Expect:** It connects and the status shows traffic; the mode behaves as named; the firewall treats the tunnel's networks as local when you say so.

**Report:** `F-VPN-01: pass`, `F-VPN-03: pass`, `F-VPN-07: pass`, `F-FW-13: pass` (or `fail + what you saw`).

Rows: F-VPN-01 (VPN core, WireGuard import (`iyakki-vpn::wgconf`, `profile`, `secret`;); F-VPN-03 (WireGuard generic-netlink messages (`iyakki-vpn::wgnl`, step 6 part)); F-VPN-07 (VPN modes per profile (`iyakki-vpn::profile::Settings`)); F-FW-13 (Firewall).

### 18. The CLI tile: tools, saved hosts and SSH

**Machine:** Mac mini 2018. **About 20 min.** **Moves 4 rows.**

1. Open the CLIs tab (administrator): with your Yes install kubectl, check it shows installed and remove it again.
2. Connections tab: add a host, edit, rename, Connect: a new host key is shown and asked about first; open an interactive session and type.
3. Ask the assistant to run a command on that host.

**Expect:** The download is checked and finishes; the host key question comes first and a changed key is refused; the terminal takes keys; a question appears every time for the assistant.

**Report:** `F-CLI-02: pass`, `F-CLI-06: pass`, `F-CLI-07: pass`, `F-CLI-08: pass` (or `fail + what you saw`).

Rows: F-CLI-02 (Install and manage command-line tools from the CLIs tab (administrator); F-CLI-06 (SSH safety for the assistant, the terminal and the tab); F-CLI-07 (Connections tab in the CLI tile); F-CLI-08 (Interactive SSH sessions in the CLI tile).

### 19. MacBook keyboard, brightness, speakers and browser sound

**Machine:** MacBookPro14,1. **About 20 min.** **Moves 4 rows.**

1. Use the keyboard and trackpad, then the brightness and keyboard-light keys.
2. Play a sound from Settings, Sound and then from a web page in the full browser.

**Expect:** Keys and trackpad work; brightness changes in steps; the speakers are at a sensible volume (not quiet) and browser sound comes out with the mixer switch on.

**Report:** `F-KBD-10: pass`, `F-KBD-03: pass`, `F-DRIVERS-27: pass`, `F-SND-15: pass` (or `fail + what you saw`).

Rows: F-KBD-10 (Apple SPI keyboard and trackpad (MacBook8,1 .); F-KBD-03 (Brightness and keyboard-light keys); F-DRIVERS-27 (Speaker output of the Macs with the Cirrus CS8409 codec and no T2 chip); F-SND-15 (Browser sound through the mixer, behind its own switch (Settings -> So).

### 20. GPUs for local models

**Machine:** Lenovo (T7 stick). **About 30 min.** **Moves 4 rows.**

1. Open Store, Setup, the card GPU for local models (Vulkan) and Open its page: the GPUs with names, driver, firmware and whether Vulkan works.
2. Run the card (a detached job), then the NVIDIA step if the card has the vendor driver.
3. If an AMD Lemonade server is on the network, Settings, Models, Detect.

**Expect:** Both the AMD and the NVIDIA GPU are found and a pod prints a Vulkan device; Lemonade is found and added.

**Report:** `F-GPU-01: pass`, `F-GPU-03: pass`, `F-GPU-05: pass`, `F-AI-09: pass` (or `fail + what you saw`).

Rows: F-GPU-01 (GPU for local models in k3s, stage 1 (any vendor, Vulkan)); F-GPU-03 (GPU for local models in k3s, stage 3, NVIDIA with the vendor driver); F-GPU-05 (GPU page and test prompt); F-AI-09 (AMD Lemonade Server as a provider).

### 21. Apple keyboard: the function keys, input profile and layout

**Machine:** Mac mini 2018. **About 15 min.** **Moves 3 rows.**

1. With an Apple keyboard open Settings, Keyboard: set Function keys mode to media keys first, then F-keys first, then automatic; press F10 or F11 each time.
2. Change the Input profile (Automatic, Mac, PC) and the ISO and Option/Command choices and type some text.
3. Unplug and plug the keyboard.

**Expect:** The key does the media action or the plain F-key as the mode says; the profile changes what the Command and Option keys do; the settings come back after unplugging.

**Report:** `F-KBD-06: pass`, `F-KBD-07: pass`, `F-KBD-19: pass` (or `fail + what you saw`).

Rows: F-KBD-06 (Settings → Keyboard (administrators)); F-KBD-07 (Apple keyboards); F-KBD-19 (Input profile (Settings -> Keyboard)).

### 22. Screen lock, screen off and the screen saver

**Machine:** Mac mini 2018. **About 15 min.** **Moves 3 rows.**

1. Lock the screen from the menu: a password box covers the screen while apps keep running; type the password and press Unlock.
2. Settings, Screen: turn the screen off by the setting, then press a key.
3. Settings, Screen saver: choose Starfield, then Slideshow (put pictures in Pictures first) and start the preview.

**Expect:** The lock asks for the password and gives the session back; the monitor goes dark and a key brings it back; stars, or photos with a cross-fade, appear.

**Report:** `F-LOCK-01: pass`, `F-SCRN-03: pass`, `F-SCRN-05: pass` (or `fail + what you saw`).

Rows: F-LOCK-01 (Lock screen); F-SCRN-03 (Screen off); F-SCRN-05 (Screen saver extras (the lock is F-LOCK-01 to F-LOCK-05)).

### 23. Long jobs in the chat and the IDE

**Machine:** Mac mini 2018. **About 15 min.** **Moves 3 rows.**

1. From the chat or the IDE start a long job (for example an install) and leave the chat.
2. Come back: look for the row with spinner, label, elapsed time, a bar, the last line, Log and Stop.
3. Let it finish and look for the note the assistant gets (it tells you once).

**Expect:** The job went on and its log is readable; the row has all its parts and Dismiss appears at the end; the assistant is told once and an Autopilot step that ends with a job running waits instead of failing.

**Report:** `F-JOBS-01: pass`, `F-JOBS-03: pass`, `F-JOBS-05: pass` (or `fail + what you saw`).

Rows: F-JOBS-01 (Detached jobs engine (`docs/JOBS.md`)); F-JOBS-03 (The assistant is told once when a job finished (a note before its next); F-JOBS-05 (Jobs strip in the chat and the AI IDE's chat pane).

### 24. Local models and what the chat does with their answers

**Machine:** Any machine. **About 15 min.** **Moves 3 rows.** ◇

1. Settings, Models: add an OpenAI-style server (for example Ollama) or a hosted one with your key; open the model list (L) and the per-model request options.
2. Chat with a model that thinks aloud (a reasoning model) and read the answer; look at the token counts the chat shows after a reply.

**Expect:** The list and the options work; the thinking text is never shown as the answer; the token counts are the server's own numbers, not guesses.

**Report:** `F-AI-08: pass`, `F-AI-13: pass`, `F-AI-14: pass` (or `fail + what you saw`).

Rows: F-AI-08 (Exact token counts from providers); F-AI-13 (Provider model list and per-model request options); F-AI-14 (Reasoning replies are never the answer).

### 25. Dashboards, data and tables in the chat

**Machine:** Mac mini 2018. **About 15 min.** **Moves 3 rows.**

1. Open the DASH tile: hover a chart, click a legend entry, drag across a chart to zoom; add and move a panel (they never overlap).
2. In the chat ask the assistant to open a CSV file and run a query on it.

**Expect:** A crosshair with a tooltip, series toggles and zoom; panels stay on the grid; the answer is drawn as a table with a rule under the header and an Open in Data viewer button.

**Report:** `F-DASH-05: pass`, `F-DASH-06: pass`, `F-DATA-08: pass` (or `fail + what you saw`).

Rows: F-DASH-05 (Dashboard model and files); F-DASH-06 (DASH tile (Work group)); F-DATA-08 (The chat draws the answer of a table tool (`data_open`, `data_query`, ).

### 26. The browser on a real article

**Machine:** Mac mini 2018. **About 15 min.** **Moves 3 rows.**

1. Open the BROWSER tile on a real article, follow an in-page # link, use Find, back, forward and reload.
2. Use Ctrl with + , - and 0 and Ctrl with the mouse wheel, and press Y for text only zoom.
3. Use Tab and Enter to move through a page and a form without the mouse.

**Expect:** Jumps land on the anchor and matches are lit; the letters change size and the size is remembered for the site; every link, button and field can be reached and used from the keyboard.

**Report:** `F-BROW-01: pass`, `F-BROW-97: pass`, `F-BROW-38: pass` (or `fail + what you saw`).

Rows: F-BROW-01 (BROWSER tile, first cut); F-BROW-97 (Text-only zoom (Y); F-BROW-38 (Accessibility and input in the built-in reader engine).

### 27. The browser's start page, forms and look

**Machine:** Mac mini 2018. **About 15 min.** **Moves 3 rows.**

1. Visit one https site twice and open the start page: a small picture of that site is among the most visited.
2. Type profile set in the address bar and save a name, e-mail, phone and address; on a sign-up page click a field and press Ctrl+Y.
3. Look at the tab strip and groups: open, group and close tabs.

**Expect:** The picture appears; matching fields fill and the others stay empty; the tabs and groups look and work as the page says.

**Report:** `F-BROX-02: pass`, `F-BROW-96: pass`, `F-BROW-76: pass` (or `fail + what you saw`).

Rows: F-BROX-02 (Small pictures of the sites visited most on the browser's start page); F-BROW-96 (Browser form helpers beyond passwords); F-BROW-76 (Browser tile look and feel (`browser_chrome.rs`, `browser_groups.rs`)).

### 28. Touch screen

**Machine:** Any machine. **About 15 min.** **Moves 3 rows.**

1. On a touch monitor tap with a finger; on a multi-display set-up touch each screen.
2. Touch a text field.

**Expect:** The pointer follows the first finger and lands under it on the right display; the on-screen keyboard opens for a field (a mouse click opens nothing and a physical key closes it).

**Report:** `F-BOOT-06: pass`, `F-DISP-07: pass`, `F-KBD-17: pass` (or `fail + what you saw`).

Rows: F-BOOT-06 (Touch screens); F-DISP-07 (Touch screens on several displays (`touchmap.rs`, `touchscreen.rs`)); F-KBD-17 (On-screen keyboard for touch screens (`osk.rs`, `shell_osk.rs`)).

### 29. Remote files: the SFTP server and a shared folder

**Machine:** Mac mini 2018. **About 20 min.** **Moves 3 rows.**

1. Settings, Remote access, SFTP: start the server (F); from another computer connect with sftp using a listed key; stop it.
2. Share a folder (docs/SHARED-FOLDERS.md) and open its address from a phone on the same network.

**Expect:** Port, addresses and key fingerprints are shown; sftp works with the listed key and not without; the phone shows a clean list (folders first, sizes, dates), downloads and resumes a large file.

**Report:** `F-FILE-05: pass`, `F-FILE-06: pass`, `F-SHARE-01: pass` (or `fail + what you saw`).

Rows: F-FILE-05 (SSH 2 server transport for the SFTP server (in-house, `ssh_server.rs`)); F-FILE-06 (SFTP server in Settings, Remote access); F-SHARE-01 (Shared folder server (in-house HTTP/1.1, `sharedsite.rs`, one port per).

### 30. The PDF viewer

**Machine:** Mac mini 2018. **About 10 min.** **Moves 2 rows.**

1. Open the PDF tile on a file in Downloads: next, previous, zoom, fit width, search (watch the N of M count), copy page text.
2. In the chat ask the assistant to open that PDF at page 3.

**Expect:** All the controls work; the assistant asks and then the viewer opens on that page.

**Report:** `F-PDF-05: pass`, `F-PDF-12: pass` (or `fail + what you saw`).

Rows: F-PDF-05 (PDF viewer (`PdfView`, hosted by `PdfApp`)); F-PDF-12 (Chat tool `pdf_open` (asks, shows the file and page on the card)).

### 31. Saved files and streaming downloads in the browser

**Machine:** Mac mini 2018. **About 10 min.** **Moves 2 rows.**

1. Download a large file in the browser: look for the progress bar and sizes; press Esc halfway.
2. Download a text file, a CSV and a music file and open each from the Downloads list.

**Expect:** Esc stops it and the part file is deleted; text opens in EDIT, CSV in DATA, music in MEDIA.

**Report:** `F-BROW-94: pass`, `F-BROX-01: pass` (or `fail + what you saw`).

Rows: F-BROW-94 (Downloads that stream); F-BROX-01 (Open a saved file from the Downloads list in the tile that suits it).

### 32. Version, and files that survive power-off

**Machine:** Mac mini 2018. **About 10 min.** **Moves 2 rows.**

1. Open About and read the boot line: version, build date, git describe, variant and kernel.
2. Make a note in your home, sign out, power the machine off and on, sign in.

**Expect:** The version is shown the same everywhere; the file is still there.

**Report:** `F-REL-01: pass`, `F-STORE-09: pass` (or `fail + what you saw`).

Rows: F-REL-01 (Version everywhere); F-STORE-09 (People's files survive power-off).

### 33. Sample media and the in-house player

**Machine:** Mac mini 2018. **About 10 min.** **Moves 2 rows.**

1. Open MEDIA, the folder IYAKKI samples; play the MP3 melody, the kuthu beat and the 12 second WebM video.
2. On an image with no window system play a video with sound.

**Expect:** Sound is heard and the video plays inside the page; picture and sound start together and pause and resume work.

**Report:** `F-MEDIA-07: pass`, `F-MEDIA-08: pass` (or `fail + what you saw`).

Rows: F-MEDIA-07 (Sample media in every image and video on the web image); F-MEDIA-08 (In-house video and sound player for images with no window system).

### 34. The AI IDE feel: chat box and docked preview

**Machine:** Mac mini 2018. **About 10 min.** **Moves 2 rows.**

1. Open the AI IDE, click the chat box, type, press Send and Enter; change the screen size and repeat.
2. Press Run on a small app: the preview opens beside the editor.

**Expect:** The chat box takes focus and does not close the app; the preview is docked beside the editor.

**Report:** `F-IDE-18: pass`, `F-IDE-25: pass` (or `fail + what you saw`).

Rows: F-IDE-18 (AI IDE chat input, Send and the Model/Agent/Approvals row lie inside t); F-IDE-25 (Docked Preview tab in the AI IDE).

### 35. File pickers

**Machine:** Mac mini 2018. **About 10 min.** **Moves 2 rows.**

1. Open the Git private key box, the IDE deploy form and the project settings and press Browse (or Ctrl+O).
2. Do the same on the IYAKKI menu video path, the Linux page folder and the store-server page.

**Expect:** The picker opens inside the window, Esc closes it, the chosen file fills the right box.

**Report:** `F-SHELL-60: pass`, `F-SHELL-61: pass` (or `fail + what you saw`).

Rows: F-SHELL-60 (File picker (Browse, Ctrl+O, the live chip) for typed-path fields that); F-SHELL-61 (Browse (a file picker over the screen, Ctrl+O on the store-server page).

### 36. QR codes and Claude's sign-in code

**Machine:** Any machine. **About 10 min.** **Moves 2 rows.**

1. Make QR codes of level L and H and a Wi-Fi code (WPA and hidden) and scan with a phone.
2. Start Claude's sign-in in the CLI tile and bring its long code over with the QR instead of typing.

**Expect:** The phone offers to join or open; the long sign-in code arrives without typing.

**Report:** `F-EXTA-15: pass`, `F-EXTA-09: pass` (or `fail + what you saw`).

Rows: F-EXTA-15 (QR encoder extended (`iyakki-agent::qr`)); F-EXTA-09 (Getting Claude's long sign-in code in without typing (`docs/EXTERNAL-A).

### 37. Chat extras: screen capture and the built-in model chip

**Machine:** Mac mini 2018. **About 10 min.** **Moves 2 rows.**

1. In the chat ask the assistant to take a picture of the screen: a question with Allow for 5 minutes appears; allow once.
2. Pick an external agent in the chat and click the Built-in model chip beside Model.

**Expect:** The picture shows in the chat; the chat returns to the built-in model.

**Report:** `F-ASSIST-12: pass`, `F-EXTA-17: pass` (or `fail + what you saw`).

Rows: F-ASSIST-12 (`screen_capture`); F-EXTA-17 (A one-click chip "Built-in model" ("Built-in" in a narrow pane, left o).

### 38. The full web engine: fullscreen and blocking

**Machine:** Mac mini 2018. **About 15 min.** **Moves 2 rows.**

1. Open the full web engine, play a YouTube video and press F or the fullscreen button.
2. Switch the ads and trackers blocker on with a list downloaded and open a page with ads.

**Expect:** The video goes fullscreen; the ads are not drawn.

**Report:** `F-BROW-60: pass`, `F-BROW-55: pass` (or `fail + what you saw`).

Rows: F-BROW-60 (Page fullscreen in the full web engine (YouTube's button, F, double cl); F-BROW-55 (Blocking in the full web engine (WPE WebKit)).

### 39. TALES pictures and the cartoon composer

**Machine:** Mac mini 2018. **About 15 min.** **Moves 2 rows.**

1. Open TALES and make a story with the built-in cartoon composer (a small model picks scenes).
2. Make another with an external agent as the picture writer, if one is set up.

**Expect:** Pictures are drawn by the writer you chose and the composer's scenes are shown.

**Report:** `F-TALES-15: pass`, `F-TALES-16: pass` (or `fail + what you saw`).

Rows: F-TALES-15 (TALES pictures are drawn by the writer the person chose); F-TALES-16 (Built-in cartoon composer for small models).

### 40. The web client on a phone

**Machine:** Any machine. **About 20 min.** **Moves 2 rows.**

1. Open the web client on a phone over a slow connection; try Low bandwidth and Very low by hand.
2. Tap, long-press (right click), drag, two-finger scroll, pinch zoom, press the keyboard button, go fullscreen.

**Expect:** Quality drops by itself and the picture still updates; every gesture works.

**Report:** `F-WEB-11: pass`, `F-WEB-13: pass` (or `fail + what you saw`).

Rows: F-WEB-11 (Remote display, adaptive quality); F-WEB-13 (Web client).

## Which check for which machine

- **Mac mini 2018**: checks 1, 3, 5, 7, 8, 9, 12, 13, 14, 15, 17, 18, 21, 22, 23, 25, 26, 27, 29, 30, 31, 32, 33, 34, 35, 37, 38, 39.
- **MacBookPro14,1**: checks 4, 10, 19.
- **Lenovo (T7 stick)**: checks 6, 16, 20.
- **Any machine**: checks 2, 11, 24, 28, 36, 40.

### More checks (after the top 40)

- **Platform services and the cluster page** (Mac mini 2018, about 10 min, 1 row): 1. Open Settings, Dev cluster, Platform services. 2. Check one service's live state. Expect: A list with live state that asks the cluster read-only, without errors. Report: `F-K8S-18`.
- **Matrix mirror in the inbox** (Any machine, about 10 min, 1 row): 1. With a Matrix room joined send yourself a message from another client. 2. Open the INBOX. Expect: The message appears in the inbox and the assistant can read it. Report: `F-MSG-07`.
- **Wrong display modes and the 4K link** (Mac mini 2018, about 15 min, 1 row): 1. Plug in a display whose mode list is wrong (a 4K monitor on a USB-C cable that only offers 1920x1080) and open Settings, Display. 2. Look at the list after a few seconds without touching it. Expect: The list is read again by itself and the 4K size appears. Report: `F-DISP-13`.

## New today: look at these too

These are the things built on 2026-11-13. Most of their rows are still partial for reasons a look cannot fix (the Impl column is open), so a pass does not make them done; it still tells us whether they work. Report them in the same words (`pass` or `fail + what you saw`); an id marked *(Impl N: stays partial)* is reported the same way.

### N1. Live scripts in the browser: buttons, typing and the window size

**Machine:** Mac mini 2018. **About 15 min.**

1. On your PC save the small page below as live.html and serve it: python3 -m http.server 8765 --bind 0.0.0.0 in that folder. In the BROWSER tile open http://<the PC's address>:8765/live.html (scripts must be on for the site: press j).
2. Click Add three times, tick the box, type a name in the field.
3. Drag the window narrower and wider, or turn the screen.

The page for step 1:

```html
<!doctype html><meta charset=utf-8><title>Live page test</title>
<p id=n>0</p><button type=button id=b>Add</button>
<p><input type=checkbox id=c> <span id=s>off</span></p>
<p><input type=text id=t placeholder="Your name"> <span id=h>nobody</span></p>
<style>#narrow{display:none} @media (max-width:700px){#wide{display:none}#narrow{display:block}}</style>
<p id=wide>wide window</p><p id=narrow>narrow window</p>
<script>
let n=0;
document.getElementById('b').addEventListener('click',()=>{n++;document.getElementById('n').textContent=String(n)});
document.getElementById('c').addEventListener('change',e=>{document.getElementById('s').textContent=e.target.checked?'on':'off'});
document.getElementById('t').addEventListener('input',e=>{document.getElementById('h').textContent='Hello '+e.target.value});
</script>
```

**Expect:** The number goes 1, 2, 3 without the page reloading; the line under the box says on or off; the greeting follows your typing and what you typed stays in the field with the cursor in it; the wide or narrow paragraph changes with the window size after a moment. A button inside a form, a link with a handler and a password field are NOT expected to reach the page's scripts yet.

**Report:** `F-BJS-01` *(Impl 55: stays partial)*, `F-BROW-28` *(Impl 96: stays partial)*.

### N2. HTTP/2 and the page speed numbers

**Machine:** Any machine. **About 10 min.**

1. On the Linux PC run: cargo test -p iyakki-proxy --lib live_h2 -- --ignored --nocapture (needs the internet).
2. On the MacBookPro14,1 run: sh dist/scripts/browser-speed-report.sh and send the file it writes (speed-report-<name>-<date>.txt).

**Expect:** Google, Cloudflare and Wikipedia answer over HTTP/2 on the PC too (a second machine; it was run on the Mac only); the speed table has numbers for the MacBook beside the Mac's in docs/BROWSER-SPEED.md. HTTP/2 itself cannot be seen on screen.

**Report:** `F-BROX-10` *(Impl 85: stays partial)*, `F-BROX-15` *(Impl 25: stays partial)*.

### N3. The known-bad site list and the certificate view

**Machine:** Any machine. **About 10 min.**

1. In the chat ask the assistant to add evil.example to the known-bad site list with the reason test, and approve the card.
2. Open evil.example in the BROWSER tile and then a normal site. Then ask the assistant to show the certificate of www.wikipedia.org.
3. Ask the assistant to remove evil.example again.

**Expect:** The page for evil.example is an explanation with your reason and nothing was sent; the normal site opens; the certificate answer gives who it was issued to and by, the dates, the fingerprint and says it is trusted; the removal opens the address again.

**Report:** `F-BROW-98` *(Impl 80: stays partial)*.

### N4. PDF passwords and PDF forms

**Machine:** Mac mini 2018. **About 20 min.**

1. Make a protected PDF: open any PDF in Preview, File, Export, tick Encrypt, password test; copy it to Downloads. Open it in the PDF tile: give a wrong password, then the right one.
2. Open any fillable PDF (a government form) in the PDF tile, press Form, click a text row, type, press Enter, tick a check box, press Save copy (n).
3. Open the new name (filled).pdf in Preview and look at the values; in the chat ask the assistant to fill a field with pdf_form_fill.

**Expect:** The wrong password is said to be wrong and nothing is shown; the right one opens the file; the filled copy is next to the original with your values and the original is unchanged; the assistant asks first and writes a new file.

**Report:** `F-BROX-07` *(Impl 80: stays partial)*, `F-BROX-08` *(Impl 70: stays partial)*.

### N5. The web UI tiles: Runs, Chat, Edit, Settings

**Machine:** Any machine. **About 20 min.**

1. Sign in to the web UI from another computer and open the Tiles page.
2. Open Chat: send a message, choose a model, press Stop on a long answer. Open Runs: your run is listed and opens. Open Edit: type, Open and Save a file in your folder. Open Settings: read the overview.
3. In Chat ask for something that needs approval: the question appears as a line only and is answered in the approvals list.

**Expect:** Each tile draws and responds; the Runs list reads again by itself; the editor keeps its text and saves; the overview shows no keys or passwords; a tile never answers an approval itself.

**Report:** `F-WEB-53` *(Impl 50: stays partial)*, `F-WEB-54` *(Impl 40: stays partial)*, `F-WEB-55` *(Impl 60: stays partial)*, `F-WEB-56` *(Impl 30: stays partial)*, `F-WEB-52` *(Impl 90: stays partial)*, `F-WEB-04`.

### N6. Themes: the three built-in ones and a theme file of your own

**Machine:** Mac mini 2018. **About 15 min.**

1. Change the theme to Light, then High contrast, then Dark (Settings, Display) and look at BROWSER, MEDIA and the chat in each.
2. Make a file /var/lib/iyakki/themes/<your name>/midnight.theme (with iyakki dev shell) holding the lines: name = Midnight blue, base = dark, bg_top = #0b1020, text = #f2f4ff. Ask the assistant for display_themes, choose it with display_theme_set, and sign out and in.
3. Make a second file whose text colour is nearly the same as its background and ask for display_themes again.

**Expect:** Every app is readable in each built-in theme; the new theme is listed and applies after you sign in again; the unreadable one is listed with the reason and never becomes active.

**Report:** `F-SHELL-41`, `F-SHELL-62` *(Impl 70: stays partial)*.

### N7. Browser data protection

**Machine:** Mac mini 2018. **About 20 min.**

1. Settings, Browser, press P to turn Protect browser data on. Open the full browser, sign in to a test site, close the browser and sign out.
2. Look in the web profile folder (iyakki dev shell): the cookies and storage are in one sealed file, not in plain files. Sign in again and open the site.
3. Turn it off again and sign out and in.

**Expect:** You are still signed in to the site after signing in to IYAKKI again; no plain cookie or storage files are left; turning it off puts everything back. Note anything slow and say how long the sign-in took.

**Report:** `F-VAULT-06`, `F-VAULT-07` *(Impl 90: stays partial)*, `F-VAULT-10` *(Impl 90: stays partial)*.

### N8. Google Gemini as the chat and speech provider (experimental)

**Machine:** Any machine. **About 20 min.**

1. Make a Google AI Studio key (aistudio.google.com, Get API key; the free tier needs no card). Keep it in a file outside the repo with mode 600, for example: printf '%s' 'YOUR-KEY' > ~/gemini.key && chmod 600 ~/gemini.key. On the PC you can then run: IYAKKI_LIVE_GEMINI_KEY_FILE=$HOME/gemini.key cargo test -p iyakki-agent --lib live_gemini -- --ignored --nocapture.
2. On the device open Settings, Models and press g (administrators): paste the key from that file in the first field, choose the tier, choose Gemini for speech and for the assistant, and answer the consent card (read it first: it says what is sent).
3. Record a short sentence in the VOICE tile and read the text; ask the assistant a question with a tool (for example list a folder). Then ask for gemini_status and press C to take your consent back.

**Expect:** The test request answers OK; the sentence is written out correctly; the assistant answers and its tool calls work or fail in words; the status never shows the key; after you take the consent back nothing goes to Google. Write down what Google's endpoint refused, if anything.

**Report:** `F-ASSIST-52` *(Impl 70: stays partial)*, `F-VOICE-41` *(Impl 70: stays partial)*.

### N9. My IYAKKI and the echo canceller

**Machine:** Any machine. **About 5 min.**

1. Nothing to try tonight: the canceller is written and tested with made-up signals but not connected to the assistant (it has no copy of what the speaker plays). Do not report this row.

**Expect:** -

**Report:** `F-VOICE-36` *(Impl 60: stays partial)*.

### N10. A media server and its listing

**Machine:** Any machine. **About 15 min.**

1. Settings, Shared folders: share a folder with a few songs and switch the media server on (it asks first). On a phone with a DLNA player (VLC) open the list of media servers and browse it.
2. Play one song.

**Expect:** The server is listed; the songs show with their title, artist and length; they play.

**Report:** `F-MEDIA-02` *(Impl 97: stays partial)*.

## Every other row that needs a person (Impl 100), by machine

These rows are not in a check above. Version 1's try-and-expect line is copied where there was one; a row without one needs its steps from the author (read its Notes; report `skipped` if unclear).

### Mac mini 2018 (5 rows)

- **F-HOTSPOT-03** (100/50) EAPOL-Key frames. Try: With the hotspot feature on, join its Wi-Fi from a phone. Expect: the phone connects (this is the key exchange). Report: `F-HOTSPOT-03: pass` or `F-HOTSPOT-03: fail + what you saw`.
- **F-IDE-01** (100/30) AI IDE tile. Try: Open the AI IDE. Create a workspace, create, rename and delete a file in the tree, run a command in the terminal and the run/test commands. Expect: each works inside the workspace only. Report: `F-IDE-01: pass` or `F-IDE-01: fail + what you saw`.
- **F-NET-11** (100/30) Bluetooth of the Mac mini 2018. Try: Open Store, Setup, Hardware, the Mac drivers page; install the Bluetooth patch file (internet needed, owner says Yes first). Restart. Open Settings, Bluetooth. Expect: the page says whether the controller got an address. Report: `F-NET-11: pass` or `F-NET-11: fail + what you saw`.
- **F-SETUP-10** (100/50) Setup item **Voice. Try: In Store, Setup choose Voice: Whisper tiny + Kokoro. Read the review (downloads, licences). Only if you say Yes: install. Expect: both models appear and read-aloud works. Report: `F-SETUP-10: pass` or `F-SETUP-10: fail + what you saw`.
- **F-VOICE-23** (100/50) VOICE tile. Try: Open VOICE, RECORD tab (key F4) with a microphone attached. Press Record, speak, press Stop. Expect: the level meter moves and a recording is saved. Report: `F-VOICE-23: pass` or `F-VOICE-23: fail + what you saw`.

### MacBookPro14,1 (1 rows)

- **F-NET-22** (100/30) Bluetooth low-energy keyboards. Try: Put a low-energy Bluetooth keyboard or mouse in pairing mode and pair it in Settings, Bluetooth (docs/BLUETOOTH-INPUT.md). Type or move. Restart; Expect: it reconnects. Report: `F-NET-22: pass` or `F-NET-22: fail + what you saw`.

### Lenovo (T7 stick) (3 rows)

- **F-AIHW-01** (100/30) AI hardware profile. Try: Open the chat, IDE or TALES model picker. Expect: a fits line says what a model can use on this AMD + NVIDIA computer. Report: `F-AIHW-01: pass` or `F-AIHW-01: fail + what you saw`.
- **F-CLI-04** (100/30) Ollama in the CLIs tab. Try: In the CLIs tab, with your Yes for a download of over 1 GB, install Ollama, start it, see it in Models, stop it. Expect: it names the graphics card use or CPU. Report: `F-CLI-04: pass` or `F-CLI-04: fail + what you saw`.
- **F-GPU-04** (100/70) PC image. Try: Boot the PC image (variant pc). Sign in. Expect: it boots, the GPU firmware loads, About says pc-x86_64. Report: `F-GPU-04: pass` or `F-GPU-04: fail + what you saw`.

### Any machine (109 rows)

- **F-AI-12** (100/30) NVIDIA NIM as a model provider with a guided setup. Steps from the author needed.
- **F-AI-16** (100/50) Waiting that actually waits. Steps from the author needed.
- **F-APPS-50** (100/50) Private store repair, reset and remove. Steps from the author needed.
- **F-APPS-51** (100/30) Publishing into a private store that was deployed here but never added. Try: Deploy a private store with the Platform wizard, then open Publish in the IDE. Expect: Private store on this device (not added yet) is listed first and is the default; publish a tile and add the store. Report: `F-APPS-51: pass` or `F-APPS-51: fail + what you saw`.
- **F-APPSTORE-19** (100/50) Add a store from a portal link. Steps from the author needed.
- **F-ASSIST-10** (100/70) Assistant robustness. Steps from the author needed.
- **F-ASSIST-14** (100/70) Stall guard and actionable file errors. Steps from the author needed.
- **F-BOOT-29** (100/70) Lean boot. Steps from the author needed.
- **F-BOOT-31** (100/50) Experimental. Steps from the author needed.
- **F-BOOT-41** (100/70) Push mode (`dist/buildroot/quick-push.sh`). Steps from the author needed.
- **F-BROX-04** (100/50) Animated WebP in the browser. Steps from the author needed.
- **F-BROX-06** (100/50) Inline `<svg>` in a page. Steps from the author needed.
- **F-BROX-11** (100/50) Browser speed. Steps from the author needed.
- **F-BROX-12** (100/50) Browser speed. Steps from the author needed.
- **F-CLIENT-08** (100/30) What a client device may ask for, in Settings and by chat, EXPERIMENTAL. Steps from the author needed.
- **F-CLIENT-09** (100/30) The window program carries the media half and tells the truth about the key, and the Mac says Cmd+V, EXPERIMEN. Steps from the author needed.
- **F-CLOUD-06** (100/30) Launch cloud mode button in the K8S tile (`k8s_cloud.rs`, `cloudlaunch.rs`). Steps from the author needed.
- **F-CLOUD-07** (100/50) Tenant and namespace roles (`cloudroles.rs`, `iyakki cloud roles`). Steps from the author needed.
- **F-CLOUD-08** (100/30) Storage for a cloud tenant (`cloudstorage.rs`). Steps from the author needed.
- **F-CLOUD-10** (100/50) The cluster's own vault service (`cloudvault.rs`). Steps from the author needed.
- **F-CLOUD-12** (100/50) Upgrade (WebSocket) through the local listener. Steps from the author needed.
- **F-CLOUD-14** (100/50) TLS passthrough by SNI (own ClientHello reader), plain-HTTP to HTTPS 308 redirect, and a port gate (`PortGate`. Steps from the author needed.
- **F-CLOUD-16** (100/50) The vault service (`cloudvaultd.rs`, `iyakki vault serve`, `vault keygen`, `vault enroll`). Steps from the author needed.
- **F-CLOUD-17** (100/50) Vault authorisation and rotation. Steps from the author needed.
- **F-CLOUD-19** (100/30) Cloud control plane (`cloudcontrol.rs`, `iyakki cloud reconcile`). Steps from the author needed.
- **F-CLOUD-21** (100/30) Desktop connector client (`cloudclient.rs`, `pixelcodec.rs`). Steps from the author needed.
- **F-CLOUD-24** (100/30) File and clone routes of the cloud profile (`webservice/files.rs`, `cloudfiles.rs`). Steps from the author needed.
- **F-CLOUD-25** (100/50) Snapshots and backups of a tenant volume (`cloudsnapshot.rs`, `iyakki cloud snapshot create, list, verify, res. Steps from the author needed.
- **F-CLOUD-31** (100/50) Automatic key rotation when a role is revoked (`cloudrotate.rs`, `Access::revoke`, web routes `tenant/members`. Steps from the author needed.
- **F-CLOUD-41** (100/50) Mutual TLS on the local listener (`cloudlisten_mtls.rs`, `Options::client_ca`). Steps from the author needed.
- **F-CLOUD-43** (100/50) The firewall behind `PortGate` (`iyakki_fw::gaterule`, `cloudlisten_gate.rs`). Steps from the author needed.
- **F-CLOUD-44** (100/30) Replay protection and rate limits of the tenant vault service (`cloudvaultguard.rs`, `cloudvaultd.rs`, `cloudk. Steps from the author needed.
- **F-CLOUD-45** (100/30) Rotation requested from a pod (`cloudrotate.rs`, `cloudvaultd.rs`). Steps from the author needed.
- **F-CLOUD-47** (100/50) Binary files in the files API (`PUT /api/v1/files/upload`, `GET /api/v1/files/download`). Steps from the author needed.
- **F-CLOUD-48** (100/50) Per-namespace volumes for a tenant (`cloudstorage.rs`, `isolation. Steps from the author needed.
- **F-CLOUD-49** (100/70) `iyakki cloud listen` uses the finished parts by flags (`cloudlisten_setup.rs`). Steps from the author needed.
- **F-CLOUD-50** (100/70) Cloud container image and one build script (`dist/cloud/build-cloud.sh docker`; docs/CLOUD-IMAGES.md). Steps from the author needed.
- **F-CLOUD-51** (100/50) Config drive for a cloud node (`cloudseed.rs`, like cloud-init NoCloud). Steps from the author needed.
- **F-CLOUD-52** (100/50) First-run adopt mode of a headless cloud node (`cloudadopt.rs`, `iyakki-remote::adopt_server`, `iyakki-core::c. Steps from the author needed.
- **F-CLOUD-53** (100/30) Guarded self-install of a cloud node to its internal disk. Steps from the author needed.
- **F-CLOUD-70** (100/50) Tenant scheduler service (`cloudflows/service.rs`, `lease.rs`, `real.rs`). Steps from the author needed.
- **F-CLOUD-71** (100/30) Quotas, time zone and engine gate for cloud flows. Steps from the author needed.
- **F-CLOUD-73** (100/30) A flow `approval` node as a tenant approval (`cloudflows/approval.rs`). Steps from the author needed.
- **F-CLOUD-74** (100/50) Notifications from flows and the scheduler (`cloudflows/notify.rs`). Steps from the author needed.
- **F-CLOUD-75** (100/50) Where a cloud run executes (`cloudflows/executor.rs`, `job.rs`). Steps from the author needed.
- **F-CORE-10** (100/50) Core contract over developer mode's TLS channel (`devmode::contract`, `/v1/dev/core/...`). Steps from the author needed.
- **F-DASH-11** (100/50) Cluster addresses in DASH sources. Steps from the author needed.
- **F-DEVT-05** (100/30) DEV TOOLS QR code tool. Try: Open DEV TOOLS, the QR tool. Make a Wi-Fi code, a URL and a contact; scan each with a phone camera. Expect: the phone reads each one. Report: `F-DEVT-05: pass` or `F-DEVT-05: fail + what you saw`.
- **F-DRIVERS-18** (100/70) T2 MacBooks. Try: On the T2 MacBook boot the Mac image. Expect: the internal keyboard, trackpad and keyboard backlight work. Report: `F-DRIVERS-18: pass` or `F-DRIVERS-18: fail + what you saw`.
- **F-DRIVERS-21** (100/30) Model table for Apple's recovery download (`iyakki-drivers::macboard`). Steps from the author needed.
- **F-DRIVERS-40** (100/30) Intel HEX to Broadcom `.hcd` converter (`iyakki-drivers::ihex`). Steps from the author needed.
- **F-DRIVERS-52** (100/30) Hardware firmware, getting the files onto a machine whose Wi-Fi has no firmware (`fwextra_net.rs`, `fwextra_cl. Steps from the author needed.
- **F-FLOW-06** (100/70) Agent-monitored healing. Steps from the author needed.
- **F-FLOW-14** (100/50) Thrift RPC node over HTTP with the JSON protocol, written in-house (`iyakki-flow::thrift`, no Thrift library o. Steps from the author needed.
- **F-FLOW-27** (100/70) The assistant knows the open flow and its failure, on every path the person can reach the chat from FLOWS (the. Steps from the author needed.
- **F-FLOW-29** (100/70) Camera node `cam_photo` (output path and taken_at; optional device and keep). Steps from the author needed.
- **F-FLOW-34** (100/70) Autopilot for fixes. Steps from the author needed.
- **F-FLOW-35** (100/50) Upgrade class, detection and suggestion only. Steps from the author needed.
- **F-FLOW-39** (100/50) FLOWS canvas nodes. Steps from the author needed.
- **F-FW-14** (100/70) Firewall. Steps from the author needed.
- **F-GIT-19** (100/70) Chat tools `git_clone_repo` (url, branch, depth, `submodules`, connection name) and `git_clones_list`. Steps from the author needed.
- **F-HOTSPOT-05** (100/70) DHCP server (`dhcp`). Steps from the author needed.
- **F-IDE-09** (100/30) Browser tab in the AI IDE. Steps from the author needed.
- **F-IDE-19** (100/70) Assistant skills for build and deploy to k3s. Steps from the author needed.
- **F-INST-03** (100/70) GPT disk install. Steps from the author needed.
- **F-KBD-11** (100/30) T2 MacBook keyboard. Try: Test the trackpad: tap, two-finger scroll, gestures, and resting a palm while typing. Expect: each works; say which does not. Report: `F-KBD-11: pass` or `F-KBD-11: fail + what you saw`.
- **F-KBD-12** (100/30) Touch Bar display driver in user space. Try: Open Settings, Keyboard (docs/TOUCH-BAR.md). Expect: the Touch Bar shows an image. If it is upside down or mirrored, use the two flips. Report: `F-KBD-12: pass` or `F-KBD-12: fail + what you saw`.
- **F-LINAPP-09** (100/30) Image apps. Steps from the author needed.
- **F-NET-05** (100/30) Bluetooth userland (the umbrella over scanning and pairing, HID, tethering and the A2DP sink, which are the ro. Steps from the author needed.
- **F-NET-09** (100/70) Private MAC address (Off / Per network / Random each connection) per Ethernet and Wi-Fi, like macOS Private Wi. Steps from the author needed.
- **F-PKG-14** (100/30) Run on other base operating systems. Steps from the author needed.
- **F-PKG-15** (100/70) Incremental image builds. Steps from the author needed.
- **F-PWR-01** (100/30) Power settings (Settings -> System -> Power). Steps from the author needed.
- **F-QA-10** (100/50) Guard against programs the image does not ship. Steps from the author needed.
- **F-QA-12** (100/50) Fetching the signed downloads manifest (`iyakki-agent::dlmanifest::fetch`, docs/development/DOWNLOADS-MANIFEST. Steps from the author needed.
- **F-QA-14** (100/50) Our own SHA-256 pins in the downloads manifest (`iyakki-agent::dlmanifest::pins`, `dlmanifest pin`). Steps from the author needed.
- **F-QA-15** (100/50) Overflow detector for the shell (`monkey_tests::overflow_every_tile_at_the_largest_text_size`, KI-015). Steps from the author needed.
- **F-REL-05** (100/70) `./run.sh release [version]`. Steps from the author needed.
- **F-REL-08** (100/30) Firmware licence audit (`docs/FIRMWARE-LICENCES.md`). Steps from the author needed.
- **F-REL-10** (100/30) Licence gate fixes for the images. Steps from the author needed.
- **F-SCAN-01** (100/30) Local network scanner (`iyakki-netscan`). Steps from the author needed.
- **F-SDK-01** (100/50) SDK developer experience. Steps from the author needed.
- **F-SDK-02** (100/30) SDK emulator. Try: On the developer computer run the SDK emulator window on a small WASM tile (`iyakki-sdk-emulator emulate TILE`). Expect: it opens, keys and mouse reach the tile, the side panel works. Report: `F-SDK-02: pass` or `F-SDK-02: fail + what you saw`.
- **F-SDK-03** (100/50) SDK bridge phase 1 for tiles. Steps from the author needed.
- **F-SETUP-11** (100/50) Setup item **Prometheus (metrics)** (`iyakki quick-install prometheus`, Cluster group, needs k3s). Steps from the author needed.
- **F-SETUP-12** (100/50) Setup item **Graph database (for AI)** (`iyakki quick-install graph-db`, Data group in Store -> Setup, Cluster. Steps from the author needed.
- **F-SETUP-13** (100/50) Setup item **Gitea (versioning for your work)** (`iyakki quick-install gitea`, Coding agents group in Store ->. Steps from the author needed.
- **F-SETUP-14** (100/50) Setup item **ClickHouse (analytics database)** (`iyakki quick-install clickhouse`, Data group in Store -> Setu. Steps from the author needed.
- **F-SHARE-06** (100/50) Settings, Shared folders page and form (list, New share, Start/Stop, Copy link, Show QR, Edit, New secret, Del. Steps from the author needed.
- **F-SHELL-20** (100/50) `./run.sh window` on a Mac with Apple silicon (M-series). Steps from the author needed.
- **F-SHELL-31** (100/50) Moving tiles between groups, per person. Steps from the author needed.
- **F-SHELL-46** (100/50) Clicks. Try: With an external monitor open a two-pane screen (50:50, 30:70, 70:30) with tabs. Click in each pane, switch tabs, drag, use the wheel. Expect: each acts in the pane under the pointer. Report: `F-SHELL-46: pass` or `F-SHELL-46: fail + what you saw`.
- **F-SHELL-52** (100/50) Settings is an icon in the home screen's top bar instead of a tile. Steps from the author needed.
- **F-SHELL-55** (100/50) One set of top bar icons on every bar. Steps from the author needed.
- **F-SND-05** (100/30) Sound on the T2 MacBook's `aaudio`. Try: Open RADIO, play a station; open TALES and read a story aloud. Expect: sound plays, no hw params error. Report: `F-SND-05: pass` or `F-SND-05: fail + what you saw`.
- **F-SND-06** (100/30) T2 Mac browser sound and safe capture. Try: Open the web engine and play a video with sound (YouTube). Listen. Expect: sound is heard (needs the new image). Report: `F-SND-06: pass` or `F-SND-06: fail + what you saw`.
- **F-SND-09** (100/30) Audio mixer core (`iyakki-agent::mixer`, own code). Steps from the author needed.
- **F-SND-13** (100/50) An alarm caller for the mixer. Steps from the author needed.
- **F-SND-20** (100/30) Built-in. Try: With your Yes (a freeze is possible, be ready to power-cycle): Settings, Sound, Microphone, tick Use the Mac's built-in microphone, accept the risk card. Record a sound in VOICE, RECORD. Expect: your voice is heard on playback. Report: `F-SND-20: pass` or `F-SND-20: fail + what you saw`.
- **F-STARTUP-02** (100/70) Startup on screen and in developer mode. Steps from the author needed.
- **F-STUDY-05** (100/30) Lesson of a day (`iyakki-agent::studylesson`, LEARN lesson page). Steps from the author needed.
- **F-TCFG-16** (100/50) Tenant directory writer. Steps from the author needed.
- **F-UPG-06** (100/70) Upgrade run in QEMU. Steps from the author needed.
- **F-VIEW-01** (100/30) Desktop views, the model (`iyakki-agent::views`). Steps from the author needed.
- **F-VOICE-06** (100/50) Browser voice helper for the desktop window. Steps from the author needed.
- **F-VOICE-31** (100/50) A tidy model (`model.tidy`, Settings -> Models key y) used before the chat model for wording fixes and revisio. Steps from the author needed.
- **F-VOICE-34** (100/50) Wake phrase data and evaluation. Steps from the author needed.
- **F-VPN-12** (100/50) Settings -> Network -> VPN page (`settings_vpn.rs`). Steps from the author needed.
- **F-WASM-09** (100/70) Tile host runner on the iyakki-wasm sandbox. Steps from the author needed.

