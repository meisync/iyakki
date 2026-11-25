"""fakecam: the fake network camera (dist/autotest/fakecam.py) speaks HTTP and RTSP on a loopback ephemeral port, and its pictures are
valid baseline JPEGs with the block where the schedule says. Every server is stopped in tearDown; nothing waits on a sleep (events and
blocking reads with a socket timeout only)."""
import base64
import hashlib
import os
import socket
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
import fakecam as F  # noqa: E402
import fakecam_jpeg as E  # noqa: E402

TIMEOUT = 10.0


# ---- an independent JPEG validator: walks the markers, decodes the Huffman scan and reads back the block means -------------------

def _extend(bits, size):
    return bits if bits >= 1 << (size - 1) else bits - (1 << size) + 1


def _huffman_map(bits, vals):
    """(length, code) -> symbol, from the DHT counts and values."""
    out, code, k = {}, 0, 0
    for length in range(1, 17):
        for _ in range(bits[length - 1]):
            out[(length, code)] = vals[k]
            code += 1
            k += 1
        code <<= 1
    return out


def validate_jpeg(data):
    """Parse a baseline 4:2:0 JPEG as a decoder does: segments, tables (the standard Huffman tables), the frame header, and the whole
    entropy-coded scan (every block decodes, the padding is ones, EOI follows). Returns the picture size, the MCU count, the quantised
    DC value of each luma block (in decoding order) and the luma quantisation table."""
    if data[:2] != b"\xff\xd8":
        raise AssertionError("no SOI")
    if data[-2:] != b"\xff\xd9":
        raise AssertionError("no EOI at the end")
    i, dqt, dht, frame = 2, {}, {}, None
    scan_at = None
    while scan_at is None:
        assert data[i] == 0xFF, "marker expected at %d" % i
        marker = data[i + 1]
        length = int.from_bytes(data[i + 2:i + 4], "big")
        seg = data[i + 4:i + 2 + length]
        if marker == 0xDB:
            j = 0
            while j < len(seg):
                assert seg[j] >> 4 == 0, "8-bit tables only"
                dqt[seg[j] & 15] = list(seg[j + 1:j + 65])
                j += 65
        elif marker == 0xC4:
            j = 0
            while j < len(seg):
                cls_id = seg[j]
                bits = list(seg[j + 1:j + 17])
                n = sum(bits)
                vals = list(seg[j + 17:j + 17 + n])
                dht[cls_id] = _huffman_map(bits, vals)
                j += 17 + n
        elif marker == 0xC0:
            assert seg[0] == 8, "8-bit samples"
            height, width = int.from_bytes(seg[1:3], "big"), int.from_bytes(seg[3:5], "big")
            assert seg[5] == 3, "three components"
            assert [seg[7], seg[10], seg[13]] == [0x22, 0x11, 0x11], "4:2:0 sampling"
            frame = (width, height)
        elif marker == 0xDA:
            assert seg[0] == 3 and [seg[1], seg[2], seg[3], seg[4], seg[5], seg[6]] == [1, 0x00, 2, 0x11, 3, 0x11], "scan selectors"
            scan_at = i + 2 + length
        else:
            assert marker not in (0xC1, 0xC2), "only baseline"
        i += 2 + length
    assert frame is not None and set(dqt) == {0, 1} and set(dht) == {0x00, 0x01, 0x10, 0x11}, "frame header and tables"
    width, height = frame
    assert width % 16 == 0 and height % 16 == 0
    scan = data[scan_at:-2]
    # bit reader with byte unstuffing
    pos, acc, nbits = 0, 0, 0

    def bit():
        nonlocal pos, acc, nbits
        if nbits == 0:
            if pos >= len(scan):
                raise AssertionError("the scan ends early")
            b = scan[pos]
            pos += 1
            if b == 0xFF:
                nxt = scan[pos] if pos < len(scan) else None
                assert nxt == 0x00, "a marker inside the scan"
                pos += 1
            acc, nbits = b, 8
        nbits -= 1
        return (acc >> nbits) & 1

    def bits(n):
        v = 0
        for _ in range(n):
            v = (v << 1) | bit()
        return v

    def symbol(table):
        code, length = 0, 0
        while True:
            code = (code << 1) | bit()
            length += 1
            assert length <= 16, "no code of up to 16 bits"
            if (length, code) in table:
                return table[(length, code)]

    mcus = (width // 16) * (height // 16)
    pred = [0, 0, 0]
    luma_dc = []
    for _ in range(mcus):
        for comp in range(6):
            c = 0 if comp < 4 else comp - 3
            tbl = 0 if c == 0 else 1
            s = symbol(dht[tbl])
            diff = _extend(bits(s), s) if s else 0
            pred[c] += diff
            if c == 0:
                luma_dc.append(pred[0])
            k = 1
            while k < 64:
                rs = symbol(dht[0x10 + tbl])
                r, size = rs >> 4, rs & 15
                if size == 0:
                    if r == 15:
                        k += 16
                        continue
                    break
                k += r
                assert k < 64, "a run past the end of the block"
                bits(size)
                k += 1
    # the rest is padding of ones, then EOI
    assert (nbits == 0 or (acc & ((1 << nbits) - 1)) == (1 << nbits) - 1), "padding must be ones"
    assert pos == len(scan), "bytes left after the scan"
    return {"width": width, "height": height, "mcus": mcus, "luma_dc": luma_dc, "q_luma": dqt[0]}


def luma_mean(summary, x, y):
    """Mean luma of the 8x8 block covering pixel (x, y), from its DC term (the mean is DC / 8 + 128 once dequantised)."""
    per_row = summary["width"] // 16
    mcu = (y // 16) * per_row + x // 16
    sub = (1 if x % 16 >= 8 else 0) + (2 if y % 16 >= 8 else 0)
    dc = summary["luma_dc"][mcu * 4 + sub]
    return dc * summary["q_luma"][0] / 8 + 128


def read_exact(sock, n):
    out = b""
    while len(out) < n:
        chunk = sock.recv(n - len(out))
        if not chunk:
            raise AssertionError("connection closed after %d of %d bytes" % (len(out), n))
        out += chunk
    return out


def read_line_head(sock):
    buf = b""
    while not buf.endswith(b"\r\n\r\n"):
        ch = sock.recv(1)
        if not ch:
            raise AssertionError("closed in a head")
        buf += ch
    return buf.decode("latin-1")


def digest_header(user, password, method, uri, nonce, realm=F.REALM):
    ha1 = hashlib.md5(("%s:%s:%s" % (user, realm, password)).encode()).hexdigest()
    ha2 = hashlib.md5(("%s:%s" % (method, uri)).encode()).hexdigest()
    nc, cnonce = "00000001", "0a1b2c3d"
    resp = hashlib.md5(("%s:%s:%s:%s:auth:%s" % (ha1, nonce, nc, cnonce, ha2)).encode()).hexdigest()
    return ('Digest username="%s", realm="%s", nonce="%s", uri="%s", qop=auth, nc=%s, cnonce="%s", response="%s"'
            % (user, realm, nonce, uri, nc, cnonce, resp))


def read_frame(client):
    """The scan of the next frame and how many packets it took (reassembled as RFC 2435 says; offsets checked)."""
    scan, expected, packets = b"", 0, 0
    while True:
        ch, pkt = client.packet()
        assert ch == 0 and pkt[0] == 0x80 and pkt[1] & 0x7F == 26, "channel 0, RTP version 2, payload type 26"
        payload = pkt[12:]
        assert int.from_bytes(payload[1:4], "big") == expected, "fragment offset"
        body = payload[8:]
        if payload[1:4] == b"\x00\x00\x00":
            assert payload[4:8] == bytes([1, 255, 22, 18]), "type 1, Q 255, 176 x 144"
            body = body[132:]
        scan += body
        expected += len(body)
        packets += 1
        if pkt[1] & 0x80:
            return scan, packets


class Rtsp:
    """A minimal RTSP client: one request at a time, interleaved packets skipped."""

    def __init__(self, port):
        self.sock = socket.create_connection(("127.0.0.1", port), timeout=TIMEOUT)
        self.cseq = 0

    def close(self):
        self.sock.close()

    def request(self, method, uri, headers=()):
        self.cseq += 1
        head = "%s %s RTSP/1.0\r\nCSeq: %d\r\n%s\r\n" % (method, uri, self.cseq, "".join("%s\r\n" % h for h in headers))
        self.sock.sendall(head.encode())
        return self.response()

    def response(self):
        while True:
            lead = read_exact(self.sock, 1)
            if lead == b"$":
                ch = read_exact(self.sock, 1)
                n = int.from_bytes(read_exact(self.sock, 2), "big")
                read_exact(self.sock, n)
                continue
            head = lead.decode("latin-1") + read_line_head(self.sock)
            break
        lines = head.split("\r\n")
        code = int(lines[0].split()[1])
        headers = {}
        for line in lines[1:]:
            if ":" in line:
                k, v = line.split(":", 1)
                headers[k.strip().lower()] = v.strip()
        body = read_exact(self.sock, int(headers.get("content-length", "0")))
        return code, headers, body

    def packet(self):
        """The next interleaved packet: (channel, payload)."""
        lead = read_exact(self.sock, 1)
        assert lead == b"$", "an interleaved packet expected"
        ch = read_exact(self.sock, 1)[0]
        n = int.from_bytes(read_exact(self.sock, 2), "big")
        return ch, read_exact(self.sock, n)


class CameraTestCase(unittest.TestCase):
    def start(self, **kw):
        self.cam = F.FakeCamera(**kw).start(http_port=0, rtsp_port=0)
        self.addCleanup(self.cam.stop)
        return self.cam

    def http_get(self, path, headers=()):
        s = socket.create_connection(("127.0.0.1", self.cam.http_port), timeout=TIMEOUT)
        self.addCleanup(s.close)
        s.sendall(("GET %s HTTP/1.1\r\nHost: 127.0.0.1\r\n%s\r\n" % (path, "".join("%s\r\n" % h for h in headers))).encode())
        return s

    def rtsp(self):
        c = Rtsp(self.cam.rtsp_port)
        self.addCleanup(c.close)
        return c


class ScheduleTests(unittest.TestCase):
    def test_motion_windows_are_read_and_checked(self):
        self.assertEqual(F.parse_schedule("motion at 5 seconds for 10 seconds"), [(5.0, 15.0)])
        self.assertEqual(F.parse_schedule("motion at 1 for 2; motion at 20.5 for 1"), [(1.0, 3.0), (20.5, 21.5)])
        self.assertEqual(F.parse_schedule("none"), [])
        self.assertEqual(F.parse_schedule(""), [])
        for bad in ("motion at x for 2", "motion for 2", "motion at 3 for 0", "rain at 3 for 2"):
            with self.assertRaises(F.ScheduleError, msg=bad):
                F.parse_schedule(bad)

    def test_the_picture_is_still_outside_the_window_and_the_block_moves_inside(self):
        windows = [(5.0, 15.0)]
        self.assertEqual(F.picture_at(windows, 5, 4.9), ("still", False))
        self.assertEqual(F.picture_at(windows, 5, 15.0), ("still", False))
        names = [F.picture_at(windows, 5, 6.0, frame=n)[0] for n in range(4)]
        self.assertEqual(names, ["left", "right", "left", "right"])
        self.assertTrue(F.picture_at(windows, 5, 6.0, frame=0)[1])


class PictureTests(unittest.TestCase):
    def test_committed_pictures_are_valid_jpegs_with_the_block_where_they_should_be(self):
        pics = F.load_pictures()
        for name, pic in pics.items():
            self.assertEqual(pic.data[:2], b"\xff\xd8", name)
            s = validate_jpeg(pic.data)
            self.assertEqual((s["width"], s["height"]), (176, 144), name)
            self.assertEqual(s["mcus"], 99, name)
        still, left, right = (validate_jpeg(pics[n].data) for n in ("still", "left", "right"))
        self.assertLess(luma_mean(left, 24, 104), 30)       # the block (black) at the left
        self.assertGreater(luma_mean(left, 144, 104), 150)  # and no block at the right
        self.assertLess(luma_mean(right, 144, 104), 30)     # the block at the right
        self.assertGreater(luma_mean(right, 24, 104), 25)
        self.assertGreater(luma_mean(still, 24, 104), 25)   # the still scene: the grey ramp, no block
        self.assertGreater(luma_mean(still, 144, 104), 150)

    def test_the_validator_refuses_a_damaged_scan(self):
        data = F.load_pictures()["still"].data
        cut = data[:-40] + data[-2:]  # the scan loses its last 38 bytes
        with self.assertRaises(AssertionError):
            validate_jpeg(cut)
        with self.assertRaises(AssertionError):
            validate_jpeg(b"\x00\x00" + data[2:])

    def test_the_encoder_output_passes_the_validator_for_another_size(self):
        data = E.encode(32, 16, lambda x, y: (x * 8 % 256, y * 16 % 256, 128), quality=75)
        s = validate_jpeg(data)
        self.assertEqual((s["width"], s["height"], s["mcus"]), (32, 16, 2))

    def test_rfc2435_packets_rebuild_the_scan(self):
        pic = F.load_pictures()["still"]
        w, h, qt, scan = F.split_jpeg(pic.data)
        self.assertEqual((w, h), (176, 144))
        self.assertEqual(len(qt), 128)
        rebuilt = b""
        for k, (marker, payload) in enumerate(pic.payloads):
            off = int.from_bytes(payload[1:4], "big")
            self.assertEqual(off, len(rebuilt))
            self.assertEqual(payload[4:8], bytes([1, 255, 22, 18]))
            body = payload[8:]
            if off == 0:
                self.assertEqual(body[:4], bytes([0, 0, 0, 128]))
                self.assertEqual(body[4:132], qt)
                body = body[132:]
            self.assertLessEqual(len(payload), F.MAX_PAYLOAD)
            rebuilt += body
            self.assertEqual(marker, k == len(pic.payloads) - 1)
        self.assertEqual(rebuilt, scan)


class HttpTests(CameraTestCase):
    def test_a_snapshot_needs_the_password_when_one_is_set_and_refuses_a_wrong_one(self):
        cam = self.start(user="admin", password="pw-not-in-log", auth="basic")
        s = self.http_get("/snapshot.jpg")
        head = read_line_head(s)
        self.assertIn(" 401 ", head.split("\r\n")[0])
        self.assertIn('WWW-Authenticate: Basic realm="fakecam"', head)
        s.close()
        good = "Basic " + base64.b64encode(b"admin:pw-not-in-log").decode()
        s = self.http_get("/snapshot.jpg", ["Authorization: " + good])
        head = read_line_head(s)
        self.assertIn(" 200 ", head.split("\r\n")[0])
        length = int(dict(l.split(": ", 1) for l in head.split("\r\n")[1:] if ": " in l)["Content-Length"])
        data = read_exact(s, length)
        self.assertEqual(validate_jpeg(data)["mcus"], 99)
        s.close()
        bad = "Basic " + base64.b64encode(b"admin:nope").decode()
        s = self.http_get("/snapshot.jpg", ["Authorization: " + bad])
        self.assertIn(" 401 ", read_line_head(s).split("\r\n")[0])
        s.close()
        self.assertTrue(cam.wait_event("http.request", count=3))
        statuses = [(e["status"], e["auth"]) for e in cam.events("http.request")]
        self.assertEqual(statuses, [(401, "challenge"), (200, "ok"), (401, "refused")])

    def test_the_multipart_stream_sends_pictures_with_a_digest_sign_in(self):
        cam = self.start(user="admin", password="pw-digest", auth="digest", fps=200)
        s = self.http_get("/video.mjpg")
        head = read_line_head(s)
        self.assertIn(" 401 ", head.split("\r\n")[0])
        nonce = head.split('nonce="')[1].split('"')[0]
        s.close()
        s = self.http_get("/video.mjpg", ["Authorization: " + digest_header("admin", "pw-digest", "GET", "/video.mjpg", nonce)])
        head = read_line_head(s)
        self.assertIn(" 200 ", head.split("\r\n")[0])
        self.assertIn("multipart/x-mixed-replace; boundary=fakecamframe", head)
        for n in range(2):
            line = b""
            while not line.endswith(b"\r\n") or line.strip() == b"":
                line += read_exact(s, 1)
            self.assertEqual(line.strip(), b"--fakecamframe")
            part_head = read_line_head(s).split("\r\n")
            length = int([h for h in part_head if h.startswith("Content-Length")][0].split(": ")[1])
            data = read_exact(s, length)
            self.assertEqual(read_exact(s, 2), b"\r\n")
            self.assertEqual(validate_jpeg(data)["mcus"], 99)
        s.close()
        self.assertTrue(cam.wait_event("http.stream.end"))
        end = cam.events("http.stream.end")[0]
        self.assertGreaterEqual(end["frames"], 2)
        self.assertNotIn("pw-digest", repr(cam.events()))

    def test_an_unknown_path_is_a_404_and_is_logged(self):
        cam = self.start()
        s = self.http_get("/nothing-here")
        self.assertIn(" 404 ", read_line_head(s).split("\r\n")[0])
        s.close()
        self.assertTrue(cam.wait_event("http.request"))
        self.assertEqual(cam.events("http.request")[0]["status"], 404)


class RtspTests(CameraTestCase):
    def test_options_describe_and_the_sdp_the_client_reads(self):
        cam = self.start()
        c = self.rtsp()
        code, headers, _ = c.request("OPTIONS", "rtsp://127.0.0.1/live")
        self.assertEqual(code, 200)
        code, headers, body = c.request("DESCRIBE", "rtsp://127.0.0.1:%d/live" % cam.rtsp_port, ["Accept: application/sdp"])
        self.assertEqual(code, 200)
        self.assertEqual(headers["content-type"], "application/sdp")
        self.assertTrue(headers["content-base"].startswith("rtsp://127.0.0.1:%d/live" % cam.rtsp_port))
        sdp = body.decode()
        self.assertIn("m=video 0 RTP/AVP 26", sdp)
        self.assertIn("a=control:track1", sdp)

    def test_digest_sign_in_then_setup_play_and_pictures_in_interleaved_packets(self):
        cam = self.start(user="admin", password="pw-rtsp", auth="digest", fps=100)
        c = self.rtsp()
        uri = "rtsp://127.0.0.1:%d/live" % cam.rtsp_port
        code, headers, _ = c.request("DESCRIBE", uri, ["Accept: application/sdp"])
        self.assertEqual(code, 401)
        nonce = headers["www-authenticate"].split('nonce="')[1].split('"')[0]
        auth = ["Authorization: " + digest_header("admin", "pw-rtsp", "DESCRIBE", uri, nonce)]
        code, headers, body = c.request("DESCRIBE", uri, ["Accept: application/sdp"] + auth)
        self.assertEqual(code, 200)
        code, headers, _ = c.request("SETUP", uri + "/track1", ["Transport: RTP/AVP/TCP;unicast;interleaved=0-1"] + [
            "Authorization: " + digest_header("admin", "pw-rtsp", "SETUP", uri + "/track1", nonce)])
        self.assertEqual(code, 200)
        self.assertIn("interleaved=0-1", headers["transport"])
        session = headers["session"].split(";")[0]
        self.assertTrue(session)
        code, headers, _ = c.request("PLAY", uri, ["Session: " + session, "Range: npt=0.000-"] + [
            "Authorization: " + digest_header("admin", "pw-rtsp", "PLAY", uri, nonce)])
        self.assertEqual(code, 200)
        scan, frame_packets = read_frame(c)
        self.assertEqual(scan, F.split_jpeg(F.load_pictures()["still"].data)[3])
        self.assertGreater(frame_packets, 1)
        code, _, _ = c.request("TEARDOWN", uri, ["Session: " + session])
        self.assertEqual(code, 200)
        self.assertTrue(cam.wait_event("rtsp.stream.end"))
        self.assertEqual(cam.events("rtsp.stream.end")[0]["reason"], "teardown")
        self.assertEqual([e["method"] for e in cam.events("rtsp.request")], ["DESCRIBE", "DESCRIBE", "SETUP", "PLAY", "TEARDOWN"])
        self.assertEqual([e["auth"] for e in cam.events("rtsp.request")], ["challenge", "ok", "ok", "ok", None])
        self.assertNotIn("pw-rtsp", repr(cam.events()))

    def test_inside_the_motion_window_the_block_alternates_frame_by_frame(self):
        cam = self.start(schedule="motion at 0 seconds for 100000 seconds", fps=1000)
        c = self.rtsp()
        uri = "rtsp://127.0.0.1:%d/live" % cam.rtsp_port
        c.request("DESCRIBE", uri)
        c.request("SETUP", uri + "/track1", ["Transport: RTP/AVP/TCP;unicast;interleaved=0-1"])
        code, _, _ = c.request("PLAY", uri)
        self.assertEqual(code, 200)
        left, right = [F.split_jpeg(F.load_pictures()[n].data)[3] for n in ("left", "right")]
        frames = [read_frame(c)[0] for _ in range(3)]
        self.assertEqual(frames, [left, right, left])
        self.assertTrue(cam.wait_event("motion.start"))
        code, _, _ = c.request("TEARDOWN", uri)
        self.assertEqual(code, 200)

    def test_setup_over_udp_is_refused_and_the_camera_stays_up(self):
        cam = self.start()
        c = self.rtsp()
        uri = "rtsp://127.0.0.1:%d/live" % cam.rtsp_port
        c.request("DESCRIBE", uri)
        code, _, _ = c.request("SETUP", uri + "/track1", ["Transport: RTP/AVP;unicast;client_port=5000-5001"])
        self.assertEqual(code, 461)
        code, _, _ = c.request("OPTIONS", uri)
        self.assertEqual(code, 200)

    def test_a_wrong_password_is_refused_on_play(self):
        cam = self.start(user="admin", password="right-one", auth="basic")
        c = self.rtsp()
        uri = "rtsp://127.0.0.1:%d/live" % cam.rtsp_port
        bad = "Authorization: Basic " + base64.b64encode(b"admin:wrong-one").decode()
        code, _, _ = c.request("DESCRIBE", uri, [bad])
        self.assertEqual(code, 401)
        self.assertEqual(cam.events("rtsp.request")[-1]["auth"], "refused")


class LifeCycleTests(unittest.TestCase):
    def test_the_event_log_is_json_lines_and_holds_no_password(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "fakecam.jsonl")
            cam = F.FakeCamera(user="admin", password="log-secret", auth="basic", log_path=path, schedule="motion at 0 for 1000")
            cam.start(http_port=0, rtsp_port=0)
            try:
                s = socket.create_connection(("127.0.0.1", cam.http_port), timeout=TIMEOUT)
                s.sendall(b"GET /snapshot.jpg HTTP/1.1\r\nHost: x\r\n\r\n")
                read_line_head(s)
                s.close()
                self.assertTrue(cam.wait_event("http.request"))
                self.assertTrue(cam.wait_event("motion.start"))
            finally:
                cam.stop()
            import json
            with open(path, encoding="utf-8") as f:
                text = f.read()
            lines = [json.loads(x) for x in text.splitlines()]
            self.assertEqual(lines[0]["event"], "start")
            self.assertEqual(lines[-1]["event"], "stop")
            self.assertIn("http.request", [x["event"] for x in lines])
            self.assertNotIn("log-secret", text)
            self.assertNotIn("Authorization", text)

    def test_nothing_is_listening_after_stop(self):
        cam = F.FakeCamera().start(http_port=0, rtsp_port=0)
        ports = (cam.http_port, cam.rtsp_port)
        cam.stop()
        for port in ports:
            with self.assertRaises(OSError):
                socket.create_connection(("127.0.0.1", port), timeout=TIMEOUT).close()

    def test_a_password_needs_a_user_and_the_auth_kind_is_checked(self):
        with self.assertRaises(ValueError):
            F.FakeCamera(user="admin")
        with self.assertRaises(ValueError):
            F.FakeCamera(user="admin", password="x", auth="ntlm")


if __name__ == "__main__":
    unittest.main()
