//! Brings a direct link up when its interface appears, without a DHCP server, and keeps the table of peers.
//!
//! One [`Manager::tick`] (every two seconds in [`start`]) does, for every interface [`super::classify`] calls a direct link:
//! switch it up and forbid forwarding on it (nothing is routed or translated between the cable and any other network); once the
//! cable has a link, take an IPv4 link-local address after ARP probing and leave the IPv6 link-local address to the kernel; send
//! the beacon, read the beacons that arrived, and (when asked in Settings) add the fixed private pair once the peer's MAC is known.
//! A USB network interface that might also be an ordinary adapter (cdc_ether, r8152, ...) is only treated this way after
//! [`super::DHCP_GRACE_SECS`] seconds without an address, and is let go the moment a real address arrives.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::sync::Arc;

use super::beacon::Beacon;
use super::linklocal::{self, parse_mac};
use super::{classify, Config, Env, LinkInfo, Peer, Snapshot, Transport};

/// Seconds between beacons, and how long a peer stays in the table after its last beacon.
pub const BEACON_EVERY: i64 = 3;
pub const PEER_TTL: i64 = 15;
const MAX_PEERS: usize = 16;

#[derive(Clone, Debug, Default)]
struct Link {
    transport: Option<Transport>,
    /// First time the cable had a link and no address (for the grace period of ambiguous interfaces).
    waiting_since: Option<i64>,
    managed: bool,
    link_local: Option<Ipv4Addr>,
    private: Option<Ipv4Addr>,
    last_beacon: i64,
    /// Next time to try again after the link-local probes failed.
    retry_at: i64,
}

#[derive(Default)]
pub struct Manager {
    links: BTreeMap<String, Link>,
    peers: Vec<Peer>,
}

impl Manager {
    pub fn new() -> Manager {
        Manager::default()
    }

    /// The interfaces the firewall treats as direct links right now.
    pub fn zone(&self) -> Vec<String> {
        self.links.iter().filter(|(_, l)| l.managed).map(|(n, _)| n.clone()).collect()
    }

