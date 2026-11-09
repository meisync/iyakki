//! A small SSH 2 server that offers one thing: the `sftp` subsystem (see `docs/development/SFTP-SERVER.md`). In-house, over primitives already in
//! the tree: X25519 (`curve25519-sha256`), Ed25519 (host key and public-key login), AES-256-CTR with HMAC-SHA-256 (encrypt-then-MAC and the
//! older MAC-then-encrypt form), SHA-256.
//!
//! What it does not do, on purpose: no shell, no command, no pseudo-terminal, no port or agent forwarding, no X11, no compression, no
//! other ciphers or key types (clients without Ed25519 host keys, AES-CTR and HMAC-SHA-256 cannot connect; every current OpenSSH, Dropbear,
//! PuTTY, WinSCP and libssh2 can). "Strict key exchange" (the Terrapin fix) is on when the client offers it. Re-keying by the client works.
//!
//! Safety limits: a login grace time, four failed logins per connection, packets over 35 000 bytes are refused before they are read, a
//! channel window and packet size are enforced both ways, one session channel at a time, an idle timeout, and every read is checked against
//! the stop switch. Secrets (passwords, key material) are held in `Zeroizing` buffers and never reach the audit callback.

use std::io::{Read, Write};
use std::net::{IpAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aes::cipher::{KeyIvInit, StreamCipher};
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

const KEX_ALGS: &str = "curve25519-sha256,curve25519-sha256@libssh.org,kex-strict-s-v00@openssh.com";
const HOSTKEY_ALG: &str = "ssh-ed25519";
const CIPHER: &str = "aes256-ctr";
const MACS: [&str; 2] = ["hmac-sha2-256-etm@openssh.com", "hmac-sha2-256"];

// message numbers
const DISCONNECT: u8 = 1;
const IGNORE: u8 = 2;
const UNIMPLEMENTED: u8 = 3;
const DEBUG: u8 = 4;
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

struct Dir {
    ctr: Aes256Ctr,
    mac_key: Zeroizing<[u8; 32]>,
    etm: bool,
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
}

fn rand<const N: usize>() -> R<[u8; N]> {
    let mut b = [0u8; N];
    iyakki_sec::random_fill(&mut b).map_err(|_| Fail::Io("no random numbers".into()))?;
    Ok(b)
}

impl<S: Read + Write> Transport<S> {
    pub fn new(s: S, stop: Arc<AtomicBool>) -> R<Transport<S>> {
        Ok(Transport { s, rseq: 0, wseq: 0, inn: None, out: None, stop, deadline: Instant::now() + LOGIN_GRACE, pad: rand::<256>()? })
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
        let payload = match &mut self.inn {
            None => {
                let mut h = [0u8; 4];
                self.read_full(&mut h)?;
                let len = u32::from_be_bytes(h) as usize;
                check_len(len, 8)?;
                let mut body = vec![0u8; len];
                self.read_full(&mut body)?;
                unpad(&body)?
            }
            Some(d) if d.etm => {
                let mut h = [0u8; 4];
                // `d` is borrowed from self; take what is needed first
                let key = d.mac_key.clone();
                self.read_full(&mut h)?;
                let len = u32::from_be_bytes(h) as usize;
                check_len(len, 16)?;
                if len % 16 != 0 {
                    return Err(Fail::Protocol("packet length is not a block multiple"));
                }
                let mut body = vec![0u8; len + 32];
                self.read_full(&mut body)?;
                let (ct, mac) = body.split_at_mut(len);
                let want = hmac_sha256(&*key, &[&seq.to_be_bytes(), &h, ct]);
                if !ct_eq(&want, mac) {
                    return Err(Fail::Protocol("bad MAC"));
                }
                let Some(Dir { ctr, .. }) = self.inn.as_mut() else { return Err(Fail::Closed) };
                ctr.apply_keystream(ct);
                unpad(ct)?
            }
            Some(d) => {
                let key = d.mac_key.clone();
                let mut first = [0u8; 16];
                self.read_full(&mut first)?;
                let Some(Dir { ctr, .. }) = self.inn.as_mut() else { return Err(Fail::Closed) };
                ctr.apply_keystream(&mut first);
                let len = u32::from_be_bytes([first[0], first[1], first[2], first[3]]) as usize;
                check_len(len, 12)?;
                if (len + 4) % 16 != 0 {
                    return Err(Fail::Protocol("packet length is not a block multiple"));
                }
                let mut rest = vec![0u8; len + 4 - 16 + 32];
                self.read_full(&mut rest)?;
                let (ct, mac) = rest.split_at_mut(len + 4 - 16);
                let Some(Dir { ctr, .. }) = self.inn.as_mut() else { return Err(Fail::Closed) };
                ctr.apply_keystream(ct);
                let mut plain = first.to_vec();
                plain.extend_from_slice(ct);
                let want = hmac_sha256(&*key, &[&seq.to_be_bytes(), &plain]);
                if !ct_eq(&want, mac) {
                    return Err(Fail::Protocol("bad MAC"));
                }
                unpad(&plain[4..])?
            }
        };
        Ok(payload)
    }

    pub fn write_packet(&mut self, payload: &[u8]) -> R<()> {
        if payload.len() > MAX_PACKET - 64 {
            return Err(Fail::Protocol("outgoing packet too large"));
        }
        let seq = self.wseq;
        self.wseq = self.wseq.wrapping_add(1);
        let (etm, counted) = match &self.out {
            Some(d) if d.etm => (true, 0),
            Some(_) => (false, 4),
            None => (false, 4),
        };
        let mut pad = 16 - ((1 + payload.len() + counted) % 16);
        if pad < 4 {
            pad += 16;
        }
        // a packet is at least 16 bytes with its length field
        if 4 + 1 + payload.len() + pad < 16 {
            pad += 16;
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
            Some(d) if etm => {
                d.ctr.apply_keystream(&mut pkt[4..]);
                let mac = hmac_sha256(&*d.mac_key, &[&seq.to_be_bytes(), &pkt]);
                pkt.extend_from_slice(&mac);
            }
            Some(d) => {
                let mac = hmac_sha256(&*d.mac_key, &[&seq.to_be_bytes(), &pkt]);
                d.ctr.apply_keystream(&mut pkt);
                pkt.extend_from_slice(&mac);
            }
        }
        self.write_raw(&pkt)
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

fn our_kexinit() -> R<Vec<u8>> {
    let mut p = vec![KEXINIT];
    p.extend_from_slice(&rand::<16>()?);
    for list in [KEX_ALGS, HOSTKEY_ALG, CIPHER, CIPHER, &MACS.join(","), &MACS.join(","), "none", "none", "", ""] {
        put_str(&mut p, list.as_bytes());
    }
    p.push(0);
    put_u32(&mut p, 0);
    Ok(p)
}

struct Chosen {
    kex_ok: bool,
    strict: bool,
    mac_c2s_etm: bool,
    mac_s2c_etm: bool,
}

/// Compare the client's KEXINIT with what is offered: the first algorithm of the client's list that the server has wins (RFC 4253, 7.1).
fn negotiate(client: &[u8]) -> R<Chosen> {
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
    if pick(&host, &[HOSTKEY_ALG]).is_none() {
        return Err(Fail::Protocol("no common host key type (ssh-ed25519 needed)"));
    }
    if pick(&enc_c, &[CIPHER]).is_none() || pick(&enc_s, &[CIPHER]).is_none() {
        return Err(Fail::Protocol("no common cipher (aes256-ctr needed)"));
    }
    let mc = pick(&mac_c, &MACS).ok_or(Fail::Protocol("no common MAC"))?;
    let ms = pick(&mac_s, &MACS).ok_or(Fail::Protocol("no common MAC"))?;
    if !comp_c.iter().any(|c| c == "none") || !comp_s.iter().any(|c| c == "none") {
        return Err(Fail::Protocol("no common compression setting"));
    }
    Ok(Chosen { kex_ok: true, strict: kex.iter().any(|k| k == "kex-strict-c-v00@openssh.com"), mac_c2s_etm: mc.contains("etm"), mac_s2c_etm: ms.contains("etm") })
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
    pub gate: &'a dyn Gate,
    pub peer: IpAddr,
}

impl<S: Read + Write> Transport<S> {
    /// Run one key exchange. `client_init` is the client's KEXINIT when it already arrived (a re-key); `sid` is the session id of the first
    /// exchange (empty on the first). Returns the session id and whether strict mode is on.
    fn kex(&mut self, host: &KeyPair, vs: &str, vc: &str, client_init: Option<Vec<u8>>, sid: &[u8], strict_known: Option<bool>) -> R<(Vec<u8>, bool)> {
        let ours = our_kexinit()?;
        self.write_packet(&ours)?;
        let theirs = match client_init {
            Some(p) => p,
            None => self.read_packet()?,
        };
        if theirs.first() != Some(&KEXINIT) {
            return Err(Fail::Protocol("expected KEXINIT"));
        }
        let ch = negotiate(&theirs)?;
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
        let ks = host.public_blob();
        let h = exchange_hash(vc, vs, &theirs, &ours, &ks, &qc, &qs, &*k);
        // I_C is the client's KEXINIT and I_S ours: the call above passes (theirs, ours) in the order of the hash
        let sid = if sid.is_empty() { h.to_vec() } else { sid.to_vec() };
        let sig = host.sign_wire(&h);
        let mut reply = vec![KEX_ECDH_REPLY];
        put_str(&mut reply, &ks);
        put_str(&mut reply, &qs);
        put_str(&mut reply, &sig);
        self.write_packet(&reply)?;
        self.write_packet(&[NEWKEYS])?;
        if strict {
            self.wseq = 0;
        }
        // the keys for the packets sent from now on
        let iv_c = derive(&*k, &h, b'A', &sid);
        let iv_s = derive(&*k, &h, b'B', &sid);
        let key_c = derive(&*k, &h, b'C', &sid);
        let key_s = derive(&*k, &h, b'D', &sid);
        let mac_c = derive(&*k, &h, b'E', &sid);
        let mac_s = derive(&*k, &h, b'F', &sid);
        self.out = Some(Dir { ctr: Aes256Ctr::new((&*key_s).into(), (&iv_s[..16]).into()), mac_key: mac_s, etm: ch.mac_s2c_etm });
        let nk = self.read_packet()?;
        if nk.first() != Some(&NEWKEYS) {
            return Err(Fail::Protocol("expected NEWKEYS"));
        }
        if strict {
            self.rseq = 0;
        }
        self.inn = Some(Dir { ctr: Aes256Ctr::new((&*key_c).into(), (&iv_c[..16]).into()), mac_key: mac_c, etm: ch.mac_c2s_etm });
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
    let mut t = match Transport::new(stream, stop.clone()) {
        Ok(t) => t,
        Err(e) => return e,
    };
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
    let (sid, strict) = t.kex(srv.host_key, VERSION_LINE, &vc, None, &[], None)?;
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
                if algo != b"ssh-ed25519" || !srv.gate.key_ok(&user, blob, srv.peer) {
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
                    if verify_ed25519(blob, sig, &signed) {
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
    loop {
        let seq = t.rseq;
        let p = t.read_packet()?;
        t.set_deadline(IDLE);
        let mut r = Reader { b: p.get(1..).ok_or(Fail::Protocol("empty packet"))? };
        match p[0] {
            IGNORE | DEBUG | UNIMPLEMENTED => {}
            DISCONNECT => return Ok(()),
            KEXINIT => {
                let (_, s) = t.kex(srv.host_key, VERSION_LINE, vc, Some(p.clone()), sid, Some(strict))?;
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
        let (a, b) = pipe();
        let stop = Arc::new(AtomicBool::new(false));
        let mut x = Transport::new(a, stop.clone()).unwrap();
        let mut y = Transport::new(b, stop).unwrap();
        x.deadline = Instant::now() + Duration::from_secs(5);
        y.deadline = x.deadline;
        let dir = |k: u8| Dir { ctr: Aes256Ctr::new((&[k; 32]).into(), (&[k; 16]).into()), mac_key: Zeroizing::new([k.wrapping_add(1); 32]), etm };
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
        let srv = Server { host_key: &key, gate: &NoGate, peer: "127.0.0.1".parse().unwrap() };
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
        let rsa = kexinit_with(["curve25519-sha256", "ssh-rsa", CIPHER, CIPHER, MACS[0], MACS[0], "none", "none", "", ""]);
        assert!(matches!(run_against([b"SSH-2.0-x\r\n".as_slice(), &clear_packet(&rsa)].concat()), Fail::Protocol(why) if why.contains("host key")));
        // a weak (all zero) client key is refused
        let good = kexinit_with(["curve25519-sha256", "ssh-ed25519", CIPHER, CIPHER, MACS[0], MACS[0], "none", "none", "", ""]);
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
}
