#!/usr/bin/env python3
"""Regenerates the test plans from retriage-2026-11-06.tsv (class B rows) and FEATURES.md.
Run from the repo root:  python3 docs/project/test-plans/generate_plans.py
Every class B row lands in exactly one plan; the script stops if the count is not right."""
import csv, re, os, sys, collections

ROOT = os.getcwd()
OUT = os.path.join(ROOT, "docs/project/test-plans")
TSV = os.path.join(ROOT, "docs/project/retriage-2026-11-06.tsv")

names = {}
for line in open(os.path.join(ROOT, "FEATURES.md"), encoding="utf-8"):
    if line.startswith("| F-"):
        c = [x.strip() for x in line.split(" | ")]
        names[c[0].lstrip("| ").strip()] = c[1]

rows = []
with open(TSV, encoding="utf-8") as f:
    for r in csv.DictReader(f, delimiter="\t"):
        if r["class"] == "B":
            rows.append(r)

SETS = {
 "apple": {"F-BOOT-08", "F-APPLE-01", "F-SHELL-20"},
 "bios": {"F-BOOT-01", "F-UPD-02", "F-BOOT-33", "F-REL-01", "F-SELFTEST-01", "F-UPG-06", "F-BOOT-35"},
 "acc": {"F-DISP-07", "F-DISP-10", "F-KBD-06", "F-KBD-07", "F-KBD-17", "F-KBD-19", "F-SETUP-06", "F-SHELL-13",
         "F-SHELL-35", "F-SHELL-37", "F-SHELL-39", "F-SHELL-47", "F-SHELL-48", "F-SHELL-50", "F-SHELL-52",
         "F-SHELL-53", "F-SHELL-54", "F-SHELL-55", "F-FLOW-24", "F-FLOW-39", "F-BROW-89", "F-BROW-76",
         "F-CHAT-23", "F-TALES-19", "F-VOICE-08", "F-VOICE-21", "F-VOICE-22", "F-LDR-04", "F-DISP-11"},
 "phone": {"F-SHELL-49", "F-EXTA-09", "F-EXTA-14", "F-EXTA-15", "F-INBOX-07", "F-INBOX-08", "F-WEBSVC-03",
           "F-CAM-14", "F-DEVT-05"},
 "macmini_sys": {"F-HOTSPOT-09", "F-HOTSPOT-10", "F-HOTSPOT-11", "F-HOTSPOT-13"},
 "pcamd": {"F-HOTSPOT-17", "F-DASH-04", "F-AIHW-01", "F-KBD-16", "F-KBD-05"},
 "nvidia": {"F-GPU-01", "F-GPU-04", "F-AIHW-02"},
 "k3s": {"F-EXTA-13", "F-MSG-06", "F-GIT-17", "F-AIHW-04", "F-IDE-02", "F-CLI-03", "F-LINAPP-09", "F-FLOW-05",
         "F-FLOW-31", "F-BUG-01", "F-DASH-11", "F-DASH-12", "F-DASH-13", "F-DASH-14", "F-DASH-15", "F-DASH-17",
         "F-DASH-18", "F-CLUSTER-05", "F-IDE-19", "F-IDE-21", "F-ASSIST-23", "F-ASSIST-25", "F-ASSIST-26", "F-ASSIST-29"},
 "accounts": {"F-AI-02", "F-AI-12", "F-STUDY-01", "F-BROW-39", "F-BROW-44", "F-TALES-11", "F-TALES-15", "F-MSG-02",
              "F-MSG-05", "F-INBOX-06", "F-APPSTORE-06", "F-APPSTORE-23", "F-QA-14", "F-CLI-09", "F-GIT-16"},
 "live": {"F-MEDIA-03", "F-MEDIA-04", "F-DASH-08", "F-SDK-03", "F-VOICE-15", "F-VOICE-16", "F-VOICE-17",
          "F-FLOW-06", "F-FLOW-20", "F-FLOW-21", "F-FLOW-22", "F-FLOW-25", "F-FLOW-27", "F-FLOW-28", "F-FLOW-29",
          "F-FLOW-30", "F-FLOW-33", "F-FLOW-34", "F-SETUP-10"},
}
SYS_PREFIX = {"BOOT", "DISP", "SND", "NET", "HOTSPOT", "DRIVERS", "FW", "KBD", "SCRN", "ZRAM", "SYS", "STARTUP",
              "LDR", "CLOCK", "PWR", "DEV", "DEVMODE", "DEVT", "UPD", "UPG", "CAM", "HW", "DOCTOR", "CHECKUP",
              "ENGLOG", "VAULT", "BATT", "GPU", "SDK", "ZRAM", "K3S"}
