# Who IYAKKI is for, what it offers them, and a video plan

Working document for the product owner and whoever makes the demo video. It is a **plan and a claims list**, not marketing
copy: every "today" below names something that exists in the alpha (0.1.0-alpha.1), and anything that is partial, untested on a
real device or missing says so. Before a claim goes into a video or a web page, check it against `FEATURES.md` and
`KNOWN-ISSUES.md` and film it on real hardware. Status words: **today** (works in the alpha, tested in code), **partial** (works
with limits), **idea** (not built; do not show or promise).

## 1. The one-sentence pitch

*IYAKKI is a small operating system built around an AI assistant that can actually do things on the computer, only with your
permission, and always in front of you.*

## 2. Selling points that hold for everyone

| Point | What it means in plain words | Where it is true today |
|---|---|---|
| **An assistant that acts, with a leash** | Chat can read your files, run tools, open pages, play music, draw and write, but every step that changes something asks first, and you can read the log afterwards. | Approval cards, permissions per person, tool log (`ASSISTANT-ROBUSTNESS.md`, `SECURITY.md`). Alpha. |
| **Your models, your choice** | Works with a model on this computer, on another machine in your home or office (Ollama, LM Studio, Lemonade, vLLM style servers), or a cloud one. Nothing is forced to leave the building. | Provider settings, LAN scan finds model servers. Tested with Lemonade and Ollama. |
| **Private by default** | One computer, many people, each with their own vault, files and chats. Passwords and keys stay in the vault, never in files or chat text. | Per-user vault and data; two-step sign-in with an authenticator is designed, not built. |
| **Small, fast, free** | Boots from a USB stick or a disk in seconds, runs on old Intel Macs and ordinary PCs, Apache-2.0, permissive dependencies only. | Booted on one real Mac mini and one MacBook Pro (T2); other machines untested. |
| **Everything in one place** | Chat, browser, editor, files, calendar, radio, media, notes, Kubernetes, Git and more are tiles on one launcher, all aware of the assistant. | About 30 tiles. Many are partial. |
| **Extend it yourself** | Anyone can add a tile app, a skill (instructions for the assistant) or a tool server (MCP) without recompiling the OS. | Tile apps, skills, MCP, store. Developer mode installs apps from another computer without a restart. |
| **Honest about its limits** | The OS shows what the assistant did, what it cost in tokens, and why it stopped. | Usage tile, logs, run history. |

## 3. Audiences

Each block: who they are, the problem, what they can do, status, and what is missing.

### 3.1 Students and learners

**Problem.** Study tools are scattered, cloud assistants see everything you type, and a good laptop is expensive.
**Why IYAKKI.** An old laptop becomes a study computer with a private tutor; nothing needs an account.

| They can | How | Status |
|---|---|---|
| Ask questions about their own notes, PDFs and course files | Chat plus the knowledge base (documents are indexed locally, answers cite them) | today |
| Get explanations, quizzes and summaries | Chat with a local or home-server model | today |
| Have stories and lessons read aloud, with the words highlighted | TALES (write a story on a topic; reads it aloud) | partial: voice breaks on the Mac are being fixed; highlight sync is improving |
| Learn to code with an assistant that edits and runs code safely | AI IDE with a sandboxed terminal, tests and Git | today |
| Practise a language with radio and read-aloud | RADIO (80 languages, many stations), voice | partial: MP3 stations today; AAC needs the optional audio add-on |
| Keep a calendar, tasks and reminders | CALENDAR, tasks, alarms | today |
| Look things up safely | BROWSER with content blocking | partial: TikTok-style H.264 video needs an optional add-on |

Missing: a study-plan helper, flash cards, handwriting or camera capture of notes (the camera works on some Macs; no study
workflow around it), school accounts.

### 3.2 Developers and makers

**Problem.** Setting up a dev machine, wiring an assistant into tools, and trusting it with a shell.
**Why IYAKKI.** The assistant works inside a workspace it is allowed into, with tests, Git and a local cluster in the box.

| They can | How | Status |
|---|---|---|
| Code with an assistant that edits, runs tests and shows diffs | AI IDE | today |
| Connect Gitea, GitHub or GitLab and browse repositories | Settings, Git connection; GIT tile | partial: just built, not yet run against real hosts |
| Deploy to a local k3s cluster and watch it | K3S dev cluster, K8S tile, deploy tools in chat | today |
| Run Python or tools in a container with approvals | LAB tile, CLI tile | today |
| Build their own tile apps, skills and MCP servers | App authoring docs, developer mode from another computer (`iyakki dev install`), store | today |
| Let Claude Code or another agent work inside IYAKKI | External agents with sign-in by QR | partial |
| See what each model costs | USAGE tile | today |

