#!/usr/bin/env python3
"""The first scenarios, plus the reader of the small text scenario files (scenarios/*.scn).

A scenario is `@scenario(...) def name(t)`; `t` is a devui.Test. Rules (docs/development/DEVICE-UI-TESTS.md):
 - never press an approval, the power menu, sign out, lock or type a password (the guard in devui refuses them);
 - a scenario that changes a setting reads the original first and registers t.on_restore(...) BEFORE changing it;
 - raise devui.Skip("not in this build") for a feature that is missing, never fail for it.
"""
import os
import re
import time

import devui
from devui import Skip, scenario

HERE = os.path.dirname(os.path.abspath(__file__))
THEMES = ["Dark", "Light", "High contrast"]
SIZES = ["Small", "Normal", "Large", "Larger", "Huge"]


# ---- helpers ---------------------------------------------------------------------------------

def open_settings(t):
    t.dev.key("s")  # on Home `s` is the Settings shortcut (allowed by the guard)
    return t.dev.wait_for(app="Settings", timeout=15)


def open_page(t, name):
    st = t.dev.state()
    if st.app.lower() != "settings":
        st = open_settings(t)
    if st.page.lower() != name.lower():
        b = st.button(name)
        if not b or b.x > 320:
            raise Skip("Settings has no page %r in this build" % name)
        t.dev.click_sidebar(name)
    return t.dev.wait_for(app="Settings", page=name, timeout=12)


def checked_choice(st, options):
    """Which option shows the tick (`✓ Dark`) in the state's text."""
    for _, _, text in st.lines:
        m = re.match(r"^\s*[✓✔]\s*(.+?)\s*$", text)
        if m:
            label = re.sub(r"\s+\d+%$", "", m.group(1))  # "Normal 100%" -> "Normal"
            if label in options:
                return label
    return None


def choose(t, label, options):
    t.dev.click(label)
    end = time.time() + 8
    while time.time() < end:
        if checked_choice(t.dev.state(), options) == label:
            return True
        time.sleep(0.6)
    return False


# ---- scenarios -------------------------------------------------------------------------------

@scenario("home-launcher", doc="Home renders: not blank, group tabs and tiles present, tile labels readable by OCR, header and footer there.")
def home_launcher(t):
    st = t.dev.state()
    s = t.shot("home", st)
    t.assert_not_blank(s)
    groups = [b.label for b in st.buttons if b.label.startswith("group ")]
    tiles = [b for b in st.buttons if b.w >= 150 and b.h >= 150 and not b.label.startswith("group ")]
    t.check("group tabs present", len(groups) >= 1, "%d groups: %s" % (len(groups), ", ".join(groups[:6])))
    t.check("app tiles present", len(tiles) >= 1, "%d tiles: %s" % (len(tiles), ", ".join(b.label for b in tiles[:8])))
    t.check("header shows the clock and the account", st.line(r"^\d{1,2}:\d\d$") is not None and st.line(r"^admin|^\w+$") is not None, "")
    t.check("footer hint line present", st.line(r"choose|Enter open") is not None, "")
    t.assert_no_overflow(st)
    if devui.HAVE_TESSERACT:
        ocr = s.ocr_text().lower()
        found = [b.label for b in tiles if b.label.lower() in ocr]
        t.check("tile labels readable by OCR (at least 80%)", not tiles or len(found) >= 0.8 * len(tiles), "read %d of %d: %s" % (len(found), len(tiles), ", ".join(found)))
    else:
        t.skipped_check("tile labels readable by OCR", "no tesseract")
    dots = [b for b in st.buttons if re.search(r"page \d|dot", b.label, re.I)]
    t.note("page dots: %s" % ("%d buttons" % len(dots) if dots else "none shown (one page)"))
    t.assert_text_contrast(s, "IYAKKI", 4.5, "header title contrast >= 4.5")