def num(i): return int(i.rsplit("-", 1)[1])
def pre(i): return i.split("-")[1]

def assign(r):
    i, nx = r["ID"], r["next"]
    p = pre(i)
    if i in SETS["apple"]: return "apple-silicon-vm"
    if i in SETS["bios"]: return "pc-bios-and-qemu"
    if p in ("INST", "DISKS", "BACKUP", "LOSS") or i in ("F-PKG-06", "F-STORE-09", "F-DIAGSHARE-02"):
        return "usb-stick-and-install"
    if i in SETS["macmini_sys"]: return "mac-mini-2018"
    if "MacBook" in nx and "Mac mini" not in nx and p not in ("K8S",) and i not in SETS["phone"] | SETS["accounts"] | SETS["live"] | SETS["acc"]:
        return "macbook-pro-t2"
    if i in SETS["acc"]: return "accessibility-and-touch"
    if i in SETS["phone"] or p in ("HOTSPOT", "NAME", "AUTH", "SHARE", "WEBDEV") or (p == "WEB" and 13 <= num(i) <= 20):
        return "phone-and-network"
    if i in SETS["k3s"] or p in ("K8S", "K3S", "CLUSTER", "SBX", "APPS") or (p == "SETUP" and 9 <= num(i) <= 16) or (p == "BROW" and 81 <= num(i) <= 84):
        return "k3s-and-cluster"
    if p in ("CLOUD", "TCFG", "CONN", "WEB"): return "cloud-tenants-and-web"
    if i in SETS["accounts"] or p in ("EXTA", "GIT", "SSHTUN", "VPN"): return "real-accounts-and-keys"
    if i in SETS["live"] or p in ("ASSIST", "IDE", "STUDY", "LAB", "MODELHOST", "TOK", "AI") or (p == "TALES" and i in ("F-TALES-13","F-TALES-16","F-TALES-18","F-TALES-20")):
        return "live-models"
    if p in ("BROW", "RADIO", "MEDIA", "ENGLOG"): return "browser-real-sites"
    if i in SETS["nvidia"] or re.search(r"4090|NVIDIA|nouveau|Intel iGPU", nx): return "pc-uefi-intel-nvidia"
    if i in SETS["pcamd"] or (re.search(r"\bPC\b|laptop", nx) and "Mac" not in nx): return "pc-uefi-amd"
    if p in SYS_PREFIX: return "mac-mini-2018"
    return "mac-mini-2018-apps"

# ordering inside a plan: problems found earliest first
STAGE = ["BOOT", "LDR", "CLOCK", "DISP", "KBD", "SCRN", "SND", "NET", "HOTSPOT", "DRIVERS", "CAM", "HW", "BATT",
         "UPD", "UPG", "DEV", "DEVMODE", "FW", "SYS", "ZRAM"]
def key(r):
    p = pre(r["ID"])
    return (STAGE.index(p) if p in STAGE else 50, p, num(r["ID"]))

