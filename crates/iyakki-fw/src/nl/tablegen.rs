//! A small, general table writer on top of the encoder in the parent module: any table with named sets and base chains on the
//! OUTPUT hook, described by typed values (never by text). The VPN connector uses it for its kill-switch / fail-closed guard
//! (`inet iyakki_vpn`) and its DNS redirect (`inet iyakki_vpn_dns`); the images ship no `nft` program, so this is how those
//! tables reach the kernel. The rules of `inet iyakki_fw` still go through [`super::encode_replace`]; the two never share a table.
//!
//! Everything here is pure (bytes in, bytes out) except the socket methods in the parent module. The attribute numbers follow
//! `linux/netfilter/nf_tables.h`; the byte tests below decode what the encoder wrote, and ignored tests in
//! `tests/tablegen_kernel.rs` load the result into a real kernel.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use super::*;

const NFT_MSG_GETTABLE: u16 = 1;
const NFT_MSG_NEWSET: u16 = 9;
const NFT_MSG_NEWSETELEM: u16 = 12;
const NFT_MSG_DELSETELEM: u16 = 14;
const NF_INET_PRE_ROUTING: u32 = 0;
const NF_INET_LOCAL_IN: u32 = 1;
const NF_INET_FORWARD: u32 = 2;
const NF_INET_LOCAL_OUT: u32 = 3;
const NF_INET_POST_ROUTING: u32 = 4;
const NFT_RETURN: i32 = -5;
const META_OIFNAME: u32 = 7;
const CT_KEY_STATE: u32 = 0;
const META_MARK: u32 = 3;
const META_SKUID: u32 = 6;
const NFT_SET_TIMEOUT: u32 = 0x10;
const NFT_SET_INTERVAL: u32 = 0x4;
const NFT_SET_CONCAT: u32 = 0x80;
/// nft's data type number of a service (port) in a concatenation.
const TYPE_INET_SERVICE: u32 = 13;
const NFT_SET_ELEM_INTERVAL_END: u32 = 1;
/// `ipaddr` / `ip6addr` in nft's data type table (a hint for listings; the kernel keeps it).
const TYPE_IPADDR: u32 = 7;
const TYPE_IP6ADDR: u32 = 8;

const NFPROTO_BRIDGE: u8 = 7;

pub const MAX_RULES: usize = 500;
pub const MAX_SETS: usize = 8;
pub const MAX_MEMBERS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChainKind {
    Filter,
    Nat,
    /// `type route`: a locally made packet is routed again after the chain changed its mark (policy routing, F-VPN-08).
    Route,
}

/// Where a chain hooks in. Output is the default (the VPN guard); the others are for the internet-sharing table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Hook {
    #[default]
    Output,
    Input,
    Forward,
    Postrouting,
    /// Before the routing decision (destination NAT).
    Prerouting,
}

impl Hook {
    fn number(self) -> u32 {
        match self {
            Hook::Input => NF_INET_LOCAL_IN,
            Hook::Forward => NF_INET_FORWARD,
            Hook::Output => NF_INET_LOCAL_OUT,
            Hook::Postrouting => NF_INET_POST_ROUTING,
            Hook::Prerouting => NF_INET_PRE_ROUTING,
        }
    }
}

/// The protocol family a table lives in. Almost everything here is `inet`; `bridge` is for rules that must see frames between the
/// ports of one bridge (isolating the phones of a Bluetooth access point from each other), where only interface names and a
/// verdict can be matched (see [`TableSpec::check_in`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Family {
    #[default]
    Inet,
    Bridge,
}

impl Family {
    fn proto(self) -> u8 {
        match self {
            Family::Inet => NFPROTO_INET,
            Family::Bridge => NFPROTO_BRIDGE,
        }
    }
    pub fn word(self) -> &'static str {
        match self {
            Family::Inet => "inet",
            Family::Bridge => "bridge",
        }
    }
}

/// `ct state` bits: established and related (replies), as nftables numbers them.
pub const CT_ESTABLISHED_RELATED: u32 = 0x2 | 0x4;
/// `ct state invalid` (the bit nftables numbers 1).
pub const CT_INVALID: u32 = 0x1;
/// The connection tracking status bit that says the destination was rewritten (`ct status dnat`).
const CT_STATUS_DNAT: u32 = 0x20;
const CT_KEY_STATUS: u32 = 2;

/// An interface name, exact or by prefix (`cni*`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IfName {
    Exact(String),
    Prefix(String),
}

