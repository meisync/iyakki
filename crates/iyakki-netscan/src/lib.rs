//! A scanner for the local network. It finds the machines that answer, and says what runs on them: Ollama and other model
//! servers (with the models they hold), cameras and recorders (RTSP, ONVIF), media servers, printers, smart-home hubs,
//! databases, and a few things worth a second look (an open Docker API, a Redis without a password).
//!
//! What it does and does not do:
//! * It stays on the local network: private and link-local addresses, and this machine. Other addresses are refused.
//! * It only asks questions anyone may ask (connect, read a banner, fetch a home page or a public information URL, one
//!   multicast announcement query). No passwords are tried, nothing is changed, nothing is exploited.
//! * It is slow on purpose: a limited number of probes at once, short timeouts, and a stop switch.

pub mod discover;
pub mod mdns;
pub mod model;
pub mod net;
pub mod probe;

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use model::{Confidence, HostInfo, Kind, Service};
pub use probe::{default_ports, Timing};

#[derive(Clone, Debug)]
pub struct Options {
    pub ports: Vec<u16>,
    pub timing: Timing,
    /// Probes in flight at once.
    pub workers: usize,
    /// Listen for SSDP, WS-Discovery and mDNS announcements (for as long as this says).
    pub announce_wait: Option<Duration>,
    /// Give up after this long.
    pub total: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Options { ports: default_ports(), timing: Timing::default(), workers: 48, announce_wait: Some(Duration::from_millis(2500)), total: Duration::from_secs(150) }
    }
}

#[derive(Clone, Debug)]
pub enum Event {
    Started { hosts: usize, ports: usize },
    /// How many of the (host, port) questions have been answered.
    Progress { done: usize, total: usize },
    /// A host, with everything known about it so far.
    Host(HostInfo),
    Finished { hosts: usize, services: usize, cancelled: bool },
}

/// Scan `targets` (already checked with [`net::parse_targets`] or taken from [`net::hosts_of`]) and send what is found.
/// Blocks until done; run it in a thread. Setting `cancel` stops it quickly.
pub fn scan(targets: Vec<Ipv4Addr>, opts: Options, cancel: Arc<AtomicBool>, tx: Sender<Event>) {
    let targets: Vec<Ipv4Addr> = targets.into_iter().filter(|ip| net::allowed(*ip)).take(net::MAX_HOSTS).collect();
    let jobs: Vec<(Ipv4Addr, u16)> = targets.iter().flat_map(|ip| opts.ports.iter().map(move |p| (*ip, *p))).collect();
    let _ = tx.send(Event::Started { hosts: targets.len(), ports: opts.ports.len() });
    let hosts: Arc<Mutex<BTreeMap<Ipv4Addr, HostInfo>>> = Arc::new(Mutex::new(BTreeMap::new()));
    let next = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(AtomicUsize::new(0));
    let started = Instant::now();
    let total = jobs.len();
    let jobs = Arc::new(jobs);

    let announce = opts.announce_wait.map(|wait| {
        let (hosts, targets, tx, cancel) = (hosts.clone(), targets.clone(), tx.clone(), cancel.clone());
        std::thread::spawn(move || {
            let (a, b, c) = (std::thread::spawn(move || discover::ssdp(wait)), std::thread::spawn(move || discover::ws_discovery(wait)), std::thread::spawn(move || discover::mdns(wait)));
            for found in [a, b, c].into_iter().flat_map(|h| h.join().unwrap_or_default()) {
                if cancel.load(Ordering::Relaxed) || !targets.contains(&found.ip) {
                    continue;
                }
                let info = {
                    let mut g = hosts.lock().unwrap();
                    let h = g.entry(found.ip).or_insert_with(|| HostInfo::new(found.ip));
                    if h.name.is_none() {
                        h.name = found.name.clone();
                    }
                    h.add(found.service);
                    h.clone()
                };
                let _ = tx.send(Event::Host(info));
            }
        })
    });

    let workers: Vec<_> = (0..opts.workers.clamp(1, 256).min(total.max(1)))
        .map(|_| {
            let (jobs, next, done, hosts, tx, cancel) = (jobs.clone(), next.clone(), done.clone(), hosts.clone(), tx.clone(), cancel.clone());
            let (timing, limit) = (opts.timing, opts.total);
            std::thread::spawn(move || loop {
                if cancel.load(Ordering::Relaxed) || started.elapsed() > limit {
                    break;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(&(ip, port)) = jobs.get(i) else { break };
                if let Some(svc) = probe::connect(ip, port, timing.connect).map(drop).and_then(|_| probe::identify(ip, port, &timing)) {
                    let info = {
                        let mut g = hosts.lock().unwrap();
                        let h = g.entry(ip).or_insert_with(|| HostInfo::new(ip));
                        h.add(svc);
                        h.clone()
                    };
                    let _ = tx.send(Event::Host(info));
                }
                let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                if d % 25 == 0 || d == total {
                    let _ = tx.send(Event::Progress { done: d, total });
                }
            })
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
    if let Some(a) = announce {
        let _ = a.join();
    }
    // hardware addresses of the neighbours that answered
    let arp = net::arp_table();
    let mut g = hosts.lock().unwrap();
    for (ip, mac) in arp {
        if let Some(h) = g.get_mut(&ip) {
            h.mac = Some(mac);
            let _ = tx.send(Event::Host(h.clone()));
        }
    }
    let services = g.values().map(|h| h.services.len()).sum();
    let _ = tx.send(Event::Finished { hosts: g.len(), services, cancelled: cancel.load(Ordering::Relaxed) });
}

/// Scan and wait for the answer: the hosts that had something to say, in address order.
pub fn scan_blocking(targets: Vec<Ipv4Addr>, opts: Options) -> Vec<HostInfo> {
    let (tx, rx) = std::sync::mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let h = std::thread::spawn(move || scan(targets, opts, cancel, tx));
    let mut all: BTreeMap<Ipv4Addr, HostInfo> = BTreeMap::new();
    for ev in rx {
        if let Event::Host(info) = ev {
            all.insert(info.ip, info);
        }
    }
    let _ = h.join();
    all.into_values().collect()
}

#[cfg(test)]
mod tests;