# ---- log hints ----
def logs(r):
    i, nx, p = r["ID"], r["next"], pre(r["ID"])
    base = "`iyakki dev diag` output"
    t = nx.lower()
    if i in ("F-DISP-12", "F-DISP-09", "F-KBD-18") or "touch" in t and p in ("DISP", "KBD"):
        return base + " and the `touch-recover:` and `touchprobe` lines from `iyakki dev logs --system`"
    if p in ("SND", "VOICE", "RADIO", "TALES", "MEDIA") or re.search(r"sound|listen|audio|speaker", t):
        return base + ", `iyakki dev logs --system`, and the sound lines from `iyakki dev dmesg`"
    if p in ("BOOT", "LDR", "SYS", "ZRAM", "CLOCK", "UPD", "UPG", "LOSS", "STORE"):
        return base + ", `iyakki dev dmesg`, and the newest `kmsg-<boot-id>.log` in /var/lib/iyakki/log"
    if p in ("K8S", "K3S", "CLUSTER", "SETUP", "DASH", "SBX", "APPS", "CLOUD", "TCFG"):
        return base + ", `iyakki dev logs --system`, and `iyakki dev kube logs <pod>` for the pod that failed"
    if p in ("BROW", "WEBDEV", "ENGLOG", "WEB", "CONN"):
        return base + " and the browser log file `engine.log` from /var/lib/iyakki/log (check it for secrets first)"
    if p in ("HOTSPOT", "NET", "NAME", "DRIVERS"):
        return base + ", `iyakki dev logs --system`, and `iyakki dev dmesg` (Wi-Fi and firmware lines)"
    if p == "FW":
        return base + " and `iyakki dev logs --system`; say which firewall rules were shown on the page"
    return base + " and `iyakki dev logs --system`; a photo of the screen if it is a display problem"

# ---- time estimate ----
def minutes(r):
    t = r["next"].lower()
    if "overnight" in t: return 60
    if "drain" in t: return 30
    if "five minute" in t: return 10
    if re.search(r"cold boot|several cold|reboot|restart", t): return 8
    if re.search(r"install|build|rebuild|deploy|clone|restore|update", t): return 10
    return 5

EXTRA = {  # facts from 2026-11-06 and first-look notes, shown under the check
 "F-DISP-12": "Today's fact: on the Mac mini the touch panel stays silent after a cold boot and the old recovery ladder failed. This step is the new manual 'Reset touch screen' (it retrains the display link). Power off fully, boot cold, do NOT touch the screen with the mouse first, press Reset touch screen, then touch. Attach the `touch-recover:` log lines either way.",
 "F-DISP-09": "Do the cold boot with each iyakki.touchscreen option, one at a time. Write down which option, and whether touch worked before and after Reset touch screen.",
 "F-KBD-18": "Run this right after F-DISP-12 while the cold-boot state is still fresh. Attach the `touch-recover:` and touchprobe/touchfix log lines.",
 "F-BROW-61": "Verified on the Mac mini today (browser sound with the radio playing). Repeat on the MacBook; a pass on the Mac mini does not count here.",
 "F-SND-15": "Browser sound with the radio playing was verified on the Mac mini today. Re-check after any new image.",
 "F-SND-03": "First look wanted: the new per-sound volume rows. Move each slider (radio, browser, cues) alone and check the others do not change.",
 "F-SND-21": "First look wanted: the new per-sound volume rows on a real screen.",
 "F-VOICE-19": "First look wanted: per-sound volume for login, logout and notification cues.",
 "F-SHELL-58": "First look wanted: the Lock menu on a real screen. Check Lock, unlock with the password, and that the vault opens again.",
 "F-SHELL-59": "First look wanted: the time zone question after the first sign-in. Check the wording, the default shown, and that the clock is right after you answer.",
 "F-BROW-88": "First look wanted: tab restore. Open three tabs, close the browser tile, open it again; also try after a reboot.",
 "F-AUTH-01": "First look wanted: the AUTH tile has not been seen on a real screen yet. Compare the codes with a phone authenticator for at least two full 30-second turns.",
 "F-AUTH-03": "First look wanted: AUTH tile on a real screen (vault locked and unlocked).",
 "F-AUTH-04": "First look wanted: AUTH tile on a real screen; point the camera at an otpauth QR on a second screen or paper.",
 "F-AUTH-05": "First look wanted: AUTH tile. Time the wipe with a stopwatch.",
 "F-BROW-04": "Also repeat the laptop freeze (see 'Known open problem' at the top of this plan): load youtube.com with NO network and photograph the screen.",
 "F-BROW-50": "Also repeat the laptop freeze (see 'Known open problem' at the top of this plan).",
 "F-BROW-59": "Also repeat the laptop freeze (see 'Known open problem' at the top of this plan) with the GPU path on.",
}
FREEZE = """## Known open problem: freeze after opening youtube.com with no network

A laptop (AMD Radeon 610M, 2560x1600) froze about 20 seconds after youtube.com was opened with no network.
Repeat it before anything else on this machine:

- [ ] EXTRA-FREEZE Repeat the freeze: boot the image, make sure there is no network (unplug Ethernet, Wi-Fi off), open the browser tile, open youtube.com, wait 60 seconds. -> expect the page shows an error or a retry message and the screen keeps responding; if it freezes, **photograph the screen** (the whole screen, including any clock or cursor), note how many seconds after opening it froze, and whether the mouse still moved; if it fails, copy: after the power cycle, `iyakki dev diag` output, the newest `kmsg-<boot-id>.log` and `engine.log` from /var/lib/iyakki/log, and `iyakki dev logs --system`.
  Result: ______ (pass / fail / note)

(This extra step has no row ID and is not part of the 567-row count.)
"""

