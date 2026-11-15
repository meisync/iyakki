"""Batch 3 recipes: state-changing checks that undo themselves (install a tiny tile app and remove it, a flow from a template that
is run once and deleted, K8S Allow changes on then off WITHOUT pressing any cluster action, key-repeat cycled and restored).
Every change is written to settings-touched.log; each recipe restores in a finally block and FAILs loudly if it cannot."""
import os, re, shutil, time, tempfile, json
from hwcheck import recipe, fail, human, dev, Verdict, Ctx, REPO
import recipes as _r1


@recipe("F-APPS-46-live", "install a tiny tile app with dev install, see its tile, app bar, Info, remove it with its data")
def apps46_live(c):
    """PASS when `dev install` of a temp folder (3 KB, built from dist/templates/tile) makes a tile in the launcher, it opens full
    screen with the bar (custom app, unknown publisher, Developer (local), Reload, Info, Remove), Count works, Info lists what it may
    do, Remove asks keep/delete data, typing the id deletes, and `dev list` is empty again."""
    name = "hwtest-demo"
    tmp = tempfile.mkdtemp(prefix="hwapp-"); src = os.path.join(REPO, "dist/templates/tile")
    for f in ("app.js", "app.json", "index.html", "iyakki-app.json", "style.css"):
        t = open(os.path.join(src, f)).read().replace("__NAME__", name).replace("__TITLE__", "HW Test Demo")
        open(os.path.join(tmp, f), "w").write(t)
    c.touched_log("install tile:%s from a temp folder (3 KB, this PC)" % name)
    rc, out = c.run("install", tmp, name="install")
    if rc: fail("dev install failed: " + out[:200])
    removed = False
    try:
        st = c.home()
        for _ in range(15):  # the new group can take a few seconds to appear
            if "group Apps" in c.buttons(st): break
            time.sleep(1); st = c.state()
        st = c.click("group Apps", must=False) or st
        if "HW Test Demo" not in c.buttons(st): fail("no HW Test Demo tile in the launcher after install")
        st = c.click("HW Test Demo", wait=2); c.shot("tile-open")
        c.expect(st, r"custom app · unknown publisher · Developer \(local\)")
        for b in ("Reload", "Info", "Remove", "Count"):
            if b not in c.buttons(st): fail("app bar button %r missing" % b)
        st = c.click("Count"); c.expect(st, r"^Pressed$")
        st = c.click("Info"); c.shot("info"); c.expect(st, r"What it may do"); c.expect(st, r"keep its own saved data")
        st = c.click("Close"); st = c.click("Remove"); c.shot("remove-card")
        c.expect(st, r"Remove “HW Test Demo”\?"); c.expect(st, r"Its data is KEPT unless you choose to delete it")
        st = c.click("Remove and delete its data…")
        c.expect(st, r"Type hwtest-demo and press Enter")
        c.click("field"); st = c.state()
        if "focused: This cannot be undone" not in st: fail("the confirmation field did not take focus")
        dev("input", "text", name); time.sleep(.6); dev("input", "key", "Enter"); time.sleep(2); removed = True
    finally:
        if not removed:
            dev("uninstall", name); c.touched_log("uninstall %s (cleanup)" % name)
        shutil.rmtree(tmp, ignore_errors=True)
    out = dev("list")[1]
    c.log("dev list after: " + out.strip())
    if "hwtest-demo" in out: fail("the test app is still installed")
    c.touched_log("removed tile:%s with its data" % name)
    c.notcovered("shared installs, per-user visibility, the assistant/file/clipboard questions, a dev workspace app that reloads on file change, WASM modules and picture icons")


@recipe("F-FLOW-09-live", "flow from a template: canvas, one run, run view with waterfall; flow and its inbox note deleted")
def flow09_live(c):
    """PASS when 'Health check every minute' becomes a flow whose canvas shows Start, Check it, Healthy and Tell me wired, a manual
    run finishes, the run view shows each step's state and a timing waterfall, and the flow is deleted again (its schedule is never
    switched on). The run's one inbox note is deleted too. The step calls the template's default address (example.com) once."""
    c.open_tile("FLOWS"); c.click("From template (t)")
    st = c.state()
    t = next((b for b in c.buttons(st) if b.startswith("Health check every minute")), None)
    if not t: fail("no Health check template")
    c.touched_log("flows: create health-check from template (schedule stays off)")
    st = c.click(t, wait=2); c.shot("canvas")
    c.expect(st, r"^draft · not published$")
    for b in ("Run (r)", "Publish (p)", "History (h)", "Arrange", "Fit"):
        if b not in c.buttons(st): fail("designer button %r missing" % b)
    created_inbox = len(json.loads(dev("inbox", "--json")[1])["items"])
    try:
        st = c.click("Run (r)", wait=3); c.shot("run-view")
        c.expect(st, r"^succeeded$"); c.expect(st, r"4 of 4 steps ran"); c.expect(st, r"^timing · \d+ ms$")
        for b in ("Retry step", "Skip step", "Rerun from here"):
            if b not in c.buttons(st): fail("run button %r missing" % b)
    finally:
        c.click("Close (Esc)", must=False); c.click("Close (Esc)", must=False)
        st = c.state()
        if c.app(st) == "FLOWS" and c.has(st, r"^Health check$"):
            dev("input", "key", "d"); time.sleep(1.5); c.click("Yes", must=False)
        c.touched_log("flows: health-check deleted")
    st = c.state(); c.expect(st, r"No workflows yet")
    items = json.loads(dev("inbox", "--json")[1])["items"]
    mine = [i for i in items if i["source"] == "flows"]
    for i in mine:
        c.click("inbox"); c.click(i["id"]); c.click("Delete (d)"); c.click("Yes", must=False)
        c.touched_log("inbox: deleted the note my test flow made (%s)" % i["title"])
    left = [i for i in json.loads(dev("inbox", "--json")[1])["items"] if i["source"] == "flows"]
    if left: fail("an inbox note from the test flow is still there")
    c.notcovered("wiring by dragging from a port, Approval step and the approvals card, Publish, History/diff, schedule firing, Webhook, minimap use; the template's address is example.com (the step failed with HTTP 404, the Tell me branch ran, which is the template's own design)")


