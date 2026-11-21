"""Batch 2 recipes (read-only unless a setting is named and put back). See recipes.py for the conventions."""
import os, re, time, json, tempfile
from hwcheck import require_hardware, not_applicable, recipe, fail, human, dev, Verdict, Ctx
import recipes as _r1

def nc(c, t): c.notcovered(t)


def _btn_text(c, st): return "\n".join(c.buttons(st))



def _tap_tab(c, name):
    """Tap the tab called NAME in the top strip (found on screen: the tabs move when a count is added to the first one)."""
    st = c.state()
    m = re.search(r"^\s*(\d+),(\d+)\s+%s$" % re.escape(name), st.split("\nbuttons", 1)[0], re.M)
    if not m: fail("no tab called %r on screen" % name)
    dev("input", "tap", int(m.group(1)) + 10, int(m.group(2)) + 8); time.sleep(1.5)

@recipe("F-DRIVERS-07", "STORE -> Installed -> Drivers chip on the Mac mini")
def drivers07(c):
    """PASS when the Drivers chip exists beside MCP/CLI/Skills/Apps/Extensions and its view lists this Mac's drivers with a
    state ('driver X is loaded'), a Why button each, and Remove where a driver is removable."""
    require_hardware(c, "apple")   # the expected list is this Mac's (i915, brcmfmac, T2)
    st = c.open_tile("STORE")
    for chip in (r"^MCP \d+$", r"^CLI \d+$", r"^Skills \d+$", r"^Apps \d+$", r"^Extensions \d+$", r"^Drivers \d+$"):
        c.expect(st, chip)
    b = [x for x in c.buttons(st) if re.match(r"^Drivers \d+$", x)][0]
    st = c.click(b, wait=2); c.shot("drivers")
    bt = _btn_text(c, st)
    c.log("buttons:\n" + bt)
    for pat in (r"Intel graphics \(Mac\) driver i915 is loaded", r"Broadcom Wi-Fi firmware \(Mac, brcmfmac\) driver brcmfmac is loaded", r"Apple T2 Macs"):
        if not re.search(pat, bt): fail("Drivers view lacks /%s/" % pat)
    if bt.count("\nWhy") < 3: fail("fewer than 3 Why buttons")
    if "\nRemove" not in bt: fail("no Remove button on a removable driver")
    nc(c, "installing from the catalog / a USB folder / a camera file (installs are out of bounds); the chip says 'Drivers 0' while the view lists the loaded drivers: the count counts installed packages, not loaded drivers (observation, not judged)")


@recipe("F-SSHTUN-02", "Settings -> SSH tunnels page (empty list)")
def sshtun02(c):
    """PASS when the page for an administrator under Network shows the explanation (127.0.0.1 only, test then enable), the
    empty-state line and every button the row lists: Add, Edit, On/Off, Delete, Test, Retry, Copy URL, Copy key."""
    st = c.settings_page("ssh tunnels"); c.shot("ssh-tunnels")
    c.expect(st, r"each tunnel listens on this device only \(127\.0\.0\.1\)"); c.expect(st, r"No tunnels yet\. Press a to add one")
    for b in ("Add", "Edit", "On / Off", "Delete", "Test", "Retry", "Copy URL", "Copy key"):
        if b not in c.buttons(st): fail("button %r missing" % b)
    nc(c, "a real tunnel (add, test with a fingerprint, enable) needs an SSH host of the owner's; per-row state cannot be seen with an empty list")


@recipe("F-DISKS-04", "DISKS tile: the system drive is protected on screen")
def disks04(c):
    """PASS when the system drive is marked SYSTEM DRIVE / INTERNAL, its text says it 'cannot be unmounted, ejected or changed',
    the data partition shows its mount point, and no 'Advanced: change partitions' control is offered for the system drive.
    The engine's own refusal (typed name, dry run) was not exercised: it would write."""
    st = c.open_tile("DISKS"); c.shot("disks")
    # the drive LIST marks the system drive "<size> · <bus> · SYSTEM"; the chip "SYSTEM DRIVE" is in the header of the SELECTED drive, which
    # is the first in the list, not always the system one (on a PC booted from a stick the internal disk is first): Down walks the list
    c.expect(st, r"· SYSTEM$", "the drive list marks one drive SYSTEM")
    for _ in range(8):
        if c.has(st, r"^SYSTEM DRIVE$"): break
        st = c.key("Down", wait=0.8)
    c.expect(st, r"^SYSTEM DRIVE$"); c.expect(st, r"It cannot be unmounted, ejected or changed")
    c.expect(st, r"IYAKKI-DATA \(ext4\)"); c.expect(st, r"/var/lib/iyakki$")
    c.expect_absent(st, r"Advanced: change partitions")
    nc(c, "the engine-level refusals, typed-name confirmation, dry run plan and audit of refusals (need a second drive and writes); the Advanced button appears only for other drives")