@scenario("settings-pages", doc="Open Settings and walk eight pages through the sidebar and the search box; each page's title is on screen.")
def settings_pages(t):
    st = open_settings(t)
    side = [b.label for b in st.buttons if b.x < 320 and b.w >= 200 and b.label != "field"]
    wanted = ["Overview", "Account", "Models", "Network", "Firewall", "Display", "Sound", "Keyboard", "Voice", "Browser", "Vault", "Displays"]
    pages = [p for p in wanted if p in side]
    pages += [p for p in side if p not in pages]
    pages = pages[:8]
    if len(pages) < 8:
        t.note("only %d pages in the sidebar" % len(pages))
    t.check("at least 8 sidebar pages", len(side) >= 8, "%d pages" % len(side))
    for p in pages:
        st = open_page(t, p)
        s = t.shot("page-" + p, st)
        t.check("page state says %r" % p, st.page.lower() == p.lower(), "page=%s" % st.page)
        t.assert_text(s, r"\b%s\b" % re.escape(p), "title %r read on the screenshot" % p)
        t.assert_not_blank(s, "page %r not blank" % p)
        t.assert_no_overflow(st, "page %r: nothing cut off" % p)
    # the search box: `/`, then the name, Down, Enter (text is typed only while the shell reports a focused field)
    target = next((p for p in ("Firewall", "Sound", "Keyboard") if p in side), None)
    if target:
        t.dev.key("/")
        try:
            st = t.dev.wait_for(timeout=4, text=r"Search")
        except devui.DevError:
            st = t.dev.state()
        if st.focused:
            t.dev.text(target[:5])
            t.dev.key("down")
            t.dev.key("enter")
            time.sleep(1.2)
            st = t.dev.state()
            t.shot("search-" + target, st)
            t.check("search opened %r" % target, st.page.lower() == target.lower(), "page=%s focused=%r" % (st.page, st.focused))
        else:
            t.skipped_check("search box", "the `/` key did not focus a field in this build")


@scenario("display-theme", restore="the theme read at the start is chosen again (finally)", doc="Settings, Display: switch Dark, Light, High contrast through the buttons; title and body contrast; restore.")
def display_theme(t):
    st = open_page(t, "Display")
    original = checked_choice(st, THEMES)
    if not original:
        raise Skip("no theme choice on the Display page in this build")
    t.note("original theme: %s" % original)
    t.on_restore(lambda: choose(t, original, THEMES) or (_ for _ in ()).throw(RuntimeError("could not set theme %s back" % original)), "theme " + original)
    minimum = {"Dark": 4.5, "Light": 4.5, "High contrast": 7.0}
    for th in THEMES:
        ok = choose(t, th, THEMES)
        t.check("theme %s took effect" % th, ok, "tick moved" if ok else "tick did not move")
        time.sleep(1.0)
        st = t.dev.state()
        s = t.shot("theme-" + th.replace(" ", "-").lower(), st)
        t.assert_not_blank(s, "theme %s not blank" % th)
        t.assert_line_contrast(s, r"^Display$", minimum[th], "theme %s: page title contrast >= %.1f" % (th, minimum[th]))
        t.assert_line_contrast(s, r"The screen in use", minimum[th], "theme %s: body line contrast >= %.1f" % (th, minimum[th]))
        t.assert_no_overflow(st, "theme %s: nothing cut off" % th)


@scenario("wallpapers", restore="`w` pressed until the first wallpaper is back, checked by picture (finally)", doc="Cycle the wallpapers with `w` (Settings, Display page); name, title and Home header/footer contrast for each; restore the first.")
def wallpapers(t):
    st = open_page(t, "Display")
    base = t.shot("wallpaper-original", st)
    names = []

    def back():
        # the cycle is short (five): press `w` until the picture matches the first one again
        t.dev.go_home()
        open_page(t, "Display")
        for _ in range(8):
            now = t.dev.screenshot("restore-check")
            d = t.differs(base, now)
            if d is None or d < 0.5:
                return
            t.dev.key("w")
            time.sleep(1.2)
        raise RuntimeError("the first wallpaper did not come back after 8 presses")
    t.on_restore(back, "first wallpaper")
    for i in range(1, 6):
        t.dev.key("w")
        time.sleep(1.3)
        st = t.dev.state()
        m = st.line(r"^Wallpaper: ")
        names.append(re.sub(r"^Wallpaper:\s*", "", m[2]).rstrip(".") if m else "?")
        s = t.shot("wallpaper-%d-settings" % i, st)
        t.assert_line_contrast(s, r"^Display$", 4.5, "wallpaper %d (%s): Settings title contrast >= 4.5" % (i, names[-1]))
        t.assert_line_contrast(s, r"The screen in use", 4.5, "wallpaper %d (%s): Settings subtitle contrast >= 4.5" % (i, names[-1]))
        t.dev.click("home")
        hs = t.dev.wait_for(app="Home", timeout=10)
        h = t.shot("wallpaper-%d-home" % i, hs)
        t.assert_text_contrast(h, "IYAKKI", 4.5, "wallpaper %d: Home header title contrast >= 4.5" % i)
        foot = hs.line(r"choose|Enter open")
        if foot:
            t.assert_line_contrast(h, re.escape(foot[2][:12]), 3.0, "wallpaper %d: Home footer hint contrast >= 3.0" % i, min_x=0)
        t.dev.key("s")
        t.dev.wait_for(app="Settings", timeout=10)
        open_page(t, "Display")
    t.note("wallpapers seen: %s" % ", ".join(names))
    t.check("five presses show different wallpapers", len(set(names)) >= 4 and "?" not in names, ", ".join(names))
    now = t.dev.screenshot("after-cycle")
    d = t.differs(base, now)
    if d is not None:
        t.check("a full cycle of `w` comes round to the first wallpaper", d < 0.5, "difference %.2f" % d)


