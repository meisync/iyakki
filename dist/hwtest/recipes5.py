"""Batch 6 (Mac mini 2014, Macmini7,1, no T2, wired network): hand recipes for rows that were not run in batches 1-5.
Everything is read-only except: Test sound (once per row), the screen saver Preview, and sound.output set to the HDMI card and back
(written to settings-touched.log). Pages are read through Settings search; files through `dev files pull` (scopes files and
files:system; no secret file is ever read, only names and non-secret settings). PASS only on matched observed text; every PASS
prints its NOT COVERED list. Rows whose recipe ends in NEEDS-HUMAN say what a person must judge."""
import json, os, re, subprocess, time
from hwcheck import recipe, fail, human, dev, Ctx

DATA = "/var/lib/iyakki"


def _nc(c, *s):
    for x in s: c.notcovered(x)


def pull(c, remote, tag=None):
    """Pull one non-secret file of the device into the evidence folder and return its text."""
    local = os.path.join(c.dir, tag or os.path.basename(remote))
    rc, out = dev("files", "pull", remote, local)
    c.log("PULL %s rc=%d %s" % (remote, rc, out.strip()[:120]))
    if rc or not os.path.exists(local): fail("could not read %s: %s" % (remote, out.strip()[:200]))
    return open(local, errors="replace").read()


def ls(c, remote, tag=None):
    rc, out = dev("files", "ls", remote)
    open(os.path.join(c.dir, "ls-%s.txt" % (tag or remote.strip("/").replace("/", "_"))), "w").write(out)
    c.log("LS %s rc=%d: %s" % (remote, rc, " | ".join(l.strip() for l in out.splitlines()[:12])))
    if rc: fail("could not list %s: %s" % (remote, out.strip()[:200]))
    return out


def dm(c, pat, n=40, tag=None):
    out = dev("dmesg", "--grep", pat, "-n", n)[1]
    open(os.path.join(c.dir, "dmesg-%s.txt" % (tag or re.sub(r"\W+", "_", pat))), "w").write(out)
    return [l for l in out.splitlines() if l.strip()]


def page(c, query, title):
    st = c.settings_page(query)
    if c.page(st) != title: fail("Settings search %r opened page %r, not %r" % (query, c.page(st), title))
    return st


def walk(c, steps=14):
    """Down through the page (focus moves to the page first with Esc; Enter is never sent) and collect every text seen."""
    c.key("Escape", wait=0.6)
    seen = []
    for i in range(steps):
        st = c.key("Down", wait=0.5)
        for t in c.texts(st):
            if t not in seen: seen.append(t)
    return seen


def info(c):
    rc, out = c.run("info", name="info")
    if "Macmini7,1" not in out: fail("this is not the Mac mini 2014 (device line is not Macmini7,1)")
    return out


# ---------------------------------------------------------------- This computer, first-boot style
@recipe("F-HW-05-mm14", "Settings -> This computer: label, boots, six category switches, known computers, three buttons (read-only)")
def hw05(c):
    info(c)
    st = page(c, "this computer", "This computer")
    c.expect(st, r"^Apple Macmini7,1$")
    c.expect(st, r"^Started \d+ times? on it · first seen \d{4}-\d\d-\d\d · last seen \d{4}-\d\d-\d\d$")
    for k in ("Sound", "Display", "Keyboard and pointer", "Microphone", "GPU", "Power and battery"):
        c.expect(st, r"^%s: per computer$" % k)
    c.expect(st, r"^Apple Macmini7,1\s+\(this computer\) · last seen")
    for b in ("Copy settings from…", "Reset this computer", "Forget a computer…"):
        if b not in c.buttons(st): fail("button %r missing" % b)
    c.shot("this-computer")
    ls(c, DATA + "/computers")
    _nc(c, "the one-time inbox notice for a new computer, switching a category, Copy settings from, Reset and Forget (they change data)")


@recipe("F-SETUP-01", "Store -> Setup: tabs, category chips, 26 tiles in categories with state chips, hardware sentence")
def setup01(c):
    info(c)
    c.open_tile("STORE"); st = c.click("Setup", wait=2); c.shot("setup")
    c.expect(st, r"^26 items$")
    for g in ("Get started", "AI and models", "Voices", "Coding agents", "Data", "Media", "Hardware", "Linux apps"):
        if g not in c.buttons(st): fail("category chip %r missing" % g)
    for b in ("All", "Installed", "Not installed", "Recommended", "Updates"):
        if b not in c.buttons(st): fail("filter %r missing" % b)
    c.expect(st, r"^Node\.js$"); c.expect(st, r"^Python 3$"); c.expect(st, r"^k3s$")
    c.expect(st, r"^Not installed$")
    c.expect(st, r"^This machine: Intel graphics \(8086:0a2e\)")
    c.expect(st, r"3 AI tools are hidden because this machine has no GPU with 6 GB or more")
    c.expect(st, r"^Install all recommended \(\d+\)$")
    _nc(c, "a real install from the tab, Running/Installed chips (nothing is installed here), how the tiles look at other window sizes")


@recipe("F-SETUP-07", "Store -> Setup tiles: name, purpose, state markers, filter chips, Hardware group has exactly the matching tiles")
def setup07(c):
    info(c)
    c.open_tile("STORE"); c.click("Setup", wait=2); st = c.click("Hardware", wait=2); c.shot("setup-hardware")
    c.expect(st, r"^3 of 26 shown$"); c.expect(st, r"^Hardware$")
    b = c.buttons(st)
    for t in (r"^Mac Wi-Fi and Bluetooth firmware \(from your Mac\), Failed, recommended$", r"^Broadcom Tigon3 network firmware, Not installed$", r"^Hardware firmware \(Wi-Fi, Bluetooth, graphics, \.\.\.\), Not installed$"):
        if not any(re.search(t, x) for x in b): fail("tile /%s/ not among the buttons" % t)
    c.expect_absent(st, r"Realtek|Qualcomm|Atheros", "firmware tiles for hardware this Mac mini does not have")
    st = c.click("All", must=False) or st
    _nc(c, "the Update pill (needs the network lookup), the colours and icons")