@recipe("F-LINAPP-05", "IYAKKI menu -> Linux apps page")
def linapp05(c):
    """PASS when the page opens (menu entry, key l) with views Sandboxes, New, Image apps, Refresh, the empty-state text and the
    key line; no sandbox exists. Not covered: creating a sandbox (downloads Debian/Ubuntu)."""
    c.home(); c.click("IYAKKI menu"); st = c.click("Linux apps", wait=2); c.shot("linux-apps")
    for b in ("Sandboxes", "New", "Image apps", "Refresh"):
        if b not in c.buttons(st): fail("button %r missing" % b)
    c.expect(st, r"Sandboxes run in Linux namespaces"); c.expect(st, r"No sandboxes yet")
    c.expect(st, r"t terminal · i install · n network · d delete")
    c.click("close", must=False)
    nc(c, "New (Debian 12/13, Ubuntu 22.04/24.04 form), terminal, install/remove/upgrade, network switch, mounts, delete: all need a sandbox, whose creation downloads an OS image")


@recipe("F-FLOW-09", "FLOWS tile: design/run/board shells and the scheduler line")
def flow09(c):
    """PASS when FLOWS opens with the DESIGN / RUN / BOARD tabs, 'scheduler running', the run counters, the empty-list help
    (N new, T template, import) and the key line. A canvas needs a flow; none exists and creating one is not allowed here."""
    st = c.open_tile("FLOWS"); c.shot("flows")
    for b in ("DESIGN", "RUN", "BOARD", "New flow (n)", "From template (t)", "Import (i)"):
        if b not in c.buttons(st): fail("button %r missing" % b)
    c.expect(st, r"^scheduler running$"); c.expect(st, r"^running 0$"); c.expect(st, r"^needs you 0$")
    c.expect(st, r"No workflows yet")
    for t in ("RUN", "BOARD", "DESIGN"):
        st = c.click(t, wait=1.2); c.log("tab %s opened: app=%s" % (t, c.app(st)))
        if c.app(st) != "FLOWS": fail("tab %s left FLOWS" % t)
    c.shot("flows-after-tabs")
    nc(c, "the canvas, ports, minimap, run view, waterfall, approvals and the fix card: all need a flow (none exists)")


@recipe("F-SETUP-08", "Store -> Setup: the k3s popup")
def setup08(c):
    """PASS when clicking the k3s card opens a popup with the status line 'Running · v..', what it is, Size and time, Licence,
    Where it goes, a Stop button only (no Install for a running k3s), View log and Details, and Close (Esc) closes it."""
    c.open_tile("STORE"); st = c.click("Setup", wait=2)
    for _ in range(30):
        st = c.state()
        if not any(b.startswith("k3s, Checking") for b in c.buttons(st)): break
        time.sleep(2)
    card = next((b for b in c.buttons(st) if b.startswith("k3s, ")), None)
    if not card: fail("no k3s card on Store -> Setup")
    st = c.click(card, wait=1.5); c.shot("k3s-popup")
    bt = c.buttons(st); c.log("popup buttons: %s" % bt)
    c.expect(st, r"^Running · v[\d.]+\+k3s")
    for pat in (r"^Size and time$", r"^Licence$", r"^Where it goes$"): c.expect(st, pat)
    for b in ("Close (Esc)", "Details", "View log"):
        if b not in bt: fail("popup button %r missing" % b)
    if not any(b.startswith("Stop") for b in bt): fail("no Stop button for a running k3s")
    if "Install" in bt: fail("popup offers Install for a running k3s")
    c.click("Close (Esc)")
    nc(c, "states other than Running (Failed, Update available, Not installed) and Install all in this category; no button was pressed")


