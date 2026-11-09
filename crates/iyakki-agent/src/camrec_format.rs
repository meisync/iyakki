//! The camera recording segment file (F-CAM-03): an authenticated, encrypted file of received frames, stored as they arrived (no
//! re-encoding).
//!
//! Decision (see `docs/development/CAMERAS.md`): the data-volume container of `iyakki-volcrypt` is a whole-disk sector store with key
//! slots; recordings are many small append-only files that must survive a power cut and be deleted one by one, so they do not fit it.
//! What is reused is its primitives: the AES-256-XTS cipher (`iyakki_volcrypt::xts`) and HMAC-SHA256 with a constant-time comparison
//! (`iyakki_volcrypt::mac`). The format is encrypt-then-MAC per frame:
//!
//! ```text
//! file   = header record*
//! header = "IYAKREC1" kind:u8 0 0 0 start_ms:u64le nonce:16  mac:32          (68 bytes)
//! record = cipher_len:u32le ts_ms:u64le flags:u8  cipher  mac:32
//! ```
//!
//! * One 256-bit key per camera (held in the owner's vault). A fresh random `nonce` per file gives the file its own keys: the XTS
//!   key pair (64 bytes) and the MAC key come from HMAC-SHA256 of the camera key and the nonce, so no two files share a tweak space.
//! * A record's plaintext is `len:u32le data` padded with zeros to a multiple of 16; it is encrypted with AES-256-XTS, the record
//!   number as the tweak. The MAC covers the record number, the 13 header bytes and the cipher text, so a changed, moved or swapped
//!   record is found. The file header is MACed as well, which is also how a wrong key is told from a damaged file.
//! * The time stamp and flags stay readable (the file name says when it starts anyway): the timeline and the index work without the
//!   key, and an index can be rebuilt from the files alone. What is visible to someone with the disk: when frames arrived, how
//!   large they are, which are key frames. What is not: the pictures.
//! * Not detected: frames removed from the END of a file (or whole files deleted). A power cut looks the same.
//! * A record cut short by a power cut is simply not there: a scan stops at the first incomplete record and every earlier one reads.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use iyakki_volcrypt::mac::{ct_eq, hmac_sha256};
use iyakki_volcrypt::xts::Xts;
use zeroize::{Zeroize, Zeroizing};

pub const MAGIC: &[u8; 8] = b"IYAKREC1";
pub const HEADER_LEN: u64 = 68;
const REC_HEAD: usize = 13;
const MAC_LEN: usize = 32;
/// The largest frame stored (the RTSP client drops larger ones anyway).
pub const MAX_FRAME: usize = 8 * 1024 * 1024;
const MAX_CIPHER: usize = MAX_FRAME + 32;

/// This record is a key frame (H.264 IDR; every Motion-JPEG frame is one).
pub const F_KEY: u8 = 1;
/// This record holds the H.264 parameter sets (SPS, PPS) in Annex-B form; every H.264 segment starts with one.
pub const F_PARAMS: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecKind {
    /// Complete JPEG files, one per record.
    Jpeg = 1,
    /// H.264 access units in Annex-B form, one per record.
    H264 = 2,
    /// Motion events (F-CAM-04): JSON records and small JPEG thumbnails, see `camdetect::events`.
    Events = 3,
}

impl RecKind {
    fn from(b: u8) -> Option<RecKind> {
        match b {
            1 => Some(RecKind::Jpeg),
            2 => Some(RecKind::H264),
            3 => Some(RecKind::Events),
            _ => None,
        }
    }
    pub fn word(self) -> &'static str {
        match self {
            RecKind::Jpeg => "Motion-JPEG",
            RecKind::H264 => "H.264",
            RecKind::Events => "motion events",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReadErr {
    /// Not a recording file of ours.
    NotARecording,
    /// The key does not open this file (a different key, or a damaged header).
    WrongKey,
    /// A record does not match its checksum: the file was changed or damaged after it was written.
    Tampered(usize),
    OutOfRange,
    Io(String),
}

impl std::fmt::Display for ReadErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadErr::NotARecording => write!(f, "this is not a camera recording"),
            ReadErr::WrongKey => write!(f, "the recording's key does not fit (the key in your vault is not the one it was recorded with)"),
            ReadErr::Tampered(i) => write!(f, "picture {} of this recording was changed or damaged after it was written; it is not shown", i + 1),
            ReadErr::OutOfRange => write!(f, "no such picture in this recording"),
            ReadErr::Io(m) => write!(f, "could not read the recording: {m}"),
        }
    }
}

