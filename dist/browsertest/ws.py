# SPDX-License-Identifier: Apache-2.0
# Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).
"""A minimal WebSocket client (RFC 6455, client side) written for the in-house browser test harness.

Only what the Chrome DevTools protocol needs: the opening handshake, text and binary messages in either direction, fragmentation on
receive, masked frames on send, ping/pong and close. Standard library only. The frame functions are pure so they are tested without a
network (test_ws.py).
"""
import base64
import hashlib
import os
import socket
import struct

GUID = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
OP_CONT, OP_TEXT, OP_BINARY, OP_CLOSE, OP_PING, OP_PONG = 0x0, 0x1, 0x2, 0x8, 0x9, 0xA


class WsError(Exception):
    pass


def accept_key(key):
    """The Sec-WebSocket-Accept value for a Sec-WebSocket-Key (both text)."""
    return base64.b64encode(hashlib.sha1(key.encode("ascii") + GUID).digest()).decode("ascii")


def encode_frame(opcode, payload, mask=True, fin=True, mask_key=None):
    """One frame. A client must mask (mask=True); a server frame is unmasked (used by the tests' fake server)."""
    b0 = (0x80 if fin else 0) | opcode
    n = len(payload)
    if n < 126:
        head = struct.pack("!BB", b0, (0x80 if mask else 0) | n)
    elif n < 65536:
        head = struct.pack("!BBH", b0, (0x80 if mask else 0) | 126, n)
    else:
        head = struct.pack("!BBQ", b0, (0x80 if mask else 0) | 127, n)
    if not mask:
        return head + payload
    key = mask_key if mask_key is not None else os.urandom(4)
    return head + key + apply_mask(payload, key)


