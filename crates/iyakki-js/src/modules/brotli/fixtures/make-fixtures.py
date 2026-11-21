#!/usr/bin/env python3
"""Regenerates the Brotli fixtures of the in-house decoder's tests (F-BROX-19). Needs the system's libbrotlienc and libbrotlidec (a build-machine
tool, never shipped): the files are made with the reference encoder and checked with the reference decoder before they are written.
text.txt is 40 KB of this repository's own documents plus a few lines in Hindi and Tamil written for this purpose; blob.bin is 6000 bytes of
a fixed pseudo-random sequence (it does not compress, so the encoder writes it as an uncompressed meta-block)."""
import ctypes, glob, os
os.chdir(os.path.dirname(os.path.abspath(__file__)))
enc = ctypes.CDLL('libbrotlienc.so.1'); dec = ctypes.CDLL('libbrotlidec.so.1')
def compress(data, quality, lgwin, mode=0):
    n = enc.BrotliEncoderMaxCompressedSize(ctypes.c_size_t(len(data))) + 64
    out = ctypes.create_string_buffer(n); size = ctypes.c_size_t(n)
    ok = enc.BrotliEncoderCompress(quality, lgwin, mode, ctypes.c_size_t(len(data)), data, ctypes.byref(size), out)
    assert ok == 1
    return out.raw[:size.value]
def decompress(data, cap):
    out = ctypes.create_string_buffer(cap); size = ctypes.c_size_t(cap)
    assert dec.BrotliDecoderDecompress(ctypes.c_size_t(len(data)), data, ctypes.byref(size), out) == 1
    return out.raw[:size.value]
docs = b''
for f in sorted(glob.glob('../../../../../../docs/*.md')):
    docs += open(f, 'rb').read() + b'\n'
    if len(docs) > 36000: break
text = docs[:36000] + 'नमस्ते दुनिया, यह एक परीक्षण है। वणक्कम् உலகம் — இது ஒரு சோதனை. Grüße, café, naïve façade.\n'.encode() * 20
text = text[:40000]
open('text.txt', 'wb').write(text)
x = 12345; blob = bytearray()
for _ in range(6000):
    x = (x * 1103515245 + 12345) & 0x7fffffff; blob.append((x >> 16) & 255)
open('blob.bin', 'wb').write(bytes(blob))
jobs = [('text.q0', text, 0, 22, 0), ('text.q2', text, 2, 22, 0), ('text.q5', text, 5, 22, 0), ('text.q9', text, 9, 22, 0), ('text.q11', text, 11, 22, 0),
        ('text.q11.w10', text, 11, 10, 0), ('text.q11.w24', text, 11, 24, 0), ('text.q11.font', text, 11, 22, 2), ('text.q4.text', text, 4, 18, 1),
        ('blob.q5', bytes(blob), 5, 22, 0), ('blob.q11', bytes(blob), 11, 16, 0), ('mixed.q9', text[:9000] + bytes(blob)[:3000] + text[9000:20000], 9, 20, 0),
        ('empty.q5', b'', 5, 22, 0)]
for name, data, q, w, m in jobs:
    c = compress(data, q, w, m)
    assert decompress(c, len(data) + 16) == data
    open(name + '.br', 'wb').write(c)
    print(name, len(data), len(c))
