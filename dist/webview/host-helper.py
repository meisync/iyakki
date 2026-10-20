#!/usr/bin/env python3
"""IYAKKI web view helper for a development desktop (window mode): WebKitGTK rendered into a picture the shell shows.

It speaks docs/WEBVIEW-PROTOCOL.md: the shell starts it as
    host-helper.py --socket-fd 0 --shm PATH
sends messages over the socket, and reads the rendered pixels from the shared file. Needs python3-gi, gir1.2-webkit2-4.1
and GTK 3 (Ubuntu/Debian: python3-gi gir1.2-webkit2-4.1 gir1.2-gtk-3.0). Not part of the IYAKKI image (that uses a
WPE WebKit helper that speaks the same protocol).

Two ways of rendering, chosen by IYAKKI_WEBVIEW_BACKEND (default: xvfb when the Xvfb program is installed, else offscreen):

  xvfb       a private virtual X server (Xvfb, started and stopped by this helper, invisible) holds an ordinary GTK window with
             the WebKitWebView. Everything works, including video (GStreamer needs a real window for its GL video path).
             Each frame is read back with gdk_pixbuf_get_from_window, compared with the last one, and only the changed rows
             are converted and written. Needs: apt install xvfb.
  offscreen  a GtkOffscreenWindow in this process; 'damage-event' says what changed. No extra program, but the engine has no
             GL there: pages, search, typing, animations work, video does not (and some video pages crash the engine).

At most IYAKKI_WEBVIEW_FPS (default 30) frames a second are sent.
"""
import os
import sys
import struct
import time
import json
import socket
import select
import shutil
import subprocess

# set by load_gtk() once the display is chosen
Gtk = Gdk = GLib = WebKit2 = None


def load_gtk():
    global Gtk, Gdk, GLib, WebKit2
    try:
        import gi
        gi.require_version("Gtk", "3.0")
        gi.require_version("Gdk", "3.0")
        gi.require_version("WebKit2", "4.1")
        from gi.repository import Gtk as _Gtk, Gdk as _Gdk, GLib as _GLib, WebKit2 as _WebKit2
    except Exception as e:  # reported to the shell through its log line
        sys.stderr.write("host-helper: WebKitGTK is not available: %s\n(install python3-gi gir1.2-webkit2-4.1 gir1.2-gtk-3.0)\n" % e)
        sys.exit(3)
    Gtk, Gdk, GLib, WebKit2 = _Gtk, _Gdk, _GLib, _WebKit2


PROTOCOL_VERSION = 1

# --- message types (docs/WEBVIEW-PROTOCOL.md) -----------------------------------------------------------------------
T_HELLO, T_NAVIGATE, T_RELOAD, T_STOP, T_BACK, T_FORWARD, T_RESIZE, T_MOUSE, T_WHEEL, T_KEY, T_TEXT, T_ZOOM, T_FOCUS, T_VISIBLE, T_ACK, T_QUIT, T_SCRIPT = range(1, 18)
F_READY, F_FRAME, F_TITLE, F_URL, F_LOADING, F_HISTORY, F_CURSOR, F_DOWNLOAD, F_CRASHED, F_LOG, F_CLIPBOARD, F_SCRIPT_RESULT = range(0x81, 0x8D)

MOD_SHIFT, MOD_CTRL, MOD_ALT, MOD_SUPER = 1, 2, 4, 8
CUR_DEFAULT, CUR_POINTER, CUR_TEXT = 0, 1, 2
MAX_SCREEN = (4096, 2304)


class Reader:
    def __init__(self, buf):
        self.b = buf
        self.o = 0

    def u8(self):
        v = self.b[self.o]
        self.o += 1
        return v

    def u32(self):
        v = struct.unpack_from("<I", self.b, self.o)[0]
        self.o += 4
        return v

    def i32(self):
        v = struct.unpack_from("<i", self.b, self.o)[0]
        self.o += 4
        return v

    def s(self):
        n = self.u32()
        if n > (1 << 20) or self.o + n > len(self.b):
            raise ValueError("bad string")
        v = bytes(self.b[self.o:self.o + n]).decode("utf-8", "replace")
        self.o += n
        return v


def pack_str(s):
    b = s.encode("utf-8")[: 1 << 20]
    return struct.pack("<I", len(b)) + b


