//! The sharing state machine. [`Manager::tick`] is called a few times a second by the service with the current request; it starts a
//! session, serves DHCP and DNS, watches the uplink and the idle time, and tears everything down again. It touches the machine only
//! through [`Os`] and [`Io`], so the tests run it against fakes.
//!
//! Rules it keeps:
//! * every change is undone in reverse order, on stop, on a failed start, on idle switch-off, when the uplink is lost and after a crash
//!   ([`Manager::recover`] reads the record written before the first change);
//! * only the table `iyakki_share` is ever removed, and `ip_forward` goes back to what it was;
//! * one session at a time; a changed request (new id) restarts it; a failed start is not retried until the request changes;
//! * audit lines carry interface names, addresses and counts, never secrets.

use super::plan::{apply_policy, make_plan, Plan};
use super::rules::table_spec;
use super::{Client, IfaceInfo, LinkKind, Request, Saved, ShareError, State, Status, ACTIVITY_BYTES, GATEWAY_NAME, LEASE_SECS};
use iyakki_fw::nl::tablegen::TableSpec;
use iyakki_hotspot::dhcp::{DhcpConfig, DhcpServer, Dest};
use iyakki_hotspot::dns::{ClientDecision, DnsConfig, Forwarder};
use std::collections::BTreeMap;
use std::net::Ipv4Addr;

/// What the service reads and changes on the machine.
pub trait Os {
    fn now_ms(&self) -> u64;
    fn random_u16(&mut self) -> u16;
    fn interfaces(&mut self) -> Vec<IfaceInfo>;
    /// Can this build start a Wi-Fi access point on some card? (No: the real netlink transport is not there yet.)
    fn wifi_ap_available(&mut self) -> bool;
    fn resolvers(&mut self) -> Vec<Ipv4Addr>;
    /// The administrator's sharing policy (read again every few seconds).
    fn policy(&mut self) -> super::policy::Policy;
    fn forward_get(&mut self) -> Result<String, String>;
    fn forward_set(&mut self, value: &str) -> Result<(), String>;
    fn link_up(&mut self, iface: &str) -> Result<(), String>;
    fn add_address(&mut self, iface: &str, ip: Ipv4Addr, prefix: u8) -> Result<(), String>;
    fn remove_address(&mut self, iface: &str, ip: Ipv4Addr, prefix: u8) -> Result<(), String>;
    /// Load the table `iyakki_share` (replacing a stale one).
    fn apply_rules(&mut self, spec: &TableSpec) -> Result<(), String>;
    /// Delete the table `iyakki_share` (nothing else). Not an error when it is not there.
    fn remove_rules(&mut self) -> Result<(), String>;
    fn rules_present(&mut self) -> Result<bool, String>;
    /// The Firewall's own table learns which interface is the shared one (DHCP and DNS in, nothing else).
    fn set_firewall_zone(&mut self, ifaces: Vec<String>);
    fn open_io(&mut self, device: &str, gateway: Ipv4Addr, resolvers: &[Ipv4Addr]) -> Result<Box<dyn Io>, String>;
    fn load_saved(&mut self) -> Option<Saved>;
    fn save_saved(&mut self, s: Option<&Saved>);
    fn audit(&mut self, line: &str);
    /// Make the hotspot's access point next to the Wi-Fi client and return its interface name.
    fn wifi_ap_start(&mut self) -> Result<String, String>;
    /// End the access point (not an error when it is not there).
    fn wifi_ap_stop(&mut self);
}

pub enum Inbound {
    Dhcp(Vec<u8>),
    DnsQuery { src: Ipv4Addr, sport: u16, data: Vec<u8> },
    DnsAnswer(Vec<u8>),
}

/// The sockets of a running session.
pub trait Io {
    fn poll(&mut self) -> Vec<Inbound>;
    fn send_dhcp(&mut self, bytes: &[u8], dest: Dest);
    fn send_dns_client(&mut self, ip: Ipv4Addr, port: u16, bytes: &[u8]);
    fn send_dns_upstream(&mut self, bytes: &[u8]);
}

/// How often the interfaces are looked at while a session runs.
const HEALTH_MS: u64 = 2000;

struct Session {
    plan: Plan,
    request_id: u64,
    by_admin: bool,
    started: u64,
    idle_hours: u8,
    io: Box<dyn Io>,
    dhcp: DhcpServer,
    dns: Forwarder,
    saved: Saved,
    last_health: u64,
    last_bytes: u64,
    base_rx: u64,
    base_tx: u64,
    rx: u64,
    tx: u64,
    last_active: u64,
    known: BTreeMap<[u8; 6], (String, Ipv4Addr)>,
}