PLANS = collections.OrderedDict()
def plan(key, title, who, need, safe, order, extra=""):
    PLANS[key] = dict(title=title, who=who, need=need, safe=safe, order=order, extra=extra)

plan("mac-mini-2018", "Mac mini 2018 (T2): system, boot, display, touch, sound, network, update",
 "Main test machine. Anyone with the Mac mini, a monitor (touch monitor for the touch rows), keyboard and mouse.",
 ["Mac mini 2018 (T2) with a monitor; a touch monitor for touch rows; USB or Bluetooth keyboard and mouse.",
  "The IYAKKI image on a USB stick (or already installed on the Mac mini). Check the version in Settings -> About.",
  "An update pack on a second USB stick for the update and rollback rows.",
  "Ethernet cable and a Wi-Fi network you may use. For hotspot rows see phone-and-network.md.",
  "A second computer on the same network with the IYAKKI dev tools (for `iyakki dev ...`), paired once.",
  "A USB microphone and speakers/HDMI audio for sound rows. A USB camera for camera rows."],
 "Boot from the stick. Do not click Install. Nothing is written to the Mac internal disk unless a step says so in bold. Rows that change the boot order, update the installed copy or crash the machine are marked in bold; do them last.",
 ["Boot rows (splash, timings): they show a bad image early.", "Display and touch (cold boot first: power off for 30 seconds, then boot, and try touch BEFORE doing anything else).",
  "Sound, then network (cable, Wi-Fi, Bluetooth), then camera.", "Firewall and developer mode.", "Update and rollback, then crash and power-pull rows last."],
 "")
plan("mac-mini-2018-apps", "Mac mini 2018: apps on the real screen",
 "Anyone with the Mac mini and the image installed or booted. No accounts needed.",
 ["Mac mini 2018 with monitor, keyboard, mouse; the IYAKKI image; network for some rows.", "A USB stick (for rows that mention one).", "Sample files the row names (for example the 130-note inbox file in the repo test data)."],
 "Boot from the stick; do not install. Sign in as admin. Nothing is erased unless a step says so in bold.",
 ["Shell, top bar, bell, inbox (quick, many problems show here).", "Store and Setup.", "Flows, Git, Linux windows, Fixes and the rest."])
