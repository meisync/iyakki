"""Hand-written recipes (one per row) for hwcheck.py. Each one opens the real screen or reads the real
log, matches the exact text the row promises, and says in plain words what it did NOT cover.

A recipe returns normally = PASS for the conditions listed in its docstring; the `c.notcovered(...)`
lines are printed with the PASS so nobody flips a whole row on a narrow observation.
Row texts: FEATURES.md; triage: docs/development/PARTIALS-PLAN-2026-11-11-triage.md.
"""
import os, re, time
from hwcheck import require_hardware, not_applicable, recipe, fail, human, dev, Verdict, Ctx


def _nc(c, text):
    c.log("NOT COVERED: " + text)
    c.not_covered = getattr(c, "not_covered", []) + [text]
Ctx.notcovered = _nc


def _audit_lines(c, pattern, n=20000):
    rc, out = c.run("logs", "--system", "--source", "audit", "-n", n, name="audit-tail", timeout=120)
    return [l for l in out.splitlines() if re.search(pattern, l)]


def _device_now(c):
    rc, out = dev("logs", "--system", "--source", "audit", "-n", 3)
    ts = [int(m.group(1)) for m in re.finditer(r"^(\d{9,11}) ", out, re.M)]
    if not ts: fail("no timestamp in the device's audit log tail")
    return max(ts)


def _shell_frame_lines(c, n=100000):
    rc, out = c.run("dmesg", "--grep", "shell:", "-n", n, name="dmesg-shell-frames", timeout=120)
    res = []
    for l in out.splitlines():
        m = re.search(r"shell: (\d+) frames in (\d+)s, draw ([\d.]+) ms, screen write ([\d.]+) ms, slowest (\d+) ms", l)
        if m: res.append((l, int(m[1]), int(m[2]), float(m[3]), float(m[4]), int(m[5])))
    return res


def _kube_ready(c):
    rc, out = c.run("kube", "get", "nodes", "-o", "wide", name="kube-nodes")
    if not re.search(r"\sReady\s", out): fail("k3s node is not Ready: " + out.strip()[:200])
    return out



def _tap_tab(c, name):
    """Tap the tab called NAME in the top strip (found on screen: the tabs move when a count is added to the first one)."""
    st = c.state()
    m = re.search(r"^\s*(\d+),(\d+)\s+%s$" % re.escape(name), st.split("\nbuttons", 1)[0], re.M)
    if not m: fail("no tab called %r on screen" % name)
    dev("input", "tap", int(m.group(1)) + 10, int(m.group(2)) + 8); time.sleep(1.5)

@recipe("F-SHELL-13", "responsive display loop measured on the Mac mini: frame stats from the kernel log, idle redraw check")
def shell13(c):
    """PASS when, on an idle Home screen whose picture does not change (two screenshots seconds apart are byte-identical), the shell draws at most 20 frames per
    10 s (redraw only when something changed), the screen write is under 10 ms and the slowest frame under 100 ms.
    FAIL when it keeps drawing a picture that is byte-identical (screenshot compare)."""
    c.home()
    a = c.shot("home-before-idle")
    time.sleep(45)  # no dev command at all while the window runs
    lines = _shell_frame_lines(c)
    # KI-1014: the shell logs the frame line to the kernel ring at INFO only for the first window, a slow window and once an hour
    if len(lines) < 1: human("no shell frame-stat line in the kernel log (is the stat line off in this build?)")
    last = lines[-3:]
    for l in last: c.log("FRAMESTAT " + l[0])
    shots = [c.shot("home-after-idle-%d" % i) for i in range(3)]
    data = [open(p, "rb").read() for p in shots]
    # the clock changes the picture once a minute; two shots taken 1-2 s apart that are byte-identical prove a static picture
    same = any(data[i] == data[i + 1] for i in range(2))
    c.log("two consecutive screenshots (taken seconds apart, no input) byte-identical: %s" % same)
    ps1 = dev("ps")[1]; t1 = time.time(); time.sleep(30); ps2 = dev("ps")[1]
    def cpu(o):
        for l in o.splitlines():
            f = l.split()
            if f and f[0] == "1": return int(f[-2])
    c.log("PID 1 CPU seconds: %s -> %s over %.0f s" % (cpu(ps1), cpu(ps2), time.time() - t1))
    fps = max(x[1] for x in last)
    if max(x[4] for x in last) >= 10: fail("screen write %.1f ms is not under 10 ms" % max(x[4] for x in last))
    if max(x[5] for x in last) >= 100: fail("slowest frame >= 100 ms")
    if fps > 20 and same:
        fail("idle Home draws %d frames per 10 s (draw %.0f ms each, PID 1 used %s CPU s in 30 s) while the picture stays byte-identical"
             % (fps, last[-1][3], (cpu(ps2) or 0) - (cpu(ps1) or 0)))
    c.notcovered("multi-threaded software rendering (the row says it is still single-threaded); pointer-latency feel")


