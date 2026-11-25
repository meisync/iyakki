#!/usr/bin/env python3
"""A fake network camera for the automated camera tests (decision D-CAM-AUTO-01). Standard library only, runs on the host.

It serves three things on loopback:
  * a Motion-JPEG stream over HTTP (`/video.mjpg`, multipart/x-mixed-replace) and a single picture (`/snapshot.jpg`);
  * an RTSP server over TCP (OPTIONS, DESCRIBE, SETUP, PLAY, GET_PARAMETER, TEARDOWN) that sends RFC 2435 Motion-JPEG as RTP packets
    interleaved on channel 0 (SDP payload type 26), the way the in-house client in crates/iyakki-rtsp expects it;
  * an optional sign-in: Basic, or Digest (MD5, qop=auth) with one user name and password.

The pictures are three committed JPEGs (dist/autotest/data): a still scene, and the same scene with a block at the left and at the
right. The schedule `motion at T seconds for N seconds` (entries separated by `;`) makes the block alternate between the two positions
in that window, so motion detection can be triggered at a known time. Times count from the moment the camera starts.

Every request the client makes, and every stream, goes to an event log (JSON lines, `--log FILE`), with the time in seconds since the
start. Passwords and Authorization headers are never written.

Scenario use (the QEMU guest reaches the host at 10.0.2.2, which QEMU user networking forwards to the host's loopback):

    python3 dist/autotest/fakecam.py --rtsp-port 8554 --http-port 8080 --user admin --password-env FAKECAM_PASSWORD \
        --auth digest --schedule "motion at 5 seconds for 10 seconds" --log RESULT/fakecam.jsonl

It prints `fakecam ready http=PORT rtsp=PORT` on stdout once both listeners are bound, and exits on SIGTERM or SIGINT.
"""
import argparse
import base64
import hashlib
import hmac
import json
import os
import re
import secrets
import select
import signal
import socket
import socketserver
import sys
import threading
import time
from urllib.parse import urlsplit

HERE = os.path.dirname(os.path.abspath(__file__))
DATA_DIR = os.path.join(HERE, "data")
PICTURE_FILES = {"still": "fakecam-still.jpg", "left": "fakecam-block-left.jpg", "right": "fakecam-block-right.jpg"}
REALM = "fakecam"
BOUNDARY = "fakecamframe"
MAX_HEAD = 16384
MAX_PAYLOAD = 1000  # RTP payload bytes per packet (under the Ethernet MTU)
RTP_JPEG_PAYLOAD_TYPE = 26
CLOCK_RATE = 90000

_WINDOW = re.compile(r"^\s*motion\s+at\s+(\d+(?:\.\d+)?)\s*(?:seconds?|s)?\s+for\s+(\d+(?:\.\d+)?)\s*(?:seconds?|s)?\s*$", re.I)
_PARAM = re.compile(r'(\w+)\s*=\s*(?:"([^"]*)"|([^,\s]*))')


class ScheduleError(ValueError):
    pass


def parse_schedule(text):
    """`motion at T seconds for N seconds` entries (separated by `;` or new lines) as (start, end) pairs. `none` or empty: no motion."""
    windows = []
    for part in re.split(r"[;\n]", text or ""):
        part = part.strip()
        if not part or part.lower() == "none":
            continue
        m = _WINDOW.match(part)
        if not m:
            raise ScheduleError("cannot read %r; write 'motion at T seconds for N seconds'" % part)
        start, length = float(m.group(1)), float(m.group(2))
        if length <= 0:
            raise ScheduleError("the motion must last more than 0 seconds: %r" % part)
        windows.append((start, start + length))
    return windows


def scene_at(windows, t):
    """Whether the block moves at time t (seconds since the start)."""
    return any(start <= t < end for start, end in windows)