    pub fn tick(&mut self, env: &dyn Env, cfg: &Config) -> Snapshot {
        let now = env.now();
        let udc = env.udc_present();
        let ifs = env.net_ifaces();
        // forget interfaces that went away
        self.links.retain(|n, _| ifs.iter().any(|i| &i.name == n));
        self.peers.retain(|p| ifs.iter().any(|i| i.name == p.iface) && now - p.last_seen <= PEER_TTL);
        let me = env.me();
        for i in &ifs {
            let Some(t) = classify(i, udc) else {
                self.links.remove(&i.name);
                continue;
            };
            if !cfg.enabled {
                self.links.remove(&i.name);
                continue;
            }
            let l = self.links.entry(i.name.clone()).or_default();
            l.transport = Some(t);
            let real_addr = i.ipv4.iter().any(|a| !linklocal::is_link_local(*a));
            if t == Transport::UsbNet {
                if real_addr {
                    // DHCP (or a person) gave it a real address: an ordinary network, not a direct link
                    if l.managed {
                        let _ = env.del_ipv4(&i.name, 0);
                    }
                    *l = Link::default();
                    continue;
                }
                if !i.carrier {
                    l.waiting_since = None;
                    continue;
                }
                let since = *l.waiting_since.get_or_insert(now);
                if !l.managed && now - since < super::DHCP_GRACE_SECS {
                    continue;
                }
            } else if real_addr && !l.managed {
                // a person or a DHCP client already configured it: leave it alone
                continue;
            }
            if !i.up {
                let _ = env.set_up(&i.name);
            }
            if !l.managed {
                let _ = env.no_forwarding(&i.name);
                l.managed = true;
            }
            if !i.carrier {
                continue;
            }
            let Some(mac) = parse_mac(&i.mac) else { continue };
            if l.link_local.is_none() {
                if let Some(existing) = i.ipv4.iter().copied().find(|a| linklocal::is_link_local(*a)) {
                    l.link_local = Some(existing);
                } else if now >= l.retry_at {
                    match linklocal::acquire(env, &i.name, mac, linklocal::MAX_ATTEMPTS) {
                        Ok(ip) => {
                            if env.add_ipv4(&i.name, 0, ip, 16).is_ok() {
                                env.arp_announce(&i.name, mac, ip);
                                l.link_local = Some(ip);
                            } else {
                                l.retry_at = now + 10;
                            }
                        }
                        Err(_) => l.retry_at = now + 30,
                    }
                }
            }
            // beacons
            if let Some(local) = l.link_local.or(l.private) {
                if now - l.last_beacon >= BEACON_EVERY {
                    let mut b = me.clone();
                    b.mac = i.mac.clone();
                    let _ = env.beacon_send(&i.name, local, &b.encode());
                    l.last_beacon = now;
                }
            }
            for (src, bytes) in env.beacon_recv(&i.name).into_iter().take(64) {
                let Ok(b) = Beacon::parse(&bytes) else { continue };
                if b.id == me.id || b.mac == i.mac {
                    continue; // our own packet
                }
                let known = self.peers.iter().position(|p| p.iface == i.name && p.beacon.id == b.id);
                match known {
                    Some(k) => self.peers[k] = Peer { iface: i.name.clone(), addr: src, beacon: b, last_seen: now },
                    None if self.peers.len() < MAX_PEERS => self.peers.push(Peer { iface: i.name.clone(), addr: src, beacon: b, last_seen: now }),
                    None => {}
                }
            }
            // the fixed private pair, when asked: agreed from the two MAC addresses
            if cfg.fixed_pair && l.private.is_none() {
                if let Some(p) = self.peers.iter().find(|p| p.iface == i.name) {
                    if let Some((mine, _)) = parse_mac(&p.beacon.mac).and_then(|pm| linklocal::private_pair(&mac, &pm)) {
                        if env.add_ipv4(&i.name, 1, mine, 30).is_ok() {
                            l.private = Some(mine);
                        }
                    }
                }
            } else if !cfg.fixed_pair && l.private.is_some() {
                let _ = env.del_ipv4(&i.name, 1);
                l.private = None;
            }
        }
        self.snapshot(env, &ifs, now)
    }

    fn snapshot(&self, env: &dyn Env, ifs: &[super::NetIf], now: i64) -> Snapshot {
        let _ = env;
        let links = self
            .links
            .iter()
            .filter(|(_, l)| l.managed)
            .filter_map(|(n, l)| {
                let i = ifs.iter().find(|i| &i.name == n)?;
                Some(LinkInfo { name: n.clone(), transport: l.transport?, carrier: i.carrier, speed_mbps: i.speed_mbps, link_local: l.link_local, private: l.private, ipv6_ll: i.ipv6_ll.clone(), mac: i.mac.clone() })
            })
            .collect();
        Snapshot { at: now, links, peers: self.peers.clone() }
    }
}

/// Start the thread that keeps links up: every two seconds a tick; the state goes to [`super::STATE_FILE`] and the firewall is told
/// which interfaces are direct links (`on_zone`: interface names and the "allow more" switch). Call it once.
pub fn start(env: Arc<dyn Env>, on_zone: Arc<dyn Fn(Vec<String>, bool) + Send + Sync>) {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let _ = std::thread::Builder::new().name("iyakki-directlink".into()).spawn(move || {
            let mut m = Manager::new();
            loop {
                let cfg = env.config();
                let snap = m.tick(env.as_ref(), &cfg);
                env.write_snapshot(&snap);
                on_zone(m.zone(), cfg.allow_more);
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        });
    });
}

/// [`start`] on the real machine: the link manager for this computer, with its settings and state in `data`. `on_zone` is how the
/// firewall hears about direct links (`iyakki_fw::service::set_direct_links`). Call it once, as the machine's first process.
pub fn start_system(data: &std::path::Path, on_zone: Arc<dyn Fn(Vec<String>, bool) + Send + Sync>) {
    start(Arc::new(super::real::RealEnv::new(data, &data.join("devmode/client/root"))), on_zone);
}