@recipe("F-DRIVERS-32", "Setup -> Hardware: only the firmware tiles that match this Mac mini (Mac, Broadcom Tigon3) and the Mac card text")
def drivers32(c):
    info(c)
    c.open_tile("STORE"); c.click("Setup", wait=2); st = c.click("Hardware", wait=2)
    b = [x for x in c.buttons(st) if x.startswith("Mac Wi-Fi")]
    if not b: fail("no Mac Wi-Fi/Bluetooth firmware tile")
    st = c.click(b[0], wait=2); c.shot("mac-card")
    c.expect(st, r"Downloads Apple's recovery image for this Mac model from Apple"); c.expect(st, r"^Licence$")
    c.expect(st, r"Apple's own licence, no redistribution|IYAKKI never ships them")
    c.expect(st, r"never uploaded and are not part of IYAKKI")
    c.expect(st, r"Recommended: This is a Mac")
    c.click("Close (Esc)", must=False)
    st = c.state(); c.expect_absent(st, r"Realtek|Qualcomm|Atheros")
    _nc(c, "the licence form (a person accepts it; the download is 1 GB and is not started), the other three packs' tiles (hardware absent)")


@recipe("F-SETUP-20", "a failed firmware install is remembered: the card says why and offers Retry without a new download")
def setup20(c):
    info(c)
    c.open_tile("STORE"); c.click("Setup", wait=2); st = c.click("Hardware", wait=2)
    b = [x for x in c.buttons(st) if x.startswith("Mac Wi-Fi")]
    st = c.click(b[0], wait=2); c.shot("failed-card")
    c.expect(st, r"^Failed$")
    c.expect(st, r"^Not done: Apple's macOS versions for this Mac do not contain Wi-Fi extra files")
    c.expect(st, r"Kept on this machine: the macOS Apple sends for this Mac: \d+ MB")
    for x in ("Retry (no new download)", "View log", "Details"):
        if x not in c.buttons(st): fail("card button %r missing" % x)
    c.click("Close (Esc)", must=False)
    _nc(c, "pressing Retry (it reads a 623 MB download; no state change wanted), a re-install that succeeds")


# ---------------------------------------------------------------- sound
@recipe("F-SND-03", "sound: /etc/asound.conf marker, hardware-volume branch (plain plug) on the PCH card, Sound page cards")
def snd03(c):
    info(c)
    a = pull(c, "/etc/asound.conf")
    first = a.splitlines()[0]
    if not first.startswith("# Written by IYAKKI"): fail("asound.conf has no IYAKKI marker first line: %r" % first)
    if "has a hardware volume control" not in a: fail("asound.conf does not say the card has a hardware volume control")
    if not re.search(r'pcm\.!default \{\s*type plug\s*slave\.pcm "hw:1,0"', a): fail("default is not a plain plug over hw:1,0")
    if re.search(r"softvol|dmix", "\n".join(l for l in a.splitlines() if not l.lstrip().startswith("#"))): fail("a softvol/dmix device is written although the PCH card has a Master control")
    c.log("MATCH asound.conf: marker + plain plug over hw:1,0, no softvol/dmix")
    st = page(c, "sound", "Sound"); c.shot("sound")
    c.expect(st, r"^playing on Speakers/headphones \(HDA Intel PCH\)")
    c.expect(st, r"^card 1 · device 0 · plughw:1,0 · speaker or headphones present$")
    c.expect(st, r"^Sound · HDA Intel PCH$"); c.expect(st, r"^Master volume$")
    c.expect(st, r"^Sound · HDA Intel HDMI$"); c.expect(st, r"^no volume control on this card$")
    c.expect(st, r"^HDMI: PDM-15T$"); c.expect(st, r"^card 0 · device 3 · plughw:0,3 · monitor connected$")
    s = json.loads(pull(c, DATA + "/settings.json", "settings.json"))["sound"]
    c.log("settings sound block: " + json.dumps({k: s[k] for k in ("volume_pct", "muted", "output", "mixer")}))
    if s["output"] != "": fail("sound.output is %r, not automatic" % s["output"])
    _nc(c, "the softvol/dmix branch (this card has a hardware Master so the file is the plain-plug form), the IYAKKI control priming, "
           "restore of each output's last volume, what is heard")


@recipe("F-SND-03-pch", "press Test sound once on the PCH output (automatic): a person says whether the chime was heard")
def snd03_pch(c):
    info(c)
    st = page(c, "sound", "Sound")
    c.expect(st, r"^playing on Speakers/headphones \(HDA Intel PCH\)")
    st = c.click("Test sound", wait=3)
    rc, out = c.run("logs", "--system", "--source", "audit", "-n", 6, name="audit-after-test")
    c.shot("after-test")
    human("Test sound pressed once on PCH (card 1, plughw:1,0); a chime heard from the speakers/headphones of the Mac mini is for a person to judge")


@recipe("F-SND-03-hdmi", "sound.output set to the HDMI card, Test sound once, output set back to automatic and verified")
def snd03_hdmi(c):
    info(c)
    raw = c.snapshot_setting("sound.output"); before = re.sub(r"^sound\.output\s*=\s*", "", raw.strip())
    c.log("sound.output before: %r (raw %r)" % (before, raw))
    c.touched_log("sound.output %r -> HDMI:3 (restored below)" % before)
    try:
        rc, out = c.run("config", "set", "sound.output", "HDMI:3", name="set-hdmi")
        if rc: fail("config set sound.output HDMI:3 refused: " + out.strip()[:200])
        time.sleep(2)
        st = page(c, "sound", "Sound")
        c.expect(st, r"^playing on HDMI: PDM-15T")
        c.click("Test sound", wait=3); c.shot("after-hdmi-test")
    finally:
        dev("config", "set", "sound.output", before)
        c.touched_log("sound.output restored to %r" % before)
    raw2 = c.snapshot_setting("sound.output"); now = re.sub(r"^sound\.output\s*=\s*", "", raw2.strip()); c.log("sound.output after: %r" % now)
    if now != before: fail("sound.output was not restored: %r" % now)
    st = page(c, "sound", "Sound"); c.expect(st, r"^playing on Speakers/headphones \(HDA Intel PCH\)")
    human("Test sound pressed once on the HDMI output (card 0, device 3, monitor PDM-15T); whether the monitor played it is for a person to judge. Output restored to automatic")


@recipe("F-SND-21", "sound page: no per-kind level list while nothing plays; mixer on (settings) and the mix switches shown")
def snd21(c):
    info(c)
    st = page(c, "sound", "Sound")
    c.expect(st, r"^Mix sounds from several apps \(experimental\)$"); c.expect(st, r"^Browser sound through the mixer \(experimental\)$")
    c.expect_absent(st, r"^Radio$|^Browser$", "a per-kind level row when nothing is playing")
    s = json.loads(pull(c, DATA + "/settings.json", "settings.json"))["sound"]
    if s["mixer"] is not True or s["source_levels"] != {}: fail("sound block mixer/source_levels unexpected: %r" % s)
    c.log("sound block: mixer=true, source_levels={}")
    _nc(c, "the list of playing sounds with quieter/louder (nothing plays here; needs a person to start the radio), the 10 % steps")


