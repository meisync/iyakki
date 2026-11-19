#!/usr/bin/env python3
"""Writes small AES-256 protected PDFs (revision 5 and 6) for the reader tests, from ISO 32000-2 as the Python `cryptography`
package implements AES and the standard library implements SHA-2. Used once to make the files next to it; the second reader
(poppler's pdftotext) is what proves the files are right, not this script.
usage: make-aes256.py R USERPW OWNERPW TEXT OUT"""
import hashlib, os, struct, sys
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes


def cbc(key, iv, data, enc=True):
    c = Cipher(algorithms.AES(key), modes.CBC(iv))
    x = c.encryptor() if enc else c.decryptor()
    return x.update(data) + x.finalize()


def h(r, pw, salt, udata=b""):
    k = hashlib.sha256(pw + salt + udata).digest()
    if r == 5:
        return k
    i = 0
    while True:
        k1 = (pw + k + udata) * 64
        e = cbc(k[:16], k[16:32], k1)
        m = sum(e[:16]) % 3
        k = [hashlib.sha256, hashlib.sha384, hashlib.sha512][m](e).digest()
        i += 1
        if i >= 64 and e[-1] <= i - 32:
            return k[:32]


def main():
    r, upw, opw, text, out = int(sys.argv[1]), sys.argv[2].encode(), sys.argv[3].encode(), sys.argv[4], sys.argv[5]
    key = os.urandom(32)
    vs, ks = os.urandom(8), os.urandom(8)
    u = h(r, upw, vs) + vs + ks
    ue = cbc(h(r, upw, ks), bytes(16), key)
    ovs, oks = os.urandom(8), os.urandom(8)
    o = h(r, opw, ovs, u) + ovs + oks
    oe = cbc(h(r, opw, oks, u), bytes(16), key)
    p = -4
    perms = struct.pack("<i", p) + b"\xff\xff\xff\xff" + b"TadbXXXX"
    pe = Cipher(algorithms.AES(key), modes.ECB()).encryptor()
    perms = pe.update(perms) + pe.finalize()

    def enc_stream(data):
        iv = os.urandom(16)
        pad = 16 - len(data) % 16
        return iv + cbc(key, iv, data + bytes([pad]) * pad)

    content = enc_stream(f"BT /F1 12 Tf 72 700 Td ({text}) Tj ET".encode())
    hx = lambda b: "<" + b.hex() + ">"
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        b"<< /Length %d >>\nstream\n" % len(content) + content + b"\nendstream",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        (f"<< /Filter /Standard /V 5 /R {r} /Length 256 /P {p} /CF << /StdCF << /AuthEvent /DocOpen /CFM /AESV3 /Length 32 >> >> "
         f"/StmF /StdCF /StrF /StdCF /O {hx(o)} /U {hx(u)} /OE {hx(oe)} /UE {hx(ue)} /Perms {hx(perms)} >>").encode(),
    ]
    b = bytearray(b"%PDF-2.0\n")
    offs = []
    for i, ob in enumerate(objs, 1):
        offs.append(len(b))
        b += b"%d 0 obj\n" % i + ob + b"\nendobj\n"
    x = len(b)
    b += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1)
    for o_ in offs:
        b += b"%010d 00000 n \n" % o_
    fid = os.urandom(16).hex()
    b += f"trailer\n<< /Size {len(objs)+1} /Root 1 0 R /Encrypt 6 0 R /ID [<{fid}> <{fid}>] >>\nstartxref\n{x}\n%%EOF\n".encode()
    open(out, "wb").write(b)


main()