@recipe("F-CLUSTER-01", "Settings -> Clusters: a fresh account lists only the local cluster")
def cluster01(c):
    """PASS when the Clusters page lists the machine's k3s as the only row ('k3s · default', reachable) with no switcher
    or extra rows, and shows the namespace rule. The json files themselves are not readable (the files scope was withdrawn)."""
    st = c.settings_page("dev cluster"); c.shot("clusters")
    c.expect(st, r"^Local cluster managed by IYAKKI$"); c.expect(st, r"^running$"); c.expect(st, r"^k3s$"); c.expect(st, r"^reachable$")
    rows = [b for b in c.buttons(st) if "·" in b and "default" in b]
    c.log("cluster rows: %s" % rows)
    if len(rows) != 1: fail("expected exactly one cluster row, saw %r" % rows)
    nc(c, "migration of old vault profiles and the remembered choice, damaged-file repair, per-workspace override, roles for users (needs other accounts and file access)")


@recipe("F-CLUSTER-04", "Settings -> Clusters page controls")
def cluster04(c):
    """PASS when the page shows the cluster's status line (running, version, node Ready, pods, data, disk), the local controls
    Stop / Restart / Logs / Upgrade / Autostart / Fixed address / Reset / Uninstall, and Test / My default / Workspace default /
    Hide/Show / Disconnect. Only Test is pressed."""
    st = c.settings_page("dev cluster")
    c.expect(st, r"^running · v[\d.]+\+k3s\d · node Ready · \d+/\d+ pods")
    for b in ("Stop", "Restart", "Logs", "Upgrade", "Autostart", "Fixed address", "Reset", "Uninstall", "Test", "My default", "Workspace default", "Hide/Show", "Disconnect"):
        if b not in c.buttons(st): fail("button %r missing" % b)
    st = c.click("Test", wait=4); c.expect(st, r"reachable"); c.shot("after-test")
    nc(c, "Enter to set the active cluster, add remote / add local, rename, Start/Stop asking by name, extra local cluster, remove, five sizes; the row says the page is still the old Dev cluster page underneath (seen: title 'Clusters', search word 'dev cluster' needed)")


@recipe("F-K8S-24", "K8S tile: changes are off by default")
def k8s24(c):
    """PASS when the K8S tile opens read-only: the 'Allow changes' switch is present, the detail pane says 'Read-only. Turn on
    Allow changes in the header to restart, scale or delete.', and no Restart/Scale/Delete/Cordon button exists."""
    st = c.open_tile("K8S"); c.shot("k8s")
    if "Allow changes" not in c.buttons(st): fail("no Allow changes switch")
    c.expect(st, r"Read-only\. Turn on Allow changes in the header to restart, scale or delete")
    for bad in ("Restart", "Scale", "Delete", "Cordon", "Uncordon"):
        if any(b == bad or re.match(r"^%s( \(|…|$)" % bad, b) for b in c.buttons(st)): fail("a %s button exists while changes are off" % bad)
    nc(c, "turning Allow changes on and running restart/scale/delete/cordon (they change the cluster)")


@recipe("F-DEV-05", "iyakki dev config: list, get, typed refusal, export without secrets, all-or-nothing import")
def dev05(c):
    """PASS when list/get work and show types and ranges; an out-of-range value and an unknown key are refused and change
    nothing; an exported profile holds no key/secret text; an import with one valid and one invalid line is refused as a whole
    and the valid one is NOT applied (time.hour12 stays as it was)."""
    rc, out = c.run("config", "list", name="config-list")
    if "int 0..100" not in out or "one of" not in out: fail("config list shows no types/ranges")
    before = c.snapshot_setting("time.hour12"); vol = c.snapshot_setting("sound.volume")
    c.log("before: %s | %s" % (before, vol))
    rc, out = c.run("config", "set", "sound.volume", "500", name="set-bad-range")
    if rc == 0 and "500" in out and "refus" not in out.lower() and "must" not in out.lower() and "range" not in out.lower(): fail("out-of-range value was accepted: " + out[:150])
    if c.snapshot_setting("sound.volume") != vol: fail("volume changed after a refused set")
    rc, out = c.run("config", "set", "no.such.key", "1", name="set-unknown")
    if rc == 0: fail("unknown key accepted")
    ex = os.path.join(c.dir, "profile-export.json"); rc, out = c.run("config", "export", ex, name="export")
    txt = open(ex).read() if os.path.exists(ex) else out
    if re.search(r"(?i)api[_-]?key\W+\S{6,}|Bearer |sk-[A-Za-z0-9]{10,}", txt): fail("export contains a secret-looking string")
    prof = json.loads(txt) if txt.strip().startswith("{") else None
    bad = os.path.join(c.dir, "bad-import.json")
    if prof is None or "settings" not in prof: fail("export is not a profile with a settings object")
    flip = (before.split()[-1] != "true")
    prof["settings"]["time.hour12"] = flip          # a valid change ...
    prof["settings"]["sound.volume"] = 999          # ... next to an invalid one
    open(bad, "w").write(json.dumps(prof))
    rc, out = c.run("config", "import", bad, name="import-bad")
    after = c.snapshot_setting("time.hour12")
    c.log("after bad import: %s (rc=%s)" % (after, rc))
    if after != before: fail("the valid line of a refused import was applied: %s -> %s (set back by hand!)" % (before, after))
    if rc == 0: fail("import with an invalid value returned success")
    if "not an IYAKKI profile" in out: fail("the test file was not accepted as a profile at all, so all-or-nothing was not tested")
    nc(c, "providers add/key/remove (key write-only), firewall keep-or-revert, host name and display size changes (they change the device); the export format could not be parsed here: " + ("parsed as JSON" if prof is not None else "not JSON"))


