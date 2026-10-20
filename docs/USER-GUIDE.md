# IYAKKI user guide

**For:** people who use IYAKKI day to day (any role). Administrators also read [`ADMIN-GUIDE.md`](ADMIN-GUIDE.md).
**You will get:** what every tile does, how to drive it, its keys, and its limits.

Every screen has a help popup: press **F1** and it lists that screen's keys. The key lists below were taken from those
popups in the source, so they match this build. Which tiles you see depends on your role (see
[Roles](#roles-and-what-you-see)).

![The launcher](media/shots/01-launcher.png)

Contents: [Global keys](#global-keys-and-the-help-overlay) · [Launcher](#the-launcher) · [Approvals and modes](#approvals-ask-first-auto-approve-safe-autopilot) ·
[Inbox](#inbox) · [Tiles](#tiles) · [Voice](#voice) · [Look and size](#density-and-text-size)

## Global keys and the help overlay

| Key | What it does |
|---|---|
| **F1** | Help for the current screen, then the keys that work everywhere |
| **F8** or the Devices button | The Devices popover (next to the screenshot icon): network, Bluetooth, screens, sound, camera, keyboard and pointer, storage and battery at a glance; see [The top bar](#the-top-bar-clock-and-devices) |
| **F7** or the clock | Settings, Time |
| **F10** or a click on the name IYAKKI | The IYAKKI menu: About, Quick install, Media player, Mac drivers, Settings; see [The IYAKKI menu](#the-iyakki-menu) |
| **F9** or the bell | Open or close the INBOX from anywhere (the app you left keeps running) |
| **F12** or Print Screen | Screenshot to the clipboard: click two corners, or Enter for the whole screen, then save a PNG or close. Saved to `~/Pictures` if your account may write there, else the data folder's `screenshots` |
| **Esc** | Back (leaves a text field first) |
| **Ctrl+V** | Paste the latest copy |
| **Ctrl+Shift+V** or right-click | Clipboard history |
| `o` (launcher) | Sign out |
| `p` (launcher) | Power menu, if your role allows |

**Selecting and copying.** Drag over text in the chat transcript, terminal output and detail panes to select (double-click
a word, triple-click a paragraph, Shift+click to extend), then **Ctrl+C**. A plain click does not copy; hover a message
or code block for its copy button. **Right-click** opens a menu (Cut/Copy/Paste/Select all over a text field, the app's own
entries, always "Clipboard history..."). In window mode the clipboard is shared with your system.

## The top bar: clock and Devices

**The clock.** Every screen after sign-in shows the time (24-hour by default, `14:05`) at the top, with a short date beside it
when there is room (`Sat 17 Oct`). Point at it for the full date, the time zone and, when needed, a warning; click it (or press
**F7**) to open Settings, Time. A clock that was never set (a Mac mini or a board without a working clock battery starts in
1970) is shown in amber with a `!` and the tooltip says so: the machine asks a time server as soon as the network is up
(below). The calendar and the assistant use the same local time.

**Settings, Time.** *Everyone*: `h` 12/24-hour clock and `s` seconds in the bar (your own choice). *Administrators*: `z` time
zone (type to search the list), `u` back to the machine's own zone, `n` network time on or off, `c` another time server, `y`
ask the server now, `b` set the clock to the network time even if that moves it back by more than 5 seconds, `a` **Detect
automatically** on or off and `d` detect now. Detect automatically is off until you switch it on, because it asks an internet
service (ipapi.co, then worldtimeapi.org) which time zone this network's **public IP address** is in; nothing else is sent.
While it is on the zone is looked up again after sign-in and every 6 hours (10 minutes after a failure) and only a real zone
this machine knows is applied; the page shows the last answer or why it failed. Choosing a zone with `z` makes it fixed and
switches detection off. The zone list comes from the image's zone files (tzdata, public domain, is part of the images); a
machine without them uses a built-in table of the common zones with their daylight-saving rules. Network time is a small built-in SNTP client: it asks `time.cloudflare.com`, then
`pool.ntp.org`, once the network is up and every 4 hours, and sets the clock (it is PID 1, so it may). It sets a clock that is
before 2024 at once, steps forward at any size, ignores differences under half a second, and **never puts the clock back by
more than 5 seconds by itself**: the page tells you and waits for `b`. A desktop window follows the computer it runs on and
only offers the 12/24-hour and seconds choices.

**Devices.** The screen icon beside the camera (or **F8**) opens a compact list, read in the background so drawing never waits:

| Row | What it shows | Click or Enter goes to |
|---|---|---|
| Ethernet, Wi-Fi | link state, address, speed; for Wi-Fi the network name and signal (`-48 dBm (excellent)`); the first row also shows the host name | Settings, Network |
| Bluetooth | adapter present and powered, paired devices; **Scan** when the Bluetooth service exists | Settings, Bluetooth |
| Display | each connected output with monitor name, resolution, refresh rate and the interface size | Settings, Display |
| Sound | output card, volume, microphone; **Test** plays a short tone, **Mute** toggles (`m`) | Settings, Sound |
| Camera, Keyboard and pointer | what the kernel found | |
| Storage | drives and free space on the data volume | Settings, Storage |
| Battery | level and charging, only when there is one | |

A red dot means something is expected and missing ("Wi-Fi: no interface found", "Bluetooth: no adapter driver or firmware") and
the line gives the plain-words reason from the Drivers list. **Driver help** (button, or `d`) opens STORE, Drivers. Keys: Up/Down,
Enter, `a` the row's first button, `r` read again, Esc or F8 closes. On macOS or a desktop window the rows it cannot see say "not
available on this OS". Everything is read from `/sys` and `/proc`; a missing file is just "not there".

**Installs.** While Quick install is installing something, a small turning ring with the number of installs sits left of the
clock (filled to the progress when the installs report one); for ten minutes after the last one ended it shows a check or a
cross. Click it to see every running and recent install with its bar and last line, **Log** for the live output (it follows the
end; scroll up to read, Esc closes it), **Stop**, **Dismiss**, and *All jobs in RUNS*.

## The IYAKKI menu

Click the name **IYAKKI** at the top left of the home screen, or press **F10** anywhere. Up/Down and Enter or the letter of an
entry; Esc goes back a page and closes; a click outside closes.

* **About IYAKKI** (`a`): version, build, variant, the running kernel (with the Mac image's patches when it has them),
  IYAKKI's licence (Apache-2.0), a short *Licence boundary* in plain words (the base OS runs as separate programs and is never
  linked; GPL kernel code on the Mac image; firmware; Apple firmware never shipped; sources in the image's legal-info) and,
  scrollable, every licence inside: the Rust crates linked into the program grouped by licence, and the programs of the images.
* **Quick install** (`q`, administrators): four sections, each item with its state and its licence; the order is shown before
  Install. *Cluster*: k3s, and DuckDB, Elasticsearch, Indic-TTS and the Local image registry (a registry pod with a volume in the cluster, for images built on this device such as the private store's server; no cluster restart) which install k3s first. *AI coding*: Claude Code, GitHub
  Copilot CLI (Node.js needed). *Media*: the media player (a static ffmpeg, downloaded from its builder and checked with its checksum). *Voice* (pick one or both): Kokoro (English, pulled into Lemonade, which is installed first)
  and Indic-TTS (Indian languages, in k3s; the model weights have **no stated licence**, an open decision, so nothing is
  bundled and you supply the models). *Local models* (pick one or both): Lemonade, Ollama; both are started and added to
  Settings, Models when they answer. Every step runs as a background job you can follow beside the clock; Indic-TTS opens the
  Platform services deploy review once k3s is there. A desktop window never installs a cluster.
* **Media player** (`m`): which player the image has, a file to play, Stop, and MEDIA. Video and sound (the camera's `.avi`
  recordings too) play in IYAKKI's own player once the media player is installed (Quick install, Media; the page has an
  Install button); without it the page says so. See [`MEDIA-PLAYER.md`](MEDIA-PLAYER.md).
* **Mac drivers** (`d`): Wi-Fi firmware (run `dist/drivers/mac-wifi-firmware-from-macos.sh` on the Mac's macOS, then STORE,
  Drivers, Install from folder or USB), T2 audio, Bluetooth and the USB camera as the system sees them, with STORE, Drivers.
* **Settings** (`s`): Settings is no longer a tile; open it here, with F7 or by clicking the clock.
* **Developer mode** (`v`, administrators): opens Settings, Developer mode. While it is on, a small amber **DEV** sign (DEV ·
  pairing while a pairing code is shown) sits in the top bar; click it for the same page.

## The launcher

Tiles are sorted into four sections shown as tabs at the top: **Home** (CHAT, RAG, MCP, CLI, SKILLS, CALENDAR, TALES), **Web & Media**
(BROWSER, RADIO, MEDIA, CAMERA), **Work** (EDIT, RUNS, SRE PILOT, AI IDE, DATA, DASH, FLOWS) and **System**
(USERS, NET SCAN, MESH, USAGE, SYSTEM, STORE). Custom apps go to the section their app.json names, else Work. Arrow keys
choose, Enter opens, **Tab / Shift+Tab** (or clicking a tab) switches section (your last section is remembered),
**PageUp/PageDown**, the wheel or the dots turn the page when a section has more tiles than fit, **1-9** open the Nth tile of
the whole list. A tile with a DEMO ribbon (SRE PILOT) shows sample data only.
Tiles your role may not use are locked: a message names your role.

## Approvals: Ask first, Auto-approve safe, Autopilot

The assistant can only do what your role and policy allow ([`ADMIN-GUIDE.md`](ADMIN-GUIDE.md#accounts-roles-and-policy)). Within that:

* **Reading is free.** Writing a file, calling an API, running a program or starting a container pauses for
  **Allow once** (`Ctrl+Y`), **Allow for this session** (`Ctrl+U`) or **Deny** (`Ctrl+N`). No answer means deny.
* **Allow for this session** sticks for the whole chat: the same tool in the same scope is not asked again, in later messages
  and in Autopilot steps too. The scope is: the same program and subcommand for a command (`git status` does not cover
  `git push`), the same cluster and namespace for the cluster tools, the same host for web calls, any file in the workspace
  (or in the same folder outside it) for file writes. Shells and interpreters (`bash -c`, `python`, `ssh`, `curl`) are covered
  only as the exact same command line. "Approved this session: N" next to the mode chip lists what you allowed
  (`Ctrl+Shift+Q`): click Revoke to take one back, or Reset approvals to forget all. A new chat starts with none.
* **Calls that always ask** never get a session answer: the card says "Always asks: <reason>" and has only Allow once and
  Deny. These are secrets and the vault, screen capture (which has its own "Allow for 5 minutes"), browser interaction with
  sites, destructive actions (delete, `rm`, namespace removal, HTTP DELETE, teardown), anything outside your workspace,
  installs from outside tools, store installs and publishing, platform deploy and remove, driver installs, drive writes, and
  any change to a remote cluster.
* **One chip with three modes** (`Ctrl+Q` cycles; shown as "Approvals: Ask first / Auto-approve safe / Autopilot"):
  **Ask first** asks for anything you have not approved earlier and remembers your session approvals. **Auto-approve safe**
  runs the assistant's ordinary writes and commands without a prompt, each one still audited, and asks only for the calls
  above; it ends when you sign out or leave the workspace unless kept ("Auto-approve safe · kept", remembered per person
  and workspace). Earlier versions called these Ask, AUTO and AUTO pinned; the saved choice carries over.
* **Autopilot** (the third mode of the chip in the AI IDE chat pane, or Ctrl+Shift+A): runs the plan to the end within its
  budgets and still stops for the always-ask items. give a goal; the assistant plans it on the board, works through the rows,
  retries a failing row up to three times, verifies, and writes a summary. Budgets: 40 steps, 30 minutes, your token budget,
  3 steps without progress. Typing a message pauses it; Ctrl+Shift+R resumes.
* **Change the mode while the assistant works.** The chip (and `Ctrl+Q`, `Ctrl+Shift+A`) stays enabled during a run, in CHAT,
  the AI IDE chat, the DASH and FLOWS chats and for a coding agent (Claude Code) working through the bridge. The change applies
  to the run at once, a line in the conversation marks the point ("Approvals changed to … during this run"), and the audit log
  has `approvals mode changed to … during run <id> by <you>`. Hover the chip to see what changes now:
  * to **Auto-approve safe**: a card that is waiting is approved at once if Auto would have run it ("approved by switching to
    Auto-approve safe"); always-ask cards keep waiting;
  * to **Ask first**: the next risky call asks; a tool already running finishes;
  * **Autopilot on** (workspace chat only): the current turn finishes, then the run goes on into the plan with fresh budgets;
  * **Autopilot off**: the current step finishes (its board updates are written), then Autopilot stops, saves its place and
    writes a summary; jobs it started keep running in the Jobs strip. Turn it on and press Resume to continue. The **Pause**
    button still pauses at once.
  Session approvals are not changed by a switch. In the Tasks tab each running task has its own mode button (`a` cycles
  Autopilot → Ask first → Auto-approve safe); leaving Autopilot pauses that task after its step, and Resume works the plan again.
* **Budgets per run** (tokens, seconds, files written, cost) come from your account and can be changed by an administrator in
  USERS (`b`).

## Inbox

The bell at the top of every screen (or F9). Three pages (`Tab` or `1 2 3`):

* **Notifications:** reminders that rang, finished runs, new devices. `Enter` open, `r` read, `f` flag, `c` add to the calendar,
  `s` snooze, `d` delete, `A` all read, `X` clear read.
* **Tasks:** pending, doing, done; assistants can add to-dos. `n` new, `Enter`/Right next state, `e` edit, `p` priority,
  `c` calendar, `x` drop, `d` delete, `f` filter.
* **Approvals:** every waiting tool call from any app, one at a time: `y` approve, `n` reject, `Enter` for details.

Alarms and reminders ring wherever you are: Enter dismisses, `S` snoozes five minutes.

## Where your files live

Your home folder is `/home/<your name>`. Photos and videos from CAMERA (Pictures, Videos), TALES stories, AI IDE workspaces,
downloads and the sign-ins of command-line tools you run in IYAKKI (for example `~/.claude`) are kept there. On an installed
IYAKKI, or any stick with a data volume, `/home` is stored on that volume, so everything stays after a restart; the terminal,
the assistant's tool runs and jobs start in your home folder and use it as `HOME`. A session without a data volume
(*SYSTEM -> Storage* says "RAM") loses these files at power-off: save or persist the disk first. Other people's home folders
are private to them (mode 0700).

## Tiles

### CHAT
Talk to a model. Chats are saved with memory, listed in a side panel, shown as Markdown.
`Enter` send · `Ctrl+N` new chat · `Ctrl+P` choose model · `Ctrl+W` hide/show the chat list · `Ctrl+L` chat picker · `Ctrl+B` delete
chat · `Ctrl+D` use my documents (RAG) · `Ctrl+E` edit a file with the assistant · **`Ctrl+G` agent mode** · `Ctrl+Q` approval mode
(Ask first / Auto-approve safe / Autopilot) · `Ctrl+K` stop the run · `Ctrl+R` dictate · `Ctrl+O` read answers aloud · `Ctrl+Shift+H` save the last answer to the
computer (window mode: its table as `result.csv`, else a code block, else `answer.md`).
The message box is three lines tall and grows to ten or twelve lines as you type, then scrolls inside (mouse wheel over it, arrows, Page keys); it takes
up to 20,000 characters, so a long error log or spec can be pasted whole with its line breaks. `Ctrl+M` makes a new line (`Ctrl+J` too, outside the AI IDE);
`Enter` sends. If a paste is longer than the limit, the first 20,000 characters are kept and a red line under the box says how many were left out. The AI IDE's chat
and the other chats use the same box.

* In agent mode it can read files, search your documents, search the web and read pages (`web_search`, `web_fetch`), weather,
  share prices, call APIs, run read-only text commands, run `ssh`, `scp`, `sftp`, `aws`, `gcloud` and installed CLIs (each call asks), plan in your calendar,
  play music, manage models and run code in a limited container (`sandbox_run`, needs the container cluster of the image).
  It can use your own **browser tab** read-only by default; clicking and typing need your approval per site and it never
  types passwords ([`BROWSER-AGENT.md`](BROWSER-AGENT.md), [`WEB-RESEARCH.md`](WEB-RESEARCH.md)).
* Anything slow (an SDK install, a big build, a download) runs as a **job** (`job_start`, you approve it): it keeps running when the chat
  ends, shows in a **Jobs strip** above the composer with progress, **Log** and **Stop**, and you get an inbox note when it ends
  ([`JOBS.md`](JOBS.md)).
* SVG drawings and images in answers are shown as pictures. Pasting something that looks like a key or token offers to
  store it in your vault instead of sending it.
* Drop files on the window: text files are quoted, pictures are saved to Uploads.
* **Limits:** answers are only as good as the model; "thinking" models can sit silent for a while; a model without tool
  calling uses a plain-text protocol ([`ASSISTANT-ROBUSTNESS.md`](ASSISTANT-ROBUSTNESS.md)).

![Chat](media/shots/chat.png)

### RAG
Search your documents with hybrid keyword + vector search and ask a model grounded in them, with citations. `Enter` run ·
`Tab` search/ask · `Ctrl+O` your documents (delete what you do not need) · click a result to copy. Documents are `.txt`/`.md`
files in `docs/shared/` (everyone) and `docs/<username>/` (private) of the data folder. External backends (Qdrant, Chroma)
are added in Settings -> RAG.

### MCP
Connect Model Context Protocol servers, browse and call their tools. `Up/Down` choose · `Enter` connect · `N` add · `D` remove.
On the arguments screen type JSON and Enter. A sample server is included. Which servers you may use is part of your policy.

### SKILLS
Short Markdown instructions the assistant follows when a request matches. `v` add what you copied (a skill or a signed `.iyp`
package) · `n` write one in a form · `l` load from a file · `d` delete · `m` only for you or for everyone (if allowed) · `k`
trusted publishers. A package is always shown to you first.

### CLI
Two pages (`Tab`): **Terminal** (policy-checked commands such as `help`, `ls`, `cat`, `policy`, `skill list`, pipes, `curl`,
`date`, `alarm`; `Enter` run, Up/Down history, PgUp/PgDn scroll) and **CLIs**, a catalogue administrators install from:
kubectl, k3s, Helm, Terraform, GitHub CLI, AWS CLI, Google Cloud CLI, Azure CLI, Arduino CLI, esptool, espflash, Ollama, DuckDB,
grpcurl, Claude Code, GitHub Copilot CLI, Codex CLI, Gemini CLI and Aider. `i` install or update (you confirm the exact
command or checksummed download), `x` remove, `s`/`t` start/stop the container cluster (k3s) or Ollama. Installed tools are
available to the chat. The terminal is not a pty: no interactive programs (signing in to Claude Code or GitHub Copilot has its own **Login** button, described next). The list also has the **SSH client**, which is found on
the machine rather than downloaded, and a third tab, **Connections**: saved hosts with a Connect button, the host-key
fingerprint you confirm the first time, and keys made into your vault. `ssh`, `scp`, `sftp`, `sshtrust`, `sshhosts` and `sshforget`
also work in the Terminal tab. See [`CLI-SSH.md`](CLI-SSH.md).

**Login for Claude Code and GitHub Copilot.** When one of them is installed, the CLIs list, Settings → Agents, Quick install (the
IYAKKI menu) and the chat's model list (Ctrl+P) show a **Login** button and what is signed in ("signed in as you@example.com" or
"not signed in"); an agent that is installed but not signed in says **Sign in first**. Login opens a dialog: for Claude Code you
choose *Claude subscription* (the usual), *Anthropic Console (API billing)* or *SSO*; then the dialog shows the sign-in address
(with **Copy address** and a QR code you can scan with a phone) and, for GitHub Copilot, a one-time code to enter at
github.com/login/device (**Copy code**). Open the address on any device, sign in, and for Claude Code type the code the browser
shows into the dialog's field and press Enter. To avoid typing the long code of Claude: press **Open in browser** (the dialog hides, you sign in in the BROWSER tile, copy the code, press Back, and tap **Paste the copied code**), or scan the **second QR code** with the phone you signed in with and paste the code there. For GitHub Copilot your token is kept in your vault, not in a plain file (unlock your vault if the dialog says it is locked). The dialog says **Signed in** when it is done; **Cancel** stops it. **Logout**
(Claude Code) signs out. Your login is kept by the vendor's program in your own home folder and survives a restart; IYAKKI never
sees your password or token and records only that a login started or finished. Details: [`EXTERNAL-AGENTS.md`](EXTERNAL-AGENTS.md).

### EDIT
An editor written in-house (11 languages highlighted). `Ctrl+S` save · `Ctrl+O` open · `Ctrl+N` new tab · `Ctrl+W` close tab ·
`Ctrl+F` find · `Ctrl+L` comment line · `F5` run (`.py .js .sh .wasm`) · `Ctrl+Z` undo. Chat can propose edits: `y` accept,
`n` reject, `a` accept all, `Enter` apply, `Esc` discard. Nothing changes until you accept and save.

### RUNS
History of every agent run, verified against its hash-chained ledger. Click a run; `c` copy transcript, `d` delete.
The second button, **Jobs** (or `j`), lists the **detached jobs** the assistant or Claude Code started (`job_start`): a spinner or a
check, the time, a sliding or filled progress bar, the last output line, **Log** and **Stop** (**Dismiss** once it ended). Jobs keep
running when the chat ends; the same rows also show in the chat and the AI IDE's chat pane ([`JOBS.md`](JOBS.md)).

### SETTINGS
Pages in a list grouped Assistant (Models, Agents, RAG, Skills, Voice), Apps (Browser, Host access, Dev cluster), Security
(Vault, Secret providers, Account) and Devices (Network, Bluetooth, Display, Sound, Storage) and System (Memory, **Host name**, **Time**, Install to disk, **Developer mode**). **Dev cluster** (administrators) holds the cluster profiles, the registry, the local cluster setup, and the **Platform services** page (`P`): Gitea and the private store deployed to a cluster you choose after you approve an exact plan ([`PLATFORM-SERVICES.md`](PLATFORM-SERVICES.md)). Settings, **System** has **Memory** (zram swap and swap file, administrators; [`ZRAM-AND-INSTALL.md`](ZRAM-AND-INSTALL.md)) and, on a booted IYAKKI stick, **Install to disk**. `Ctrl+F` or `/` searches
pages. Hardware pages and most changes are administrator-only. Keys per page are shown in the header and F1: Models
`a e d t m b k x c`; Vault `n d Tab l f`; Network `Enter`, `w` join Wi-Fi, `m` private MAC address, `a` apply it now, `r`; Display `u U + -`; Sound `o t + - m Enter`; Voice
`s v c m p t y`; Storage `m` make data disk (erases it); Account `p` change password, `u U` text size. Details:
[`ADMIN-GUIDE.md`](ADMIN-GUIDE.md).

**Host name (Settings, System, Host name; administrators).** The name your network and the terminal prompt use. It was chosen
in the installer or defaults to the image's own; here `e` types a new one (lower-case letters `a-z`, digits and `-`, up to 63
characters, not starting or ending with `-`; the page says what is wrong when it is refused), `y` **Apply now** changes the
running system and `/etc/hostname` at once and keeps it in `system.conf` for the next boots, `v` only saves it. Other keys of
`system.conf` are left alone. The boot option `iyakki.hostname=NAME` overrides the file and locks the page. On the command line:
`iyakki hostname` shows it, `iyakki hostname NAME` sets it (as root). A desktop window shows the host computer's name and changes
nothing.

**Developer mode (Settings, System, Developer mode; administrators).** Lets a computer you pair install, upgrade, remove, open and
watch apps on this device (`iyakki dev install`, `push --watch`, `logs --follow`, `run`, `screenshot`), like `adb` for IYAKKI. Off by
default. `e` switches it on or off, `h` chooses when it switches itself off (4 h, 1 h, 8 h, 24 h, 72 h or never), `p` shows a pairing code
and this device's key fingerprint for the computer to type, `x` revokes a paired computer, `l` shows the activity log (every action:
which computer, which account, what, when). What it installs is labelled **Developer (local)** everywhere and goes through the STORE's
checks; driver packs cannot be installed this way. Details and the security model: [`DEVELOPER-MODE.md`](DEVELOPER-MODE.md).

![Settings](media/shots/settings.png)

**Private MAC address (Settings, Network).** Every network adapter has a hardware address (the MAC address) that the router sees and
that tracks you from one network to the next. `m` chooses what IYAKKI shows instead, separately for Ethernet and for Wi-Fi:

| Choice | What the router sees | Default for |
|---|---|---|
| **Off (hardware address)** | the adapter's real address | Ethernet (so DHCP reservations and router rules keep working) |
| **Per network** | a made-up address that is the same every time you join the same Wi-Fi network (or use the same Ethernet link) and different on every other network and every other device | Wi-Fi |
| **Random each connection** | a new made-up address every time the adapter is brought up or a Wi-Fi network is joined | nobody |

Each adapter's row under the list shows the mode, the hardware address, the address in use now and whether it is locally
administered. A change is saved at once and takes effect on the next connection (start-up, joining a network, switching the adapter
on); `a` applies it now, after a question, because the adapter must go down for a few seconds. The question is stronger when that
adapter carries the SSH or remote-screen session you are using. Some adapters and drivers refuse to change their address; the page
then says "not supported by this driver" and the adapter keeps its hardware address. Everyone sees the address in use on the
System tile (`MAC: aa:bb:... (private)`); only administrators change it. On the command line: `iyakki net mac` ([`ADMIN-GUIDE.md`](ADMIN-GUIDE.md)).
In the desktop window the host computer owns the addresses and the page only shows them.

### BROWSER
A browser written in-house: HTML + CSS, forms, cookies, images, JavaScript (QuickJS in WebAssembly), tabs, find, bookmarks,
history, downloads, reader view. `g` address/search · `Tab` links · `b f r` back/forward/reload · `Ctrl+T/W/Shift+T` tabs ·
`Ctrl+D/B/H/J` bookmark/bookmarks/history/downloads · `Ctrl+K` fill saved sign-in · `Ctrl+F` find · `j` allow JavaScript for
this site · `c` developer panel · `+ - 0` zoom · `R` reader · `U` view source · `X` forget site data · `N` night colours ·
**`F` the full web engine** (WebKit, needs its helper, [`FULL-BROWSER.md`](FULL-BROWSER.md)) · `Q` ask / `S` summarise / `T`
translate / `M` keep in knowledge (asks each time). **Searching:** words typed in the bar are searched on Bing's plain results page (Settings, Browser: DuckDuckGo, Brave Search or Wikipedia); if that page is busy or asks for a check, the next one is tried and a note says so. Google's own results page only answers with "enable JavaScript", so Google searches are run on your chosen engine too. A name that does not exist (`youtub.com`) shows "Couldn't find ..." with a search for it and, for near misses of popular sites, "Did you mean youtube.com?". YouTube, Gmail, Google sign-in and similar apps need the full engine: the `web` or `macweb` image (the Mac image with the web engine). **In the full engine** click a box and type; the keyboard goes to the page (Esc gives it back to the browser, Ctrl+L is the address bar, Ctrl+T/Ctrl+W tabs); Ctrl+C/Ctrl+V and a page's own Copy button use IYAKKI's clipboard; use a mouse (touchpads are not read). **Limits:** no video, canvas or IndexedDB in the built-in engine; CJK
and emoji show as boxes; heavy web apps need `F`. Honest list: [`BROWSER-ROADMAP.md`](BROWSER-ROADMAP.md).

![Browser](media/shots/browser.png)

### RADIO
Internet radio in many languages (My stations, Tamil, Telugu, Malayalam, English, Hindi, Spanish, French, Arabic,
Portuguese, Russian, Chinese, Bengali). Left/Right language, Up/Down station, `Enter` play, `Space`/`s` stop, `[ ]` previous/next,
`+ -` volume, `a` keep, `n` add your own (name and MP3 stream address), `d` delete. Plays MP3 streams only. Esc leaves; the
music keeps playing.

### MEDIA
Your music, videos and pictures from folders your account may read. `Tab` pages (Tracks, Artists, Albums, Videos, Playlists,
Folders, Types, Top rated, Photos, Downloads) · `/` search in plain words ("melody songs by anirudh above 4 stars") ·
`1-5` rate · `t` tags · `D` queue downloads from direct addresses · `Space` stop · `, .` previous/next · `z` shuffle · `l`
repeat. Three CC0 samples are included. MP3 plays inside IYAKKI; other audio/video needs an installed player (mpv, VLC,
ffplay, mplayer), or the web image. [`MEDIA-SEARCH.md`](MEDIA-SEARCH.md).

### CAMERA
A USB webcam as a camera. The tile is greyed with "Connect a camera" until a camera is plugged in (it wakes up within
about two seconds; choosing the greyed tile says why there is none: a Mac mini has no built-in camera, and the built-in
camera of a Mac with a T2 chip is not reachable). The live picture comes as MJPEG from the camera when it offers it, else YUYV.

- **Photo** (`1`): `Space` or the shutter saves a JPEG in your **Pictures** folder (`photo-<date>-<time>.jpg`), with a flash
  and a thumbnail in the strip under the picture; click a thumbnail to look at it (`←` `→` move, `Esc` closes).
- **Video** (`2`): `Space` starts and stops recording; the time and size are shown. Saved as Motion-JPEG in an AVI file in
  **Videos** (`video-<date>-<time>.avi`), which mpv, VLC, ffplay and MEDIA on the web image play. **Picture only, no sound.**
  Past 1 GB a recording continues in a `-part2.avi` file.
- **Passport** (`3`): a frame with guide lines (crown line, eye line, chin band, head oval) for the country chosen with `p`
  (type any word: `india`, `usa visa`, `schengen`, `deutschland`…). The table holds USA passport, US visa (DS-160), India
  passport and e-Visa, UK, Schengen/EU, Germany, France, Canada (50×70 mm), Australia, China passport and visa, Japan,
  Singapore, Malaysia (35×50 mm), UAE, Saudi eVisa, Sri Lanka, New Zealand, South Africa, Brazil, Mexico, Russia and a
  **Custom size** (your own width and height). Each entry shows its official source and the day it was checked; entries
  that could not be confirmed on an official page say **UNVERIFIED** (US visa upload limits, Sri Lanka, New Zealand head
  size, South Africa, Brazil, Mexico, Russia): check those yourself.

  `-` `=` zoom the frame, the arrows move it. The shutter counts down 3 seconds, then saves the crop as a 300 dpi PNG
  (`passport-<country>-...png`) and, where the country has rules for online forms (US, India e-Visa, UK, Canada, China visa,
  Japan, Singapore, Saudi eVisa, New Zealand), a JPEG within its pixel and file-size limits (`...-online.jpg`; for the US visa
  600–1200 px square, at most 240 KB). Then `4` makes a printable sheet on 4×6 in photo paper and `a` one on A4, each as PNG
  and PDF with as many copies as fit and thin cutting lines; print at 100% (no scaling). Stand before a plain wall in the
  colour the country asks for (France does not allow white) with even light; nothing removes the background for you.
- `t` timer off / 3 s / 10 s · `m` mirror the live picture (saved pictures are never mirrored) · `c` choose the camera ·
  `r` picture size · `[ ]` brightness and `; '` contrast when the camera has them · `Esc` back (a recording is finished and kept).

**The assistant and the camera.** In the chat you can say "take my photo", "record 20 seconds of video" or "make an
India e-Visa photo". The assistant uses the tools `camera_list`, `camera_photo`, `camera_record_start` (with a time limit,
at most 10 minutes), `camera_record_stop`, `camera_passport` (country and sheet; it answers with the files and the rules used)
and `camera_recent`. It **asks you every time** it would take a picture; **Allow for 5 minutes** is the longest permission,
and Auto or Autopilot never answer for you. While it uses the camera a red **Camera in use by the assistant** sign shows on
every screen. Pictures go only into your Pictures and Videos folders; the assistant gets the file names, never the image,
and nothing is uploaded. If the CAMERA app is open, the camera is busy for the assistant: close the app first.

Guests have no CAMERA tile. Nothing leaves the machine.

![Camera](media/shots/camera-photo.png)
![Passport frame](media/shots/camera-passport.png)

### CALENDAR
Month view, events with repeats and reminders, a clock with alarms. Arrows move, `PgUp/PgDn` month, `T` today, `N` new
event, `A` show alarms, `D` delete, `Tab` clock page. The chat reads and plans in it.

### INBOX
See [Inbox](#inbox).

### SYSTEM
Overview (processor, memory, load, disks, network, heat), Processes (administrators; `k` stop, `K` kill, sort `c m p n`),
Agents (every run going on; `x` stop, `X` stop all), Hardware (administrators). `Tab` pages. The Overview shows the host name. `iyakki top` shows similar
output in a terminal.

![System](media/shots/system.png)

### USAGE
Token usage per model as a chart: `1`-`5` for 24 hours, 7 days, 30 days, 13 weeks, 12 months; `e` (admins) own or everyone.
Only totals are kept, never what was said.

### NET SCAN
Finds Ollama servers, cameras, media servers and other services on private addresses; read-only. `s` scan · `n` next network ·
`t` scan an address or range · `a` add a model server to Models · `c` copy an address · `f`/`1-6` filter. Every scan is audited.

### MESH
Consul service view: Services, Nodes, Key/Value, Setup (`1-4`). `r` reload, `n` new key, `e` change value, `d` delete, `c` connection
(admins), `t` test. Needs a Consul to talk to.

### USERS (administrators)
Accounts, roles and per-user limits. See the next section and [`ADMIN-GUIDE.md`](ADMIN-GUIDE.md#accounts-roles-and-policy).

### SRE PILOT (demo)
An animated picture of a site-reliability assistant with sample data; nothing is connected to a real system. Click a part,
`Space` pauses, `1 2 3` speed, `W` whiteboard photo. "Click To Schedule a Demo" is a dummy button.

### AI IDE
One workspace (folder) per product with editor tabs, file tree, terminal, tests, chat limited to the workspace, a feature
board (reads and writes `FEATURES.md`; rows can wait for other rows, a Graph view draws the order and who is working on what), a Context tab (what the whole team and every agent must remember, kept in the committed `.iyakki/context.json`; say "remember that ..." in the chat to add to it), a Browser tab, parallel tasks, and a Kubernetes view with a Deploy flow
(Build, Get the image to the cluster, Apply, Check the rollout; every step approved). Keys: `Ctrl+Shift+W` workspaces ·
`Ctrl+P` open file · `Ctrl+Shift+F` search project · `Ctrl+B`/`Ctrl+J` tree/panel · `Ctrl+Shift+E D C T` focus tree/editor/chat/terminal
· `Ctrl+Shift+B/G/Y/O/K` board/tasks/context/browser/cluster · `F5` run · `Ctrl+Shift+R` project settings · `Ctrl+=` / `Ctrl+-` density · `F1`.
A lock stops a second window from editing the same workspace (Take over, Open read-only, Cancel). Details:
[`AI-IDE.md`](AI-IDE.md), [`K3S-DEV.md`](K3S-DEV.md), [`PARALLEL-TASKS.md`](PARALLEL-TASKS.md).

![AI IDE](media/shots/ide-01-main.png)
![Feature board](media/shots/ide-04-board.png)
![Kubernetes view](media/shots/ide-k8s.png)

### DATA
Open a CSV, TSV or JSON-lines file in a virtual-scrolled table (a million rows are fine). `o` open · header click sort ·
`/` filter (`price > 10 and contains(name, 'tea')`, `x` clears) · `c`/`r` copy cell/row · `v` filter to this value · `< > =`
column width · `s` summary · `e` export the view as CSV. Files come from your home folder, your workspaces and shared host
folders. [`DATA.md`](DATA.md).

![Data](media/shots/data.png)

### DASH
Dashboards over Prometheus, Elasticsearch/OpenSearch, ClickHouse and DuckDB, read-only by rule, credentials from the vault.
Panels: line, area, bar, stacked bar, pie, donut, gauge, stat, heatmap, table, text. Arrows choose a panel, `Enter`/`e` edit,
`a` add, `d` duplicate, `Delete` remove (`Ctrl+Z` undo), `1`-`7` time range, `r` refresh, `v` table view, `i` ask the assistant,
`Ctrl+I` assistant pane, `Ctrl+S` save. Dashboards are JSON files in your `Dashboards` folder. [`DASHBOARD.md`](DASHBOARD.md).

![Dashboard](media/shots/dash.png)

### FLOWS
Design, run, schedule and watch workflows (HTTP, command, browser, gRPC, agent, approval, notify, sub-flow, for-each nodes);
failed runs can be examined by the assistant, which proposes fixes you approve. Tabs `1 2 3` = DESIGN, RUN, BOARD. DESIGN:
`a` add step, `f` fit, `g` arrange, `t` text view, `T` test step, `r` run, `p` publish, `h` versions, Ctrl+Z/Y/C/V/D. RUN: `r` retry,
`s` skip, `R` rerun from here, `c` cancel, `y`/`n` approve/reject. Schedules run while you are signed in. Thrift nodes and
webhook triggers are planned, not built. [`FLOWS.md`](FLOWS.md).

![Flows](media/shots/flows.png)

### TALES
An AI story teller that writes, draws and reads aloud, as a conversation. The story is on the left; on the right is a chat panel
like the AI IDE's. Type an idea of up to 4000 characters in the idea box (three lines tall, it grows to ten and then scrolls; you can say it in words: *a 5-minute funny story in Tamil about a fox*) and press `Enter` or
**Send** (`Ctrl+J` makes a new line; a longer idea is cut with a notice that gives the counts); choose who it is for (Kids, Teens, Adults), the kind and tone (drop-downs), one **Model: …** chip (`Ctrl+P`, `Ctrl+G` too) that opens the same chooser as the chat: *Same as chat* first, then the models of your providers, then Claude Code, Copilot and the other agents with their models and *Custom model id…* (an agent never gets tools here); **Options ▾** (`Ctrl+O`) has the length, the language,
*Pictures off* and the reading and saving switches. Pictures are automatic (real images when a picture model is set up, otherwise drawings by the writer, or the built-in cartoon composer for small models; **Options -> Drawing** chooses, and the chip says what drew each picture; a character sheet keeps the characters looking the same). Then keep talking: *make it shorter*, *change the goat's name to Beeru*, *translate it to Tamil*, *redraw picture 2 as a
comic*. Each correction is a new **Version** (the earlier ones stay, choose them from *Version ▾*); **Stop** keeps what was written;
a message sent while it writes steers it. The conversation is saved with the story. `Space`/`p` play-pause, `s` stop, `r` replay,
`v` voice, `+ -` speed, `Ctrl+S` save to `Stories/<title>/`, `e` export (.html, .md, audio), `f` favourite, `l` saved stories, `n` a
new story. Reading aloud needs a voice: open IYAKKI in a browser, or Settings -> Voice (the hint under the player has a **How**).
While it reads, the sentence and the word being said are lit and the page follows; click a sentence to read from there (Settings -> Voice -> Highlight chooses *Sentence + word*, *Sentence only* or *Off*). The model runs with no tools and strict child-safety rules. [`STORY.md`](STORY.md).

![Tales](media/shots/tales.png)

### Custom apps (your own tiles)
Apps made for IYAKKI appear as tiles in the section they name (else **Work**) after the system tiles, with the label *custom app*. Open one and it fills the screen; the bar
shows who made it and its trust level, **Reload**, **Info** (what it may do) and **Remove** (it asks whether to keep or delete the app's saved data). *Back* returns to the launcher.
Install them from the STORE (the sample calculator, notes and converter are in the built-in catalog) or build your own in the AI IDE (New app, *IYAKKI tile app*). An
administrator can add a workspace folder as an **unsigned dev app** (marked DEV). An app installed from a paired developer computer in
developer mode is marked DEV too and labelled **Developer (local)**; a new build pushed from that computer reloads it while it is open
([`DEVELOPER-MODE.md`](DEVELOPER-MODE.md)). Accounts that may not use the browser do not get custom apps.

### STORE
Find, install, update and remove skills, WASM tools, MCP servers, themes, driver packs and CLI tools. Opens on **Installed**.
Every app wears a trust badge (Official, Verified, Trusted, Community, Unsigned, Revoked) and you review hash, signature,
permissions in plain words before anything installs. The online official catalog and payments are not switched on in the alpha
(a built-in catalog, folders and USB sticks work offline). `Enter` details · `/` search · `i` install · `u` update · `r`
remove · `d` turn off/on · `o` open where it lives.

* **Keeping things current.** `c` (Check) looks for news in your catalogs and asks the makers of your command-line tools for their newest
  version; nothing is fetched otherwise (the daily check in Settings is off by default). Rows with an update show **Update**, the
  *Updates N* chip narrows the list to them, and **Update all** (`U`) shows a review first, then updates one item at a time. Anything that
  needs a decision (it asks for **new permissions**, has warnings, comes from another store, is pinned) is skipped and listed; update it
  by hand with `u` and you see exactly what is new before anything changes. After an update the new version is tested; if it does not
  work the old one is put back. `b` **goes back** to the previous version (one is kept) and pins it; `p` pins a version so updates skip it
  (press again to unpin). The item's page shows where it came from, what is new, the version history, its data and what uses it.
  Built-in things say *Built in*; things you wrote or installed from a file say *No way to check*; a store you removed says *Source
  gone*; with no network the list says *Can't check offline*.
* **Removing.** `r` shows what goes, what stays and what uses it. Its data (and vault entries named after it) is **kept** unless you
  choose *Remove and delete data* and type its name. Platform services (Gitea, the private store) are upgraded and removed from the
  SERVICES screen, which names the cluster and asks every time.
* **From the terminal and the assistant.** In the terminal: `store list --outdated`, `store update <id>` or `--all`, `store remove <id>
  [--keep-data|--delete-data --confirm=<name>]`, `store rollback <id>`, `store pin <id>` (an id is `skill:name`, `cli:name`, `mcp:name`).
  The MCP, SKILLS and CLI tiles have `u` for the same actions. The assistant can list what is outdated; updating, removing and going back
  always ask you, every time, and it cannot give an update new permissions.
* **Your own arrangement.** Give things tags (`t`; in Discover `#`), put them in groups (`m`, or drag them in *Group by: My groups*, `v`)
  and give them your own title (`e`). Your title is only for lists: the real name, id and publisher stay under it, on the item's page and
  in every install or update review. Tags and groups are searchable, tags filter (click several), `g` (Discover: `O`) opens the organise
  menu (rename, reorder or delete groups, export or import your arrangement as a file). Keyboard: Shift+Up/Down reorder, on a group
  heading Enter folds, `e` renames, `k` colours, `x` deletes. It is yours alone and is never sent anywhere. Details:
  [`STORE.md`](STORE.md) sections 15 and 16.

* **Store switcher.** With two or more stores, Discover shows a chip **Store: All** (or a store's name). `s` or a click cycles
  through them; the list shows only that store's apps, and your choice is remembered. An app listed by several stores shows which one it
  comes from and "Also in" the others; installing always uses the store of the version you are looking at.
* **Add a store** (administrators): Settings, **Add a store** (`a`), paste the https link of a portal page (ending in `/`) or of a
  `catalog.json`. IYAKKI reads it and shows its name, number of apps, the signer's **fingerprint** and the trust level; nothing is
  enabled until you press `y` ("They match") after comparing the fingerprint with the one the store's owner gave you. `w` adds the
  official IYAKKI store once its address exists (it says "coming soon" today). Up to 8 added stores.
* **Private store.** Your own store on your own cluster (`iyakki store-server`, deployed from Settings, Dev cluster, Platform
  services, or the STORE card "Private store"). Add it like any store; withdrawn packages are hidden, but copies already installed
  stay. To make something to publish, see [`APP-AUTHORING.md`](APP-AUTHORING.md) (the AI IDE's top bar `...`, *New app, MCP server or
  skill...*, `Ctrl+Shift+N`). Details: [`STORE.md`](STORE.md) sections 13 and 14.
  **If the install stopped half-way, or you lost the token or want to start over:** Settings, **Repair, reset or remove the private store...** shows
  what is there and what is missing, and offers **Repair** (puts back what is missing, keeps your apps), **Reset and reinstall** (deletes the store and deploys
  it again; the old apps are gone) and **Remove** (choose *keep my published apps*, the default, or *delete everything*; the image registry is asked about
  separately). Anything that deletes needs the namespace typed. See [`STORE.md`](STORE.md) section 13.2.

![Store](media/shots/store.png)

## Roles and what you see

| Role | Tiles |
|---|---|
| admin | all, including USERS |
| operator | everything except USERS |
| user | everything except USERS, MESH, NET SCAN (restricted terminal; no MCP servers by default) |
| guest | CHAT, SRE PILOT, INBOX only; no terminal, no files, no network |

An administrator can switch tiles and limits per person in USERS. You cannot do more than your role, and neither can the assistant.

## Sound

**Where sound plays (Settings, Devices, Sound; administrators).** The radio, MEDIA, reading aloud (TALES, chat), the voice test,
alarms and the Devices popover's Test all play on one output, shown at the top of the page: **Sound output · Automatic** with the
output in use and why ("monitor PDM-15T is connected over HDMI/DP"), or why there is none. Automatic picks, in order: speakers or
headphones that are connected; an HDMI/DisplayPort output with a monitor on it (one whose monitor reports speakers first); a USB audio
device; the built-in speaker of a T2 Mac (Mac image with apple-bce: `Built-in speaker (Apple T2 Audio)`; headphones in its jack come
first); an analog output even when no jack reads as plugged in; and otherwise plays nothing and says *no sound output: connect a monitor
with speakers over HDMI/DP or a USB audio device; built-in speakers on T2 Macs need the apple-bce driver of the Mac image*. It looks again every few seconds, so a monitor plugged in later, or moved to
another port, is used by the next sound, and the radio carries on there after a short gap.

Below that each output is listed by name (`HDMI: PDM-15T`, `HDMI 2 (no monitor)`, `USB Audio: ...`, `Speakers/headphones (...)`) with
its card, device, ALSA name and state, and **IN USE** on the one that plays. `o` (button **Output**) chooses Automatic or one output;
`Enter` on an output row uses it. A chosen output that is unplugged falls back to Automatic until it comes back. `t` (**Test sound**)
plays a short gentle chime on the selected output row, or on the one in use. **IYAKKI volume** (`+` `-`, the slider, `m` mute) is
applied by IYAKKI itself to the sound it plays; HDMI on Intel has no hardware volume, so for HDMI this is the volume. Cards that do
have a mixer keep their own volume rows below. The choice is the machine's and is kept in the settings file. The SYSTEM tile and the
Devices popover show the output in use, or why there is none. A play command in Settings -> Voice that names its own device
(`aplay -D hw:1,0`) is left as it is; other players (`pw-play`, `play`) are not changed. On a computer with PulseAudio or PipeWire,
Automatic leaves the choice to that sound server. Engines that speak by themselves (espeak-ng or speech-dispatcher speaking
directly) use the system's default device; engines that make a WAV (piper, espeak-ng to a file, HTTP) go to the chosen output.

## Voice

Dictation (`Ctrl+R` in chat) and read-aloud (`Ctrl+O`, TALES) use the engines in Settings -> Voice. **Reading aloud needs no setup when
the computer already has a speech engine**: IYAKKI looks once in the background (piper, espeak-ng, speech-dispatcher, festival, and the
system voices of macOS and Windows) and uses what it finds, unless an administrator chose something else. Settings -> Voice lists each
engine with its voices, whether it speaks your language and can save audio, and for the others why not (for example "piper has no voice
file", with how to install one); choosers for engine, voice and speed, a Test button and Re-check (keys `s v c m p t y`). In the
**desktop window**, when the web engine cannot speak, the button **Browser voice** (also the chip in TALES) opens your default browser
once on a small page that speaks for IYAKKI and shows *connected*; it works on this computer only and closes when you press Disconnect or
close the tab. In a browser tab on `iyakki web` the browser's own voice is used. In chat, a reply read aloud (`Ctrl+O` switches it on) has the same lit sentence and word, a **Pause**/**Resume** button beside **Stop** (`Ctrl+K`), and a click on a sentence reads from there; *Highlight* in Settings -> Voice (key `h`) turns the word or all of it off. The speech engines are not shipped with the images
(licences); with none, the story is still written and saved. [`VOICE.md`](VOICE.md).

**Indic voices (Tamil, Telugu, Malayalam, Kannada, Hindi, Bengali, Marathi and more).** espeak-ng is robotic in these languages; Settings -> **Indic voices**
(administrators) installs neural voices (AI4Bharat Indic-TTS) and switches the reading to them (`e`); female or male with `g`, a test
with `y`. Each voice is about 260 MB; `d` asks first and shows where it would come from and how big it is, `u` installs from a USB
stick on a computer that is never online, `x` deletes. The voices run in a small server (on this computer, a PC, or your k3s cluster:
[`VOICE.md`](VOICE.md)); its address is shown and text is sent only there. If a language is not installed, reading falls back to the
next engine and the page says why in one line. Default: off. Check the licence note on the page before you pass the voices on.

## Density and text size

Text and button size is per person: Settings -> Account (and Display for administrators): `u` bigger, `U` smaller
(Small 85%, Normal, Large 125%, Larger 150%, Huge 200%). The AI IDE has its own density: `Ctrl+=` looser, `Ctrl+-` denser
(Comfortable, Compact, Dense). `IYAKKI_REDUCED_MOTION=1` freezes the small animations. At very large sizes on a small screen
some pages scroll or cut off.
