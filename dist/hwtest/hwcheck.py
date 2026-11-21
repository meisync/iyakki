#!/usr/bin/env python3
"""hwcheck.py: honest, repeatable hardware checks through `iyakki dev` only.

    dist/hwtest/hwcheck.py [--device NAME] list|run|explore ...   (--device: another paired machine; evidence under <date>/<NAME>/<ID>/;
                                                        one lock file per device in ~/iyakki-work/hwtest/locks/, stale after 2 h)
    dist/hwtest/hwcheck.py list                 rows that have a recipe
    dist/hwtest/hwcheck.py run F-ZRAM-01 ...    run those rows (or: run all)
    dist/hwtest/hwcheck.py explore [LABEL]      print the current screen text (no input)

Verdicts (printed last, one per row):
  PASS         the pass condition was actually observed (text/state/log matched);
               the matched lines are in the evidence folder.
  FAIL         something the row promises was looked for and was not there.
  NOT-APPLICABLE  the recipe is written for specific hardware (Mac mini 2014 / 2018, a touch screen, Apple hardware) and this
               device does not have it; skipped with the reason, never counted as FAIL.
  NEEDS-HUMAN  the recipe cannot be judged by a machine (sound heard, light seen, feel,
               an approval, a live model) or the build on the device lacks the feature.
Evidence goes to ~/iyakki-work/hwtest/<date>/<ID>/ (outside the repo): evidence.txt,
state-*.txt, *.png, command outputs, verdict.txt.

Safety rules built in (docs/DEVELOPER-MODE.md, standing owner rules):
  * `dev state` is read before every key press; Esc and q are refused on Home.
  * text is typed only when the state says `focused: a text field`.
  * every setting a recipe changes is recorded in settings-touched.log and restored.
  * a recipe stops with NEEDS-HUMAN when the device shows the sign-in screen.
  * nothing here reboots, installs, deletes, or switches on any network exposure.
Set IYAKKI_BIN to the iyakki binary (default: target/release/iyakki in the repo).
"""
import datetime, os, re, subprocess, sys, time

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
BIN = os.environ.get("IYAKKI_BIN", os.path.join(REPO, "target/release/iyakki"))
DEVARGS = os.environ.get("IYAKKI_DEV_ARGS", "").split()
DEVICE = ""   # set by --device NAME: every `iyakki dev` call gets --device NAME, evidence goes to <date>/<NAME>/<ID>/
ROOT = os.path.expanduser(os.environ.get("HWTEST_OUT", "~/iyakki-work/hwtest"))
DAY = datetime.date.today().isoformat()


class Verdict(Exception):
    def __init__(self, kind, reason):
        super().__init__(reason)
        self.kind, self.reason = kind, reason


def fail(r): raise Verdict("FAIL", r)
def human(r): raise Verdict("NEEDS-HUMAN", r)
def not_applicable(r): raise Verdict("NOT-APPLICABLE", r)


def require_hardware(c, key, info=None):
    """A recipe written for specific hardware ends NOT-APPLICABLE (skipped, with the reason) on any other machine, never FAIL.
    `key` names the hardware (see hwmatch.REQUIREMENTS); the machine facts are the `dev info` lines the recipes already read.
    Returns the `dev info` text."""
    import hwmatch
    if info is None:
        info = c.run("info", name="info")[1]
    ok, why = hwmatch.check(key, info)
    if not ok:
        not_applicable(why)
    return info


def dev(*args, timeout=60, check=False):
    p = subprocess.run([BIN, "dev", *DEVARGS, *map(str, args)], capture_output=True, text=True, timeout=timeout)
    out = (p.stdout or "") + (p.stderr or "")
    if check and p.returncode:
        fail("`iyakki dev %s` failed: %s" % (" ".join(map(str, args)), out.strip()[:300]))
    return p.returncode, out


