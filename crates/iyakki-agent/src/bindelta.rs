//! A binary delta, built in-house (docs/development/REMOTE-PATCH-DELTA.md): the difference between an old and a new version of one
//! big file (a relinked program), small enough to send instead of the whole file, and applied on the device.
//!
//! The method is the one bsdiff made known, with a hash index instead of a suffix array so that an 80 MB program needs about 64 MB for the
//! index (not 640 MB) and seconds (not minutes): the new file is walked, runs that also exist in the old file are found through a hash of
//! 16 bytes, extended forward and backward even across a few changed bytes (a relinked program differs in many addresses), and written as
//!
//! * `ctrl`: for each piece, how many bytes to add to the old file's bytes, how many new bytes follow as they are, and how far to move in the
//!   old file (all varints);
//! * `diff`: the byte-wise differences of the first part (almost all zeros when only addresses moved);
//! * `extra`: the new bytes that exist nowhere in the old file.
//!
//! Each of the three streams is compressed with zstd (`ruzstd`, which the agent already uses). Layout: magic, version, lengths, SHA-256 of
//! the old and the new file, the three streams, and a SHA-256 of everything before it. Nothing is trusted before it is checked:
//! [`apply_delta`] checks the trailer, then the old file's length and hash, then every piece's bounds, then the new file's hash.

use std::io::Read;

use sha2::{Digest, Sha256};

pub const MAGIC: &[u8; 8] = b"IYBDELT1";
pub const VERSION: u32 = 1;
/// The largest file a delta is made for or applied to (the patch layer caps its target at the same size).
pub const MAX_FILE: u64 = 512 << 20;
/// Bytes hashed to find a run, and the shortest run that is used.
const WINDOW: usize = 16;
/// The old file is indexed at every `STRIDE`th byte; any run of `WINDOW + STRIDE` equal bytes contains an indexed position.
const STRIDE: usize = 8;
/// A match is measured over at most this many bytes per step (keeps long equal stretches linear; the extension goes on beyond it).
const PROBE: usize = 512;
const EMPTY: u32 = u32::MAX;
/// header: magic 8, version 4, old len 8, new len 8, old sha 32, new sha 32
const HEADER: usize = 8 + 4 + 8 + 8 + 32 + 32;

pub fn sha256(b: &[u8]) -> [u8; 32] {
    Sha256::digest(b).into()
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// What a delta says about the two files it connects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub old_len: u64,
    pub new_len: u64,
    pub old_sha256: String,
    pub new_sha256: String,
}

// -------------------------------------------------------------------------------------------- making

fn slot(b: &[u8], i: usize, shift: u32) -> usize {
    let a = u64::from_le_bytes(b[i..i + 8].try_into().unwrap_or([0; 8]));
    let c = u64::from_le_bytes(b[i + 8..i + 16].try_into().unwrap_or([0; 8]));
    let h = (a.wrapping_mul(0x9E37_79B1_85EB_CA87) ^ c.wrapping_mul(0xC2B2_AE3D_27D4_EB4F).rotate_left(29)).wrapping_mul(0x1656_67B1_9E37_79F9);
    (h >> shift) as usize
}

struct Index<'a> {
    old: &'a [u8],
    table: Vec<u32>,
    shift: u32,
}