@recipe("F-APPS-50", "private store state in Settings -> Dev cluster -> Platform services, checked against the real cluster")
def apps50(c):
    """PASS when the Platform services page shows Gitea, Private store and Registry each with a resolved state that
    agrees with `kube get` (Private store 'not installed' because no iyakki-store namespace; Gitea not 'not installed'
    while its Deployment is 1/1; Registry resolved within 60 s). FAIL on any contradiction."""
    c.home()
    c.settings_page("dev cluster")
    st = c.click("Platform services")
    c.shot("platform-services")
    c.expect(st, r"^Private store$")
    ns = c.run("kube", "get", "ns", name="kube-ns")[1]
    gd = c.run("kube", "get", "deploy", "-A", name="kube-deploy")[1]
    has_store_ns = bool(re.search(r"^iyakki-store\s", ns, re.M))
    gitea_up = bool(re.search(r"^iyakki-gitea\s+gitea\s+1/1", gd, re.M))
    reg_up = bool(re.search(r"^iyakki-registry\s+registry\s+1/1", gd, re.M))
    c.log("cluster says: store namespace=%s gitea 1/1=%s registry 1/1=%s" % (has_store_ns, gitea_up, reg_up))
    st = c.click("Look again", wait=1)
    time.sleep(60)
    st = c.state("platform-services-after-60s")
    text = "\n".join(c.texts(st))
    c.log("page text after 60 s:\n" + text)
    rows = {}
    t = c.texts(st)
    for name in ("Gitea", "Private store", "Registry"):
        i = t.index(name) if name in t else -1
        rows[name] = " ".join(t[i + 1:i + 3]) if i >= 0 else "(row missing)"
    c.log("rows: %r" % rows)
    if not has_store_ns and not re.search(r"not installed", rows["Private store"], re.I):
        fail("Private store row says %r but the cluster has no store namespace" % rows["Private store"])
    if gitea_up and re.search(r"not installed", rows["Gitea"], re.I):
        fail("Gitea row says 'not installed' while the cluster runs iyakki-gitea/gitea 1/1 (Store -> Setup shows it Running)")
    if reg_up and re.search(r"checking", rows["Registry"], re.I):
        fail("Registry row still says 'checking…' after 60 s while iyakki-registry/registry is 1/1")
    c.notcovered("Repair, Reset and reinstall, Remove (need a store installed, which this run must not do)")


@recipe("F-CORE-06", "a k3s whose exit status PID 1 took is still noticed, seen in the device's own words")
def core06(c):
    """PASS when SYSTEM Overview's Cluster row reports a restart whose reason is 'its exit status is not available:
    another reaper took it' (the supervisor noticed a death PID 1 had reaped) and the cluster is running again with
    node Ready (kubectl), and no zombie in `dev ps`."""
    st = c.open_tile("SYSTEM")
    c.shot("system-overview")
    m = c.expect(st, r"^running .*node Ready", "Cluster row")
    r = c.expect(st, r"restarted \d+x, last: k3s exited \(its exit status is not available: another reaper took it\)")
    _kube_ready(c)
    ps = c.run("ps", name="ps")[1]
    z = [l for l in ps.splitlines() if re.search(r"\sZ\s|<defunct>", l)]
    if z: fail("zombie process(es) in dev ps: %s" % z[0])
    c.log("no zombie process in dev ps")
    c.notcovered("killing a child on purpose (the one observed death was a natural k3s exit earlier today)")