@scenario("appearance", doc="Settings, Appearance (when the build has the page): shot, nothing cut off.")
def appearance(t):
    st = open_settings(t)
    if not st.button("Appearance"):
        raise Skip("no Appearance page in this build")
    st = open_page(t, "Appearance")
    s = t.shot("appearance", st)
    t.assert_not_blank(s)
    t.assert_no_overflow(st)
    t.assert_text(s, r"Appearance")


@scenario("model-picker", doc="Chat, Ctrl+P: the model list, scrolled with the keys; shot. Never presses Enter in the picker.")
def model_picker(t):
    st = t.dev.state()
    chat = st.button("CHAT") or st.button("Chat")
    if not chat:
        for g in [b for b in st.buttons if b.label.startswith("group ")]:
            if "ask" in g.label.lower():
                t.dev.click(g.label)
                time.sleep(1.0)
                st = t.dev.state()
                chat = st.button("CHAT") or st.button("Chat")
                break
    if not chat:
        raise Skip("no CHAT tile on Home in this build")
    t.dev.click(chat.label)
    st = t.dev.wait_for(timeout=15)
    if st.app.lower() in ("home", "settings"):
        raise Skip("the chat did not open (app %r)" % st.app)
    t.note("chat app name: %s" % st.app)
    t.shot("chat", st)
    t.dev.key("ctrl+p")
    time.sleep(1.5)
    st2 = t.dev.state()
    s = t.shot("picker", st2)
    t.assert_not_blank(s)
    names = [b.label for b in st2.buttons]
    t.note("buttons after Ctrl+P: %d" % len(names))
    rows = len([b for b in names if b])
    t.note("`dev input` has no mouse wheel (tap, swipe, key, text only): the list is scrolled with the Down key")
    if rows > 40:
        t.skipped_check("scrollbar region for a long list", "no scrollbar detector yet (%d buttons); look at the shot" % rows)
    else:
        t.skipped_check("scrollbar region for a long list", "the list fits on the screen in this build (%d buttons), no scrollbar expected" % rows)
    for i in range(6):
        t.dev.key("down")
    s2 = t.shot("picker-scrolled", t.dev.state())
    if devui.HAVE_PIL:
        d = t.differs(s, s2)
        t.check("the list reacts to the Down key (picture changes)", d is not None and d > 0.05, "difference %.2f" % (d or 0))
    # leave without choosing: Esc closes the picker (the guard refuses Esc once Home is in front)
    for _ in range(3):
        st = t.dev.state()
        if st.app.lower() == "home":
            break
        t.dev.key("esc")
        time.sleep(0.8)


def size_now(t):
    """The UI size: `dev config get display.size` ("display.size = Normal"), the same value the Display page's buttons set."""
    out = t.dev.config_get("display.size")
    m = re.search(r"=\s*(\w+)", out)
    return m.group(1) if m else out.strip()


def choose_size(t, label):
    st = t.dev.state()
    b = next((b for b in st.buttons if re.match(r"^%s\s+\d+%%$" % re.escape(label), b.label)), None)
    if not b:
        raise RuntimeError("no size button %r" % label)
    t.dev.click(b.label)
    end = time.time() + 8
    while time.time() < end:
        if size_now(t) == label:
            return True
        time.sleep(0.6)
    return False


