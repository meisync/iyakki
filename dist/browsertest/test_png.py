# SPDX-License-Identifier: Apache-2.0
# Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).
"""Unit tests of the PNG helper (no browser)."""
import os
import struct
import sys
import unittest
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import png  # noqa: E402


class Png(unittest.TestCase):
    def test_roundtrip_and_compare(self):
        w, h = 7, 5
        px = bytes((x * 31 + y * 7 + k * 50) & 255 for y in range(h) for x in range(w) for k in range(4))
        self.assertEqual(png.decode(png.encode(w, h, px)), (w, h, px))
        other = bytearray(px)
        other[4 * (2 * w + 3) + 1] ^= 9
        self.assertEqual(png.compare(px, bytes(other), w, h), (9, 1))
        self.assertEqual(png.compare(px, px, w, h), (0, 0))

    def test_every_filter_decodes(self):
        # hand-made 3x4 RGB picture filtered with each of the five filters, decoded back
        w, h, bpp = 3, 4, 3
        pic = [[(x * 40 + y * 9 + k * 20) & 255 for x in range(w) for k in range(bpp)] for y in range(h)]
        raw = b""
        prev = [0] * (w * bpp)
        for y, line in enumerate(pic):
            f = y if y < 5 else 0
            out = []
            for i, v in enumerate(line):
                a = line[i - bpp] if i >= bpp else 0
                b = prev[i]
                c = prev[i - bpp] if i >= bpp else 0
                if f == 0:
                    p = 0
                elif f == 1:
                    p = a
                elif f == 2:
                    p = b
                elif f == 3:
                    p = (a + b) >> 1
                else:
                    pa, pb, pc = abs(b - c), abs(a - c), abs(a + b - 2 * c)
                    p = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                out.append((v - p) & 255)
            raw += bytes([f]) + bytes(out)
            prev = line

        def chunk(t, d):
            return struct.pack("!I", len(d)) + t + d + struct.pack("!I", zlib.crc32(t + d) & 0xFFFFFFFF)

        data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack("!IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")
        gw, gh, rgba = png.decode(data)
        self.assertEqual((gw, gh), (w, h))
        for y in range(h):
            for x in range(w):
                self.assertEqual(list(rgba[(y * w + x) * 4:(y * w + x) * 4 + 3]), pic[y][x * 3:x * 3 + 3], (x, y))

    def test_rejects_garbage(self):
        with self.assertRaises(ValueError):
            png.decode(b"GIF89a....")


if __name__ == "__main__":
    unittest.main()