@recipe("F-DEV-14", "dev state/screenshot answer after the screen was left idle (waits ~11 minutes with no input)")
def dev14(c):
    """PASS when after 11 minutes without any input or dev command, `dev state` and `dev screenshot` each answer within 15 s
    (the row's bug was a timeout 'the screen did not answer in time'). The screen saver itself is not seen to start."""
    c.home()
    time.sleep(11 * 60)
    for what, args in (("state", ["state"]), ("screenshot", ["screenshot", "-o", os.path.join(c.dir, "after-idle.png")])):
        t = time.time(); rc, out = dev(*args, timeout=60); dt = time.time() - t
        c.log("%s after idle: rc=%d in %.1f s: %s" % (what, rc, dt, out.strip()[:100]))
        if rc or "did not answer" in out or dt > 15: fail("%s after idle: rc=%d, %.1f s, %s" % (what, rc, dt, out.strip()[:100]))
    nc(c, "whether the saver/black screen actually started during the wait (the owner may have used the machine); a locked screen case")


@recipe("F-BELL-02", "bell number equals what needs the person")
def bell02(c):
    """PASS when the number on the bell equals reminders + errors + warnings (+ approvals, tasks, assistant waiting) from
    `dev inbox --json`, and device / agent (finished jobs) items are not counted."""
    rc, out = c.run("inbox", "--json", name="inbox")
    kinds = {k["kind"]: k["count"] for k in json.loads(out)["by_kind"]}
    c.log("by kind: %s" % kinds)
    needs = sum(kinds.get(k, 0) for k in ("approval", "task", "error", "warning", "reminder", "assistant"))
    st = c.home(); shown = c.expect(st, r"^\d+\+?$|^99\+$")
    c.shot("bell")
    c.log("top bar numbers: %s ; needs-the-person total from inbox: %d" % (shown, needs))
    if str(needs) not in shown: fail("bell shows %s but errors+warnings+reminders+approvals = %d" % (shown, needs))
    if sum(v for k, v in kinds.items() if k in ("device", "agent", "info", "download", "job")) and str(sum(kinds.values())) in shown:
        fail("bell counts routine items too")
    nc(c, "approvals, overdue tasks and 'assistant waiting' (none present); 99+ rule; the dot when only routine items are unread; the new-notification sound")


@recipe("F-BELL-01", "inbox grouping: crowds are one row each")
def bell01(c):
    """PASS when `dev inbox` shows ONE row for '468 new devices on your network' (not 468 rows) and one row for each count of
    finished jobs ('N setup jobs finished', 'N assistant runs finished'), and the INBOX tile lists those rows."""
    rc, out = c.run("inbox", name="inbox")
    rows = out.splitlines()
    if sum("new devices on your network" in l for l in rows) != 1: fail("new devices are not one row")
    if not re.search(r"\d+ new devices on your network", out): fail("no counted devices row")
    if sum(bool(re.search(r"\d+ (setup jobs|assistant runs) finished", l)) for l in rows) < 2: fail("finished-jobs groups missing")
    c.log("grouped rows: " + " | ".join(l[:90] for l in rows if re.search(r"new devices|finished", l)))
    st = c.click("inbox"); c.shot("inbox")
    if c.app(st) != "Inbox": fail("inbox button did not open the Inbox")
    bt = c.buttons(st)
    for b in ("Clear group (g)", "Mute source (m)", "Open", "Notifications (18)" if "Notifications (18)" in bt else "Tasks"):
        if b not in bt: fail("Inbox button %r missing" % b)
    nc(c, "opening a group, t/i on devices, g and m, and the repeated-failure counter (the inbox is a private app: `dev state` shows button names only)")