def picture_at(windows, fps, t, frame=None):
    """(picture name, motion) at time t: the still scene, or in a motion window the block at the left and at the right, alternating
    each frame (`frame` is the frame's number in its stream; without it, the frame is counted from the clock at `fps`)."""
    if not scene_at(windows, t):
        return "still", False
    n = int(t * fps) if frame is None else frame
    return ("left" if n % 2 == 0 else "right"), True


# ---- JPEG pieces for RFC 2435 ---------------------------------------------------------------------------------------------------

def split_jpeg(data):
    """(width, height, quantisation tables in zig-zag order (luma then chroma, 128 bytes), entropy-coded scan) of a 4:2:0 baseline
    picture written by fakecam_jpeg.py. Anything else is refused."""
    if data[:2] != b"\xff\xd8":
        raise ValueError("not a JPEG file")
    tables, size, i = {}, None, 2
    while True:
        if i + 4 > len(data) or data[i] != 0xFF:
            raise ValueError("broken JPEG segment at byte %d" % i)
        marker = data[i + 1]
        length = int.from_bytes(data[i + 2:i + 4], "big")
        seg = data[i + 4:i + 2 + length]
        if marker == 0xDB:
            j = 0
            while j < len(seg):
                if seg[j] >> 4:
                    raise ValueError("16-bit quantisation tables are not sent")
                tables[seg[j] & 0x0F] = seg[j + 1:j + 65]
                j += 65
        elif marker == 0xC0:
            height, width = int.from_bytes(seg[1:3], "big"), int.from_bytes(seg[3:5], "big")
            if seg[5] != 3 or [seg[7], seg[10], seg[13]] != [0x22, 0x11, 0x11] or width % 16 or height % 16:
                raise ValueError("only 4:2:0 colour pictures with sides in multiples of 16 are sent")
            size = (width, height)
        elif marker == 0xDA:
            if size is None or 0 not in tables or 1 not in tables:
                raise ValueError("the picture has no frame header or tables before its scan")
            scan = data[i + 2 + length:]
            if scan.endswith(b"\xff\xd9"):
                scan = scan[:-2]
            return size[0], size[1], bytes(tables[0] + tables[1]), scan
        elif marker in (0xC1, 0xC2, 0xC3):
            raise ValueError("only baseline pictures are sent")
        i += 2 + length