/// A camera's recording key: 256 random bits.
pub struct RecKey(Zeroizing<[u8; 32]>);

impl RecKey {
    pub fn generate() -> Result<RecKey, String> {
        let mut k = [0u8; 32];
        iyakki_sec::random_fill(&mut k).map_err(|_| "the system could not supply random numbers".to_string())?;
        let r = RecKey(Zeroizing::new(k));
        k.zeroize();
        Ok(r)
    }

    pub fn from_bytes(b: [u8; 32]) -> RecKey {
        RecKey(Zeroizing::new(b))
    }

    /// The form kept in the vault.
    pub fn to_hex(&self) -> Zeroizing<String> {
        let mut s = Zeroizing::new(String::with_capacity(64));
        for b in self.0.iter() {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    pub fn from_hex(t: &str) -> Option<RecKey> {
        let t = t.trim();
        if t.len() != 64 || !t.is_ascii() {
            return None;
        }
        let mut k = [0u8; 32];
        for (i, c) in t.as_bytes().chunks(2).enumerate() {
            k[i] = u8::from_str_radix(std::str::from_utf8(c).ok()?, 16).ok()?;
        }
        let r = RecKey(Zeroizing::new(k));
        k.zeroize();
        Some(r)
    }
}

struct SegKeys {
    xts: Xts,
    mac: Zeroizing<[u8; 32]>,
}

fn derive(key: &RecKey, nonce: &[u8; 16]) -> SegKeys {
    let a = hmac_sha256(&key.0[..], &[b"iyakki camera recording xts 1", nonce]);
    let b = hmac_sha256(&key.0[..], &[b"iyakki camera recording xts 2", nonce]);
    let mut k = Zeroizing::new([0u8; 64]);
    k[..32].copy_from_slice(&a);
    k[32..].copy_from_slice(&b);
    let xts = Xts::new(&k);
    let mac = Zeroizing::new(hmac_sha256(&key.0[..], &[b"iyakki camera recording mac", nonce]));
    SegKeys { xts, mac }
}

fn record_mac(mac: &[u8; 32], idx: u64, head: &[u8], cipher: &[u8]) -> [u8; 32] {
    hmac_sha256(mac, &[b"rec", &idx.to_le_bytes(), head, cipher])
}

fn header_bytes(kind: RecKind, start_ms: u64, nonce: &[u8; 16]) -> [u8; 36] {
    let mut h = [0u8; 36];
    h[..8].copy_from_slice(MAGIC);
    h[8] = kind as u8;
    h[12..20].copy_from_slice(&start_ms.to_le_bytes());
    h[20..36].copy_from_slice(nonce);
    h
}

// ------------------------------------------------------------------------------------------------ writing

pub struct SegmentWriter {
    file: std::fs::File,
    keys: SegKeys,
    idx: u64,
    pub bytes: u64,
    pub frames: u64,
    pub start_ms: u64,
    pub last_ms: u64,
}

impl SegmentWriter {
    /// A new file (never replaces one), mode 0600, with its header written.
    pub fn create(path: &Path, key: &RecKey, kind: RecKind, start_ms: u64) -> Result<SegmentWriter, String> {
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("could not make {}: {e}", d.display()))?;
        }
        let mut nonce = [0u8; 16];
        iyakki_sec::random_fill(&mut nonce).map_err(|_| "the system could not supply random numbers".to_string())?;
        let keys = derive(key, &nonce);
        let h = header_bytes(kind, start_ms, &nonce);
        let m = hmac_sha256(&keys.mac[..], &[b"hdr", &h]);
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).map_err(|e| format!("could not start the recording file: {e}"))?;
        let mut all = Vec::with_capacity(HEADER_LEN as usize);
        all.extend_from_slice(&h);
        all.extend_from_slice(&m);
        file.write_all(&all).map_err(|e| format!("could not write the recording: {e}"))?;
        Ok(SegmentWriter { file, keys, idx: 0, bytes: HEADER_LEN, frames: 0, start_ms, last_ms: start_ms })
    }

    /// Append one frame: encrypted, MACed, written with one call.
    pub fn append(&mut self, ts_ms: u64, flags: u8, data: &[u8]) -> Result<(), String> {
        if data.is_empty() || data.len() > MAX_FRAME {
            return Err("a picture of an unusable size was skipped".into());
        }
        let padded = (4 + data.len()).div_ceil(16) * 16;
        let mut plain = Zeroizing::new(vec![0u8; padded]);
        plain[..4].copy_from_slice(&(data.len() as u32).to_le_bytes());
        plain[4..4 + data.len()].copy_from_slice(data);
        if !self.keys.xts.encrypt_sector(self.idx, &mut plain) {
            return Err("encryption failed".into());
        }
        let mut head = [0u8; REC_HEAD];
        head[..4].copy_from_slice(&(padded as u32).to_le_bytes());
        head[4..12].copy_from_slice(&ts_ms.to_le_bytes());
        head[12] = flags;
        let m = record_mac(&self.keys.mac, self.idx, &head, &plain);
        let mut out = Vec::with_capacity(REC_HEAD + padded + MAC_LEN);
        out.extend_from_slice(&head);
        out.extend_from_slice(&plain);
        out.extend_from_slice(&m);
        self.file.write_all(&out).map_err(|e| format!("could not write the recording: {e}"))?;
        self.idx += 1;
        self.frames += 1;
        self.bytes += out.len() as u64;
        self.last_ms = ts_ms;
        Ok(())
    }

    /// Ask the disk to keep what was written (a power cut after this loses nothing before it).
    pub fn sync(&mut self) {
        let _ = self.file.sync_data();
    }
}