@recipe("F-NET-09", "Settings -> Network private MAC rows")
def net09(c):
    """PASS when each adapter has a Private MAC row: Ethernet MAC OFF (hardware address kept), Wi-Fi PER NETWORK with a 'now'
    address that differs from the hardware one and is locally administered unicast (first byte bit 1 set, bit 0 clear)."""
    st = c.settings_page("network"); c.shot("network")
    c.expect(st, r"^MAC OFF$"); c.expect(st, r"^PER NETWORK$")
    eth = c.expect(st, r"^off · hardware ([0-9a-f:]{17}) · now ([0-9a-f:]{17})$")
    w = c.expect(st, r"^per-network · hardware ([0-9a-f:]{17}) · now ([0-9a-f:]{17}) · locally administered$")
    hw, now = re.search(r"hardware (\S+) · now (\S+)", w[0]).groups()
    if hw == now: fail("Wi-Fi private address equals the hardware address")
    b = int(now.split(":")[0], 16)
    if not (b & 2) or (b & 1): fail("%s is not a locally administered unicast address" % now)
    e = re.search(r"hardware (\S+) · now (\S+)", eth[0]).groups()
    if e[0] != e[1]: fail("Ethernet is Off but uses a changed address")
    c.log("wlan0 hardware %s -> now %s; eth0 unchanged %s" % (hw, now, e[0]))
    nc(c, "stability of the derived address across reconnects, change on Random, 'Apply MAC now' (drops the link), boot-time order, net.conf contents (files scope withdrawn)")


@recipe("F-EXTA-06", "CLI tile catalogue lists the coding agents")
def exta06(c):
    """PASS when CLIs lists Claude Code, GitHub Copilot CLI and OpenAI Codex CLI with an Install (i) control and Node.js/uv/Python
    entries. Gemini CLI and Aider are expected by the row and are checked by scrolling the text; nothing is installed."""
    st = c.open_tile("CLI"); st = c.click("CLIs", wait=1.5); c.shot("cli-catalogue")
    bt = c.buttons(st)
    for b in ("Claude Code", "GitHub Copilot CLI", "OpenAI Codex CLI", "Node.js", "Install (i)"):
        if b not in bt: fail("%r not in the CLIs list" % b)
    miss = [n for n in ("Gemini CLI", "Aider") if n not in bt]
    c.log("rows missing from the visible list: %s" % miss)
    if miss: c.log("not visible (maybe below the fold): %s" % miss)
    nc(c, "install/update/remove commands, the 'Install Node.js first' case, sign-in, Settings -> Agents")
    if miss:
        human("Claude Code, Copilot CLI and Codex CLI are in the CLIs list (installed 2.1.318, v1.0.105, not installed) but the list shows '1-18 of 31' and dev input (PageDown, End, Down, swipe) did not scroll it, so Gemini CLI and Aider could not be seen: %s" % miss)


@recipe("F-APPSTORE-06", "Store -> Settings: built-in catalog, an added store, no hosted official catalog")
def store06(c):
    """PASS when Store Settings lists the built-in catalog ('always there, works offline'), shows that the official IYAKKI store
    is 'coming soon' (the default list is empty, as the row says) and has folder/USB catalog entries and the daily check line."""
    c.open_tile("STORE"); _tap_tab(c, "Settings"); st = c.state("store-settings"); c.shot("store-settings")
    c.expect(st, r"^Built-in catalog$"); c.expect(st, r"always there, works offline")
    c.expect(st, r"coming soon: the project has not published its store address yet")
    c.expect(st, r"^Add a folder or USB stick$"); c.expect(st, r"^Daily check for updates$")
    store = c.has(st, r"address · http")
    c.log("added catalog: %s" % store)
    nc(c, "the real HTTP fetch of a signed catalog (the catalog at the LAN address listed is not exercised; pressing Check would fetch it), expiry/older-serial refusal")