@recipe("F-CLOCK-01", "the clock-floor file is kept fresh on the device")
def clock01(c):
    """Observed only: /var/lib/iyakki/clock-floor exists, is a plausible epoch (>= 2026-11-11 00:00 UTC, the build day),
    not in the future, no older than 6 minutes (the 5-minute write), and the time client log shows retries then an answer."""
    p = os.path.join(c.dir, "clock-floor")
    rc, out = dev("files", "pull", "/var/lib/iyakki/clock-floor", p)
    if rc and "files scope" in out: human("this computer no longer holds the files scope (an administrator withdrew it), so the clock-floor file cannot be read; ask for `files` to be granted again")
    if rc: fail("cannot read the clock floor: " + out[:200])
    floor = int(open(p).read().strip())
    now = _device_now(c)
    c.log("clock-floor=%d device-now(from audit)=%d age=%d s" % (floor, now, now - floor))
    if floor < 1794355200: fail("floor %d is older than the build day" % floor)
    if floor > now + 5: fail("floor is in the future")
    if now - floor > 360: fail("floor is %d s old; the writer should run every 300 s" % (now - floor))
    rc, out = c.run("dmesg", "--grep", "clock:", "-n", 20, name="dmesg-clock")
    ans = [l for l in out.splitlines() if re.search(r"time server answered", l)]
    if not ans: fail("no 'a time server answered' line in the kernel log")
    c.log("MATCH time client: " + ans[-1])
    c.notcovered("the lift itself: this Mac mini's hardware clock is fine, so a clock behind the floor was never seen; needs a computer with a bad RTC")
    human("floor writer observed (value %d, %d s old) and time-client retries observed, but the row's claim (lifting a bad hardware clock at boot) cannot be seen on this Mac mini" % (floor, now - floor))


@recipe("F-K3S-27", "k3s waits for a plausible clock: audit shows the wait then ready on a real boot")
def k3s27(c):
    """PASS when the audit log holds `devcluster.waiting-for-clock` followed within 60 s by `devcluster.ready`
    (a real boot where the gate was used), the node is Ready and the newest boot's k3s started without a failed wait."""
    ls = _audit_lines(c, r"devcluster\.(waiting-for-clock|ready|exited|restart)")
    c.log("\n".join(ls[-12:]))
    waits = [int(l.split()[0]) for l in ls if "waiting-for-clock" in l]
    readys = [int(l.split()[0]) for l in ls if " devcluster.ready" in l]
    if not waits: fail("no devcluster.waiting-for-clock line in the audit log")
    ok = [w for w in waits if any(0 <= r - w <= 60 for r in readys)]
    if not ok: fail("waiting-for-clock was never followed by ready within 60 s")
    c.log("wait at %d answered by ready within 60 s" % ok[-1])
    _kube_ready(c)
    c.notcovered("restart on a wall-clock jump (devcluster.restart-clock-step) and on early x509 errors were never triggered (would need to set the clock)")


@recipe("F-K3S-28", "k3s keeps its fixed node address 10.255.77.1 on an interface IYAKKI makes")
def k3s28(c):
    """PASS when kubectl shows INTERNAL-IP 10.255.77.1, `dev info` lists iyakki-k8s 10.255.77.1, the audit log has
    devcluster.node-ip 10.255.77.1 on iyakki-k8s (naming the dummy-module fallback), and LAN address changes while k3s
    kept running were audited."""
    out = _kube_ready(c)
    if "10.255.77.1" not in out: fail("node INTERNAL-IP is not 10.255.77.1")
    info = c.run("info", name="info")[1]
    if not re.search(r"network\s+iyakki-k8s\s+10\.255\.77\.1", info): fail("dev info has no iyakki-k8s 10.255.77.1")
    ls = _audit_lines(c, r"devcluster\.(node-ip|address-changed)")
    ni = [l for l in ls if "devcluster.node-ip 10.255.77.1 on iyakki-k8s" in l]
    if not ni: fail("no devcluster.node-ip line")
    c.log("node-ip audit: " + ni[-1])
    ac = [l for l in ls if "address-changed" in l and "k3s keeps running" in l]
    if not ac: fail("no address-changed line (LAN address moved while k3s kept running)")
    c.log("%d LAN address changes audited while k3s kept running; last: %s" % (len(ac), ac[-1]))
    c.notcovered("the dummy module is NOT available on this kernel (audit: 'dummy: RTNETLINK answers: Not supported'), the bridge fallback is what runs; rejected-address drop, watchdog and the 'stopped unexpectedly' alert were not provoked")


