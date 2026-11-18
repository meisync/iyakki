//! The emulator's PNG writer against readers that are not ours: Python's standard library (`zlib` and a PNG unfilter written in the test, so it runs
//! wherever there is a `python3`) and, on a Mac, `sips`. Both read the picture the emulator wrote and give back the same pixels. Skipped, with a
//! message, where the tool is missing.

use crate::png;
use crate::sha256::sha256_hex;
use std::process::Command;

fn picture() -> (u32, u32, Vec<u8>) {
    let (w, h) = (97u32, 61u32); // odd sizes: rows that are not a multiple of anything
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for (i, p) in rgba.chunks_mut(4).enumerate() {
        let (x, y) = (i as u32 % w, i as u32 / w);
        p.copy_from_slice(&match (x / 16, y / 16) {
            (0, _) => [(x * 16) as u8, (y * 4) as u8, 40, 255],
            (1, 0) => [200, 30, 30, 255],
            (2, 1) => [(x ^ y) as u8, 255 - y as u8, 90, 255],
            _ => [10, 20, (x + y) as u8, 255],
        });
    }
    (w, h, rgba)
}

const PY: &str = r#"
import sys, zlib, struct, hashlib
d = open(sys.argv[1], 'rb').read()
assert d[:8] == b'\x89PNG\r\n\x1a\n', 'signature'
i, idat, hdr = 8, b'', None
while i < len(d):
    n, t = struct.unpack('>I4s', d[i:i+8]); body = d[i+8:i+8+n]
    assert zlib.crc32(t + body) & 0xffffffff == struct.unpack('>I', d[i+8+n:i+12+n])[0], 'crc of ' + t.decode()
    if t == b'IHDR': hdr = struct.unpack('>IIBBBBB', body)
    if t == b'IDAT': idat += body
    i += 12 + n
w, h, depth, ctype, _, _, interlace = hdr
assert depth == 8 and ctype in (2, 6) and interlace == 0, hdr
bpp = 4 if ctype == 6 else 3
raw = zlib.decompress(idat); stride = w * bpp
out = bytearray(); prev = bytearray(stride); p = 0
for _ in range(h):
    f = raw[p]; line = bytearray(raw[p+1:p+1+stride]); p += 1 + stride
    for x in range(stride):
        a = line[x-bpp] if x >= bpp else 0; b = prev[x]; c = prev[x-bpp] if x >= bpp else 0
        if f == 1: line[x] = (line[x] + a) & 255
        elif f == 2: line[x] = (line[x] + b) & 255
        elif f == 3: line[x] = (line[x] + ((a + b) >> 1)) & 255
        elif f == 4:
            pa, pb, pc = abs(b - c), abs(a - c), abs(a + b - 2*c)
            pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
            line[x] = (line[x] + pr) & 255
    out += line; prev = line
if bpp == 3:
    rgba = bytearray()
    for k in range(0, len(out), 3): rgba += out[k:k+3] + b'\xff'
    out = rgba
print(w, h, hashlib.sha256(bytes(out)).hexdigest())
"#;

#[test]
fn a_picture_the_emulator_wrote_is_read_back_to_the_same_pixels_by_pythons_zlib_and_by_apples_image_reader() {
    let (w, h, rgba) = picture();
    let file = png::encode(w, h, &rgba);
    let dir = std::env::temp_dir().join(format!("iyakki-png-real-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("shot.png");
    std::fs::write(&path, &file).unwrap();
    let mut readers = 0;
    // a reader written in the test over the standard library: header, CRC of every chunk, filters, zlib
    match Command::new("python3").arg("-c").arg(PY).arg(&path).output() {
        Ok(o) if o.status.success() => {
            let say = String::from_utf8_lossy(&o.stdout).trim().to_string();
            assert_eq!(say, format!("{w} {h} {}", sha256_hex(&rgba)), "python read other pixels");
            readers += 1;
        }
        Ok(o) => panic!("python could not read the picture: {}", String::from_utf8_lossy(&o.stderr)),
        Err(_) => eprintln!("skipped: no python3 on this machine"),
    }
    // Apple's own image reader, asked to turn the picture into an uncompressed bitmap
    let bmp = dir.join("shot.bmp");
    if Command::new("sips").args(["-s", "format", "bmp"]).arg(&path).arg("--out").arg(&bmp).output().is_ok_and(|o| o.status.success()) {
        let b = std::fs::read(&bmp).unwrap();
        let off = u32::from_le_bytes(b[10..14].try_into().unwrap()) as usize;
        let (bw, bh) = (u32::from_le_bytes(b[18..22].try_into().unwrap()), i32::from_le_bytes(b[22..26].try_into().unwrap()));
        let bpp = u16::from_le_bytes(b[28..30].try_into().unwrap()) as usize / 8;
        assert_eq!((bw, bh.unsigned_abs()), (w, h));
        let stride = (bw as usize * bpp).div_ceil(4) * 4;
        for (y, x) in [(0u32, 0u32), (5, 40), (30, 70), (60, 96), (h - 1, w - 1)] {
            let row = if bh > 0 { h - 1 - y } else { y } as usize;
            let o = off + row * stride + x as usize * bpp;
            let want = &rgba[((y * w + x) * 4) as usize..][..3];
            assert_eq!([b[o + 2], b[o + 1], b[o]], [want[0], want[1], want[2]], "pixel {x},{y}");
        }
        readers += 1;
    } else {
        eprintln!("skipped: no sips on this machine (it only reads pictures on a Mac)");
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(readers > 0 || Command::new("python3").arg("--version").output().is_err(), "no reader ran although one exists");
}
