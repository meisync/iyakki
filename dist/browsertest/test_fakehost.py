# SPDX-License-Identifier: Apache-2.0
# Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).
"""Unit test of the server side of the WebSocket against the client side (no browser)."""
import json
import os
import socket
import sys
import threading
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ws  # noqa: E402


class Pair(unittest.TestCase):
    def serve_one(self, script):
        srv = socket.socket()
        srv.bind(("127.0.0.1", 0))
        srv.listen(1)

        def run():
            c, _ = srv.accept()
            data = b""
            while b"\r\n\r\n" not in data:
                data += c.recv(4096)
            hdrs = {}
            for ln in data.decode().split("\r\n")[1:]:
                if ":" in ln:
                    k, v = ln.split(":", 1)
                    hdrs[k.strip()] = v.strip()
            s = ws.ServerSocket.accept(c, hdrs)
            try:
                script(s)
            finally:
                s.close()
                srv.close()

        t = threading.Thread(target=run, daemon=True)
        t.start()
        return srv.getsockname()[1], t

    def test_server_and_client_talk(self):
        got = []

        def script(s):
            got.append(s.recv(5))
            s.send(json.dumps({"ok": 1}))
            s.send(b"\x01\x02")

        port, t = self.serve_one(script)
        c = ws.WebSocket.connect("127.0.0.1", port, "/client/ws")
        c.send("hello")
        self.assertEqual(json.loads(c.recv(5)), {"ok": 1})
        self.assertEqual(c.recv(5), b"\x01\x02")
        with self.assertRaises(ws.WsError):
            c.recv(5)
        t.join(5)
        self.assertEqual(got, ["hello"])
        c.close()

    def test_a_server_never_masks_its_answers_to_ping_and_close(self):
        # a browser refuses masked server frames ("A server must not mask any frames"): the pong and the close echo go out unmasked
        self.assertFalse(ws.ServerSocket.mask_out)
        answers = []

        def script(s):
            try:
                s.recv(5)  # reads the client's ping (answers it) and then the close
            except ws.WsError:
                pass

        port, t = self.serve_one(script)
        raw = socket.create_connection(("127.0.0.1", port))
        raw.sendall(b"GET / HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n")
        buf = b""
        while b"\r\n\r\n" not in buf:
            buf += raw.recv(4096)
        buf = buf.split(b"\r\n\r\n", 1)[1]
        raw.sendall(ws.encode_frame(ws.OP_PING, b"zz"))
        raw.sendall(ws.encode_frame(ws.OP_CLOSE, b"\x03\xe8"))
        raw.settimeout(5)
        while True:
            f = ws.decode_frame(buf)
            if f:
                self.assertEqual(buf[1] & 0x80, 0, "a server frame carries the mask bit")
                answers.append(f)
                buf = buf[f[3]:]
                if len(answers) == 2:
                    break
                continue
            more = raw.recv(4096)
            if not more:
                break
            buf += more
        t.join(5)
        raw.close()
        # decode_frame reports payloads; masking is visible in the second header byte
        self.assertEqual([a[1] for a in answers], [ws.OP_PONG, ws.OP_CLOSE])
        self.assertEqual(answers[0][2], b"zz")




class FakeHostServesEveryClientPageFile(unittest.TestCase):
    def test_every_path_of_the_real_client_page_is_served_by_the_fake_host(self):
        import re
        import fakehost
        root = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
        src = open(os.path.join(root, "crates/iyakki-remote/src/clientpage.rs")).read()
        paths = re.search(r"pub const PATHS: \[&str; \d+\] = \[([^\]]+)\]", src).group(1)
        real = set(re.findall(r'"(/connect[^"]*)"', paths))
        files, _csp, _perm = fakehost.page_files(root)
        self.assertEqual(real - set(files), set(), "the real page asks for files the harness does not serve (404 in the console)")


if __name__ == "__main__":
    unittest.main()