impl<'a> Index<'a> {
    fn new(old: &'a [u8]) -> Index<'a> {
        let entries = old.len() / STRIDE + 1;
        let bits = (usize::BITS - entries.leading_zeros()).clamp(10, 26);
        let shift = 64 - bits;
        let mut table = Vec::new();
        // a file of 4 GiB or more cannot be indexed with 32-bit positions: no index, everything is sent as new bytes
        if old.len() >= WINDOW && (old.len() as u64) < u64::from(EMPTY) {
            table = vec![EMPTY; 1usize << bits];
            let mut p = 0;
            while p + WINDOW <= old.len() {
                let s = slot(old, p, shift);
                if table[s] == EMPTY {
                    table[s] = p as u32;
                }
                p += STRIDE;
            }
        }
        Index { old, table, shift }
    }

    /// The run in the old file that equals the new file from `at`: `(old position, length)`, length 0 when there is none.
    fn search(&self, new: &[u8], at: usize) -> (usize, usize) {
        if self.table.is_empty() || at + WINDOW > new.len() {
            return (0, 0);
        }
        let cand = self.table[slot(new, at, self.shift)];
        if cand == EMPTY {
            return (0, 0);
        }
        let pos = cand as usize;
        let max = PROBE.min(new.len() - at).min(self.old.len() - pos);
        let n = common(&self.old[pos..pos + max], &new[at..at + max]);
        if n < WINDOW { (0, 0) } else { (pos, n) }
    }
}

fn common(a: &[u8], b: &[u8]) -> usize {
    let n = a.len().min(b.len());
    let mut i = 0;
    while i + 8 <= n && a[i..i + 8] == b[i..i + 8] {
        i += 8;
    }
    while i < n && a[i] == b[i] {
        i += 1;
    }
    i
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn put_signed(out: &mut Vec<u8>, v: i64) {
    put_varint(out, ((v << 1) ^ (v >> 63)) as u64);
}

fn compress(raw: &[u8]) -> Vec<u8> {
    ruzstd::encoding::compress_to_vec(raw, ruzstd::encoding::CompressionLevel::Fastest)
}

/// The delta that turns `old` into `new`. Always succeeds (at worst the whole new file is carried as new bytes).
pub fn make_delta(old: &[u8], new: &[u8]) -> Vec<u8> {
    let (ctrl, diff, extra) = scan(old, new);
    let mut out = Vec::with_capacity(HEADER + 64 + diff.len() / 8 + extra.len() / 2);
    out.extend(MAGIC);
    out.extend(VERSION.to_be_bytes());
    out.extend((old.len() as u64).to_be_bytes());
    out.extend((new.len() as u64).to_be_bytes());
    out.extend(sha256(old));
    out.extend(sha256(new));
    for raw in [ctrl, diff, extra] {
        let z = compress(&raw);
        out.extend((raw.len() as u64).to_be_bytes());
        out.extend((z.len() as u64).to_be_bytes());
        out.extend(z);
    }
    let sum = sha256(&out);
    out.extend(sum);
    out
}

/// The three raw streams. The control flow follows bsdiff's scan; only the way a candidate run is found differs.
fn scan(old: &[u8], new: &[u8]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let index = Index::new(old);
    let (osz, nsz) = (old.len() as i64, new.len() as i64);
    let mut ctrl = Vec::new();
    let mut diff: Vec<u8> = Vec::with_capacity(new.len() / 2);
    let mut extra: Vec<u8> = Vec::new();
    let (mut scan, mut len, mut pos) = (0i64, 0i64, 0i64);
    let (mut lastscan, mut lastpos, mut lastoffset) = (0i64, 0i64, 0i64);
    let at = |b: &[u8], i: i64| b[i as usize];
    while scan < nsz {
        let mut oldscore = 0i64;
        scan += len;
        let mut scsc = scan;
        while scan < nsz {
            let (p, l) = index.search(new, scan as usize);
            pos = p as i64;
            len = l as i64;
            while scsc < scan + len {
                if scsc + lastoffset < osz && scsc + lastoffset >= 0 && at(old, scsc + lastoffset) == at(new, scsc) {
                    oldscore += 1;
                }
                scsc += 1;
            }
            if (len == oldscore && len != 0) || len > oldscore + 8 {
                break;
            }
            if scan + lastoffset < osz && scan + lastoffset >= 0 && at(old, scan + lastoffset) == at(new, scan) {
                oldscore -= 1;
            }
            scan += 1;
        }
        if len != oldscore || scan == nsz {
            // forward from the end of the last piece
            let (mut s, mut sf, mut lenf, mut i) = (0i64, 0i64, 0i64, 0i64);
            while lastscan + i < scan && lastpos + i < osz {
                if at(old, lastpos + i) == at(new, lastscan + i) {
                    s += 1;
                }
                i += 1;
                if s * 2 - i > sf * 2 - lenf {
                    sf = s;
                    lenf = i;
                }
            }
            // backward from the start of this one
            let mut lenb = 0i64;
            if scan < nsz {
                let (mut s, mut sb, mut i) = (0i64, 0i64, 1i64);
                while scan >= lastscan + i && pos >= i {
                    if at(old, pos - i) == at(new, scan - i) {
                        s += 1;
                    }
                    if s * 2 - i > sb * 2 - lenb {
                        sb = s;
                        lenb = i;
                    }
                    i += 1;
                }
            }
            if lastscan + lenf > scan - lenb {
                let overlap = (lastscan + lenf) - (scan - lenb);
                let (mut s, mut ss, mut lens) = (0i64, 0i64, 0i64);
                for i in 0..overlap {
                    if at(new, lastscan + lenf - overlap + i) == at(old, lastpos + lenf - overlap + i) {
                        s += 1;
                    }
                    if at(new, scan - lenb + i) == at(old, pos - lenb + i) {
                        s -= 1;
                    }
                    if s > ss {
                        ss = s;
                        lens = i + 1;
                    }
                }
                lenf += lens - overlap;
                lenb -= lens;
            }
            for i in 0..lenf {
                diff.push(at(new, lastscan + i).wrapping_sub(at(old, lastpos + i)));
            }
            let extra_len = (scan - lenb) - (lastscan + lenf);
            extra.extend_from_slice(&new[(lastscan + lenf) as usize..(lastscan + lenf + extra_len) as usize]);
            put_varint(&mut ctrl, lenf as u64);
            put_varint(&mut ctrl, extra_len as u64);
            put_signed(&mut ctrl, (pos - lenb) - (lastpos + lenf));
            lastscan = scan - lenb;
            lastpos = pos - lenb;
            lastoffset = pos - scan;
        }
    }
    (ctrl, diff, extra)
}

// -------------------------------------------------------------------------------------------- reading

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.b.len()).ok_or_else(|| cut())?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }

    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().map_err(|_| cut())?))
    }
}

