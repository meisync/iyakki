#!/usr/bin/env python3
"""devui: drive a real IYAKKI device (or a QEMU guest) through developer mode, take screenshots and check them.

Standard library only. Pillow and tesseract are used when they are installed; without them the image and OCR
checks are reported as "skipped: no PIL / no tesseract" and never as a pass.

The pure parts (guard rules, state and TSV parsing, contrast maths, report writer) need no device and are tested
by dist/devtest/tests. See docs/development/DEVICE-UI-TESTS.md for the rules.
"""
import base64
import html
import json
import os
import re
import shutil
import subprocess
import time

try:
    from PIL import Image, ImageChops, ImageStat
    HAVE_PIL = True
except ImportError:  # pragma: no cover - depends on the host
    Image = ImageChops = ImageStat = None
    HAVE_PIL = False
HAVE_TESSERACT = shutil.which("tesseract") is not None

DEFAULT_CLIENT = os.environ.get("IYAKKI_CLIENT") or os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "target", "release", "iyakki")


class DevError(Exception):
    """A client call failed (after retries)."""


class Refused(Exception):
    """The safe-guard layer refused a key, a tap or a text. Nothing was sent."""


class Skip(Exception):
    """The scenario cannot run here (feature not in this build, wrong device, wrong screen). Not a failure."""


class CheckFailed(AssertionError):
    pass


# --------------------------------------------------------------------------------------------------
# the screen state (`iyakki dev state --json`)

class State:
    def __init__(self, data):
        self.raw = data
        self.app = data.get("app", "") or ""
        self.page = data.get("page", "") or ""
        self.focused = data.get("focused", "") or ""
        self.focused_secret = bool(data.get("focused_secret"))
        self.cut = bool(data.get("cut"))
        self.text_hidden = bool(data.get("text_hidden"))
        scr = data.get("screen") or [0, 0]
        self.width, self.height = int(scr[0]), int(scr[1])
        self.lines = [(int(l.get("x", 0)), int(l.get("y", 0)), str(l.get("text", ""))) for l in data.get("lines", [])]
        self.buttons = [Button(b) for b in data.get("buttons", [])]

    def text(self):
        return "\n".join(t for _, _, t in self.lines)

    def has_text(self, pattern):
        return re.search(pattern, self.text(), re.I) is not None

    def button(self, label):
        """Exact label first, then a part of a label if only one matches (the same rule as `dev input click`)."""
        exact = [b for b in self.buttons if b.label == label]
        if len(exact) >= 1:
            return exact[0]
        part = [b for b in self.buttons if label.lower() in b.label.lower()]
        return part[0] if len(part) == 1 else None

    def line(self, pattern):
        for x, y, t in self.lines:
            if re.search(pattern, t, re.I):
                return (x, y, t)
        return None


