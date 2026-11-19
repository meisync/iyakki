//! DNS-driven routing for HYBRID mode, as a **pure state machine**. The executor (a later step) owns the sockets and the clock;
//! it feeds [`Event`]s in and carries out the returned [`Action`]s in order.
//!
//! How it works: a local forwarder receives the machine's DNS queries (the capture rule is [`crate::plan::dns_capture`]; the
//! system `resolv.conf` is **not** rewritten). A name the profile's filters send through the VPN is asked of the VPN's DNS
//! server *through the tunnel*; every other name goes to the system resolver unchanged. The addresses in a tunnel answer
//! become **host routes through the tunnel interface**, with the answer's TTL, a lifetime cap and a cap on the number of
//! routes. The route (and the fail-closed set entry) is added *before* the answer is handed back to the program, so the first
//! connection already goes the right way.
//!
//! Safety rules enforced here (each has a test):
//! * a tunnel answer that names an address inside the cluster, the LAN, loopback, link-local, a private range that is not the
//!   tunnel's own, or the VPN server itself is **refused** (SERVFAIL): this is the DNS-rebinding / route-hijack case;
//! * an answer with more than [`MAX_ADDRS_PER_ANSWER`] addresses is refused rather than partly routed;
//! * a reply is accepted only from the upstream the query went to, with the id we chose and the same question;
//! * the number of open queries and of routes is capped, and everything times out;
//! * with the tunnel down and fail-closed on, a name that belongs in the tunnel is answered REFUSED and never sent anywhere;
//!   with fail-closed off it is resolved by the system resolver (direct);
//! * an address that an explicit exclusion covers gets no route.
//!
//! Limits: DNS over HTTPS/TLS and programs with their own resolver are not seen; CNAME targets are not matched.

use std::collections::BTreeMap;
use std::net::IpAddr;

use crate::dnswire::{self, RCODE_REFUSED, RCODE_SERVFAIL};
use crate::filter::{FilterSet, Reason, Route};
use crate::net::{check_route, Cidr, Context, RouteVerdict};

pub const MAX_ADDRS_PER_ANSWER: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Upstream {
    /// The VPN's DNS server, asked through the tunnel.
    Vpn,
    /// The resolver the system used before (read from `resolv.conf` at start, never changed).
    System,
}

#[derive(Clone, Debug)]
pub struct Config {
    /// Never resolve or leak a tunnel name when the tunnel is down.
    pub fail_closed: bool,
    pub max_routes: usize,
    pub max_pending: usize,
    pub min_ttl: u32,
    pub max_ttl: u32,
    /// Seconds a route outlives the TTL, so a connection made just before expiry is not cut.
    pub grace: u32,
    pub query_timeout: u64,
    /// The VPN server addresses: never routed through the tunnel (that would loop).
    pub endpoints: Vec<IpAddr>,
    /// Cluster CIDRs, LAN subnets, the tunnel's own networks.
    pub ctx: Context,
}

impl Config {
    pub fn new(fail_closed: bool, endpoints: Vec<IpAddr>, ctx: Context) -> Config {
        Config { fail_closed, max_routes: 256, max_pending: 256, min_ttl: 30, max_ttl: 3600, grace: 30, query_timeout: 5, endpoints, ctx }
    }
}

#[derive(Clone, Debug)]
pub enum Event {
    /// The tunnel came up or went down.
    TunnelState(bool),
    /// A program asked. `upstream_id` is a fresh random transaction id chosen by the executor.
    Query { client: u64, packet: Vec<u8>, upstream_id: u16, now: u64 },
    Reply { from: Upstream, packet: Vec<u8>, now: u64 },
    /// Time passed: expire queries and routes.
    Tick { now: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Send { to: Upstream, packet: Vec<u8> },
    Respond { client: u64, packet: Vec<u8> },
    /// Add a host route through the tunnel interface **and** the address to the fail-closed set, until `expires` (seconds).
    RouteAdd { ip: IpAddr, expires: u64 },
    RouteDel { ip: IpAddr },
    /// For the log (never contains a key; contains names only as short as the question).
    Note(String),
}

struct Pending {
    id: u16,
    upstream: Upstream,
    client: u64,
    client_id: u16,
    query: Vec<u8>,
    name: String,
    qtype: u16,
    qclass: u16,
    deadline: u64,
}

pub struct Forwarder {
    cfg: Config,
    filters: FilterSet,
    tunnel_up: bool,
    pending: Vec<Pending>,
    routes: BTreeMap<IpAddr, u64>,
}

fn note_name(n: &str) -> String {
    n.chars().take(80).collect()
}

impl Forwarder {
    pub fn new(cfg: Config, filters: FilterSet) -> Forwarder {
        Forwarder { cfg, filters, tunnel_up: false, pending: Vec::new(), routes: BTreeMap::new() }
    }

