//! Our own nf_tables writer: builds the netlink messages that create `inet iyakki_fw` and sends them to the kernel in one
//! atomic batch (all of it or none of it). No other program is involved. The encoding follows `linux/netfilter/nf_tables.h`.
//!
//! The encoder ([`encode_replace`], [`encode_remove`]) is pure and tested byte for byte; [`NfSocket`] talks to the kernel
//! (Linux only; it needs CAP_NET_ADMIN, which PID 1 has).

use crate::ruleset::{Addr, Ruleset, Step, CHAIN, LOG_GROUP, TABLE};

const NLM_F_REQUEST: u16 = 1;
const NLM_F_ACK: u16 = 4;
const NLM_F_DUMP: u16 = 0x300;
const NLM_F_CREATE: u16 = 0x400;
const NLM_F_APPEND: u16 = 0x800;
const NLA_F_NESTED: u16 = 0x8000;

const NFNL_SUBSYS_NFTABLES: u16 = 10;
const NFNL_MSG_BATCH_BEGIN: u16 = 0x10;
const NFNL_MSG_BATCH_END: u16 = 0x11;
const NFT_MSG_NEWTABLE: u16 = 0;
const NFT_MSG_DELTABLE: u16 = 2;
const NFT_MSG_NEWCHAIN: u16 = 3;
const NFT_MSG_NEWRULE: u16 = 6;
const NFT_MSG_GETRULE: u16 = 7;
const NFPROTO_INET: u8 = 1;

const NF_DROP: i32 = 0;
const NF_ACCEPT: i32 = 1;
const NF_INET_LOCAL_IN: u32 = 1;

const REG1: u32 = 1;
const CMP_EQ: u32 = 0;
const CMP_NEQ: u32 = 1;
const CMP_LTE: u32 = 3;
const CMP_GTE: u32 = 5;
const META_IIFNAME: u32 = 6;
const META_NFPROTO: u32 = 15;
const META_L4PROTO: u32 = 16;
const META_PKTTYPE: u32 = 19;
const PAYLOAD_NETWORK: u32 = 1;
const PAYLOAD_TRANSPORT: u32 = 2;

fn pad4(v: &mut Vec<u8>) {
    while v.len() % 4 != 0 {
        v.push(0);
    }
}

fn attr(buf: &mut Vec<u8>, ty: u16, data: &[u8]) {
    buf.extend_from_slice(&((4 + data.len()) as u16).to_ne_bytes());
    buf.extend_from_slice(&ty.to_ne_bytes());
    buf.extend_from_slice(data);
    pad4(buf);
}

fn nest(buf: &mut Vec<u8>, ty: u16, f: impl FnOnce(&mut Vec<u8>)) {
    let start = buf.len();
    buf.extend_from_slice(&[0, 0]);
    buf.extend_from_slice(&(ty | NLA_F_NESTED).to_ne_bytes());
    f(buf);
    let len = (buf.len() - start) as u16;
    buf[start..start + 2].copy_from_slice(&len.to_ne_bytes());
}

fn be32(v: u32) -> [u8; 4] {
    v.to_be_bytes()
}

fn cstr(s: &str) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    v
}

/// One netlink message: header, nfgenmsg, attributes (written by `f`).
fn msg(out: &mut Vec<u8>, ty: u16, flags: u16, seq: u32, family: u8, res_id: u16, f: impl FnOnce(&mut Vec<u8>)) {
    let start = out.len();
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&ty.to_ne_bytes());
    out.extend_from_slice(&flags.to_ne_bytes());
    out.extend_from_slice(&seq.to_ne_bytes());
    out.extend_from_slice(&0u32.to_ne_bytes());
    out.push(family);
    out.push(0);
    out.extend_from_slice(&res_id.to_be_bytes());
    f(out);
    let len = (out.len() - start) as u32;
    out[start..start + 4].copy_from_slice(&len.to_ne_bytes());
}

/// `expr` element of a rule: name and its attributes.
fn expr(buf: &mut Vec<u8>, name: &str, f: impl FnOnce(&mut Vec<u8>)) {
    nest(buf, 1 /* NFTA_LIST_ELEM */, |b| {
        attr(b, 1 /* NFTA_EXPR_NAME */, &cstr(name));
        nest(b, 2 /* NFTA_EXPR_DATA */, f);
    });
}

fn meta_load(buf: &mut Vec<u8>, key: u32) {
    expr(buf, "meta", |b| {
        attr(b, 1, &be32(REG1)); // DREG
        attr(b, 2, &be32(key)); // KEY
    });
}

