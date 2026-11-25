"""A baseline JPEG encoder in pure Python (standard library only), for the fake camera's pictures.

It writes 4:2:0 colour JPEG files with the Annex K Huffman tables, the Annex K quantisation tables scaled by a quality, and the
quantisation tables in the file (so they can also be sent in-band by RFC 2435, Q 255). That is enough for the IYAKKI camera client,
which assumes the standard Huffman tables. It is not a general encoder: sizes must be multiples of 16.

Run `python3 dist/autotest/fakecam_jpeg.py` to write the committed pictures under dist/autotest/data/ (they are fixed bytes; the
fake camera reads them, it does not encode at run time).
"""
import math
import os
import sys

DC_LUMA_BITS = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0]
DC_CHROMA_BITS = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0]
DC_VALS = list(range(12))
AC_LUMA_BITS = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7D]
AC_CHROMA_BITS = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77]
AC_LUMA_VALS = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xA1, 0x08,
    0x23, 0x42, 0xB1, 0xC1, 0x15, 0x52, 0xD1, 0xF0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0A, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x25, 0x26, 0x27, 0x28,
    0x29, 0x2A, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59,
    0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
    0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6,
    0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE1, 0xE2,
    0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA,
]
AC_CHROMA_VALS = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71, 0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91,
    0xA1, 0xB1, 0xC1, 0x09, 0x23, 0x33, 0x52, 0xF0, 0x15, 0x62, 0x72, 0xD1, 0x0A, 0x16, 0x24, 0x34, 0xE1, 0x25, 0xF1, 0x17, 0x18, 0x19, 0x1A, 0x26,
    0x27, 0x28, 0x29, 0x2A, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58,
    0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
    0x88, 0x89, 0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4,
    0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA,
    0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA,
]
Q_LUMA = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56, 14, 17, 22, 29, 51, 87, 80, 62,
    18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113, 92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
]
Q_CHROMA = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99, 47, 66, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
]
ZIGZAG = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28,
    35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
]