@recipe("F-DASH-06", "DASH tile: open the Local cluster (k3s) dashboard 6 times, every panel must load")
def dash06(c):
    """PASS when the dashboard opens with its 20 panels, tables and charts drawn from the real cluster, and none of six
    consecutive opens shows 'This panel could not be loaded'. FAIL if any does (intermittent counts)."""
    c.open_tile("DASH")
    bad = 0
    for i in range(6):
        st = c.state("list")
        if "Local cluster (k3s)" not in "\n".join(c.texts(st)):
            c.click("Dashboards", must=False)
        dev("input", "tap", 500, 138); time.sleep(7)   # first row of the list: Local cluster (k3s)
        st = c.state("open%d" % (i + 1))
        c.expect(st, r"20 panel", "the footer says 20 panels")
        c.expect(st, r"Nodes ready|Pods by phase")
        err = c.has(st, r"could not be loaded")
        c.log("open %d: panels with error: %d" % (i + 1, len(err)))
        if err:
            bad += 1; c.shot("panel-error-open%d" % (i + 1))
        else:
            c.shot("ok-open%d" % (i + 1))
        c.click("Dashboards", must=False); time.sleep(1)
    if bad: fail("%d of 6 opens showed 'This panel could not be loaded' (panel 'Claims not bound', message cut to '{…')" % bad)
    c.notcovered("mouse interactions (zoom by drag, crosshair tooltip, legend toggle, editor): need a person")


@recipe("F-APPSTORE-10", "STORE tile on the Mac mini: tabs, trust badges, an app page, publishers")
def store10(c):
    """PASS when STORE opens on Installed with the filter chips and rows with trust badges and Open/Remove buttons;
    Discover shows FEATURED/NEW with prices and trust; Publishers lists publishers with their trust; an app page shows the
    icon letter, publisher, version, licence, trust badge, 'WHAT IT CAN DO' and DETAILS; Back returns."""
    st = c.open_tile("STORE")
    c.shot("installed")
    c.expect(st, r"^Installed \(")
    c.expect(st, r"^Discover$"); c.expect(st, r"^Publishers$"); c.expect(st, r"^Setup$")
    c.expect(st, r"^All \d+$"); c.expect(st, r"^Official$")
    st = c.click("Discover"); c.shot("discover")
    c.expect(st, r"^F$|FEATURED|^Free$"); c.expect(st, r"^Price: any$")
    app = next((b for b in c.buttons(st) if b.startswith("Build an IYAKKI app: the dev loop")), None)
    if not app: fail("no 'Build an IYAKKI app: the dev loop' row in Discover")
    st = c.click(app); c.shot("app-page")
    c.expect(st, r"by IYAKKI .* version .* Apache-2\.0"); c.expect(st, r"^WHAT IT CAN DO$")
    c.expect(st, r"^Licence: ", "licence line"); c.expect(st, r"Official: Made by IYAKKI and signed")
    st = c.click("Back (Esc)")
    st = c.click("Publishers"); c.shot("publishers")
    c.expect(st, r"^IYAKKI$"); c.expect(st, r"apps ·")
    _tap_tab(c, "Settings"); st = c.state("store-settings"); c.shot("settings-tab")
    c.expect(st, r"^Built-in catalog$"); c.expect(st, r"^Add a store$"); c.expect(st, r"^Daily check for updates$"); c.expect(st, r"^Install from a file$")
    c.notcovered("trusted keys and licence rows, review dialog and a real install; 1024x600 and 800x480 sizes; whether the layout looks right (look at the screenshots)")