Missing: an emulator for tile apps on the developer's PC, `iyakki dev mcp`, CI for Gitea.

### 3.3 Operations, SRE and cloud engineers

**Problem.** Many consoles, alerts at night, risky one-off commands.
**Why IYAKKI.** A control desk where an assistant reads the cluster and proposes fixes, and a person approves.

| They can | How | Status |
|---|---|---|
| Browse clusters, pods, events and logs, with retries and a log of failed calls | K8S tile | today |
| Ask the assistant to diagnose a failing deploy | SRE pilot, chat tools (`k8s_status`, logs) | partial |
| Run `kubectl`, `aws`, `gcloud`, `az`, `terraform` style tools through approvals | CLI tile, chat | partial: depends on the tools being installed (downloaded at the user's click) |
| Write workflows that run on a timer, retry, and ask for help when they fail | FLOWS (graph editor, scheduler, healing agent) | today (alpha) |
| Query data and show dashboards | DATA and DASH tiles | partial |
| Look at a Consul service mesh | MESH tile | today |
| Run IYAKKI headless on a server or cloud VM and use it from a browser | Cloud mode, web serving | partial: cloud images are not the focus of the alpha |

### 3.4 Office and knowledge workers

**Problem.** Email, documents and spreadsheets spread over many apps; confidential files cannot go to a public chatbot.
**Why IYAKKI.** Ask questions across your own documents without uploading them anywhere.

| They can | How | Status |
|---|---|---|
| Ask questions across folders of documents | Knowledge base (BM25 plus vectors) | today |
| Open and analyse tables (CSV, TSV, JSON lines) with filters, joins and charts | DATA, DASH | today |
| Write and edit text and Markdown with an assistant | EDIT tile, chat | today |
| Keep a calendar with repeats and reminders | CALENDAR | today |
| Be told about approvals and notes in one inbox | INBOX bell | today |
| Share a folder with a colleague on the network | Shared folders (read-only link with a token) | partial |
| Automate a repeated job (fetch a page, fill a report, notify) | FLOWS, browser steps | partial |

Missing: email and calendar sync with outside services, office file formats (docx, xlsx) beyond text and tables, team accounts across machines.

### 3.5 Home, family and small shops

**Problem.** A spare computer that should be useful for the whole household: music, stories for children, photos, a screen that answers questions.
**Why IYAKKI.** One simple screen with big tiles; each person has their own sign-in; nothing needs an account.

| They can | How | Status |
|---|---|---|
| Play internet radio (Tamil, Hindi, Telugu, Malayalam, English and more) and ask the assistant to add a station | RADIO, chat `radio_search`, `radio_play`, `radio_add` | today for MP3 stations |
| Watch video and play music from a drive or the network | MEDIA | partial |
| Have bedtime stories written and read aloud, with pictures | TALES (story, voice, picture generation on a home GPU server) | partial: voice breaks and slow drawing are being fixed |
| Use as a kitchen or shop display | Screen saver, clock, calendar | today |
| Find what is on the home network (printers, cameras, model servers) | SCAN tile | today |

### 3.6 Home automation with agents

**Honest position.** IYAKKI has **no dedicated smart-home integration yet** (no Home Assistant, MQTT or Zigbee support). What it
has is the general machinery an automation needs: an assistant with tools, a scheduler, HTTP and command steps, browser steps,
notifications and a network scanner.

| They could | How | Status |
|---|---|---|
| Find devices on the home network | SCAN tile | today |
| Call a device or hub through its web API on a schedule | FLOWS (HTTP steps, timers, retries, notify) | today, you write the flow |
| Ask the assistant to build such a flow and test it | Chat plus FLOWS tools | partial |
| Have the assistant react to a calendar entry or an alarm | Calendar, alarms, flows | partial |
| Speak to it | Voice (speech to text, text to speech) | partial: microphone on T2 Macs is off |
| Use Home Assistant, MQTT, Matter, cameras as first-class tools | MCP servers or a new connector | **idea** |

The shortest way to a convincing demo is an MCP server for Home Assistant (or plain REST calls through FLOWS). Decide that before
promising "home automation" in public.

### 3.7 Automation of anything on the internet (and the cloud)

**Problem.** Repeated web chores and cloud tasks that need a person only for approvals.
**Why IYAKKI.** A browser the assistant can drive, flows that run unattended, and approvals that reach your phone-sized inbox.

| They can | How | Status |
|---|---|---|
| Have the assistant browse, read and summarise pages | BROWSER agent tools, `web_fetch`, `web_research` | today |
| Run a repeated web job on a timer (check a price, a status page, a feed) | FLOWS with HTTP and browser steps; `web_feed`, `stock_quote`, `weather` tools | today |
| Be asked before anything is posted, bought or deleted | Approval cards and step-up checks | today |
| Call cloud APIs and CLIs with keys kept in the vault | Vault requests, `http_request`, CLI tile | today |
| Download media with a clear record of what was fetched | `media_download` (tool downloaded by the user) | partial |
| Run the same flows on a server | Cloud mode and Kubernetes image | partial |

Limits to state plainly: sites that block automated browsers, sign-in flows with two-step checks, and terms of service are
the user's responsibility; the OS does not bypass them.

## 4. How the audiences line up (for a one-page summary)

| Audience | Strongest reason | First thing to show |
|---|---|---|
| Students | Private tutor on an old laptop | Ask questions about my own notes, then hear a story read aloud |
| Developers | Assistant inside a safe workspace with Git and a cluster | Ask the assistant to fix a failing test; see the diff and approve |
| Ops and cloud | Cluster desk with approvals | Pod crash loop found and explained, fix proposed, approved |
| Office | Documents stay on the machine | Ask a question across a folder, with sources |
| Home | One screen the whole family can use | Radio and a bedtime story |
| Home automation | General automation tools, honest about gaps | Scan, then a flow that calls a device on a schedule |
| Internet and cloud | Unattended jobs with human approvals | A flow that checks something, then asks me before acting |

## 5. Video plan

**Rules for filming.** Real hardware only (the Mac mini and the MacBook Pro are the tested machines). No faked screens. Show the
approval card in at least two scenes: it is the thing that sets IYAKKI apart. Do not film anything marked **idea** or
**partial** until it has been fixed and tested; the voice breaks and the slow TALES drawing must be fixed first. Do not show
tokens, keys or the pairing code on screen.

**A. The 90-second main video.**

| Time | Scene | Shows | Voice-over idea |
|---|---|---|---|
| 0:00 | A stick goes into an old Mac; the boot menu | Fast boot | "An old computer. A new kind of operating system." |
| 0:10 | The launcher with big tiles | The tiles | "Everything you need, and an assistant inside." |
| 0:20 | Chat: "summarise the notes in my study folder" | Sources cited | "It reads your own files, locally." |
| 0:35 | Chat: "set up this flow" then an approval card | Approval | "It asks before it acts." |
| 0:50 | K8S tile: a failing pod explained | Ops | "It finds problems, you decide." |
| 1:05 | TALES: a story read aloud with highlights; RADIO: Tamil station | Home | "And it is for the whole family." |
| 1:20 | Settings: model on a home GPU server | Choice | "Your models. Your network." |
| 1:30 | Logo and "free, Apache-2.0" | End | "IYAKKI. An OS for agents." |

**B. Short clips per audience (30 to 45 seconds each).** One per section 3: student, developer, ops, office, family, home
automation, internet automation. Each: problem (5 s), one real task with an approval (25 s), the limit stated in a caption (5 s).

**C. A five-minute walkthrough** for developers: install from the USB installer, first sign-in, connect a model, connect Git,
make a change with the IDE, deploy to the local cluster, install a tile app from another computer in developer mode.

**What must be ready before filming**
1. TALES voice without breaks and a smooth screen (in progress).
2. The Git and GIT tile tested on a real Gitea host.
3. A decision on the home-automation story (MCP for Home Assistant, or flows only).
4. A clean install on a spare disk, so the first minute is true.
5. Captions in English first; Tamil subtitles are a good second version for the Tamil Nadu audience.
6. Licence check of everything on screen: fonts, music (use none or the radio's own streams only with permission), icons.

## 6. Things not to say yet

* "Runs on any PC": only two real machines are tested; Wi-Fi and Bluetooth on the Mac mini do not work yet.
* "Works with Home Assistant": not built.
* "Secure": the alpha has not had an outside security review; say "built around permissions and approvals".
* "Plays any video": H.264 and AAC need an optional add-on that is not downloadable yet.
* "Two-step sign-in": designed, not built.