fn payload_load(buf: &mut Vec<u8>, base: u32, off: u32, len: u32) {
    expr(buf, "payload", |b| {
        attr(b, 1, &be32(REG1));
        attr(b, 2, &be32(base));
        attr(b, 3, &be32(off));
        attr(b, 4, &be32(len));
    });
}

fn cmp(buf: &mut Vec<u8>, op: u32, data: &[u8]) {
    expr(buf, "cmp", |b| {
        attr(b, 1, &be32(REG1)); // SREG
        attr(b, 2, &be32(op));
        nest(b, 3, |d| attr(d, 1, data)); // DATA { VALUE }
    });
}

fn bitwise(buf: &mut Vec<u8>, mask: &[u8]) {
    expr(buf, "bitwise", |b| {
        attr(b, 1, &be32(REG1));
        attr(b, 2, &be32(REG1));
        attr(b, 3, &be32(mask.len() as u32));
        nest(b, 4, |d| attr(d, 1, mask));
        nest(b, 5, |d| attr(d, 1, &vec![0u8; mask.len()]));
    });
}

fn verdict(buf: &mut Vec<u8>, code: i32) {
    expr(buf, "immediate", |b| {
        attr(b, 1, &be32(0)); // DREG = verdict register
        nest(b, 2, |d| nest(d, 2 /* NFTA_DATA_VERDICT */, |v| attr(v, 1, &code.to_be_bytes())));
    });
}

fn addr_bytes(ip: &std::net::IpAddr) -> Vec<u8> {
    match ip {
        std::net::IpAddr::V4(a) => a.octets().to_vec(),
        std::net::IpAddr::V6(a) => a.octets().to_vec(),
    }
}

fn mask_bytes(len: u8, total: usize) -> Vec<u8> {
    let mut m = vec![0u8; total];
    for (i, b) in m.iter_mut().enumerate() {
        let done = (i * 8) as i32;
        let left = i32::from(len) - done;
        *b = if left >= 8 {
            0xff
        } else if left <= 0 {
            0
        } else {
            0xffu8 << (8 - left)
        };
    }
    m
}

fn range_or_eq(buf: &mut Vec<u8>, lo: &[u8], hi: &[u8]) {
    if lo == hi {
        cmp(buf, CMP_EQ, lo);
    } else {
        cmp(buf, CMP_GTE, lo);
        cmp(buf, CMP_LTE, hi);
    }
}

/// The expressions of one step.
fn encode_step(buf: &mut Vec<u8>, s: &Step) {
    match s {
        Step::Iif { name, prefix } => {
            meta_load(buf, META_IIFNAME);
            if *prefix {
                cmp(buf, CMP_EQ, name.as_bytes());
            } else {
                let mut d = cstr(name);
                d.resize(16, 0);
                cmp(buf, CMP_EQ, &d);
            }
        }
        Step::PktTypeHost => {
            meta_load(buf, META_PKTTYPE);
            cmp(buf, CMP_EQ, &[0]);
        }
        Step::CtState(mask) => {
            expr(buf, "ct", |b| {
                attr(b, 1, &be32(REG1)); // DREG
                attr(b, 2, &be32(0)); // KEY = state
            });
            bitwise(buf, &mask.to_ne_bytes());
            cmp(buf, CMP_NEQ, &[0, 0, 0, 0]);
        }
        Step::Nfproto(n) => {
            meta_load(buf, META_NFPROTO);
            cmp(buf, CMP_EQ, &[*n]);
        }
        Step::L4(p) => {
            meta_load(buf, META_L4PROTO);
            cmp(buf, CMP_EQ, &[*p]);
        }
        Step::Saddr(a) => {
            let (off, len) = if a.is_v4() { (12, 4) } else { (8, 16) };
            match a {
                Addr::Cidr(_, 0) => {}
                Addr::Exact(ip) => {
                    payload_load(buf, PAYLOAD_NETWORK, off, len);
                    cmp(buf, CMP_EQ, &addr_bytes(ip));
                }
                Addr::Range(lo, hi) => {
                    payload_load(buf, PAYLOAD_NETWORK, off, len);
                    range_or_eq(buf, &addr_bytes(lo), &addr_bytes(hi));
                }
                Addr::Cidr(ip, l) => {
                    payload_load(buf, PAYLOAD_NETWORK, off, len);
                    if usize::from(*l) < len as usize * 8 {
                        bitwise(buf, &mask_bytes(*l, len as usize));
                    }
                    cmp(buf, CMP_EQ, &addr_bytes(ip));
                }
            }
        }
        Step::Dport(lo, hi) => {
            payload_load(buf, PAYLOAD_TRANSPORT, 2, 2);
            range_or_eq(buf, &lo.to_be_bytes(), &hi.to_be_bytes());
        }
        Step::IcmpType(lo, hi) | Step::Icmp6Type(lo, hi) => {
            payload_load(buf, PAYLOAD_TRANSPORT, 0, 1);
            range_or_eq(buf, &[*lo], &[*hi]);
        }
        Step::Limit { per_second, burst } => expr(buf, "limit", |b| {
            attr(b, 1, &u64::from(*per_second).to_be_bytes());
            attr(b, 2, &1u64.to_be_bytes()); // unit: one second
            attr(b, 3, &be32(*burst));
            attr(b, 4, &be32(0)); // packets
            attr(b, 5, &be32(0));
        }),
        Step::Log(g) => expr(buf, "log", |b| attr(b, 1, &g.to_be_bytes())),
        Step::Counter => expr(buf, "counter", |b| {
            attr(b, 1, &0u64.to_be_bytes());
            attr(b, 2, &0u64.to_be_bytes());
        }),
        Step::Accept => verdict(buf, NF_ACCEPT),
        Step::Drop => verdict(buf, NF_DROP),
    }
}