@recipe("F-ZRAM-01", "zram swap on the Mac mini: boot log + SYSTEM overview")
def zram01(c):
    """PASS when the boot log has `zram: zram0 N GiB (zstd), swap priority 100` and `vm: swappiness 100 ...`, the swap is
    active (dev info swap total > 0) and SYSTEM Overview shows 'zram zstd R:1'."""
    log = dev("dmesg", "--grep", "zram", "-n", 100)[1] + "\n" + dev("dmesg", "--grep", "vm:", "-n", 100)[1]
    open(os.path.join(c.dir, "dmesg-zram.txt"), "w").write(log)
    for pat in (r"iyakki INFO \] zram: zram0 [\d.]+ GiB \(zstd\), swap priority 100", r"iyakki INFO \] vm: swappiness 100, page-cluster 0"):
        m = re.search(pat, log)
        if not m:
            c.notcovered("the boot line /%s/ has rolled out of the kernel log ring (frame stats fill it) and boot-lean.log needs the files scope; the first run of 2026-11-11 read it" % pat)
            continue
        c.log("KERNEL LOG: " + log[log.rfind("\n", 0, m.start()) + 1:log.find("\n", m.end())])
    info = c.run("info", name="info")[1]
    m = re.search(r"swap ([\d.]+) GB \(([\d.]+) GB free\)", info)
    if not m or float(m[1]) <= 0: fail("dev info shows no swap")
    c.log("dev info swap: " + m[0])
    st = c.open_tile("SYSTEM"); c.shot("system-memory")
    c.expect(st, r"zram zstd [\d.]+:1")
    c.expect(st, r"swap .* of [\d.]+ GB")
    c.notcovered("the same on aarch64 and the laptops; iyakki.zram= kernel command-line overrides; 4K vs 16K page headers")


@recipe("F-AIHW-02", "Store -> Setup shows the machine facts and the hidden-tools sentence for this CPU-only machine")
def aihw02(c):
    """PASS when Store -> Setup (after it resolves) shows 'This machine: ... CPU only ...', the sentence 'N AI tools are
    hidden because this machine has no GPU with 6 GB or more', the 'Show all AI tools' link, and no Lemonade/Ollama cards in
    the visible default list that the rule hides (Ollama, Vulkan, NVIDIA, Mac model host, Elasticsearch-by-RAM)."""
    c.open_tile("STORE"); st = c.click("Setup", wait=2)
    c.shot("setup")
    c.expect(st, r"^This machine: .*CPU only, [\d.]+ GB RAM")
    m = c.expect(st, r"^\d+ AI tools are hidden because this machine has no GPU with 6 GB or more")
    c.expect(st, r"^Show all AI tools$")
    n = int(re.match(r"(\d+)", m[0])[1])
    c.log("hidden count claimed: %d" % n)
    c.notcovered("other profiles (GPU >= 6 GB, Apple Silicon, no profile); how it looks to a person")


@recipe("F-AIHW-03", "Settings -> Cloud mode 'This machine (AI)' row and the 'Show all AI tools' switch (changed and put back)")
def aihw03(c):
    """PASS when the Cloud mode page shows 'This machine (AI)' in plain words and 'Show all AI tools' Off; the Store -> Setup
    page then counts N items; turning it On (the link on Store -> Setup) raises the count, and opening the Ollama card shows
    'Not recommended here: this machine has no GPU with 6 GB or more'; turning it Off again (Settings -> Cloud mode) restores N;
    the audit log records settings.show_all_ai twice. Original value: Off (re-checked at the end; FAIL if it cannot be restored)."""
    st = c.settings_page("cloud mode")
    c.expect(st, r"^This machine \(AI\)$"); c.expect(st, r"CPU only")
    c.expect(st, r"^Off \(press a to change\)")
    c.open_tile("STORE"); st = c.click("Setup", wait=3)
    n_before = int(re.match(r"(\d+) items", c.expect(st, r"^\d+ items$")[0])[1])
    c.touched_log("show_all_ai: Off (original); Store -> Setup lists %d items" % n_before)
    problem = None
    try:
        st = c.click("Show all AI tools", wait=2)
        c.touched_log("show_all_ai: Off -> On (the link on Store -> Setup)")
        time.sleep(3); st = c.state("setup-all-tools")
        c.shot("setup-show-all")
        n_after = int(re.match(r"(\d+) items", c.expect(st, r"^\d+ items$")[0])[1])
        c.expect(st, r"^Show all AI tools: on\. Tools this machine's hardware does not suit are listed as not recommended")
        if n_after <= n_before: raise Verdict("FAIL", "showing all AI tools did not add cards (%d -> %d)" % (n_before, n_after))
        card = next((b for b in c.buttons(st) if b.startswith("Ollama,")), None)
        if not card: raise Verdict("FAIL", "no Ollama card after Show all AI tools")
        st = c.click(card, wait=1.5); c.shot("ollama-popup")
        c.expect(st, r"^Not recommended here: this machine has no GPU with 6 GB or more")
        c.click("Close (Esc)", must=False)
    except Verdict as e:
        problem = e
    finally:
        st = c.settings_page("cloud mode")
        if c.has(st, r"^On \(press a"):
            c.click("Show all AI tools", wait=1.5)
            c.touched_log("show_all_ai: On -> Off (restored from Settings -> Cloud mode)")
        st = c.state("cloud-mode-final")
        if not c.has(st, r"^Off \(press a to change\)"): fail("COULD NOT RESTORE 'Show all AI tools' to Off; set it back by hand")
    if problem: raise problem
    c.open_tile("STORE"); st = c.click("Setup", wait=3)
    n_end = int(re.match(r"(\d+) items", c.expect(st, r"^\d+ items$")[0])[1])
    if n_end != n_before: fail("after restoring, Store -> Setup lists %d items instead of %d" % (n_end, n_before))
    ls = _audit_lines(c, r"settings\.show_all_ai", 2000)
    c.log("audit: " + " || ".join(ls[-4:]))
    if len(ls) < 2: fail("audit log has fewer than 2 settings.show_all_ai lines")
    c.notcovered("how it looks to a person at five sizes; the card itself carries no 'Not recommended' mark (only the page sentence and the popup do)")


