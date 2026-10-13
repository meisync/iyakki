//! New-device alerts: notice machines that appear on the local network that nobody has seen before. It only listens: the
//! system's own neighbour table (`/proc/net/arp`, which fills as this machine talks to others) is read every half minute,
//! and the results of a NET SCAN can be added. The first look is taken as "what is normally here" and raises no alerts; after
//! that, a hardware address never seen before is a new device until someone marks it as trusted. Phones and laptops that
//! change their hardware address on each network (a privacy setting) will show up as new each time.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

pub const MAX_DEVICES: usize = 2000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Neighbor {
    pub ip: String,
    pub mac: String,
    pub iface: String,
}

/// The entries of a `/proc/net/arp`-style table that are complete (the machine answered).
pub fn parse_arp(text: &str) -> Vec<Neighbor> {
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            // IP address, HW type, Flags, HW address, Mask, Device
            let (ip, flags, mac, iface) = (f.first()?, f.get(2)?, f.get(3)?, f.get(5)?);
            let complete = u32::from_str_radix(flags.trim_start_matches("0x"), 16).is_ok_and(|v| v & 2 != 0);
            let mac = mac.to_lowercase();
            (complete && mac != "00:00:00:00:00:00" && mac.len() == 17 && mac.split(':').count() == 6 && mac != "ff:ff:ff:ff:ff:ff").then(|| Neighbor { ip: ip.to_string(), mac, iface: iface.to_string() })
        })
        .collect()
}

pub fn read_arp(path: &Path) -> Vec<Neighbor> {
    parse_arp(&std::fs::read_to_string(path).unwrap_or_default())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub mac: String,
    pub ip: String,
    pub iface: String,
    pub first_seen: i64,
    pub last_seen: i64,
    /// Known and expected here (everything seen on the first look is).
    pub trusted: bool,
    /// What the person calls it ("Printer", "Anna's phone").
    #[serde(default)]
    pub name: String,
}

#[derive(Serialize, Deserialize, Default)]
struct State {
    /// The first look has been taken.
    learned: bool,
    #[serde(default = "yes")]
    enabled: bool,
    devices: BTreeMap<String, Device>,
}

fn yes() -> bool {
    true
}

pub struct DeviceWatch {
    path: PathBuf,
    state: Mutex<State>,
    /// Devices seen for the first time that nobody has been told about yet.
    fresh: Mutex<Vec<Device>>,
}

impl DeviceWatch {
    pub fn open(path: &Path) -> DeviceWatch {
        let state = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(State { learned: false, enabled: true, devices: BTreeMap::new() });
        DeviceWatch { path: path.into(), state: Mutex::new(state), fresh: Mutex::new(Vec::new()) }
    }

    fn save(&self, st: &State) {
        use std::os::unix::fs::OpenOptionsExt;
        use std::io::Write;
        if let Some(d) = self.path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let tmp = self.path.with_extension("tmp");
        if let Ok(json) = serde_json::to_string(st) {
            if std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).and_then(|mut f| f.write_all(json.as_bytes())).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }

    pub fn enabled(&self) -> bool {
        self.state.lock().unwrap().enabled
    }

    pub fn set_enabled(&self, on: bool) {
        let mut st = self.state.lock().unwrap();
        st.enabled = on;
        self.save(&st);
    }

    /// Take in what is on the network now. Returns the devices that are new since the last look (none on the very first look,
    /// which is taken as what is normally here).
    pub fn observe(&self, seen: &[Neighbor], now: i64) -> Vec<Device> {
        let mut st = self.state.lock().unwrap();
        if !st.enabled {
            return Vec::new();
        }
        let first_look = !st.learned;
        st.learned = true;
        let mut new = Vec::new();
        for n in seen {
            match st.devices.get_mut(&n.mac) {
                Some(d) => {
                    d.last_seen = now;
                    d.ip = n.ip.clone();
                    d.iface = n.iface.clone();
                }
                None => {
                    if st.devices.len() >= MAX_DEVICES {
                        continue;
                    }
                    let d = Device { mac: n.mac.clone(), ip: n.ip.clone(), iface: n.iface.clone(), first_seen: now, last_seen: now, trusted: first_look, name: String::new() };
                    if !first_look {
                        new.push(d.clone());
                    }
                    st.devices.insert(n.mac.clone(), d);
                }
            }
        }
        self.save(&st);
        drop(st);
        self.fresh.lock().unwrap().extend(new.iter().cloned());
        new
    }

    /// The new devices nobody has been told about yet (each is returned once).
    pub fn take_fresh(&self) -> Vec<Device> {
        std::mem::take(&mut *self.fresh.lock().unwrap())
    }

    /// Every device, the untrusted ones first, then by when they were last seen.
    pub fn list(&self) -> Vec<Device> {
        let mut v: Vec<Device> = self.state.lock().unwrap().devices.values().cloned().collect();
        v.sort_by(|a, b| a.trusted.cmp(&b.trusted).then(b.last_seen.cmp(&a.last_seen)).then(a.mac.cmp(&b.mac)));
        v
    }

    pub fn untrusted_count(&self) -> usize {
        self.state.lock().unwrap().devices.values().filter(|d| !d.trusted).count()
    }

    fn edit(&self, mac: &str, f: impl FnOnce(&mut Device)) -> bool {
        let mut st = self.state.lock().unwrap();
        let Some(d) = st.devices.get_mut(&mac.to_lowercase()) else { return false };
        f(d);
        self.save(&st);
        true
    }

    pub fn trust(&self, mac: &str) -> bool {
        self.edit(mac, |d| d.trusted = true)
    }