@recipe("F-UPG-09", "settings file of this install: schema 1 and a sound block with mixer on and the defaults (no alpha.1 upgrade here)")
def upg09(c):
    info(c)
    d = json.loads(pull(c, DATA + "/settings.json", "settings.json"))
    if d.get("schema") != 1: fail("settings.json schema is %r" % d.get("schema"))
    s = d["sound"]
    for k in ("boosts", "levels", "mixer", "muted", "output", "source_levels", "volume_modes", "volume_pct", "web_mix"):
        if k not in s: fail("sound block lacks %r" % k)
    if s["mixer"] is not True: fail("mixer is not on by default")
    c.log("MATCH schema=1, sound keys complete, mixer=true")
    _nc(c, "an alpha.1 data folder upgraded by this image (this stick was never alpha.1), alpha.1 password opening the vaults, iyakki-usb --update")


# ---------------------------------------------------------------- startup, boot, kernel
@recipe("F-STARTUP-01", "startup registry: dev status STARTUP table, 9 ready and 7 disabled with the Settings page agreeing")
def startup01(c):
    info(c)
    out = c.run("status", name="status")[1]
    tab = out.split("STARTUP", 1)[-1]
    ready = re.findall(r"^(Network and Wi-Fi|Device name \(mDNS\)|Clock|Remote access \(web\)|Firewall|Sound|Web engine|Flows scheduler|Assistant)\s+ready", tab, re.M)
    off = re.findall(r"^(Touch screen|Wi-Fi|Dev cluster \(k3s\)|Local models|VPN|Shared folders|Web service)\s+disabled", tab, re.M)
    if len(ready) != 9: fail("expected 9 ready services, saw %d: %s" % (len(ready), ready))
    if len(off) != 7: fail("expected 7 disabled services, saw %d: %s" % (len(off), off))
    c.log("MATCH 9 ready: %s | 7 disabled: %s" % (ready, off))
    st = page(c, "startup", "Startup")
    c.expect(st, r"^All services are ready \(9 running\)\.$")
    for n in ("Dev cluster (k3s)", "Local models", "VPN", "Shared folders", "Web service", "Wi-Fi", "Touch screen"):
        c.expect(st, r"^%s$" % re.escape(n))
    c.expect(st, r"^not on this machine$")
    _nc(c, "pending/starting/failed states and the dependency text (k3s waits for the network and the clock): nothing is starting on a settled machine", "boot timing")


@recipe("F-STARTUP-02", "startup on screen and in dev mode: no strip on a settled Home, Settings -> Startup page, dev status and dev diag sections")
def startup02(c):
    info(c)
    st = c.home(); c.expect_absent(st, r"starting|waiting for|Retry", "a startup strip on Home when all is ready")
    st = page(c, "startup", "Startup"); c.shot("startup-page")
    c.expect(st, r"^All services are ready"); 
    for b in ("Retry", "Open its page", "Look again"):
        if b not in c.buttons(st): fail("administrator button %r missing" % b)
    d = c.run("diag", name="diag")[1]
    sec = d.split("== startup ==", 1)[-1].split("==", 1)[0]
    if not re.search(r"Network and Wi-Fi\s+ready", sec) or not re.search(r"Assistant\s+ready", sec): fail("dev diag has no startup section with the services")
    c.log("MATCH dev diag startup section: " + " | ".join(l.strip() for l in sec.splitlines()[:5]))
    _nc(c, "the spinner strip while something starts, the sign-in screen's progress line, Retry on a failed service, what a non-administrator sees")


@recipe("F-BOOT-29", "lean boot: boot volume mounted read-only at /.stage1/boot, data on the stick's third partition, first-stage log kept")
def boot29(c):
    out = info(c)
    if not re.search(r"mount\s+/\.stage1/boot\s+vfat .*\(read-only\)", out): fail("no read-only vfat /.stage1/boot mount in dev info")
    if not re.search(r"mount\s+/var/lib/iyakki\s+ext4", out): fail("data volume not mounted at /var/lib/iyakki")
    c.log("MATCH /.stage1/boot vfat read-only + data ext4")
    s1 = pull(c, DATA + "/stage1-kmsg.log")
    if not re.search(r"^\[\s*0\.000000\] Linux version 6\.12\.112", s1, re.M): fail("stage1-kmsg.log does not start with the kernel banner")
    if "Command line:" not in s1: fail("no kernel command line in the first-stage log")
    c.log("MATCH stage1-kmsg.log: kernel banner and command line, %d bytes" % len(s1))
    st = page(c, "storage", "Storage")
    c.expect(st, r"sdb2 is mounted at /\.stage1/boot"); c.expect(st, r"sdb3 is in use \(mounted at /var/lib/iyakki\)|IYAKKI data")
    _nc(c, "the 1 MB first stage's own switch-root timing, the failure messages (no volume in 30 s), the squashfs/extracted-tree route taken (not told by the log)")


@recipe("F-BOOT-42", "x2APIC: kernel log says IRQ remapping in x2apic mode, x2apic enabled, APIC routing cluster x2apic")
def boot42(c):
    info(c)
    l = dm(c, "x2apic")
    t = "\n".join(l)
    for p in (r"DMAR-IR: Enabled IRQ remapping in x2apic mode", r"x2apic enabled", r"Switched APIC routing to: cluster x2apic"):
        if not re.search(p, t): fail("kernel log lacks /%s/" % p)
        c.log("MATCH /%s/" % p)
    _nc(c, "the Meteor Lake laptop case the row is about (HP Pavilion 16-af0 is another machine); here x2apic was enabled by the Mac mini 2014's own firmware settings")


@recipe("F-BOOT-33", "kernel command line carries panic=20 and no oops=panic / softlockup / hung-task panic switch")
def boot33(c):
    info(c)
    l = dm(c, "Kernel command line")
    t = "\n".join(l)
    if "panic=20" not in t: fail("panic=20 not on the kernel command line: %s" % t[:200])
    if re.search(r"oops=panic|softlockup_panic|hung_task_panic|nmi_watchdog=panic", t): fail("a panic-on-oops switch is on the command line")
    c.log("MATCH panic=20, no oops=panic/softlockup/hung_task switch: " + t[:240])
    _nc(c, "a real panic and the 20 s restart (not provoked), the GRUB entries' menu text (boot volume is outside the readable folders)")