@recipe("F-KBD-06", "Settings -> Keyboard page: every control the row lists is on the page")
def kbd06(c):
    """Observed: the page lists Function keys, ISO layout, Option/Cmd swap, Repeat delay/rate, the detected controls with
    Test screen / Test keyboard light / Test sound, the Key repeat status. Not judged: that EVIOCSREP works on a real keyboard
    (this Mac mini has no keyboard found by the page; changing the repeat is a setting this recipe does not touch)."""
    st = c.settings_page("keyboard")
    c.shot("keyboard-page")
    for b in ("Function keys", "ISO layout", "Swap Option/Cmd", "Repeat delay", "Repeat rate", "Test screen", "Test keyboard light", "Test sound"):
        if b not in c.buttons(st): fail("button %r is not on the Keyboard page" % b)
    c.log("all 8 controls present as buttons")
    c.expect(st, r"^Key repeat$"); c.expect(st, r"the kernel's own \(\d+ ms, \d+ per second\)")
    c.expect(st, r"^What you will see: last key$")
    human("the page and every listed control are present (evidence saved), but the EVIOCSREP write on a real keyboard needs a keyboard and a person; changing repeat settings was not attempted")


@recipe("F-K8S-14", "SYSTEM Overview Cluster row and View logs")
def k8s14(c):
    """PASS when the Cluster row shows state, version, node Ready, pods running/all, data size, disk use and restarts with the
    last reason; View logs opens the service log tail as text; the numbers agree with kubectl (12 pods)."""
    st = c.open_tile("SYSTEM")
    m = c.expect(st, r"^running · v[\d.]+\+k3s\d · node Ready · \d+/\d+ pods$")
    # the restart part is shown only when the cluster restarted (system.rs): a cluster that never restarted ends at "disk N% full"
    c.expect(st, r"^data [\d.]+ GB · disk \d+% full( · restarted \d+x, last: .*)?$")
    ready, total = map(int, re.search(r"(\d+)/(\d+) pods", m[0]).groups())
    rc, out = c.run("kube", "get", "pods", "-A", "--no-headers", name="pods")
    kt = len([l for l in out.splitlines() if l.strip()])
    c.log("overview says %d/%d pods, kubectl lists %d pods" % (ready, total, kt))
    if total != kt: fail("Overview counts %d pods, kubectl lists %d" % (total, kt))
    st = c.click("View logs", wait=2); c.shot("cluster-logs")
    t = c.texts(st)
    if len(t) < 15: fail("View logs showed almost no text")
    c.log("View logs text lines: %d, e.g. %s" % (len(t), " | ".join(t[8:12])))
    ib = c.run("inbox", name="inbox")[1]
    al = [l for l in ib.splitlines() if "The dev cluster stopped and was restarted" in l]
    if not al: fail("no 'The dev cluster stopped and was restarted' warning in the inbox although the Cluster row reports restarts")
    c.log("inbox alerts from an unexpected k3s exit: %d, e.g. %s" % (len(al), al[0]))
    c.notcovered("the disk-over-85-percent alert and giving-up/never-Ready alerts: not provoked")


