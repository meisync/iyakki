//! An SSH 2 CLIENT that opens one thing: the `sftp` subsystem, for the remote files client (`remotefiles.rs`). In-house, over the primitives
//! the server side already uses (`ssh_server.rs`): `curve25519-sha256`, `ssh-ed25519` host keys and keys, `aes256-ctr` with
//! `hmac-sha2-256-etm@openssh.com` or `hmac-sha2-256`, no compression. Strict key exchange (the Terrapin fix) is used when the server
//! offers it. The server may re-key at any time (OpenSSH does after about a gigabyte): that is answered.
//!
//! * The host key is not judged here. The caller's `host_ok(kind, fingerprint)` says whether it is the pinned one; if not the connection
//!   ends before any password or key signature is sent, with [`SshErr::HostKey`] (the caller shows the fingerprint card or the refusal).
//! * Servers that only offer RSA/ECDSA host keys, or other ciphers, cannot be used (said plainly in the error). Every current OpenSSH,
//!   Dropbear and this program's own server offer the set above.
//! * Secrets (password, key) are only held in `Zeroizing` buffers and never in an error text.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aes::cipher::{KeyIvInit, StreamCipher};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::ssh_server::{exchange_hash, hmac_sha256, verify_ed25519, MAX_PACKET};
use crate::sshkey::{fingerprint_of_blob, put_str, put_u32, KeyPair, Reader};

type Aes256Ctr = ctr::Ctr128BE<aes::Aes256>;

const VERSION_LINE: &str = "SSH-2.0-IYAKKI_Files";
const KEX_ALGS: &str = "curve25519-sha256,curve25519-sha256@libssh.org,kex-strict-c-v00@openssh.com";
const MACS: [&str; 2] = ["hmac-sha2-256-etm@openssh.com", "hmac-sha2-256"];
const WINDOW: u32 = 2 * 1024 * 1024;
const CHAN_PACKET: u32 = 32_768;
const IDLE: Duration = Duration::from_secs(60);

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
const USERAUTH_BANNER: u8 = 53;
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

#[derive(Debug)]
pub enum SshErr {
    /// The server's key is not the pinned one (or nothing is pinned yet). Nothing secret was sent.
    HostKey { kind: String, fingerprint: String },
    Auth(String),
    Other(String),
}

impl std::fmt::Display for SshErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SshErr::HostKey { fingerprint, .. } => write!(f, "the server's identity {fingerprint} is not trusted"),
            SshErr::Auth(s) | SshErr::Other(s) => f.write_str(s),
        }
    }
}

impl From<String> for SshErr {
    fn from(s: String) -> SshErr {
        SshErr::Other(s)
    }
}
impl From<&str> for SshErr {
    fn from(s: &str) -> SshErr {
        SshErr::Other(s.into())
    }
}