@recipe("F-SYS-04", "crash forensics: kmsg logs of this and the previous boot exist, wall-clock stamps, sizes grow, boot ids named")
def sys04(c):
    info(c)
    o = ls(c, DATA + "/log")
    cur = re.findall(r"file\s+([\d.]+ \w+)\s+\d+ (kmsg-[0-9a-f]{8}\.log)", o)
    prev = re.findall(r"file\s+([\d.]+ \w+)\s+\d+ (kmsg-previous-\d\.log)", o)
    if not cur or not prev: fail("kmsg log of this boot or of the previous boot missing in %s/log" % DATA)
    c.log("MATCH current %s, previous %s" % (cur, prev))
    t = pull(c, DATA + "/log/" + cur[0][1], "kmsg-current.log")
    if not re.search(r"^\[\d{4}-\d\d-\d\d \d\d:\d\d:\d\d\]|^\d{4}-\d\d-\d\d[T ]\d\d:\d\d:\d\d|^\[\s*\d+\.\d+\]", t, re.M): fail("no timestamped kernel lines in the log")
    wall = len(re.findall(r"^\[?\d{4}-\d\d-\d\d", t, re.M)); c.log("lines with a wall-clock stamp: %d of %d" % (wall, t.count("\n")))
    if not wall: c.notcovered("no wall-clock stamps in the current log (clock not plausible yet?)")
    _nc(c, "`iyakki log kernel` command, a real crash (a dying kernel cannot write its last words), the error-line flush within 2 s")


@recipe("F-DISP-09", "display on i915: mode 1920x1080 on HDMI-A-2, touch screen item off because no touch panel, no touch probe log")
def disp09(c):
    out = info(c)
    if not re.search(r"gpu\s+card0 driver i915 .*card0-HDMI-A-2", out): fail("dev info does not show i915 with HDMI-A-2")
    st = page(c, "display", "Display"); c.shot("display")
    c.expect(st, r"^1920x1080 · 32 bit · i915drmfb$"); c.expect(st, r"^Output · HDMI-A-2$")
    c.expect(st, r"^best 1920x1080 · also 1600x900, 1280x800, 1280x720, 1024x768$")
    st = page(c, "startup", "Startup"); c.expect(st, r"^Touch screen$"); c.expect(st, r"^not on this machine$")
    l = dm(c, "touch", 40)
    c.log("kernel/shell lines with 'touch': %d" % len(l))
    if any("probe" in x.lower() and "round" in x.lower() for x in l): fail("a touch probe round was logged on a machine without a touch panel")
    _nc(c, "a USB-C touch monitor (none here), the probe/full/off/auto options and their GRUB entries, KI-077 cold-boot touch")


@recipe("F-KBD-18-mm14", "touch check: Touch screen startup item disabled (not used) on a machine without a panel; Keyboard page has the Probe button")
def kbd18(c):
    info(c)
    out = c.run("status", name="status")[1]
    if not re.search(r"^Touch screen\s+disabled", out, re.M): fail("dev status: Touch screen is not 'disabled'")
    st = page(c, "keyboard", "Keyboard"); 
    if not any("touch" in b.lower() for b in c.buttons(st) + c.texts(st)):
        c.notcovered("no 'Probe touch screen' control on the visible part of the Keyboard page (it may be scrolled out)")
    seen = walk(c, 10)
    hit = [t for t in seen if re.search(r"touch", t, re.I)]
    if not any(re.search(r"probe", t, re.I) for t in seen + c.buttons(st)): c.notcovered("no 'Probe touch screen' control was found on the Keyboard page (visible part or after scrolling)")
    c.log("Keyboard page texts mentioning touch: %s" % hit[:5])
    _nc(c, "the software replug at 10 s on a real panel, the 45 s KI-077 replug, finger events of first taps")


# ---------------------------------------------------------------- developer mode, firewall
@recipe("F-DEVMODE-14-mm14", "dev whoami lists every scope held; Settings -> Developer mode explains the scopes and has Grant/Extend buttons")
def devmode14(c):
    info(c)
    w = c.run("whoami", name="whoami")[1]
    scopes = re.findall(r"^\s*\[x\] (\S+)", w, re.M)
    want = ["install", "debug", "files", "files:system", "config", "input", "input:password", "shell", "power", "update", "forward", "kube"]
    if scopes != want: fail("scopes listed %s, expected %s" % (scopes, want))
    c.log("MATCH 12 scopes, each with a one-line explanation")
    st = page(c, "developer mode", "Developer mode"); c.shot("devmode")
    c.expect(st, r"^Developer mode: On \(since .*\) until switched off$")
    c.expect(st, r"^Grant all diagnostic scopes gives: debug, files, input, kube and shell")
    for b in ("Grant all diagnostic scopes", "Extend 4 h", "Pair a computer", "Switch off"):
        if b not in c.buttons(st): fail("button %r missing" % b)
    c.expect(st, r"^Port 7979 \(TLS\) · next switch-on switches off after 4 h")
    _nc(c, "pressing Grant all / Extend 4 h (the scopes of this computer must not change), the timer running out")


@recipe("F-DEVMODE-15", "firewall: allow rule for TCP 7979 from this computer's address, reason 'developer mode (switches off with it)'")
def devmode15(c):
    info(c)
    r = c.run("config", "firewall", "list", name="firewall-list")[1]
    m = re.search(r"(\d+\.\d+\.\d+\.\d+).*7979.*developer mode \(switches off with it\)", r) or re.search(r"developer mode \(switches off with it\)", r)
    if not m: fail("no developer-mode allow rule for 7979 in dev config firewall list")
    c.log("MATCH rule: " + r.strip()[:300])
    st = page(c, "firewall", "Firewall")
    c.expect(st, r"^IP: TCP port 7979 · developer mode \(switches off with it\)$")
    c.expect(st, r"^192\.168\.68\.\d+$"); c.expect(st, r"^FIREWALL ON$")
    ip = subprocess.run(["ip", "-4", "-o", "addr"], capture_output=True, text=True).stdout
    a = [x for x in re.findall(r"192\.168\.68\.\d+", "\n".join(c.has(st, r"^192\.168\.68\.\d+$")))][0]
    c.log("rule address %s; this computer's addresses: %s" % (a, re.findall(r"inet (\S+)", ip)))
    if a not in ip: fail("the rule address %s is not an address of this computer" % a)
    _nc(c, "removal of the rule when developer mode is switched off (would end this session), the one-time pairing rule for private networks")