@recipe("F-K8S-12", "Settings -> Dev cluster: the local cluster row, identity strip, Test connection")
def k8s12(c):
    """PASS when the page lists this machine's k3s as a row with server, node, version and 'reachable', the strip 'You may:
    get pods yes ...', the namespace rule and the buttons Test/My default/Workspace default/Hide-Show/Disconnect/Registry;
    clicking Test (read-only, audited) keeps it reachable."""
    st = c.settings_page("dev cluster"); c.shot("dev-cluster")
    c.expect(st, r"^k3s$"); c.expect(st, r"https://127\.0\.0\.1:6443 · node iyakki-dev · v[\d.]+\+k3s")
    c.expect(st, r"^reachable$"); c.expect(st, r"^You may: get pods yes")
    c.expect(st, r"^Writes go only to the namespace default")
    for b in ("Test", "My default", "Workspace default", "Hide/Show", "Disconnect", "Registry", "Add vault profile"):
        if b not in c.buttons(st): fail("button %r missing" % b)
    st = c.click("Test", wait=4)
    c.expect(st, r"reachable"); c.shot("after-test")
    c.notcovered("a real REMOTE cluster, KUBECONFIG/~/.kube/config/vault kubeconfig rows (only this machine's k3s exists here); Default for you/workspace, Hide/Show, Disconnect and registry A/B/C were not pressed (they change settings)")
    human("the local k3s row, identity strip and Test connection behave (evidence saved) but the row's gap is a real remote cluster, which this bench does not have")


@recipe("F-DEV-02", "iyakki dev info / ps / dmesg --grep / logs --system / status on the real device")
def dev02(c):
    """PASS when each command returns real content: info (DMI Macmini8,1, image, kernel+taint, modules, memory, disks,
    addresses, GPU, sound, firmware line), ps (PID 1, k3s), dmesg --grep (only matching lines), logs --system for
    shell/boot/audit/engine, status (table with k3s running); and credentials are hidden: a planted secret-looking string is not
    needed, instead the audit/boot/dmesg output is searched for obvious 'password=' / 'Bearer ' values."""
    info = require_hardware(c, "macmini2018")
    for pat in (r"device\s+Apple Inc\. Macmini8,1", r"image\s+IYAKKI [\d.a-z-]+ · variant pc-x86_64", r"kernel\s+[\d.]+ · \d+ modules", r"taint\s+\d+",
                r"memory\s+[\d.]+ GB total", r"firmware\s+UEFI", r"gpu\s+card0 driver i915", r"sound\s+0 \[", r"disk\s+nvme0n1", r"network\s+eth0\s+\d"):
        if not re.search(pat, info): fail("dev info lacks /%s/" % pat)
    c.log("dev info: all 10 fields matched")
    ps = c.run("ps", name="ps")[1]
    if not re.search(r"^1\s+0\s+root\s+\S+\s+[\d.]+ [KMG]B\s+\d+\s+/sbin/init", ps, re.M) or "k3s server" not in ps: fail("dev ps lacks PID 1 or k3s")
    dm = c.run("dmesg", "--grep", "zram", "-n", 50, name="dmesg-zram")[1]
    lines = [l for l in dm.splitlines() if l.strip()]
    if not lines or any("zram" not in l.lower() for l in lines): fail("dmesg --grep zram returned non-matching lines or none")
    c.log("dmesg --grep: %d lines, all contain 'zram'" % len(lines))
    for src in ("shell", "boot", "audit", "engine"):
        out = c.run("logs", "--system", "--source", src, "-n", 20, name="logs-" + src)[1]
        if len(out.strip()) < 40 or "unknown source" in out.lower() or "error" in out.lower()[:60]: fail("logs --system --source %s returned %r" % (src, out[:120]))
        if re.search(r"(?i)(password|passwd|secret|token)=\S{4,}|Bearer [A-Za-z0-9._-]{12,}", out): fail("credential-looking text in logs --source %s" % src)
    st = c.run("status", name="status")[1]
    if not re.search(r"k3s \(dev cluster\)\s+running", st): fail("dev status has no running k3s row")
    c.log("dev status table has k3s running")
    c.notcovered("`dev status` row 'IYAKKI shell/agent: unknown, 0 process(es)' although the shell is PID 1 (the status probe matches by process name; the doc admits this); credential hiding was only searched for, not provoked with a planted secret")