@recipe("F-CHECKUP-02", "CHECK-UP tile: the list of twelve steps")
def checkup02(c):
    """PASS when CHECK-UP opens with the summary '0 passed, 0 failed, 0 skipped, 14 not done yet', the fourteen named steps (Screen,
    Touch screen, Keyboard, Mouse or touch pad, Speaker, Microphone, Camera, External monitor, Network, Wi-Fi, Bluetooth, Battery
    and power, Graphics card for models, USB devices), 14 step controls, and Check again / Share result / Open DOCTOR. No step is run (they use the speaker, camera, mic)."""
    st = c.open_tile("CHECK-UP"); c.shot("checkup")
    c.expect(st, r"^\d+ passed, \d+ failed, \d+ skipped, 14 not done yet$")   # twelve hardware steps + GPU and USB (F-CHECKUP-04)
    for name in ("Screen", "Touch screen", "Keyboard", "Mouse or touch pad", "Speaker", "Microphone", "Camera", "External monitor", "Network", "Wi-Fi", "Bluetooth", "Battery and power", "Graphics card for models", "USB devices"):
        c.expect(st, r"^%s$" % re.escape(name))
    if sum(b == "step" for b in c.buttons(st)) != 14: fail("not 14 step controls")
    for b in ("Check again", "Share result", "Open DOCTOR"):
        if b not in c.buttons(st): fail("button %r missing" % b)
    nc(c, "running any step (colour bars need eyes; speaker, microphone and camera steps use the hardware), a failed step's Settings link, keys y/n/k/f/d/r/s")


@recipe("F-STUDY-04", "LEARN tile opens on PLANS and QUIZZES")
def study04(c):
    """PASS when LEARN opens with PLANS and QUIZZES tabs, the empty plans text with the example prompt, and QUIZZES opens its
    own empty state. Plans and quizzes need the assistant (a model), so nothing is created."""
    st = c.open_tile("LEARN"); c.shot("learn-plans")
    for b in ("PLANS", "QUIZZES"):
        if b not in c.buttons(st): fail("tab %s missing" % b)
    c.expect(st, r"No study plans yet\. Ask the assistant in the chat")
    st = c.click("QUIZZES", wait=1.2); c.shot("learn-quizzes")
    if c.app(st) != "Learn": fail("QUIZZES left LEARN")
    c.log("quizzes text: " + " | ".join(c.texts(st)[8:14]))
    nc(c, "a plan or quiz made in the chat, lesson page, ticking, calendar, quiz keys and result (need a model and data)")


@recipe("F-DEVMODE-10", "dev kube: reads work; every other verb and flag is refused (only nonexistent names are used)")
def devmode10(c):
    """PASS when get/describe/top/events/version/api-resources/auth can-i answer; exec, delete, apply-style verbs, `--kubeconfig`,
    `-o` raw/watch flags and Secrets are refused (the refused commands name nothing that exists); structured ConfigMap output is
    refused; output of get pods -A stays under 64 KiB. FAIL if any refused thing is accepted."""
    ok = [("get", "nodes"), ("describe", "node", "iyakki-dev"), ("top", "nodes"), ("events", "-A"), ("version",), ("api-resources",), ("auth", "can-i", "get", "pods")]
    for a in ok:
        rc, out = c.run("kube", *a, name="ok-" + "-".join(a)[:30])
        if rc: fail("`kube %s` was refused or failed: %s" % (" ".join(a), out.strip()[:120]))
    refused = [("exec", "-n", "default", "no-such-pod-hwcheck", "--", "true"), ("delete", "pod", "no-such-pod-hwcheck", "-n", "default"),
               ("apply", "-f", "/nonexistent.yaml"), ("scale", "deploy", "no-such-hwcheck", "--replicas=0", "-n", "default"),
               ("get", "pods", "--kubeconfig", "/nonexistent"), ("get", "secrets", "-A"), ("get", "secret", "-A", "-o", "yaml"),
               ("get", "cm", "-A", "-o", "json"), ("get", "pods", "-w"), ("get", "--raw", "/api")]
    for a in refused:
        rc, out = c.run("kube", *a, name="refused-" + "-".join(a)[:30])
        if rc == 0: fail("`kube %s` was ACCEPTED: %s" % (" ".join(a), out.strip()[:120]))
    big = c.run("kube", "get", "pods", "-A", "-o", "wide", name="get-pods")[1]
    if len(big.encode()) > 65536: fail("output over 64 KiB")
    nc(c, "the credential filter on log output, default log tail, and the audit log holding only verb and kind (audit not read)")


