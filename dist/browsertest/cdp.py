# SPDX-License-Identifier: Apache-2.0
# Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).
"""In-house Chrome DevTools protocol client for the real-browser tests. Standard library only (the WebSocket is ws.py).

Starts headless Chrome with a throw-away profile folder, on a random debugging port bound to 127.0.0.1 only, and ALWAYS kills it
(normal end, exception, time limit, SIGINT/SIGTERM, interpreter exit). Offers: open a page, evaluate JavaScript, wait for a condition,
screenshot, console and page errors, failed requests, viewport and colour-scheme emulation, keyboard and mouse input.
"""
import atexit
import base64
import json
import os
import shutil
import signal
import subprocess
import tempfile
import threading
import time

import ws

# Where a Chrome or Chromium is looked for: IYAKKI_CHROME first, then the usual places on macOS and Linux, then the names on PATH.
CHROME_PATHS = [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/usr/bin/google-chrome", "/usr/bin/google-chrome-stable", "/opt/google/chrome/chrome",
    "/usr/bin/chromium", "/usr/bin/chromium-browser", "/usr/lib/chromium/chromium", "/usr/lib/chromium-browser/chromium-browser",
    "/snap/bin/chromium", "/usr/local/bin/chromium",
]
CHROME_NAMES = ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "chrome"]
MIN_MAJOR = 112  # --headless=new and the DevTools calls used here


_live = []  # every Chrome started by this process, so the exit hooks can kill them


def find_chrome():
    """The path of a Chrome or Chromium, or None. A shell-script wrapper (some distributions install one) is accepted: it is only run, never read."""
    env = os.environ.get("IYAKKI_CHROME", "")
    if env:
        return env if os.path.isfile(env) and os.access(env, os.X_OK) else None  # an explicit choice that does not exist is not replaced by a guess
    for c in CHROME_PATHS:
        if os.path.isfile(c) and os.access(c, os.X_OK):
            return c
    for n in CHROME_NAMES:
        w = shutil.which(n)
        if w:
            return w
    return None


def chrome_major(binary):
    """The major version number of a Chrome binary (`--version`), or 0 when it cannot be told."""
    try:
        out = subprocess.run([binary, "--version"], capture_output=True, text=True, timeout=20, stdin=subprocess.DEVNULL).stdout
    except (OSError, subprocess.SubprocessError):
        return 0
    import re
    m = re.search(r"(\d+)\.\d+\.\d+", out)
    return int(m.group(1)) if m else 0


def _kill_all():
    for c in list(_live):
        c.stop()


atexit.register(_kill_all)


def _on_signal(signum, _frame):
    _kill_all()
    raise SystemExit(128 + signum)


def install_signal_handlers():
    for s in (signal.SIGINT, signal.SIGTERM):
        signal.signal(s, _on_signal)


class CdpError(Exception):
    pass


class Chrome:
    """A headless Chrome. Use as a context manager. `work` is a folder under which the throw-away profile is made."""

    def __init__(self, work, binary=None, hard_limit=900, extra_args=()):
        self.binary = binary or find_chrome()
        if not self.binary:
            raise CdpError("no Chrome found (set IYAKKI_CHROME)")
        os.makedirs(work, exist_ok=True)
        self.profile = tempfile.mkdtemp(prefix="profile-", dir=work)
        self.hard_limit = hard_limit
        self.extra = list(extra_args)
        self.proc = None
        self.port = None
        self.browser_path = None
        self.conn = None
        self.log_path = os.path.join(self.profile, "chrome.log")
        self._timer = None

    # -- lifecycle
    def start(self):
        args = [
            self.binary, "--headless=new", "--no-first-run", "--disable-extensions", "--no-default-browser-check",
            "--user-data-dir=" + self.profile, "--remote-debugging-port=0", "--remote-debugging-address=127.0.0.1",
            "--ignore-certificate-errors", "--disable-background-networking", "--disable-sync", "--mute-audio",
            "--password-store=basic", "--use-mock-keychain", "--disable-gpu", "--disable-dev-shm-usage", "about:blank",
        ] + self.extra
        # a Chrome run as root (a container, a CI job) refuses to start without this; the page under test is our own and local
        if hasattr(os, "geteuid") and os.geteuid() == 0:
            args.insert(1, "--no-sandbox")
        args[1:1] = os.environ.get("IYAKKI_CHROME_FLAGS", "").split()
        log = open(self.log_path, "wb")
        self.proc = subprocess.Popen(args, stdout=log, stderr=log, stdin=subprocess.DEVNULL, start_new_session=True)
        _live.append(self)
        # a time limit that kills Chrome even if the test code hangs
        self._timer = threading.Timer(self.hard_limit, self.stop)
        self._timer.daemon = True
        self._timer.start()
        port_file = os.path.join(self.profile, "DevToolsActivePort")
        t0 = time.time()
        while time.time() - t0 < 30:
            if self.proc.poll() is not None:
                self.stop()
                raise CdpError("Chrome exited early; see " + self.log_path)
            if os.path.isfile(port_file):
                lines = open(port_file).read().split("\n")
                if len(lines) >= 2 and lines[0].isdigit() and lines[1]:
                    self.port, self.browser_path = int(lines[0]), lines[1]
                    break
            time.sleep(0.1)
        else:
            self.stop()
            raise CdpError("Chrome did not open its debugging port")
        self.conn = Connection(ws.WebSocket.connect("127.0.0.1", self.port, self.browser_path))
        return self

    def stop(self):
        if self._timer:
            self._timer.cancel()
        if self.conn:
            try:
                self.conn.close()
            except Exception:
                pass
            self.conn = None
        p, self.proc = self.proc, None
        if p is not None:
            try:
                os.killpg(p.pid, signal.SIGTERM)
            except (ProcessLookupError, PermissionError):
                pass
            try:
                p.wait(5)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(p.pid, signal.SIGKILL)
                except (ProcessLookupError, PermissionError):
                    pass
                try:
                    p.wait(5)
                except subprocess.TimeoutExpired:
                    pass
        if self in _live:
            _live.remove(self)
        shutil.rmtree(self.profile, ignore_errors=True)

    def __enter__(self):
        return self.start()

    def __exit__(self, *_):
        self.stop()

    def version(self):
        return self.conn.call("Browser.getVersion")

    def new_page(self, width=1280, height=800, dpr=1, dark=False):
        tid = self.conn.call("Target.createTarget", {"url": "about:blank"})["targetId"]
        sid = self.conn.call("Target.attachToTarget", {"targetId": tid, "flatten": True})["sessionId"]
        p = Page(self.conn, tid, sid)
        p.setup()
        p.set_viewport(width, height, dpr)
        p.set_color_scheme("dark" if dark else "light")
        return p