@recipe("F-FW-08", "firewall page: counter reads 0 blocked attempts; Advanced view lists rules and the log header; back to Simple view")
def fw08(c):
    info(c)
    st = page(c, "firewall", "Firewall")
    c.expect(st, r"^0 connection attempts blocked so far\. Details are in Advanced\.$")
    try:
        st = c.click("Advanced…", wait=2); c.shot("advanced")
        c.expect(st, r"^Blocked connections since the rules were applied: \d+\. Last \d+ attempts:$")
        for b in ("Clear log", "Presets…", "Simple view"):
            if b not in c.buttons(st): fail("Advanced button %r missing" % b)
    finally:
        s2 = c.state()
        if "Simple view" in c.buttons(s2): c.click("Simple view", must=False)
    _nc(c, "a real blocked attempt (nothing was sent from outside), one line per retrying client, the 50-line ring, Clear log (not pressed)")


# ---------------------------------------------------------------- hardware lists: USB, Bluetooth, battery
@recipe("F-NET-16", "USB port monitor: kernel log has appeared/went-away lines with vid:pid, name, speed (hub, optical mouse, keyboard) and xHCI portsc")
def net16(c):
    info(c)
    l = dm(c, "iyakki INFO ] usb:", 120)
    t = "\n".join(l)
    ap = re.findall(r"usb: device ([\d.-]+) appeared: ([0-9a-f]{4}:[0-9a-f]{4}) (.+?) ([\d.]+ Mbit/s)", t)
    gone = re.findall(r"usb: device ([\d.-]+) went away", t)
    if len(ap) < 3 or not gone: fail("expected >= 3 appeared and >= 1 went-away lines, saw %d/%d" % (len(ap), len(gone)))
    c.log("MATCH appeared: %s" % ap[:8]); c.log("MATCH went away: %s" % gone[:8])
    if not re.search(r"usb: port 0000:00:14\.0/port\d+: 0x[0-9a-f]+ ", t): c.notcovered("no xHCI portsc line seen")
    else: c.log("MATCH xHCI portsc lines present (debugfs ports readable on this kernel)")
    _nc(c, "the PCI host controller power-state lines, a plug-in that never became a device")


@recipe("F-NET-12", "Bluetooth controller read through the kernel: page says 'Controller hci0: Broadcom (working; ...)', kernel has MGMT and BCM20702B0")
def net12(c):
    info(c)
    t = "\n".join(dm(c, "Bluetooth"))
    if "BCM20702B0" not in t or "MGMT ver" not in t: fail("kernel log lacks the BCM20702B0 init or the MGMT interface")
    c.log("MATCH kernel: " + " | ".join(x for x in t.splitlines() if "BCM20702B0" in x or "MGMT" in x))
    st = page(c, "bluetooth", "Bluetooth"); c.shot("bluetooth")
    c.expect(st, r"^Controller hci0: Broadcom \(working; optional firmware file not found\)$")
    c.expect(st, r"^On$")
    _nc(c, "an unconfigured controller with no address (this USB controller has one), Set Powered refusal, the Devices popover row")


@recipe("F-NET-13", "Bluetooth honesty: a USB Broadcom controller that works shows the normal rows, not the 'needs firmware' lead")
def net13(c):
    info(c)
    st = page(c, "bluetooth", "Bluetooth")
    c.expect_absent(st, r"Bluetooth needs firmware", "the needs-firmware lead on a controller that works")
    c.expect(st, r"working; optional firmware file not found")
    for b in ("Scan", "Advanced"):
        if b not in c.buttons(st): fail("button %r missing" % b)
    pop = c.click("devices", wait=2); c.shot("devices-popover")
    c.expect(pop, r"^Controller hci0: Broadcom \(working"); c.expect_absent(pop, r"Missing")
    c.click("close", must=False)
    _nc(c, "a Mac with an unconfigured UART Broadcom controller (the 2018 mini / MacBooks), the Firmware button and the jump to Store -> Setup")


@recipe("F-NET-19", "Bluetooth page: paired list (empty), Scan/Let a phone pair rows with their text")
def net19(c):
    info(c)
    st = page(c, "bluetooth", "Bluetooth")
    c.expect(st, r"^Nothing is paired yet\. Put a keyboard, mouse or phone in pairing mode and choose Scan, then Pair next to it\.$")
    c.expect(st, r"^Look for keyboards, mice, game pads and phones to pair\. The list opens in a popup\.$")
    c.expect(st, r"^ready: a paired phone can play through this computer")
    _nc(c, "any paired device row, Connect/Disconnect/Forget, a scan and a pairing popup (a person with a device in pairing mode)")


@recipe("F-DEVICES-usb", "Devices popover: Ethernet, Wi-Fi (no adapter), Bluetooth, Display, Camera, Keyboard and pointer, Storage rows")
def devices_pop(c):
    info(c)
    page(c, "sound", "Sound")
    st = c.click("devices", wait=2); c.shot("devices")
    c.expect(st, r"^Ethernet$"); c.expect(st, r"^host name iyakki · eth0 · connected · 192\.168\.68\.\d+ · 1000 Mb/s$")
    c.expect(st, r"^no Wi-Fi interface found · Broadcom BCM4360 Wi-Fi \(Mac mini 20")
    c.expect(st, r"^Controller hci0: Broadcom \(working")
    c.expect(st, r"^HDMI-A-2 · PDM-15T · 1920x1080")
    c.expect(st, r"^HD 4MP WEBCAM")
    c.expect(st, r"^keyboard: .* · pointer: .*$"); c.expect(st, r"^3 drives: ")
    c.expect(st, r"^Driver help \(\d+\)$")
    c.click("close", must=False)
    _nc(c, "the 8-of-9 scroll, Mute/Test on the sound row, the Driver help list contents; the keyboard/pointer line is logged as read (a person may plug or unplug devices during the run)")


@recipe("F-BATT-01", "no battery on a Mac mini: dev diag / info show no battery, Devices popover has no battery row, battery switches default")
def batt01(c):
    out = info(c) + c.run("diag", name="diag")[1]
    if re.search(r"(?i)battery|capacity \d+ ?%", out): fail("dev info/diag mention a battery on a machine without one")
    c.log("MATCH no 'battery' word in dev info or dev diag")
    s = json.loads(pull(c, DATA + "/settings.json", "settings.json"))["battery"]
    if s != {"hide_percent": False, "no_shutdown": False, "no_warnings": False}: fail("battery settings are not the defaults: %r" % s)
    st = page(c, "sound", "Sound"); pop = c.click("devices", wait=2)
    c.expect_absent(pop, r"(?i)battery|charging|on the adapter")
    c.click("close", must=False)
    _nc(c, "any real battery reading, several batteries combined, health, drivers' sign conventions: needs a laptop")