def apply_mask(data, key):
    if not data:
        return b""
    # XOR with the repeating 4-byte key, done on whole integers (fast enough for screenshots of a few MB)
    n = len(data)
    k = (key * (n // 4 + 1))[:n]
    return (int.from_bytes(data, "big") ^ int.from_bytes(k, "big")).to_bytes(n, "big")


def decode_frame(buf, max_len=256 * 1024 * 1024):
    """Parse one frame from the start of `buf` (bytes). Returns (fin, opcode, payload, used) or None when more bytes are needed."""
    if len(buf) < 2:
        return None
    b0, b1 = buf[0], buf[1]
    if b0 & 0x70:
        raise WsError("reserved bits set")
    fin, opcode, masked, n, pos = bool(b0 & 0x80), b0 & 0x0F, bool(b1 & 0x80), b1 & 0x7F, 2
    if n == 126:
        if len(buf) < 4:
            return None
        n, pos = struct.unpack("!H", buf[2:4])[0], 4
    elif n == 127:
        if len(buf) < 10:
            return None
        n, pos = struct.unpack("!Q", buf[2:10])[0], 10
    if n > max_len:
        raise WsError("frame too large")
    if opcode >= 0x8 and (not fin or n > 125):
        raise WsError("bad control frame")
    key = b""
    if masked:
        if len(buf) < pos + 4:
            return None
        key, pos = buf[pos:pos + 4], pos + 4
    if len(buf) < pos + n:
        return None
    payload = buf[pos:pos + n]
    if masked:
        payload = apply_mask(payload, key)
    return fin, opcode, payload, pos + n


class WebSocket:
    mask_out = True  # a client masks every frame it sends; the server side turns this off

    def __init__(self, sock):
        self.sock = sock
        self.buf = b""
        self.closed = False
        self._frag = None  # (opcode, [parts])

    @classmethod
    def connect(cls, host, port, path, timeout=10.0):
        s = socket.create_connection((host, port), timeout=timeout)
        key = base64.b64encode(os.urandom(16)).decode("ascii")
        req = ("GET %s HTTP/1.1\r\nHost: %s:%d\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
               "Sec-WebSocket-Key: %s\r\nSec-WebSocket-Version: 13\r\n\r\n") % (path, host, port, key)
        s.sendall(req.encode("ascii"))
        data = b""
        while b"\r\n\r\n" not in data:
            chunk = s.recv(4096)
            if not chunk:
                s.close()
                raise WsError("closed during the handshake")
            data += chunk
            if len(data) > 65536:
                s.close()
                raise WsError("handshake answer too long")
        head, rest = data.split(b"\r\n\r\n", 1)
        try:
            check_handshake_answer(head.decode("latin-1"), key)
        except WsError:
            s.close()
            raise
        ws = cls(s)
        ws.buf = rest
        s.settimeout(None)
        return ws

    def send(self, data):
        if isinstance(data, str):
            self.sock.sendall(encode_frame(OP_TEXT, data.encode("utf-8")))
        else:
            self.sock.sendall(encode_frame(OP_BINARY, bytes(data)))

    def recv(self, timeout=None):
        """The next whole text (str) or binary (bytes) message; None on timeout; raises WsError when the peer closed."""
        while True:
            f = decode_frame(self.buf)
            if f is None:
                self.sock.settimeout(timeout)
                try:
                    chunk = self.sock.recv(1 << 20)
                except socket.timeout:
                    return None
                except ConnectionError as e:  # a reset is the peer going away: the same as a close
                    self.closed = True
                    raise WsError("connection lost: %s" % e)
                if not chunk:
                    self.closed = True
                    raise WsError("connection closed")
                self.buf += chunk
                continue
            fin, op, payload, used = f
            self.buf = self.buf[used:]
            if op == OP_PING:
                self.sock.sendall(encode_frame(OP_PONG, payload, mask=self.mask_out))
            elif op == OP_PONG:
                pass
            elif op == OP_CLOSE:
                try:
                    self.sock.sendall(encode_frame(OP_CLOSE, payload[:2], mask=self.mask_out))
                except OSError:
                    pass
                self.closed = True
                raise WsError("closed by the peer")
            elif op in (OP_TEXT, OP_BINARY):
                if self._frag is not None:
                    raise WsError("new message inside a fragmented one")
                if fin:
                    return payload.decode("utf-8") if op == OP_TEXT else payload
                self._frag = (op, [payload])
            elif op == OP_CONT:
                if self._frag is None:
                    raise WsError("continuation without a start")
                self._frag[1].append(payload)
                if fin:
                    o, parts = self._frag
                    self._frag = None
                    whole = b"".join(parts)
                    return whole.decode("utf-8") if o == OP_TEXT else whole
            else:
                raise WsError("unknown opcode %d" % op)

    def close(self):
        if not self.closed:
            try:
                self.sock.sendall(encode_frame(OP_CLOSE, struct.pack("!H", 1000)))
            except OSError:
                pass
        self.closed = True
        try:
            self.sock.close()
        except OSError:
            pass


def check_handshake_answer(head, key):
    lines = head.split("\r\n")
    parts = lines[0].split(" ", 2)
    if len(parts) < 2 or parts[1] != "101":
        raise WsError("the server did not switch protocols: " + lines[0])
    hdr = {}
    for ln in lines[1:]:
        if ":" in ln:
            k, v = ln.split(":", 1)
            hdr[k.strip().lower()] = v.strip()
    if hdr.get("upgrade", "").lower() != "websocket" or "upgrade" not in hdr.get("connection", "").lower():
        raise WsError("missing Upgrade headers")
    if hdr.get("sec-websocket-accept") != accept_key(key):
        raise WsError("wrong Sec-WebSocket-Accept")


class ServerSocket(WebSocket):
    """The server side of a WebSocket (used by the fake host in tests): frames out are unmasked, frames in must be masked."""
    mask_out = False

    @classmethod
    def accept(cls, sock, request_headers):
        key = None
        for k, v in request_headers.items():
            if k.lower() == "sec-websocket-key":
                key = v
        if not key:
            raise WsError("no Sec-WebSocket-Key")
        sock.sendall(("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: %s\r\n\r\n" % accept_key(key)).encode())
        return cls(sock)

    def send(self, data):
        if isinstance(data, str):
            self.sock.sendall(encode_frame(OP_TEXT, data.encode("utf-8"), mask=False))
        else:
            self.sock.sendall(encode_frame(OP_BINARY, bytes(data), mask=False))

    def close(self):
        if not self.closed:
            try:
                self.sock.sendall(encode_frame(OP_CLOSE, struct.pack("!H", 1000), mask=False))
            except OSError:
                pass
        self.closed = True
        try:
            self.sock.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        try:
            self.sock.close()
        except OSError:
            pass