impl Drop for SegmentWriter {
    fn drop(&mut self) {
        let _ = self.file.sync_data();
    }
}

// ------------------------------------------------------------------------------------------------ reading

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameRef {
    pub offset: u64,
    pub cipher_len: u32,
    pub ts_ms: u64,
    pub flags: u8,
}

/// What can be read from a file without a key.
#[derive(Clone, Debug)]
pub struct Scan {
    pub kind: RecKind,
    pub start_ms: u64,
    pub nonce: [u8; 16],
    pub frames: Vec<FrameRef>,
    /// Bytes up to the end of the last complete record.
    pub valid_len: u64,
    pub file_len: u64,
}

impl Scan {
    pub fn end_ms(&self) -> u64 {
        self.frames.last().map_or(self.start_ms, |f| f.ts_ms.max(self.start_ms))
    }
    /// The file ends with a half-written record (a power cut) or garbage.
    pub fn cut_short(&self) -> bool {
        self.valid_len != self.file_len
    }
}

fn io(e: std::io::Error) -> ReadErr {
    ReadErr::Io(e.to_string())
}

/// Read the header and the record heads (not the pictures), stopping at the first incomplete or implausible record.
pub fn scan(path: &Path) -> Result<Scan, ReadErr> {
    let f = std::fs::File::open(path).map_err(io)?;
    let file_len = f.metadata().map_err(io)?.len();
    let mut r = std::io::BufReader::with_capacity(64 * 1024, f);
    let mut h = [0u8; HEADER_LEN as usize];
    if file_len < HEADER_LEN || r.read_exact(&mut h).is_err() || &h[..8] != MAGIC {
        return Err(ReadErr::NotARecording);
    }
    let kind = RecKind::from(h[8]).ok_or(ReadErr::NotARecording)?;
    let start_ms = u64::from_le_bytes(h[12..20].try_into().unwrap_or([0; 8]));
    let mut nonce = [0u8; 16];
    nonce.copy_from_slice(&h[20..36]);
    let mut frames = Vec::new();
    let mut pos = HEADER_LEN;
    loop {
        if pos + REC_HEAD as u64 > file_len {
            break;
        }
        let mut head = [0u8; REC_HEAD];
        if r.read_exact(&mut head).is_err() {
            break;
        }
        let cl = u32::from_le_bytes(head[..4].try_into().unwrap_or([0; 4])) as usize;
        if cl < 16 || cl % 16 != 0 || cl > MAX_CIPHER {
            break;
        }
        let end = pos + (REC_HEAD + cl + MAC_LEN) as u64;
        if end > file_len {
            break;
        }
        frames.push(FrameRef { offset: pos, cipher_len: cl as u32, ts_ms: u64::from_le_bytes(head[4..12].try_into().unwrap_or([0; 8])), flags: head[12] });
        if r.seek_relative((cl + MAC_LEN) as i64).is_err() {
            frames.pop();
            break;
        }
        pos = end;
    }
    Ok(Scan { kind, start_ms, nonce, frames, valid_len: pos, file_len })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub ts_ms: u64,
    pub flags: u8,
    pub data: Vec<u8>,
}