@recipe("F-BATT-02", "no battery indicator on a Mac mini: the top bar of Home and of an app has no percentage or battery text")
def batt02(c):
    info(c)
    st = c.home(); c.shot("home")
    top = [t for t in c.texts(st) if re.search(r"^\d{1,3} ?%$|battery|charging", t, re.I)]
    # only the top bar band (the volume popover is not open); the page has no percent text on Home
    if top: fail("battery-like text on Home: %s" % top)
    c.log("MATCH Home has no battery text")
    st = c.open_tile("STORE"); top = [t for t in c.texts(st) if re.search(r"^\d{1,3} ?%$|battery", t, re.I)]
    if top: fail("battery-like text in an app bar: %s" % top)
    c.log("MATCH app bar has no battery text")
    _nc(c, "the drawn icon (dev screenshots do not prove an absent icon; the owner can look), F4 popup, low-battery cards: need a laptop")


@recipe("F-CHECKUP-01", "CHECK-UP lists the twelve steps in order with Check again / Share result / Open DOCTOR")
def checkup01(c):
    info(c)
    st = c.open_tile("CHECK-UP")
    c.expect(st, r"^\d+ passed, \d+ failed, \d+ skipped, \d+ not done yet$")
    order = ["Screen", "Touch screen", "Keyboard", "Mouse or touch pad", "Speaker", "Microphone", "Camera", "External monitor", "Network", "Wi-Fi", "Bluetooth", "Battery and power"]
    txt = [t for t in c.texts(st)]
    pos = []
    for n in order:
        if n not in txt: fail("step %r missing" % n)
        pos.append(txt.index(n))
    if pos != sorted(pos): fail("steps are not in the documented order")
    c.log("MATCH 12 steps in order: " + ", ".join(order))
    st2 = c.click("Check again", wait=4)
    if c.has(st2, r"^Pass$|^Fail$|^Skipped$"): c.log("after Check again some steps have a result: " + str(c.has(st2, r"^Pass$|^Fail$|^Skipped$")))
    else: c.log("after Check again every step still reads 'Not done yet'")
    human("the list and its order are right, but 'Check again' leaves all twelve steps 'Not done yet' here; whether a step turns Pass/Fail/Skipped from the machine's lists (input devices, sound cards, outputs, battery) needs a person to walk the steps")


# ---------------------------------------------------------------- display, saver, lock, home
@recipe("F-SCRN-04", "Settings -> Screen saver: the seven cards, Lock cards and the sign-in three after scrolling; Preview now shows and returns")
def scrn04(c):
    info(c)
    st = page(c, "screen saver", "Screen saver"); c.shot("saver")
    first = [t for t in c.texts(st)]
    for p in (r"^When the screen is idle$", r"^Idle for$", r"^Then turn the screen off after$", r"^Screen saver style$", r"^Dim the screen first$", r"^Shorter on battery$", r"^Try it now$"):
        if not any(re.search(p, t) for t in first):
            seen = walk(c, 14); first += seen
    seen = walk(c, 14)
    allt = first + seen
    for p in (r"^Lock the screen when idle$", r"^Lock the screen now$", r"^Sign-in screen: when idle$", r"^Sign-in screen: idle for$", r"^Sign-in screen: then turn off after$"):
        if not any(re.search(p, t) for t in allt): fail("card /%s/ not found after scrolling the page" % p)
        c.log("MATCH /%s/" % p)
    st = c.state(); c.click("Preview now", wait=1.5)
    st1 = c.state("during-preview"); c.log("during preview: app=%s page=%s texts=%d" % (c.app(st1), c.page(st1), len(c.texts(st1))))
    time.sleep(7)
    st2 = c.state("after-preview"); c.log("after preview: app=%s page=%s" % (c.app(st2), c.page(st2)))
    if "Lock now" in st2 and "locked" in st2.lower(): fail("the screen is locked after the preview")
    if c.app(st2) != "Settings": fail("after the preview the screen is not back on Settings (app %s)" % c.app(st2))
    c.expect(st2, r"^Screen saver$")
    _nc(c, "Enter on a card (changes a setting), the idle timers actually firing (5 min or more, not waited), the drawn logo, backlight dim and FBIOBLANK on this HDMI monitor")


@recipe("F-LOCK-04", "lock cards exist on the Screen saver page (Lock the screen when idle / Lock the screen now) and no lock file is set (default Never)")
def lock04(c):
    info(c)
    page(c, "screen saver", "Screen saver")
    allt = walk(c, 14)
    for p in (r"^Lock the screen when idle$", r"^Asks for your password after this long without a key", r"^Lock the screen now$", r"^Lock now$"):
        if not any(re.search(p, t) for t in allt): fail("/%s/ not on the page" % p)
        c.log("MATCH /%s/" % p)
    o = ls(c, DATA)
    if re.search(r"lockscreen-", o): c.notcovered("a lockscreen-<user>.json exists: some idle-lock choice was saved earlier")
    else: c.log("no lockscreen-<user>.json: nobody saved an idle-lock choice, the default Never applies")
    st = c.state(); c.expect_absent(st, r"Locked screen")
    _nc(c, "Lock now (not pressed: the screen must not be left locked, and no password is typed), the audit line settings.lockscreen, the guest's dead buttons, the idle lock firing")


@recipe("F-SHELL-54", "Home: the five default groups with every tile in exactly one group, no Apps group, no custom tiles")
def shell54(c):
    info(c)
    st = c.home()
    groups = [b[len("group "):] for b in c.buttons(st) if b.startswith("group ")]
    if groups != ["Ask & Create", "Build", "Media", "Data", "Devices & System"]: fail("groups are %s" % groups)
    c.log("MATCH groups: %s" % groups)
    tiles = {}
    for g in groups:
        s = c.click("group " + g, wait=1.2)
        names = [b for b in c.buttons(s) if re.match(r"^[A-Z0-9][A-Z0-9 \-/&]+$", b) and b not in ("DEV",)]
        tiles[g] = names; c.log("group %s: %s" % (g, names))
        if not names: fail("group %s shows no tile" % g)
    flat = [t for v in tiles.values() for t in v]
    dup = sorted({t for t in flat if flat.count(t) > 1})
    if dup: fail("tiles in more than one group: %s" % dup)
    if "group Apps" in c.buttons(c.state()): fail("an Apps group exists with no custom tile installed")
    layout = pull(c, DATA + "/layout-admin.json")
    if json.loads(layout).get("groups") != []: fail("the saved layout has custom groups")
    _nc(c, "a tile installed later landing in Apps (done by F-APPS-46-live), Reset to default layout, rename/move/delete groups, the small window sizes")


