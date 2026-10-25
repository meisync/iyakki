//! A made-up machine for the direct-link tests (the engine's and Settings'): interfaces, a Thunderbolt tree, a socket for beacons, a
//! developer-mode peer and a pairing that only record. Nothing here opens a socket, writes a file or runs a program.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::sync::Mutex;

use super::beacon::Beacon;
use super::linklocal::Probe;
use super::tb::TbView;
use super::{Config, Env, Hello, NetIf, Peer, Snapshot, SpeedSample};

pub struct St {
    pub now: i64,
    pub ifaces: Vec<NetIf>,
    pub udc: bool,
    pub tb: TbView,
    pub keys: BTreeMap<String, String>,
    pub taken: Vec<Ipv4Addr>,
    pub probes: Vec<(String, Ipv4Addr)>,
    pub sent: Vec<(String, Ipv4Addr, Vec<u8>)>,
    pub inbox: BTreeMap<String, Vec<(Ipv4Addr, Vec<u8>)>>,
    pub discovered: Vec<Peer>,
    pub snapshot: Option<Snapshot>,
    pub config: Config,
    pub me: Beacon,
    pub hello: Result<Hello, String>,
    pub paired: Option<String>,
    pub pair_code: String,
    pub speed: Result<SpeedSample, String>,
    pub rtt: Result<f64, String>,
    /// Every change, in order: "up X", "add X ip/prefix", "del X", "noforward X", "authorize D n [key]", "pair", "announce X".
    pub calls: Vec<String>,
    pub fail_add: bool,
    pub fail_auth: bool,
    pub probe_fails: bool,
}

pub struct FakeEnv(pub Mutex<St>);

pub fn me() -> Beacon {
    Beacon { host: "dev-pc".into(), version: "0.1.0-test".into(), role: "host".into(), dev_on: false, pairing_open: false, mac: String::new(), id: "00112233445566778899aabbccddeeff".into(), port: super::DEV_PORT }
}