@scenario("text-size", restore="display.size read at the start is chosen again through the button, else `config set` (finally)", doc="Settings, Display: change the UI size, shot, restore.")
def text_size(t):
    st = open_page(t, "Display")
    try:
        original = size_now(t)
    except devui.DevError:
        raise Skip("display.size cannot be read (scope config missing)")
    if original not in SIZES or not any(re.match(r"^Large\s+\d+%$", b.label) for b in st.buttons):
        raise Skip("no text-size buttons on the Display page in this build")
    t.note("original size: %s" % original)

    def put_back():
        open_page(t, "Display")
        if not choose_size(t, original):
            t.dev.config_set("display.size", original)
            if size_now(t) != original:
                raise RuntimeError("display.size is not %s again" % original)
    t.on_restore(put_back, "size " + original)
    base = t.shot("size-%s-original" % original, st)
    other = "Large" if original != "Large" else "Normal"
    ok = choose_size(t, other)
    t.check("size %s took effect (display.size)" % other, ok, "display.size=%s" % size_now(t))
    time.sleep(1.5)
    st2 = t.dev.state()
    s = t.shot("size-" + other, st2)
    t.assert_not_blank(s)
    t.assert_no_overflow(st2, "size %s: nothing cut off" % other)
    if devui.HAVE_PIL:
        d = t.differs(base, s)
        t.check("the screen changed with the size", d is not None and d > 0.5, "difference %.2f" % (d or 0))
    t.assert_line_contrast(s, r"^Display$", 4.5, "size %s: page title contrast >= 4.5" % other)


# ---- the small text format -------------------------------------------------------------------
# One step per line; `#` starts a comment.
#   name: overview-smoke              (header lines first)
#   devices: real qemu
#   restore: none
#   key s | click Display | click-sidebar Firewall | text abc | wait app=Settings [page=Display] [text=REGEX] | shot LABEL
#   expect-text REGEX | expect-not-blank | expect-page NAME | skip-unless-button LABEL | restore-click LABEL
# `restore-click` registers a click that runs at the end even when a check failed.

def load_text_scenarios(folder=os.path.join(HERE, "scenarios")):
    if not os.path.isdir(folder):
        return
    for fn in sorted(os.listdir(folder)):
        if fn.endswith(".scn"):
            _register_text(os.path.join(folder, fn))


def parse_scn(text):
    head, steps = {}, []
    for raw in text.splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        m = re.match(r"^(name|devices|restore|doc):\s*(.*)$", line)
        if m and not steps:
            head[m.group(1)] = m.group(2).strip()
        else:
            steps.append(line)
    return head, steps


def _register_text(path):
    head, steps = parse_scn(open(path).read())
    name = head.get("name") or os.path.splitext(os.path.basename(path))[0]

    def fn(t, steps=steps):
        last = [None]
        for line in steps:
            verb, _, arg = line.partition(" ")
            arg = arg.strip()
            if verb == "key":
                t.dev.key(arg)
            elif verb == "text":
                t.dev.text(arg)
            elif verb == "click":
                t.dev.click(arg)
            elif verb == "click-sidebar":
                t.dev.click_sidebar(arg)
            elif verb == "wait":
                kv = {k: v.strip('"') for k, v in re.findall(r'(\w+)=("[^"]*"|\S+)', arg)}
                t.dev.wait_for(app=kv.get("app"), page=kv.get("page"), text=kv.get("text"))
            elif verb == "shot":
                last[0] = t.shot(arg or "shot", t.dev.state())
            elif verb == "expect-text":
                t.assert_text(last[0] or t.shot("expect", t.dev.state()), arg)
            elif verb == "expect-not-blank":
                t.assert_not_blank(last[0] or t.shot("blank-check", t.dev.state()))
            elif verb == "expect-page":
                st = t.dev.state()
                t.check("page is %s" % arg, st.page.lower() == arg.lower(), "page=%s" % st.page)
            elif verb == "skip-unless-button":
                if not t.dev.state().button(arg):
                    raise Skip("no button %r in this build" % arg)
            elif verb == "restore-click":
                t.on_restore(lambda a=arg: t.dev.click(a), "click " + arg)
            else:
                raise devui.CheckFailed("unknown step %r in %s" % (line, os.path.basename(path)))
    devui.SCENARIOS[name] = {"name": name, "fn": fn, "devices": tuple((head.get("devices") or "real qemu").split()), "needs_signin": True,
                             "start_app": "Home", "restore": head.get("restore", "none (read-only)"), "doc": head.get("doc", "text scenario " + os.path.basename(path))}


load_text_scenarios()