/// A recording file opened with its key: the header is checked, any frame can be read and checked.
pub struct Segment {
    path: PathBuf,
    pub scan: Scan,
    keys: SegKeys,
}

impl Segment {
    pub fn open(path: &Path, key: &RecKey) -> Result<Segment, ReadErr> {
        let scan = scan(path)?;
        let keys = derive(key, &scan.nonce);
        let mut f = std::fs::File::open(path).map_err(io)?;
        let mut h = [0u8; HEADER_LEN as usize];
        f.read_exact(&mut h).map_err(io)?;
        let want = hmac_sha256(&keys.mac[..], &[b"hdr", &h[..36]]);
        if !ct_eq(&want, &h[36..]) {
            return Err(ReadErr::WrongKey);
        }
        Ok(Segment { path: path.to_path_buf(), scan, keys })
    }

    pub fn len(&self) -> usize {
        self.scan.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.scan.frames.is_empty()
    }

    /// Read, check and decrypt frame `i`.
    pub fn frame(&self, i: usize) -> Result<Frame, ReadErr> {
        let fr = self.scan.frames.get(i).ok_or(ReadErr::OutOfRange)?;
        let mut f = std::fs::File::open(&self.path).map_err(io)?;
        f.seek(SeekFrom::Start(fr.offset)).map_err(io)?;
        let mut buf = vec![0u8; REC_HEAD + fr.cipher_len as usize + MAC_LEN];
        f.read_exact(&mut buf).map_err(io)?;
        let (head, rest) = buf.split_at(REC_HEAD);
        let (cipher, mac) = rest.split_at(fr.cipher_len as usize);
        let want = record_mac(&self.keys.mac, i as u64, head, cipher);
        if !ct_eq(&want, mac) {
            return Err(ReadErr::Tampered(i));
        }
        let mut plain = Zeroizing::new(cipher.to_vec());
        if !self.keys.xts.decrypt_sector(i as u64, &mut plain) {
            return Err(ReadErr::Tampered(i));
        }
        let n = u32::from_le_bytes(plain[..4].try_into().unwrap_or([0; 4])) as usize;
        if n == 0 || 4 + n > plain.len() {
            return Err(ReadErr::Tampered(i));
        }
        Ok(Frame { ts_ms: fr.ts_ms, flags: fr.flags, data: plain[4..4 + n].to_vec() })
    }
}

#[cfg(test)]
#[path = "camrec_format_tests.rs"]
mod tests;