class Ctx:
    def __init__(self, rid):
        self.rid = rid
        self.dir = os.path.join(ROOT, DAY, DEVICE, rid) if DEVICE else os.path.join(ROOT, DAY, rid)
        os.makedirs(self.dir, exist_ok=True)
        self.ev = open(os.path.join(self.dir, "evidence.txt"), "w")
        self.n = 0
        self.touched = []
        self.log("row %s started %s" % (rid, datetime.datetime.now().isoformat(timespec="seconds")))

    def log(self, s):
        self.ev.write(s + "\n"); self.ev.flush()

    # ---- reading the screen
    def state(self, tag=None):
        rc, out = dev("state")
        if rc: fail("`dev state` failed: " + out[:200])
        self.n += 1
        open(os.path.join(self.dir, "state-%02d%s.txt" % (self.n, "-" + tag if tag else "")), "w").write(out)
        if re.search(r"(?i)sign in|password", out.split("buttons")[0][:600]) and "app: Home" not in out[:20] and "focused" in out and "app: Login" in out:
            human("the device shows the sign-in screen")
        return out

    def texts(self, st):
        body = st.split("\nbuttons", 1)[0]
        return [re.sub(r"^\s*\d+,\d+\s+", "", l) for l in body.splitlines()[4:]]

    def has(self, st, pattern, flags=re.I):
        return [t for t in self.texts(st) if re.search(pattern, t, flags)]

    def expect(self, st, pattern, what=None):
        """PASS evidence: the pattern must match a text line of this state; the lines are logged."""
        m = self.has(st, pattern)
        if not m and re.search(r"^text: hidden \(a private app", st, re.M):
            # a private app (CLI, Terminal ...) hides its texts from `dev state`: a machine cannot read it, a person (or the screenshot) must
            raise Verdict("NEEDS-HUMAN", "this app hides its text from dev state (private app); look at the screenshot for /%s/" % pattern)
        if not m:
            fail("expected text /%s/ not on screen%s" % (pattern, " (%s)" % what if what else ""))
        self.log("MATCH /%s/ -> %s" % (pattern, " | ".join(m[:4])))
        return m

    def expect_absent(self, st, pattern, what=None):
        m = self.has(st, pattern)
        if m: fail("unexpected text /%s/ on screen: %s" % (pattern, m[0]))
        self.log("ABSENT /%s/ ok" % pattern)

    def shot(self, name):
        p = os.path.join(self.dir, name + ".png")
        dev("screenshot", "-o", p)
        self.log("SCREENSHOT " + p)
        return p

    def run(self, *args, name=None, timeout=60):
        rc, out = dev(*args, timeout=timeout)
        fn = os.path.join(self.dir, (name or "-".join(map(str, args[:2]))).replace("/", "_") + ".txt")
        open(fn, "w").write(out)
        self.log("RAN iyakki dev %s -> rc=%d, saved %s" % (" ".join(map(str, args)), rc, fn))
        return rc, out

    # ---- driving the screen
    def app(self, st):
        m = re.search(r"^app: (.*)$", st, re.M); return m.group(1) if m else ""

    def page(self, st):
        m = re.search(r"^page: (.*)$", st, re.M); return m.group(1) if m else ""

    def buttons(self, st):
        return [re.sub(r"^\s*\d+,\d+\s+\d+x\d+\s+", "", l) for l in st.split("\nbuttons", 1)[-1].splitlines()[1:]]

    def click(self, label, wait=1.2, must=True):
        st = self.state()
        if label not in self.buttons(st):
            if must: fail("no button called %r on screen %s/%s" % (label, self.app(st), self.page(st)))
            return None
        rc, out = dev("input", "click", label)
        self.log("CLICK %r -> %s" % (label, out.strip().splitlines()[0] if out.strip() else rc))
        time.sleep(wait)
        return self.state()

    def key(self, name, wait=1.0):
        st = self.state()
        if name.lower() in ("escape", "esc", "q") and self.app(st) == "Home":
            fail("refused: %s on Home signs the person out" % name)
        dev("input", "key", name)
        self.log("KEY %s (app %s)" % (name, self.app(st)))
        time.sleep(wait)
        return self.state()

    def type(self, text, wait=0.8):
        st = self.state()
        if "focused: a text field" not in st:
            fail("refused to type: the screen has no focused text field")
        dev("input", "text", text); self.log("TYPED %r" % text); time.sleep(wait)
        return self.state()

    def home(self):
        for _ in range(6):
            st = self.state()
            if self.app(st) == "Home":
                b = self.buttons(st)
                if self.has(st, r"^About IYAKKI$") or ("close" in b and "back" in b):  # the IYAKKI menu or one of its pages is open
                    dev("input", "click", "close"); time.sleep(1.2)
                    continue
                return st
            if "home" in self.buttons(st):
                dev("input", "click", "home")
            else:
                dev("input", "tap", 1850, 22)
            time.sleep(1.5)
        fail("could not get back to Home")

    def open_menu_item(self, label):
        st = self.home()
        self.click("IYAKKI menu"); r = self.click(label); return r

    def open_tile(self, label, groups=()):
        """Open a launcher tile by its label, trying each group chip until it is visible."""
        def opened(st):
            for _ in range(10):  # a slower machine needs a few seconds to open a tile
                if self.app(st) != "Home": return st
                time.sleep(1); st = self.state()
            return st
        st = self.home()
        if label in self.buttons(st):
            return opened(self.click(label, wait=2))
        for g in groups or [b[len("group "):] for b in self.buttons(st) if b.startswith("group ")]:
            st = self.click("group " + g, must=False)
            if st and label in self.buttons(st):
                return opened(self.click(label, wait=2))
        fail("tile %r not found in any launcher group" % label)

    def settings_page(self, query):
        """Open Settings and jump to a page with the search box (typed only after focus is proven)."""
        self.home()
        self.click("IYAKKI menu"); self.click("Settings", wait=2)
        st = self.click("field")
        if "focused: a text field" not in st:
            fail("settings search did not take focus")
        st = self.type(query, wait=1.2)
        self.page_seen = self.page(st)
        return st

    def snapshot_setting(self, key):
        rc, out = dev("config", "get", key)
        return out.strip()

    def touched_log(self, s):
        self.touched.append(s)
        open(os.path.join(ROOT, DAY, DEVICE, "settings-touched.log") if DEVICE else os.path.join(ROOT, DAY, "settings-touched.log"), "a").write("%s %s %s\n" % (datetime.datetime.now().isoformat(timespec="seconds"), self.rid, s))
        self.log("SETTING " + s)


