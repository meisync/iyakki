//! The blocked-connection log: the kernel's log rule sends a rate-limited copy of the first bytes of each blocked packet to
//! an nflog group; this reads them into a ring of the last 50 attempts in memory. Nothing is written to the kernel log or
//! to disk. Netfilter's own limit (10 per second) keeps a flood from filling it.

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub const RING: usize = 50;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blocked {
    pub at: i64,
    pub src: IpAddr,
    /// IP protocol number (6 tcp, 17 udp, 1 icmp, 58 icmpv6).
    pub proto: u8,
    /// Destination port; 0 for protocols without ports.
    pub dport: u16,
    /// How many identical attempts in a row this entry stands for (a client retrying a connection).
    pub count: u32,
}

impl Blocked {
    pub fn proto_name(&self) -> String {
        match self.proto {
            6 => "tcp".into(),
            17 => "udp".into(),
            1 => "icmp".into(),
            58 => "icmpv6".into(),
            n => format!("proto {n}"),
        }
    }
}

#[derive(Default)]
pub struct BlockLog {
    ring: Mutex<VecDeque<Blocked>>,
    stop: AtomicBool,
}

impl BlockLog {
    pub fn new() -> Arc<BlockLog> {
        Arc::new(BlockLog::default())
    }
    pub fn push(&self, b: Blocked) {
        let mut r = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(last) = r.back_mut() {
            if last.src == b.src && last.proto == b.proto && last.dport == b.dport && b.at - last.at <= 10 {
                last.count += 1;
                last.at = b.at;
                return;
            }
        }
        if r.len() == RING {
            r.pop_front();
        }
        r.push_back(b);
    }
    /// Newest first.
    pub fn recent(&self) -> Vec<Blocked> {
        self.ring.lock().unwrap_or_else(|e| e.into_inner()).iter().rev().cloned().collect()
    }
    pub fn clear(&self) {
        self.ring.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Source, protocol and destination port of an IP packet's first bytes.
pub fn parse_packet(d: &[u8]) -> Option<(IpAddr, u8, u16)> {
    match d.first()? >> 4 {
        4 if d.len() >= 20 => {
            let ihl = usize::from(d[0] & 15) * 4;
            let proto = d[9];
            let src = IpAddr::V4(Ipv4Addr::new(d[12], d[13], d[14], d[15]));
            let port = if matches!(proto, 6 | 17) && d.len() >= ihl + 4 && ihl >= 20 { u16::from_be_bytes([d[ihl + 2], d[ihl + 3]]) } else { 0 };
            Some((src, proto, port))
        }
        6 if d.len() >= 40 => {
            let proto = d[6];
            let mut o = [0u8; 16];
            o.copy_from_slice(&d[8..24]);
            let port = if matches!(proto, 6 | 17) && d.len() >= 44 { u16::from_be_bytes([d[42], d[43]]) } else { 0 };
            Some((IpAddr::V6(Ipv6Addr::from(o)), proto, port))
        }
        _ => None,
    }
}

/// The packet bytes of an nflog packet message payload (after the 16-byte netlink header).
pub fn payload_of(msg: &[u8]) -> Option<&[u8]> {
    if msg.len() < 4 {
        return None;
    }
    crate::nl::attrs(&msg[4..]).into_iter().find(|(t, _)| *t == 9).map(|(_, v)| v)
}

#[cfg(target_os = "linux")]
mod reader {
    use super::*;

    fn cfg_msg(group: u16, seq: u32, f: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
        let mut out = vec![0u8; 16];
        out[4..6].copy_from_slice(&((4u16 << 8) | 1).to_ne_bytes());
        out[6..8].copy_from_slice(&5u16.to_ne_bytes()); // REQUEST | ACK
        out[8..12].copy_from_slice(&seq.to_ne_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&group.to_be_bytes());
        f(&mut out);
        let l = out.len() as u32;
        out[0..4].copy_from_slice(&l.to_ne_bytes());
        out
    }

    fn put(out: &mut Vec<u8>, ty: u16, data: &[u8]) {
        out.extend_from_slice(&((4 + data.len()) as u16).to_ne_bytes());
        out.extend_from_slice(&ty.to_ne_bytes());
        out.extend_from_slice(data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }

    /// Subscribe to `group` and fill `log` until `log.stop()`. Returns the thread's handle; failure to subscribe is reported.
    pub fn start(log: Arc<BlockLog>, group: u16, now: fn() -> i64) -> Result<std::thread::JoinHandle<()>, String> {
        let mut sock = crate::nl::NfSocket::open(1).map_err(|e| e.to_string())?;
        let seq = sock.next_seq();
        let bind = cfg_msg(group, seq, |o| put(o, 1, &[1, 0, 0, 0]));
        let mut mode = Vec::new();
        mode.extend_from_slice(&128u32.to_be_bytes());
        mode.extend_from_slice(&[2, 0]);
        let mode_msg = cfg_msg(group, seq + 1, |o| put(o, 2, &mode));
        sock.run(&bind, 1).map_err(|e| e.to_string())?;
        sock.run(&mode_msg, 1).map_err(|e| e.to_string())?;
        std::thread::Builder::new()
            .name("iyakki-fw-log".into())
            .spawn(move || {
                while !log.stop.load(Ordering::Relaxed) {
                    let Some(buf) = sock.recv_raw() else { continue };
                    for (t, p) in crate::nl::split_messages(&buf) {
                        if t == 4 << 8 {
                            if let Some((src, proto, dport)) = payload_of(p).and_then(parse_packet) {
                                log.push(Blocked { at: now(), src, proto, dport, count: 1 });
                            }
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())
    }
}

#[cfg(target_os = "linux")]
pub use reader::start;