fn batch_begin(out: &mut Vec<u8>, seq: u32) {
    msg(out, NFNL_MSG_BATCH_BEGIN, NLM_F_REQUEST, seq, 0, NFNL_SUBSYS_NFTABLES, |_| {});
}
fn batch_end(out: &mut Vec<u8>, seq: u32) {
    msg(out, NFNL_MSG_BATCH_END, NLM_F_REQUEST, seq, 0, NFNL_SUBSYS_NFTABLES, |_| {});
}

fn ty(t: u16) -> u16 {
    (NFNL_SUBSYS_NFTABLES << 8) | t
}

fn table_msg(out: &mut Vec<u8>, seq: u32, t: u16, flags: u16) {
    msg(out, ty(t), NLM_F_REQUEST | NLM_F_ACK | flags, seq, NFPROTO_INET, 0, |b| attr(b, 1, &cstr(TABLE)));
}

/// A batch that creates the table if it is missing, deletes it (and everything in it) and makes it again with `rs`:
/// the kernel applies all of it or none of it, and nothing outside our table is touched. Returns the bytes and how many
/// acknowledgements to expect.
pub fn encode_replace(rs: &Ruleset, seq0: u32) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    let mut seq = seq0;
    let mut acks = 0;
    batch_begin(&mut out, seq);
    for (t, fl) in [(NFT_MSG_NEWTABLE, NLM_F_CREATE), (NFT_MSG_DELTABLE, 0), (NFT_MSG_NEWTABLE, NLM_F_CREATE)] {
        seq += 1;
        table_msg(&mut out, seq, t, fl);
        acks += 1;
    }
    seq += 1;
    acks += 1;
    msg(&mut out, ty(NFT_MSG_NEWCHAIN), NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE, seq, NFPROTO_INET, 0, |b| {
        attr(b, 1, &cstr(TABLE));
        attr(b, 3, &cstr(CHAIN));
        nest(b, 4, |h| {
            attr(h, 1, &be32(NF_INET_LOCAL_IN));
            attr(h, 2, &be32(0)); // priority 0 = filter
        });
        attr(b, 5, &be32(NF_DROP as u32)); // policy drop
        attr(b, 7, &cstr("filter"));
    });
    for r in &rs.rules {
        seq += 1;
        acks += 1;
        msg(&mut out, ty(NFT_MSG_NEWRULE), NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_APPEND, seq, NFPROTO_INET, 0, |b| {
            attr(b, 1, &cstr(TABLE));
            attr(b, 2, &cstr(CHAIN));
            nest(b, 4, |e| {
                for s in &r.steps {
                    encode_step(e, s);
                }
            });
        });
    }
    batch_end(&mut out, seq + 1);
    (out, acks)
}

/// A batch that removes our table (made first so that deleting a missing table is not an error).
pub fn encode_remove(seq0: u32) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    batch_begin(&mut out, seq0);
    table_msg(&mut out, seq0 + 1, NFT_MSG_NEWTABLE, NLM_F_CREATE);
    table_msg(&mut out, seq0 + 2, NFT_MSG_DELTABLE, 0);
    batch_end(&mut out, seq0 + 3);
    (out, 2)
}

/// The request that dumps our rules (to read the counter).
pub fn encode_dump_rules(seq: u32) -> Vec<u8> {
    let mut out = Vec::new();
    msg(&mut out, ty(NFT_MSG_GETRULE), NLM_F_REQUEST | NLM_F_DUMP, seq, NFPROTO_INET, 0, |b| attr(b, 1, &cstr(TABLE)));
    out
}