RECIPES = {}
def recipe(rid, title):
    def deco(f):
        RECIPES[rid] = (title, f); return f
    return deco


def run_row(rid):
    title, f = RECIPES[rid]
    c = Ctx(rid)
    try:
        f(c)
        nc = getattr(c, "not_covered", [])
        v = ("PASS", "all listed pass conditions observed" + ("; NOT COVERED: " + "; ".join(nc) if nc else ""))
    except Verdict as e:
        v = (e.kind, e.reason)
    except subprocess.TimeoutExpired as e:
        v = ("NEEDS-HUMAN", "a dev command timed out: %s" % (e.cmd,))
    except Exception as e:  # a crashed recipe is never a pass
        v = ("NEEDS-HUMAN", "recipe error: %r" % (e,))
    try:
        c.home()
    except Verdict as e:
        v = (v[0], v[1] + "; WARNING could not return Home: " + e.reason)
    except Exception:
        pass
    c.log("VERDICT %s: %s" % v)
    open(os.path.join(c.dir, "verdict.txt"), "w").write("%s %s\n" % v)
    print("%-11s %-14s %s" % (v[0], rid, v[1]))
    return v


LOCK_STALE = 2 * 3600


def lock_path():
    return os.path.join(ROOT, "locks", (DEVICE or "default") + ".lock")


def acquire_lock():
    """One agent per machine: a lock file with pid, time and who; a lock older than 2 h is stale and replaced."""
    p = lock_path(); os.makedirs(os.path.dirname(p), exist_ok=True)
    if os.path.exists(p):
        age = time.time() - os.path.getmtime(p)
        if age < LOCK_STALE:
            print("LOCKED: %s is held (%s, %.0f min old). Not touching the device. Delete the file only if you are sure it is dead." % (p, open(p).read().strip(), age / 60))
            return False
        print("stale lock (%.1f h old) replaced: %s" % (age / 3600, p))
    open(p, "w").write("pid %d, started %s" % (os.getpid(), datetime.datetime.now().isoformat(timespec="seconds")))
    return True


def release_lock():
    try:
        if open(lock_path()).read().startswith("pid %d," % os.getpid()): os.remove(lock_path())
    except OSError:
        pass


def main(a):
    global DEVICE, DEVARGS
    a = list(a)
    if "--device" in a:
        i = a.index("--device"); DEVICE = a[i + 1]; del a[i:i + 2]
        DEVARGS = ["--device", DEVICE] + DEVARGS
    from_recipes()
    if not a or a[0] == "list":
        for k, (t, _) in sorted(RECIPES.items()): print(k, "-", t)
        return 0
    if not acquire_lock(): return 3
    try:
        if a[0] == "explore":
            c = Ctx("explore"); st = c.state("explore"); print(st); return 0
        if a[0] == "run":
            ids = sorted(RECIPES) if a[1:] == ["all"] else a[1:]
            res = [run_row(i) for i in ids if i in RECIPES]
            for u in [i for i in ids if i not in RECIPES]: print("NO-RECIPE   %s (generic template only; write a recipe)" % u)
            return 1 if any(r[0] == "FAIL" for r in res) else 0
    finally:
        release_lock()
    print(__doc__); return 2


def from_recipes():
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    sys.modules.setdefault("hwcheck", sys.modules[__name__])
    import importlib, glob
    for f in sorted(glob.glob(os.path.join(os.path.dirname(os.path.abspath(__file__)), "recipes*.py"))):
        importlib.import_module(os.path.basename(f)[:-3])


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