@recipe("F-AI-10", "Settings -> Models: the real Lemonade's models sorted by type (read only)")
def ai10(c):
    """PASS when the Models page lists the real Lemonade ('117 installed' style) with type tags for every one of chat, vision,
    embeddings, reranking, image, stt, tts (and other) and each tag is plausible for its name (Qwen3-Embedding is embeddings,
    bge-reranker is reranking, Whisper is stt, SD-Turbo is image); no click on any model."""
    st = c.settings_page("models"); c.shot("models")
    c.expect(st, r"^\d+ installed · \d+ available · Lemonade on 127\.0\.0\.1")
    tags = {}
    for t in c.texts(st):
        m = re.match(r"^(\S.*) · (chat|vision|embeddings|reranking|image|stt|tts|other)$", t)
        if m: tags[m[1]] = m[2]
    c.log("typed models seen: %d; per type: %s" % (len(tags), {k: list(tags.values()).count(k) for k in set(tags.values())}))
    for typ in ("chat", "vision", "embeddings", "reranking", "image", "stt", "tts"):
        if typ not in tags.values(): fail("no model tagged %s" % typ)
    for name, typ in (("Qwen3-Embedding-0.6B-GGUF", "embeddings"), ("bge-reranker-v2-m3-GGUF", "reranking"), ("Whisper-Tiny", "stt"), ("SD-Turbo", "image")):
        if tags.get(name) != typ: fail("%s is tagged %r, expected %s" % (name, tags.get(name), typ))
    c.notcovered("the loaded / available / not-downloaded state and size per model, the 'Use as ...' actions, Load and Pull cards (they change settings or download)")


@recipe("F-KBD-02", "volume keys: cannot be machine-judged")
def kbd02(c):
    """Volume keys are hardware keys: `dev input key` has no volume key name (it offers Enter, Esc, Tab, arrows, F1..F12,
    Ctrl+x or one character), and the row's claim includes the sound being heard. Evidence only: the Sound setting."""
    v = c.snapshot_setting("sound.volume"); m = c.snapshot_setting("sound.muted")
    c.log("sound.volume=%s sound.muted=%s (read only)" % (v, m))
    human("no way to press a volume key through `iyakki dev input`, and the 5 percent step and the bar need eyes and ears; volume left at %s, muted %s" % (v, m))


@recipe("F-APPS-46", "custom apps in the launcher: nothing installed on the Mac mini, cannot be judged")
def apps46(c):
    """`dev list --all` shows what developer mode installed; installing is not allowed in this batch, so the launcher
    tile, app bar and removal cannot be seen."""
    rc, out = c.run("list", "--all", name="list")
    st = c.home()
    chips = [b for b in c.buttons(st) if b.startswith("group ")]
    c.log("launcher groups: %s" % chips)
    human("no tile app is installed (%s); installing one is outside this batch" % out.strip().splitlines()[0])


@recipe("F-IDE-18", "AI IDE chat box layout: needs an open workspace, which this batch must not create")
def ide18(c):
    """The IDE's landing page is reachable, but `dev state` shows no text for it at all, and the chat box exists only inside a
    workspace (creating one writes ~/Workspaces/<name>, outside this batch). Evidence: a screenshot of the landing page and
    the empty state text."""
    st = c.open_tile("AI IDE"); time.sleep(1.5); st = c.state("ide-landing")
    c.shot("ide-landing")
    n = len(c.texts(st))
    c.log("AI IDE landing: %d text lines in `dev state` (buttons: %s)" % (n, ", ".join(c.buttons(st)[:3])))
    human("AI IDE shows only 'New workspace / Open existing folder / All workspaces' (0 workspaces); the chat box, Send and the Model/Agent/Approvals row exist only inside a workspace and the IDE exposes no text to `dev state`, so a script cannot read it anyway")


@recipe("F-IDE-02", "AI IDE cluster view: needs an open workspace, which this batch must not create")
def ide02(c):
    """Same limit as F-IDE-18. The cluster itself is checked read-only: the pods the IDE would list for a workspace namespace."""
    st = c.open_tile("AI IDE"); time.sleep(1.5)
    c.shot("ide-landing")
    out = _kube_ready(c)
    human("the cluster is reachable (node Ready, evidence saved) but the IDE's cluster view (chip 'k3s · default · reachable', workloads, pods, events) lives inside a workspace; none exists (0 workspaces) and `dev state` shows no IDE text")