impl FakeEnv {
    pub fn new() -> FakeEnv {
        FakeEnv(Mutex::new(St {
            now: 1000,
            ifaces: Vec::new(),
            udc: false,
            tb: TbView::default(),
            keys: BTreeMap::new(),
            taken: Vec::new(),
            probes: Vec::new(),
            sent: Vec::new(),
            inbox: BTreeMap::new(),
            discovered: Vec::new(),
            snapshot: None,
            config: Config::default(),
            me: me(),
            hello: Err("connection refused".into()),
            paired: None,
            pair_code: "K7QFM-2XPRT".into(),
            speed: Err("not measured".into()),
            rtt: Ok(0.3),
            calls: Vec::new(),
            fail_add: false,
            fail_auth: false,
            probe_fails: false,
        }))
    }
    pub fn with<R>(&self, f: impl FnOnce(&mut St) -> R) -> R {
        f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
    pub fn calls(&self) -> Vec<String> {
        self.with(|s| s.calls.clone())
    }
}

impl Default for FakeEnv {
    fn default() -> FakeEnv {
        FakeEnv::new()
    }
}

pub fn iface(name: &str, driver: &str, mac: &str, up: bool, carrier: bool) -> NetIf {
    NetIf { name: name.into(), driver: driver.into(), mac: mac.into(), up, carrier, speed_mbps: if carrier { Some(10000) } else { None }, ipv4: Vec::new(), ipv6_ll: Vec::new() }
}

impl Env for FakeEnv {
    fn now(&self) -> i64 {
        self.with(|s| s.now)
    }
    fn net_ifaces(&self) -> Vec<NetIf> {
        self.with(|s| s.ifaces.clone())
    }
    fn udc_present(&self) -> bool {
        self.with(|s| s.udc)
    }
    fn tb(&self) -> TbView {
        self.with(|s| s.tb.clone())
    }
    fn tb_write_authorized(&self, device: &str, level: u8, key: Option<&str>) -> Result<(), String> {
        self.with(|s| {
            if s.fail_auth {
                return Err("Operation not permitted".into());
            }
            s.calls.push(format!("authorize {device} {level}{}", key.map(|k| format!(" key={}", &k[..k.len().min(4)])).unwrap_or_default()));
            if let Some(d) = s.tb.devices.iter_mut().find(|d| d.name == device) {
                d.authorized = Some(level);
            }
            Ok(())
        })
    }
    fn tb_key(&self, unique_id: &str) -> Option<String> {
        self.with(|s| s.keys.get(unique_id).cloned())
    }
    fn tb_store_key(&self, unique_id: &str, key: &str) -> Result<(), String> {
        self.with(|s| s.keys.insert(unique_id.to_string(), key.to_string()));
        Ok(())
    }
    fn random_hex32(&self) -> String {
        "ab".repeat(32)
    }
    fn set_up(&self, name: &str) -> Result<(), String> {
        self.with(|s| {
            s.calls.push(format!("up {name}"));
            if let Some(i) = s.ifaces.iter_mut().find(|i| i.name == name) {
                i.up = true;
            }
            Ok(())
        })
    }
    fn add_ipv4(&self, name: &str, label: u8, ip: Ipv4Addr, prefix: u8) -> Result<(), String> {
        self.with(|s| {
            if s.fail_add {
                return Err("busy".into());
            }
            s.calls.push(format!("add {name}:{label} {ip}/{prefix}"));
            if let Some(i) = s.ifaces.iter_mut().find(|i| i.name == name) {
                i.ipv4.push(ip);
            }
            Ok(())
        })
    }
    fn del_ipv4(&self, name: &str, label: u8) -> Result<(), String> {
        self.with(|s| {
            s.calls.push(format!("del {name}:{label}"));
            if let Some(i) = s.ifaces.iter_mut().find(|i| i.name == name) {
                if label == 0 {
                    i.ipv4.clear();
                } else {
                    i.ipv4.retain(|a| a.octets()[0] != 10);
                }
            }
            Ok(())
        })
    }
    fn no_forwarding(&self, name: &str) -> Result<(), String> {
        self.with(|s| s.calls.push(format!("noforward {name}")));
        Ok(())
    }
    fn arp_probe(&self, name: &str, _mac: [u8; 6], ip: Ipv4Addr) -> Probe {
        self.with(|s| {
            s.probes.push((name.to_string(), ip));
            if s.probe_fails {
                Probe::Failed
            } else if s.taken.contains(&ip) {
                Probe::InUse([2, 0, 0, 0, 0, 9])
            } else {
                Probe::Free
            }
        })
    }
    fn arp_announce(&self, name: &str, _mac: [u8; 6], _ip: Ipv4Addr) {
        self.with(|s| s.calls.push(format!("announce {name}")));
    }
    fn beacon_send(&self, name: &str, local: Ipv4Addr, payload: &[u8]) -> Result<(), String> {
        self.with(|s| s.sent.push((name.to_string(), local, payload.to_vec())));
        Ok(())
    }
    fn beacon_recv(&self, name: &str) -> Vec<(Ipv4Addr, Vec<u8>)> {
        self.with(|s| s.inbox.remove(name).unwrap_or_default())
    }
    fn discover(&self, _ms: u64) -> Vec<Peer> {
        self.with(|s| s.discovered.clone())
    }
    fn snapshot(&self) -> Option<Snapshot> {
        self.with(|s| s.snapshot.clone())
    }
    fn write_snapshot(&self, snap: &Snapshot) {
        self.with(|s| s.snapshot = Some(snap.clone()));
    }
    fn config(&self) -> Config {
        self.with(|s| s.config)
    }
    fn save_config(&self, c: &Config) -> Result<(), String> {
        self.with(|s| s.config = *c);
        Ok(())
    }
    fn me(&self) -> Beacon {
        self.with(|s| s.me.clone())
    }
    fn dev_hello(&self, _ip: Ipv4Addr) -> Result<Hello, String> {
        self.with(|s| s.hello.clone())
    }
    fn paired_as(&self, _ip: Ipv4Addr) -> Option<String> {
        self.with(|s| s.paired.clone())
    }
    fn pair(&self, _ip: Ipv4Addr, code: &str, expect_fingerprint: &str) -> Result<String, String> {
        self.with(|s| {
            s.calls.push(format!("pair {code} {expect_fingerprint}"));
            if code != s.pair_code {
                return Err("the code is wrong".into());
            }
            s.paired = Some("other-box".into());
            Ok("Paired with other-box.".into())
        })
    }
    fn tcp_rtt_ms(&self, _ip: Ipv4Addr, _port: u16) -> Result<f64, String> {
        self.with(|s| s.rtt.clone())
    }
    fn speed(&self, _ip: Ipv4Addr) -> Result<SpeedSample, String> {
        self.with(|s| s.speed.clone())
    }
}