def packetize_jpeg(width, height, qtables, scan, max_payload=MAX_PAYLOAD):
    """RFC 2435 payloads of one picture: type 1 (4:2:0), Q 255 with the tables in the first packet of the frame. Returns
    [(marker, payload)]; the marker is set on the last payload."""
    out, off = [], 0
    while True:
        head = bytes([0]) + off.to_bytes(3, "big") + bytes([1, 255, (width // 8) & 0xFF, (height // 8) & 0xFF])
        extra = bytes([0, 0]) + len(qtables).to_bytes(2, "big") + qtables if off == 0 else b""
        take = min(max_payload - len(head) - len(extra), len(scan) - off)
        off += take
        out.append((off >= len(scan), head + extra + scan[off - take:off]))
        if off >= len(scan):
            return out


def rtp_header(payload_type, marker, seq, timestamp, ssrc):
    return bytes([0x80, (payload_type & 0x7F) | (0x80 if marker else 0)]) + (seq & 0xFFFF).to_bytes(2, "big") + (timestamp & 0xFFFFFFFF).to_bytes(4, "big") + (ssrc & 0xFFFFFFFF).to_bytes(4, "big")


class Picture:
    def __init__(self, data):
        self.data = data
        self.width, self.height, qtables, scan = split_jpeg(data)
        self.payloads = packetize_jpeg(self.width, self.height, qtables, scan)


def load_pictures(directory=DATA_DIR):
    pictures = {}
    for name, filename in PICTURE_FILES.items():
        with open(os.path.join(directory, filename), "rb") as f:
            pictures[name] = Picture(f.read())
    return pictures


# ---- HTTP and RTSP wire helpers -------------------------------------------------------------------------------------------------

class _Closed(Exception):
    pass


class _Reader:
    """Bytes from one connection; whole heads and bodies are taken from `buf`."""

    def __init__(self, sock):
        self.sock = sock
        self.buf = bytearray()

    def take_head(self):
        i = self.buf.find(b"\r\n\r\n")
        if i < 0:
            if len(self.buf) > MAX_HEAD:
                raise _Closed("request head too long")
            return None
        text = bytes(self.buf[:i + 4]).decode("iso-8859-1")
        del self.buf[:i + 4]
        return text

    def fill(self):
        data = self.sock.recv(4096)
        if not data:
            raise _Closed("the client closed the connection")
        self.buf += data

    def read_head(self):
        while True:
            text = self.take_head()
            if text is not None:
                return text
            self.fill()

    def discard_body(self, n):
        while len(self.buf) < n:
            self.fill()
        del self.buf[:n]


def parse_head(text):
    lines = text.split("\r\n")
    headers = {}
    for line in lines[1:]:
        if ":" in line:
            k, v = line.split(":", 1)
            headers[k.strip().lower()] = v.strip()
    return lines[0], headers


def _scrub(uri):
    """The request target for the log: any user name and password in it removed."""
    return re.sub(r"^([a-zA-Z]+://)[^/@]*@", r"\1", uri)[:200]


# ---- the camera -----------------------------------------------------------------------------------------------------------------

class _Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True

    def __init__(self, address, handler, camera, kind):
        self.camera = camera
        self.kind = kind
        super().__init__(address, handler)


class _Handler(socketserver.BaseRequestHandler):
    def handle(self):
        cam = self.server.camera
        if not cam._track(self.request):
            return
        try:
            if self.server.kind == "rtsp":
                cam._rtsp(self.request)
            else:
                cam._http(self.request)
        except (OSError, _Closed):
            pass
        finally:
            cam._untrack(self.request)


class FakeCamera:
    """The camera. `start()` binds the listeners (port 0: an ephemeral loopback port, printed and kept on the object); `stop()` closes
    them and every open connection. Nothing is left listening after stop()."""

    def __init__(self, *, user=None, password=None, auth="digest", schedule="", fps=5.0, log_path=None, clock=time.monotonic,
                 rtsp_path="/live", http_path="/video.mjpg", snapshot_path="/snapshot.jpg", pictures=None, schedule_thread=True):
        if (user is None) != (password is None):
            raise ValueError("a user name and a password go together")
        if auth not in ("basic", "digest"):
            raise ValueError("auth must be basic or digest")
        if not fps or fps <= 0:
            raise ValueError("fps must be above 0")
        self.user, self.password, self.auth = user, password, auth
        self.windows = parse_schedule(schedule) if isinstance(schedule, str) else [tuple(w) for w in schedule]
        self.fps = float(fps)
        self.rtsp_path = "/" + rtsp_path.strip("/")
        self.http_path = "/" + http_path.strip("/")
        self.snapshot_path = "/" + snapshot_path.strip("/")
        self.host = "127.0.0.1"
        self.http_port = None
        self.rtsp_port = None
        self._clock = clock
        self._t0 = None
        self._pictures = pictures if pictures is not None else load_pictures()
        self._nonce = secrets.token_hex(16)
        self._stop = threading.Event()
        self._cv = threading.Condition()
        self._events = []
        self._conns = set()
        self._servers = []
        self._threads = []
        self._log_path = log_path
        self._log = None
        self._schedule_thread = schedule_thread

    # -- time and events --

    def now(self):
        return self._clock() - self._t0 if self._t0 is not None else 0.0

    def emit(self, event, **fields):
        entry = {"t": round(self.now(), 3), "event": event}
        entry.update(fields)
        with self._cv:
            self._events.append(entry)
            if self._log is not None:
                self._log.write(json.dumps(entry, sort_keys=True) + "\n")
                self._log.flush()
            self._cv.notify_all()

    def events(self, name=None):
        with self._cv:
            return [dict(e) for e in self._events if name is None or e["event"] == name]

    def wait_event(self, name, count=1, timeout=10.0):
        """Block until `count` events called `name` were logged (a condition, not a sleep). False on timeout."""
        with self._cv:
            return self._cv.wait_for(lambda: sum(1 for e in self._events if e["event"] == name) >= count, timeout)

    # -- life cycle --

    def start(self, host="127.0.0.1", http=True, rtsp=True, http_port=0, rtsp_port=0):
        self.host = host
        self._t0 = self._clock()
        if self._log_path:
            self._log = open(self._log_path, "w", encoding="utf-8")
        try:
            if http:
                self._listen(_Server((host, http_port), _Handler, self, "http"), "http_port", "fakecam-http")
            if rtsp:
                self._listen(_Server((host, rtsp_port), _Handler, self, "rtsp"), "rtsp_port", "fakecam-rtsp")
        except Exception:
            self.stop()
            raise
        self.emit("start", http_port=self.http_port, rtsp_port=self.rtsp_port, auth=(self.auth if self.user else "none"),
                  fps=self.fps, schedule=[[s, e] for s, e in self.windows], http_path=self.http_path,
                  snapshot_path=self.snapshot_path, rtsp_path=self.rtsp_path)
        if self.windows and self._schedule_thread:
            t = threading.Thread(target=self._run_schedule, name="fakecam-schedule", daemon=True)
            t.start()
            self._threads.append(t)
        return self

    def _listen(self, server, attr, name):
        setattr(self, attr, server.server_address[1])
        self._servers.append(server)
        t = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05}, name=name, daemon=True)
        t.start()
        self._threads.append(t)

    def _run_schedule(self):
        """Log `motion.start` and `motion.end` at their times, so a runner can match them with what the camera showed."""
        marks = sorted([(s, "motion.start") for s, _ in self.windows] + [(e, "motion.end") for _, e in self.windows])
        for at, name in marks:
            if self._stop.wait(max(0.0, at - self.now())):
                return
            self.emit(name, at=at)

    def request_stop(self):
        """Ask the camera to stop (safe from a signal handler); `stop()` does the closing."""
        self._stop.set()

    def stop(self):
        self._stop.set()
        for server in self._servers:
            try:
                server.shutdown()
            except Exception:
                pass
            server.server_close()
        with self._cv:
            conns = list(self._conns)
        for conn in conns:
            try:
                conn.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
        for t in self._threads:
            if t is not threading.current_thread():
                t.join(2.0)
        if self._log is not None:
            self.emit("stop")
            self._log.close()
            self._log = None

    def _track(self, sock):
        with self._cv:
            if self._stop.is_set():
                return False
            self._conns.add(sock)
            return True

    def _untrack(self, sock):
        with self._cv:
            self._conns.discard(sock)

    # -- sign-in --

    def check_auth(self, method, uri, header):
        """'none' (no sign-in set), 'ok', 'challenge' (no credentials sent) or 'refused' (credentials that do not match)."""
        if self.user is None:
            return "none"
        if not header:
            return "challenge"
        if self.auth == "basic":
            want = "Basic " + base64.b64encode(("%s:%s" % (self.user, self.password)).encode()).decode()
            ok = hmac.compare_digest(header, want)
        else:
            ok = self._digest_ok(method, uri, header)
        return "ok" if ok else "refused"

    def challenge_header(self):
        if self.auth == "basic":
            return 'WWW-Authenticate: Basic realm="%s"' % REALM
        return 'WWW-Authenticate: Digest realm="%s", nonce="%s", qop="auth"' % (REALM, self._nonce)

    def _digest_ok(self, method, uri, header):
        if not header.lower().startswith("digest "):
            return False
        p = {k.lower(): (q if q else bare) for k, q, bare in _PARAM.findall(header[7:])}
        if p.get("username") != self.user or p.get("nonce") != self._nonce or p.get("uri") != uri or p.get("qop") != "auth":
            return False
        md5 = lambda s: hashlib.md5(s.encode()).hexdigest()  # noqa: E731 (RFC 2617 digest, not a secret hash)
        ha1 = md5("%s:%s:%s" % (self.user, REALM, self.password))
        ha2 = md5("%s:%s" % (method, uri))
        want = md5("%s:%s:%s:%s:auth:%s" % (ha1, self._nonce, p.get("nc", ""), p.get("cnonce", ""), ha2))
        return hmac.compare_digest(p.get("response", ""), want)

    # -- HTTP --

    def _http(self, sock):
        rd = _Reader(sock)
        first, hs = parse_head(rd.read_head())
        parts = first.split()
        if len(parts) < 2:
            return
        method, target = parts[0], parts[1]
        path = urlsplit(target).path
        if path not in (self.http_path, self.snapshot_path):
            self.emit("http.request", method=method, path=_scrub(path), status=404, auth=None)
            sock.sendall(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            return
        if method != "GET":
            self.emit("http.request", method=method, path=path, status=405, auth=None)
            sock.sendall(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            return
        auth = self.check_auth("GET", target, hs.get("authorization", ""))
        if auth in ("challenge", "refused"):
            self.emit("http.request", method=method, path=path, status=401, auth=auth)
            sock.sendall(("HTTP/1.1 401 Unauthorized\r\n%s\r\nContent-Length: 0\r\nConnection: close\r\n\r\n" % self.challenge_header()).encode())
            return
        if path == self.snapshot_path:
            data = self._pictures[self.picture_now()[0]].data
            self.emit("http.request", method=method, path=path, status=200, auth=auth)
            sock.sendall(("HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: %d\r\nConnection: close\r\n\r\n" % len(data)).encode() + data)
            return
        self.emit("http.request", method=method, path=path, status=200, auth=auth)
        self._http_stream(sock)

    def _http_stream(self, sock):
        sock.sendall(("HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=%s\r\nCache-Control: no-cache\r\n"
                      "Connection: close\r\n\r\n" % BOUNDARY).encode())
        self.emit("http.stream.start", path=self.http_path)
        frames = motion_frames = 0
        try:
            while not self._stop.is_set():
                name, motion = self.picture_now(frames)
                data = self._pictures[name].data
                sock.sendall(("--%s\r\nContent-Type: image/jpeg\r\nContent-Length: %d\r\n\r\n" % (BOUNDARY, len(data))).encode() + data + b"\r\n")
                frames += 1
                motion_frames += motion
                if self._stop.wait(1.0 / self.fps):
                    break
        finally:
            self.emit("http.stream.end", path=self.http_path, frames=frames, motion_frames=motion_frames)

    def picture_now(self, frame=None):
        return picture_at(self.windows, self.fps, self.now(), frame)

    # -- RTSP --

    def _rtsp(self, sock):
        rd = _Reader(sock)
        st = {"sdp": False, "setup": False, "playing": False, "session": None}
        while not self._stop.is_set():
            action = self._rtsp_request(sock, rd, rd.read_head(), st)
            if action == "play":
                self._rtsp_play(sock, rd, st)
                return
            if action == "close":
                return

    def _rtsp_request(self, sock, rd, text, st):
        """Answer one request. Returns 'play' (start the pictures), 'close' (TEARDOWN) or None."""
        first, hs = parse_head(text)
        parts = first.split()
        cseq = hs.get("cseq", "0")
        if len(parts) != 3 or not parts[2].startswith("RTSP/"):
            self._rtsp_reply(sock, 400, "Bad Request", cseq)
            return "close"
        method, uri = parts[0], parts[1]
        clen = hs.get("content-length", "0")
        if not clen.isdigit() or int(clen) > MAX_HEAD:
            self._rtsp_reply(sock, 400, "Bad Request", cseq)
            return "close"
        rd.discard_body(int(clen))
        auth = None
        headers = []
        body = b""
        action = None
        if method == "OPTIONS" or method == "GET_PARAMETER" or method == "SET_PARAMETER":
            code, reason = 200, "OK"
            headers = ["Public: OPTIONS, DESCRIBE, SETUP, PLAY, GET_PARAMETER, TEARDOWN"]
        elif method == "TEARDOWN":
            code, reason, action = 200, "OK", "close"
        elif method in ("DESCRIBE", "SETUP", "PLAY"):
            path = urlsplit(uri).path
            if method == "DESCRIBE" and not path.startswith(self.rtsp_path):
                code, reason = 404, "Not Found"
            else:
                auth = self.check_auth(method, uri, hs.get("authorization", ""))
            if auth in ("challenge", "refused"):
                code, reason, headers = 401, "Unauthorized", [self.challenge_header()]
            elif auth is None:
                code, reason = 404, "Not Found"
            elif method == "DESCRIBE":
                code, reason = 200, "OK"
                body = self._sdp().encode()
                headers = ["Content-Base: rtsp://%s:%d%s/" % (self.host, self.rtsp_port, self.rtsp_path), "Content-Type: application/sdp"]
                st["sdp"] = True
            elif method == "SETUP":
                transport = hs.get("transport", "")
                if not st["sdp"] or st["playing"] or not path.endswith("/track1"):
                    code, reason = 455, "Method Not Valid in This State"
                elif "TCP" not in transport.upper():
                    code, reason = 461, "Unsupported Transport"
                else:
                    st["setup"] = True
                    st["session"] = st["session"] or secrets.token_hex(4)
                    code, reason = 200, "OK"
                    headers = ["Session: %s;timeout=60" % st["session"], "Transport: RTP/AVP/TCP;unicast;interleaved=0-1"]
            else:  # PLAY
                if not st["setup"] or st["playing"]:
                    code, reason = 455, "Method Not Valid in This State"
                else:
                    code, reason = 200, "OK"
                    headers = ["Session: %s" % st["session"], "Range: npt=0.000-"]
                    action = "play"
        else:
            code, reason = 501, "Not Implemented"
        self._rtsp_reply(sock, code, reason, cseq, headers, body)
        self.emit("rtsp.request", method=method, uri=_scrub(uri), cseq=cseq, status=code, auth=auth)
        return action

    def _sdp(self):
        return ("v=0\r\no=- 0 0 IN IP4 %s\r\ns=fakecam\r\nc=IN IP4 0.0.0.0\r\nt=0 0\r\nm=video 0 RTP/AVP %d\r\na=control:track1\r\n"
                % (self.host, RTP_JPEG_PAYLOAD_TYPE))

    @staticmethod
    def _rtsp_reply(sock, code, reason, cseq, headers=(), body=b""):
        head = "\r\n".join(["RTSP/1.0 %d %s" % (code, reason), "CSeq: %s" % cseq] + list(headers) + ["Content-Length: %d" % len(body)])
        sock.sendall(head.encode() + b"\r\n\r\n" + body)

    def _rtsp_play(self, sock, rd, st):
        st["playing"] = True
        self.emit("rtsp.stream.start", path=self.rtsp_path)
        frames = motion_frames = 0
        seq, ts, ssrc = secrets.randbits(16), secrets.randbits(32), secrets.randbits(32)
        step = int(CLOCK_RATE / self.fps)  # the RTP clock per frame
        interval = 1.0 / self.fps
        due = self._clock()
        reason = "stopped"
        try:
            while not self._stop.is_set():
                now = self._clock()
                if now >= due:
                    name, motion = self.picture_now(frames)
                    pic = self._pictures[name]
                    out = bytearray()
                    for k, (marker, payload) in enumerate(pic.payloads):
                        packet = rtp_header(RTP_JPEG_PAYLOAD_TYPE, marker, seq + k, ts, ssrc) + payload
                        out += b"$\x00" + len(packet).to_bytes(2, "big") + packet
                    sock.sendall(bytes(out))
                    seq += len(pic.payloads)
                    ts += step
                    frames += 1
                    motion_frames += motion
                    due = max(due + interval, now)
                    continue
                readable, _, _ = select.select([sock], [], [], due - now)
                if not readable:
                    continue
                data = sock.recv(4096)
                if not data:
                    reason = "client closed"
                    return
                rd.buf += data
                if self._drain_requests(sock, rd, st):
                    reason = "teardown"
                    return
        finally:
            self.emit("rtsp.stream.end", path=self.rtsp_path, frames=frames, motion_frames=motion_frames, reason=reason)

    def _drain_requests(self, sock, rd, st):
        """Requests and interleaved packets the client sent while pictures run. True when the client said TEARDOWN."""
        while True:
            if rd.buf[:1] == b"$":
                if len(rd.buf) < 4 or len(rd.buf) < 4 + int.from_bytes(rd.buf[2:4], "big"):
                    return False
                del rd.buf[:4 + int.from_bytes(rd.buf[2:4], "big")]
                continue
            text = rd.take_head()
            if text is None:
                return False
            if self._rtsp_request(sock, rd, text, st) == "close":
                return True


# ---- command line ---------------------------------------------------------------------------------------------------------------

def main(argv=None):
    ap = argparse.ArgumentParser(description="IYAKKI fake network camera for the automated camera tests (host side).")
    ap.add_argument("--host", default="127.0.0.1", help="address to listen on (loopback by default; QEMU user networking reaches it there)")
    ap.add_argument("--http-port", type=int, default=0, help="HTTP port (0: pick one; the ready line says which)")
    ap.add_argument("--rtsp-port", type=int, default=0, help="RTSP port (0: pick one)")
    ap.add_argument("--no-http", action="store_true", help="do not serve HTTP")
    ap.add_argument("--no-rtsp", action="store_true", help="do not serve RTSP")
    ap.add_argument("--user", help="the user name the client must give (with --password-env)")
    ap.add_argument("--password-env", default="FAKECAM_PASSWORD", help="environment variable that holds the password (never on the command line)")
    ap.add_argument("--auth", choices=["basic", "digest"], default="digest")
    ap.add_argument("--schedule", default="none", help="e.g. 'motion at 5 seconds for 10 seconds'; entries separated by ';'")
    ap.add_argument("--fps", type=float, default=5.0)
    ap.add_argument("--rtsp-path", default="/live")
    ap.add_argument("--http-path", default="/video.mjpg")
    ap.add_argument("--snapshot-path", default="/snapshot.jpg")
    ap.add_argument("--log", help="JSON-lines event log file")
    args = ap.parse_args(argv)
    password = None
    if args.user is not None:
        password = os.environ.get(args.password_env)
        if password is None:
            ap.error("set the password in %s" % args.password_env)
    try:
        cam = FakeCamera(user=args.user, password=password, auth=args.auth, schedule=args.schedule, fps=args.fps, log_path=args.log,
                         rtsp_path=args.rtsp_path, http_path=args.http_path, snapshot_path=args.snapshot_path)
    except (ScheduleError, ValueError) as e:
        ap.error(str(e))
    signal.signal(signal.SIGTERM, lambda *_: cam.request_stop())
    signal.signal(signal.SIGINT, lambda *_: cam.request_stop())
    cam.start(host=args.host, http=not args.no_http, rtsp=not args.no_rtsp, http_port=args.http_port, rtsp_port=args.rtsp_port)
    print("fakecam ready http=%s rtsp=%s" % (cam.http_port or 0, cam.rtsp_port or 0), flush=True)
    while not cam._stop.wait(0.5):
        pass
    cam.stop()
    return 0


if __name__ == "__main__":
    sys.exit(main())
