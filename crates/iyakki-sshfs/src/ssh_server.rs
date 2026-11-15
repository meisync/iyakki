//! A small SSH 2 server that offers one thing: the `sftp` subsystem (see `docs/development/SFTP-SERVER.md`). In-house, over primitives already in
//! the tree: X25519 (`curve25519-sha256`), Ed25519 (host key and public-key login), `chacha20-poly1305@openssh.com`, `aes256-gcm@openssh.com`
//! and AES-256-CTR with HMAC-SHA-256 (encrypt-then-MAC and the older MAC-then-encrypt form), SHA-256. The AEAD ciphers come from permissive
//! crates that are already in the lock file (`chacha20`, `poly1305`, `aes-gcm`); the packet framing around them is ours.
//!
//! What it does not do, on purpose: no shell, no command, no pseudo-terminal, no port or agent forwarding, no X11, no compression, no
//! other key types (clients with neither Ed25519 nor ECDSA P-256 host keys cannot connect, F-FILE-14 plans an RSA host key; login keys may also be RSA of 2048 to 8192 bits or ECDSA P-256, see `sshalg.rs`; every current OpenSSH, Dropbear, PuTTY,
//! WinSCP and libssh2 can). "Strict key exchange" (the Terrapin fix) is on when the client offers it. Re-keying by the client works, and the
//! server starts a re-key itself after [`REKEY_BYTES`] (1 GB) in both directions together; packets it would send meanwhile are held until the
//! new keys are in use.
//!
//! Safety limits: a login grace time, four failed logins per connection, packets over 35 000 bytes are refused before they are read, a
//! channel window and packet size are enforced both ways, one session channel at a time, an idle timeout, and every read is checked against
//! the stop switch. Secrets (passwords, key material) are held in `Zeroizing` buffers and never reach the audit callback.

use std::io::{Read, Write};
use std::net::{IpAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aes::cipher::{KeyIvInit, StreamCipher, StreamCipherSeek};
use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce, Tag};
use chacha20::ChaCha20;
use poly1305::Poly1305;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::sftp_session::Session;
use crate::sshkey::{put_str, put_u32, KeyPair, Reader};

type Aes256Ctr = ctr::Ctr128BE<aes::Aes256>;

pub const VERSION_LINE: &str = "SSH-2.0-IYAKKI_SFTP";
/// Largest packet length accepted (RFC 4253 asks every server to take 35 000).
pub const MAX_PACKET: usize = 35_000;
const LOGIN_GRACE: Duration = Duration::from_secs(30);
const IDLE: Duration = Duration::from_secs(15 * 60);
const MAX_AUTH_FAILURES: u32 = 4;
const WINDOW: u32 = 2 * 1024 * 1024;
const CHAN_MAX_PACKET: u32 = 32_768;
/// Answers waiting for the client's window: a client that keeps asking and never opens its window is cut off, not buffered without end.
const OUT_CAP: usize = 8 * 1024 * 1024;

const KEX_ALGS: &str = "curve25519-sha256,curve25519-sha256@libssh.org,kex-strict-s-v00@openssh.com";
const HOSTKEY_ALG: &str = "ssh-ed25519";
const CIPHER_CTR: &str = "aes256-ctr";
const CIPHER_GCM: &str = "aes256-gcm@openssh.com";
const CIPHER_CHACHA: &str = "chacha20-poly1305@openssh.com";
/// What is offered, the first of the client's own list that is here wins.
const CIPHERS: [&str; 3] = [CIPHER_CHACHA, CIPHER_GCM, CIPHER_CTR];
/// Bytes read plus written under one set of keys before the server asks for a re-key (RFC 4253, 9 advises 1 GB).
pub const REKEY_BYTES: u64 = 1 << 30;
/// Packets held back during a re-key before the peer is told off for not answering.
const MAX_HELD: usize = 512;
const MACS: [&str; 2] = ["hmac-sha2-256-etm@openssh.com", "hmac-sha2-256"];

// message numbers
const DISCONNECT: u8 = 1;
const IGNORE: u8 = 2;
const UNIMPLEMENTED: u8 = 3;
const DEBUG: u8 = 4;
const EXT_INFO: u8 = 7;
const SERVICE_REQUEST: u8 = 5;
const SERVICE_ACCEPT: u8 = 6;
const KEXINIT: u8 = 20;
const NEWKEYS: u8 = 21;
const KEX_ECDH_INIT: u8 = 30;
const KEX_ECDH_REPLY: u8 = 31;
const USERAUTH_REQUEST: u8 = 50;
const USERAUTH_FAILURE: u8 = 51;
const USERAUTH_SUCCESS: u8 = 52;
const USERAUTH_PK_OK: u8 = 60;
const GLOBAL_REQUEST: u8 = 80;
const REQUEST_FAILURE: u8 = 82;
const CHANNEL_OPEN: u8 = 90;
const CHANNEL_OPEN_CONFIRMATION: u8 = 91;
const CHANNEL_OPEN_FAILURE: u8 = 92;
const CHANNEL_WINDOW_ADJUST: u8 = 93;
const CHANNEL_DATA: u8 = 94;
const CHANNEL_EXTENDED_DATA: u8 = 95;
const CHANNEL_EOF: u8 = 96;
const CHANNEL_CLOSE: u8 = 97;
const CHANNEL_REQUEST: u8 = 98;
const CHANNEL_SUCCESS: u8 = 99;
const CHANNEL_FAILURE: u8 = 100;

/// Who may log in and what they get. The real one checks the account list, authorised keys and the account's password.
pub trait Gate: Send + Sync {
    /// Is this public key blob authorised for this user?
    fn key_ok(&self, user: &str, blob: &[u8], peer: IpAddr) -> bool;
    /// Is the password right (and password login on)? The password is never stored or logged by the caller.
    fn password_ok(&self, user: &str, password: &str, peer: IpAddr) -> bool;
    /// The SFTP session of a user who has logged in.
    fn session(&self, user: &str, peer: IpAddr) -> Option<Session>;
    /// An audit line without secrets.
    fn audit(&self, event: &str, detail: &str);
}