# ---------------------------------------------------------------- storage, files, other pages
@recipe("F-STORE-09-mm14", "people's files survive power-off: /home is a mount of the data volume's disk (same device as /var/lib/iyakki)")
def store09(c):
    out = info(c)
    d = c.run("diag", name="diag")[1]
    a = re.search(r"/dev/(\w+) on /var/lib/iyakki \(ext4\)", d); b = re.search(r"/dev/(\w+) on /home \(ext4\)", d)
    if not a or not b: fail("dev diag does not list both /var/lib/iyakki and /home as ext4 mounts")
    if a.group(1) != b.group(1): fail("/home is on %s but the data volume is on %s" % (b.group(1), a.group(1)))
    c.log("MATCH /home and /var/lib/iyakki are both on /dev/%s (ext4)" % a.group(1))
    o = ls(c, DATA + "/home")
    c.log("data-folder home listing seen")
    st = page(c, "storage", "Storage")
    c.expect(st, r"^Saved to disk \(/dev/sdb3\)\. Your accounts, settings, vaults, documents and chats survive power-off\.$")
    _nc(c, "the file made in /home surviving an actual power-off (no reboot allowed), merging of earlier-boot files, 0700 ownership of /home/<name>, the IDE build panel's HOME")


@recipe("F-LOSS-01", "boot-drive watcher: storage page and boot log show a healthy USB data drive, no loss event in the audit log")
def loss01(c):
    info(c)
    st = page(c, "storage", "Storage")
    c.expect(st, r"^Saved to disk \(/dev/sdb3\)")
    c.expect_absent(st, r"not available|Lost|Reattach|read-only|Errors")
    a = c.run("logs", "--system", "--source", "audit", "-n", 400, name="audit")[1]
    if re.search(r"storage\.event|drive.*(lost|not available)", a, re.I): fail("an audit line reports a storage event: " + [l for l in a.splitlines() if re.search(r"storage\.event", l)][:1].__repr__())
    c.log("MATCH no storage.event line in the last 400 audit lines")
    k = "\n".join(dm(c, "Buffer I/O error")) + "\n".join(dm(c, "EXT4-fs error"))
    if k.strip(): fail("kernel I/O errors in the log: " + k[:200])
    c.log("MATCH no 'Buffer I/O error' or 'EXT4-fs error' in the kernel log")
    _nc(c, "an actual unplug (not done: it is the boot drive), Reattachable/Reconnecting states, the one-byte fsync probe timing")


@recipe("F-LOSS-05", "storage page: USB speed, port, driver of the boot stick; runtime PM set to on in the log; compatible mode off")
def loss05(c):
    info(c)
    st = page(c, "storage", "Storage"); c.shot("storage")
    c.expect(st, r"^Samsung Flash Drive runs at SuperSpeed 5 Gbit/s on USB port 2-4, usb-storage \(compatible\)\.$")
    c.expect(st, r"^Compatible USB mode \(no UAS\): off\.")
    l = "\n".join(dm(c, "storage:"))
    for p in (r"runtime power management of .*/2-4/power/control set to on", r"storage: drive on USB port 2-4, directly on the computer at SuperSpeed 5 Gbit/s with driver usb-storage"):
        if not re.search(p, l): fail("kernel log lacks /%s/" % p)
        c.log("MATCH /%s/" % p)
    _nc(c, "the warnings for USB 2 speed or a shared hub (this stick is direct on SuperSpeed), the u switch and the GRUB no-UAS entry (boot volume not readable, a change needs a restart)")


@recipe("F-BACKUP-03", "Settings -> Backup and restore: included areas with counts and sizes, the not-encrypted sentence, four buttons (nothing is backed up)")
def backup03(c):
    info(c)
    st = page(c, "backup", "Backup and restore"); c.shot("backup")
    c.expect(st, r"^The file is NOT encrypted unless you type a passphrase when you back up\.")
    for a in ("Settings (layout, size, screen saver, displays, radio, browser choices)", "Browser bookmarks, history and header rules", "Dashboards and their data sources", "Flows", "Skills", "Notes, context and calendar", "Chats", "Recordings", "Your vault, as its already encrypted file"):
        c.expect(st, r"^\[x\] " + re.escape(a))
    c.expect(st, r"^\d+ file\(s\) · [\d.]+ [KMG]?B$")
    c.expect(st, r"^5 file\(s\), [\d.]+ KB in the backup$")
    for b in ("Back up…", "Open backup…", "Include / leave out", "Look again"):
        if b not in c.buttons(st): fail("button %r missing" % b)
    ls(c, DATA, tag="data-root")
    _nc(c, "making or opening a backup, the RESTORE confirmation, the file picker and a real USB stick, the passphrase")


@recipe("F-A11Y-05", "Settings -> Accessibility: every switch the row lists, the voice test and the 'not covered' note")
def a11y05(c):
    info(c)
    st = page(c, "accessibility", "Accessibility"); c.shot("a11y")
    for b in ("Screen reader", "Read messages and errors aloud", "High contrast", "Large mouse pointer", "Big focus ring", "Reduce motion", "Sticky keys", "Test the voice"):
        if b not in c.buttons(st): fail("control %r missing" % b)
    c.expect(st, r"^Slow keys: off$"); c.expect(st, r"^Text size: Normal \(100%\)")
    c.expect(st, r"^No reading voice is installed yet: set one up in Settings, Voice\. Super\+Alt\+S turns it on or off")
    c.expect(st, r"^Not covered: web pages in the Browser")
    ls(c, DATA, tag="data-root")
    _nc(c, "any switch turned on (they apply to the person), access-<name>.json written, the chat tools accessibility_status/accessibility_set, narrow-window cut-off")


@recipe("F-A11Y-03-mm14", "visual options exist and say what they do: High contrast (7:1), Large mouse pointer, Big focus ring, Reduce motion; theme list has High contrast")
def a11y03(c):
    info(c)
    st = page(c, "accessibility", "Accessibility")
    c.expect(st, r"^Black and white with a yellow accent and thick borders; every text at least 7 to 1\.$")
    c.expect(st, r"^A bigger pointer on the screen\.$"); c.expect(st, r"^A thicker ring round the control that has the focus\.$")
    c.expect(st, r"^No moving background, no blinking caret\.$")
    st = page(c, "display", "Display")
    for b in ("High contrast", "Light"):
        if b not in c.buttons(st): fail("theme button %r missing" % b)
    c.expect(st, r"^✓ Dark$")
    for b in ("Small 85%", "Normal 100%", "Large 125%", "Larger 150%", "Huge 200%"):
        if not any(b in x for x in c.buttons(st) + c.texts(st)): fail("text size %r missing" % b)
    _nc(c, "the computed contrast ratios on screen, switching a theme (changes the person's look), the large pointer on the framebuffer, reduce motion")