class Button:
    def __init__(self, b):
        self.label = str(b.get("label", ""))
        self.x, self.y, self.w, self.h = int(b.get("x", 0)), int(b.get("y", 0)), int(b.get("w", 0)), int(b.get("h", 0))
        self.secret = bool(b.get("secret"))

    @property
    def centre(self):
        return (self.x + self.w // 2, self.y + self.h // 2)


def parse_state(text):
    return State(json.loads(text))


# --------------------------------------------------------------------------------------------------
# the safe-guard layer: pure functions, tested without a device

SIGNED_OUT_APPS = re.compile(r"sign.?in|log.?in|lock|welcome|first.?start", re.I)
# keys that always sign out, lock, power off or restart, wherever the person is
ALWAYS_REFUSED_KEYS = {"power", "poweroff", "xf86poweroff", "sleep", "suspend", "super+l", "win+l", "meta+l", "ctrl+win+q",
                       "ctrl+super+q", "ctrl+meta+q", "ctrl+alt+delete", "ctrl+alt+del", "ctrl+alt+backspace"}
# on Home single letters are one-key shortcuts (o signs out, p opens the power menu, q and Esc sign out)
HOME_KEYS_ALLOWED = {"left", "right", "up", "down", "tab", "shift+tab", "pgup", "pgdn", "pageup", "pagedown", "home", "end",
                     "s", "w", "enter", "return"}
SIGN_OUT_KEYS = {"esc", "escape", "q"}
DANGEROUS_LABELS = re.compile(r"power|shut.?down|restart|reboot|sign.?out|log.?out|lock|delete|erase|wipe|forget|remove|reset|format", re.I)


def norm_key(key):
    k = key.strip().lower().replace(" ", "")
    k = k.replace("control", "ctrl").replace("super", "super").replace("windows", "win")
    return k


def key_refusal(key, st):
    """Return None when the key may be sent, else the reason it is refused. `st` is the current State or None (unknown)."""
    k = norm_key(key)
    if not k:
        return "empty key name"
    if k in ALWAYS_REFUSED_KEYS or k.endswith("+q") and "ctrl" in k and ("win" in k or "super" in k or "meta" in k):
        return "key %r signs out, locks or powers off" % key
    if st is None:
        return "screen state unknown: no key is sent blind"
    if SIGNED_OUT_APPS.search(st.app) and st.app.lower() != "settings":
        return "the screen shows %r (not signed in): scenarios never sign in" % st.app
    if st.focused_secret:
        return "a secret (password) field has focus"
    on_home = st.app.strip().lower() in ("home", "")
    if on_home and k not in HOME_KEYS_ALLOWED:
        why = "signs the person out" if k in SIGN_OUT_KEYS else "is a one-key shortcut on Home (apps, sign out, power menu)"
        return "key %r on Home %s" % (key, why)
    return None


def text_refusal(text, st):
    """Text is only typed into a focused, non-secret field, never on Home (there it is a string of shortcuts)."""
    if st is None:
        return "screen state unknown: no text is sent blind"
    if st.app.strip().lower() in ("home", ""):
        return "text on Home is a string of one-key shortcuts"
    if SIGNED_OUT_APPS.search(st.app) and st.app.lower() != "settings":
        return "the screen shows %r (not signed in)" % st.app
    if st.focused_secret:
        return "a secret (password) field has focus"
    if not st.focused:
        return "no text field has focus"
    return None


def tap_refusal(label, st):
    if st is not None and SIGNED_OUT_APPS.search(st.app) and st.app.lower() != "settings":
        return "the screen shows %r (not signed in)" % st.app
    if label and DANGEROUS_LABELS.search(label):
        return "target %r looks like a power, sign-out or delete action" % label
    return None


# --------------------------------------------------------------------------------------------------
# OCR (tesseract TSV) and image maths

class Word:
    def __init__(self, text, left, top, width, height, conf, line):
        self.text, self.left, self.top, self.width, self.height, self.conf, self.line = text, left, top, width, height, conf, line

    @property
    def box(self):
        return (self.left, self.top, self.left + self.width, self.top + self.height)


def parse_tsv(tsv):
    """tesseract `tsv` output -> list of Word (level 5 rows with text)."""
    words = []
    for row in tsv.splitlines()[1:]:
        c = row.split("\t")
        if len(c) < 12 or c[0] != "5":
            continue
        t = c[11].strip()
        if not t:
            continue
        try:
            words.append(Word(t, int(c[6]), int(c[7]), int(c[8]), int(c[9]), float(c[10]), (int(c[2]), int(c[3]), int(c[4]))))
        except ValueError:
            continue
    return words


def words_text(words):
    lines, last = [], None
    for w in words:
        if last is None or w.line != last:
            lines.append([])
        lines[-1].append(w.text)
        last = w.line
    return "\n".join(" ".join(l) for l in lines)


def find_phrase(words, phrase, min_conf=0.0):
    """Find a phrase (case-insensitive, words in a row on one line) -> (left, top, right, bottom) or None."""
    want = [p.lower() for p in phrase.split()]
    if not want:
        return None
    norm = lambda s: re.sub(r"^\W+|\W+$", "", s.lower())
    for i in range(len(words) - len(want) + 1):
        seg = words[i:i + len(want)]
        if all(norm(seg[j].text) == norm(want[j]) and seg[j].conf >= min_conf for j in range(len(want))) and len({s.line for s in seg}) == 1:
            return (min(s.left for s in seg), min(s.top for s in seg), max(s.left + s.width for s in seg), max(s.top + s.height for s in seg))
    return None


def scale_point(x, y, image_size, screen_size):
    """OCR pixels (screenshot) -> screen coordinates `dev input tap` takes."""
    iw, ih = image_size
    sw, sh = screen_size
    if not iw or not ih:
        return (x, y)
    return (int(round(x * sw / iw)), int(round(y * sh / ih)))


def rel_luminance(rgb):
    def ch(c):
        c = c / 255.0
        return c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4
    r, g, b = rgb[:3]
    return 0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b)