def scaled_table(base, quality):
    """The quantisation table for a quality 1..99, in zig-zag order (the order the DQT segment and RFC 2435 use)."""
    q = max(1, min(99, quality))
    factor = 5000 // q if q < 50 else 200 - 2 * q
    return [max(1, min(255, (base[ZIGZAG[i]] * factor + 50) // 100)) for i in range(64)]


def _codes(bits, vals):
    """Canonical Huffman codes (JPEG Annex C): symbol -> (code, length)."""
    table, code, k = {}, 0, 0
    for length in range(1, 17):
        for _ in range(bits[length - 1]):
            table[vals[k]] = (code, length)
            code += 1
            k += 1
        code <<= 1
    return table


def _dct_1d(v):
    out = []
    for u in range(8):
        cu = math.sqrt(0.5) if u == 0 else 1.0
        s = sum(v[x] * math.cos((2 * x + 1) * u * math.pi / 16) for x in range(8))
        out.append(0.5 * cu * s)
    return out


def _fdct(block):
    """Forward 8x8 DCT of level-shifted samples (rows, then columns)."""
    rows = [_dct_1d(block[8 * y:8 * y + 8]) for y in range(8)]
    out = [0.0] * 64
    for u in range(8):
        col = _dct_1d([rows[y][u] for y in range(8)])
        for v in range(8):
            out[8 * v + u] = col[v]
    return out


class _Bits:
    def __init__(self):
        self.out = bytearray()
        self.acc = 0
        self.n = 0

    def put(self, value, length):
        for i in range(length - 1, -1, -1):
            self.acc = (self.acc << 1) | ((value >> i) & 1)
            self.n += 1
            if self.n == 8:
                self._byte(self.acc)
                self.acc, self.n = 0, 0

    def _byte(self, b):
        self.out.append(b)
        if b == 0xFF:
            self.out.append(0x00)

    def finish(self):
        if self.n:
            self.put((1 << (8 - self.n)) - 1, 8 - self.n)
        return bytes(self.out)


def _magnitude(v):
    a = abs(v)
    size = a.bit_length()
    return size, (v if v >= 0 else v + (1 << size) - 1)


def _encode_block(bw, coeffs, prev_dc, dc_t, ac_t):
    """coeffs: 64 quantised values in zig-zag order. Returns the new DC predictor."""
    diff = coeffs[0] - prev_dc
    size, bits = _magnitude(diff)
    code, length = dc_t[size]
    bw.put(code, length)
    if size:
        bw.put(bits, size)
    run = 0
    for k in range(1, 64):
        v = coeffs[k]
        if v == 0:
            run += 1
            continue
        while run > 15:
            code, length = ac_t[0xF0]
            bw.put(code, length)
            run -= 16
        size, bits = _magnitude(v)
        code, length = ac_t[(run << 4) | size]
        bw.put(code, length)
        bw.put(bits, size)
        run = 0
    if run:
        code, length = ac_t[0x00]
        bw.put(code, length)
    return coeffs[0]


def _planes(width, height, pixel):
    """Y, Cb, Cr planes (full size for Y, halved for chroma) from pixel(x, y) -> (r, g, b)."""
    y_p, cb_full, cr_full = [0.0] * (width * height), [0.0] * (width * height), [0.0] * (width * height)
    for y in range(height):
        for x in range(width):
            r, g, b = pixel(x, y)
            i = y * width + x
            y_p[i] = 0.299 * r + 0.587 * g + 0.114 * b
            cb_full[i] = -0.168736 * r - 0.331264 * g + 0.5 * b + 128
            cr_full[i] = 0.5 * r - 0.418688 * g - 0.081312 * b + 128
    cw, ch = width // 2, height // 2
    cb, cr = [0.0] * (cw * ch), [0.0] * (cw * ch)
    for y in range(ch):
        for x in range(cw):
            idx = [(2 * y + dy) * width + 2 * x + dx for dy in (0, 1) for dx in (0, 1)]
            cb[y * cw + x] = sum(cb_full[i] for i in idx) / 4
            cr[y * cw + x] = sum(cr_full[i] for i in idx) / 4
    return y_p, cb, cr, cw


def _block(plane, stride, bx, by):
    out = []
    for y in range(8):
        for x in range(8):
            out.append(plane[(by + y) * stride + bx + x] - 128)
    return out


def _quantise(coeffs, table_zz):
    out = [0] * 64
    for i in range(64):
        q = table_zz[i]
        c = coeffs[ZIGZAG[i]]
        out[i] = int(round(c / q))
    return out


def encode(width, height, pixel, quality=50):
    """A baseline JPEG (bytes) of a width x height picture; `pixel(x, y)` gives (r, g, b) 0..255. Width and height: multiples of 16."""
    if width % 16 or height % 16:
        raise ValueError("width and height must be multiples of 16")
    qy = scaled_table(Q_LUMA, quality)
    qc = scaled_table(Q_CHROMA, quality)
    y_p, cb, cr, cw = _planes(width, height, pixel)
    dc_l, ac_l = _codes(DC_LUMA_BITS, DC_VALS), _codes(AC_LUMA_BITS, AC_LUMA_VALS)
    dc_c, ac_c = _codes(DC_CHROMA_BITS, DC_VALS), _codes(AC_CHROMA_BITS, AC_CHROMA_VALS)
    bw = _Bits()
    pred = [0, 0, 0]
    for my in range(height // 16):
        for mx in range(width // 16):
            for (bx, by) in ((0, 0), (8, 0), (0, 8), (8, 8)):
                blk = _block(y_p, width, mx * 16 + bx, my * 16 + by)
                pred[0] = _encode_block(bw, _quantise(_fdct(blk), qy), pred[0], dc_l, ac_l)
            for k, plane in ((1, cb), (2, cr)):
                blk = _block(plane, cw, mx * 8, my * 8)
                pred[k] = _encode_block(bw, _quantise(_fdct(blk), qc), pred[k], dc_c, ac_c)
    scan = bw.finish()

    def seg(marker, body):
        return bytes([0xFF, marker]) + (len(body) + 2).to_bytes(2, "big") + body

    out = bytearray(b"\xff\xd8")
    out += seg(0xDB, bytes([0]) + bytes(qy) + bytes([1]) + bytes(qc))
    out += seg(0xC0, bytes([8]) + height.to_bytes(2, "big") + width.to_bytes(2, "big") + bytes([3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]))
    for cls_id, bits, vals in ((0x00, DC_LUMA_BITS, DC_VALS), (0x10, AC_LUMA_BITS, AC_LUMA_VALS), (0x01, DC_CHROMA_BITS, DC_VALS), (0x11, AC_CHROMA_BITS, AC_CHROMA_VALS)):
        out += seg(0xC4, bytes([cls_id]) + bytes(bits) + bytes(vals))
    out += seg(0xDA, bytes([3, 1, 0x00, 2, 0x11, 3, 0x11, 0, 63, 0]))
    out += scan
    out += b"\xff\xd9"
    return bytes(out)


def _pattern(width, height, block=None):
    """Colour bars over the top two thirds, a grey ramp below, and an optional white-edged black block (x, y, size)."""
    bars = [(255, 255, 255), (255, 255, 0), (0, 255, 255), (0, 255, 0), (255, 0, 255), (255, 0, 0), (0, 0, 255), (0, 0, 0)]
    bar_h = height * 2 // 3

    def pixel(x, y):
        if y < bar_h:
            return bars[min(7, x * 8 // width)]
        g = 255 * (x) // (width - 1)
        return (g, g, g)

    if block is None:
        return pixel

    bx, by, size = block

    def with_block(x, y):
        if bx <= x < bx + size and by <= y < by + size:
            edge = x in (bx, bx + size - 1) or y in (by, by + size - 1)
            return (255, 255, 255) if edge else (0, 0, 0)
        return pixel(x, y)

    return with_block


# the three committed pictures: still (no block), and the block at the left and at the right position (motion alternates them)
PICTURES = {
    "fakecam-still.jpg": (None,),
    "fakecam-block-left.jpg": ((16, 96, 24),),
    "fakecam-block-right.jpg": ((136, 96, 24),),
}
WIDTH, HEIGHT = 176, 144


def write_data(directory):
    os.makedirs(directory, exist_ok=True)
    written = []
    for name, (block,) in PICTURES.items():
        data = encode(WIDTH, HEIGHT, _pattern(WIDTH, HEIGHT, block), quality=50)
        path = os.path.join(directory, name)
        with open(path, "wb") as f:
            f.write(data)
        written.append((path, len(data)))
    return written


if __name__ == "__main__":
    here = os.path.dirname(os.path.abspath(__file__))
    target = sys.argv[1] if len(sys.argv) > 1 else os.path.join(here, "data")
    for path, size in write_data(target):
        print("%s %d bytes" % (path, size))