plan("macbook-pro-t2", "MacBook Pro (T2: 14,1 / 16,2): battery, keyboard, trackpad, camera, sound",
 "Anyone with a T2 MacBook Pro and a USB-C stick.",
 ["MacBook Pro with a T2 chip (MacBookPro14,1 or 16,2; write down the model with `iyakki dev info`).", "The IYAKKI image on a USB-C stick; charger; a USB-C hub with Ethernet if Wi-Fi firmware is missing.",
  "For camera rows: the Apple camera file the row names, supplied by the person (never shipped)."],
 "Boot from the stick. Do not click Install. The battery rows drain the battery: keep the charger close and mark those steps as bold-risk (the machine will shut down at 3 percent).",
 ["Boot and keyboard/trackpad first (without them nothing else can be tested).", "Sound, Wi-Fi, camera.", "Battery drain last."])
plan("pc-uefi-amd", "UEFI PC (AMD or any other): boot, Wi-Fi cards, firmware, laptop keys",
 "Anyone with a spare UEFI PC or laptop, preferably with AMD graphics.",
 ["A UEFI PC or laptop that may be booted from USB (Secure Boot setting noted).", "The IYAKKI PC image on a USB stick.", "Network cable. For firmware rows: a PC with the matching extra card (Wi-Fi/Ethernet) named in the row.", "A second Wi-Fi card for the hotspot row."],
 "Boot from the stick; do not install. Nothing on the PC disk is touched unless a step says so in bold.",
 ["Boot and display.", "Network and firmware cards.", "Laptop keys and suspend.", "Everything else."],
 FREEZE)
plan("pc-uefi-intel-nvidia", "UEFI PC with Intel and/or NVIDIA graphics",
 "Anyone with a PC that has an NVIDIA card (4090 PC) or an Intel iGPU.",
 ["PC with an NVIDIA card (nouveau/NVK path) and/or an Intel iGPU machine.", "The IYAKKI PC image on a USB stick.", "Network cable."],
 "Boot from the stick; do not install. Nothing on the PC disk is touched unless a step says so in bold.",
 ["Boot, firmware, graphics entry.", "GPU in a pod (needs k3s running).", "Hidden-tool reasons on the tiles page."])
plan("pc-bios-and-qemu", "BIOS PC and QEMU first: broken update fall-back, release file, self test",
 "Anyone with a build PC (QEMU) and an old BIOS PC.",
 ["A build PC with QEMU and the built image (the developer gives you the commands).", "An older PC that boots in BIOS (legacy) mode.", "A second stick."],
 "QEMU runs never touch real disks. On the BIOS PC boot from the stick and do not install.",
 ["QEMU rows first (cheap), then the BIOS PC, then the UEFI PC."])
plan("apple-silicon-vm", "Apple Silicon Mac: VM and native window",
 "Anyone with an M-series Mac.",
 ["An M-series Mac (macOS), UTM installed.", "The aarch64 image.", "Ollama installed for the Metal row."],
 "Everything runs in a VM or a normal window; nothing outside the VM is changed.",
 ["VM boot first, then the native window, then the Metal model."])
plan("phone-and-network", "Phone and home network: hotspot, QR, authenticator, remote web access, names",
 "Anyone with phones (iPhone and Android), a second computer and a home Wi-Fi router.",
 ["The Mac mini (hotspot host) and the IYAKKI image; an iPhone and an Android phone; a Windows or macOS laptop; a home router you may look into.", "A phone authenticator app and a safe test QR code.", "Browsers: Chrome, Firefox, Safari."],
 "Only the settings you switch on in the steps change. Turn Remote access, Announce and the hotspot off again at the end.",
 ["Hotspot join and DHCP.", "Names (.local) and Announce.", "Remote access and the web client in browsers.", "QR and authenticator.", "Sharing and inbox in browsers."])