def contrast_ratio(l1, l2):
    hi, lo = max(l1, l2), min(l1, l2)
    return (hi + 0.05) / (lo + 0.05)


def region_contrast(pixels):
    """Contrast (WCAG ratio) of the text in a region given as a list of RGB tuples.
    Background = the commonest luminance band; text = the luminance furthest from it (1st/99th percentile, so a few
    anti-aliased pixels do not decide). A flat region gives 1.0."""
    if not pixels:
        return 1.0
    lums = sorted(rel_luminance(p) for p in pixels)
    n = len(lums)
    bands = {}
    for l in lums:
        bands.setdefault(int(l * 20), []).append(l)
    bg = sorted(max(bands.values(), key=len))[len(max(bands.values(), key=len)) // 2]
    lo, hi = lums[int(n * 0.01)], lums[min(n - 1, int(n * 0.99))]
    fg = lo if abs(lo - bg) > abs(hi - bg) else hi
    return contrast_ratio(bg, fg)


def blank_stats(pixels):
    """(mean luminance 0..255, standard deviation, distinct colours) of a list of RGB tuples."""
    n = len(pixels)
    if n == 0:
        return (0.0, 0.0, 0)
    lum = [0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2] for p in pixels]
    mean = sum(lum) / n
    var = sum((v - mean) ** 2 for v in lum) / n
    return (mean, var ** 0.5, len(set(pixels)))


def is_blank(stats, lo=4.0, hi=251.0, min_std=3.0, min_colours=12):
    mean, std, colours = stats
    return not (lo <= mean <= hi and std >= min_std and colours >= min_colours)


def image_pixels(img, box=None, limit=60000):
    if box:
        img = img.crop(box)
    px = list(img.convert("RGB").getdata())
    if len(px) > limit:
        step = len(px) // limit + 1
        px = px[::step]
    return px


# --------------------------------------------------------------------------------------------------
# one screenshot

class Shot:
    def __init__(self, path, label, index, state=None):
        self.path, self.label, self.index, self.state = path, label, index, state
        self._words = None

    @property
    def image(self):
        return Image.open(self.path) if HAVE_PIL else None

    def words(self):
        if self._words is None:
            self._words = []
            if HAVE_TESSERACT:
                r = subprocess.run(["tesseract", self.path, "-", "--psm", "11", "tsv"], capture_output=True, text=True, timeout=180)
                self._words = parse_tsv(r.stdout)
        return self._words

    def ocr_text(self):
        return words_text(self.words())

    def find(self, phrase):
        return find_phrase(self.words(), phrase)

    def size(self):
        return self.image.size if HAVE_PIL else (0, 0)


# --------------------------------------------------------------------------------------------------
# the device

