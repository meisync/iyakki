"""Screen-driving helpers for the session checks: the guest's own shell is driven through developer mode (`iyakki dev input`, `dev state`,
`dev screenshot`) as a person would with a keyboard. Never types a password, never presses a power key by accident (the Home screen's one-key
shortcuts are avoided: Esc, q, o, p and r on Home sign out or open the power menu). The helpers take the Session `s`."""
import re, time, os

GROUP_OF = {}


class UiFail(AssertionError):
    pass


def need(cond, msg):
    if not cond:
        raise UiFail(msg)


def raw(s, *args):
    rc, out = s.dev("input", *args)
    if rc:
        raise UiFail("dev input %s: %s" % (" ".join(args), out.strip()[-200:]))
    return out


def state(s):
    return s.ui().state()


def text(s):
    return state(s).text()


def click(s, label, wait=0.8):
    raw(s, "click", label)
    time.sleep(wait)


def has_button(s, label):
    return any(b.label == label for b in state(s).buttons)


def buttons(s):
    return [b.label for b in state(s).buttons]


def key(s, name, wait=0.6):
    raw(s, "key", name)
    time.sleep(wait)


def typ(s, txt, wait=0.5):
    raw(s, "text", txt)
    time.sleep(wait)


def clear_tz_card(s):
    if has_button(s, "Not now"):
        click(s, "Not now")


def home(s):
    """Back to Home through the top bar's home button; never Esc on Home."""
    for _ in range(5):
        st = state(s)
        if st.app.lower() == "home":
            clear_tz_card(s)
            return st
        if st.button("home"):
            click(s, "home", 1.0)
        else:
            key(s, "Esc", 1.0)
    return state(s)


def open_tile(s, group, tile):
    home(s)
    click(s, "group " + group)
    click(s, tile, 1.5)
    return state(s)


def open_settings(s, page):
    """Settings and one page by its search box: `s`, `/`, the page name, Down, Enter."""
    st = home(s)
    if state(s).app != "Settings":
        key(s, "s", 1.5)
    key(s, "/", 0.5)
    key(s, "Ctrl+a", 0.2)
    typ(s, page, 0.6)
    key(s, "Down", 0.4)
    key(s, "Enter", 1.6)
    st = state(s)
    need(st.app == "Settings", "Settings did not open (saw %s)" % st.app)
    return st


def wait_text(s, pattern, secs=10, interval=0.8):
    end = time.time() + secs
    t = ""
    while time.time() < end:
        t = text(s)
        if re.search(pattern, t, re.S):
            return t
        time.sleep(interval)
    raise UiFail("text %r did not appear (saw: %s)" % (pattern, t[:300].replace("\n", " | ")))


def shot(s, name):
    path = os.path.join(s.log_dir, name + ".png")
    rc, out = s.dev("screenshot", "-o", path)
    need(rc == 0, "screenshot failed: %s" % out[-120:])
    return path


def mean_luma(png):
    from PIL import Image, ImageStat
    im = Image.open(png).convert("L")
    return ImageStat.Stat(im).mean[0]


def tap_text(s, txt, wait=1.0, nth=0, contains=False):
    """Tap a line of the screen's text by its place (for things the shell draws without a button name). Returns True when it was found."""
    st = state(s)
    hits = [(x, y, t) for x, y, t in st.lines if (txt in t if contains else t.strip() == txt)]
    if len(hits) <= nth:
        return False
    x, y, t = hits[nth]
    raw(s, "tap", str(x + 8), str(y + 8))
    time.sleep(wait)
    return True


def tap_nth(s, label, n=0, wait=0.8):
    """Tap the n-th button (top to bottom, left to right) called `label` (for pages that use one name twice)."""
    bs = sorted([b for b in state(s).buttons if b.label == label], key=lambda b: (b.y, b.x))
    if len(bs) <= n:
        raise UiFail("no %d-th button %r" % (n + 1, label))
    b = bs[n]
    raw(s, "tap", str(b.x + b.w // 2), str(b.y + b.h // 2))
    time.sleep(wait)


def grant_scope(s, scope, want=True):
    """On Settings, Developer mode: step the scope list with Next scope and switch `scope` on or off for the paired computer."""
    open_settings(s, "Developer mode")
    for _ in range(14):
        t = text(s)
        m = re.search(r"▶ \[( |x)\] (\S+)", t)
        if m and m.group(2) == scope:
            on = m.group(1) == "x"
            if on != want:
                click(s, "Scope on/off", 1.5)
            t = text(s)
            m = re.search(r"\[( |x)\] %s\b" % re.escape(scope), t)
            return bool(m and (m.group(1) == "x") == want)
        click(s, "Next scope", 0.8)
    return False