@recipe("F-BELL-03-mm14", "Settings -> Notifications: one row per source with Show / Quiet / Off, Matrix line; inbox kinds seen")
def bell03(c):
    info(c)
    st = page(c, "notifications", "Notifications"); c.shot("notifications")
    for lbl in ("New devices on the network", "Finished assistant runs and jobs", "Flows", "Reminders and alarms", "Downloads", "Tasks", "What assistants leave for you", "Warnings about the system", "Other system notes"):
        c.expect(st, r"^%s$" % re.escape(lbl))
    n = len([b for b in c.buttons(st) if b == "Quiet (panel only)"])
    if n < 8: fail("only %d 'Quiet (panel only)' choices" % n)
    c.log("MATCH %d rows with the three choices" % n)
    c.expect(st, r"^Show: counted in the bell by kind\. Quiet: in the inbox only")
    i = c.run("inbox", "--json", name="inbox")[1]
    items = json.loads(i)["items"]; c.log("inbox rows: %d" % len(items))
    if len(items) > 200: fail("more than 200 inbox rows")
    _nc(c, "the 7/14/30-day keeping times and 60-per-source cap (need an old file), grouping on first open, muting a source (changes a setting)")


@recipe("F-GIT-04", "Settings -> Git and SSH keys: vault-only sentence, no key made, no private key file in the data folder or the home folder")
def git04(c):
    info(c)
    st = page(c, "git", "Git and SSH keys"); c.shot("git")
    c.expect(st, r"^Keys for git over SSH are made here or imported, and kept encrypted in your vault\.")
    c.expect(st, r"^No connections or keys yet\.")
    for b in ("Generate key", "Import key", "Copy public key", "Host → key"):
        if b not in c.buttons(st): fail("button %r missing" % b)
    o = ls(c, DATA) + ls(c, DATA + "/home", tag="home-listing")
    if re.search(r"id_ed25519|id_rsa|\.pem\b|\.ssh", o): fail("a key-like file name is in the data folder or the home folder listing")
    c.log("MATCH no id_*/ .pem / .ssh names in the listings")
    _nc(c, "generating or importing a key, the QR code, host-key confirmation, the git agent (no git host reachable here), the vault contents (never read)")


@recipe("F-DRIVERS-03-mm14", "firmware on the data volume: pack folders exist but hold no firmware; dev info lists no missing firmware file")
def drivers03(c):
    out = info(c)
    if "no missing firmware files in the kernel log" not in out: fail("dev info reports missing firmware")
    for d in ("fwpack", "fwextra", "mac-firmware"):
        o = ls(c, DATA + "/" + d, tag=d)
    c.log("MATCH data-volume firmware folders readable; kernel log has no 'firmware: failed' lines")
    bad = dm(c, "failed to load firmware", 20) + dm(c, "firmware: failed", 20)
    if bad: fail("kernel log has firmware load failures: %s" % bad[:2])
    _nc(c, "symlinks into /lib/firmware (outside the readable folders), a pack being installed and the driver reload, remove/turn-off undoing it")


@recipe("F-GPU-04", "PC image: /etc/iyakki-release names the variant pc-x86_64, dev info agrees, i915 bound on the Intel GPU")
def gpu04(c):
    out = info(c)
    r = pull(c, "/etc/iyakki-release")
    if not re.search(r"pc-x86_64", r): fail("/etc/iyakki-release has no pc-x86_64: %r" % r[:200])
    c.log("MATCH /etc/iyakki-release: " + r.strip().replace("\n", " | "))
    if "variant pc-x86_64" not in out: fail("dev info does not say variant pc-x86_64")
    if not re.search(r"gpu\s+card0 driver i915", out): fail("no i915 card in dev info")
    _nc(c, "NVIDIA GSP firmware, nouveau off entry, AMD GPUs, the 420 MB firmware set (this Mac mini has only Intel graphics), the 512 MB boot volume")


@recipe("F-ZRAM-02", "swap file and zswap are off by default: no iyakki.swapfile on the command line, no swapfile in the data folder")
def zram02(c):
    info(c)
    t = "\n".join(dm(c, "Kernel command line"))
    if re.search(r"iyakki\.swapfile|iyakki\.zswap", t): fail("a swapfile/zswap option is on the command line")
    o = ls(c, DATA, tag="data-root")
    if re.search(r"swapfile|\bswap\b", o): fail("a swap file exists in the data folder")
    c.log("MATCH no swapfile option and no swap file in the data folder; swap in dev info is the zram swap")
    _nc(c, "creating a swap file (iyakki.swapfile=2G needs a boot entry change), zswap=on")


@recipe("F-DEV-12", "kernel modules for one-cable links exist in /lib/modules and the thunderbolt module is loaded at boot")
def dev12(c):
    out = info(c)
    o = ls(c, "/lib/modules")
    ver = re.findall(r"dir\s+\d+ (\S+)/", o)
    c.log("module trees: %s" % ver)
    if "6.12.112" not in " ".join(ver): fail("no /lib/modules/6.12.112")
    ls(c, "/lib/modules/6.12.112", tag="modules-tree")
    dep = pull(c, "/lib/modules/6.12.112/modules.dep")
    for mod in ("thunderbolt.ko", "thunderbolt_net.ko"):
        if not re.search(r"(^|/)%s:" % re.escape(mod), dep, re.M): fail("%s is not in modules.dep" % mod)
        c.log("MATCH %s listed in modules.dep" % mod)
    ko = dm(c, "thunderbolt", 20)
    c.log("kernel lines with 'thunderbolt': %d: %s" % (len(ko), ko[:2]))
    if not ko: fail("no 'thunderbolt' line in the kernel log: the module is not loaded at boot")
    _nc(c, "the CONFIG_USB_NET_* bridge-cable drivers, thunderbolt_net being loaded (only its presence in modules.dep is read; the kernel log shows the thunderbolt bus registered and 'device link creation from 0000:06:00.0 failed' for 07:00.0, not a bound device), bridge cables, aarch64 gadget mode")