#[derive(Default)]
pub struct Manager {
    session: Option<Session>,
    /// The request id that already failed or ended (no retry until the request changes).
    settled_for: Option<u64>,
    status: Status,
}

fn hex_mac(m: &[u8; 6]) -> String {
    m.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
}

impl Manager {
    pub fn new() -> Manager {
        Manager::default()
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn running(&self) -> bool {
        self.session.is_some()
    }

    /// At start of the service: take away what a crashed run left behind (the table, the firewall zone, the address, `ip_forward`).
    pub fn recover(&mut self, os: &mut dyn Os) {
        let had_rules = os.rules_present().unwrap_or(false);
        let saved = os.load_saved();
        if had_rules || saved.is_some() {
            let _ = os.remove_rules();
            os.set_firewall_zone(Vec::new());
            if let Some(s) = &saved {
                if s.downstream == crate::wifi_ap::AP_IFACE {
                    os.wifi_ap_stop();
                }
                let _ = os.remove_address(&s.downstream, s.address, s.prefix);
                if matches!(s.previous_forward.as_str(), "0" | "1") {
                    let _ = os.forward_set(&s.previous_forward);
                }
            }
            os.save_saved(None);
            os.audit("share: cleaned up what an earlier run left behind");
        }
    }

    /// One step. `req` is the latest request from the shell or the command line.
    pub fn tick(&mut self, os: &mut dyn Os, req: &Request) -> Status {
        let now = os.now_ms();
        if let Some(s) = &self.session {
            if !req.on || req.id != s.request_id {
                let why = if req.on { "settings changed" } else { "switched off" };
                self.stop(os, why, State::Off, String::new());
            }
        }
        if self.session.is_none() && req.on && self.settled_for != Some(req.id) {
            match self.start(os, req) {
                Ok(s) => {
                    self.session = Some(s);
                    self.settled_for = None;
                }
                Err(e) => {
                    os.audit(&format!("share: could not start: {}", e.line()));
                    self.settled_for = Some(req.id);
                    self.status = Status { state: State::Failed, note: e.line(), request_id: req.id, updated: now / 1000, idle_hours: req.idle_hours, isolate: req.isolate, ..Status::default() };
                }
            }
        }
        if self.session.is_none() {
            if !req.on && self.status.state == State::Failed {
                self.status = Status { request_id: req.id, updated: now / 1000, ..Status::default() };
            } else if !req.on && self.status.state == State::On {
                self.status = Status { request_id: req.id, updated: now / 1000, ..Status::default() };
            }
            self.status.request_id = req.id;
            self.status.updated = now / 1000;
            return self.status.clone();
        }
        self.serve(os, now);
        self.health(os, now);
        self.refresh(now);
        self.status.clone()
    }

    fn start(&mut self, os: &mut dyn Os, req: &Request) -> Result<Session, ShareError> {
        if !req.wifi {
            return self.start_plan(os, req);
        }
        // the hotspot: the access point is made first so that it is an interface like the cable of a wired share
        let ap = os.wifi_ap_start().map_err(ShareError::Machine)?;
        let req = Request { via: Some(ap), ..req.clone() };
        let r = self.start_plan(os, &req);
        if r.is_err() {
            os.wifi_ap_stop();
        }
        r
    }

    fn start_plan(&mut self, os: &mut dyn Os, req: &Request) -> Result<Session, ShareError> {
        let ifs = os.interfaces();
        let ap = os.wifi_ap_available();
        let resolvers = os.resolvers();
        let mut plan = make_plan(&ifs, req, ap, &resolvers)?;
        apply_policy(&mut plan, &os.policy(), req)?;
        // a stale table from a crash is removed first
        if os.rules_present().unwrap_or(false) {
            let _ = os.remove_rules();
        }
        let prev = os.forward_get().map_err(ShareError::Machine)?;
        let prev = if prev.trim() == "1" { "1" } else { "0" };
        let gw = plan.gateway();
        let saved = Saved { downstream: plan.down.clone(), address: gw, prefix: 24, previous_forward: prev.into() };
        os.save_saved(Some(&saved));
        let mut did_addr = false;
        let mut did_rules = false;
        let r = (|| {
            os.link_up(&plan.down).map_err(ShareError::Machine)?;
            os.add_address(&plan.down, gw, 24).map_err(|e| ShareError::Machine(format!("{}: {e}", plan.down)))?;
            did_addr = true;
            let io = os.open_io(&plan.down, gw, &plan.resolvers).map_err(ShareError::Socket)?;
            os.apply_rules(&table_spec(&plan)).map_err(ShareError::Firewall)?;
            did_rules = true;
            os.set_firewall_zone(vec![plan.down.clone()]);
            os.forward_set("1").map_err(ShareError::Machine)?;
            Ok(io)
        })();
        let io = match r {
            Ok(io) => io,
            Err(e) => {
                if did_rules {
                    let _ = os.remove_rules();
                }
                os.set_firewall_zone(Vec::new());
                let _ = os.forward_set(prev);
                if did_addr {
                    let _ = os.remove_address(&plan.down, gw, 24);
                }
                os.save_saved(None);
                return Err(e);
            }
        };
        let mut dhcp = DhcpServer::new(DhcpConfig { server_ip: gw, mask: iyakki_hotspot::subnet::HotspotNet::MASK, broadcast: plan.net.broadcast(), pool_start: plan.net.pool_start(), pool_end: plan.net.pool_end(plan.pool), lease_secs: LEASE_SECS, dns: gw });
        dhcp.allow_any_station();
        let dns = Forwarder::new(DnsConfig { server_ip: gw, subnet: plan.net.net, gateway_name: GATEWAY_NAME.into(), max_pending: 64 });
        let now = os.now_ms();
        let (rx, tx) = ifs.iter().find(|i| i.name == plan.down).map(|i| (i.rx_bytes, i.tx_bytes)).unwrap_or((0, 0));
        os.audit(&format!("share: started, internet from {} through {}, network {}/24, isolation {}, switches off after {} h idle", plan.uplink, plan.down, gw, if plan.isolate { "on" } else { "off" }, req.idle_hours));
        Ok(Session { plan, request_id: req.id, by_admin: req.by_admin, started: now, idle_hours: req.idle_hours, io, dhcp, dns, saved, last_health: now, last_bytes: rx + tx, base_rx: rx, base_tx: tx, rx: 0, tx: 0, last_active: now, known: BTreeMap::new() })
    }

    fn serve(&mut self, os: &mut dyn Os, now_ms: u64) {
        let Some(s) = self.session.as_mut() else { return };
        for ev in s.io.poll() {
            match ev {
                Inbound::Dhcp(p) => {
                    if let Some(r) = s.dhcp.handle(&p, now_ms / 1000) {
                        s.io.send_dhcp(&r.bytes, r.dest);
                    }
                }
                Inbound::DnsQuery { src, sport, data } => {
                    let id = os.random_u16();
                    match s.dns.handle_client(src, sport, &data, id, now_ms) {
                        ClientDecision::Drop => {}
                        ClientDecision::Reply(b) => s.io.send_dns_client(src, sport, &b),
                        ClientDecision::Forward(b) => s.io.send_dns_upstream(&b),
                    }
                }
                Inbound::DnsAnswer(p) => {
                    if let Some((ip, port, b)) = s.dns.handle_upstream(&p, now_ms) {
                        s.io.send_dns_client(ip, port, &b);
                    }
                }
            }
        }
    }

    /// Every couple of seconds: is the uplink still there, is the cable still there, did anyone use it, who joined or left.
    fn health(&mut self, os: &mut dyn Os, now_ms: u64) {
        let Some(s) = self.session.as_mut() else { return };
        if now_ms.saturating_sub(s.last_health) < HEALTH_MS {
            return;
        }
        s.last_health = now_ms;
        let ifs = os.interfaces();
        let up = ifs.iter().find(|i| i.name == s.plan.uplink);
        let down = ifs.iter().find(|i| i.name == s.plan.down);
        let policy = os.policy();
        let blocked = policy.may_start(s.plan.kind, s.by_admin).and_then(|_| policy.check_names(&s.plan.down, &s.plan.uplink)).err();
        let too_long = policy.max_hours > 0 && now_ms.saturating_sub(s.started) / 1000 >= u64::from(policy.max_hours) * 3600;
        let verdict: Option<(ShareError, &str)> = if let Some(e) = blocked {
            Some((e, "the administrator's policy no longer allows it"))
        } else if too_long {
            Some((ShareError::Blocked(format!("Sharing stopped: the administrator allows at most {} h per session.", policy.max_hours)), "the longest allowed session ended"))
        } else if !down.is_some_and(|d| d.up) {
            Some((ShareError::DownstreamLost(s.plan.down.clone()), "the shared device disappeared"))
        } else if !up.is_some_and(|u| u.default_route && u.up) {
            Some((ShareError::UplinkLost(s.plan.uplink.clone()), "the uplink lost its route"))
        } else if ifs.iter().any(|i| i.name != s.plan.down && i.cidrs.iter().any(|c| c.overlaps(&s.plan.net.net))) {
            Some((ShareError::AddressClash, "another network now uses the shared range"))
        } else {
            None
        };
        if let Some((e, why)) = verdict {
            self.stop(os, why, State::Failed, e.line());
            return;
        }
        let Some(s) = self.session.as_mut() else { return };
        if let Some(d) = down {
            let total = d.rx_bytes.saturating_add(d.tx_bytes);
            if total.saturating_sub(s.last_bytes) > ACTIVITY_BYTES {
                s.last_active = now_ms;
            }
            s.last_bytes = total;
            s.rx = d.rx_bytes.saturating_sub(s.base_rx);
            s.tx = d.tx_bytes.saturating_sub(s.base_tx);
        }
        let leases = s.dhcp.leases(now_ms / 1000);
        if !leases.is_empty() {
            s.last_active = now_ms;
        }
        let mut now_known: BTreeMap<[u8; 6], (String, Ipv4Addr)> = BTreeMap::new();
        for l in &leases {
            now_known.insert(l.mac, (l.hostname.clone(), l.ip));
        }
        let mut lines = Vec::new();
        for (m, (h, ip)) in &now_known {
            if !s.known.contains_key(m) {
                lines.push(format!("share: device joined {} {} {}", hex_mac(m), ip, if h.is_empty() { "(no name)" } else { h }));
            }
        }
        for (m, (_, ip)) in &s.known {
            if !now_known.contains_key(m) {
                lines.push(format!("share: device left {} {}", hex_mac(m), ip));
            }
        }
        s.known = now_known;
        let idle_for = now_ms.saturating_sub(s.last_active) / 1000;
        let limit = s.plan.idle_secs;
        for l in lines {
            os.audit(&l);
        }
        if idle_for >= limit {
            let h = s.idle_hours;
            self.stop(os, "idle", State::Off, format!("It switched itself off after {h} h with nobody using it."));
        }
    }

    fn refresh(&mut self, now_ms: u64) {
        let Some(s) = self.session.as_ref() else { return };
        let secs = now_ms / 1000;
        let clients: Vec<Client> = s.dhcp.leases(secs).into_iter().map(|l| Client { name: l.hostname, ip: l.ip.to_string(), mac: hex_mac(&l.mac), expires_in: l.expires.saturating_sub(secs) }).collect();
        let idle_for = now_ms.saturating_sub(s.last_active) / 1000;
        let off_in = Some(s.plan.idle_secs.saturating_sub(idle_for));
        self.status = Status {
            state: State::On,
            uplink: s.plan.uplink.clone(),
            downstream: s.plan.down.clone(),
            subnet: format!("{}/24", Ipv4Addr::from(s.plan.net.net.network())),
            gateway: s.plan.gateway().to_string(),
            clients,
            rx_bytes: s.rx,
            tx_bytes: s.tx,
            idle_hours: s.idle_hours,
            isolate: s.plan.isolate,
            off_in_secs: off_in,
            note: String::new(),
            request_id: s.request_id,
            updated: secs,
        };
    }

    /// Undo everything in reverse order and leave the status as `state` with `note`.
    fn stop(&mut self, os: &mut dyn Os, why: &str, state: State, note: String) {
        let Some(s) = self.session.take() else { return };
        let _ = os.forward_set(&s.saved.previous_forward);
        let _ = os.remove_rules();
        os.set_firewall_zone(Vec::new());
        let n = s.dhcp.leases(os.now_ms() / 1000).len();
        drop(s.io);
        let _ = os.remove_address(&s.saved.downstream, s.saved.address, s.saved.prefix);
        if s.plan.kind == LinkKind::Wifi {
            os.wifi_ap_stop();
        }
        os.save_saved(None);
        os.audit(&format!("share: stopped ({why}), {n} device(s) were connected, {} bytes in, {} bytes out", s.rx, s.tx));
        self.settled_for = Some(s.request_id);
        self.status = Status { state, note, request_id: s.request_id, updated: os.now_ms() / 1000, idle_hours: s.idle_hours, isolate: s.plan.isolate, ..Status::default() };
    }

    /// Stop now (the service is shutting down).
    pub fn shutdown(&mut self, os: &mut dyn Os) {
        self.stop(os, "service stopped", State::Off, String::new());
    }
}