fn cut() -> String {
    "the delta is cut off or damaged".to_string()
}

fn get_varint(b: &[u8], at: &mut usize) -> Result<u64, String> {
    let (mut v, mut shift) = (0u64, 0u32);
    loop {
        let x = *b.get(*at).ok_or_else(cut)?;
        *at += 1;
        if shift > 63 {
            return Err("the delta's control data is damaged".into());
        }
        v |= u64::from(x & 0x7f) << shift;
        if x & 0x80 == 0 {
            return Ok(v);
        }
        shift += 7;
    }
}

fn get_signed(b: &[u8], at: &mut usize) -> Result<i64, String> {
    let u = get_varint(b, at)?;
    Ok(((u >> 1) as i64) ^ -((u & 1) as i64))
}

fn decompress(z: &[u8], raw_len: u64) -> Result<Vec<u8>, String> {
    let mut dec = ruzstd::decoding::StreamingDecoder::new(z).map_err(|e| format!("the delta's data is not readable ({e})"))?;
    let mut out = Vec::with_capacity(raw_len as usize);
    // one byte more than announced: a stream that is longer than it says is damaged
    (&mut dec).take(raw_len + 1).read_to_end(&mut out).map_err(|e| format!("the delta's data is damaged ({e})"))?;
    if out.len() as u64 != raw_len {
        return Err("the delta's data has the wrong length: it is damaged".into());
    }
    Ok(out)
}

