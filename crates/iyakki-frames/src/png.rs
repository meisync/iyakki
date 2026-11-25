//! A minimal PNG writer (RGBA, 8 bits, no interlace) for screenshots. The zlib stream comes from `miniz_oxide`.

fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    for (n, slot) in t.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *slot = c;
    }
    t
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8], table: &[u32; 256]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut crc = 0xffff_ffffu32;
    for b in kind.iter().chain(data.iter()) {
        crc = table[((crc ^ *b as u32) & 0xff) as usize] ^ (crc >> 8);
    }
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&(crc ^ 0xffff_ffff).to_be_bytes());
}

/// `rgba` holds `w * h * 4` bytes (shorter input is padded with white).
pub fn encode_rgba(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let table = crc_table();
    let stride = w as usize * 4;
    let mut raw = Vec::with_capacity((stride + 1) * h as usize);
    for y in 0..h as usize {
        raw.push(0); // filter: none
        let a = y * stride;
        let row = rgba.get(a..(a + stride).min(rgba.len())).unwrap_or(&[]);
        raw.extend_from_slice(row);
        raw.extend(std::iter::repeat_n(255u8, stride - row.len()));
    }
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 3);
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr, &table);
    chunk(&mut out, b"IDAT", &z, &table);
    chunk(&mut out, b"IEND", &[], &table);
    out
}