/// One netlink message read from a buffer: (type, payload after the 16-byte header).
pub fn split_messages(buf: &[u8]) -> Vec<(u16, &[u8])> {
    let mut v = Vec::new();
    let mut i = 0;
    while i + 16 <= buf.len() {
        let len = u32::from_ne_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]) as usize;
        if len < 16 || i + len > buf.len() {
            break;
        }
        let t = u16::from_ne_bytes([buf[i + 4], buf[i + 5]]);
        v.push((t, &buf[i + 16..i + len]));
        i += (len + 3) & !3;
    }
    v
}

/// Attributes of a payload (type without flags, value).
pub fn attrs(mut b: &[u8]) -> Vec<(u16, &[u8])> {
    let mut v = Vec::new();
    while b.len() >= 4 {
        let len = u16::from_ne_bytes([b[0], b[1]]) as usize;
        let t = u16::from_ne_bytes([b[2], b[3]]) & 0x3fff;
        if len < 4 || len > b.len() {
            break;
        }
        v.push((t, &b[4..len]));
        let adv = (len + 3) & !3;
        if adv >= b.len() {
            break;
        }
        b = &b[adv..];
    }
    v
}

/// Packets and bytes of every `counter` in the rules of a dump reply.
pub fn counters_of_dump(reply: &[u8]) -> (u64, u64) {
    let (mut pk, mut by) = (0u64, 0u64);
    for (t, payload) in split_messages(reply) {
        if t != ty(NFT_MSG_NEWRULE) || payload.len() < 4 {
            continue;
        }
        for (at, av) in attrs(&payload[4..]) {
            if at != 4 {
                continue;
            }
            for (_, elem) in attrs(av) {
                let a = attrs(elem);
                let name = a.iter().find(|(t, _)| *t == 1).map(|(_, v)| v.split(|b| *b == 0).next().unwrap_or(&[]));
                if name != Some(b"counter") {
                    continue;
                }
                if let Some((_, d)) = a.iter().find(|(t, _)| *t == 2) {
                    for (ct, cv) in attrs(d) {
                        let n = cv.get(..8).map(|x| u64::from_be_bytes(x.try_into().unwrap_or([0; 8]))).unwrap_or(0);
                        match ct {
                            1 => by = by.saturating_add(n),
                            2 => pk = pk.saturating_add(n),
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    (pk, by)
}

/// What the kernel said.
#[derive(Debug, PartialEq, Eq)]
pub enum NlError {
    /// The kernel has no nf_tables (or it cannot be loaded).
    Unavailable(String),
    Failed(String),
}

impl std::fmt::Display for NlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NlError::Unavailable(s) | NlError::Failed(s) => f.write_str(s),
        }
    }
}

/// Words for the errors the kernel gives.
pub fn errno_error(errno: i32) -> NlError {
    match errno {
        libc::EOPNOTSUPP | libc::EPROTONOSUPPORT | libc::EAFNOSUPPORT | libc::ENOPROTOOPT => NlError::Unavailable("Firewall not available in this kernel".into()),
        libc::EPERM | libc::EACCES => NlError::Failed("not allowed to change the firewall (needs administrator rights)".into()),
        libc::ENOENT => NlError::Failed("the kernel refused a rule: a part of the firewall it needs (a kernel module) is missing".into()),
        libc::ENOMEM | libc::ENOBUFS => NlError::Failed("the kernel is out of memory for the firewall".into()),
        e => NlError::Failed(format!("the kernel refused the firewall rules (error {e})")),
    }
}

#[cfg(target_os = "linux")]
pub use sock::NfSocket;

#[cfg(target_os = "linux")]
mod sock {
    use super::*;

    /// A NETLINK_NETFILTER socket.
    pub struct NfSocket {
        fd: i32,
        seq: u32,
    }

    impl Drop for NfSocket {
        fn drop(&mut self) {
            // SAFETY: fd was opened by us and is closed once.
            unsafe { libc::close(self.fd) };
        }
    }

    impl NfSocket {
        pub fn open(timeout_secs: u64) -> Result<NfSocket, NlError> {
            // SAFETY: plain syscalls with valid arguments.
            let fd = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_RAW | libc::SOCK_CLOEXEC, libc::NETLINK_NETFILTER) };
            if fd < 0 {
                return Err(errno_error(std::io::Error::last_os_error().raw_os_error().unwrap_or(0)));
            }
            let s = NfSocket { fd, seq: (std::process::id() & 0xffff) << 8 };
            let tv = libc::timeval { tv_sec: timeout_secs as _, tv_usec: 0 };
            // SAFETY: tv outlives the call; size matches.
            unsafe { libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_RCVTIMEO, (&tv as *const libc::timeval).cast(), std::mem::size_of::<libc::timeval>() as u32) };
            // SAFETY: zeroed sockaddr_nl with family set is valid.
            let mut sa: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
            sa.nl_family = libc::AF_NETLINK as u16;
            // SAFETY: sa is a valid sockaddr_nl of the given size.
            let r = unsafe { libc::bind(fd, (&sa as *const libc::sockaddr_nl).cast(), std::mem::size_of::<libc::sockaddr_nl>() as u32) };
            if r < 0 {
                return Err(errno_error(std::io::Error::last_os_error().raw_os_error().unwrap_or(0)));
            }
            Ok(s)
        }

        pub fn next_seq(&mut self) -> u32 {
            self.seq = self.seq.wrapping_add(16);
            self.seq
        }

        fn send(&self, b: &[u8]) -> Result<(), NlError> {
            // SAFETY: b is a valid buffer for its length.
            let n = unsafe { libc::send(self.fd, b.as_ptr().cast(), b.len(), 0) };
            if n < 0 || n as usize != b.len() {
                return Err(errno_error(std::io::Error::last_os_error().raw_os_error().unwrap_or(0)));
            }
            Ok(())
        }

        fn recv(&self) -> Result<Vec<u8>, NlError> {
            let mut buf = vec![0u8; 65536];
            // SAFETY: buf is valid for its length.
            let n = unsafe { libc::recv(self.fd, buf.as_mut_ptr().cast(), buf.len(), 0) };
            if n < 0 {
                let e = std::io::Error::last_os_error();
                return Err(if e.kind() == std::io::ErrorKind::WouldBlock { NlError::Failed("the kernel did not answer in time".into()) } else { errno_error(e.raw_os_error().unwrap_or(0)) });
            }
            buf.truncate(n as usize);
            Ok(buf)
        }

        /// One read with the socket's timeout; `None` when nothing arrived or it failed.
        pub fn recv_raw(&self) -> Option<Vec<u8>> {
            self.recv().ok()
        }

        /// Send a batch and wait for every acknowledgement; the first error wins.
        pub fn run(&mut self, batch: &[u8], acks: usize) -> Result<(), NlError> {
            self.send(batch)?;
            let mut got = 0;
            let mut first_err: Option<NlError> = None;
            while got < acks {
                let r = match self.recv() {
                    Ok(r) => r,
                    Err(e) => return Err(first_err.unwrap_or(e)),
                };
                for (t, p) in split_messages(&r) {
                    if t == 2 && p.len() >= 4 {
                        let code = i32::from_ne_bytes([p[0], p[1], p[2], p[3]]);
                        got += 1;
                        if code != 0 && first_err.is_none() {
                            first_err = Some(errno_error(-code));
                        }
                    }
                }
                if first_err.is_some() && got >= 1 {
                    // the batch was refused as a whole; the rest of the answers carry no news
                    break;
                }
            }
            first_err.map_or(Ok(()), Err)
        }

        /// Read the packet and byte counters of our rules.
        pub fn counters(&mut self) -> Result<(u64, u64), NlError> {
            let seq = self.next_seq();
            self.send(&encode_dump_rules(seq))?;
            let mut all = Vec::new();
            loop {
                let r = self.recv()?;
                let mut done = false;
                for (t, p) in split_messages(&r) {
                    if t == 3 {
                        done = true;
                    } else if t == 2 && p.len() >= 4 {
                        let code = i32::from_ne_bytes([p[0], p[1], p[2], p[3]]);
                        if code != 0 {
                            return Err(errno_error(-code));
                        }
                        done = true;
                    }
                }
                all.extend_from_slice(&r);
                if done {
                    break;
                }
            }
            Ok(counters_of_dump(&all))
        }

        pub fn replace(&mut self, rs: &Ruleset) -> Result<(), NlError> {
            let seq = self.next_seq();
            let (b, acks) = encode_replace(rs, seq);
            self.run(&b, acks)
        }

        pub fn remove(&mut self) -> Result<(), NlError> {
            let seq = self.next_seq();
            let (b, acks) = encode_remove(seq);
            self.run(&b, acks)
        }
    }
}

/// The group the log rule writes to (re-exported for the reader).
pub const fn log_group() -> u16 {
    LOG_GROUP
}