class Device:
    """One paired device (or QEMU guest) reached with the iyakki client. Every input goes through the guard."""

    def __init__(self, name, run_dir=None, client=None, home=None, kind="real", timeout=40, retries=2, pause=0.6):
        self.name, self.kind = name, kind
        self.client = client or DEFAULT_CLIENT
        self.home = home
        self.run_dir = run_dir or os.getcwd()
        self.timeout, self.retries, self.pause = timeout, retries, pause
        self.counter = 0
        self.scenario_dir = self.run_dir
        self.log = []

    # -- plumbing
    def _cmd(self, args):
        c = [self.client, "dev"] + list(args) + ["--device", self.name]
        if self.home:
            c += ["--home", self.home]
        return c

    def run(self, args, retries=None, timeout=None):
        """Run `iyakki dev ARGS`; returns stdout. Retries on a timeout or a connection error (idempotent calls only)."""
        retries = self.retries if retries is None else retries
        last = ""
        for attempt in range(retries + 1):
            try:
                r = subprocess.run(self._cmd(args), capture_output=True, text=True, timeout=timeout or self.timeout)
            except subprocess.TimeoutExpired:
                last = "timeout after %ss" % (timeout or self.timeout)
            else:
                self.log.append((args[:2], r.returncode))
                if r.returncode == 0:
                    return r.stdout
                last = (r.stdout + r.stderr).strip()[-300:]
                if re.search(r"refus|not allowed|scope|approval|denied", last, re.I):
                    break  # not transient
            time.sleep(1.0 + attempt)
        raise DevError("iyakki dev %s: %s" % (" ".join(args[:2]), last))

    # -- reading
    def state(self):
        return parse_state(self.run(["state", "--json"]))

    def try_state(self):
        try:
            return self.state()
        except (DevError, ValueError):
            return None

    def info(self):
        return self.run(["info"])

    def config_get(self, key):
        return self.run(["config", "get", key]).strip()

    def config_set(self, key, value):
        return self.run(["config", "set", key, value], retries=0)

    def screenshot(self, label, state=None):
        """Take a screenshot into the scenario folder as NN-label.png and return a Shot."""
        self.counter += 1
        os.makedirs(self.scenario_dir, exist_ok=True)
        safe = re.sub(r"[^A-Za-z0-9._-]+", "-", label).strip("-") or "shot"
        path = os.path.join(self.scenario_dir, "%02d-%s.png" % (self.counter, safe))
        self.run(["screenshot", "-o", path])
        if not os.path.isfile(path) or os.path.getsize(path) < 100:
            raise DevError("screenshot %s was not written" % path)
        return Shot(path, label, self.counter, state)

    # -- input (every call is checked against a fresh state first)
    def _fresh(self):
        st = self.try_state()
        return st

    def key(self, name):
        st = self._fresh()
        why = key_refusal(name, st)
        if why:
            raise Refused(why)
        self.run(["input", "key", name], retries=0)
        time.sleep(self.pause)

    def text(self, s):
        st = self._fresh()
        why = text_refusal(s, st)
        if why:
            raise Refused(why)
        self.run(["input", "text", s], retries=0)
        time.sleep(self.pause)

    def tap(self, x, y, label=""):
        st = self._fresh()
        why = tap_refusal(label, st)
        if why:
            raise Refused(why)
        self.run(["input", "tap", str(int(x)), str(int(y))], retries=0)
        time.sleep(self.pause)

    def click(self, label):
        """Press a button by its name (`dev input click`); the shell refuses covered, ambiguous or secret ones."""
        st = self._fresh()
        why = tap_refusal(label, st)
        if why:
            raise Refused(why)
        self.run(["input", "click", label], retries=0)
        time.sleep(self.pause)

    def click_sidebar(self, label):
        """A Settings page in the sidebar (x < 320). The same words can also be a chip on the Overview page, which makes
        `dev input click` refuse them as ambiguous; the sidebar button is tapped by its place instead."""
        st = self._fresh()
        side = [b for b in (st.buttons if st else []) if b.label == label and b.x < 320 and not b.secret]
        if side:
            return self.tap(*side[0].centre, label=label)
        return self.click(label)

    def tap_text(self, text, shot=None):
        """Tap the centre of a text. First the shell's own state (exact places), then OCR of a screenshot (TSV words)
        scaled to the screen size from `dev state`."""
        st = self._fresh()
        if st is None:
            raise DevError("no screen state")
        b = st.button(text)
        if b and not b.secret:
            return self.tap(*b.centre, label=b.label)
        for x, y, t in st.lines:
            if t.lower() == text.lower() and not DANGEROUS_LABELS.search(t):
                return self.tap(x + 20, y + 12, label=t)
        shot = shot or self.screenshot("find-" + text)
        box = shot.find(text)
        if not box:
            raise DevError("text %r not found on the screen" % text)
        cx, cy = (box[0] + box[2]) // 2, (box[1] + box[3]) // 2
        x, y = scale_point(cx, cy, shot.size(), (st.width, st.height))
        return self.tap(x, y, label=text)

    # -- waiting and asserting
    def wait_for(self, app=None, page=None, text=None, timeout=15, interval=0.8):
        """Wait until the state matches (app/page exactly, case-insensitive; text as a regex over the state's text)."""
        end = time.time() + timeout
        st = None
        while True:
            st = self.try_state()
            if st and (app is None or st.app.lower() == app.lower()) and (page is None or st.page.lower() == page.lower()) \
                    and (text is None or st.has_text(text)):
                return st
            if time.time() >= end:
                raise DevError("timed out waiting for app=%r page=%r text=%r (saw app=%r page=%r)" % (app, page, text, st and st.app, st and st.page))
            time.sleep(interval)

    def go_home(self):
        """Back to Home without ever pressing Esc on Home: the top bar's home button, else Esc only while another app is in front."""
        for _ in range(4):
            st = self.state()
            if st.app.lower() == "home":
                return st
            home = st.button("home")
            if home:
                self.click("home")
            else:
                self.key("esc")  # the guard refuses this on Home; here another app is in front
            time.sleep(0.8)
        return self.state()