    pub fn trust_all(&self) -> usize {
        let mut st = self.state.lock().unwrap();
        let mut n = 0;
        for d in st.devices.values_mut().filter(|d| !d.trusted) {
            d.trusted = true;
            n += 1;
        }
        self.save(&st);
        n
    }

    pub fn rename(&self, mac: &str, name: &str) -> bool {
        let name: String = name.trim().chars().filter(|c| !c.is_control()).take(60).collect();
        self.edit(mac, |d| d.name = name)
    }

    /// Forget a device (if it comes back it is new again).
    pub fn forget(&self, mac: &str) -> bool {
        let mut st = self.state.lock().unwrap();
        let gone = st.devices.remove(&mac.to_lowercase()).is_some();
        self.save(&st);
        gone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARP: &str = "IP address       HW type     Flags       HW address            Mask     Device\n\
        192.168.1.1      0x1         0x2         AA:BB:CC:00:00:01     *        eth0\n\
        192.168.1.20     0x1         0x2         aa:bb:cc:00:00:20     *        eth0\n\
        192.168.1.30     0x1         0x0         00:00:00:00:00:00     *        eth0\n\
        192.168.1.31     0x1         0x2         00:00:00:00:00:00     *        eth0\n\
        192.168.1.40     0x1         0x0         aa:bb:cc:00:00:40     *        eth0\n";

    fn n(ip: &str, mac: &str) -> Neighbor {
        Neighbor { ip: ip.into(), mac: mac.into(), iface: "eth0".into() }
    }

    #[test]
    fn the_neighbour_table_is_read_keeping_only_machines_that_answered() {
        let v = parse_arp(ARP);
        assert_eq!(v.iter().map(|x| (x.ip.as_str(), x.mac.as_str())).collect::<Vec<_>>(), vec![("192.168.1.1", "aa:bb:cc:00:00:01"), ("192.168.1.20", "aa:bb:cc:00:00:20")], "incomplete and empty entries are skipped, addresses are lower case");
        assert!(parse_arp("").is_empty() && parse_arp("garbage").is_empty());
    }

    #[test]
    fn the_first_look_is_what_is_normal_and_later_strangers_are_new_until_trusted() {
        let d = std::env::temp_dir().join(format!("iyakki-devwatch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let path = d.join("devices.json");
        let w = DeviceWatch::open(&path);
        assert!(w.observe(&[n("192.168.1.1", "aa:bb:cc:00:00:01"), n("192.168.1.20", "aa:bb:cc:00:00:20")], 100).is_empty(), "no alerts on the first look");
        assert_eq!(w.untrusted_count(), 0);
        // the same machines again, one with a new address: still nothing
        assert!(w.observe(&[n("192.168.1.1", "aa:bb:cc:00:00:01"), n("192.168.1.99", "aa:bb:cc:00:00:20")], 200).is_empty());
        assert_eq!(w.list().iter().find(|x| x.mac == "aa:bb:cc:00:00:20").unwrap().ip, "192.168.1.99");
        // a stranger arrives
        let new = w.observe(&[n("192.168.1.1", "aa:bb:cc:00:00:01"), n("192.168.1.50", "de:ad:be:ef:00:01")], 300);
        assert_eq!(new.len(), 1);
        assert_eq!((new[0].mac.as_str(), new[0].ip.as_str(), new[0].trusted, new[0].first_seen), ("de:ad:be:ef:00:01", "192.168.1.50", false, 300));
        assert_eq!(w.take_fresh().len(), 1);
        assert!(w.take_fresh().is_empty(), "each alert is handed out once");
        // seen again: not new, still untrusted and listed first
        assert!(w.observe(&[n("192.168.1.50", "de:ad:be:ef:00:01")], 400).is_empty());
        assert_eq!(w.list()[0].mac, "de:ad:be:ef:00:01");
        assert_eq!(w.untrusted_count(), 1);
        // trust, rename, forget
        assert!(w.trust("DE:AD:BE:EF:00:01") && w.rename("de:ad:be:ef:00:01", "  Guest phone ") && !w.trust("00:11:22:33:44:55"));
        assert_eq!((w.untrusted_count(), w.list().iter().find(|x| x.mac == "de:ad:be:ef:00:01").unwrap().name.as_str()), (0, "Guest phone"));
        assert!(w.forget("de:ad:be:ef:00:01") && !w.forget("de:ad:be:ef:00:01"));
        assert_eq!(w.observe(&[n("192.168.1.50", "de:ad:be:ef:00:01")], 500).len(), 1, "a forgotten device is new again");
        assert_eq!(w.trust_all(), 1);
        // it all survives a restart, and the baseline is not taken again
        let w2 = DeviceWatch::open(&path);
        assert_eq!(w2.list().len(), 3);
        assert_eq!(w2.observe(&[n("192.168.1.77", "11:22:33:44:55:66")], 600).len(), 1);
        // switched off: nothing is recorded or alerted
        w2.set_enabled(false);
        assert!(w2.observe(&[n("192.168.1.78", "11:22:33:44:55:77")], 700).is_empty());
        assert!(!DeviceWatch::open(&path).enabled());
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode_is_private(), true);
        let _ = std::fs::remove_dir_all(&d);
    }

    trait Private {
        fn mode_is_private(&self) -> bool;
    }
    impl Private for std::fs::Permissions {
        fn mode_is_private(&self) -> bool {
            use std::os::unix::fs::PermissionsExt;
            self.mode() & 0o077 == 0
        }
    }
}
