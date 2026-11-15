# SPDX-License-Identifier: Apache-2.0
# Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).
"""A fake IYAKKI machine for the browser client page: serves the real page files under /connect with the real Content-Security-Policy,
and speaks the client protocol (iyakki_core_api::client) on /client/ws from a script. No hardware, no signature check (the golden
signature vectors are checked by the glue tests); it records every message the page sends, so a test can see what a real click or key
press became. One page at a time."""
import http.server
import json
import os
import re
import threading
import time

import ws

ENGINE_FILES = {"engine.js": "js/engine.js", "keys.js": "js/keys.js", "media.js": "js/media.js", "audio-worklet.js": "js/audio-worklet.js", "engine.wasm": "assets/iyakki-client-engine.wasm"}


def page_files(root):
    eng = os.path.join(root, "crates/iyakki-client-engine")
    cp = os.path.join(root, "crates/iyakki-remote/src/clientpage")
    files = {"/connect": ("text/html; charset=utf-8", open(os.path.join(cp, "index.html"), "rb").read()),
             "/connect/page.js": ("text/javascript; charset=utf-8", open(os.path.join(cp, "page.js"), "rb").read()),
             "/connect/page.css": ("text/css; charset=utf-8", open(os.path.join(cp, "page.css"), "rb").read())}
    for name, rel in ENGINE_FILES.items():
        files["/connect/" + name] = ("application/wasm" if name.endswith(".wasm") else "text/javascript; charset=utf-8", open(os.path.join(eng, rel), "rb").read())
    src = open(os.path.join(root, "crates/iyakki-remote/src/clientpage.rs")).read()
    csp = re.search(r'pub const CSP: &str = "([^"]+)";', src).group(1)
    perm = re.search(r'pub const PERMISSIONS_POLICY: &str = "([^"]+)";', src).group(1)
    return files, csp, perm


class FakeHost:
    def __init__(self, root, frame_bin, token="tok-1", password="right-pw", person="ann", caps=("picture", "input", "events", "calls")):
        self.files, self.csp, self.perm = page_files(root)
        self.frame = frame_bin
        self.token, self.password, self.person, self.caps = token, password, person, list(caps)
        self.lock = threading.Lock()
        self.messages = []  # what the page sent, parsed
        self.binaries = []
        self.connections = 0
        self.known = {}  # device id -> public key
        self.script = {}  # hooks for a test: "after_signin" -> callable(sock), "drop_after_picture" -> bool
        self.sock = None
        host = self

        class H(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *a):
                pass

            def do_GET(self):
                path = self.path.split("?", 1)[0]
                if path == "/client/ws" and self.headers.get("Upgrade", "").lower() == "websocket":
                    return host._serve_ws(self)
                if path in host.files:
                    ctype, body = host.files[path]
                    self.send_response(200)
                    self.send_header("Content-Type", ctype)
                    self.send_header("Content-Length", str(len(body)))
                    self.send_header("Content-Security-Policy", host.csp)
                    self.send_header("Referrer-Policy", "no-referrer")
                    self.send_header("Cache-Control", "no-store")
                    self.send_header("Permissions-Policy", host.perm)
                    self.end_headers()
                    self.wfile.write(body)
                    return
                self.send_response(404)
                self.send_header("Content-Length", "0")
                self.end_headers()

        self.srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
        self.srv.daemon_threads = True
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()

    @property
    def base(self):
        return "http://127.0.0.1:%d" % self.srv.server_address[1]

    def stop(self):
        try:
            if self.sock:
                self.sock.close()
        except Exception:
            pass
        self.srv.shutdown()
        self.srv.server_close()

    def sent(self, t):
        with self.lock:
            return [m for m in self.messages if m.get("t") == t]

    def wait_for(self, t, count=1, timeout=10):
        t0 = time.time()
        while time.time() - t0 < timeout:
            if len(self.sent(t)) >= count:
                return self.sent(t)
            time.sleep(0.05)
        raise TimeoutError("the page never sent %s (got %s)" % (t, [m.get("t") for m in self.messages]))

    def _serve_ws(self, h):
        h.close_connection = True
        sock = ws.ServerSocket.accept(h.connection, h.headers)
        with self.lock:
            self.connections += 1
            self.sock = sock
        device = None
        try:
            while True:
                m = sock.recv(None)
                if isinstance(m, bytes):
                    with self.lock:
                        self.binaries.append(m)
                    continue
                d = json.loads(m)
                with self.lock:
                    self.messages.append(d)
                t = d.get("t")
                if t == "hello":
                    sock.send(json.dumps({"t": "hello_ok", "v": 1, "caps": self.caps, "challenge": "chal-%d" % self.connections, "tile": 64, "max_message": 65536}))
                elif t == "enrol":
                    if d.get("token") != self.token:
                        sock.send(json.dumps({"t": "error", "code": "token_unknown", "message": "no such link"}))
                        continue
                    device = "dev-%d" % (len(self.known) + 1)
                    self.known[device] = d["public"]
                    sock.send(json.dumps({"t": "enrolled", "device": device, "challenge": "chal-e%d" % self.connections}))
                elif t == "auth":
                    sock.send(json.dumps({"t": "authed", "device": device or "dev-1", "name": "Test browser"}))
                elif t == "sign_in":
                    if d.get("password") == self.password:
                        sock.send(json.dumps({"t": "signed_in", "person": d.get("user", self.person)}))
                    else:
                        sock.send(json.dumps({"t": "error", "code": "sign_in_refused", "message": "no"}))
                elif t == "picture_open":
                    sock.send(json.dumps({"t": "picture_ok"}))
                    sock.send(b"\x01" + self.frame)
                    if self.script.get("after_picture"):
                        self.script["after_picture"](sock)
                elif t == "audio_open":
                    sock.send(json.dumps({"t": "audio_ok", "rate": 48000, "channels": 2, "chunk_ms": 20, "side": d.get("side") or "client"}))
                    # ten 20 ms chunks of a 440 Hz tone: tag 2, sequence number (u32 little endian), 960 frames of 16-bit stereo
                    import math
                    import struct
                    for seq in range(10):
                        pcm = b"".join(struct.pack("<hh", int(8000 * math.sin(2 * math.pi * 440 * (seq * 960 + i) / 48000)), int(8000 * math.sin(2 * math.pi * 440 * (seq * 960 + i) / 48000))) for i in range(960))
                        sock.send(b"\x02" + struct.pack("<I", seq) + pcm)
                elif t == "clipboard_get":
                    sock.send(json.dumps({"t": "clipboard_text", "text": "text from the machine"}))
                elif t == "clipboard_put":
                    sock.send(json.dumps({"t": "clipboard_ok"}))
                elif t == "camera_list":
                    sock.send(json.dumps({"t": "camera_list", "cameras": [{"unit": "usb:video0", "name": "Desk camera"}]}))
                elif t == "ping":
                    sock.send(json.dumps({"t": "pong", "ts": d.get("ts", 0)}))
        except (ws.WsError, OSError, ValueError):
            pass
        finally:
            sock.close()