class Connection:
    """One DevTools WebSocket: a reader thread routes answers to callers and events to the page that owns the session."""

    def __init__(self, sock):
        self.ws = sock
        self.next_id = 1
        self.lock = threading.Lock()
        self.waiting = {}  # id -> [event, result]
        self.listeners = {}  # sessionId -> callback(method, params)
        self.dead = None
        self.t = threading.Thread(target=self._read, daemon=True)
        self.t.start()

    def _read(self):
        try:
            while True:
                m = self.ws.recv(None)
                if m is None or isinstance(m, bytes):
                    continue
                d = json.loads(m)
                if "id" in d:
                    with self.lock:
                        w = self.waiting.pop(d["id"], None)
                    if w:
                        w[1] = d
                        w[0].set()
                else:
                    cb = self.listeners.get(d.get("sessionId"))
                    if cb:
                        cb(d.get("method"), d.get("params", {}))
        except Exception as e:  # the socket closed: wake every waiter
            self.dead = str(e)
            with self.lock:
                ws_ = list(self.waiting.values())
                self.waiting.clear()
            for w in ws_:
                w[0].set()

    def call(self, method, params=None, session=None, timeout=30):
        if self.dead:
            raise CdpError("connection closed: " + self.dead)
        with self.lock:
            i = self.next_id
            self.next_id += 1
            w = [threading.Event(), None]
            self.waiting[i] = w
        msg = {"id": i, "method": method, "params": params or {}}
        if session:
            msg["sessionId"] = session
        self.ws.send(json.dumps(msg))
        if not w[0].wait(timeout):
            raise CdpError("timeout waiting for " + method)
        d = w[1]
        if d is None:
            raise CdpError("connection closed during " + method)
        if "error" in d:
            raise CdpError("%s: %s" % (method, d["error"]))
        return d.get("result", {})

    def close(self):
        self.ws.close()