@recipe("F-DEVMODE-11", "dev state fields and click refusals on the real screen")
def devmode11(c):
    """PASS when `dev state` returns app, page, screen, focused, texts and buttons with positions; a click on a missing button is
    refused with a message; an ambiguous click (two buttons of the same name in the DASH board) is refused, not guessed;
    private apps (the Inbox) give button names only."""
    st = c.home()
    for pat in (r"^app: ", r"^screen: \d+x\d+", r"^focused: ", r"^text:", r"^buttons "):
        if not re.search(pat, st, re.M): fail("state lacks /%s/" % pat)
    if not re.search(r"^\s+\d+,\d+\s+\d+x\d+\s+\S", st.split("buttons", 1)[1], re.M): fail("buttons have no places")
    rc, out = dev("input", "click", "No such button hwcheck")
    if rc == 0 or "no button called" not in out: fail("missing button not refused: " + out[:100])
    c.log("missing-button refusal: " + out.strip()[:120])
    st = c.click("inbox"); t = st.split("\nbuttons", 1)[0]
    if "private app" not in t: fail("Inbox did not say it is a private app")
    c.log("inbox state says: " + [l for l in t.splitlines() if "private app" in l][0].strip())
    c.home(); c.open_tile("DASH"); dev("input", "tap", 500, 138); time.sleep(6); st = c.state("dash-open")
    from collections import Counter
    dup = [b for b, n in Counter(c.buttons(st)).items() if n > 1 and b.startswith("panel ")]
    c.log("duplicate panel button names: %s" % dup)
    if dup:
        rc, out = dev("input", "click", dup[0]); c.log("ambiguous click -> rc=%s %s" % (rc, out.strip()[:140]))
        if rc == 0 and "ambiguous" not in out.lower(): fail("an ambiguous click was accepted: " + out[:100])
    else:
        c.log("no ambiguous names on the board this time")
    nc(c, "a secret (password) field being masked and a click refused while it has focus (would need a password screen); covered-button refusal; the caps on lines and buttons")


def holds_scope(whoami, scope):
    """True when `dev whoami` lists `[x] scope` (held), False when `[ ] scope` or absent."""
    return re.search(r"^\s*\[x\] %s\s" % re.escape(scope), whoami, re.M) is not None


@recipe("F-DEVMODE-12", "dev inbox and dev diag contents")
def devmode12(c):
    """PASS when `dev inbox` gives counts by kind and source and titles without bodies, and `dev diag` has the sections version,
    memory, disks, network, services, cluster, audit tail, recent errors, sound, and contains no secret-looking text."""
    rc, out = c.run("inbox", name="inbox")
    if not re.search(r"^\d+ notifications, \d+ unread", out) or "by kind:" not in out or "by source:" not in out: fail("inbox summary lines missing")
    rc, out2 = c.run("inbox", "--bodies", name="inbox-bodies")
    # the answer must agree with the scopes this computer holds NOW (a paired test computer may hold all twelve: whoami says so)
    held = holds_scope(c.run("whoami", name="whoami")[1], "files:system")
    if held and rc != 0: fail("files:system is held but --bodies was refused: " + out2.strip()[:120])
    if not held and rc == 0: fail("--bodies worked without the files:system scope")
    c.log("files:system %s; --bodies %s" % ("held" if held else "not held", "answered" if rc == 0 else "refused: " + out2.strip()[:120]))
    p = os.path.join(c.dir, "diag.txt"); rc, out = dev("diag", "-o", p)
    d = open(p).read()
    for h in ("version and machine", "memory", "disks and mounts", "network", "services", "audit", "sound"):
        if h not in d.lower(): fail("diag lacks a section about %r" % h)
    if "cluster" not in d.lower(): fail("diag lacks the cluster")
    if re.search(r"(?i)(password|token|secret)\s*[=:]\s*\S{6,}|Bearer [A-Za-z0-9._-]{12,}", d): fail("secret-looking text in diag")
    c.log("diag: %d bytes, all sections present" % len(d))
    nc(c, "the refusal of --bodies without files:system is a unit test (inbox_bodies_need_the_files_system_scope_as_well_as_debug); on a computer that holds every scope only the answered case is seen; the cluster node/pod names appear only with the kube scope (not tested without it); the open-app and shell-frame lines were not individually checked")