# --------------------------------------------------------------------------------------------------
# a scenario run: checks, shots, restore steps

class Test:
    """What a scenario function receives: the device, helpers that record checks, and the restore stack."""

    def __init__(self, dev, name, out_dir):
        self.dev, self.name, self.out_dir = dev, name, out_dir
        self.checks, self.shots, self.notes = [], [], []
        self._restores = []
        dev.scenario_dir = out_dir
        dev.counter = 0

    # shots
    def shot(self, label, state=None):
        s = self.dev.screenshot(label, state)
        self.shots.append(s)
        return s

    def note(self, text):
        self.notes.append(text)

    # checks
    def check(self, name, ok, detail=""):
        self.checks.append({"name": name, "ok": bool(ok), "detail": str(detail)})
        return bool(ok)

    def skipped_check(self, name, why):
        self.checks.append({"name": name, "ok": True, "skipped": True, "detail": "skipped: " + why})

    def assert_text(self, shot, pattern, name=None):
        """The pattern (regex) appears in the OCR text of the shot, or else in the state's text."""
        name = name or "text %r" % pattern
        if not HAVE_TESSERACT:
            st = shot.state
            if st is not None:
                m = re.search(pattern, st.text(), re.I)
                return self.check(name + " (state text, no OCR)", m is not None, ("found %r" % m.group(0)[:60]) if m else "not in the state text")
            self.skipped_check(name, "no tesseract")
            return True
        text = shot.ocr_text()
        return self.check(name, re.search(pattern, text, re.I) is not None, "OCR: " + text[:160].replace("\n", " | "))

    def assert_not_blank(self, shot, name="screenshot is not blank"):
        if not HAVE_PIL:
            self.skipped_check(name, "no PIL")
            return True
        stats = blank_stats(image_pixels(shot.image))
        return self.check(name, not is_blank(stats), "mean luminance %.0f, std %.1f, %d colours" % stats)

    def assert_contrast(self, shot, region, minimum, name=None):
        """region = (left, top, right, bottom) in screenshot pixels."""
        name = name or "contrast of region %s >= %.1f" % (region, minimum)
        if not HAVE_PIL:
            self.skipped_check(name, "no PIL")
            return True
        r = region_contrast(image_pixels(shot.image, region))
        return self.check(name, r >= minimum, "ratio %.2f (minimum %.1f)" % (r, minimum))

    def assert_text_contrast(self, shot, phrase, minimum, name=None, pad=6):
        """Find the phrase (OCR box, else its place in the state) and check the contrast of the box around it."""
        name = name or "contrast of %r >= %.1f" % (phrase, minimum)
        if not HAVE_PIL:
            self.skipped_check(name, "no PIL")
            return True
        box = shot.find(phrase) if HAVE_TESSERACT else None
        if not box and shot.state is not None:
            ln = shot.state.line(re.escape(phrase))
            if ln:
                x, y, t = ln
                iw, ih = shot.size()
                sx = iw / float(shot.state.width or iw)
                box = (int(x * sx), int(y * sx), int((x + 14 * len(phrase)) * sx), int((y + 34) * sx))
        if not box:
            return self.check(name, False, "text %r not found on the screen" % phrase)
        iw, ih = shot.size()
        region = (max(0, box[0] - pad), max(0, box[1] - pad), min(iw, box[2] + pad), min(ih, box[3] + pad))
        r = region_contrast(image_pixels(shot.image, region))
        return self.check(name, r >= minimum, "ratio %.2f (minimum %.1f) at %s" % (r, minimum, region))

    def assert_line_contrast(self, shot, pattern, minimum, name=None, min_x=300, pad=6):
        """Contrast of the state line matching `pattern` (content area: x >= min_x, so the sidebar's twin is skipped).
        The box is estimated from the line's place and length; the background is the commonest tone, so a roomy box is fine."""
        name = name or "contrast of %r >= %.1f" % (pattern, minimum)
        if not HAVE_PIL:
            self.skipped_check(name, "no PIL")
            return True
        st = shot.state
        ln = None
        if st is not None:
            for x, y, t in st.lines:
                if x >= min_x and re.search(pattern, t, re.I):
                    ln = (x, y, t)
                    break
        if not ln:
            return self.check(name, False, "no line matching %r in the state" % pattern)
        iw, ih = shot.size()
        sx = iw / float(st.width or iw)
        x, y, t = ln
        box = (int(x * sx) - pad, int(y * sx) - pad, int(min(st.width, x + min(900, 14 * len(t))) * sx) + pad, int((y + 34) * sx) + pad)
        box = (max(0, box[0]), max(0, box[1]), min(iw, box[2]), min(ih, box[3]))
        r = region_contrast(image_pixels(shot.image, box))
        return self.check(name, r >= minimum, "ratio %.2f (minimum %.1f) for %r at %s" % (r, minimum, t[:30], box))

    def assert_no_overflow(self, st, name="nothing is cut off or outside the screen"):
        # only the sideways direction counts: a page taller than the screen scrolls, its lower lines are not cut off
        bad = [b.label for b in st.buttons if b.x < 0 or b.x + b.w > st.width + 2]
        bad += ["text@%d,%d" % (x, y) for x, y, _ in st.lines if x < 0 or x >= st.width]
        return self.check(name, not st.cut and not bad, "cut=%s outside=%s" % (st.cut, bad[:5]))

    def differs(self, a, b, minimum=0.5):
        """Mean absolute pixel difference between two shots (0..255)."""
        if not HAVE_PIL:
            return None
        d = ImageChops.difference(a.image.convert("RGB"), b.image.convert("RGB"))
        return sum(ImageStat.Stat(d).mean) / 3.0

    # restore
    def on_restore(self, fn, label=""):
        self._restores.append((label, fn))

    def run_restores(self):
        errs = []
        while self._restores:
            label, fn = self._restores.pop()
            try:
                fn()
                self.notes.append("restored %s" % (label or "state"))
            except Exception as e:  # noqa: BLE001 - restore failures are reported, not hidden
                errs.append("restore %s failed: %s" % (label, e))
                self.check("restore " + (label or "state"), False, str(e))
        return errs