class Hello:
    """The first message (read before any GTK code runs, because it decides the virtual screen's size)."""

    def __init__(self, r):
        self.version, self.w, self.h, self.scale, self.max_w, self.max_h = r.u32(), r.u32(), r.u32(), r.u32(), r.u32(), r.u32()
        self.ua, self.profile, self.locale = r.s(), r.s(), r.s()
        n = r.u32()
        self.hosts = [r.s() for _ in range(min(n, 4096))]


class Xvfb:
    """A private invisible X server; killed when the helper ends (or when the helper is killed: PR_SET_PDEATHSIG)."""

    def __init__(self, w, h):
        import ctypes
        rfd, wfd = os.pipe()

        def die_with_parent():
            try:
                ctypes.CDLL("libc.so.6").prctl(1, 15)  # PR_SET_PDEATHSIG, SIGTERM
            except Exception:
                pass

        self.proc = subprocess.Popen([shutil.which("Xvfb"), "-displayfd", str(wfd), "-screen", "0", "%dx%dx24" % (w, h), "-nolisten", "tcp", "-noreset"],
                                     pass_fds=(wfd,), stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, preexec_fn=die_with_parent)
        os.close(wfd)
        ready, _, _ = select.select([rfd], [], [], 10)
        line = os.read(rfd, 32) if ready else b""
        os.close(rfd)
        if not line.strip().isdigit():
            self.proc.kill()
            raise RuntimeError("Xvfb did not start")
        self.display = ":" + line.strip().decode()
        # the number is announced a moment before the server accepts connections: wait until it does
        path = "/tmp/.X11-unix/X" + self.display[1:]
        for _ in range(100):
            c = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            try:
                c.connect(path)
                c.close()
                return
            except OSError:
                c.close()
                time.sleep(0.05)
        self.proc.kill()
        raise RuntimeError("Xvfb does not accept connections")

    def stop(self):
        try:
            self.proc.terminate()
            self.proc.wait(timeout=2)
        except Exception:
            try:
                self.proc.kill()
            except Exception:
                pass


