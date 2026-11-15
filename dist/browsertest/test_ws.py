# SPDX-License-Identifier: Apache-2.0
# Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).
"""Unit tests of the in-house WebSocket client; they need no browser. Run: python3 -m unittest dist/browsertest/test_ws.py"""
import os
import socket
import sys
import threading
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ws  # noqa: E402


class Frames(unittest.TestCase):
    def test_rfc_examples(self):
        # RFC 6455 section 1.3: the accept key of the sample handshake
        self.assertEqual(ws.accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")
        # section 5.7: a single-frame masked "Hello" and an unmasked one
        masked = bytes([0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58])
        self.assertEqual(ws.decode_frame(masked), (True, ws.OP_TEXT, b"Hello", len(masked)))
        self.assertEqual(ws.encode_frame(ws.OP_TEXT, b"Hello", mask=True, mask_key=bytes([0x37, 0xfa, 0x21, 0x3d])), masked)
        self.assertEqual(ws.encode_frame(ws.OP_TEXT, b"Hello", mask=False), bytes([0x81, 0x05]) + b"Hello")

    def test_length_forms_roundtrip(self):
        for n in (0, 1, 125, 126, 127, 65535, 65536, 200000):
            p = bytes((i * 7) & 0xFF for i in range(n))
            for mask in (True, False):
                f = ws.encode_frame(ws.OP_BINARY, p, mask=mask)
                fin, op, got, used = ws.decode_frame(f + b"extra")
                self.assertEqual((fin, op, got, used), (True, ws.OP_BINARY, p, len(f)), n)

    def test_partial_input_asks_for_more(self):
        f = ws.encode_frame(ws.OP_TEXT, b"x" * 300)
        for cut in (0, 1, 2, 3, 5, 100, len(f) - 1):
            self.assertIsNone(ws.decode_frame(f[:cut]), cut)

    def test_bad_frames_are_refused(self):
        with self.assertRaises(ws.WsError):
            ws.decode_frame(bytes([0xF1, 0x00]))  # reserved bits
        with self.assertRaises(ws.WsError):
            ws.decode_frame(bytes([0x09, 0x00]))  # a fragmented control frame
        with self.assertRaises(ws.WsError):
            ws.decode_frame(bytes([0x89, 126, 0, 200]) + b"x" * 200)  # a control frame over 125 bytes
        with self.assertRaises(ws.WsError):
            ws.decode_frame(bytes([0x82, 127]) + (1 << 40).to_bytes(8, "big"), max_len=1 << 20)

    def test_handshake_answer_checks(self):
        key = "dGhlIHNhbXBsZSBub25jZQ=="
        good = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        ws.check_handshake_answer(good, key)
        for bad in (good.replace("101", "200"), good.replace("websocket", "other"), good.replace("xOo=", "xOp="), good.replace("Connection: Upgrade\r\n", "")):
            with self.assertRaises(ws.WsError):
                ws.check_handshake_answer(bad, key)


def fake_server(script):
    """Listen on a loopback port; run script(conn) on the first connection after a correct handshake. Returns (port, thread)."""
    srv = socket.socket()
    srv.bind(("127.0.0.1", 0))
    srv.listen(1)

    def run():
        c, _ = srv.accept()
        data = b""
        while b"\r\n\r\n" not in data:
            data += c.recv(4096)
        key = [ln.split(":", 1)[1].strip() for ln in data.decode().split("\r\n") if ln.lower().startswith("sec-websocket-key")][0]
        c.sendall(("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: %s\r\n\r\n" % ws.accept_key(key)).encode())
        try:
            script(c)
        finally:
            c.close()
            srv.close()

    t = threading.Thread(target=run, daemon=True)
    t.start()
    return srv.getsockname()[1], t


def read_client_frame(c, buf=b""):
    while True:
        f = ws.decode_frame(buf)
        if f:
            return f, buf[f[3]:]
        buf += c.recv(65536)


class OverTheWire(unittest.TestCase):
    def test_echo_ping_fragments_and_close(self):
        seen = {}

        def script(c):
            (fin, op, payload, _), rest = read_client_frame(c)
            seen["masked_text"] = (op, payload)
            c.sendall(ws.encode_frame(ws.OP_PING, b"p", mask=False))
            (_, pop, ppay, _), rest = read_client_frame(c, rest)
            seen["pong"] = (pop, ppay)
            # a text message in three fragments with a ping in between, then a binary one, then close
            c.sendall(ws.encode_frame(ws.OP_TEXT, "heé".encode(), mask=False, fin=False)[:])
            c.sendall(ws.encode_frame(ws.OP_PING, b"", mask=False))
            c.sendall(ws.encode_frame(ws.OP_CONT, b"l", mask=False, fin=False))
            c.sendall(ws.encode_frame(ws.OP_CONT, b"lo", mask=False, fin=True))
            c.sendall(ws.encode_frame(ws.OP_BINARY, bytes(range(256)) * 300, mask=False))
            c.sendall(ws.encode_frame(ws.OP_CLOSE, b"\x03\xe8", mask=False))
            # read what the client still sends (its pongs and its close echo) so that closing does not reset the connection under it
            c.settimeout(2)
            try:
                while c.recv(4096):
                    pass
            except OSError:
                pass

        port, t = fake_server(script)
        w = ws.WebSocket.connect("127.0.0.1", port, "/devtools/x")
        w.send("hi")
        self.assertEqual(w.recv(5), "heéllo")
        self.assertEqual(w.recv(5), bytes(range(256)) * 300)
        with self.assertRaises(ws.WsError):
            w.recv(5)
        t.join(5)
        w.close()
        self.assertEqual(seen["masked_text"], (ws.OP_TEXT, b"hi"))
        self.assertEqual(seen["pong"], (ws.OP_PONG, b"p"))

    def test_a_wrong_accept_is_refused(self):
        srv = socket.socket()
        srv.bind(("127.0.0.1", 0))
        srv.listen(1)

        def run():
            c, _ = srv.accept()
            c.recv(4096)
            c.sendall(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: AAAA\r\n\r\n")
            c.close()

        threading.Thread(target=run, daemon=True).start()
        with self.assertRaises(ws.WsError):
            ws.WebSocket.connect("127.0.0.1", srv.getsockname()[1], "/")
        srv.close()

    def test_recv_times_out_with_none(self):
        port, t = fake_server(lambda c: c.recv(1))
        w = ws.WebSocket.connect("127.0.0.1", port, "/")
        self.assertIsNone(w.recv(0.2))
        w.close()


if __name__ == "__main__":
    unittest.main()