plan("real-accounts-and-keys", "Real accounts and keys: providers, external agents, git hosts, Matrix, VPN, SSH",
 "Anyone who can create throwaway accounts. See 'what a throwaway account needs' below.",
 ["Provider keys: OpenAI, Claude, Gemini, a free NVIDIA key (each a throwaway key with a spend limit).", "External agent accounts: Claude Code, Copilot CLI.", "Git hosts: a Gitea, a GitHub and a GitLab account with a token limited to a throwaway repo.", "A Matrix server (Tuwunel or Synapse) and a test room; an SSH host you may log into; a WireGuard server and a provider config file.", "A device running the image (Mac mini or PC) with network."],
 "Use throwaway accounts only. Revoke every key and token after the session. Never paste a key into a bug report: the logs must not hold one (that is itself a check).",
 ["Keys and sign-in screens (fast, find scope errors).", "Git hosts.", "External agents.", "Matrix, SSH, VPN."],
 """## What a throwaway account needs

| Service | What to create | Limit it to |
|---|---|---|
| OpenAI / Claude / Gemini / NVIDIA | one API key each | a small spend limit; delete after |
| Claude Code, Copilot CLI | a test login | no company data; log out at the end |
| Gitea / GitHub / GitLab | a test user and a token | one private throwaway repo; read and write on that repo only |
| Matrix | a test user and a room | a private room with only you |
| SSH host | a user with a test key | a home folder with nothing important |
| WireGuard / OpenVPN | a test peer or provider file | a server that does not carry real traffic |
""")
plan("live-models", "Live models: local Ollama / Lemonade and the apps that use a model",
 "Anyone with a PC or Mac running Ollama (qwen3:8b, qwen2.5-coder:14b) or Lemonade.",
 ["The device with the image, plus a model server on the LAN: Ollama with qwen3:8b and qwen2.5-coder:14b, and Lemonade (chat, image, whisper, speech models).", "A speaker or headphones for rows that read aloud. A camera for vision rows.", "For some rows, the dev k3s cluster (see k3s-and-cluster.md)."],
 "Models only answer questions; nothing on the machine is erased. Autopilot rows can change files in a test workspace: use a throwaway workspace.",
 ["Model discovery and Settings -> Models.", "Chat, IDE and assistant runs.", "Flows, Learn, Lab, Tales, Voice.", "Long runs (overnight) last."])
plan("k3s-and-cluster", "k3s and clusters: Kubernetes tile, Setup services, store, dashboards",
 "Anyone who can start k3s on the Mac mini and has a PC for a second view.",
 ["Mac mini 2018 with the image and network; k3s startable from Settings.", "A second computer on the LAN (to test that ports are closed or open from outside).", "A second real cluster for the Dev cluster rows and a cluster with a GPU node for the badge row.", "Registry options and Gitea from Setup (needs internet)."],
 "k3s runs in its own sandbox. 'Remove all', 'Delete' and 'Allow changes' rows destroy test data in k3s only: they are marked in bold.",
 ["Start k3s and the K8S tile.", "Setup services (Prometheus, Neo4j, Gitea, ClickHouse).", "Store, registry, dashboards.", "Break and repair rows last."])
plan("cloud-tenants-and-web", "Cloud tenants and the web client: pods, ingress, Files, Git, Schedules",
 "Anyone with a real cluster (cloud mode) and browsers.",
 ["A real cluster with ingress and a tenant (cloud mode), reachable by DNS name; a machine with ports 80/443 for the listener row.", "Browsers on a phone and on a desktop; a second user account.", "A real provider key for the team-config rows (see real-accounts-and-keys.md)."],
 "Use a test tenant. Rows that restore snapshots or rotate keys are marked in bold.",
 ["Tenant opens and signs in.", "Pages: Files, Git, Schedules, Flows.", "Two tenants, switching.", "Rotation, snapshots, replicas last."])