class Helper:
    def __init__(self, sock, shm_fd, hello, backend):
        self.sock = sock
        self.shm_fd = shm_fd
        self.hello = hello
        self.backend = backend
        self.inbuf = bytearray()
        self.w, self.h = max(hello.w, 1), max(hello.h, 1)
        self.zoom = 100
        self.scale = max(hello.scale, 50)
        self.max_w, self.max_h = max(hello.max_w, 1), max(hello.max_h, 1)
        self.slot_bytes = self.max_w * self.max_h * 4
        self.busy = [False, False]
        self.next_slot = 0
        self.dirty = None  # (y0, y1) rows in xvfb mode, (x0, y0, x1, y1) in offscreen mode
        self.visible = True
        self.allow = hello.hosts or ["*"]
        self.win = None
        self.view = None
        self.fps = max(1, int(os.environ.get("IYAKKI_WEBVIEW_FPS", "30")))
        self.last_frame = 0.0
        self.flush_source = 0
        self.committed = ""
        self.crashes = 0
        self.prev = None
        self.maybe = True
        self.cursor = None
        self.idle_since = time.monotonic()

    # --- sending ------------------------------------------------------------------------------------------------
    def send(self, ty, payload=b""):
        data = struct.pack("<IB", len(payload) + 1, ty) + payload
        try:
            self.sock.setblocking(True)
            self.sock.sendall(data)
        except OSError:
            Gtk.main_quit()
        finally:
            try:
                self.sock.setblocking(False)
            except OSError:
                pass

    def log(self, text):
        self.send(F_LOG, pack_str(text))

    # --- setup -----------------------------------------------------------------------------------------------
    def setup(self):
        hello = self.hello
        if hello.profile:
            data = os.path.join(hello.profile, "data")
            cache = os.path.join(hello.profile, "cache")
            os.makedirs(data, mode=0o700, exist_ok=True)
            os.makedirs(cache, mode=0o700, exist_ok=True)
            dm = WebKit2.WebsiteDataManager(base_data_directory=data, base_cache_directory=cache)
            ctx = WebKit2.WebContext.new_with_website_data_manager(dm)
            dm.get_cookie_manager().set_persistent_storage(os.path.join(data, "cookies.sqlite"), WebKit2.CookiePersistentStorage.SQLITE)
            dm.get_cookie_manager().set_accept_policy(WebKit2.CookieAcceptPolicy.NO_THIRD_PARTY)
        else:
            dm = WebKit2.WebsiteDataManager.new_ephemeral()
            ctx = WebKit2.WebContext.new_with_website_data_manager(dm)
        if hello.locale:
            ctx.set_preferred_languages([hello.locale])
        ctx.connect("download-started", self.on_download)
        settings = WebKit2.Settings()
        settings.set_enable_javascript(True)
        settings.set_enable_media(True)
        settings.set_enable_mediasource(True)
        settings.set_enable_webaudio(True)
        settings.set_enable_webgl(True)
        settings.set_enable_media_stream(False)
        settings.set_enable_encrypted_media(True)
        settings.set_enable_smooth_scrolling(True)
        settings.set_media_playback_requires_user_gesture(False)
        settings.set_javascript_can_open_windows_automatically(False)
        hw = os.environ.get("IYAKKI_WEBVIEW_HW", "ondemand")
        settings.set_hardware_acceleration_policy({"always": WebKit2.HardwareAccelerationPolicy.ALWAYS, "never": WebKit2.HardwareAccelerationPolicy.NEVER}.get(hw, WebKit2.HardwareAccelerationPolicy.ON_DEMAND))
        if hello.ua:
            name, _, ver = hello.ua.partition("/")
            settings.set_user_agent_with_application_details(name, ver or None)
        self.view = WebKit2.WebView.new_with_context(ctx)
        self.view.set_settings(settings)
        if self.backend == "xvfb":
            self.win = Gtk.Window()
            self.win.set_decorated(False)
            self.win.set_resizable(True)
            self.win.set_default_size(self.w, self.h)
            self.win.move(0, 0)
        else:
            self.win = Gtk.OffscreenWindow()
            self.win.set_default_size(self.w, self.h)
        self.view.set_size_request(self.w, self.h)
        self.win.add(self.view)
        self.apply_zoom()
        v = self.view
        v.connect("notify::title", lambda *_: self.send(F_TITLE, pack_str(v.get_title() or "")))
        v.connect("notify::uri", lambda *_: self.on_uri())
        v.connect("notify::estimated-load-progress", lambda *_: self.on_progress())
        v.connect("load-changed", self.on_load_changed)
        v.connect("load-failed", self.on_load_failed)
        v.connect("decide-policy", self.on_policy)
        v.connect("create", self.on_create)
        v.connect("mouse-target-changed", self.on_target)
        v.connect("permission-request", lambda _v, req: (req.deny(), True)[1])
        v.connect("web-process-terminated", self.on_crash)
        v.get_back_forward_list().connect("changed", lambda *_: self.send_history())
        if self.backend == "xvfb":
            self.win.show_all()
            self.win.get_window().move_resize(0, 0, self.w, self.h)
            self.win.get_frame_clock().connect("after-paint", lambda c: self.on_paint())
            GLib.timeout_add(250, self.on_keepalive)
        else:
            self.win.connect("damage-event", self.on_damage)
            self.win.show_all()
        engine_version = "%d.%d.%d" % (WebKit2.get_major_version(), WebKit2.get_minor_version(), WebKit2.get_micro_version())
        self.send(F_READY, struct.pack("<I", PROTOCOL_VERSION) + pack_str("WebKitGTK") + pack_str(engine_version + " (" + self.backend + ")"))
        self.mark_all()

    def apply_zoom(self):
        if self.view:
            self.view.set_zoom_level(self.scale * self.zoom / 10000.0)

    # --- engine signals ------------------------------------------------------------------------------------------
    def host_allowed(self, uri):
        scheme = uri.split(":", 1)[0].lower()
        if scheme in ("about", "data", "blob"):
            return True
        if scheme not in ("http", "https"):
            return False
        try:
            host = (GLib.Uri.parse(uri, GLib.UriFlags.NONE).get_host() or "").lower()
        except Exception:
            return False
        for a in self.allow:
            a = a.lower()
            if a == "*" or a == host or host.endswith("." + a):
                return True
        return False

    def on_policy(self, view, decision, dtype):
        if dtype == WebKit2.PolicyDecisionType.NAVIGATION_ACTION:
            uri = decision.get_navigation_action().get_request().get_uri()
            if not self.host_allowed(uri):
                self.log("blocked navigation to " + uri)
                decision.ignore()
                return True
        elif dtype == WebKit2.PolicyDecisionType.RESPONSE:
            if not decision.is_mime_type_supported():
                decision.download()
                return True
        return False

    def on_create(self, view, action):
        # window.open / target=_blank: stay in this view (the shell has one page area per tab)
        uri = action.get_request().get_uri()
        if uri and uri != "about:blank":
            GLib.idle_add(lambda: (view.load_uri(uri), False)[1])
        return None

    def on_uri(self):
        uri = self.view.get_uri() or ""
        if uri != self.committed:
            self.committed = uri
            self.send(F_URL, pack_str(uri))
        self.send_history()

    def on_progress(self):
        p = int(self.view.get_estimated_load_progress() * 100)
        self.send(F_LOADING, struct.pack("<BB", 1 if self.view.is_loading() else 0, min(p, 100)))

    def on_load_changed(self, view, event):
        if event == WebKit2.LoadEvent.FINISHED:
            self.send(F_LOADING, struct.pack("<BB", 0, 100))
        elif event == WebKit2.LoadEvent.STARTED:
            self.send(F_LOADING, struct.pack("<BB", 1, 0))
        self.send_history()

    def on_load_failed(self, view, event, uri, error):
        self.log("load failed: %s: %s" % (uri, error.message))
        self.send(F_LOADING, struct.pack("<BB", 0, 100))
        return False

    def send_history(self):
        if self.view:
            self.send(F_HISTORY, struct.pack("<BB", 1 if self.view.can_go_back() else 0, 1 if self.view.can_go_forward() else 0))

    def on_target(self, view, hit, mods):
        kind = CUR_DEFAULT
        if hit.context_is_link():
            kind = CUR_POINTER
        elif hit.context_is_editable() or hit.context_is_selection():
            kind = CUR_TEXT
        if kind != self.cursor:
            self.cursor = kind
            self.send(F_CURSOR, struct.pack("<B", kind))

    def on_crash(self, view, reason):
        self.crashes += 1
        self.send(F_CRASHED, pack_str("the web process stopped (%s)" % reason.value_nick))
        if self.crashes < 5:
            GLib.timeout_add(500, lambda: (view.reload(), False)[1])

    def on_download(self, ctx, download):
        url = download.get_request().get_uri()
        state = {"sent": False}

        def report(name):
            if not state["sent"]:
                state["sent"] = True
                self.send(F_DOWNLOAD, pack_str(url) + pack_str(name))
                download.cancel()  # the shell decides what to save, and where

        def decide(d, suggested):
            report(suggested or os.path.basename(url.split("?")[0]) or "download")
            return True

        download.connect("decide-destination", decide)
        GLib.timeout_add(1500, lambda: (report(os.path.basename(url.split("?")[0]) or "download"), False)[1])

    # --- frames -----------------------------------------------------------------------------------------------
    def mark_all(self):
        self.prev = None
        self.maybe = True
        if self.backend == "xvfb":
            self.dirty = (0, self.h)
        else:
            self.dirty = (0, 0, self.w, self.h)
        self.schedule_flush()

    def on_damage(self, win, ev):
        a = ev.area
        x0, y0, x1, y1 = max(a.x, 0), max(a.y, 0), min(a.x + a.width, self.w), min(a.y + a.height, self.h)
        if x1 > x0 and y1 > y0:
            d = self.dirty
            self.dirty = (x0, y0, x1, y1) if d is None else (min(d[0], x0), min(d[1], y0), max(d[2], x1), max(d[3], y1))
            self.schedule_flush()
        return False

    def on_paint(self):
        self.maybe = True
        self.idle_since = time.monotonic()
        self.schedule_flush()

    def on_keepalive(self):
        # a change the frame clock did not announce (video textures, the cursor in a text field)
        if time.monotonic() - self.idle_since < 1.0 or self.prev is None:
            self.maybe = True
            self.schedule_flush()
        return True

    def schedule_flush(self):
        if self.flush_source or not self.visible:
            return
        wait = max(0.0, 1.0 / self.fps - (time.monotonic() - self.last_frame))
        self.flush_source = GLib.timeout_add(int(wait * 1000) + 1, self.flush)

    def free_slot(self):
        if not self.busy[self.next_slot]:
            return self.next_slot
        if not self.busy[1 - self.next_slot]:
            return 1 - self.next_slot
        return None

    def flush(self):
        self.flush_source = 0
        if not self.visible or self.win is None:
            return False
        if self.backend == "xvfb":
            return self.flush_xvfb()
        return self.flush_offscreen()

    def emit(self, slot, band, rect):
        """Write converted rows `band` (BGRA, whole width) starting at row rect[1] into the slot and announce `rect`."""
        stride = self.w * 4
        os.pwrite(self.shm_fd, bytes(band), slot * self.slot_bytes + rect[1] * stride)
        self.busy[slot] = True
        self.next_slot = 1 - slot
        self.last_frame = time.monotonic()
        self.send(F_FRAME, struct.pack("<BIII", slot, self.w, self.h, 1) + struct.pack("<IIII", rect[0], rect[1], rect[2] - rect[0], rect[3] - rect[1]))

    def convert(self, src, rs, channels, y0, y1):
        """Rows y0..y1 of an RGB(A) pixbuf as BGRA, all alpha 255."""
        w = self.w
        n = y1 - y0
        out = bytearray(w * 4 * n)
        if rs == w * channels:
            band = src[y0 * rs : y1 * rs]
            out[0::4] = band[2::channels]
            out[1::4] = band[1::channels]
            out[2::4] = band[0::channels]
        else:
            for row in range(y0, y1):
                line = src[row * rs : row * rs + w * channels]
                o = (row - y0) * w * 4
                out[o : o + w * 4 : 4] = line[2::channels]
                out[o + 1 : o + w * 4 : 4] = line[1::channels]
                out[o + 2 : o + w * 4 : 4] = line[0::channels]
        out[3::4] = b"\xff" * (w * n)
        return out

    def flush_offscreen(self):
        d = self.dirty
        if d is None:
            return False
        slot = self.free_slot()
        if slot is None:
            return False  # an Ack will call us again
        pb = self.win.get_pixbuf()
        if pb is None:
            return False
        w, h = pb.get_width(), pb.get_height()
        if (w, h) != (self.w, self.h):
            self.w, self.h = min(w, self.max_w), min(h, self.max_h)
            d = (0, 0, self.w, self.h)
        y0, y1 = d[1], min(d[3], h)
        band = self.convert(pb.get_pixels(), pb.get_rowstride(), pb.get_n_channels(), y0, y1)
        self.dirty = None
        self.emit(slot, band, (d[0], y0, min(d[2], self.w), y1))
        return False

    def flush_xvfb(self):
        if not self.maybe and self.dirty is None:
            return False
        slot = self.free_slot()
        if slot is None:
            return False
        gw = self.win.get_window()
        if gw is None:
            return False
        w, h = min(gw.get_width(), self.w), min(gw.get_height(), self.h)
        if (w, h) != (self.w, self.h):
            # the window has not reached the requested size yet; try again shortly
            self.maybe = True
            self.flush_source = GLib.timeout_add(30, self.flush)
            return False
        pb = Gdk.pixbuf_get_from_window(gw, 0, 0, w, h)
        if pb is None:
            return False
        self.maybe = False
        src = pb.get_pixels()
        rs, ch = pb.get_rowstride(), pb.get_n_channels()
        prev = self.prev
        self.prev = src
        if prev is None or len(prev) != len(src):
            y0, y1 = 0, h
        else:
            if src == prev:
                self.dirty = None
                return False
            # first and last changed rows
            y0 = 0
            while y0 < h and src[y0 * rs : (y0 + 1) * rs] == prev[y0 * rs : (y0 + 1) * rs]:
                y0 += 1
            y1 = h
            while y1 > y0 and src[(y1 - 1) * rs : y1 * rs] == prev[(y1 - 1) * rs : y1 * rs]:
                y1 -= 1
        self.dirty = None
        band = self.convert(src, rs, ch, y0, y1)
        self.emit(slot, band, (0, y0, w, y1))
        return False

    # --- input ------------------------------------------------------------------------------------------------
    def stamp(self):
        return int(time.monotonic() * 1000) & 0x7FFFFFFF

    def new_event(self, etype, toplevel=False):
        e = Gdk.Event.new(etype)
        e.window = (self.win if toplevel else self.view).get_window()
        e.send_event = True
        e.time = self.stamp()
        return e

    def gdk_state(self, mods):
        s = 0
        if mods & MOD_SHIFT:
            s |= Gdk.ModifierType.SHIFT_MASK
        if mods & MOD_CTRL:
            s |= Gdk.ModifierType.CONTROL_MASK
        if mods & MOD_ALT:
            s |= Gdk.ModifierType.MOD1_MASK
        if mods & MOD_SUPER:
            s |= Gdk.ModifierType.SUPER_MASK
        return s

    def ready_for_input(self):
        return self.view is not None and self.view.get_window() is not None

    def pointer(self):
        return Gdk.Display.get_default().get_default_seat().get_pointer()

    def keyboard(self):
        return Gdk.Display.get_default().get_default_seat().get_keyboard()

    def on_mouse(self, kind, x, y, button, buttons, mods):
        if not self.ready_for_input():
            return
        state = self.gdk_state(mods)
        for bit, mask in ((1, Gdk.ModifierType.BUTTON1_MASK), (2, Gdk.ModifierType.BUTTON2_MASK), (4, Gdk.ModifierType.BUTTON3_MASK)):
            if buttons & bit:
                state |= mask
        if kind == 0:
            e = self.new_event(Gdk.EventType.MOTION_NOTIFY)
        else:
            e = self.new_event(Gdk.EventType.BUTTON_PRESS if kind == 1 else Gdk.EventType.BUTTON_RELEASE)
            e.button = button or 1
            if kind == 1:
                self.view.grab_focus()
        e.x, e.y, e.x_root, e.y_root, e.state = float(x), float(y), float(x), float(y), state
        e.set_device(self.pointer())
        Gtk.main_do_event(e)
        self.idle_since = time.monotonic()

    def on_wheel(self, x, y, dx, dy, mods):
        if not self.ready_for_input():
            return
        e = self.new_event(Gdk.EventType.SCROLL)
        e.x, e.y, e.x_root, e.y_root = float(x), float(y), float(x), float(y)
        e.direction = Gdk.ScrollDirection.SMOOTH
        e.delta_x, e.delta_y = dx / 120.0, dy / 120.0
        e.state = self.gdk_state(mods)
        e.set_device(self.pointer())
        Gtk.main_do_event(e)
        self.idle_since = time.monotonic()

    def on_key(self, down, keysym, mods, text):
        if not self.ready_for_input():
            return
        e = self.new_event(Gdk.EventType.KEY_PRESS if down else Gdk.EventType.KEY_RELEASE)
        e.keyval = keysym
        e.state = self.gdk_state(mods)
        ok, keys = Gdk.Keymap.get_for_display(Gdk.Display.get_default()).get_entries_for_keyval(keysym)
        if ok and keys:
            e.hardware_keycode = keys[0].keycode
            e.group = keys[0].group
        e.set_device(self.keyboard())
        Gtk.main_do_event(e)
        self.idle_since = time.monotonic()
        if down and (mods & MOD_CTRL) and keysym in (0x63, 0x43, 0x78, 0x58):  # Ctrl+C / Ctrl+X: tell the shell what was copied
            GLib.timeout_add(150, self.report_selection)

    def report_selection(self):
        js = "(function(){var a=document.activeElement;if(a&&a.selectionStart!=null&&a.selectionEnd>a.selectionStart){return a.value.substring(a.selectionStart,a.selectionEnd)}return String(window.getSelection())})()"

        def done(view, res, _):
            try:
                val = view.evaluate_javascript_finish(res)
                s = val.to_string() if val is not None else ""
                if s:
                    self.send(F_CLIPBOARD, pack_str(s))
            except Exception:
                pass

        self.view.evaluate_javascript(js, -1, None, None, None, done, None)
        return False

    def on_script(self, sid, js, timeout_ms):
        """Protocol 1.1 Script: evaluate js in the top-level page, answer ScriptResult with the value as JSON text."""
        state = {"done": False}

        def reply(ok, text):
            if state["done"]:
                return
            state["done"] = True
            self.send(F_SCRIPT_RESULT, struct.pack("<IB", sid, 1 if ok else 0) + pack_str(text))

        def done(view, res, _):
            try:
                val = view.evaluate_javascript_finish(res)
                if val is None or val.is_undefined() or val.is_null():
                    reply(True, "null")
                else:
                    reply(True, val.to_json(0) or "null")
            except Exception as e:
                reply(False, str(e)[:500])

        def timed_out():
            reply(False, "script timed out")
            return False

        try:
            self.view.evaluate_javascript(js, -1, None, None, None, done, None)
        except Exception as e:
            reply(False, str(e)[:500])
            return
        GLib.timeout_add(max(100, min(timeout_ms or 10000, 120000)), timed_out)
        self.idle_since = time.monotonic()

    def on_text(self, text):
        js = "document.execCommand('insertText', false, %s)" % json.dumps(text)
        self.view.evaluate_javascript(js, -1, None, None, None, None, None)

    def on_focus(self, on):
        if not self.ready_for_input():
            return
        if on:
            self.view.grab_focus()
        e = self.new_event(Gdk.EventType.FOCUS_CHANGE, toplevel=True)
        e.focus_change.in_ = 1 if on else 0
        e.set_device(self.keyboard())
        Gtk.main_do_event(e)

    # --- dispatch -------------------------------------------------------------------------------------------
    def handle(self, ty, p):
        r = Reader(p)
        if self.view is None:
            return
        if ty == T_NAVIGATE:
            self.view.load_uri(r.s())
        elif ty == T_RELOAD:
            if r.u8():
                self.view.reload_bypass_cache()
            else:
                self.view.reload()
        elif ty == T_STOP:
            self.view.stop_loading()
        elif ty == T_BACK:
            self.view.go_back()
        elif ty == T_FORWARD:
            self.view.go_forward()
        elif ty == T_RESIZE:
            w, h = r.u32(), r.u32()
            self.w, self.h = max(1, min(w, self.max_w, MAX_SCREEN[0])), max(1, min(h, self.max_h, MAX_SCREEN[1]))
            self.view.set_size_request(self.w, self.h)
            if self.backend == "xvfb":
                self.win.resize(self.w, self.h)
                if self.win.get_window() is not None:
                    self.win.get_window().move_resize(0, 0, self.w, self.h)
            else:
                self.win.resize(self.w, self.h)
            self.mark_all()
        elif ty == T_MOUSE:
            kind, x, y, button, buttons, mods = r.u8(), r.i32(), r.i32(), r.u8(), r.u8(), r.u32()
            self.on_mouse(kind, x, y, button, buttons, mods)
        elif ty == T_WHEEL:
            self.on_wheel(r.i32(), r.i32(), r.i32(), r.i32(), r.u32())
        elif ty == T_KEY:
            down, keysym, mods = r.u8(), r.u32(), r.u32()
            self.on_key(bool(down), keysym, mods, r.s())
        elif ty == T_TEXT:
            self.on_text(r.s())
        elif ty == T_ZOOM:
            self.zoom = max(25, min(r.u32(), 500))
            self.apply_zoom()
        elif ty == T_FOCUS:
            self.on_focus(bool(r.u8()))
        elif ty == T_VISIBLE:
            self.visible = bool(r.u8())
            if self.visible:
                self.mark_all()
        elif ty == T_ACK:
            s = r.u8()
            if s < 2:
                self.busy[s] = False
            if self.dirty is not None or self.maybe:
                self.schedule_flush()
        elif ty == T_SCRIPT:
            sid, tmo = r.u32(), r.u32()
            self.on_script(sid, r.s(), tmo)
        elif ty == T_QUIT:
            Gtk.main_quit()

    def feed(self, chunk):
        self.inbuf += chunk
        while len(self.inbuf) >= 4:
            n = struct.unpack_from("<I", self.inbuf, 0)[0]
            if n == 0 or n > (4 << 20):
                sys.stderr.write("host-helper: impossible message length %d\n" % n)
                Gtk.main_quit()
                return
            if len(self.inbuf) < 4 + n:
                break
            ty = self.inbuf[4]
            payload = bytes(self.inbuf[5 : 4 + n])
            del self.inbuf[: 4 + n]
            try:
                self.handle(ty, payload)
            except Exception as e:  # a bad message must not take the helper down
                import traceback
                sys.stderr.write("host-helper: message %#x failed: %s\n%s" % (ty, e, traceback.format_exc()))

    def on_readable(self, fd, cond):
        if cond & (GLib.IOCondition.HUP | GLib.IOCondition.ERR) and not cond & GLib.IOCondition.IN:
            Gtk.main_quit()
            return False
        try:
            chunk = self.sock.recv(1 << 16)
        except BlockingIOError:
            return True
        except OSError:
            Gtk.main_quit()
            return False
        if not chunk:
            Gtk.main_quit()
            return False
        self.feed(chunk)
        return True


