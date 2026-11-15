//! The scan of the local network on the fake machine: canned subnets and hosts, a record of the scans asked for, or a real scanner behind it.

use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use iyakki_core_api::netscan::{net, Event, HostInfo, NetScan, Options};

#[derive(Default)]
pub struct FakeNetScan {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    subnets: Vec<net::Subnet>,
    hosts: Vec<HostInfo>,
    calls: Vec<String>,
    behind: Option<Arc<dyn NetScan>>,
}

impl FakeNetScan {
    pub fn set_subnets(&self, s: Vec<net::Subnet>) {
        self.state.lock().unwrap().subnets = s;
    }
    pub fn set_hosts(&self, h: Vec<HostInfo>) {
        self.state.lock().unwrap().hosts = h;
    }
    /// Let a real scanner answer (a test that scans its own loopback listener).
    pub fn behind(&self, real: Arc<dyn NetScan>) {
        self.state.lock().unwrap().behind = Some(real);
    }
    pub fn calls(&self) -> Vec<String> {
        self.state.lock().unwrap().calls.clone()
    }
}

impl NetScan for FakeNetScan {
    fn subnets(&self) -> Vec<net::Subnet> {
        let s = self.state.lock().unwrap();
        match &s.behind {
            Some(r) => r.subnets(),
            None => s.subnets.clone(),
        }
    }
    fn start(&self, targets: Vec<Ipv4Addr>, opts: Options, cancel: Arc<AtomicBool>, tx: Sender<Event>) -> Result<(), String> {
        let (behind, hosts) = {
            let mut s = self.state.lock().unwrap();
            s.calls.push(format!("start({} targets, {} ports)", targets.len(), opts.ports.len()));
            (s.behind.clone(), s.hosts.clone())
        };
        if let Some(r) = behind {
            return r.start(targets, opts, cancel, tx);
        }
        let _ = tx.send(Event::Started { hosts: targets.len(), ports: opts.ports.len() });
        let mut services = 0;
        let n = hosts.len();
        for h in hosts {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            services += h.services.len();
            let _ = tx.send(Event::Host(h));
        }
        let _ = tx.send(Event::Finished { hosts: n, services, cancelled: cancel.load(Ordering::Relaxed) });
        Ok(())
    }
}