# --------------------------------------------------------------------------------------------------
# scenario registry

SCENARIOS = {}


def scenario(name, devices=("real", "qemu"), needs_signin=True, start_app="Home", restore="none (read-only)", doc=""):
    def deco(fn):
        SCENARIOS[name] = {"name": name, "fn": fn, "devices": tuple(devices), "needs_signin": needs_signin,
                           "start_app": start_app, "restore": restore, "doc": doc or (fn.__doc__ or "").strip()}
        return fn
    return deco


def run_scenario(dev, sc, out_root):
    """Run one scenario; returns the result dict. The restore steps run in a finally, whatever the checks said."""
    out_dir = os.path.join(out_root, sc["name"])
    t = Test(dev, sc["name"], out_dir)
    res = {"name": sc["name"], "status": "pass", "reason": "", "started": time.strftime("%Y-%m-%d %H:%M:%S"), "restore": sc["restore"]}
    t0 = time.time()
    try:
        st = dev.state()
        if dev.kind not in sc["devices"]:
            raise Skip("scenario supports %s only" % "/".join(sc["devices"]))
        if st.app.lower() not in ("home", "settings"):
            raise Skip("not run: %r is in front (signed out or someone is using the device)" % st.app)
        if st.app.lower() == "settings":
            st = dev.go_home()
        if st.app.lower() != "home":
            raise Skip("could not reach Home (app %r)" % st.app)
        sc["fn"](t)
    except Skip as e:
        res["status"], res["reason"] = "skipped", str(e)
    except Refused as e:
        res["status"], res["reason"] = "error", "guard refused: %s" % e
    except (DevError, CheckFailed, OSError) as e:
        res["status"], res["reason"] = "error", "%s: %s" % (type(e).__name__, e)
    except Exception as e:  # noqa: BLE001 - a bug in a scenario must not stop the run
        res["status"], res["reason"] = "error", "%s: %s" % (type(e).__name__, e)
    finally:
        errs = t.run_restores()
        try:
            dev.go_home()
        except Exception as e:  # noqa: BLE001
            errs.append("could not return to Home: %s" % e)
    if res["status"] == "pass":
        failed = [c for c in t.checks if not c["ok"]]
        if failed:
            res["status"], res["reason"] = "fail", "; ".join("%s (%s)" % (c["name"], c["detail"][:80]) for c in failed[:4])
        elif errs:
            res["status"], res["reason"] = "fail", "; ".join(errs)
    res["duration"] = round(time.time() - t0, 1)
    res["checks"], res["notes"] = t.checks, t.notes
    res["shots"] = [{"file": os.path.relpath(s.path, out_root), "label": s.label, "index": s.index,
                     "ocr": s.ocr_text()[:1500] if (HAVE_TESSERACT and res["status"] != "skipped") else ""} for s in t.shots]
    return res