def read_hello(sock):
    """Block until the first message (Hello) has arrived. Returns (Hello, bytes read beyond it)."""
    buf = bytearray()
    while True:
        chunk = sock.recv(1 << 16)
        if not chunk:
            sys.exit(0)
        buf += chunk
        if len(buf) >= 4:
            n = struct.unpack_from("<I", buf, 0)[0]
            if n == 0 or n > (4 << 20):
                sys.stderr.write("host-helper: impossible message length %d\n" % n)
                sys.exit(2)
            if len(buf) >= 4 + n:
                if buf[4] != T_HELLO:
                    sys.stderr.write("host-helper: the first message was not Hello\n")
                    sys.exit(2)
                return Hello(Reader(bytes(buf[5 : 4 + n]))), bytes(buf[4 + n :])


def main():
    fd, shm = 0, ""
    args = sys.argv[1:]
    i = 0
    while i < len(args):
        if args[i] == "--socket-fd":
            fd = int(args[i + 1])
            i += 1
        elif args[i] == "--shm":
            shm = args[i + 1]
            i += 1
        i += 1
    if not shm:
        sys.stderr.write("usage: host-helper.py --socket-fd N --shm PATH\n")
        return 2
    sock = socket.socket(fileno=fd)
    hello, rest = read_hello(sock)
    shm_fd = os.open(shm, os.O_RDWR)
    backend = os.environ.get("IYAKKI_WEBVIEW_BACKEND", "")
    xvfb = None
    if backend != "offscreen" and shutil.which("Xvfb"):
        try:
            sw, sh = min(max(hello.w, hello.max_w, 1280), MAX_SCREEN[0]), min(max(hello.h, hello.max_h, 720), MAX_SCREEN[1])
            xvfb = Xvfb(sw, sh)
            os.environ["DISPLAY"] = xvfb.display
            os.environ["GDK_BACKEND"] = "x11"
            os.environ.pop("WAYLAND_DISPLAY", None)
            backend = "xvfb"
        except Exception as e:
            sys.stderr.write("host-helper: Xvfb failed (%s); using the offscreen window (no video)\n" % e)
            xvfb = None
    if xvfb is None:
        if backend == "xvfb":
            sys.stderr.write("host-helper: IYAKKI_WEBVIEW_BACKEND=xvfb but Xvfb is not installed (apt install xvfb)\n")
            return 3
        backend = "offscreen"
        # WebKitGTK cannot make a GL context inside a GtkOffscreenWindow with its dmabuf renderer
        os.environ.setdefault("WEBKIT_DISABLE_DMABUF_RENDERER", "1")
        sys.stderr.write("host-helper: no Xvfb found: video will not play (apt install xvfb)\n")
    load_gtk()
    if Gdk.Display.get_default() is None:
        sys.stderr.write("host-helper: cannot open the display %s\n" % os.environ.get("DISPLAY", "(none)"))
        if xvfb:
            xvfb.stop()
        return 3
    try:
        h = Helper(sock, shm_fd, hello, backend)
        sock.setblocking(False)
        h.setup()
        if rest:
            h.feed(rest)
        GLib.io_add_watch(fd, GLib.PRIORITY_DEFAULT, GLib.IOCondition.IN | GLib.IOCondition.HUP | GLib.IOCondition.ERR, h.on_readable)
        Gtk.main()
    finally:
        if xvfb:
            xvfb.stop()
    return 0


if __name__ == "__main__":
    sys.exit(main())