/// Read the header of a delta after checking its trailer (so a damaged or cut delta is named before anything else is done with it).
pub fn info(delta: &[u8]) -> Result<Info, String> {
    if delta.len() < HEADER + 32 || &delta[..8] != MAGIC {
        return Err("this is not an IYAKKI binary delta".into());
    }
    let (body, sum) = delta.split_at(delta.len() - 32);
    if sha256(body) != sum {
        return Err("the delta's checksum does not match: it is damaged, cut off or was changed".into());
    }
    let mut r = Reader { b: body, at: 8 };
    let v = u32::from_be_bytes(r.take(4)?.try_into().map_err(|_| cut())?);
    if v != VERSION {
        return Err(format!("delta version {v} is not understood (this device reads version {VERSION})"));
    }
    let (old_len, new_len) = (r.u64()?, r.u64()?);
    if old_len > MAX_FILE || new_len > MAX_FILE {
        return Err(format!("the delta is for a file over {} MB: refused", MAX_FILE >> 20));
    }
    Ok(Info { old_len, new_len, old_sha256: hex(r.take(32)?), new_sha256: hex(r.take(32)?) })
}

/// The new file from the old file and a delta. Refuses, with a plain message and no output, when the delta is damaged, when `old` is not
/// the file the delta was made against, or when the result does not match the hash the delta promises.
pub fn apply_delta(old: &[u8], delta: &[u8]) -> Result<Vec<u8>, String> {
    let i = info(delta)?;
    if old.len() as u64 != i.old_len || hex(&sha256(old)) != i.old_sha256 {
        return Err("the file on this device is not the version this delta was made against (its length or SHA-256 differs): nothing was changed".into());
    }
    let body = &delta[..delta.len() - 32];
    let mut r = Reader { b: body, at: HEADER };
    let mut streams = Vec::with_capacity(3);
    for (n, cap) in [i.new_len.saturating_mul(3).saturating_add(64), i.new_len, i.new_len].into_iter().enumerate() {
        let (raw, zl) = (r.u64()?, r.u64()?);
        if raw > cap || zl > body.len() as u64 {
            return Err(format!("the delta's stream {n} announces more data than it can hold: it is damaged"));
        }
        streams.push(decompress(r.take(zl as usize)?, raw)?);
    }
    if r.at != body.len() {
        return Err("the delta carries more bytes than it lists".into());
    }
    let (ctrl, diff, extra) = (&streams[0], &streams[1], &streams[2]);
    let mut out: Vec<u8> = Vec::with_capacity(i.new_len as usize);
    let (mut c, mut d, mut e, mut oldpos) = (0usize, 0usize, 0usize, 0i64);
    let bad = || "the delta's pieces do not fit the files: it is damaged".to_string();
    while c < ctrl.len() {
        let add = get_varint(ctrl, &mut c)? as usize;
        let ext = get_varint(ctrl, &mut c)? as usize;
        let seek = get_signed(ctrl, &mut c)?;
        if out.len().saturating_add(add).saturating_add(ext) > i.new_len as usize || d.checked_add(add).is_none_or(|x| x > diff.len()) || e.checked_add(ext).is_none_or(|x| x > extra.len()) {
            return Err(bad());
        }
        if add > 0 {
            if oldpos < 0 || (oldpos as usize).checked_add(add).is_none_or(|x| x > old.len()) {
                return Err(bad());
            }
            let o = &old[oldpos as usize..oldpos as usize + add];
            out.extend(o.iter().zip(&diff[d..d + add]).map(|(a, b)| a.wrapping_add(*b)));
            d += add;
        }
        out.extend_from_slice(&extra[e..e + ext]);
        e += ext;
        oldpos = oldpos.checked_add(add as i64).and_then(|x| x.checked_add(seek)).ok_or_else(bad)?;
    }
    if out.len() as u64 != i.new_len || d != diff.len() || e != extra.len() {
        return Err(bad());
    }
    if hex(&sha256(&out)) != i.new_sha256 {
        return Err("the file made from the delta does not match the SHA-256 the delta promises: nothing was written".into());
    }
    Ok(out)
}