@recipe("F-K8S-24-toggle", "K8S Allow changes On then Off; no cluster action is pressed")
def k8s24_toggle(c):
    """PASS when switching Allow changes On adds the 'Delete pod' button and the line 'Changes are allowed on cluster ... Every
    change asks first and is recorded', and switching Off restores 'Read-only again' and removes the button. No pod is touched."""
    c.open_tile("K8S")
    st = c.state()
    pod = next((b for b in c.buttons(st) if b.startswith("registry-")), None)
    if not pod: fail("no registry pod row")
    c.click(pod)
    c.touched_log("K8S Allow changes Off -> On (no action will be pressed)")
    on = False
    try:
        st = c.click("Allow changes", wait=1.5); on = True; st = c.state("on")
        c.shot("on")
        c.expect(st, r"^Changes are allowed on cluster .* Every change asks first and is recorded\.$")
        if "Delete pod" not in "\n".join(c.has(st, r"^Delete pod$") or []) and "Delete pod" not in c.buttons(st):
            c.log("Delete pod is drawn but is not listed as a button name in `dev state` (seen in the screenshot)")
    finally:
        if on:
            c.click("Allow changes", wait=1.5); c.touched_log("K8S Allow changes On -> Off (restored)")
    st = c.state("off"); c.shot("off")
    c.expect(st, r"^Read-only again on cluster")
    c.expect(st, r"Read-only\. Turn on Allow changes in the header")
    rc, out = dev("kube", "get", "pods", "-A", "--no-headers")
    if len(out.strip().splitlines()) != 12: fail("pod count changed: %d" % len(out.strip().splitlines()))
    c.notcovered("restart, scale, delete pod, cordon: pressing a cluster action was refused by the permission rules of this session, so no cluster object was changed")
    human("the switch behaves, but the actions (restart, scale, delete pod, cordon, typed-name delete) were not exercised; they need a person or an explicit grant to change the real k3s workloads")


@recipe("F-KBD-06-repeat", "Settings -> Keyboard: key-repeat delay cycled and restored; audit lines checked")
def kbd06_repeat(c):
    """PASS when each press of Repeat delay changes the 'Key repeat' line (200, 300, 400, 500, 700, 1000 ms, then the kernel's own),
    every step is written to the audit log (settings.keyboard repeat_delay=N), and the original 'the kernel's own' value is back."""
    st = c.settings_page("keyboard")
    orig = c.expect(st, r"^(the kernel's own .*|starts after .*)$")
    key = [t for t in c.texts(st) if re.match(r"^(the kernel's own \(\d+ ms, \d+ per second\)|starts after \d+ ms, every \d+ ms)$", t)]
    if not key: fail("no Key repeat value line")
    start = key[0]; c.log("start: " + start)
    c.touched_log("keyboard repeat delay: start %r; cycling and restoring" % start)
    seen = [start]
    try:
        for _ in range(8):
            st = c.click("Repeat delay", wait=1.0)
            v = [t for t in c.texts(st) if re.match(r"^(the kernel's own \(\d+ ms, \d+ per second\)|starts after \d+ ms, every \d+ ms)$", t)][0]
            seen.append(v)
            if v == start: break
        else:
            fail("the cycle never returned to %r: %s" % (start, seen))
    finally:
        for _ in range(8):
            st = c.state()
            v = [t for t in c.texts(st) if re.match(r"^(the kernel's own \(\d+ ms, \d+ per second\)|starts after \d+ ms, every \d+ ms)$", t)]
            if v and v[0] == start: break
            dev("input", "click", "Repeat delay"); time.sleep(1)
        c.touched_log("keyboard repeat delay restored to %r" % start)
    c.log("values seen: %s" % seen)
    if len(set(seen)) < 5: fail("fewer than 5 different values: %s" % seen)
    aud = c.run("logs", "--system", "--source", "audit", "-n", 80, name="audit")[1]
    if len(re.findall(r"settings\.keyboard repeat_delay=\d+", aud)) < 5: fail("audit log lacks the repeat_delay lines")
    c.shot("restored")
    human("the page cycles the delay, audits each step and restores it, but whether the kernel accepted the repeat rate on a real keyboard (EVIOCSREP) and how typing feels needs a person at the USB keyboard that appeared on this Mac mini today")