plan("usb-stick-and-install", "USB stick, install and storage loss",
 "Anyone with spare USB sticks and a spare computer.",
 ["Two or three spare USB sticks (one FAT32, one ext4, one 64 GB); a spare NVMe/SD if possible.", "A spare PC, the Mac mini, and a Windows laptop disk (read-only look).", "The installer image."],
 "Use a SPARE stick and a SPARE disk only. Install rows are **destructive for the target disk**; each is marked in bold. Never point an install at a disk with data you need.",
 ["Read-only planner rows.", "Stick rows (list, mount, eject).", "Backup and restore.", "Drive loss (cable pull).", "Install rows last."])
plan("browser-real-sites", "Real sites: web engine, media, radio",
 "Anyone with the Mac mini (or a PC) and internet.",
 ["Device with the web image and internet; speakers; a second display optionally.", "Accounts are not needed. Do not sign in to a real personal account.", "Radio station list and sample MP3/WebM files from the repo."],
 "Only the browser profile of the test machine is used. Use a private test profile.",
 ["Page loads on Google and YouTube.", "Sound and video.", "Ads, downloads, add-on installs.", "Page privacy rows."])
plan("accessibility-and-touch", "Accessibility, touch, keyboard and layout on real screens",
 "Anyone, including a first-time user. Best done by someone who did not build the screens.",
 ["Mac mini plus a touch monitor, a second display, a MacBook with trackpad, a PC laptop with touch if available.", "A keyboard and mouse; a first-time user for the Esc/Close row; headphones."],
 "Only settings change. Nothing is erased.",
 ["Pointer and wheel.", "Touch: tap, drag, hold.", "On-screen keyboard.", "Read-aloud and word lighting.", "Layout at small sizes."])

def fmt(r):
    i = r["ID"]
    nm = names.get(i, i)
    nx = r["next"].strip().rstrip(".")
    bold = ""
    if re.search(r"crash|panic|power|pull|destroy|delete|remove all|write a plan|dual-boot|install to|cold-boot", nx, re.I):
        bold = " **(can destroy test data or stop the machine: read the step twice)**"
    why = r["why"].strip().rstrip(".")
    line = (f"- [ ] {i} {nm}: do this: {nx}{bold} -> expect this: it works as the row says; the open point is: {why}; "
            f"if it fails, copy: {logs(r)}.")
    out = line + "\n  Result: ______ (pass / fail / note)\n"
    if i in EXTRA: out += f"  Note: {EXTRA[i]}\n"
    return out

COMMON_LOGS = """## How to collect logs

Use developer mode on the test machine (Settings -> Developer mode; approve the pairing on the machine). From the paired computer:

- `iyakki dev diag` : one bundle with state and recent logs. Attach it to every failed row. It holds no secrets (a row checks this; if you see a key in it, tell the owner and do not share it).
- `iyakki dev logs --system` : the system log. Attach the lines around the failure.
- `iyakki dev dmesg` : the kernel log, for boot, Wi-Fi, sound and USB problems.
- The data-folder logs: files under `/var/lib/iyakki/log` (for example `engine.log` for the browser and `kmsg-<boot-id>.log` after a crash). On a machine with no network, copy them to a USB stick from the Files tile.
- A photo of the screen helps for any display, freeze or layout problem. Say how long after the action it happened.
"""

