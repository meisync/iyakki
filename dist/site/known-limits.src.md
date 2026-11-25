# Source of docs/KNOWN-LIMITS.md (hand-written plain text; dist/site/gen-known-limits.py builds the page)
#
# Rules: a line starting with "## " is a section, "- " is an item. The text of an item is for a new user: plain words, no row or issue ids.
# At the end of an item, in braces, go the ids it covers: {KI-001 KI-002 F-VOICE-35}. The generator checks that every id exists, strips
# the braces, and puts the ids into the link table at the end of the page. Lines starting with "#" are comments.
# Every LIMIT and UNPROVEN entry of KNOWN-ISSUES.md appears in the link table whether or not an item covers it.

## Before you start

- **Do not use it for anything you cannot lose.** The installer and the stick writer erase the whole drive you choose, the data volume as a whole is not encrypted (an encrypted private folder per account exists, and your vault is encrypted), and a lost password cannot be recovered by the project. {KI-001 KI-043 KI-048}
- **Do not point it at a production cluster.** The deploy tools refuse clusters that look like production, and the cluster code ran only against fakes and against a small local cluster, never against a managed cloud cluster. {KI-002}
- **It is a preview, and the numbers say so.** Parts are written but have never run on a real machine or against a real service. This page says which; the machine-by-machine list is [`TESTED-HARDWARE.md`](https://github.com/meisync/iyakki/blob/HEAD/docs/TESTED-HARDWARE.md). {KI-058 KI-061 KI-062 KI-059}
- **No model is shipped.** Chat works only after you connect a model in Settings. Only a local Ollama server and a Lemonade server on a home network were used live; OpenAI, Anthropic, Google and OpenRouter connections were tested with fakes and a few trial calls. {KI-023}

## Machines

- **Only a handful of computers have run it.** Four Macs (Mac mini 2014 and 2018, MacBook Pro 2017 and 2020), an HP laptop, a Lenovo laptop and one AMD PC booted it. Any other PC or Mac is untested, not unsupported: run the hardware check entry first and send a hardware report. The 2018 to 2020 T2 MacBooks other than the 2020 model were not tried. {KI-009 KI-012}
- **Apple Silicon Macs are not supported on bare metal.** The way for them is a small virtual machine. That recipe brought up a cluster node once, on one Mac; the rest of it is untried. {KI-010}
- **Raspberry Pi 5 and the AMD KV260 board have images, but nothing has run on a real board yet.** The ARM64 image boots in QEMU only, in slow emulation. {KI-011}
- **The image with firmware files (`hw`) was never booted on a real machine.** {KI-014}
- **Secure Boot with the project's key is proven in QEMU, and it worked on the HP and Lenovo laptops once the key was enrolled.** A newcomer enrolling the key from a public release has not been walked through. {KI-003}
- **The first start on a Mac mini was slow in the images that were timed** (20 to 30 seconds at the logo); the current loader has not been timed there. In QEMU the sign-in screen is up about 1.6 seconds after the kernel starts (2026-10 figure, not re-measured). The old size and speed goals of the project's first specification are not met. {KI-008 KI-016}
- **A 4K monitor on the first USB-C plug of a Mac mini 2018 once showed only 1080p;** another cable gave 4K. The cause is not proven. A retry fix is written and tested with fake files only. {KI-160}
- **Installing:** both installers erase what you choose and are tested on disk images and a few real disks, not on dual boot next to Windows on a real PC. The DISKS tile (mount, eject, partitions) was tested on disk images. {KI-053 KI-054}
- **Updates:** an update written onto a real stick by `iyakki-usb --update` was tested on made-up volumes only; the kept previous version and the automatic fall-back were tested with a folder as the boot partition and in QEMU. The project has no official release key or update channel yet: a signed update file is opened from a folder or a stick, or from an address an administrator types. {KI-970 KI-723 KI-056 KI-750}
- **Memory and battery:** compressed memory (zram) was seen in QEMU only; there is no hibernation; battery reporting and suspend were not recorded as tried on any machine; screen saver and screen off were tried with fakes. {KI-055 KI-049 KI-050}
- **No Windows window build,** and the macOS window runs but leaves out Linux apps, VPN, hotspot, Bluetooth, cameras, USB, the full web engine and more. Both are written for, not tried as a product. {KI-052 KI-078}

## Wi-Fi, Bluetooth, sound and cameras

- **Wi-Fi and Bluetooth on Macs need Apple's firmware,** which the project never ships and you install yourself. The Mac mini 2018's Bluetooth chip has no firmware file in any macOS version, so its Bluetooth is limited. The 2013 to 2015 Macs with a Broadcom BCM4360 Wi-Fi card have no usable driver: use a cable or a USB Wi-Fi adapter. On the Mac mini 2018 the Wi-Fi interface came up with an address, but installing the firmware from a clean state and then joining a network was not recorded there. {KI-005 KI-006 KI-094}
- **Wi-Fi extras are untried on hardware:** enterprise and WEP networks, the WPA3 choice of the hotspot, the Wi-Fi chat tools, the VPN's routing and the Bluetooth network server. The VPN was never brought up on hardware. {KI-680 KI-681 KI-683 KI-690 KI-076}
- **Microphones:** an external USB microphone and the built-in camera work on the 2020 MacBook Pro. Its built-in microphone is off on purpose because capture once hung the machine. The browser's microphone path was tested with fake devices and a PC sound card, never on that Mac. {KI-013 KI-069}
- **Sound follows a monitor you plug in or out,** but that was tested with made-up data, not yet confirmed on the Mac mini. Video through the mixer ran with a fake player. {KI-180 KI-920 KI-085 KI-007}
- **Cameras:** the camera closes when its tile leaves the screen, and a covered camera shows a question instead of a black picture; both fixes are written and tested but the Lenovo's camera-light fix is not confirmed. Choosing a camera by name and describing alerts with a real vision model were tried with fakes. {KI-080 KI-082 KI-093 KI-622 KI-770}
- **Several things were found on one machine and not re-run:** the browser-sound path, the Bluetooth headline, driver downloads with no network, and the laptop panel's refresh rate. {KI-083 KI-084 KI-210}

## Browser and media

- **Video: H.264 and AAC do not play by default** (patent policy; nothing is shipped). YouTube plays in VP9 with Opus. An optional audio and video add-on, built on your own machine at your request, is the way to get the rest; TikTok needs it. Netflix and Prime Video need a Widevine module that is not shipped and do not play. {KI-017 KI-018 KI-070}
- **The built-in reader shows Chinese, Japanese, Korean and emoji as boxes** and does not decode video. For modern sites press `F` in the Browser for the full web engine, which needs the web image or helper. {KI-019}
- **Radio:** MP3 and HLS with MP3 play with in-house code; AAC needs the add-on. {KI-022}
- **Pages:** scripted pages get clicks, keys, timers and forms but not other pointer events; PDF form filling is limited to common field types; some PDF fonts are not read; the developer tools show reader pages only. {KI-715 KI-716 KI-960 KI-140}
- **Blocking and routing:** ad and tracker blocking was never run against the real full engine; a Browser tunnel is chosen by hand, not by site. {KI-021 KI-113}
- **Saved pages are not refreshed by themselves.** {KI-746 KI-962}
- **No picture generation is shipped** (only macOS Ollama image models, which IYAKKI does not run), **and no speech engines are shipped** (their licences are their own); IYAKKI uses what the computer already has, or the browser's voice. Story pictures from real models are unjudged. {KI-031 KI-032 KI-033}

## The assistant and its tools

- **The assistant's helper rules are mostly English.** Refusal retry, stall guard, advice detection and similar helpers use English phrase tables; German, French, Spanish and Hindi tables exist but were not tried with real models. Chat messages are limited (20,000 characters; the story box 4,000) and longer text is cut with a notice. {KI-039 KI-700 KI-038}
- **Small local models are unreliable at planning and at writing manifests.** In trials, one 8-billion-parameter model followed a planning rule only partly and wrote a complete Kubernetes manifest every time with a template, while a larger one needed more turns. Pick a model with care. {KI-099 KI-100}
- **Claude Code's own shell cannot be switched off by IYAKKI;** common long commands are refused but the vendor's shell stays. Claude Code and Copilot sign-in were tried on one Mac. {KI-025 KI-024}
- **Approvals:** session grants apply to chat and the IDE chat only; INBOX approvals and parallel workers ask every time. Lines typed into an interactive developer-mode terminal are not recorded word for word, on purpose. {KI-035 KI-103}
- **Real services were mostly not used:** dashboards over real Prometheus, Elasticsearch or ClickHouse; the private store and Gitea on a real cluster; payments for paid apps (the code is written, no provider was connected); team API-key integrations; the reranking call; flows with a Thrift node or a public webhook; a remote runner. {KI-030 KI-029 KI-045 KI-646 KI-744 KI-047 KI-735}
- **Teaching lessons (LEARN) were tried only with scripted models,** never with a real one. {KI-074}

## Accounts, files and security

- **Accounts are not Linux users,** and files in `/home` belong to the system. Existing home folders may be readable by other programs on the machine: the repair that makes them private is written and tested but was not yet seen working on a device. {KI-040 KI-095}
- **There is no automatic sign-in,** on purpose, so your password opens your vault. {KI-072}
- **The root system lives in memory** (layers are kept on the boot or data volume); changes outside the data volume are lost at restart. {KI-042}
- **The scheduled backup is not encrypted unless you name a passphrase file,** because an unattended run cannot ask. {KI-401}
- **There is no official store yet:** its key and catalogue address do not exist, so the portal says "coming soon"; you can add up to eight stores yourself. A withdrawn package is marked and offered for removal but not switched off. {KI-046 KI-037 KI-034}
- **The browser front end uses a self-signed certificate** you must accept. {KI-043}
- **The firewall** (incoming blocked by default) was tested on fakes and in QEMU; the tenant egress rules are not applied by it. {KI-041 KI-643}

## Developer tools and servers

- **Tunnels and developer mode** were tried with a fake client and on a few real machines; a jump host may fail on the images, an IPv6 address is not accepted in a tunnel, and a host that git also uses may show its fingerprint again after the last tunnel is deleted. {KI-111 KI-112 KI-114 KI-115 KI-026}
- **The local registry, cluster add-ons and Quick install** have tests with fakes; a real cluster run is open. {KI-028 KI-004}
- **Shared folders** have no HTTPS and no network-name (mDNS) address. {KI-760}

## The small apps built on the new SDK

- **Chat, Edit, Settings, Runs and the Kubernetes tile are small first versions.** For example the Chat tile cannot pick tools or resume history, the Edit tile saves files up to 12 KiB, the Settings tile has four safe switches, and the Runs tile answers one question at a time. The full apps of the desktop are unchanged. {KI-823 KI-766 KI-765 KI-822 KI-764 KI-861 KI-860 KI-761 KI-990 KI-991}
- **Some apps still read the kernel directly** and move onto the core in later releases. {KI-870 KI-890 KI-910 KI-911 KI-984}
- **A crash inside an app's own thread ends that thread without restart.** {KI-121}

## Experimental: switched off unless you turn it on, and labelled in the product

- **The wake phrase "My IYAKKI"** (the voice assistant answering when you say its name): off by default, labelled experimental everywhere, until real voices and room noise have been measured. {F-VOICE-35 F-VOICE-46 F-VOICE-60}
- **The Gemini voice assistant and the Gemini provider:** optional, off by default, send audio and text to Google only after your explicit yes. {F-GEMVOICE-01 F-VOICE-41 F-ASSIST-52 KI-460}
- **The client family: Connect a client** (a browser page, the `iyakki-connect` window program, sound, clipboard and camera to a client) is experimental and off unless you switch it on. It ran in a test browser, not on the project's devices. {F-CORE-14 F-CLIENT-01 F-CLIENT-02 F-CLIENT-03 F-CLIENT-04 F-CLIENT-05 F-CLIENT-06 F-CLIENT-07 F-CLIENT-08 F-CLIENT-09 KI-087 KI-088 KI-089}
- **The SDK versions of Edit and Cli:** the browser Edit tile is labelled "scratch" and is small; the Cli tile has no browser version yet. Both are experimental until real use shows them solid. {F-WEB-54 F-SDK-34 KI-765 KI-732 KI-990 KI-991}
- **WASM tiles** (Settings, Developer mode) are off by default. {F-WASM-09 KI-110}
- **Fleet and the model router** (Settings, in the Network group) are marked experimental. {F-FLEET-09 F-ROUTER-07}
- **Compressed memory behind one switch, browser sound through the mixer, and the built-in microphone of a T2 Mac** are experimental switches, off by default. {F-BOOT-31 F-SND-15 F-SND-20}

## Parked: decided to wait, not dropped

- **Our own signed boot shim** (so no key enrolment would be needed) needs a Microsoft developer account and a public review; it waits. {F-BOOT-11}
- **A separate tile crate for faster builds** (the gain measured small) waits until a feature needs it. {F-ARCH-08}
- **Browser push notifications from a real push service, the official store key and catalogue, payments through a provider, and the owner-run relay** wait for decisions that only the owner can make. {KI-725 KI-046 KI-045}
- **Bare-metal Apple Silicon, other ARM boards and a Windows window build** are not planned for the preview. {KI-010 KI-052}