class Page:
    def __init__(self, conn, target_id, session):
        self.conn, self.target_id, self.session = conn, target_id, session
        self.console = []  # {"level","text","source"}: console messages, page exceptions, browser log entries
        self.failed = []  # failed requests and HTTP statuses of 400 and up: {"url","why"}
        self.requests = []  # every request URL, in order
        self._lock = threading.Lock()
        self._loaded = threading.Event()
        self._frame_nav = 0
        conn.listeners[session] = self._event

    def call(self, method, params=None, timeout=30):
        return self.conn.call(method, params, self.session, timeout)

    def setup(self):
        for d in ("Page", "Runtime", "Log", "Network"):
            self.call(d + ".enable")

    # -- events
    def _event(self, method, p):
        with self._lock:
            if method == "Runtime.consoleAPICalled":
                text = " ".join(_remote(a) for a in p.get("args", []))
                self.console.append({"level": p.get("type"), "text": text, "source": "console"})
            elif method == "Runtime.exceptionThrown":
                ex = p.get("exceptionDetails", {})
                desc = (ex.get("exception") or {}).get("description") or ex.get("text", "")
                self.console.append({"level": "error", "text": "uncaught: " + desc, "source": "exception"})
            elif method == "Log.entryAdded":
                e = p.get("entry", {})
                self.console.append({"level": e.get("level"), "text": e.get("text", "") + " " + e.get("url", ""), "source": e.get("source", "log")})
            elif method == "Network.requestWillBeSent":
                self.requests.append(p.get("request", {}).get("url", ""))
            elif method == "Network.loadingFailed":
                if not p.get("canceled"):
                    self.failed.append({"url": p.get("requestId"), "why": p.get("errorText", "")})
            elif method == "Network.responseReceived":
                r = p.get("response", {})
                if r.get("status", 0) >= 400:
                    self.failed.append({"url": r.get("url", ""), "why": "HTTP %d" % r["status"]})
            elif method == "Page.loadEventFired":
                self._loaded.set()

    def errors(self, ignore=()):
        """Console errors, page exceptions and failed requests so far (texts), minus any containing a string of `ignore`."""
        with self._lock:
            out = [c["text"] for c in self.console if c["level"] in ("error", "assert") or c["source"] == "exception"]
            out += ["%s %s" % (f["why"], f["url"]) for f in self.failed]
        return [t for t in out if not any(i in t for i in ignore)]

    def clear_errors(self):
        with self._lock:
            self.console.clear()
            self.failed.clear()

    # -- navigation and page state
    def goto(self, url, timeout=30):
        # a change of the fragment alone is no new load (no load event): go through a blank page so that the page really starts again
        try:
            here = self.evaluate("location.href", timeout=5)
        except CdpError:
            here = ""
        if here and here.split("#")[0] == url.split("#")[0] and url != "about:blank":
            self.goto("about:blank", timeout)
        self._loaded.clear()
        r = self.call("Page.navigate", {"url": url}, timeout)
        if r.get("errorText"):
            raise CdpError("navigate %s: %s" % (url, r["errorText"]))
        if not self._loaded.wait(timeout):
            raise CdpError("page did not finish loading: " + url)

    def evaluate(self, expr, timeout=30):
        """Evaluate an expression in the page (promises are awaited) and return its JSON value."""
        r = self.call("Runtime.evaluate", {"expression": expr, "returnByValue": True, "awaitPromise": True}, timeout)
        if "exceptionDetails" in r:
            ex = r["exceptionDetails"]
            raise CdpError("JS error: " + ((ex.get("exception") or {}).get("description") or ex.get("text", "")))
        return r["result"].get("value")

    def wait_for(self, expr, timeout=10, what=None):
        t0 = time.time()
        last = None
        while time.time() - t0 < timeout:
            try:
                v = self.evaluate(expr, timeout=5)
                if v:
                    return v
            except CdpError as e:
                last = e
            time.sleep(0.05)
        raise CdpError("timed out waiting for %s (%s)%s" % (what or expr, "%.0fs" % timeout, ": %s" % last if last else ""))

    def set_viewport(self, w, h, dpr=1, mobile=False):
        self.call("Emulation.setDeviceMetricsOverride", {"width": w, "height": h, "deviceScaleFactor": dpr, "mobile": mobile})

    def set_color_scheme(self, scheme):
        self.call("Emulation.setEmulatedMedia", {"features": [{"name": "prefers-color-scheme", "value": scheme}]})

    def screenshot(self, path, full=False):
        r = self.call("Page.captureScreenshot", {"format": "png", "captureBeyondViewport": bool(full)})
        data = base64.b64decode(r["data"])
        with open(path, "wb") as f:
            f.write(data)
        return data

    # -- input
    def click_at(self, x, y):
        for t in ("mouseMoved", "mousePressed", "mouseReleased"):
            self.call("Input.dispatchMouseEvent", {"type": t, "x": x, "y": y, "button": "left", "buttons": 1 if t == "mousePressed" else 0, "clickCount": 1})

    def click(self, selector):
        box = self.evaluate("(function(){var e=document.querySelector(%s);if(!e)return null;e.scrollIntoView({block:'center'});var r=e.getBoundingClientRect();return [r.x+r.width/2,r.y+r.height/2];})()" % json.dumps(selector))
        if not box:
            raise CdpError("no element " + selector)
        self.click_at(box[0], box[1])

    def type_text(self, text):
        self.call("Input.insertText", {"text": text})

    def press(self, key, code=None, vk=0, modifiers=0, text=None):
        base = {"key": key, "code": code or key, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk, "modifiers": modifiers}
        self.call("Input.dispatchKeyEvent", dict(base, type="keyDown", **({"text": text} if text else {})))
        self.call("Input.dispatchKeyEvent", dict(base, type="keyUp"))

    def tab(self, shift=False):
        self.press("Tab", "Tab", 9, 8 if shift else 0)

    def close(self):
        try:
            self.conn.call("Target.closeTarget", {"targetId": self.target_id}, timeout=5)
        except CdpError:
            pass


def _remote(a):
    if "value" in a:
        return str(a["value"])
    return a.get("description") or a.get("unserializableValue") or a.get("type", "")