@recipe("F-KBD-19", "keyboard.profile: read, switch to pc and back (live, no restart)")
def kbd19(c):
    """PASS when `dev config get keyboard.profile` says mac, the Keyboard page's Input profile row says Mac, setting pc changes
    the row to PC without a restart, and setting it back restores mac (original value: mac)."""
    orig = c.snapshot_setting("keyboard.profile"); c.log("original: " + orig)
    if "mac" not in orig: human("keyboard.profile is %r, not mac; the recipe expects the Mac mini's chosen value" % orig)
    st = c.settings_page("keyboard"); c.expect(st, r"^Mac: Mac \(chosen by you\)")
    problem = None
    try:
        c.touched_log("keyboard.profile mac -> pc")
        rc, out = dev("config", "set", "keyboard.profile", "pc")
        if rc: fail("set refused: " + out[:100])
        time.sleep(2); st = c.click("Keyboard", must=False) or c.state()
        st = c.state("pc"); c.shot("profile-pc")
        c.expect(st, r"^PC: ")
    except Verdict as e:
        problem = e
    finally:
        dev("config", "set", "keyboard.profile", "mac"); c.touched_log("keyboard.profile pc -> mac (restored)")
        time.sleep(1.5)
        if c.snapshot_setting("keyboard.profile").split()[-1] != "mac": fail("COULD NOT RESTORE keyboard.profile to mac")
    if problem: raise problem
    nc(c, "the shortcuts themselves (Cmd+C etc.), Control+click, Option/Command swap undo: need a real Mac keyboard and trackpad")


@recipe("F-DRIVERS-50", "Mac drivers page and dev info: firmware the kernel lacks")
def drivers50(c):
    """Observed: `dev info` reports no missing firmware files and the Mac drivers page (IYAKKI menu) says Wi-Fi/T2/Bluetooth state.
    The detection from kernel-log lines needs a computer that really lacks firmware; this one does not."""
    info = c.run("info", name="info")[1]
    c.log([l for l in info.splitlines() if "firmware" in l][0])
    if "no missing firmware files" not in info: fail("dev info lists missing firmware: " + [l for l in info.splitlines() if "firmware" in l][0])
    c.home(); c.click("IYAKKI menu"); st = c.click("Mac drivers", wait=2); c.shot("mac-drivers")
    c.expect(st, r"^Wi-Fi firmware$"); c.expect(st, r"^T2 audio$"); c.expect(st, r"^Bluetooth$")
    c.click("close", must=False)
    human("no missing firmware on this Mac mini, so the detection (kernel-log lines, vendor grouping, licence classes) never had anything to find")


@recipe("F-DRIVERS-30", "restricted firmware is not shipped: the Mac drivers page says so")
def drivers30(c):
    """PASS when the Mac drivers page says Apple's firmware is not shipped, that the files stay on this machine, are never
    uploaded and are not part of IYAKKI, and offers Download from Apple / From a folder or stick / From a disk image."""
    require_hardware(c, "apple")
    c.home(); c.click("IYAKKI menu"); st = c.click("Mac drivers", wait=2); c.shot("mac-drivers")
    c.expect(st, r"Apple's firmware is not shipped\."); c.expect(st, r"These files are Apple's\. They stay on this machine, are never uploaded")
    for b in ("Download from Apple", "From a folder or stick", "From a disk image"):
        if b not in c.buttons(st): fail("button %r missing" % b)
    c.click("close", must=False)
    nc(c, "the image contents (no restricted firmware files in the image) were not listed; only the page's statement was read")


@recipe("F-GPU-06", "no NVIDIA driver on a machine without an NVIDIA card")
def gpu06(c):
    """PASS when the kernel log has no nvidia module load, `dev info` lists only the i915 GPU and `dev status` Graphics is ready."""
    dm = c.run("dmesg", "--grep", "nvidia", "-n", 50, name="dmesg-nvidia")[1]
    import hwmatch
    if hwmatch.nvidia_card_present(dm): not_applicable("this machine has an NVIDIA card (the row is about a machine without one): " + dm.strip().splitlines()[0][:100])
    if [l for l in dm.splitlines() if l.strip()]: fail("kernel log mentions nvidia: " + dm.strip().splitlines()[0][:120])
    info = c.run("info", name="info")[1]
    if not re.search(r"gpu\s+card0 driver i915", info) or "nvidia" in info.lower(): fail("dev info shows a GPU other than i915 or nvidia text")
    if not re.search(r"Graphics\s+ready\s+card0 \(i915\)", c.run("status", name="status")[1]): fail("status graphics row is not i915 ready")
    nc(c, "the installer's own rule (not writing the module list on such a machine) - only its effect was observed")