def main():
    groups = collections.defaultdict(list)
    for r in rows: groups[assign(r)].append(r)
    total = sum(len(v) for v in groups.values())
    if total != len(rows) or len({r["ID"] for r in rows}) != len(rows):
        sys.exit("count mismatch")
    for k in groups: groups[k].sort(key=key)
    for k in groups:
        if k not in PLANS: sys.exit("no plan text for " + k)
    summary = []
    for k, p in PLANS.items():
        rs = groups.get(k, [])
        mins = 20 + sum(minutes(r) for r in rs)
        summary.append((k, p, len(rs), mins))
        body = [f"# {p['title']}\n", f"Plan file: `{k}.md` | Checks: {len(rs)} | Estimated time: about {mins} minutes ({mins/60:.1f} hours)\n",
                "Use `RESULTS-TEMPLATE.md` to report. Lines marked with (A), (code) or 'add ...' in the row text are developer tasks: skip that part and test the rest.\n",
                "## What you need\n"] + [f"- {x}" for x in p["need"]]
        body += ["", "## Safe start", "", p["safe"], "", COMMON_LOGS, "## Order of checks", ""]
        body += [f"{n}. {x}" for n, x in enumerate(p["order"], 1)]
        body += ["", "The checks below are already in this order. Do not skip a failed early row: write it down and go on.", ""]
        if p["extra"]: body += [p["extra"], ""]
        body += ["## Checks", "", "Fill the Result line: pass, fail or note. For a fail, attach the files named in the row.", ""]
        body += [fmt(r) for r in rs]
        open(os.path.join(OUT, k + ".md"), "w", encoding="utf-8").write("\n".join(body) + "\n")
    # index
    hw = {k: p["who"] for k, p, _, _ in summary}
    idx = ["# Beta test plans: index\n",
           "These plans turn the 567 'class B' rows (code exists, needs a device or a real service) from `docs/project/retriage-2026-11-06.tsv` into checklists. "
           "Plans are grouped by what the tester has in hand, not by feature. Every row is in exactly one plan.\n",
           "| Plan | Checks | Minutes | Who and what hardware |", "|---|---|---|---|"]
    for k, p, n, m in summary:
        idx.append(f"| [{k}.md]({k}.md) | {n} | {m} | {p['who']} |")
    idx.append(f"| **Total** | **{sum(s[2] for s in summary)}** | **{sum(s[3] for s in summary)}** | |")
    idx += ["", "Minutes are a rough guess (5 minutes a check, more for installs and long runs, plus 20 for setup). "
            "Reading the first-time notes: the Mac mini touch rows, the Lock menu, the time zone question, tab restore, the AUTH tile and the per-sound volume rows have not been seen on a real screen yet; look at them first.\n",
            "## Suggested order for a first week", "",
            "1. mac-mini-2018.md (the main machine; start with the cold-boot touch rows and Reset touch screen).",
            "2. mac-mini-2018-apps.md, accessibility-and-touch.md (same machine, same day).",
            "3. phone-and-network.md, usb-stick-and-install.md (needs phones and spare sticks).",
            "4. macbook-pro-t2.md, pc-uefi-amd.md (repeat the laptop freeze), pc-uefi-intel-nvidia.md, pc-bios-and-qemu.md, apple-silicon-vm.md.",
            "5. browser-real-sites.md, live-models.md, k3s-and-cluster.md.",
            "6. real-accounts-and-keys.md, cloud-tenants-and-web.md (need accounts and a cluster).", "",
            "## Other files", "", "- [RESULTS-TEMPLATE.md](RESULTS-TEMPLATE.md): how to report.",
            "- [generate_plans.py](generate_plans.py): rebuilds all plans from the TSV and FEATURES.md and checks the 567 count.", "",
            "## How results flow back", "",
            "1. The tester fills the Result line of each check and copies the template from RESULTS-TEMPLATE.md.",
            "2. The owner pastes the filled template (and the attached logs) to an assistant.",
            "3. The assistant updates the FEATURES.md rows: a pass moves the row to verified on that hardware (with the date and image version in the notes); a fail adds the symptom and the log reference to the row and to KNOWN-ISSUES; a note is kept as text.",
            "4. The matrix test (`feature_matrix.rs`) must still pass after the update. Failed rows stay in the next plan run.",
            "5. Only the owner decides when a row counts as done for the beta.", ""]
    open(os.path.join(OUT, "INDEX.md"), "w", encoding="utf-8").write("\n".join(idx))
    for k, p, n, m in summary: print(k, n, m)
    print("total", total)
main()