# --------------------------------------------------------------------------------------------------
# the report

def _thumb_data_uri(path, width=480):
    try:
        if HAVE_PIL:
            im = Image.open(path).convert("RGB")
            im.thumbnail((width, width * 10))
            import io
            buf = io.BytesIO()
            im.save(buf, "JPEG", quality=70)
            return "data:image/jpeg;base64," + base64.b64encode(buf.getvalue()).decode()
        return "data:image/png;base64," + base64.b64encode(open(path, "rb").read()).decode()
    except OSError:
        return ""


def write_json(result, path):
    with open(path, "w") as f:
        json.dump(result, f, indent=2, sort_keys=True)


def write_report(result, html_path, root):
    """One self-contained HTML file: summary table, then per scenario its checks and its shots (thumbnail + OCR text)."""
    e = html.escape
    counts = {}
    for s in result["scenarios"]:
        counts[s["status"]] = counts.get(s["status"], 0) + 1
    color = {"pass": "#1a7f37", "fail": "#cf222e", "error": "#cf222e", "skipped": "#9a6700"}
    out = ["<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'>",
           "<title>Device UI test report</title><style>body{font:15px system-ui,sans-serif;margin:16px;max-width:1200px;background:#fff;color:#111}"
           "table{border-collapse:collapse}td,th{border:1px solid #ccc;padding:4px 8px;text-align:left;vertical-align:top}"
           ".b{color:#fff;padding:1px 8px;border-radius:9px;font-size:13px}.shots{display:flex;flex-wrap:wrap;gap:12px}"
           ".shot{width:480px}.shot img{width:100%;border:1px solid #888}pre{white-space:pre-wrap;font-size:12px;background:#f4f4f4;padding:6px;max-height:140px;overflow:auto}"
           "@media(prefers-color-scheme:dark){body{background:#111;color:#eee}td,th{border-color:#555}pre{background:#222}}</style>"]
    out.append("<h1>Device UI test report</h1><p>Device <b>%s</b> (%s), started %s, client %s</p>" % (
        e(result.get("device", "")), e(result.get("kind", "")), e(result.get("started", "")), e(result.get("client_version", ""))))
    out.append("<p>%s</p>" % e(", ".join("%d %s" % (n, k) for k, n in sorted(counts.items()))))
    if result.get("notes"):
        out.append("<p>%s</p>" % e(" ".join(result["notes"])))
    out.append("<table><tr><th>Scenario<th>Result<th>Time<th>Why / failed checks</tr>")
    for s in result["scenarios"]:
        out.append("<tr><td><a href='#%s'>%s</a><td><span class=b style='background:%s'>%s</span><td>%ss<td>%s</tr>" % (
            e(s["name"]), e(s["name"]), color.get(s["status"], "#555"), e(s["status"]), s.get("duration", ""), e(s.get("reason", ""))))
    out.append("</table>")
    for s in result["scenarios"]:
        out.append("<h2 id='%s'>%s <span class=b style='background:%s'>%s</span></h2>" % (e(s["name"]), e(s["name"]), color.get(s["status"], "#555"), e(s["status"])))
        if s.get("reason"):
            out.append("<p>%s</p>" % e(s["reason"]))
        out.append("<p>Restore: %s</p>" % e(s.get("restore", "")))
        if s.get("checks"):
            out.append("<table><tr><th>Check<th>Result<th>Detail</tr>")
            for c in s["checks"]:
                tag = "skipped" if c.get("skipped") else ("pass" if c["ok"] else "FAIL")
                out.append("<tr><td>%s<td>%s<td>%s</tr>" % (e(c["name"]), tag, e(c["detail"])))
            out.append("</table>")
        if s.get("notes"):
            out.append("<p>%s</p>" % e("; ".join(s["notes"])))
        out.append("<div class=shots>")
        for sh in s.get("shots", []):
            uri = _thumb_data_uri(os.path.join(root, sh["file"]))
            out.append("<div class=shot><b>%02d %s</b><br><img src='%s' alt='%s'><pre>%s</pre></div>" % (sh["index"], e(sh["label"]), uri, e(sh["label"]), e(sh.get("ocr", ""))))
        out.append("</div>")
    out.append("</html>")
    with open(html_path, "w") as f:
        f.write("\n".join(out))