/// Hmac-SHA-256 (RFC 2104).
pub fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    for p in parts {
        inner.update(p);
    }
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// Is this peer on the local network (private, loopback or link-local)? The server refuses everyone else, whatever the firewall says.
pub fn is_local_peer(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => a.is_private() || a.is_loopback() || a.is_link_local(),
        IpAddr::V6(a) => {
            if let Some(v4) = a.to_ipv4_mapped() {
                return is_local_peer(IpAddr::V4(v4));
            }
            a.is_loopback() || (a.segments()[0] & 0xfe00) == 0xfc00 || (a.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

#[derive(Debug)]
pub enum Fail {
    /// The peer closed, timed out, or the server was stopped.
    Closed,
    /// The peer broke the protocol.
    Protocol(&'static str),
    Io(String),
}

impl From<std::io::Error> for Fail {
    fn from(e: std::io::Error) -> Fail {
        match e.kind() {
            std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionAborted => Fail::Closed,
            _ => Fail::Io(e.kind().to_string()),
        }
    }
}

type R<T> = Result<T, Fail>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cipher {
    Ctr,
    Gcm,
    Chacha,
}

impl Cipher {
    fn from_name(n: &str) -> Option<Cipher> {
        match n {
            CIPHER_CTR => Some(Cipher::Ctr),
            CIPHER_GCM => Some(Cipher::Gcm),
            CIPHER_CHACHA => Some(Cipher::Chacha),
            _ => None,
        }
    }
}

/// One direction's keys.
enum Dir {
    Ctr { ctr: Aes256Ctr, mac_key: Zeroizing<[u8; 32]>, etm: bool },
    /// RFC 5647: the length stays in the clear and is authenticated; the nonce is a 4-byte fixed part and a counter that grows by one per packet.
    Gcm { aead: Box<Aes256Gcm>, iv: [u8; 12] },
    /// OpenSSH's construction: `hdr` hides the length, `main` encrypts the payload and makes the one-time Poly1305 key; the nonce is the sequence number.
    Chacha { main: Zeroizing<[u8; 32]>, hdr: Zeroizing<[u8; 32]> },
}

fn gcm_step(iv: &mut [u8; 12]) {
    let mut c = [0u8; 8];
    c.copy_from_slice(&iv[4..]);
    iv[4..].copy_from_slice(&u64::from_be_bytes(c).wrapping_add(1).to_be_bytes());
}

fn chacha_at(key: &[u8; 32], seq: u32) -> ChaCha20 {
    // the 8-byte nonce of OpenSSH is the 64-bit sequence number; the IETF layout has the same words after a zero counter word
    let mut n = [0u8; 12];
    n[8..].copy_from_slice(&seq.to_be_bytes());
    ChaCha20::new(key.into(), &n.into())
}

/// The Poly1305 tag over the parts, and the payload cipher positioned at the second block (the first makes the one-time key).
fn poly_tag(main: &[u8; 32], seq: u32, parts: &[&[u8]]) -> ([u8; 16], ChaCha20) {
    let mut c = chacha_at(main, seq);
    let mut pk = Zeroizing::new([0u8; 32]);
    c.apply_keystream(&mut *pk);
    let tag = Poly1305::new((&*pk).into()).compute_unpadded(&parts.concat());
    c.seek(64u32);
    (tag.into(), c)
}

/// More key bytes than one hash gives: K1 = H(K || H || X || sid), K2 = H(K || H || K1) (RFC 4253, 7.2).
fn derive_64(k: &[u8], h: &[u8], x: u8, sid: &[u8]) -> Zeroizing<[u8; 64]> {
    let k1 = derive(k, h, x, sid);
    let mut km = Vec::new();
    mpint(&mut km, k);
    let mut d = Sha256::new();
    d.update(&km);
    d.update(h);
    d.update(&*k1);
    let k2: [u8; 32] = d.finalize().into();
    let mut o = Zeroizing::new([0u8; 64]);
    o[..32].copy_from_slice(&*k1);
    o[32..].copy_from_slice(&k2);
    o
}

/// The keys of one direction: `key_x`, `iv_x` and `mac_x` are the letters of RFC 4253, 7.2.
fn make_dir(cipher: Cipher, etm: bool, k: &[u8], h: &[u8], sid: &[u8], key_x: u8, iv_x: u8, mac_x: u8) -> Dir {
    match cipher {
        Cipher::Ctr => {
            let (key, iv) = (derive(k, h, key_x, sid), derive(k, h, iv_x, sid));
            Dir::Ctr { ctr: Aes256Ctr::new((&*key).into(), (&iv[..16]).into()), mac_key: derive(k, h, mac_x, sid), etm }
        }
        Cipher::Gcm => {
            let (key, iv) = (derive(k, h, key_x, sid), derive(k, h, iv_x, sid));
            let mut n = [0u8; 12];
            n.copy_from_slice(&iv[..12]);
            Dir::Gcm { aead: Box::new(Aes256Gcm::new((&*key).into())), iv: n }
        }
        Cipher::Chacha => {
            let key = derive_64(k, h, key_x, sid);
            let (mut main, mut hdr) = (Zeroizing::new([0u8; 32]), Zeroizing::new([0u8; 32]));
            // OpenSSH: the first 32 bytes are K_2 (payload), the last 32 are K_1 (length)
            main.copy_from_slice(&key[..32]);
            hdr.copy_from_slice(&key[32..]);
            Dir::Chacha { main, hdr }
        }
    }
}

/// The binary packet layer over any stream.
pub struct Transport<S: Read + Write> {
    s: S,
    rseq: u32,
    wseq: u32,
    inn: Option<Dir>,
    out: Option<Dir>,
    stop: Arc<AtomicBool>,
    deadline: Instant,
    pad: [u8; 256],
    /// Bytes read and written under the keys now in use.
    bytes: u64,
    rekey_after: u64,
    /// Our KEXINIT is out and the new keys are not in use yet: other packets wait in `held`.
    hold: bool,
    held: Vec<Vec<u8>>,
}

fn rand<const N: usize>() -> R<[u8; N]> {
    let mut b = [0u8; N];
    iyakki_sec::random_fill(&mut b).map_err(|_| Fail::Io("no random numbers".into()))?;
    Ok(b)
}

impl<S: Read + Write> Transport<S> {
    pub fn new(s: S, stop: Arc<AtomicBool>) -> R<Transport<S>> {
        Ok(Transport { s, rseq: 0, wseq: 0, inn: None, out: None, stop, deadline: Instant::now() + LOGIN_GRACE, pad: rand::<256>()?, bytes: 0, rekey_after: REKEY_BYTES, hold: false, held: Vec::new() })
    }

    pub fn set_deadline(&mut self, d: Duration) {
        self.deadline = Instant::now() + d;
    }

    /// Fill `buf`, polling the stop switch and the deadline (the socket has a short read timeout).
    fn read_full(&mut self, buf: &mut [u8]) -> R<()> {
        let mut got = 0;
        while got < buf.len() {
            if self.stop.load(Ordering::Relaxed) || Instant::now() > self.deadline {
                return Err(Fail::Closed);
            }
            match self.s.read(&mut buf[got..]) {
                Ok(0) => return Err(Fail::Closed),
                Ok(n) => got += n,
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// The peer's identification line: at most 255 bytes, ends at LF; lines before the first `SSH-` are skipped (at most a few).
    pub fn read_version(&mut self) -> R<String> {
        for _ in 0..8 {
            let mut line = Vec::new();
            loop {
                let mut b = [0u8; 1];
                self.read_full(&mut b)?;
                if b[0] == b'\n' {
                    break;
                }
                line.push(b[0]);
                if line.len() > 255 {
                    return Err(Fail::Protocol("identification line too long"));
                }
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if line.starts_with(b"SSH-") {
                let s = String::from_utf8(line).map_err(|_| Fail::Protocol("identification is not text"))?;
                if !(s.starts_with("SSH-2.0-") || s.starts_with("SSH-1.99-")) {
                    return Err(Fail::Protocol("not SSH 2"));
                }
                return Ok(s);
            }
        }
        Err(Fail::Protocol("no identification line"))
    }

    pub fn write_raw(&mut self, b: &[u8]) -> R<()> {
        self.s.write_all(b)?;
        self.s.flush()?;
        Ok(())
    }

    /// One packet's payload.
    pub fn read_packet(&mut self) -> R<Vec<u8>> {
        let seq = self.rseq;
        self.rseq = self.rseq.wrapping_add(1);
        let kind = match &self.inn {
            None => None,
            Some(Dir::Ctr { etm, .. }) => Some((Cipher::Ctr, *etm)),
            Some(Dir::Gcm { .. }) => Some((Cipher::Gcm, true)),
            Some(Dir::Chacha { .. }) => Some((Cipher::Chacha, true)),
        };
        let (payload, used) = match kind {
            None => {
                let mut h = [0u8; 4];
                self.read_full(&mut h)?;
                let len = u32::from_be_bytes(h) as usize;
                check_len(len, 8)?;
                let mut body = vec![0u8; len];
                self.read_full(&mut body)?;
                (unpad(&body)?, 4 + len)
            }
            Some((Cipher::Ctr, true)) => {
                let mut h = [0u8; 4];
                self.read_full(&mut h)?;
                let len = u32::from_be_bytes(h) as usize;
                check_len(len, 16)?;
                if len % 16 != 0 {
                    return Err(Fail::Protocol("packet length is not a block multiple"));
                }
                let mut body = vec![0u8; len + 32];
                self.read_full(&mut body)?;
                let (ct, mac) = body.split_at_mut(len);
                let Some(Dir::Ctr { ctr, mac_key, .. }) = self.inn.as_mut() else { return Err(Fail::Closed) };
                let want = hmac_sha256(&**mac_key, &[&seq.to_be_bytes(), &h, ct]);
                if !ct_eq(&want, mac) {
                    return Err(Fail::Protocol("bad MAC"));
                }
                ctr.apply_keystream(ct);
                (unpad(ct)?, 4 + len + 32)
            }
            Some((Cipher::Ctr, false)) => {
                let mut first = [0u8; 16];
                self.read_full(&mut first)?;
                let Some(Dir::Ctr { ctr, .. }) = self.inn.as_mut() else { return Err(Fail::Closed) };
                ctr.apply_keystream(&mut first);
                let len = u32::from_be_bytes([first[0], first[1], first[2], first[3]]) as usize;
                check_len(len, 12)?;
                if (len + 4) % 16 != 0 {
                    return Err(Fail::Protocol("packet length is not a block multiple"));
                }
                let mut rest = vec![0u8; len + 4 - 16 + 32];
                self.read_full(&mut rest)?;
                let (ct, mac) = rest.split_at_mut(len + 4 - 16);
                let Some(Dir::Ctr { ctr, mac_key, .. }) = self.inn.as_mut() else { return Err(Fail::Closed) };
                ctr.apply_keystream(ct);
                let mut plain = first.to_vec();
                plain.extend_from_slice(ct);
                let want = hmac_sha256(&**mac_key, &[&seq.to_be_bytes(), &plain]);
                if !ct_eq(&want, mac) {
                    return Err(Fail::Protocol("bad MAC"));
                }
                (unpad(&plain[4..])?, plain.len() + 32)
            }
            Some((Cipher::Gcm, _)) => {
                let mut h = [0u8; 4];
                self.read_full(&mut h)?;
                let len = u32::from_be_bytes(h) as usize;
                check_len(len, 16)?;
                if len % 16 != 0 {
                    return Err(Fail::Protocol("packet length is not a block multiple"));
                }
                let mut body = vec![0u8; len + 16];
                self.read_full(&mut body)?;
                let (ct, tag) = body.split_at_mut(len);
                let Some(Dir::Gcm { aead, iv }) = self.inn.as_mut() else { return Err(Fail::Closed) };
                let ok = aead.decrypt_in_place_detached(Nonce::from_slice(&iv[..]), &h, ct, Tag::from_slice(tag)).is_ok();
                gcm_step(iv);
                if !ok {
                    return Err(Fail::Protocol("bad MAC"));
                }
                (unpad(ct)?, 4 + len + 16)
            }
            Some((Cipher::Chacha, _)) => {
                let mut h = [0u8; 4];
                self.read_full(&mut h)?;
                let Some(Dir::Chacha { hdr, .. }) = self.inn.as_ref() else { return Err(Fail::Closed) };
                let mut lenb = h;
                chacha_at(hdr, seq).apply_keystream(&mut lenb);
                let len = u32::from_be_bytes(lenb) as usize;
                check_len(len, 8)?;
                if len % 8 != 0 {
                    return Err(Fail::Protocol("packet length is not a block multiple"));
                }
                let mut body = vec![0u8; len + 16];
                self.read_full(&mut body)?;
                let Some(Dir::Chacha { main, .. }) = self.inn.as_ref() else { return Err(Fail::Closed) };
                let (ct, tag) = body.split_at_mut(len);
                let (want, mut c) = poly_tag(main, seq, &[&h, ct]);
                if !ct_eq(&want, tag) {
                    return Err(Fail::Protocol("bad MAC"));
                }
                c.apply_keystream(ct);
                (unpad(ct)?, 4 + len + 16)
            }
        };
        self.bytes += used as u64;
        Ok(payload)
    }

    /// Send a packet. While a re-key is under way everything but key-exchange and disconnect messages waits and goes out under the new keys.
    pub fn write_packet(&mut self, payload: &[u8]) -> R<()> {
        if self.hold && !matches!(payload.first(), Some(1) | Some(20..=49)) {
            if self.held.len() >= MAX_HELD {
                return Err(Fail::Protocol("the peer did not answer the re-key"));
            }
            self.held.push(payload.to_vec());
            return Ok(());
        }
        self.write_now(payload)
    }

    fn write_now(&mut self, payload: &[u8]) -> R<()> {
        if payload.len() > MAX_PACKET - 64 {
            return Err(Fail::Protocol("outgoing packet too large"));
        }
        let seq = self.wseq;
        self.wseq = self.wseq.wrapping_add(1);
        // the block the padding fills to, and whether the length field counts in it
        let (block, counted) = match &self.out {
            Some(Dir::Ctr { etm: true, .. }) | Some(Dir::Gcm { .. }) => (16, 0),
            Some(Dir::Chacha { .. }) => (8, 0),
            _ => (16, 4),
        };
        let mut pad = block - ((1 + payload.len() + counted) % block);
        if pad < 4 {
            pad += block;
        }
        // a packet is at least 16 bytes with its length field
        if 4 + 1 + payload.len() + pad < 16 {
            pad += block;
        }
        let len = 1 + payload.len() + pad;
        let mut pkt = Vec::with_capacity(4 + len + 32);
        pkt.extend_from_slice(&(len as u32).to_be_bytes());
        pkt.push(pad as u8);
        pkt.extend_from_slice(payload);
        let off = (seq as usize) % (256 - 32);
        pkt.extend_from_slice(&self.pad[off..off + pad]);
        match &mut self.out {
            None => {}
            Some(Dir::Ctr { ctr, mac_key, etm: true }) => {
                ctr.apply_keystream(&mut pkt[4..]);
                let mac = hmac_sha256(&**mac_key, &[&seq.to_be_bytes(), &pkt]);
                pkt.extend_from_slice(&mac);
            }
            Some(Dir::Ctr { ctr, mac_key, etm: false }) => {
                let mac = hmac_sha256(&**mac_key, &[&seq.to_be_bytes(), &pkt]);
                ctr.apply_keystream(&mut pkt);
                pkt.extend_from_slice(&mac);
            }
            Some(Dir::Gcm { aead, iv }) => {
                let (head, rest) = pkt.split_at_mut(4);
                let tag = aead.encrypt_in_place_detached(Nonce::from_slice(&iv[..]), head, rest).map_err(|_| Fail::Io("encryption failed".into()))?;
                gcm_step(iv);
                pkt.extend_from_slice(&tag);
            }
            Some(Dir::Chacha { main, hdr }) => {
                chacha_at(hdr, seq).apply_keystream(&mut pkt[..4]);
                let (_, mut c) = poly_tag(main, seq, &[]);
                c.apply_keystream(&mut pkt[4..]);
                let (tag, _) = poly_tag(main, seq, &[&pkt]);
                pkt.extend_from_slice(&tag);
            }
        }
        self.bytes += pkt.len() as u64;
        self.write_raw(&pkt)
    }

    /// Send the packets that waited for the new keys.
    fn release_held(&mut self) -> R<()> {
        self.hold = false;
        self.bytes = 0;
        for p in std::mem::take(&mut self.held) {
            self.write_now(&p)?;
        }
        Ok(())
    }

    fn unimplemented_reply(&mut self, seq: u32) -> R<()> {
        let mut p = vec![UNIMPLEMENTED];
        put_u32(&mut p, seq);
        self.write_packet(&p)
    }
}

fn check_len(len: usize, min: usize) -> R<()> {
    if len < min || len > MAX_PACKET {
        Err(Fail::Protocol("packet length out of range"))
    } else {
        Ok(())
    }
}

/// The payload of a decrypted packet body (`padding_length || payload || padding`).
fn unpad(body: &[u8]) -> R<Vec<u8>> {
    let pad = *body.first().ok_or(Fail::Protocol("empty packet"))? as usize;
    if pad < 4 || pad + 1 >= body.len() {
        return Err(Fail::Protocol("bad padding"));
    }
    Ok(body[1..body.len() - pad].to_vec())
}

// ------------------------------------------------------------------ key exchange

fn name_list(r: &mut Reader) -> R<Vec<String>> {
    let b = r.string().ok_or(Fail::Protocol("cut-short name list"))?;
    let s = std::str::from_utf8(b).map_err(|_| Fail::Protocol("name list is not text"))?;
    Ok(s.split(',').map(str::to_string).collect())
}

fn our_kexinit(ec: bool) -> R<Vec<u8>> {
    let mut p = vec![KEXINIT];
    p.extend_from_slice(&rand::<16>()?);
    for list in [KEX_ALGS, if ec { "ssh-ed25519,ecdsa-sha2-nistp256" } else { HOSTKEY_ALG }, &CIPHERS.join(","), &CIPHERS.join(","), &MACS.join(","), &MACS.join(","), "none", "none", "", ""] {
        put_str(&mut p, list.as_bytes());
    }
    p.push(0);
    put_u32(&mut p, 0);
    Ok(p)
}

struct Chosen {
    ext_info: bool,
    host_ec: bool,
    kex_ok: bool,
    strict: bool,
    cipher_c2s: Cipher,
    cipher_s2c: Cipher,
    mac_c2s_etm: bool,
    mac_s2c_etm: bool,
}

/// Compare the client's KEXINIT with what is offered: the first algorithm of the client's list that the server has wins (RFC 4253, 7.1).
fn negotiate(client: &[u8], ec: bool) -> R<Chosen> {
    let mut r = Reader { b: client.get(17..).ok_or(Fail::Protocol("short KEXINIT"))? };
    let kex = name_list(&mut r)?;
    let host = name_list(&mut r)?;
    let enc_c = name_list(&mut r)?;
    let enc_s = name_list(&mut r)?;
    let mac_c = name_list(&mut r)?;
    let mac_s = name_list(&mut r)?;
    let comp_c = name_list(&mut r)?;
    let comp_s = name_list(&mut r)?;
    let pick = |theirs: &[String], ours: &[&str]| theirs.iter().find(|t| ours.contains(&t.as_str())).cloned();
    let kex_pick = pick(&kex, &["curve25519-sha256", "curve25519-sha256@libssh.org"]);
    if kex_pick.is_none() {
        return Err(Fail::Protocol("no common key exchange"));
    }
    let host_pick = if ec { pick(&host, &[HOSTKEY_ALG, "ecdsa-sha2-nistp256"]) } else { pick(&host, &[HOSTKEY_ALG]) };
    let Some(host_pick) = host_pick else {
        return Err(Fail::Protocol("no common host key type (ssh-ed25519 needed)"));
    };
    let cc = pick(&enc_c, &CIPHERS).and_then(|n| Cipher::from_name(&n));
    let cs = pick(&enc_s, &CIPHERS).and_then(|n| Cipher::from_name(&n));
    let (Some(cipher_c2s), Some(cipher_s2c)) = (cc, cs) else { return Err(Fail::Protocol("no common cipher (aes256-ctr, aes256-gcm or chacha20-poly1305 needed)")) };
    // the MAC list only counts for a cipher that has no tag of its own
    let need_mac = |c: Cipher| c == Cipher::Ctr;
    let mc = pick(&mac_c, &MACS);
    let ms = pick(&mac_s, &MACS);
    if (need_mac(cipher_c2s) && mc.is_none()) || (need_mac(cipher_s2c) && ms.is_none()) {
        return Err(Fail::Protocol("no common MAC"));
    }
    let (mc, ms) = (mc.unwrap_or_default(), ms.unwrap_or_default());
    if !comp_c.iter().any(|c| c == "none") || !comp_s.iter().any(|c| c == "none") {
        return Err(Fail::Protocol("no common compression setting"));
    }
    Ok(Chosen { ext_info: kex.iter().any(|k| k == "ext-info-c"), host_ec: host_pick == "ecdsa-sha2-nistp256", kex_ok: true, cipher_c2s, cipher_s2c, strict: kex.iter().any(|k| k == "kex-strict-c-v00@openssh.com"), mac_c2s_etm: mc.contains("etm"), mac_s2c_etm: ms.contains("etm") })
}

fn mpint(o: &mut Vec<u8>, be: &[u8]) {
    let t: &[u8] = &be[be.iter().position(|b| *b != 0).unwrap_or(be.len())..];
    if t.first().is_some_and(|b| b & 0x80 != 0) {
        put_u32(o, t.len() as u32 + 1);
        o.push(0);
    } else {
        put_u32(o, t.len() as u32);
    }
    o.extend_from_slice(t);
}

fn derive(k: &[u8], h: &[u8], x: u8, sid: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut km = Vec::new();
    mpint(&mut km, k);
    let mut d = Sha256::new();
    d.update(&km);
    d.update(h);
    d.update([x]);
    d.update(sid);
    Zeroizing::new(d.finalize().into())
}

/// The exchange hash H and the shared secret, from the client's side values (pure, so the test can run both ends).
pub fn exchange_hash(vc: &str, vs: &str, ic: &[u8], is: &[u8], ks: &[u8], qc: &[u8], qs: &[u8], k: &[u8]) -> [u8; 32] {
    let mut b = Vec::new();
    for s in [vc.as_bytes(), vs.as_bytes(), ic, is, ks, qc, qs] {
        put_str(&mut b, s);
    }
    mpint(&mut b, k);
    Sha256::digest(&b).into()
}

pub struct Server<'a> {
    pub host_key: &'a KeyPair,
    /// An ECDSA P-256 host key for clients that have no Ed25519 host key (F-FILE-13); `None`: only Ed25519 is offered.
    pub ec_host: Option<&'a crate::sshalg::EcHostKey>,
    pub gate: &'a dyn Gate,
    pub peer: IpAddr,
}

impl<S: Read + Write> Transport<S> {
    /// Run one key exchange. `client_init` is the client's KEXINIT when it already arrived (a re-key); `sid` is the session id of the first
    /// exchange (empty on the first). Returns the session id and whether strict mode is on.
    fn kex(&mut self, host: &KeyPair, ec: Option<&crate::sshalg::EcHostKey>, vs: &str, vc: &str, client_init: Option<Vec<u8>>, sid: &[u8], strict_known: Option<bool>, sent: Option<Vec<u8>>) -> R<(Vec<u8>, bool)> {
        // `sent` is our KEXINIT when we started this exchange ourselves (the volume re-key)
        let first = sid.is_empty();
        let ours = match sent {
            Some(o) => o,
            None => {
                let o = our_kexinit(ec.is_some())?;
                self.write_packet(&o)?;
                o
            }
        };
        let theirs = match client_init {
            Some(p) => p,
            None => self.read_packet()?,
        };
        if theirs.first() != Some(&KEXINIT) {
            return Err(Fail::Protocol("expected KEXINIT"));
        }
        let ch = negotiate(&theirs, ec.is_some())?;
        debug_assert!(ch.kex_ok);
        let strict = strict_known.unwrap_or(ch.strict);
        let init = self.read_packet()?;
        let mut r = Reader { b: init.get(1..).ok_or(Fail::Protocol("short packet"))? };
        if init.first() != Some(&KEX_ECDH_INIT) {
            return Err(Fail::Protocol("expected KEX_ECDH_INIT"));
        }
        let qc = r.string().ok_or(Fail::Protocol("cut-short client key"))?;
        let qc: [u8; 32] = qc.try_into().map_err(|_| Fail::Protocol("client key is not 32 bytes"))?;
        let mut sk = rand::<32>()?;
        let qs = x25519_dalek::x25519(sk, x25519_dalek::X25519_BASEPOINT_BYTES);
        let k = Zeroizing::new(x25519_dalek::x25519(sk, qc));
        sk.fill(0);
        if k.iter().all(|b| *b == 0) {
            return Err(Fail::Protocol("weak client key"));
        }
        let ks = match (ch.host_ec, ec) {
            (true, Some(e)) => e.public_blob(),
            _ => host.public_blob(),
        };
        let h = exchange_hash(vc, vs, &theirs, &ours, &ks, &qc, &qs, &*k);
        // I_C is the client's KEXINIT and I_S ours: the call above passes (theirs, ours) in the order of the hash
        let sid = if sid.is_empty() { h.to_vec() } else { sid.to_vec() };
        let sig = match (ch.host_ec, ec) {
            (true, Some(e)) => e.sign_wire(&h),
            _ => host.sign_wire(&h),
        };
        let mut reply = vec![KEX_ECDH_REPLY];
        put_str(&mut reply, &ks);
        put_str(&mut reply, &qs);
        put_str(&mut reply, &sig);
        self.write_packet(&reply)?;
        self.write_packet(&[NEWKEYS])?;
        if strict {
            self.wseq = 0;
        }
        // the keys for the packets sent from now on (server to client: D, B, F)
        self.out = Some(make_dir(ch.cipher_s2c, ch.mac_s2c_etm, &*k, &h, &sid, b'D', b'B', b'F'));
        let nk = self.read_packet()?;
        if nk.first() != Some(&NEWKEYS) {
            return Err(Fail::Protocol("expected NEWKEYS"));
        }
        if strict {
            self.rseq = 0;
        }
        self.inn = Some(make_dir(ch.cipher_c2s, ch.mac_c2s_etm, &*k, &h, &sid, b'C', b'A', b'E'));
        // RFC 8308: tell a client that asked which login signatures we check (without this OpenSSH 8.8 and later offers no RSA key)
        if ch.ext_info && first {
            let mut p = vec![EXT_INFO];
            put_u32(&mut p, 1);
            put_str(&mut p, b"server-sig-algs");
            put_str(&mut p, b"ssh-ed25519,ecdsa-sha2-nistp256,ecdsa-sha2-nistp384,rsa-sha2-512,rsa-sha2-256");
            self.write_packet(&p)?;
        }
        self.release_held()?;
        Ok((sid, strict))
    }
}

// ------------------------------------------------------------------ one connection

/// Close our end of the channel and wait (briefly) for the client's close, so it does not see the connection drop under it.
fn finish<S: Read + Write>(t: &mut Transport<S>, remote: u32) -> R<()> {
    let mut cl = vec![CHANNEL_CLOSE];
    put_u32(&mut cl, remote);
    t.write_packet(&cl)?;
    t.set_deadline(Duration::from_secs(5));
    for _ in 0..16 {
        match t.read_packet() {
            Ok(p) if p.first() == Some(&CHANNEL_CLOSE) || p.first() == Some(&DISCONNECT) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    Ok(())
}

fn disconnect<S: Read + Write>(t: &mut Transport<S>, code: u32, why: &str) {
    let mut p = vec![DISCONNECT];
    put_u32(&mut p, code);
    put_str(&mut p, why.as_bytes());
    put_str(&mut p, b"");
    let _ = t.write_packet(&p);
}

fn parse_user(r: &mut Reader) -> R<String> {
    let b = r.string().ok_or(Fail::Protocol("cut-short user"))?;
    if b.len() > 64 {
        return Err(Fail::Protocol("user name too long"));
    }
    String::from_utf8(b.to_vec()).map_err(|_| Fail::Protocol("user name is not text"))
}

/// Serve one connection until it ends. Never panics on hostile input; returns why it ended (for tests; the caller ignores it).
pub fn serve<S: Read + Write>(stream: S, srv: &Server, stop: Arc<AtomicBool>) -> Fail {
    serve_with(stream, srv, stop, REKEY_BYTES)
}

/// [`serve`] with the volume that triggers the server's own re-key (tests use a small one).
pub fn serve_with<S: Read + Write>(stream: S, srv: &Server, stop: Arc<AtomicBool>, rekey_after: u64) -> Fail {
    let mut t = match Transport::new(stream, stop.clone()) {
        Ok(t) => t,
        Err(e) => return e,
    };
    t.rekey_after = rekey_after;
    match serve_inner(&mut t, srv, &stop) {
        Err(e) => {
            if let Fail::Protocol(why) = &e {
                srv.gate.audit("sftp.protocol_error", why);
                disconnect(&mut t, 2, why);
            }
            e
        }
        Ok(()) => Fail::Closed,
    }
}

fn serve_inner<S: Read + Write>(t: &mut Transport<S>, srv: &Server, _stop: &Arc<AtomicBool>) -> R<()> {
    t.write_raw(format!("{VERSION_LINE}\r\n").as_bytes())?;
    let vc = t.read_version()?;
    let (sid, strict) = t.kex(srv.host_key, srv.ec_host, VERSION_LINE, &vc, None, &[], None, None)?;
    // services
    let p = t.read_packet()?;
    if p.first() != Some(&SERVICE_REQUEST) {
        return Err(Fail::Protocol("expected a service request"));
    }
    let mut r = Reader { b: &p[1..] };
    if r.string() != Some(b"ssh-userauth") {
        return Err(Fail::Protocol("unknown service"));
    }
    let mut acc = vec![SERVICE_ACCEPT];
    put_str(&mut acc, b"ssh-userauth");
    t.write_packet(&acc)?;
    let user = authenticate(t, srv, &sid)?;
    srv.gate.audit("sftp.login", &format!("{} from {}", crate::sftp_session::clean(&user), srv.peer));
    t.set_deadline(IDLE);
    connection(t, srv, &user, &vc, &sid, strict)
}

fn failure_packet() -> Vec<u8> {
    let mut f = vec![USERAUTH_FAILURE];
    put_str(&mut f, b"publickey,password");
    f.push(0);
    f
}

fn authenticate<S: Read + Write>(t: &mut Transport<S>, srv: &Server, sid: &[u8]) -> R<String> {
    let mut failures = 0;
    loop {
        let p = t.read_packet()?;
        match p.first() {
            Some(&IGNORE) | Some(&DEBUG) => continue,
            Some(&DISCONNECT) => return Err(Fail::Closed),
            Some(&USERAUTH_REQUEST) => {}
            Some(&SERVICE_REQUEST) => {
                // a second request for the same service is harmless
                let mut acc = vec![SERVICE_ACCEPT];
                put_str(&mut acc, b"ssh-userauth");
                t.write_packet(&acc)?;
                continue;
            }
            _ => return Err(Fail::Protocol("expected a login request")),
        }
        let mut r = Reader { b: &p[1..] };
        let user = parse_user(&mut r)?;
        let service = r.string().ok_or(Fail::Protocol("cut-short request"))?;
        let method = r.string().ok_or(Fail::Protocol("cut-short request"))?;
        if service != b"ssh-connection" {
            return Err(Fail::Protocol("unknown service"));
        }
        let fail;
        match method {
            b"none" => {
                t.write_packet(&failure_packet())?;
                continue;
            }
            b"publickey" => {
                let has_sig = r.b.first().copied().ok_or(Fail::Protocol("cut-short request"))? != 0;
                r.b = &r.b[1..];
                let algo = r.string().ok_or(Fail::Protocol("cut-short request"))?;
                let blob = r.string().ok_or(Fail::Protocol("cut-short request"))?;
                let ty = crate::sshalg::blob_type(blob).unwrap_or_default();
                if !crate::sshalg::algo_fits(algo, &ty) || !crate::sshalg::valid_user_blob(blob) || !srv.gate.key_ok(&user, blob, srv.peer) {
                    fail = true;
                } else if !has_sig {
                    let mut ok = vec![USERAUTH_PK_OK];
                    put_str(&mut ok, algo);
                    put_str(&mut ok, blob);
                    t.write_packet(&ok)?;
                    continue;
                } else {
                    let sig = r.string().ok_or(Fail::Protocol("cut-short request"))?;
                    let mut signed = Vec::new();
                    put_str(&mut signed, sid);
                    signed.push(USERAUTH_REQUEST);
                    put_str(&mut signed, user.as_bytes());
                    put_str(&mut signed, service);
                    put_str(&mut signed, b"publickey");
                    signed.push(1);
                    put_str(&mut signed, algo);
                    put_str(&mut signed, blob);
                    if crate::sshalg::verify_user(algo, blob, sig, &signed) {
                        t.write_packet(&[USERAUTH_SUCCESS])?;
                        return Ok(user);
                    }
                    fail = true;
                }
            }
            b"password" => {
                let change = r.b.first().copied().ok_or(Fail::Protocol("cut-short request"))?;
                r.b = &r.b[1..];
                if change != 0 {
                    fail = true;
                } else {
                    let pw = r.string().ok_or(Fail::Protocol("cut-short request"))?;
                    if pw.len() > 1024 {
                        return Err(Fail::Protocol("password too long"));
                    }
                    let pw = Zeroizing::new(String::from_utf8_lossy(pw).into_owned());
                    if srv.gate.password_ok(&user, &pw, srv.peer) {
                        t.write_packet(&[USERAUTH_SUCCESS])?;
                        return Ok(user);
                    }
                    fail = true;
                }
            }
            _ => {
                t.write_packet(&failure_packet())?;
                continue;
            }
        }
        if fail {
            failures += 1;
            srv.gate.audit("sftp.login_failed", &format!("{} from {}", crate::sftp_session::clean(&user), srv.peer));
            if failures >= MAX_AUTH_FAILURES {
                return Err(Fail::Protocol("too many failed logins"));
            }
            std::thread::sleep(Duration::from_millis(if cfg!(test) { 1 } else { 1000 }));
            t.write_packet(&failure_packet())?;
        }
    }
}

/// `blob` is `string "ssh-ed25519", string key`; `sig` is `string "ssh-ed25519", string signature`.
pub fn verify_ed25519(blob: &[u8], sig: &[u8], data: &[u8]) -> bool {
    let mut b = Reader { b: blob };
    let mut s = Reader { b: sig };
    let (Some(t1), Some(key), Some(t2), Some(sg)) = (b.string(), b.string(), s.string(), s.string()) else { return false };
    if t1 != b"ssh-ed25519" || t2 != b"ssh-ed25519" || !b.b.is_empty() || !s.b.is_empty() {
        return false;
    }
    let (Ok(key), Ok(sg)) = (<[u8; 32]>::try_from(key), <[u8; 64]>::try_from(sg)) else { return false };
    VerifyingKey::from_bytes(&key).is_ok_and(|k| k.verify(data, &Signature::from_bytes(&sg)).is_ok())
}

fn connection<S: Read + Write>(t: &mut Transport<S>, srv: &Server, user: &str, vc: &str, sid: &[u8], mut strict: bool) -> R<()> {
    struct Chan {
        remote: u32,
        window: u64,
        max_packet: u32,
        recv_window: u32,
        session: Option<Session>,
        out: std::collections::VecDeque<u8>,
    }
    let mut chan: Option<Chan> = None;
    // the client said it is done: close as soon as what is queued has been sent
    let mut closing = false;
    // our KEXINIT of a re-key we started, waiting for the client's
    let mut ours_sent: Option<Vec<u8>> = None;
    loop {
        if ours_sent.is_none() && !t.hold && t.bytes >= t.rekey_after {
            let o = our_kexinit(srv.ec_host.is_some())?;
            t.write_packet(&o)?;
            t.hold = true;
            srv.gate.audit("sftp.rekey", "volume");
            ours_sent = Some(o);
        }
        let seq = t.rseq;
        let p = t.read_packet()?;
        t.set_deadline(IDLE);
        let mut r = Reader { b: p.get(1..).ok_or(Fail::Protocol("empty packet"))? };
        match p[0] {
            IGNORE | DEBUG | UNIMPLEMENTED => {}
            DISCONNECT => return Ok(()),
            KEXINIT => {
                // either the client starts a re-key, or it answers the one we started
                let sent = std::mem::take(&mut ours_sent);
                let (_, s) = t.kex(srv.host_key, srv.ec_host, VERSION_LINE, vc, Some(p.clone()), sid, Some(strict), sent)?;
                strict = s;
            }
            GLOBAL_REQUEST => {
                let _name = r.string();
                if r.b.first().is_some_and(|w| *w != 0) {
                    t.write_packet(&[REQUEST_FAILURE])?;
                }
            }
            CHANNEL_OPEN => {
                let kind = r.string().ok_or(Fail::Protocol("cut-short request"))?;
                let sender = r.u32().ok_or(Fail::Protocol("cut-short request"))?;
                let window = r.u32().ok_or(Fail::Protocol("cut-short request"))?;
                let maxp = r.u32().ok_or(Fail::Protocol("cut-short request"))?;
                if kind != b"session" || chan.is_some() || maxp < 64 {
                    let mut f = vec![CHANNEL_OPEN_FAILURE];
                    put_u32(&mut f, sender);
                    put_u32(&mut f, 1);
                    put_str(&mut f, b"only one sftp session at a time");
                    put_str(&mut f, b"");
                    t.write_packet(&f)?;
                    continue;
                }
                chan = Some(Chan { remote: sender, window: window as u64, max_packet: maxp.min(CHAN_MAX_PACKET), recv_window: WINDOW, session: None, out: Default::default() });
                let mut ok = vec![CHANNEL_OPEN_CONFIRMATION];
                put_u32(&mut ok, sender);
                put_u32(&mut ok, 0);
                put_u32(&mut ok, WINDOW);
                put_u32(&mut ok, CHAN_MAX_PACKET);
                t.write_packet(&ok)?;
            }
            CHANNEL_REQUEST => {
                let to = r.u32().ok_or(Fail::Protocol("cut-short request"))?;
                let kind = r.string().ok_or(Fail::Protocol("cut-short request"))?;
                let want = r.b.first().copied().ok_or(Fail::Protocol("cut-short request"))? != 0;
                r.b = &r.b[1..];
                let Some(c) = chan.as_mut().filter(|_| to == 0) else { return Err(Fail::Protocol("request for an unknown channel")) };
                let mut good = false;
                if kind == b"subsystem" && c.session.is_none() {
                    if r.string() == Some(b"sftp") {
                        c.session = srv.gate.session(user, srv.peer);
                        good = c.session.is_some();
                    }
                } else if kind != b"subsystem" {
                    srv.gate.audit("sftp.refused_request", &String::from_utf8_lossy(&kind[..kind.len().min(20)]).chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect::<String>());
                }
                if want {
                    let mut a = vec![if good { CHANNEL_SUCCESS } else { CHANNEL_FAILURE }];
                    put_u32(&mut a, c.remote);
                    t.write_packet(&a)?;
                }
            }
            CHANNEL_DATA => {
                let to = r.u32().ok_or(Fail::Protocol("cut-short data"))?;
                let data = r.string().ok_or(Fail::Protocol("cut-short data"))?;
                let Some(c) = chan.as_mut().filter(|_| to == 0) else { return Err(Fail::Protocol("data for an unknown channel")) };
                if data.len() > CHAN_MAX_PACKET as usize || data.len() as u64 > c.recv_window as u64 {
                    return Err(Fail::Protocol("data over the window"));
                }
                c.recv_window -= data.len() as u32;
                if c.out.len() > OUT_CAP {
                    return Err(Fail::Protocol("answers are not being taken"));
                }
                if let Some(s) = c.session.as_mut() {
                    let answer = s.feed(data);
                    c.out.extend(answer);
                    if s.closed {
                        return finish(t, c.remote);
                    }
                } else {
                    return Err(Fail::Protocol("data before the sftp subsystem"));
                }
                if c.recv_window < WINDOW / 2 {
                    let add = WINDOW - c.recv_window;
                    c.recv_window = WINDOW;
                    let mut w = vec![CHANNEL_WINDOW_ADJUST];
                    put_u32(&mut w, c.remote);
                    put_u32(&mut w, add);
                    t.write_packet(&w)?;
                }
            }
            CHANNEL_EXTENDED_DATA => return Err(Fail::Protocol("unexpected extended data")),
            CHANNEL_WINDOW_ADJUST => {
                let to = r.u32().ok_or(Fail::Protocol("cut-short request"))?;
                let add = r.u32().ok_or(Fail::Protocol("cut-short request"))?;
                if let Some(c) = chan.as_mut().filter(|_| to == 0) {
                    c.window = (c.window + add as u64).min(u32::MAX as u64);
                }
            }
            CHANNEL_EOF => closing = true,
            CHANNEL_CLOSE => {
                if let Some(c) = chan.take() {
                    let mut cl = vec![CHANNEL_CLOSE];
                    put_u32(&mut cl, c.remote);
                    t.write_packet(&cl)?;
                }
                return Ok(());
            }
            _ => t.unimplemented_reply(seq)?,
        }
        // send what the window allows
        if let Some(c) = chan.as_mut() {
            while !c.out.is_empty() && c.window > 0 {
                let n = c.out.len().min(c.window as usize).min(c.max_packet as usize);
                let chunk: Vec<u8> = c.out.drain(..n).collect();
                let mut d = vec![CHANNEL_DATA];
                put_u32(&mut d, c.remote);
                put_str(&mut d, &chunk);
                t.write_packet(&d)?;
                c.window -= n as u64;
            }
            if closing && c.out.is_empty() {
                return finish(t, c.remote);
            }
        }
    }
}

/// A listening address that is not local network is refused before anything is read.
pub fn peer_allowed(a: &std::net::SocketAddr) -> bool {
    is_local_peer(a.ip())
}

/// Apply the short read timeout the transport polls with.
pub fn prepare(stream: &TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    stream.set_nodelay(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// An in-memory pipe end: reads what the other end wrote, `WouldBlock` when empty.
    #[derive(Clone)]
    struct End {
        rx: Arc<Mutex<VecDeque<u8>>>,
        tx: Arc<Mutex<VecDeque<u8>>>,
    }
    fn pipe() -> (End, End) {
        let (a, b) = (Arc::new(Mutex::new(VecDeque::new())), Arc::new(Mutex::new(VecDeque::new())));
        (End { rx: a.clone(), tx: b.clone() }, End { rx: b, tx: a })
    }
    impl Read for End {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let mut q = self.rx.lock().unwrap();
            if q.is_empty() {
                drop(q);
                std::thread::sleep(Duration::from_micros(100));
                return Err(std::io::ErrorKind::WouldBlock.into());
            }
            let n = buf.len().min(q.len());
            for b in buf.iter_mut().take(n) {
                *b = q.pop_front().unwrap();
            }
            Ok(n)
        }
    }
    impl Write for End {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.tx.lock().unwrap().extend(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn keys(etm: bool) -> (Transport<End>, Transport<End>) {
        keys_with(Cipher::Ctr, etm)
    }

    fn keys_with(cipher: Cipher, etm: bool) -> (Transport<End>, Transport<End>) {
        let (a, b) = pipe();
        let stop = Arc::new(AtomicBool::new(false));
        let mut x = Transport::new(a, stop.clone()).unwrap();
        let mut y = Transport::new(b, stop).unwrap();
        x.deadline = Instant::now() + Duration::from_secs(5);
        y.deadline = x.deadline;
        let dir = |k: u8| make_dir(cipher, etm, &[k; 32], &[k.wrapping_add(1); 32], &[k.wrapping_add(2); 32], b'C', b'A', b'E');
        x.out = Some(dir(7));
        y.inn = Some(dir(7));
        y.out = Some(dir(9));
        x.inn = Some(dir(9));
        (x, y)
    }

    #[test]
    fn hmac_matches_rfc_4231_case_1() {
        let got = hmac_sha256(&[0x0b; 20], &[b"Hi ", b"There"]);
        let hex: String = got.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
    }

    #[test]
    fn packets_round_trip_in_both_mac_modes_and_every_size() {
        for etm in [true, false] {
            let (mut x, mut y) = keys(etm);
            for n in [0usize, 1, 15, 16, 17, 255, 4000, 30000] {
                let payload: Vec<u8> = (0..n).map(|i| (i % 251) as u8 + 1).chain(std::iter::once(94)).collect();
                x.write_packet(&payload).unwrap();
                assert_eq!(y.read_packet().unwrap(), payload, "etm={etm} n={n}");
                y.write_packet(&payload).unwrap();
                assert_eq!(x.read_packet().unwrap(), payload);
            }
        }
    }

    #[test]
    fn a_changed_byte_is_a_bad_mac_in_both_modes() {
        for etm in [true, false] {
            for pos in [0usize, 5, 20, 40] {
                let (mut x, mut y) = keys(etm);
                x.write_packet(&[94, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21]).unwrap();
                y.s.rx.lock().unwrap()[pos] ^= 0x01;
                match y.read_packet() {
                    Err(Fail::Protocol(_)) => {}
                    other => panic!("etm={etm} pos={pos}: {other:?}"),
                }
            }
        }
    }

    #[test]
    fn hostile_lengths_are_refused_before_anything_is_read() {
        for etm in [true, false] {
            let (_x, mut y) = keys(etm);
            // a clear length of 4 GB (encrypted modes: only the ETM form shows it in the clear; the other form decrypts to noise, also refused)
            y.s.rx.lock().unwrap().extend([0xff, 0xff, 0xff, 0xff, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
            assert!(matches!(y.read_packet(), Err(Fail::Protocol(_))), "etm={etm}");
        }
        // unencrypted: zero, too small, too large, and a length that is not followed by that many bytes
        for head in [[0u8, 0, 0, 0], [0, 0, 0, 4], [0, 0, 0x88, 0xb9], [0x7f, 0xff, 0xff, 0xff]] {
            let (a, b) = pipe();
            let mut t = Transport::new(b, Arc::new(AtomicBool::new(false))).unwrap();
            a.tx.lock().unwrap().extend(head);
            assert!(matches!(t.read_packet(), Err(Fail::Protocol(_))), "{head:?}");
        }
        let (a, b) = pipe();
        let mut t = Transport::new(b, Arc::new(AtomicBool::new(false))).unwrap();
        t.deadline = Instant::now() + Duration::from_millis(100);
        a.tx.lock().unwrap().extend([0, 0, 0, 40, 1, 2, 3]);
        assert!(matches!(t.read_packet(), Err(Fail::Closed)), "a cut-short packet ends at the deadline, it does not hang or panic");
    }

    #[test]
    fn bad_padding_is_refused() {
        assert!(unpad(&[]).is_err());
        assert!(unpad(&[3, 1, 2, 3, 4]).is_err());
        assert!(unpad(&[200, 1, 2]).is_err());
        assert!(unpad(&[4, 1, 0, 0, 0, 0]).is_ok());
    }

    #[test]
    fn local_peers_only() {
        for ok in ["127.0.0.1", "10.1.2.3", "172.16.0.9", "192.168.1.7", "169.254.3.3", "::1", "fe80::1", "fd00::5", "::ffff:192.168.1.2"] {
            assert!(is_local_peer(ok.parse().unwrap()), "{ok}");
        }
        for bad in ["8.8.8.8", "172.32.0.1", "100.64.0.1", "2001:db8::1", "::ffff:8.8.8.8", "11.0.0.1"] {
            assert!(!is_local_peer(bad.parse().unwrap()), "{bad}");
        }
    }

    #[test]
    fn mpint_encoding_follows_rfc_4251() {
        let mut o = Vec::new();
        mpint(&mut o, &[0, 0, 0x80, 1]);
        assert_eq!(o, [0, 0, 0, 3, 0, 0x80, 1]);
        let mut o = Vec::new();
        mpint(&mut o, &[0, 0x7f]);
        assert_eq!(o, [0, 0, 0, 1, 0x7f]);
        let mut o = Vec::new();
        mpint(&mut o, &[0, 0]);
        assert_eq!(o, [0, 0, 0, 0]);
    }

    #[test]
    fn signatures_check_only_the_right_key_and_text() {
        let k = KeyPair::from_seed([5; 32], "t");
        let other = KeyPair::from_seed([6; 32], "o");
        let sig = k.sign_wire(b"data");
        assert!(verify_ed25519(&k.public_blob(), &sig, b"data"));
        assert!(!verify_ed25519(&k.public_blob(), &sig, b"datb"));
        assert!(!verify_ed25519(&other.public_blob(), &sig, b"data"));
        assert!(!verify_ed25519(&k.public_blob(), &sig[..sig.len() - 1], b"data"));
        assert!(!verify_ed25519(&[], &[], b""));
        assert!(!verify_ed25519(&k.public_blob()[..10], &sig, b"data"));
    }

    struct NoGate;
    impl Gate for NoGate {
        fn key_ok(&self, _: &str, _: &[u8], _: IpAddr) -> bool {
            false
        }
        fn password_ok(&self, _: &str, _: &str, _: IpAddr) -> bool {
            false
        }
        fn session(&self, _: &str, _: IpAddr) -> Option<Session> {
            None
        }
        fn audit(&self, _: &str, _: &str) {}
    }

    fn run_against(input: Vec<u8>) -> Fail {
        let (a, b) = pipe();
        a.tx.lock().unwrap().extend(input);
        let key = KeyPair::from_seed([1; 32], "h");
        let srv = Server { host_key: &key, gate: &NoGate, ec_host: None, peer: "127.0.0.1".parse().unwrap() };
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        // the pipe never closes, so the stop switch ends a read that waits for more bytes
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            flag.store(true, Ordering::Relaxed);
        });
        let r = serve(b, &srv, stop);
        t.join().unwrap();
        r
    }

    fn clear_packet(payload: &[u8]) -> Vec<u8> {
        let pad = 8 - ((5 + payload.len()) % 8) + 8;
        let mut p = ((1 + payload.len() + pad) as u32).to_be_bytes().to_vec();
        p.push(pad as u8);
        p.extend_from_slice(payload);
        p.extend(std::iter::repeat(0).take(pad));
        p
    }

    fn kexinit_with(lists: [&str; 10]) -> Vec<u8> {
        let mut p = vec![KEXINIT];
        p.extend_from_slice(&[0; 16]);
        for l in lists {
            put_str(&mut p, l.as_bytes());
        }
        p.push(0);
        put_u32(&mut p, 0);
        p
    }

    #[test]
    fn hostile_beginnings_end_the_connection_without_a_panic() {
        // identification too long, not SSH 2, no newline, and nothing at all
        assert!(matches!(run_against([b"SSH-2.0-".as_slice(), &[b'a'; 400], b"\n"].concat()), Fail::Protocol(_)));
        assert!(matches!(run_against(b"SSH-1.5-old\r\n".to_vec()), Fail::Protocol(_)));
        assert!(matches!(run_against(b"GET / HTTP/1.1\r\n\r\n".repeat(20)), Fail::Protocol(_)));
        assert!(matches!(run_against(b"SSH-2.0-x".to_vec()), Fail::Closed));
        assert!(matches!(run_against(Vec::new()), Fail::Closed));
        // good identification, then a 4 GB packet, then a packet that is not a KEXINIT
        assert!(matches!(run_against([b"SSH-2.0-x\r\n".as_slice(), &[0xff; 8]].concat()), Fail::Protocol(_)));
        assert!(matches!(run_against([b"SSH-2.0-x\r\n".as_slice(), &clear_packet(&[SERVICE_REQUEST, 0, 0, 0, 0])].concat()), Fail::Protocol(_)));
        // a KEXINIT that offers nothing we have
        let none = kexinit_with(["a", "b", "c", "d", "e", "f", "none", "none", "", ""]);
        assert!(matches!(run_against([b"SSH-2.0-x\r\n".as_slice(), &clear_packet(&none)].concat()), Fail::Protocol(why) if why.contains("key exchange")));
        // the right key exchange but only RSA host keys
        let rsa = kexinit_with(["curve25519-sha256", "ssh-rsa", CIPHER_CTR, CIPHER_CTR, MACS[0], MACS[0], "none", "none", "", ""]);
        assert!(matches!(run_against([b"SSH-2.0-x\r\n".as_slice(), &clear_packet(&rsa)].concat()), Fail::Protocol(why) if why.contains("host key")));
        // a weak (all zero) client key is refused
        let good = kexinit_with(["curve25519-sha256", "ssh-ed25519", CIPHER_CTR, CIPHER_CTR, MACS[0], MACS[0], "none", "none", "", ""]);
        let mut init = vec![KEX_ECDH_INIT];
        put_str(&mut init, &[0u8; 32]);
        let input = [b"SSH-2.0-x\r\n".as_slice(), &clear_packet(&good), &clear_packet(&init)].concat();
        assert!(matches!(run_against(input), Fail::Protocol(why) if why.contains("weak")));
        // a client key of the wrong size
        let mut init = vec![KEX_ECDH_INIT];
        put_str(&mut init, &[1u8; 5]);
        let input = [b"SSH-2.0-x\r\n".as_slice(), &clear_packet(&good), &clear_packet(&init)].concat();
        assert!(matches!(run_against(input), Fail::Protocol(why) if why.contains("32")));
    }

    #[test]
    fn the_two_aead_ciphers_round_trip_every_size_and_count_packets_in_step() {
        for cipher in [Cipher::Gcm, Cipher::Chacha] {
            let (mut x, mut y) = keys_with(cipher, true);
            for n in [0usize, 1, 7, 8, 15, 16, 17, 255, 4000, 30000, 30000] {
                let payload: Vec<u8> = (0..n).map(|i| (i % 251) as u8 + 1).chain(std::iter::once(94)).collect();
                x.write_packet(&payload).unwrap();
                assert_eq!(y.read_packet().unwrap(), payload, "{cipher:?} n={n}");
                y.write_packet(&payload).unwrap();
                assert_eq!(x.read_packet().unwrap(), payload);
            }
            // the same payload twice never looks the same on the wire (the nonce moves on)
            x.write_packet(&[94, 1, 2, 3]).unwrap();
            let first: Vec<u8> = x.s.tx.lock().unwrap().drain(..).collect();
            x.write_packet(&[94, 1, 2, 3]).unwrap();
            let second: Vec<u8> = x.s.tx.lock().unwrap().drain(..).collect();
            assert_ne!(first[4..], second[4..], "{cipher:?}");
        }
    }

    #[test]
    fn a_changed_byte_in_an_aead_packet_is_refused_wherever_it_is() {
        for cipher in [Cipher::Gcm, Cipher::Chacha] {
            let size = {
                let (mut x, _y) = keys_with(cipher, true);
                x.write_packet(&[94; 40]).unwrap();
                let n = x.s.tx.lock().unwrap().len();
                n
            };
            for pos in 0..size {
                let (mut x, mut y) = keys_with(cipher, true);
                x.write_packet(&[94; 40]).unwrap();
                y.s.rx.lock().unwrap()[pos] ^= 0x01;
                // a flipped length byte of ChaCha is a wrong length (refused or waiting for bytes that never come), anything else a bad tag
                y.deadline = Instant::now() + Duration::from_millis(30);
                match y.read_packet() {
                    Err(Fail::Protocol(_)) | Err(Fail::Closed) => {}
                    other => panic!("{cipher:?} pos={pos}: {other:?}"),
                }
            }
        }
    }

    #[test]
    fn keys_for_chacha_are_sixty_four_bytes_and_the_two_halves_differ() {
        let k = derive_64(&[1; 32], &[2; 32], b'C', &[3; 32]);
        assert_ne!(k[..32], k[32..]);
        assert_eq!(k[..32], *derive(&[1; 32], &[2; 32], b'C', &[3; 32]));
        // a different letter, a different key
        assert_ne!(k[..], derive_64(&[1; 32], &[2; 32], b'D', &[3; 32])[..]);
    }

    // ---------------------------------------------------------------- a whole conversation with a test client

    struct TestGate {
        jail: Mutex<Option<crate::sftp_fs::Jail>>,
        log: Arc<Mutex<Vec<String>>>,
    }
    impl Gate for TestGate {
        fn key_ok(&self, _: &str, _: &[u8], _: IpAddr) -> bool {
            false
        }
        fn password_ok(&self, _: &str, password: &str, _: IpAddr) -> bool {
            password == "pw"
        }
        fn session(&self, _: &str, _: IpAddr) -> Option<Session> {
            let jail = self.jail.lock().unwrap().take()?;
            Some(Session::new(jail, Box::new(|_, _| {})))
        }
        fn audit(&self, event: &str, detail: &str) {
            self.log.lock().unwrap().push(format!("{event} {detail}"));
        }
    }

    const CLIENT_VERSION: &str = "SSH-2.0-test";

    fn client_kexinit(cipher: &str, strict: bool) -> Vec<u8> {
        let mut p = vec![KEXINIT];
        p.extend_from_slice(&[9; 16]);
        let kex = if strict { "curve25519-sha256,kex-strict-c-v00@openssh.com" } else { "curve25519-sha256" };
        for list in [kex, "ssh-ed25519", cipher, cipher, MACS[0], MACS[0], "none", "none", "", ""] {
            put_str(&mut p, list.as_bytes());
        }
        p.push(0);
        put_u32(&mut p, 0);
        p
    }

    /// The client half of a key exchange; returns the session id.
    fn client_kex(t: &mut Transport<End>, cipher: Cipher, name: &str, sid: &[u8], server_init: Option<Vec<u8>>, mine: Option<Vec<u8>>, strict: bool) -> Vec<u8> {
        let first = sid.is_empty();
        let mine = match mine {
            Some(m) => m,
            None => {
                let m = client_kexinit(name, first);
                t.write_packet(&m).unwrap();
                m
            }
        };
        let theirs = server_init.unwrap_or_else(|| t.read_packet().unwrap());
        assert_eq!(theirs[0], KEXINIT);
        let mut sk = [0u8; 32];
        sk.iter_mut().enumerate().for_each(|(i, b)| *b = (i as u8).wrapping_mul(7).wrapping_add(3 + mine.len() as u8));
        let qc = x25519_dalek::x25519(sk, x25519_dalek::X25519_BASEPOINT_BYTES);
        let mut init = vec![KEX_ECDH_INIT];
        put_str(&mut init, &qc);
        t.write_packet(&init).unwrap();
        let reply = t.read_packet().unwrap();
        assert_eq!(reply[0], KEX_ECDH_REPLY);
        let mut r = Reader { b: &reply[1..] };
        let (ks, qs, sig) = (r.string().unwrap().to_vec(), r.string().unwrap().to_vec(), r.string().unwrap().to_vec());
        let k = x25519_dalek::x25519(sk, qs.as_slice().try_into().unwrap());
        let h = exchange_hash(CLIENT_VERSION, VERSION_LINE, &mine, &theirs, &ks, &qc, &qs, &k);
        assert!(verify_ed25519(&ks, &sig, &h), "the host signature must verify");
        let sid = if first { h.to_vec() } else { sid.to_vec() };
        t.write_packet(&[NEWKEYS]).unwrap();
        if strict {
            t.wseq = 0;
        }
        t.out = Some(make_dir(cipher, true, &k, &h, &sid, b'C', b'A', b'E'));
        let nk = t.read_packet().unwrap();
        assert_eq!(nk[0], NEWKEYS);
        if strict {
            t.rseq = 0;
        }
        t.inn = Some(make_dir(cipher, true, &k, &h, &sid, b'D', b'B', b'F'));
        t.bytes = 0;
        sid
    }

    fn sftp_frame(payload: &[u8]) -> Vec<u8> {
        let mut f = (payload.len() as u32).to_be_bytes().to_vec();
        f.extend_from_slice(payload);
        f
    }

    fn data_packet(remote: u32, bytes: &[u8]) -> Vec<u8> {
        let mut d = vec![CHANNEL_DATA];
        put_u32(&mut d, remote);
        put_str(&mut d, bytes);
        d
    }

    /// Log in with the password, open the sftp subsystem, send `requests` REALPATH calls; re-key whenever the server asks.
    /// Returns how many times the server started a re-key and the audit lines.
    fn converse(cipher: Cipher, name: &str, rekey_after: u64, requests: u32) -> (u32, Vec<String>) {
        let (_w, jail) = crate::testworld::world("ssh-aead");
        let log = Arc::new(Mutex::new(Vec::new()));
        let gate = TestGate { jail: Mutex::new(Some(jail)), log: log.clone() };
        let key = KeyPair::from_seed([1; 32], "h");
        let srv = Server { host_key: &key, gate: &gate, ec_host: None, peer: "127.0.0.1".parse().unwrap() };
        let (a, b) = pipe();
        let stop = Arc::new(AtomicBool::new(false));
        let mut rekeys = 0;
        std::thread::scope(|sc| {
            let st = stop.clone();
            struct StopOnDrop(Arc<AtomicBool>);
            impl Drop for StopOnDrop {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::Relaxed);
                }
            }
            // a failed assertion on this side must still end the server thread, or the scope waits for it for ever
            let _guard = StopOnDrop(stop.clone());
            let h = sc.spawn(|| serve_with(b, &srv, st, rekey_after));
            let mut t = Transport::new(a, stop.clone()).unwrap();
            t.deadline = Instant::now() + Duration::from_secs(20);
            t.write_raw(format!("{CLIENT_VERSION}\r\n").as_bytes()).unwrap();
            assert_eq!(t.read_version().unwrap(), VERSION_LINE);
            let sid = client_kex(&mut t, cipher, name, &[], None, None, true);
            let mut req = vec![SERVICE_REQUEST];
            put_str(&mut req, b"ssh-userauth");
            t.write_packet(&req).unwrap();
            assert_eq!(t.read_packet().unwrap()[0], SERVICE_ACCEPT);
            let mut au = vec![USERAUTH_REQUEST];
            for s in [b"amy".as_slice(), b"ssh-connection", b"password"] {
                put_str(&mut au, s);
            }
            au.push(0);
            put_str(&mut au, b"pw");
            t.write_packet(&au).unwrap();
            assert_eq!(t.read_packet().unwrap()[0], USERAUTH_SUCCESS);
            let mut open = vec![CHANNEL_OPEN];
            put_str(&mut open, b"session");
            for v in [5u32, 1 << 20, 32_768] {
                put_u32(&mut open, v);
            }
            t.write_packet(&open).unwrap();
            let conf = t.read_packet().unwrap();
            assert_eq!(conf[0], CHANNEL_OPEN_CONFIRMATION);
            let mut sub = vec![CHANNEL_REQUEST];
            put_u32(&mut sub, 0);
            put_str(&mut sub, b"subsystem");
            sub.push(1);
            put_str(&mut sub, b"sftp");
            t.write_packet(&sub).unwrap();
            assert_eq!(t.read_packet().unwrap()[0], CHANNEL_SUCCESS);
            let mut init = vec![FXP_INIT_TEST];
            put_u32(&mut init, 3);
            t.write_packet(&data_packet(0, &sftp_frame(&init))).unwrap();
            let mut sftp_in = Vec::new();
            let mut cur_sid = sid.clone();
            // wait for one whole SFTP answer; a server KEXINIT on the way is answered at once
            let mut answer = |t: &mut Transport<End>, want: u8| {
                loop {
                    let p = wait_packet(t);
                    match p[0] {
                        KEXINIT => {
                            rekeys += 1;
                            cur_sid = client_kex(t, cipher, name, &cur_sid, Some(p), None, true);
                        }
                        CHANNEL_DATA => {
                            let mut r = Reader { b: &p[1..] };
                            let _ = r.u32().unwrap();
                            sftp_in.extend_from_slice(r.string().unwrap());
                            if sftp_in.len() >= 4 {
                                let n = u32::from_be_bytes(sftp_in[..4].try_into().unwrap()) as usize;
                                if sftp_in.len() >= 4 + n {
                                    let body: Vec<u8> = sftp_in.drain(..4 + n).skip(4).collect();
                                    assert!(body[0] == want || body[0] == 101, "got {}", body[0]);
                                    return;
                                }
                            }
                        }
                        CHANNEL_WINDOW_ADJUST => {}
                        other => panic!("unexpected message {other}"),
                    }
                }
            };
            answer(&mut t, 2);
            for i in 0..requests {
                let mut rp = vec![16u8];
                put_u32(&mut rp, i);
                put_str(&mut rp, b"/some/where/../here.txt");
                t.write_packet(&data_packet(0, &sftp_frame(&rp))).unwrap();
                answer(&mut t, 104);
            }
            let mut cl = vec![CHANNEL_CLOSE];
            put_u32(&mut cl, 0);
            let _ = t.write_packet(&cl);
            stop.store(true, Ordering::Relaxed);
            let _ = h.join();
        });
        let lines = log.lock().unwrap().clone();
        (rekeys, lines)
    }

    const FXP_INIT_TEST: u8 = 1;

    /// The next packet from the server, waiting up to ten seconds for it to start (writes into the pipe are whole packets).
    fn wait_packet(t: &mut Transport<End>) -> Vec<u8> {
        let until = Instant::now() + Duration::from_secs(10);
        while t.s.rx.lock().unwrap().is_empty() {
            assert!(Instant::now() < until, "the server said nothing");
            std::thread::sleep(Duration::from_millis(1));
        }
        t.read_packet().unwrap()
    }

    #[test]
    fn a_client_that_asks_and_never_opens_its_window_is_cut_off_instead_of_buffered_without_end() {
        let (_w, jail) = crate::testworld::world("ssh-window");
        let log = Arc::new(Mutex::new(Vec::new()));
        let gate = TestGate { jail: Mutex::new(Some(jail)), log: log.clone() };
        let key = KeyPair::from_seed([1; 32], "h");
        let srv = Server { host_key: &key, gate: &gate, ec_host: None, peer: "127.0.0.1".parse().unwrap() };
        let (a, b) = pipe();
        let stop = Arc::new(AtomicBool::new(false));
        let ended = std::thread::scope(|sc| {
            struct StopOnDrop(Arc<AtomicBool>);
            impl Drop for StopOnDrop {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::Relaxed);
                }
            }
            let _guard = StopOnDrop(stop.clone());
            let st = stop.clone();
            let h = sc.spawn(|| serve_with(b, &srv, st, REKEY_BYTES));
            let mut t = Transport::new(a, stop.clone()).unwrap();
            t.deadline = Instant::now() + Duration::from_secs(60);
            t.write_raw(format!("{CLIENT_VERSION}\r\n").as_bytes()).unwrap();
            assert_eq!(t.read_version().unwrap(), VERSION_LINE);
            client_kex(&mut t, Cipher::Chacha, CIPHER_CHACHA, &[], None, None, true);
            let mut req = vec![SERVICE_REQUEST];
            put_str(&mut req, b"ssh-userauth");
            t.write_packet(&req).unwrap();
            assert_eq!(t.read_packet().unwrap()[0], SERVICE_ACCEPT);
            let mut au = vec![USERAUTH_REQUEST];
            for s in [b"amy".as_slice(), b"ssh-connection", b"password"] {
                put_str(&mut au, s);
            }
            au.push(0);
            put_str(&mut au, b"pw");
            t.write_packet(&au).unwrap();
            assert_eq!(t.read_packet().unwrap()[0], USERAUTH_SUCCESS);
            // a channel whose window for the server's answers is ZERO: nothing the server says can be sent
            let mut open = vec![CHANNEL_OPEN];
            put_str(&mut open, b"session");
            for v in [5u32, 0, 32_768] {
                put_u32(&mut open, v);
            }
            t.write_packet(&open).unwrap();
            assert_eq!(wait_packet(&mut t)[0], CHANNEL_OPEN_CONFIRMATION);
            let mut sub = vec![CHANNEL_REQUEST];
            put_u32(&mut sub, 0);
            put_str(&mut sub, b"subsystem");
            sub.push(1);
            put_str(&mut sub, b"sftp");
            t.write_packet(&sub).unwrap();
            assert_eq!(wait_packet(&mut t)[0], CHANNEL_SUCCESS);
            let mut init = vec![FXP_INIT_TEST];
            put_u32(&mut init, 3);
            t.write_packet(&data_packet(0, &sftp_frame(&init))).unwrap();
            // thousands of small path questions per packet, far more answers than the server may hold
            let mut frames = Vec::new();
            for i in 0..2000u32 {
                let mut rp = vec![16u8];
                put_u32(&mut rp, i);
                put_str(&mut rp, b"/a");
                frames.extend_from_slice(&sftp_frame(&rp));
            }
            let mut sent = 0;
            while sent < 2000 && !h.is_finished() {
                if t.write_packet(&data_packet(0, &frames)).is_err() {
                    break;
                }
                sent += 1;
                // the server's window adjusts and nothing else come back; read them so the pipe stays small
                while !t.s.rx.lock().unwrap().is_empty() {
                    let _ = t.read_packet();
                }
            }
            let until = Instant::now() + Duration::from_secs(20);
            while !h.is_finished() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(h.is_finished(), "the server must give up on a client that never takes its answers (sent {sent} packets)");
            (sent, h.join().unwrap())
        });
        assert!(matches!(&ended.1, Fail::Protocol(w) if w.contains("not being taken")), "{:?}", ended.1);
        assert!(ended.0 < 2000, "cut off after a bounded amount of questions: {}", ended.0);
    }

    #[test]
    fn a_login_and_sftp_conversation_works_with_each_cipher() {
        for (cipher, name) in [(Cipher::Chacha, CIPHER_CHACHA), (Cipher::Gcm, CIPHER_GCM), (Cipher::Ctr, CIPHER_CTR)] {
            let (rekeys, log) = converse(cipher, name, REKEY_BYTES, 20);
            assert_eq!(rekeys, 0, "{name}: a short conversation needs no re-key");
            assert!(log.iter().any(|l| l.starts_with("sftp.login amy")), "{name}: {log:?}");
        }
    }

    #[test]
    fn the_server_re_keys_after_the_volume_limit_and_the_conversation_goes_on() {
        for (cipher, name) in [(Cipher::Chacha, CIPHER_CHACHA), (Cipher::Gcm, CIPHER_GCM), (Cipher::Ctr, CIPHER_CTR)] {
            let (rekeys, log) = converse(cipher, name, 4_000, 150);
            assert!(rekeys >= 2, "{name}: re-keys {rekeys}");
            assert!(log.iter().any(|l| l == "sftp.rekey volume"), "{name}: {log:?}");
        }
    }

    // ---------------------------------------------------------------- a deterministic fuzz loop for what comes after the key exchange

    /// A small fixed-seed random source (SplitMix64): the same cases on every run and every machine.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }
        fn bytes(&mut self, n: usize) -> Vec<u8> {
            (0..n).map(|_| self.next() as u8).collect()
        }
        /// Between none and `max - 1` random bytes.
        fn some(&mut self, max: usize) -> Vec<u8> {
            let n = self.below(max);
            self.bytes(n)
        }
    }

    /// A stream that ends where the input ends and keeps what is written.
    struct Mem {
        input: std::io::Cursor<Vec<u8>>,
        out: Vec<u8>,
    }
    impl Read for Mem {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(buf)
        }
    }
    impl Write for Mem {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.out.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn sstr(o: &mut Vec<u8>, s: &[u8]) {
        put_str(o, s);
    }

    /// A message the conversation after the key exchange knows, with fields that are valid or nearly so.
    fn seed_message(r: &mut Rng) -> Vec<u8> {
        let mut p = Vec::new();
        match r.below(14) {
            0 => {
                p.push(USERAUTH_REQUEST);
                for s in [b"amy".as_slice(), b"ssh-connection", b"password"] {
                    sstr(&mut p, s);
                }
                p.push(0);
                sstr(&mut p, if r.below(2) == 0 { b"pw" } else { b"nope" });
            }
            1 => {
                p.push(USERAUTH_REQUEST);
                for s in [b"amy".as_slice(), b"ssh-connection", b"publickey"] {
                    sstr(&mut p, s);
                }
                p.push(r.below(2) as u8);
                sstr(&mut p, b"ssh-ed25519");
                sstr(&mut p, &r.bytes(51));
                sstr(&mut p, &r.bytes(83));
            }
            2 => {
                p.push(USERAUTH_REQUEST);
                for s in [b"amy".as_slice(), b"ssh-connection", b"none"] {
                    sstr(&mut p, s);
                }
            }
            3 => {
                p.push(SERVICE_REQUEST);
                sstr(&mut p, b"ssh-userauth");
            }
            4 => {
                p.push(CHANNEL_OPEN);
                sstr(&mut p, if r.below(4) == 0 { b"direct-tcpip" } else { b"session" });
                for v in [r.below(10) as u32, 1 << 20, 32_768] {
                    put_u32(&mut p, v);
                }
            }
            5 | 6 => {
                p.push(CHANNEL_REQUEST);
                put_u32(&mut p, r.below(2) as u32);
                sstr(&mut p, [b"subsystem".as_slice(), b"exec", b"pty-req", b"env", b"shell"][r.below(5)]);
                p.push(r.below(2) as u8);
                sstr(&mut p, if r.below(3) == 0 { b"scp" } else { b"sftp" });
            }
            7..=9 => {
                p.push(CHANNEL_DATA);
                put_u32(&mut p, r.below(2) as u32);
                // SFTP frames: INIT, REALPATH, OPEN, READ, WRITE, READDIR, REMOVE ... with random bodies
                let mut f = Vec::new();
                let ty = [1u8, 16, 3, 5, 6, 11, 12, 13, 14, 17, 18, 200, 99][r.below(13)];
                f.push(ty);
                put_u32(&mut f, r.below(5) as u32);
                f.extend(r.some(40));
                let mut frame = (f.len() as u32).to_be_bytes().to_vec();
                frame.extend(f);
                sstr(&mut p, &frame);
            }
            10 => {
                p.push(CHANNEL_WINDOW_ADJUST);
                put_u32(&mut p, 0);
                put_u32(&mut p, r.next() as u32);
            }
            11 => p.extend([CHANNEL_EOF, 0, 0, 0, 0]),
            12 => {
                p.push(GLOBAL_REQUEST);
                sstr(&mut p, b"tcpip-forward");
                p.push(1);
            }
            _ => {
                // a re-key request with a list that offers nothing, or something that is no message at all
                p.push([KEXINIT, IGNORE, DEBUG, 0, 255, CHANNEL_EXTENDED_DATA, CHANNEL_CLOSE][r.below(7)]);
                p.extend(r.some(30));
            }
        }
        p
    }

    /// Damage a message the way a hostile peer would: flip bits, cut it, pad it, write extreme lengths into it.
    fn mutate(r: &mut Rng, mut p: Vec<u8>) -> Vec<u8> {
        for _ in 0..r.below(4) {
            if p.is_empty() {
                p.push(r.next() as u8);
            }
            match r.below(6) {
                0 => {
                    let i = r.below(p.len());
                    p[i] ^= 1 << r.below(8);
                }
                1 => p.truncate(r.below(p.len() + 1)),
                2 => p.extend(r.some(64)),
                3 if p.len() >= 4 => {
                    let i = r.below(p.len() - 3);
                    p[i..i + 4].copy_from_slice(&[[0xff; 4], [0; 4], [0x7f, 0xff, 0xff, 0xff], [0, 1, 0, 0]][r.below(4)]);
                }
                4 => {
                    let i = r.below(p.len());
                    p[i] = r.next() as u8;
                }
                _ => {
                    let i = r.below(p.len());
                    p.insert(i, r.next() as u8);
                }
            }
        }
        p
    }

    fn dir_of(cipher: Cipher, etm: bool, k: u8) -> Dir {
        make_dir(cipher, etm, &[k; 32], &[k.wrapping_add(1); 32], &[k.wrapping_add(2); 32], b'C', b'A', b'E')
    }

    /// The bytes a peer would put on the wire for these payloads under the keys of `dir_of(.., 7)`.
    fn wire(cipher: Cipher, etm: bool, payloads: &[Vec<u8>]) -> Vec<u8> {
        let (a, _b) = pipe();
        let mut x = Transport::new(a.clone(), Arc::new(AtomicBool::new(false))).unwrap();
        x.out = Some(dir_of(cipher, etm, 7));
        let mut bytes = Vec::new();
        for p in payloads {
            if p.len() > MAX_PACKET - 64 {
                continue;
            }
            x.write_packet(p).unwrap();
            bytes.extend(a.tx.lock().unwrap().drain(..));
        }
        bytes
    }

    fn server_side(bytes: Vec<u8>, cipher: Cipher, etm: bool) -> Transport<Mem> {
        let mut t = Transport::new(Mem { input: std::io::Cursor::new(bytes), out: Vec::new() }, Arc::new(AtomicBool::new(false))).unwrap();
        t.inn = Some(dir_of(cipher, etm, 7));
        t.out = Some(dir_of(cipher, etm, 9));
        t
    }

    struct FuzzGate {
        world: std::path::PathBuf,
    }
    impl Gate for FuzzGate {
        fn key_ok(&self, _: &str, _: &[u8], _: IpAddr) -> bool {
            true
        }
        fn password_ok(&self, _: &str, p: &str, _: IpAddr) -> bool {
            p == "pw"
        }
        fn session(&self, _: &str, _: IpAddr) -> Option<Session> {
            let h = self.world.clone();
            let (h1, h2) = (h.clone(), h.clone());
            let jail = crate::sftp_fs::Jail::new(h, vec![], Arc::new(move |p: &std::path::Path| p.starts_with(&h1)), Arc::new(move |p: &std::path::Path| p.starts_with(&h2)));
            Some(Session::new(jail, Box::new(|_, _| {})))
        }
        fn audit(&self, _: &str, _: &str) {}
    }

    /// What every run of the fuzz must satisfy: it ends (the input is finite), it does not panic, and it ends with a plain reason.
    fn ended_cleanly(r: Result<(), Fail>) {
        match r {
            Ok(()) | Err(Fail::Closed) | Err(Fail::Protocol(_)) | Err(Fail::Io(_)) => {}
        }
    }

    #[test]
    fn fuzzed_messages_after_the_key_exchange_never_panic_or_hang_in_any_cipher() {
        let (w, _) = crate::testworld::world("fuzz-after-kex");
        let gate = FuzzGate { world: w.home.clone() };
        let key = KeyPair::from_seed([1; 32], "h");
        let srv = Server { host_key: &key, gate: &gate, ec_host: None, peer: "127.0.0.1".parse().unwrap() };
        let mut r = Rng(0x5eed_1234_5678);
        let (mut auth_runs, mut conn_runs, mut sftp_answers) = (0u32, 0u32, 0usize);
        for case in 0..3_000u32 {
            let (cipher, etm) = [(Cipher::Chacha, true), (Cipher::Gcm, true), (Cipher::Ctr, true), (Cipher::Ctr, false)][case as usize % 4];
            let n = 1 + r.below(7);
            let mut msgs: Vec<Vec<u8>> = Vec::new();
            if case % 2 == 1 {
                // a good login first, so the fuzz reaches the channel and the sftp code
                let mut login = vec![USERAUTH_REQUEST];
                for s in [b"amy".as_slice(), b"ssh-connection", b"password"] {
                    sstr(&mut login, s);
                }
                login.push(0);
                sstr(&mut login, b"pw");
                msgs.push(login);
                let mut open = vec![CHANNEL_OPEN];
                sstr(&mut open, b"session");
                for v in [3u32, 1 << 20, 32_768] {
                    put_u32(&mut open, v);
                }
                msgs.push(open);
                let mut sub = vec![CHANNEL_REQUEST];
                put_u32(&mut sub, 0);
                sstr(&mut sub, b"subsystem");
                sub.push(1);
                sstr(&mut sub, b"sftp");
                msgs.push(sub);
            }
            for _ in 0..n {
                let m = seed_message(&mut r);
                msgs.push(if r.below(3) == 0 { m } else { mutate(&mut r, m) });
            }
            let mut bytes = wire(cipher, etm, &msgs);
            // and sometimes damage the encrypted stream itself
            if r.below(4) == 0 && !bytes.is_empty() {
                for _ in 0..1 + r.below(3) {
                    let i = r.below(bytes.len());
                    bytes[i] ^= 1 << r.below(8);
                }
            }
            if r.below(10) == 0 {
                bytes.truncate(r.below(bytes.len() + 1));
            }
            let mut t = server_side(bytes.clone(), cipher, etm);
            if case % 2 == 0 {
                auth_runs += 1;
                ended_cleanly(authenticate(&mut t, &srv, b"session-id").map(|_| ()));
            } else {
                conn_runs += 1;
                // the login in front is part of the input: run the login code first, then the channel code on what is left
                let user = authenticate(&mut t, &srv, b"session-id");
                if let Ok(user) = user {
                    t.set_deadline(IDLE);
                    ended_cleanly(connection(&mut t, &srv, &user, "SSH-2.0-fuzz", b"session-id", r.below(2) == 0));
                }
            }
            sftp_answers += t.s.out.len();
        }
        assert_eq!((auth_runs, conn_runs), (1_500, 1_500));
        eprintln!("fuzz wrote back {sftp_answers} bytes");
        assert!(sftp_answers > 50_000, "the fuzz must get far enough to be answered ({sftp_answers} bytes written back)");
    }

    #[test]
    fn a_stream_cut_or_damaged_anywhere_ends_the_packet_reader_cleanly_in_every_cipher() {
        let mut r = Rng(42);
        for (cipher, etm) in [(Cipher::Chacha, true), (Cipher::Gcm, true), (Cipher::Ctr, true), (Cipher::Ctr, false)] {
            let msgs: Vec<Vec<u8>> = (0..6).map(|_| seed_message(&mut r)).collect();
            let good = wire(cipher, etm, &msgs);
            // cut at every length
            for cut in 0..good.len() {
                let mut t = server_side(good[..cut].to_vec(), cipher, etm);
                while t.read_packet().is_ok() {}
            }
            // damage every byte, one at a time: the first packet that includes it is refused, never a panic
            for pos in 0..good.len() {
                let mut bad = good.clone();
                bad[pos] ^= 0x40;
                let mut t = server_side(bad, cipher, etm);
                let mut ok = 0;
                while let Ok(p) = t.read_packet() {
                    ok += 1;
                    assert!(p.len() <= MAX_PACKET);
                    if ok > msgs.len() {
                        break;
                    }
                }
                assert!(ok <= msgs.len(), "{cipher:?} {etm} pos={pos}");
            }
        }
    }
}