    /// The routes now learned, with their expiry.
    pub fn routes(&self) -> Vec<(IpAddr, u64)> {
        self.routes.iter().map(|(a, e)| (*a, *e)).collect()
    }
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
    pub fn tunnel_up(&self) -> bool {
        self.tunnel_up
    }

    pub fn handle(&mut self, ev: Event) -> Vec<Action> {
        match ev {
            Event::TunnelState(up) => self.tunnel_state(up),
            Event::Query { client, packet, upstream_id, now } => self.query(client, &packet, upstream_id, now),
            Event::Reply { from, packet, now } => self.reply(from, &packet, now),
            Event::Tick { now } => self.tick(now),
        }
    }

    fn tunnel_state(&mut self, up: bool) -> Vec<Action> {
        let mut out = Vec::new();
        self.tunnel_up = up;
        if !up {
            // the routes belong to the interface that is gone; the learned names are asked again when needed
            for (ip, _) in std::mem::take(&mut self.routes) {
                out.push(Action::RouteDel { ip });
            }
            if self.cfg.fail_closed {
                let (dead, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending).into_iter().partition(|p| p.upstream == Upstream::Vpn);
                self.pending = keep;
                for p in dead {
                    out.extend(Self::fail(&p, RCODE_REFUSED));
                }
            }
        }
        out
    }

    fn fail(p: &Pending, rcode: u8) -> Option<Action> {
        let mut r = dnswire::error_response(&p.query, rcode).ok()?;
        r[0..2].copy_from_slice(&p.client_id.to_be_bytes());
        Some(Action::Respond { client: p.client, packet: r })
    }

    fn query(&mut self, client: u64, packet: &[u8], upstream_id: u16, now: u64) -> Vec<Action> {
        let (client_id, q) = match dnswire::parse_query(packet) {
            Ok(x) => x,
            Err(e) => return vec![Action::Note(format!("dropped a query that is not understood ({e:?})"))],
        };
        let d = self.filters.decide_domain(&q.name);
        let mut upstream = if d.route == Route::Tunnel { Upstream::Vpn } else { Upstream::System };
        if upstream == Upstream::Vpn && !self.tunnel_up {
            if self.cfg.fail_closed {
                let reply = dnswire::error_response(packet, RCODE_REFUSED).ok().map(|packet| Action::Respond { client, packet });
                return reply.into_iter().chain([Action::Note(format!("{} is a VPN name and the tunnel is down: refused", note_name(&q.name)))]).collect();
            }
            upstream = Upstream::System;
        }
        if self.pending.len() >= self.cfg.max_pending {
            let reply = dnswire::error_response(packet, RCODE_SERVFAIL).ok().map(|packet| Action::Respond { client, packet });
            return reply.into_iter().collect();
        }
        // a free transaction id for this upstream
        let mut id = upstream_id;
        for _ in 0..=u16::MAX as u32 {
            if !self.pending.iter().any(|p| p.id == id && p.upstream == upstream) {
                break;
            }
            id = id.wrapping_add(1);
        }
        self.pending.push(Pending { id, upstream, client, client_id, query: packet.to_vec(), name: q.name, qtype: q.qtype, qclass: q.qclass, deadline: now + self.cfg.query_timeout });
        vec![Action::Send { to: upstream, packet: dnswire::with_id(packet, id) }]
    }