struct Dir {
    ctr: Aes256Ctr,
    mac_key: Zeroizing<[u8; 32]>,
    etm: bool,
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

fn rand<const N: usize>() -> Result<[u8; N], String> {
    let mut b = [0u8; N];
    iyakki_sec::random_fill(&mut b).map_err(|_| "no random numbers".to_string())?;
    Ok(b)
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

fn name_list(r: &mut Reader) -> Result<Vec<String>, String> {
    let b = r.string().ok_or("the server's algorithm list is cut short")?;
    let s = std::str::from_utf8(b).map_err(|_| "the server's algorithm list is not text")?;
    Ok(s.split(',').map(str::to_string).collect())
}

/// The packet layer and the connection state over any stream.
pub struct Ssh<S: Read + Write> {
    s: S,
    rseq: u32,
    wseq: u32,
    inn: Option<Dir>,
    out: Option<Dir>,
    stop: Arc<AtomicBool>,
    deadline: Instant,
    pad: [u8; 256],
    // the channel
    remote: u32,
    window: u64,
    max_packet: u32,
    recv_window: u32,
    buf: std::collections::VecDeque<u8>,
    eof: bool,
    closed_sent: bool,
    // key exchange memory
    vc: String,
    vs: String,
    sid: Vec<u8>,
    strict: bool,
    host_blob: Vec<u8>,
}

type Res<T> = Result<T, SshErr>;

impl<S: Read + Write> Ssh<S> {
    fn read_full(&mut self, buf: &mut [u8]) -> Result<(), String> {
        let mut got = 0;
        while got < buf.len() {
            if self.stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            if Instant::now() > self.deadline {
                return Err("the server did not answer in time".into());
            }
            match self.s.read(&mut buf[got..]) {
                Ok(0) => return Err("the connection was closed by the server".into()),
                Ok(n) => got += n,
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
                Err(e) => return Err(format!("network error: {}", e.kind())),
            }
        }
        Ok(())
    }

    fn read_line(&mut self) -> Result<String, String> {
        for _ in 0..16 {
            let mut line = Vec::new();
            loop {
                let mut b = [0u8; 1];
                self.read_full(&mut b)?;
                if b[0] == b'\n' {
                    break;
                }
                line.push(b[0]);
                if line.len() > 255 {
                    return Err("the server's greeting is too long".into());
                }
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if line.starts_with(b"SSH-") {
                let s = String::from_utf8(line).map_err(|_| "the server's greeting is not text")?;
                if !(s.starts_with("SSH-2.0-") || s.starts_with("SSH-1.99-")) {
                    return Err("the server does not speak SSH 2".into());
                }
                return Ok(s);
            }
        }
        Err("the server sent no SSH greeting (is this an SSH port?)".into())
    }

    fn write_raw(&mut self, b: &[u8]) -> Result<(), String> {
        self.s.write_all(b).and_then(|_| self.s.flush()).map_err(|e| format!("network error: {}", e.kind()))
    }

    fn read_packet(&mut self) -> Result<Vec<u8>, String> {
        self.deadline = Instant::now() + IDLE;
        let seq = self.rseq;
        self.rseq = self.rseq.wrapping_add(1);
        let check = |len: usize, min: usize| if len < min || len > MAX_PACKET { Err("the server sent a packet of a size that is not allowed".to_string()) } else { Ok(()) };
        match self.inn.is_some() {
            false => {
                let mut h = [0u8; 4];
                self.read_full(&mut h)?;
                let len = u32::from_be_bytes(h) as usize;
                check(len, 8)?;
                let mut body = vec![0u8; len];
                self.read_full(&mut body)?;
                unpad(&body)
            }
            true if self.inn.as_ref().is_some_and(|d| d.etm) => {
                let key = self.inn.as_ref().map(|d| d.mac_key.clone()).ok_or("no keys")?;
                let mut h = [0u8; 4];
                self.read_full(&mut h)?;
                let len = u32::from_be_bytes(h) as usize;
                check(len, 16)?;
                if len % 16 != 0 {
                    return Err("bad packet length".into());
                }
                let mut body = vec![0u8; len + 32];
                self.read_full(&mut body)?;
                let (ct, mac) = body.split_at_mut(len);
                if !ct_eq(&hmac_sha256(&*key, &[&seq.to_be_bytes(), &h, ct]), mac) {
                    return Err("a packet failed its integrity check (the connection may be tampered with)".into());
                }
                self.inn.as_mut().ok_or("no keys")?.ctr.apply_keystream(ct);
                unpad(ct)
            }
            true => {
                let key = self.inn.as_ref().map(|d| d.mac_key.clone()).ok_or("no keys")?;
                let mut first = [0u8; 16];
                self.read_full(&mut first)?;
                self.inn.as_mut().ok_or("no keys")?.ctr.apply_keystream(&mut first);
                let len = u32::from_be_bytes([first[0], first[1], first[2], first[3]]) as usize;
                check(len, 12)?;
                if (len + 4) % 16 != 0 {
                    return Err("bad packet length".into());
                }
                let mut rest = vec![0u8; len + 4 - 16 + 32];
                self.read_full(&mut rest)?;
                let (ct, mac) = rest.split_at_mut(len + 4 - 16);
                self.inn.as_mut().ok_or("no keys")?.ctr.apply_keystream(ct);
                let mut plain = first.to_vec();
                plain.extend_from_slice(ct);
                if !ct_eq(&hmac_sha256(&*key, &[&seq.to_be_bytes(), &plain]), mac) {
                    return Err("a packet failed its integrity check (the connection may be tampered with)".into());
                }
                unpad(&plain[4..])
            }
        }
    }

    fn write_packet(&mut self, payload: &[u8]) -> Result<(), String> {
        if payload.len() > MAX_PACKET - 64 {
            return Err("outgoing packet too large".into());
        }
        let seq = self.wseq;
        self.wseq = self.wseq.wrapping_add(1);
        let etm = self.out.as_ref().is_some_and(|d| d.etm);
        let counted = if etm { 0 } else { 4 };
        let mut pad = 16 - ((1 + payload.len() + counted) % 16);
        if pad < 4 {
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

    fn our_kexinit(&self) -> Result<Vec<u8>, String> {
        let mut p = vec![KEXINIT];
        p.extend_from_slice(&rand::<16>()?);
        let kex = if self.sid.is_empty() { KEX_ALGS } else { "curve25519-sha256,curve25519-sha256@libssh.org" };
        for list in [kex, "ssh-ed25519", "aes256-ctr", "aes256-ctr", &MACS.join(","), &MACS.join(","), "none", "none", "", ""] {
            put_str(&mut p, list.as_bytes());
        }
        p.push(0);
        put_u32(&mut p, 0);
        Ok(p)
    }

    /// One key exchange. `theirs` is the server's KEXINIT when it already arrived (a re-key it started).
    fn kex(&mut self, theirs: Option<Vec<u8>>, host_ok: &mut dyn FnMut(&str, &[u8]) -> Result<(), SshErr>) -> Res<()> {
        let first = self.sid.is_empty();
        let ours = self.our_kexinit()?;
        self.write_packet(&ours)?;
        let theirs = match theirs {
            Some(p) => p,
            None => self.read_packet()?,
        };
        if theirs.first() != Some(&KEXINIT) {
            return Err("the server did not start the key exchange".into());
        }
        let mut r = Reader { b: theirs.get(17..).ok_or("the server's key exchange message is cut short")? };
        let kex = name_list(&mut r)?;
        let host = name_list(&mut r)?;
        let enc_c = name_list(&mut r)?;
        let enc_s = name_list(&mut r)?;
        let mac_c = name_list(&mut r)?;
        let mac_s = name_list(&mut r)?;
        let comp_c = name_list(&mut r)?;
        let comp_s = name_list(&mut r)?;
        let _ = (name_list(&mut r)?, name_list(&mut r)?);
        if r.b.first().is_some_and(|f| *f != 0) {
            return Err("the server guesses the key exchange in advance (not supported)".into());
        }
        let has = |l: &[String], x: &str| l.iter().any(|y| y == x);
        if !has(&kex, "curve25519-sha256") && !has(&kex, "curve25519-sha256@libssh.org") {
            return Err("the server offers no curve25519 key exchange".into());
        }
        if !has(&host, "ssh-ed25519") {
            return Err("this server has no ssh-ed25519 host key, and only that kind is supported here (on the server: ssh-keygen -t ed25519 -f /etc/ssh/ssh_host_ed25519_key)".into());
        }
        if !has(&enc_c, "aes256-ctr") || !has(&enc_s, "aes256-ctr") {
            return Err("the server does not offer the aes256-ctr cipher".into());
        }
        let pick_mac = |l: &[String]| MACS.iter().find(|m| has(l, m)).copied();
        let (Some(mc), Some(ms)) = (pick_mac(&mac_c), pick_mac(&mac_s)) else { return Err("the server offers no hmac-sha2-256 integrity check".into()) };
        if !has(&comp_c, "none") || !has(&comp_s, "none") {
            return Err("the server insists on compression".into());
        }
        if first {
            self.strict = has(&kex, "kex-strict-s-v00@openssh.com");
        }
        let mut sk = rand::<32>()?;
        let qc = x25519_dalek::x25519(sk, x25519_dalek::X25519_BASEPOINT_BYTES);
        let mut init = vec![KEX_ECDH_INIT];
        put_str(&mut init, &qc);
        self.write_packet(&init)?;
        let reply = self.read_packet()?;
        if reply.first() != Some(&KEX_ECDH_REPLY) {
            return Err("the server's key exchange reply is wrong".into());
        }
        let mut r = Reader { b: &reply[1..] };
        let (Some(ks), Some(qs), Some(sig)) = (r.string(), r.string(), r.string()) else { return Err("the server's key exchange reply is cut short".into()) };
        let qs: [u8; 32] = qs.try_into().map_err(|_| "the server's key is not 32 bytes")?;
        let k = Zeroizing::new(x25519_dalek::x25519(sk, qs));
        sk.fill(0);
        if k.iter().all(|b| *b == 0) {
            return Err("the server sent a weak key".into());
        }
        let h = exchange_hash(&self.vc, &self.vs, &ours, &theirs, ks, &qc, &qs, &*k);
        if !verify_ed25519(ks, sig, &h) {
            return Err("the server's signature does not match its key (the connection may be tampered with)".into());
        }
        if first {
            host_ok("ssh-ed25519", ks)?;
            self.host_blob = ks.to_vec();
        } else if self.host_blob != ks {
            return Err("the server changed its host key while connected".into());
        }
        if first {
            self.sid = h.to_vec();
        }
        let sid = self.sid.clone();
        self.write_packet(&[NEWKEYS])?;
        if self.strict {
            self.wseq = 0;
        }
        let (iv_c, iv_s) = (derive(&*k, &h, b'A', &sid), derive(&*k, &h, b'B', &sid));
        let (key_c, key_s) = (derive(&*k, &h, b'C', &sid), derive(&*k, &h, b'D', &sid));
        let (mac_ck, mac_sk) = (derive(&*k, &h, b'E', &sid), derive(&*k, &h, b'F', &sid));
        self.out = Some(Dir { ctr: Aes256Ctr::new((&*key_c).into(), (&iv_c[..16]).into()), mac_key: mac_ck, etm: mc.contains("etm") });
        let nk = self.read_packet()?;
        if nk.first() != Some(&NEWKEYS) {
            return Err("the server did not finish the key exchange".into());
        }
        if self.strict {
            self.rseq = 0;
        }
        self.inn = Some(Dir { ctr: Aes256Ctr::new((&*key_s).into(), (&iv_s[..16]).into()), mac_key: mac_sk, etm: ms.contains("etm") });
        Ok(())
    }

    fn userauth(&mut self, user: &str, password: Option<&str>, key: Option<&KeyPair>) -> Res<()> {
        let mut req = vec![SERVICE_REQUEST];
        put_str(&mut req, b"ssh-userauth");
        self.write_packet(&req)?;
        loop {
            let p = self.read_packet()?;
            match p.first() {
                Some(&SERVICE_ACCEPT) => break,
                Some(&IGNORE) | Some(&DEBUG) => {}
                _ => return Err("the server did not accept the login service".into()),
            }
        }
        let mut tries: Vec<&str> = Vec::new();
        if key.is_some() {
            tries.push("publickey");
        }
        if password.is_some() {
            tries.push("password");
        }
        if tries.is_empty() {
            return Err(SshErr::Auth("no password or key to sign in with".into()));
        }
        for method in tries {
            let mut p = vec![USERAUTH_REQUEST];
            put_str(&mut p, user.as_bytes());
            put_str(&mut p, b"ssh-connection");
            put_str(&mut p, method.as_bytes());
            if method == "publickey" {
                let key = key.ok_or("no key")?;
                let blob = key.public_blob();
                let mut signed = Vec::new();
                put_str(&mut signed, &self.sid);
                signed.push(USERAUTH_REQUEST);
                put_str(&mut signed, user.as_bytes());
                put_str(&mut signed, b"ssh-connection");
                put_str(&mut signed, b"publickey");
                signed.push(1);
                put_str(&mut signed, b"ssh-ed25519");
                put_str(&mut signed, &blob);
                p.push(1);
                put_str(&mut p, b"ssh-ed25519");
                put_str(&mut p, &blob);
                put_str(&mut p, &key.sign_wire(&signed));
            } else {
                p.push(0);
                put_str(&mut p, password.unwrap_or("").as_bytes());
            }
            self.write_packet(&p)?;
            let mut p = Zeroizing::new(p);
            p.fill(0);
            loop {
                let a = self.read_packet()?;
                match a.first() {
                    Some(&USERAUTH_SUCCESS) => return Ok(()),
                    Some(&USERAUTH_FAILURE) => break,
                    Some(&USERAUTH_BANNER) | Some(&IGNORE) | Some(&DEBUG) => {}
                    Some(&60) if method == "password" => return Err(SshErr::Auth("the server asks for a new password; change it on the server first".into())),
                    Some(&DISCONNECT) => return Err(SshErr::Auth("the server ended the connection while signing in".into())),
                    _ => return Err("the server's login answer is wrong".into()),
                }
            }
        }
        Err(SshErr::Auth("the server did not accept the login (wrong user name, password or key)".into()))
    }

    fn open_sftp(&mut self) -> Res<()> {
        let mut p = vec![CHANNEL_OPEN];
        put_str(&mut p, b"session");
        put_u32(&mut p, 0);
        put_u32(&mut p, WINDOW);
        put_u32(&mut p, CHAN_PACKET);
        self.write_packet(&p)?;
        loop {
            let a = self.read_packet()?;
            let mut r = Reader { b: a.get(1..).unwrap_or(&[]) };
            match a.first() {
                Some(&CHANNEL_OPEN_CONFIRMATION) => {
                    let (Some(_mine), Some(theirs), Some(win), Some(maxp)) = (r.u32(), r.u32(), r.u32(), r.u32()) else { return Err("cut-short channel answer".into()) };
                    self.remote = theirs;
                    self.window = win as u64;
                    self.max_packet = maxp.clamp(64, 32_768);
                    break;
                }
                Some(&CHANNEL_OPEN_FAILURE) => return Err("the server refused to open a session".into()),
                Some(&IGNORE) | Some(&DEBUG) | Some(&GLOBAL_REQUEST) => {}
                _ => return Err("the server's channel answer is wrong".into()),
            }
        }
        let mut p = vec![CHANNEL_REQUEST];
        put_u32(&mut p, self.remote);
        put_str(&mut p, b"subsystem");
        p.push(1);
        put_str(&mut p, b"sftp");
        self.write_packet(&p)?;
        loop {
            let a = self.read_packet()?;
            match a.first() {
                Some(&CHANNEL_SUCCESS) => return Ok(()),
                Some(&CHANNEL_FAILURE) => return Err("the server has no SFTP service for this login".into()),
                Some(&CHANNEL_WINDOW_ADJUST) => {
                    let mut r = Reader { b: a.get(5..).unwrap_or(&[]) };
                    self.window += r.u32().unwrap_or(0) as u64;
                }
                Some(&IGNORE) | Some(&DEBUG) | Some(&GLOBAL_REQUEST) => {}
                _ => return Err("the server's subsystem answer is wrong".into()),
            }
        }
    }

    /// Read one packet and act on it: channel data goes to the buffer, window and re-key messages are answered.
    fn pump(&mut self) -> Result<(), String> {
        let p = self.read_packet()?;
        let mut r = Reader { b: p.get(1..).ok_or("empty packet")? };
        match p[0] {
            IGNORE | DEBUG | UNIMPLEMENTED => {}
            DISCONNECT => {
                self.eof = true;
                return Err("the server ended the connection".into());
            }
            KEXINIT => {
                let mut never = |_: &str, _: &[u8]| Ok(());
                self.kex(Some(p.clone()), &mut never).map_err(|e| e.to_string())?;
            }
            GLOBAL_REQUEST => {
                let _ = r.string();
                if r.b.first().is_some_and(|w| *w != 0) {
                    self.write_packet(&[REQUEST_FAILURE])?;
                }
            }
            CHANNEL_WINDOW_ADJUST => {
                let (_to, add) = (r.u32(), r.u32().ok_or("cut-short window message")?);
                self.window = (self.window + add as u64).min(u32::MAX as u64 * 2);
            }
            CHANNEL_DATA | CHANNEL_EXTENDED_DATA => {
                let _to = r.u32();
                if p[0] == CHANNEL_EXTENDED_DATA {
                    r.u32();
                }
                let data = r.string().ok_or("cut-short data")?;
                if data.len() > CHAN_PACKET as usize || data.len() as u64 > self.recv_window as u64 {
                    return Err("the server sent more data than the window allows".into());
                }
                self.recv_window -= data.len() as u32;
                if p[0] == CHANNEL_DATA {
                    self.buf.extend(data);
                }
                if self.recv_window < WINDOW / 2 {
                    let mut a = vec![CHANNEL_WINDOW_ADJUST];
                    put_u32(&mut a, self.remote);
                    put_u32(&mut a, WINDOW - self.recv_window);
                    self.write_packet(&a)?;
                    self.recv_window = WINDOW;
                }
            }
            CHANNEL_EOF => self.eof = true,
            CHANNEL_CLOSE => {
                self.eof = true;
                if !self.closed_sent {
                    self.closed_sent = true;
                    let mut c = vec![CHANNEL_CLOSE];
                    put_u32(&mut c, self.remote);
                    let _ = self.write_packet(&c);
                }
            }
            CHANNEL_REQUEST => {
                let _to = r.u32();
                let _kind = r.string();
                if r.b.first().is_some_and(|w| *w != 0) {
                    let mut f = vec![CHANNEL_FAILURE];
                    put_u32(&mut f, self.remote);
                    self.write_packet(&f)?;
                }
            }
            _ => {
                let mut u = vec![UNIMPLEMENTED];
                put_u32(&mut u, self.rseq.wrapping_sub(1));
                self.write_packet(&u)?;
            }
        }
        Ok(())
    }
}

fn unpad(body: &[u8]) -> Result<Vec<u8>, String> {
    let pad = *body.first().ok_or("empty packet")? as usize;
    if pad < 4 || pad + 1 >= body.len() {
        return Err("bad padding".into());
    }
    Ok(body[1..body.len() - pad].to_vec())
}

fn ioerr(e: String) -> std::io::Error {
    std::io::Error::other(e)
}

impl<S: Read + Write> Read for Ssh<S> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            if !self.buf.is_empty() {
                let n = out.len().min(self.buf.len());
                for (i, b) in self.buf.drain(..n).enumerate() {
                    out[i] = b;
                }
                return Ok(n);
            }
            if self.eof {
                return Ok(0);
            }
            self.pump().map_err(ioerr)?;
        }
    }
}

impl<S: Read + Write> Write for Ssh<S> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        while self.window == 0 {
            if self.eof {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            self.pump().map_err(ioerr)?;
        }
        let n = data.len().min(self.max_packet as usize).min(self.window as usize).min(32_000);
        let mut p = vec![CHANNEL_DATA];
        put_u32(&mut p, self.remote);
        put_str(&mut p, &data[..n]);
        self.write_packet(&p).map_err(ioerr)?;
        self.window -= n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Connect over an open stream (whose reads time out after a short while, so the stop switch is seen): greeting, key exchange, the
/// host key check, login, and the `sftp` subsystem. The returned object is the byte stream of the SFTP session.
pub fn connect<S: Read + Write>(stream: S, user: &str, password: Option<&str>, key: Option<&KeyPair>, host_ok: &dyn Fn(&str, &str) -> bool, stop: Arc<AtomicBool>) -> Result<Ssh<S>, SshErr> {
    let mut c = Ssh {
        s: stream,
        rseq: 0,
        wseq: 0,
        inn: None,
        out: None,
        stop,
        deadline: Instant::now() + Duration::from_secs(30),
        pad: rand::<256>()?,
        remote: 0,
        window: 0,
        max_packet: 0,
        recv_window: WINDOW,
        buf: Default::default(),
        eof: false,
        closed_sent: false,
        vc: VERSION_LINE.into(),
        vs: String::new(),
        sid: vec![],
        strict: false,
        host_blob: vec![],
    };
    c.write_raw(format!("{VERSION_LINE}\r\n").as_bytes())?;
    c.vs = c.read_line()?;
    let mut check = |kind: &str, blob: &[u8]| -> Res<()> {
        let fp = fingerprint_of_blob(blob);
        if host_ok(kind, &fp) { Ok(()) } else { Err(SshErr::HostKey { kind: kind.into(), fingerprint: fp }) }
    };
    c.kex(None, &mut check)?;
    c.userauth(user, password, key)?;
    c.open_sftp()?;
    c.deadline = Instant::now() + IDLE;
    Ok(c)
}