/// One step of a rule; a rule is a list that ends in a verdict or a redirect. Address steps check the address family first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Expr {
    OifIs(IfName),
    OifNot(IfName),
    /// The interface the packet came in on (input, forward, postrouting chains).
    IifIs(IfName),
    IifNot(IfName),
    /// Connection tracking state is any of these bits ([`CT_ESTABLISHED_RELATED`]).
    CtState(u32),
    /// Source in this network (IPv4 or IPv6, `prefix` bits).
    Saddr(IpAddr, u8),
    /// Nat chains on the postrouting hook only: masquerade as the outgoing interface's address.
    Masquerade,
    /// Destination in this network (`prefix` bits).
    Daddr(IpAddr, u8),
    /// Destination is a member of the named set of the table (family decides the address type).
    DaddrSet(String),
    /// The destination address and the destination port together are an element of the named `address . port` interval set of the
    /// table (F-FW-10); needs TCP or UDP named earlier in the rule and a set made from [`SetSpec::concat`].
    DaddrDportSet(String),
    /// `meta l4proto`: 6 TCP, 17 UDP, 58 ICMPv6.
    L4(u8),
    /// Needs an earlier `L4(6)` or `L4(17)` in the same rule.
    Sport(u16),
    Dport(u16),
    MarkIs(u32),
    /// Sets the packet's mark (`meta mark set`); the rule goes on.
    SetMark(u32),
    /// The packet was made by this user account (`meta skuid`; output hooks only).
    SkuidIs(u32),
    /// Destination address in this first-last range (one family).
    DaddrRange(IpAddr, IpAddr),
    /// Destination port in this range, both ends included; needs an earlier `L4(6)` or `L4(17)`.
    DportRange(u16, u16),
    /// IPv4 (true) or IPv6 (false) packets only.
    Nfproto(bool),
    /// ICMPv6 type in this range; needs an earlier `L4(58)`.
    IcmpType(u8, u8),
    /// The destination of the connection was rewritten by a DNAT rule.
    CtStatusDnat,
    /// Counts packets and bytes of the rule.
    Counter,
    /// Answers the sender with an ICMP "port unreachable" (both address families) instead of dropping silently; ends the rule.
    Reject,
    /// Nat chains on prerouting or output only: send the connection on to this address and port. Needs an earlier `L4(6)` or `L4(17)`.
    Dnat(IpAddr, u16),
    Accept,
    Drop,
    Return,
    /// Nat chains only: redirect to this port on the loopback address. Needs an earlier `L4(6)` or `L4(17)`.
    RedirectPort(u16),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetSpec {
    pub name: String,
    pub v4: bool,
    /// Elements may carry a lifetime (seconds) when they are added later.
    pub timeout: bool,
    /// Host addresses present from the start.
    pub members: Vec<IpAddr>,
    /// Address ranges (first and last address, both inside, one family, not overlapping) present from the start. A set with
    /// any makes an interval set (`NFT_SET_INTERVAL`): each range is written as a start element and an `interval end` element.
    #[serde(default)]
    pub ranges: Vec<(IpAddr, IpAddr)>,
    /// An `address . port` interval set (concatenation of the address and a destination port, F-FW-14): address span and port span
    /// per element. When not empty, `members` and `ranges` stay empty.
    #[serde(default)]
    pub concat: Vec<((IpAddr, IpAddr), (u16, u16))>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainSpec {
    pub name: String,
    pub kind: ChainKind,
    /// The hook (OUTPUT unless said otherwise) and the priority on it.
    #[serde(default)]
    pub hook: Hook,
    pub priority: i32,
    pub policy_accept: bool,
    pub rules: Vec<Vec<Expr>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableSpec {
    /// Family `inet`.
    pub name: String,
    pub sets: Vec<SetSpec>,
    pub chains: Vec<ChainSpec>,
}

fn name_ok(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

impl TableSpec {
    /// Checks that hold whatever the caller built; the encoder refuses a spec that fails them.
    pub fn check(&self) -> Result<(), String> {
        if !name_ok(&self.name, 31) {
            return Err("not a valid table name".into());
        }
        if self.name == TABLE {
            return Err("the Firewall's own table is written by its own code, not through this one".into());
        }
        if self.sets.len() > MAX_SETS || self.chains.is_empty() || self.chains.len() > 4 {
            return Err("too many or too few sets or chains".into());
        }
        for s in &self.sets {
            if !name_ok(&s.name, 31) {
                return Err("not a valid set name".into());
            }
            if s.members.len() > MAX_MEMBERS || s.members.iter().any(|m| m.is_ipv4() != s.v4) {
                return Err(format!("set {}: too many members or a member of the wrong family", s.name));
            }
            if !s.concat.is_empty() {
                let bad = !s.members.is_empty()
                    || !s.ranges.is_empty()
                    || s.concat.len() > MAX_MEMBERS
                    || s.concat.iter().any(|((a, z), (p, q))| a.is_ipv4() != s.v4 || z.is_ipv4() != s.v4 || addr_bytes(a) > addr_bytes(z) || p > q);
                if bad {
                    return Err(format!("set {}: an address and port set needs one family, spans in order and nothing else", s.name));
                }
            }
        }
        for c in &self.chains {
            if !name_ok(&c.name, 31) {
                return Err("not a valid chain name".into());
            }
            if c.rules.len() > MAX_RULES {
                return Err("too many rules".into());
            }
            for r in &c.rules {
                self.check_rule(c, r)?;
            }
        }
        Ok(())
    }

    /// [`TableSpec::check`] plus what the family allows. A bridge table has no sets and no nat chains, and its rules match interface
    /// names only (no address, protocol, port, mark or connection state: frames of a bridge are not necessarily IP).
    pub fn check_in(&self, family: Family) -> Result<(), String> {
        self.check()?;
        if family == Family::Bridge {
            if !self.sets.is_empty() {
                return Err("a bridge table has no sets".into());
            }
            for c in &self.chains {
                if c.kind == ChainKind::Nat {
                    return Err("a bridge table has no nat chain".into());
                }
                for e in c.rules.iter().flatten() {
                    if !matches!(e, Expr::IifIs(_) | Expr::IifNot(_) | Expr::OifIs(_) | Expr::OifNot(_) | Expr::Accept | Expr::Drop | Expr::Return) {
                        return Err("a bridge table matches interface names only".into());
                    }
                }
            }
        }
        Ok(())
    }

    fn check_rule(&self, c: &ChainSpec, r: &[Expr]) -> Result<(), String> {
        let last = r.last().ok_or("an empty rule")?;
        let ends = |e: &Expr| matches!(e, Expr::Accept | Expr::Drop | Expr::Return | Expr::RedirectPort(_) | Expr::Masquerade | Expr::Reject | Expr::Dnat(..));
        if !ends(last) || r[..r.len() - 1].iter().any(ends) {
            return Err("a rule must end in exactly one verdict or redirect".into());
        }
        let mut l4: Option<u8> = None;
        for e in r {
            match e {
                Expr::OifIs(i) | Expr::OifNot(i) | Expr::IifIs(i) | Expr::IifNot(i) => {
                    let (IfName::Exact(n) | IfName::Prefix(n)) = i;
                    if !name_ok(n, 15) {
                        return Err("not a valid interface name".into());
                    }
                }
                Expr::Masquerade => {
                    if c.kind != ChainKind::Nat || c.hook != Hook::Postrouting {
                        return Err("masquerade only works in a nat chain on the postrouting hook".into());
                    }
                }
                Expr::CtState(0) => return Err("an empty connection state".into()),
                Expr::SetMark(0) => return Err("a mark of 0 is no mark".into()),
                Expr::SkuidIs(_) | Expr::SetMark(_) => {
                    if c.hook != Hook::Output {
                        return Err("a user account and a mark to set are only known on the output hook".into());
                    }
                }
                Expr::Daddr(ip, p) | Expr::Saddr(ip, p) => {
                    if u32::from(*p) > if ip.is_ipv4() { 32 } else { 128 } {
                        return Err("prefix too long".into());
                    }
                }
                Expr::DaddrSet(n) => {
                    if !self.sets.iter().any(|s| &s.name == n) {
                        return Err(format!("rule uses the set {n}, which the table does not have"));
                    }
                }
                Expr::DaddrDportSet(n) => {
                    if !self.sets.iter().any(|s| &s.name == n && !s.concat.is_empty()) {
                        return Err(format!("rule uses the address and port set {n}, which the table does not have"));
                    }
                    if !matches!(l4, Some(6 | 17)) {
                        return Err("an address and port set needs TCP or UDP named earlier in the rule".into());
                    }
                }
                Expr::DaddrRange(a, b) => {
                    if a.is_ipv4() != b.is_ipv4() || addr_bytes(a) > addr_bytes(b) {
                        return Err("an address range needs one family and the first address not above the last".into());
                    }
                }
                Expr::DportRange(lo, hi) if lo > hi => return Err("a port range with the first port above the last".into()),
                Expr::IcmpType(lo, hi) => {
                    if l4 != Some(58) || lo > hi {
                        return Err("an ICMPv6 type needs ICMPv6 named earlier and a range in order".into());
                    }
                }
                Expr::Dnat(_, port) => {
                    if c.kind != ChainKind::Nat || !matches!(c.hook, Hook::Prerouting | Hook::Output) {
                        return Err("dnat only works in a nat chain on the prerouting or output hook".into());
                    }
                    if *port == 0 || !matches!(l4, Some(6 | 17)) {
                        return Err("dnat needs TCP or UDP named earlier and a port above 0".into());
                    }
                }
                Expr::L4(p) => l4 = Some(*p),
                Expr::Sport(_) | Expr::Dport(_) | Expr::DportRange(..) | Expr::RedirectPort(_) => {
                    if !matches!(l4, Some(6 | 17)) {
                        return Err("a port needs TCP or UDP named earlier in the rule".into());
                    }
                    if matches!(e, Expr::RedirectPort(p) if *p == 0) {
                        return Err("redirect to port 0".into());
                    }
                    if matches!(e, Expr::RedirectPort(_)) && c.kind != ChainKind::Nat {
                        return Err("redirect only works in a nat chain".into());
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn masked(ip: &IpAddr, prefix: u8) -> Vec<u8> {
    let b = addr_bytes(ip);
    let m = mask_bytes(prefix, b.len());
    b.iter().zip(m).map(|(x, m)| x & m).collect()
}

fn ifname_cmp(buf: &mut Vec<u8>, key: u32, op: u32, n: &IfName) {
    meta_load(buf, key);
    match n {
        IfName::Exact(s) => {
            let mut d = cstr(s);
            d.resize(16, 0);
            cmp(buf, op, &d);
        }
        IfName::Prefix(s) => cmp(buf, op, s.as_bytes()),
    }
}

fn nfproto(buf: &mut Vec<u8>, v4: bool) {
    meta_load(buf, META_NFPROTO);
    cmp(buf, CMP_EQ, &[if v4 { 2 } else { 10 }]);
}

fn encode_expr(buf: &mut Vec<u8>, e: &Expr, sets: &[SetSpec], l4: &mut u8) {
    match e {
        Expr::OifIs(n) => ifname_cmp(buf, META_OIFNAME, CMP_EQ, n),
        Expr::OifNot(n) => ifname_cmp(buf, META_OIFNAME, CMP_NEQ, n),
        Expr::IifIs(n) => ifname_cmp(buf, META_IIFNAME, CMP_EQ, n),
        Expr::IifNot(n) => ifname_cmp(buf, META_IIFNAME, CMP_NEQ, n),
        Expr::CtState(mask) => {
            expr(buf, "ct", |b| {
                attr(b, 1, &be32(REG1)); // DREG
                attr(b, 2, &be32(CT_KEY_STATE)); // KEY
            });
            bitwise(buf, &mask.to_ne_bytes());
            cmp(buf, CMP_NEQ, &[0, 0, 0, 0]);
        }
        Expr::Saddr(ip, prefix) => {
            nfproto(buf, ip.is_ipv4());
            if *prefix == 0 {
                return;
            }
            let (off, len) = if ip.is_ipv4() { (12, 4) } else { (8, 16) };
            payload_load(buf, PAYLOAD_NETWORK, off, len);
            if usize::from(*prefix) < len as usize * 8 {
                bitwise(buf, &mask_bytes(*prefix, len as usize));
            }
            cmp(buf, CMP_EQ, &masked(ip, *prefix));
        }
        Expr::Masquerade => expr(buf, "masq", |_| {}),
        Expr::Daddr(ip, prefix) => {
            nfproto(buf, ip.is_ipv4());
            if *prefix == 0 {
                return;
            }
            let (off, len) = if ip.is_ipv4() { (16, 4) } else { (24, 16) };
            payload_load(buf, PAYLOAD_NETWORK, off, len);
            if usize::from(*prefix) < len as usize * 8 {
                bitwise(buf, &mask_bytes(*prefix, len as usize));
            }
            cmp(buf, CMP_EQ, &masked(ip, *prefix));
        }
        Expr::DaddrSet(name) => {
            let (idx, s) = sets.iter().enumerate().find(|(_, s)| &s.name == name).expect("checked");
            nfproto(buf, s.v4);
            let (off, len) = if s.v4 { (16, 4) } else { (24, 16) };
            payload_load(buf, PAYLOAD_NETWORK, off, len);
            expr(buf, "lookup", |b| {
                attr(b, 1, &cstr(name)); // SET
                attr(b, 2, &be32(REG1)); // SREG
                attr(b, 4, &be32(idx as u32 + 1)); // SET_ID
            });
        }
        Expr::DaddrDportSet(name) => {
            let (idx, s) = sets.iter().enumerate().find(|(_, s)| &s.name == name).expect("checked");
            nfproto(buf, s.v4);
            let (off, len) = if s.v4 { (16, 4) } else { (24, 16) };
            // the address into the first words of register 1 (the 32-bit register 8), the port into the next word
            payload_load_to(buf, 8, PAYLOAD_NETWORK, off, len);
            payload_load_to(buf, 8 + len / 4, PAYLOAD_TRANSPORT, 2, 2);
            expr(buf, "lookup", |b| {
                attr(b, 1, &cstr(name)); // SET
                attr(b, 2, &be32(REG1)); // SREG
                attr(b, 4, &be32(idx as u32 + 1)); // SET_ID
            });
        }
        Expr::L4(p) => {
            *l4 = *p;
            meta_load(buf, META_L4PROTO);
            cmp(buf, CMP_EQ, &[*p]);
        }
        Expr::Sport(p) => {
            payload_load(buf, PAYLOAD_TRANSPORT, 0, 2);
            cmp(buf, CMP_EQ, &p.to_be_bytes());
        }
        Expr::Dport(p) => {
            payload_load(buf, PAYLOAD_TRANSPORT, 2, 2);
            cmp(buf, CMP_EQ, &p.to_be_bytes());
        }
        Expr::DaddrRange(a, b) => {
            nfproto(buf, a.is_ipv4());
            let (off, len) = if a.is_ipv4() { (16, 4) } else { (24, 16) };
            payload_load(buf, PAYLOAD_NETWORK, off, len);
            range_or_eq(buf, &addr_bytes(a), &addr_bytes(b));
        }
        Expr::DportRange(lo, hi) => {
            payload_load(buf, PAYLOAD_TRANSPORT, 2, 2);
            range_or_eq(buf, &lo.to_be_bytes(), &hi.to_be_bytes());
        }
        Expr::Nfproto(v4) => nfproto(buf, *v4),
        Expr::IcmpType(lo, hi) => {
            payload_load(buf, PAYLOAD_TRANSPORT, 0, 1);
            range_or_eq(buf, &[*lo], &[*hi]);
        }
        Expr::CtStatusDnat => {
            expr(buf, "ct", |b| {
                attr(b, 1, &be32(REG1));
                attr(b, 2, &be32(CT_KEY_STATUS));
            });
            bitwise(buf, &CT_STATUS_DNAT.to_ne_bytes());
            cmp(buf, CMP_NEQ, &[0, 0, 0, 0]);
        }
        Expr::Counter => expr(buf, "counter", |b| {
            attr(b, 1, &0u64.to_be_bytes());
            attr(b, 2, &0u64.to_be_bytes());
        }),
        Expr::Reject => expr(buf, "reject", |b| {
            attr(b, 1, &be32(2)); // NFT_REJECT_ICMPX_UNREACH
            attr(b, 2, &[1]); // NFT_REJECT_ICMPX_PORT_UNREACH
        }),
        Expr::Dnat(ip, port) => {
            expr(buf, "immediate", |b| {
                attr(b, 1, &be32(REG1));
                nest(b, 2, |d| attr(d, 1, &addr_bytes(ip)));
            });
            expr(buf, "immediate", |b| {
                attr(b, 1, &be32(2));
                nest(b, 2, |d| attr(d, 1, &port.to_be_bytes()));
            });
            expr(buf, "nat", |b| {
                attr(b, 1, &be32(1)); // NFT_NAT_DNAT
                attr(b, 2, &be32(if ip.is_ipv4() { 2 } else { 10 })); // family
                attr(b, 3, &be32(REG1)); // ADDR_MIN
                attr(b, 5, &be32(2)); // PROTO_MIN
            });
        }
        Expr::MarkIs(m) => {
            meta_load(buf, META_MARK);
            cmp(buf, CMP_EQ, &m.to_ne_bytes());
        }
        Expr::SkuidIs(u) => {
            meta_load(buf, META_SKUID);
            cmp(buf, CMP_EQ, &u.to_ne_bytes());
        }
        Expr::SetMark(m) => {
            expr(buf, "immediate", |b| {
                attr(b, 1, &be32(REG1));
                nest(b, 2, |d| attr(d, 1, &m.to_ne_bytes()));
            });
            expr(buf, "meta", |b| {
                attr(b, 2, &be32(META_MARK)); // KEY
                attr(b, 3, &be32(REG1)); // SREG
            });
        }
        Expr::Accept => verdict(buf, NF_ACCEPT),
        Expr::Drop => verdict(buf, NF_DROP),
        Expr::Return => verdict(buf, NFT_RETURN),
        Expr::RedirectPort(port) => {
            expr(buf, "immediate", |b| {
                attr(b, 1, &be32(REG1));
                nest(b, 2, |d| attr(d, 1, &port.to_be_bytes()));
            });
            expr(buf, "redir", |b| {
                attr(b, 1, &be32(REG1)); // PROTO_MIN
                attr(b, 2, &be32(REG1)); // PROTO_MAX
            });
        }
    }
}

fn tmsg(out: &mut Vec<u8>, t: u16, flags: u16, seq: u32, f: impl FnOnce(&mut Vec<u8>)) {
    tmsg_in(Family::Inet, out, t, flags, seq, f);
}

fn tmsg_in(family: Family, out: &mut Vec<u8>, t: u16, flags: u16, seq: u32, f: impl FnOnce(&mut Vec<u8>)) {
    msg(out, ty(t), NLM_F_REQUEST | NLM_F_ACK | flags, seq, family.proto(), 0, f);
}

fn key_elems(buf: &mut Vec<u8>, ips: &[(IpAddr, Option<u64>)]) {
    nest(buf, 3 /* ELEMENTS */, |l| {
        for (ip, timeout) in ips {
            nest(l, 1 /* LIST_ELEM */, |e| {
                nest(e, 1 /* KEY */, |k| attr(k, 1, &addr_bytes(ip)));
                if let Some(s) = timeout {
                    attr(e, 4 /* TIMEOUT, ms */, &s.saturating_mul(1000).to_be_bytes());
                }
            });
        }
    });
}

/// The messages that make the named sets of a table (and fill them), numbered from `*seq`; rules refer to a set by its position + 1.
pub(super) fn encode_sets(out: &mut Vec<u8>, table: &str, sets: &[SetSpec], seq: &mut u32, acks: &mut usize) {
    for (i, s) in sets.iter().enumerate() {
        if !s.concat.is_empty() {
            encode_concat_set(out, table, s, i as u32 + 1, seq, acks);
            continue;
        }
        *seq += 1;
        *acks += 1;
        tmsg(out, NFT_MSG_NEWSET, NLM_F_CREATE, *seq, |b| {
            attr(b, 1, &cstr(table));
            attr(b, 2, &cstr(&s.name));
            attr(b, 3, &be32(if s.timeout { NFT_SET_TIMEOUT } else { 0 } | if s.ranges.is_empty() { 0 } else { NFT_SET_INTERVAL }));
            attr(b, 4, &be32(if s.v4 { TYPE_IPADDR } else { TYPE_IP6ADDR }));
            attr(b, 5, &be32(if s.v4 { 4 } else { 16 }));
            attr(b, 10, &be32(i as u32 + 1));
        });
        if !s.members.is_empty() {
            *seq += 1;
            *acks += 1;
            let items: Vec<(IpAddr, Option<u64>)> = s.members.iter().map(|m| (*m, None)).collect();
            tmsg(out, NFT_MSG_NEWSETELEM, NLM_F_CREATE, *seq, |b| {
                attr(b, 1, &cstr(table));
                attr(b, 2, &cstr(&s.name));
                key_elems(b, &items);
                attr(b, 4, &be32(i as u32 + 1));
            });
        }
        if !s.ranges.is_empty() {
            *seq += 1;
            *acks += 1;
            tmsg(out, NFT_MSG_NEWSETELEM, NLM_F_CREATE, *seq, |b| {
                attr(b, 1, &cstr(table));
                attr(b, 2, &cstr(&s.name));
                range_elems(b, &s.ranges);
                attr(b, 4, &be32(i as u32 + 1));
            });
        }
    }
}

/// The bytes of one concatenated key: the address, then the port in network order padded to a whole 32-bit word.
fn concat_key(ip: &IpAddr, port: u16) -> Vec<u8> {
    let mut k = addr_bytes(ip);
    k.extend_from_slice(&port.to_be_bytes());
    k.extend_from_slice(&[0, 0]);
    k
}

/// An `address . port` interval set and its elements, byte for byte what `nft` sends for `type ipv4_addr . inet_service; flags
/// interval`: flags interval and concatenation, the type `(address type << 6) | service`, the padded key length, a description with
/// the length of each field, and each element as a start key plus (attribute 10) the last key, inclusive.
fn encode_concat_set(out: &mut Vec<u8>, table: &str, s: &SetSpec, id: u32, seq: &mut u32, acks: &mut usize) {
    let alen: u32 = if s.v4 { 4 } else { 16 };
    *seq += 1;
    *acks += 1;
    tmsg(out, NFT_MSG_NEWSET, NLM_F_CREATE, *seq, |b| {
        attr(b, 1, &cstr(table));
        attr(b, 2, &cstr(&s.name));
        attr(b, 3, &be32(NFT_SET_INTERVAL | NFT_SET_CONCAT));
        attr(b, 4, &be32(((if s.v4 { TYPE_IPADDR } else { TYPE_IP6ADDR }) << 6) | TYPE_INET_SERVICE));
        attr(b, 5, &be32(alen + 4));
        attr(b, 10, &be32(id));
        nest(b, 9 /* DESC */, |d| {
            nest(d, 2 /* CONCAT */, |c| {
                for len in [alen, 2] {
                    nest(c, 1 /* LIST_ELEM */, |e| attr(e, 1 /* FIELD_LEN */, &be32(len)));
                }
            });
        });
    });
    *seq += 1;
    *acks += 1;
    tmsg(out, NFT_MSG_NEWSETELEM, NLM_F_CREATE, *seq, |b| {
        attr(b, 1, &cstr(table));
        attr(b, 2, &cstr(&s.name));
        nest(b, 3 /* ELEMENTS */, |l| {
            for ((a, z), (p, q)) in &s.concat {
                nest(l, 1, |e| {
                    nest(e, 1 /* KEY */, |k| attr(k, 1, &concat_key(a, *p)));
                    nest(e, 10 /* KEY_END */, |k| attr(k, 1, &concat_key(z, *q)));
                });
            }
        });
        attr(b, 4, &be32(id));
    });
}

/// The address right after `ip` (all zeros after the last one, which is how an interval that runs to the end is closed).
fn successor(ip: &IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(a) => IpAddr::V4(std::net::Ipv4Addr::from(u32::from(*a).wrapping_add(1))),
        IpAddr::V6(a) => IpAddr::V6(std::net::Ipv6Addr::from(u128::from(*a).wrapping_add(1))),
    }
}

/// Interval elements: for each range a start element and an end element (the key right after the range, flagged `interval end`).
fn range_elems(buf: &mut Vec<u8>, ranges: &[(IpAddr, IpAddr)]) {
    nest(buf, 3 /* ELEMENTS */, |l| {
        for (lo, hi) in ranges {
            nest(l, 1, |e| nest(e, 1 /* KEY */, |k| attr(k, 1, &addr_bytes(lo))));
            nest(l, 1, |e| {
                nest(e, 1 /* KEY */, |k| attr(k, 1, &addr_bytes(&successor(hi))));
                attr(e, 3 /* FLAGS */, &be32(NFT_SET_ELEM_INTERVAL_END));
            });
        }
    });
}

/// One batch: make the table if missing, delete it with everything in it, make it again with its sets, chains and rules. All of
/// it or none of it. Returns the bytes and how many acknowledgements to expect.
pub fn encode_apply(spec: &TableSpec, seq0: u32) -> Result<(Vec<u8>, usize), String> {
    encode_apply_in(spec, Family::Inet, seq0)
}

/// [`encode_apply`] for a table of the given family.
pub fn encode_apply_in(spec: &TableSpec, family: Family, seq0: u32) -> Result<(Vec<u8>, usize), String> {
    spec.check_in(family)?;
    let mut out = Vec::new();
    let mut seq = seq0;
    let mut acks = 0;
    batch_begin(&mut out, seq);
    for (t, fl) in [(NFT_MSG_NEWTABLE, NLM_F_CREATE), (NFT_MSG_DELTABLE, 0), (NFT_MSG_NEWTABLE, NLM_F_CREATE)] {
        seq += 1;
        acks += 1;
        tmsg_in(family, &mut out, t, fl, seq, |b| attr(b, 1, &cstr(&spec.name)));
    }
    encode_sets(&mut out, &spec.name, &spec.sets, &mut seq, &mut acks);
    for c in &spec.chains {
        seq += 1;
        acks += 1;
        tmsg_in(family, &mut out, NFT_MSG_NEWCHAIN, NLM_F_CREATE, seq, |b| {
            attr(b, 1, &cstr(&spec.name));
            attr(b, 3, &cstr(&c.name));
            nest(b, 4, |h| {
                attr(h, 1, &be32(c.hook.number()));
                attr(h, 2, &c.priority.to_be_bytes());
            });
            attr(b, 5, &be32(if c.policy_accept { NF_ACCEPT } else { NF_DROP } as u32));
            attr(b, 7, &cstr(match c.kind {
                ChainKind::Nat => "nat",
                ChainKind::Route => "route",
                ChainKind::Filter => "filter",
            }));
        });
        for r in &c.rules {
            seq += 1;
            acks += 1;
            tmsg_in(family, &mut out, NFT_MSG_NEWRULE, NLM_F_CREATE | NLM_F_APPEND, seq, |b| {
                attr(b, 1, &cstr(&spec.name));
                attr(b, 2, &cstr(&c.name));
                nest(b, 4, |e| {
                    let mut l4 = 0u8;
                    for x in r {
                        encode_expr(e, x, &spec.sets, &mut l4);
                    }
                });
            });
        }
    }
    batch_end(&mut out, seq + 1);
    Ok((out, acks))
}

/// One batch that removes the table (made first so that deleting a missing table is not an error).
pub fn encode_remove(name: &str, seq0: u32) -> Result<(Vec<u8>, usize), String> {
    encode_remove_in(Family::Inet, name, seq0)
}

/// [`encode_remove`] for a table of the given family.
pub fn encode_remove_in(family: Family, name: &str, seq0: u32) -> Result<(Vec<u8>, usize), String> {
    if !name_ok(name, 31) || name == TABLE {
        return Err("not a table this writer may remove".into());
    }
    let mut out = Vec::new();
    batch_begin(&mut out, seq0);
    tmsg_in(family, &mut out, NFT_MSG_NEWTABLE, NLM_F_CREATE, seq0 + 1, |b| attr(b, 1, &cstr(name)));
    tmsg_in(family, &mut out, NFT_MSG_DELTABLE, 0, seq0 + 2, |b| attr(b, 1, &cstr(name)));
    batch_end(&mut out, seq0 + 3);
    Ok((out, 2))
}

/// Add (with a lifetime in seconds when given) or delete one address in a set of the table.
pub fn encode_elem(table: &str, set: &str, ip: IpAddr, timeout_secs: Option<u64>, add: bool, seq0: u32) -> Result<(Vec<u8>, usize), String> {
    if !name_ok(table, 31) || !name_ok(set, 31) || table == TABLE {
        return Err("not a table or set this writer may change".into());
    }
    let mut out = Vec::new();
    batch_begin(&mut out, seq0);
    let t = if add { NFT_MSG_NEWSETELEM } else { NFT_MSG_DELSETELEM };
    let items = [(ip, if add { timeout_secs } else { None })];
    tmsg(&mut out, t, if add { NLM_F_CREATE } else { 0 }, seq0 + 1, |b| {
        attr(b, 1, &cstr(table));
        attr(b, 2, &cstr(set));
        key_elems(b, &items);
    });
    batch_end(&mut out, seq0 + 2);
    Ok((out, 1))
}

/// A plain "get this table" request; the answer is the table or an ENOENT error.
pub fn encode_get_table(name: &str, seq: u32) -> Vec<u8> {
    encode_get_table_in(Family::Inet, name, seq)
}

/// [`encode_get_table`] for a table of the given family.
pub fn encode_get_table_in(family: Family, name: &str, seq: u32) -> Vec<u8> {
    let mut out = Vec::new();
    msg(&mut out, ty(NFT_MSG_GETTABLE), NLM_F_REQUEST, seq, family.proto(), 0, |b| attr(b, 1, &cstr(name)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> TableSpec {
        let tun = IfName::Exact("iyvpn0".into());
        TableSpec {
            name: "iyakki_vpn".into(),
            sets: vec![SetSpec { name: "vpn_dest4".into(), v4: true, timeout: true, members: vec![], ranges: vec![], concat: vec![] }, SetSpec { name: "vpn_dest6".into(), v4: false, timeout: true, members: vec![], ranges: vec![], concat: vec![] }],
            chains: vec![ChainSpec {
                name: "output".into(),
                kind: ChainKind::Filter,
                hook: Hook::Output, priority: -10,
                policy_accept: true,
                rules: vec![
                    vec![Expr::OifIs(IfName::Exact("lo".into())), Expr::Accept],
                    vec![Expr::L4(17), Expr::Sport(68), Expr::Dport(67), Expr::Accept],
                    vec![Expr::Daddr("fe80::".parse().unwrap(), 10), Expr::L4(58), Expr::Accept],
                    vec![Expr::OifIs(IfName::Prefix("cni".into())), Expr::Accept],
                    vec![Expr::OifNot(tun.clone()), Expr::Daddr("203.0.113.0".parse().unwrap(), 24), Expr::Drop],
                    vec![Expr::OifNot(tun), Expr::DaddrSet("vpn_dest4".into()), Expr::Drop],
                    vec![Expr::Drop],
                ],
            }],
        }
    }

    fn msgs(b: &[u8]) -> Vec<(u16, Vec<u8>)> {
        split_messages(b).into_iter().map(|(t, p)| (t, p.to_vec())).collect()
    }

    fn name_of(payload: &[u8], at: u16) -> Option<String> {
        attrs(&payload[4..]).into_iter().find(|(t, _)| *t == at).map(|(_, v)| String::from_utf8_lossy(v.split(|b| *b == 0).next().unwrap_or(&[])).into_owned())
    }

    fn expr_names(rule_payload: &[u8]) -> Vec<String> {
        let list = attrs(&rule_payload[4..]).into_iter().find(|(t, _)| *t == 4).unwrap().1;
        attrs(list).into_iter().map(|(_, e)| name_of_expr(e)).collect()
    }

    fn name_of_expr(e: &[u8]) -> String {
        let a = attrs(e);
        let n = a.iter().find(|(t, _)| *t == 1).unwrap().1;
        String::from_utf8_lossy(n.split(|b| *b == 0).next().unwrap()).into_owned()
    }

    #[test]
    fn the_guard_batch_has_the_table_sets_chain_and_every_rule_in_order() {
        let spec = guard();
        let (b, acks) = encode_apply(&spec, 100).unwrap();
        let m = msgs(&b);
        let types: Vec<u16> = m.iter().map(|(t, _)| *t).collect();
        assert_eq!(types[0], 0x10, "batch begin first");
        assert_eq!(*types.last().unwrap(), 0x11, "batch end last");
        let body: Vec<u16> = types[1..types.len() - 1].iter().map(|t| t & 0xff).collect();
        assert_eq!(&body[..3], &[0, 2, 0], "table create, delete, create");
        assert_eq!(&body[3..5], &[9, 9], "two sets");
        assert_eq!(body[5], 3, "the chain");
        assert!(body[6..].iter().all(|t| *t == 6), "then only rules");
        assert_eq!(body.len() - 6, spec.chains[0].rules.len());
        assert_eq!(acks, body.len(), "one acknowledgement per message in the batch");
        // the chain: output hook (3), priority -10 as a signed big-endian number, policy accept, type filter
        let chain = &m.iter().find(|(t, _)| t & 0xff == 3 && t >> 8 == 10).unwrap().1;
        assert_eq!(name_of(chain, 1).as_deref(), Some("iyakki_vpn"));
        assert_eq!(name_of(chain, 3).as_deref(), Some("output"));
        assert_eq!(name_of(chain, 7).as_deref(), Some("filter"));
        let hook = attrs(&chain[4..]).into_iter().find(|(t, _)| *t == 4).unwrap().1;
        let h = attrs(hook);
        assert_eq!(h[0].1, &3u32.to_be_bytes());
        assert_eq!(h[1].1, &(-10i32).to_be_bytes());
        let pol = attrs(&chain[4..]).into_iter().find(|(t, _)| *t == 5).unwrap().1;
        assert_eq!(pol, &1u32.to_be_bytes());
        // sets: timeout flag, ipv4 key 4 bytes / ipv6 key 16 bytes, ids 1 and 2
        let sets: Vec<&Vec<u8>> = m.iter().filter(|(t, _)| t & 0xff == 9).map(|(_, p)| p).collect();
        let a0 = attrs(&sets[0][4..]);
        assert_eq!(a0.iter().find(|(t, _)| *t == 3).unwrap().1, &0x10u32.to_be_bytes());
        assert_eq!(a0.iter().find(|(t, _)| *t == 5).unwrap().1, &4u32.to_be_bytes());
        assert_eq!(attrs(&sets[1][4..]).iter().find(|(t, _)| *t == 5).unwrap().1, &16u32.to_be_bytes());
        assert_eq!(attrs(&sets[1][4..]).iter().find(|(t, _)| *t == 10).unwrap().1, &2u32.to_be_bytes());
    }

    #[test]
    fn rules_encode_to_the_expected_expression_chains() {
        let (b, _) = encode_apply(&guard(), 1).unwrap();
        let rules: Vec<Vec<u8>> = msgs(&b).into_iter().filter(|(t, _)| t & 0xff == 6).map(|(_, p)| p).collect();
        let names: Vec<Vec<String>> = rules.iter().map(|r| expr_names(r)).collect();
        // lo accept: load name, compare, verdict
        assert_eq!(names[0], ["meta", "cmp", "immediate"]);
        // udp sport 68 dport 67: l4proto, cmp, payload, cmp, payload, cmp, verdict
        assert_eq!(names[1], ["meta", "cmp", "payload", "cmp", "payload", "cmp", "immediate"]);
        // ip6 daddr fe80::/10 icmpv6: nfproto, payload, bitwise, cmp, l4proto, cmp, verdict
        assert_eq!(names[2], ["meta", "cmp", "payload", "bitwise", "cmp", "meta", "cmp", "immediate"]);
        // oifname != tun, ip daddr /24: name, cmp, nfproto, cmp, payload, bitwise, cmp, verdict
        assert_eq!(names[4], ["meta", "cmp", "meta", "cmp", "payload", "bitwise", "cmp", "immediate"]);
        // the set lookup
        assert_eq!(names[5], ["meta", "cmp", "meta", "cmp", "payload", "lookup", "immediate"]);
        assert_eq!(names[6], ["immediate"]);
    }

    #[test]
    fn address_compares_are_masked_and_names_are_padded() {
        let (b, _) = encode_apply(&guard(), 1).unwrap();
        // fe80::/10 compared as fe 80 00..., a /24 as 203.0.113.0; "lo" padded to the 16-byte interface name
        let want_v6 = {
            let mut v = vec![0u8; 16];
            v[0] = 0xfe;
            v[1] = 0x80;
            v
        };
        assert!(b.windows(16).any(|w| w == want_v6.as_slice()));
        assert!(b.windows(4).any(|w| w == [203, 0, 113, 0]));
        let mut lo = b"lo\0".to_vec();
        lo.resize(16, 0);
        assert!(b.windows(16).any(|w| w == lo.as_slice()));
        // the prefix match compares exactly the prefix bytes
        assert!(b.windows(3).any(|w| w == b"cni"));
    }

    fn isolation() -> TableSpec {
        TableSpec {
            name: "iyakki_share_br".into(),
            sets: vec![],
            chains: vec![ChainSpec {
                name: "forward".into(),
                kind: ChainKind::Filter,
                hook: Hook::Forward,
                priority: -200,
                policy_accept: true,
                rules: vec![vec![Expr::IifIs(IfName::Exact("bnep0".into())), Expr::Drop], vec![Expr::IifIs(IfName::Prefix("bnep".into())), Expr::OifIs(IfName::Prefix("bnep".into())), Expr::Drop]],
            }],
        }
    }

    #[test]
    fn a_bridge_table_is_written_in_the_bridge_family_on_the_forward_hook_with_interface_names_only() {
        let (b, acks) = encode_apply_in(&isolation(), Family::Bridge, 1).unwrap();
        let m = msgs(&b);
        // every table message carries the bridge family (7); the two batch markers carry the unspecified family
        let fams: Vec<u8> = m.iter().filter(|(t, _)| (t >> 8) == 10).map(|(_, p)| p[0]).collect();
        assert_eq!(fams.len(), 3 + 1 + 2, "table x3, chain, rules");
        assert!(fams.iter().all(|f| *f == 7), "{fams:?}");
        assert_eq!(acks, 6);
        // the same spec in the inet family says 1, so the family is really what changed
        let (inet, _) = encode_apply_in(&isolation(), Family::Inet, 1).unwrap();
        assert!(msgs(&inet).iter().filter(|(t, _)| (t >> 8) == 10).all(|(_, p)| p[0] == 1));
        // the chain hooks on forward (2) at priority -200 (the bridge filter priority)
        let chain = &m.iter().find(|(t, _)| t & 0xff == 3 && t >> 8 == 10).unwrap().1;
        let hook = attrs(&chain[4..]).into_iter().find(|(t, _)| *t == 4).unwrap().1.to_vec();
        let h = attrs(&hook);
        assert_eq!(h.iter().find(|(t, _)| *t == 1).unwrap().1, &2u32.to_be_bytes());
        assert_eq!(h.iter().find(|(t, _)| *t == 2).unwrap().1, &(-200i32).to_be_bytes());
        // a rule is: load the incoming interface name, compare, drop
        let rules: Vec<Vec<String>> = m.iter().filter(|(t, _)| t & 0xff == 6).map(|(_, p)| expr_names(p)).collect();
        assert_eq!(rules[0], ["meta", "cmp", "immediate"]);
        assert_eq!(rules[1], ["meta", "cmp", "meta", "cmp", "immediate"]);
        let mut n = b"bnep0\0".to_vec();
        n.resize(16, 0);
        assert!(b.windows(16).any(|w| w == n.as_slice()));
        // removal and the existence question use the family too
        let (rm, _) = encode_remove_in(Family::Bridge, "iyakki_share_br", 9).unwrap();
        assert!(msgs(&rm).iter().filter(|(t, _)| (t >> 8) == 10).all(|(_, p)| p[0] == 7));
        let get = encode_get_table_in(Family::Bridge, "iyakki_share_br", 3);
        assert_eq!(get[16], 7, "{get:?}");
        assert_eq!(encode_remove("iyakki_share_br", 9).unwrap().1, encode_remove_in(Family::Inet, "iyakki_share_br", 9).unwrap().1);
    }

    #[test]
    fn a_bridge_table_refuses_what_a_bridge_cannot_match() {
        let mut t = isolation();
        t.chains[0].rules.push(vec![Expr::Daddr("10.0.0.0".parse().unwrap(), 8), Expr::Drop]);
        assert!(encode_apply_in(&t, Family::Bridge, 1).unwrap_err().contains("interface names only"));
        assert!(encode_apply_in(&t, Family::Inet, 1).is_ok(), "the same rule is fine in inet");
        let mut t = isolation();
        t.sets.push(SetSpec { name: "s".into(), v4: true, timeout: false, members: vec![], ranges: vec![], concat: vec![] });
        assert!(encode_apply_in(&t, Family::Bridge, 1).unwrap_err().contains("no sets"));
        let mut t = isolation();
        t.chains[0].kind = ChainKind::Nat;
        t.chains[0].hook = Hook::Postrouting;
        t.chains[0].rules = vec![vec![Expr::Masquerade]];
        assert!(encode_apply_in(&t, Family::Bridge, 1).unwrap_err().contains("no nat chain"));
        // the Firewall's own table is still never written through this one
        let mut t = isolation();
        t.name = "iyakki_fw".into();
        assert!(encode_apply_in(&t, Family::Bridge, 1).is_err());
    }

    #[test]
    fn a_nat_redirect_loads_the_port_and_names_the_redir_expression() {
        let spec = TableSpec {
            name: "iyakki_vpn_dns".into(),
            sets: vec![],
            chains: vec![ChainSpec {
                name: "output".into(),
                kind: ChainKind::Nat,
                hook: Hook::Output, priority: -100,
                policy_accept: true,
                rules: vec![vec![Expr::MarkIs(0x4956_0001), Expr::Return], vec![Expr::L4(17), Expr::Dport(53), Expr::RedirectPort(15353)], vec![Expr::L4(6), Expr::Dport(53), Expr::RedirectPort(15353)]],
            }],
        };
        let (b, acks) = encode_apply(&spec, 7).unwrap();
        let m = msgs(&b);
        let chain = &m.iter().find(|(t, _)| t & 0xff == 3 && t >> 8 == 10).unwrap().1;
        assert_eq!(name_of(chain, 7).as_deref(), Some("nat"));
        let rules: Vec<Vec<String>> = m.iter().filter(|(t, _)| t & 0xff == 6).map(|(_, p)| expr_names(p)).collect();
        assert_eq!(rules[0], ["meta", "cmp", "immediate"]);
        assert_eq!(rules[1], ["meta", "cmp", "payload", "cmp", "immediate", "redir"]);
        assert!(b.windows(2).any(|w| w == 15353u16.to_be_bytes()));
        assert!(b.windows(4).any(|w| w == 0x4956_0001u32.to_ne_bytes()));
        assert_eq!(acks, 3 + 1 + 3);
    }

    #[test]
    fn element_messages_carry_the_key_and_a_lifetime_in_milliseconds() {
        let (b, acks) = encode_elem("iyakki_vpn", "vpn_dest4", "198.51.100.7".parse().unwrap(), Some(90), true, 40).unwrap();
        assert_eq!(acks, 1);
        let m = msgs(&b);
        assert_eq!(m[1].0 & 0xff, 12);
        assert!(b.windows(4).any(|w| w == [198, 51, 100, 7]));
        assert!(b.windows(8).any(|w| w == 90_000u64.to_be_bytes()));
        let (d, _) = encode_elem("iyakki_vpn", "vpn_dest4", "198.51.100.7".parse().unwrap(), None, false, 50).unwrap();
        assert_eq!(msgs(&d)[1].0 & 0xff, 14);
        assert!(!d.windows(8).any(|w| w == 90_000u64.to_be_bytes()));
    }

    #[test]
    fn bad_specs_are_refused_before_any_byte_is_written() {
        let mut s = guard();
        s.name = "iyakki_fw".into();
        assert!(encode_apply(&s, 1).is_err(), "the Firewall's table is not ours to rewrite here");
        let mut s = guard();
        s.name = "a b".into();
        assert!(encode_apply(&s, 1).is_err());
        let mut s = guard();
        s.chains[0].rules.push(vec![Expr::Dport(53), Expr::Accept]);
        assert!(encode_apply(&s, 1).is_err(), "a port without TCP/UDP");
        let mut s = guard();
        s.chains[0].rules.push(vec![Expr::DaddrSet("nope".into()), Expr::Drop]);
        assert!(encode_apply(&s, 1).is_err(), "an unknown set");
        let mut s = guard();
        s.chains[0].rules.push(vec![Expr::L4(17), Expr::Dport(53), Expr::RedirectPort(1)]);
        assert!(encode_apply(&s, 1).is_err(), "redirect outside nat");
        let mut s = guard();
        s.chains[0].rules.push(vec![Expr::Accept, Expr::Drop]);
        assert!(encode_apply(&s, 1).is_err(), "two verdicts");
        let mut s = guard();
        s.chains[0].rules.push(vec![Expr::OifIs(IfName::Exact("a\"b".into())), Expr::Drop]);
        assert!(encode_apply(&s, 1).is_err(), "odd interface name");
        let mut s = guard();
        s.sets[0].members.push("2001:db8::1".parse().unwrap());
        assert!(encode_apply(&s, 1).is_err(), "member of the wrong family");
        assert!(encode_remove("iyakki_fw", 1).is_err());
        assert!(encode_elem("iyakki_fw", "x", "10.0.0.1".parse().unwrap(), None, true, 1).is_err());
    }

    #[test]
    fn a_batch_with_static_members_sends_them_in_the_same_transaction() {
        let mut s = guard();
        s.sets[0].members = vec!["192.0.2.1".parse().unwrap(), "192.0.2.2".parse().unwrap()];
        let (b, acks) = encode_apply(&s, 1).unwrap();
        let (b0, acks0) = encode_apply(&guard(), 1).unwrap();
        assert_eq!(acks, acks0 + 1);
        assert!(b.len() > b0.len());
        assert!(b.windows(4).any(|w| w == [192, 0, 2, 2]));
    }

    #[test]
    fn input_forward_and_postrouting_chains_use_their_hooks_and_the_new_steps() {
        let down = || IfName::Exact("enx0".into());
        let spec = TableSpec {
            name: "iyakki_share".into(),
            sets: vec![],
            chains: vec![
                ChainSpec { name: "input".into(), kind: ChainKind::Filter, hook: Hook::Input, priority: -10, policy_accept: true, rules: vec![vec![Expr::IifIs(down()), Expr::L4(17), Expr::Dport(67), Expr::Accept]] },
                ChainSpec { name: "forward".into(), kind: ChainKind::Filter, hook: Hook::Forward, priority: -10, policy_accept: true, rules: vec![vec![Expr::IifNot(down()), Expr::CtState(CT_ESTABLISHED_RELATED), Expr::Saddr("10.42.0.0".parse().unwrap(), 24), Expr::Accept]] },
                ChainSpec { name: "post".into(), kind: ChainKind::Nat, hook: Hook::Postrouting, priority: 100, policy_accept: true, rules: vec![vec![Expr::OifIs(IfName::Exact("eth0".into())), Expr::Masquerade]] },
            ],
        };
        let (b, acks) = encode_apply(&spec, 1).unwrap();
        assert_eq!(acks, 3 + 3 + 3);
        let m = msgs(&b);
        let hooks: Vec<u32> = m.iter().filter(|(t, _)| t & 0xff == 3 && t >> 8 == 10).map(|(_, p)| {
            let h = attrs(&p[4..]).into_iter().find(|(t, _)| *t & 0x7fff == 4).unwrap().1.to_vec();
            u32::from_be_bytes(attrs(&h).into_iter().find(|(t, _)| *t == 1).unwrap().1.try_into().unwrap())
        }).collect();
        assert_eq!(hooks, [1, 2, 4]);
        let rules: Vec<Vec<String>> = m.iter().filter(|(t, _)| t & 0xff == 6).map(|(_, p)| expr_names(p)).collect();
        assert_eq!(rules[0], ["meta", "cmp", "meta", "cmp", "payload", "cmp", "immediate"]);
        assert_eq!(rules[1], ["meta", "cmp", "ct", "bitwise", "cmp", "meta", "cmp", "payload", "bitwise", "cmp", "immediate"]);
        assert_eq!(rules[2], ["meta", "cmp", "masq"]);
        let mut e = b"enx0\0".to_vec();
        e.resize(16, 0);
        assert!(b.windows(16).any(|w| w == e.as_slice()));
        let mut bad = spec.clone();
        bad.chains[0].rules.push(vec![Expr::Masquerade]);
        assert!(encode_apply(&bad, 1).is_err(), "masquerade outside the nat postrouting chain");
    }
}