    /// Is this address one the tunnel may carry for a name? `Err` carries the reason.
    fn address_ok(&self, ip: IpAddr) -> Result<(), String> {
        let ip = ip.to_canonical();
        if self.cfg.endpoints.contains(&ip) {
            return Err("the answer names the VPN server itself".into());
        }
        let host = Cidr::new(ip, if ip.is_ipv4() { 32 } else { 128 }).unwrap();
        match check_route(&host, &self.cfg.ctx) {
            RouteVerdict::Ok => {}
            RouteVerdict::Refuse(w) | RouteVerdict::Warn(w) => return Err(w),
        }
        if ip.is_unspecified() || ip.is_multicast() {
            return Err("the answer names an address that cannot be a destination".into());
        }
        Ok(())
    }

    fn reply(&mut self, from: Upstream, packet: &[u8], now: u64) -> Vec<Action> {
        let Ok(resp) = dnswire::parse_response(packet) else {
            return vec![Action::Note("ignored a reply that is not understood".into())];
        };
        let Some(pos) = self.pending.iter().position(|p| p.id == resp.id && p.upstream == from && p.name == resp.question.name && p.qtype == resp.question.qtype && p.qclass == resp.question.qclass) else {
            return vec![Action::Note("ignored a reply that matches no open query".into())];
        };
        let p = self.pending.remove(pos);
        let mut out = Vec::new();
        if from == Upstream::Vpn && resp.rcode == 0 && !resp.answers.is_empty() {
            if resp.answers.len() > MAX_ADDRS_PER_ANSWER {
                out.push(Action::Note(format!("{}: too many addresses in one answer; refused", note_name(&p.name))));
                out.extend(Self::fail(&p, RCODE_SERVFAIL));
                return out;
            }
            for a in &resp.answers {
                if let Err(why) = self.address_ok(a.ip) {
                    out.push(Action::Note(format!("{}: answer refused ({why})", note_name(&p.name))));
                    out.extend(Self::fail(&p, RCODE_SERVFAIL));
                    return out;
                }
            }
            for a in &resp.answers {
                let ip = a.ip.to_canonical();
                // an explicit exclusion on the address wins: no route, the program connects direct
                let d = self.filters.decide_ip(ip);
                if d.route == Route::Direct && matches!(d.reason, Reason::Rule { include: false, .. }) {
                    out.push(Action::Note(format!("{}: {ip} is excluded by an address rule; no route", note_name(&p.name))));
                    continue;
                }
                let ttl = a.ttl.clamp(self.cfg.min_ttl, self.cfg.max_ttl) as u64 + self.cfg.grace as u64;
                let expires = now + ttl;
                match self.routes.get(&ip) {
                    Some(&e) if e >= expires => continue,
                    Some(_) => {}
                    None => {
                        if self.routes.len() >= self.cfg.max_routes {
                            // make room by dropping the route that expires first
                            if let Some((&old, _)) = self.routes.iter().min_by_key(|(_, e)| **e) {
                                self.routes.remove(&old);
                                out.push(Action::RouteDel { ip: old });
                            }
                        }
                    }
                }
                self.routes.insert(ip, expires);
                out.push(Action::RouteAdd { ip, expires });
            }
        }
        let mut pkt = packet.to_vec();
        pkt[0..2].copy_from_slice(&p.client_id.to_be_bytes());
        out.push(Action::Respond { client: p.client, packet: pkt });
        out
    }

    fn tick(&mut self, now: u64) -> Vec<Action> {
        let mut out = Vec::new();
        let (late, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending).into_iter().partition(|p| p.deadline <= now);
        self.pending = keep;
        for p in late {
            out.extend(Self::fail(&p, RCODE_SERVFAIL));
        }
        let gone: Vec<IpAddr> = self.routes.iter().filter(|(_, e)| **e <= now).map(|(a, _)| *a).collect();
        for ip in gone {
            self.routes.remove(&ip);
            out.push(Action::RouteDel { ip });
        }
        out
    }
}
