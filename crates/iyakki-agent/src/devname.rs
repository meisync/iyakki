//! The stable device name (`iyakki-<random id>.local`): what the device announces and when. The protocol is in
//! `iyakki_netscan::mdns`; the thread that runs it and the firewall rule are in `iyakki-remote::devname`.
//!
//! * The id is three random bytes, made once and kept in `<data>/device-id`. It is never made from a person's name, an account or the
//!   host name (the host name of Settings stays a separate setting). Two machines made from one copied disk image would share the id;
//!   the name check at start (probing) gives the second a suffix.
//! * Announcing is on by default and off by default in cloud mode ([`effective`]); the person's choice in Settings, Network wins.
//! * What the announcement reveals: the random name, that the machine is an IYAKKI device, that developer mode is on (the
//!   `_iyakki-dev._tcp` service exists only while it is) and that a web service runs (`_iyakki-web._tcp`). Not who the person is.

use std::path::Path;

use iyakki_netscan::mdns::responder::Svc;
pub use iyakki_netscan::mdns;
use iyakki_netscan::mdns::{DEV_SERVICE, WEB_SERVICE};
use serde::{Deserialize, Serialize};

pub const ID_FILE: &str = "device-id";
pub const STATUS_FILE: &str = "devname.json";

/// Announce or not: the person's choice, else on (off in cloud mode).
pub fn effective(setting: Option<bool>, cloud: bool) -> bool {
    setting.unwrap_or(!cloud)
}

fn random3() -> [u8; 3] {
    use std::io::Read;
    let mut b = [0u8; 3];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_err() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(7, |d| d.as_nanos() as u64) ^ ((std::process::id() as u64) << 20);
        b = [(n >> 3) as u8, (n >> 11) as u8, (n >> 19) as u8];
    }
    b
}

/// The device's label (`iyakki-0a1b2c`), made the first time and kept.
pub fn label(dir: &Path) -> String {
    let p = dir.join(ID_FILE);
    if let Some(t) = std::fs::read_to_string(&p).ok().map(|t| t.trim().to_string()).filter(|t| mdns::is_device_label(t)) {
        return t;
    }
    let l = mdns::label_from(random3());
    let tmp = dir.join(format!("{ID_FILE}.tmp"));
    if std::fs::write(&tmp, format!("{l}\n")).and_then(|_| std::fs::rename(&tmp, &p)).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    l
}

/// The services to announce: the developer port while developer mode is on, the web service when it runs.
pub fn services(dev_on: bool, dev_port: u16, web_port: Option<u16>) -> Vec<Svc> {
    let mut v = Vec::new();
    if dev_on {
        v.push(Svc { kind: DEV_SERVICE.into(), port: dev_port, txt: vec![] });
    }
    if let Some(p) = web_port {
        v.push(Svc { kind: WEB_SERVICE.into(), port: p, txt: vec![] });
    }
    v
}

/// What the announcer last said about itself (for Settings and the developer-mode diagnosis).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub on: bool,
    /// The name in use now (it has a suffix after a conflict); empty when off.
    pub host: String,
    pub probing: bool,
    pub addresses: Vec<String>,
    pub services: Vec<String>,
    pub note: String,
}

impl Status {
    /// One sentence for the screens.
    pub fn line(&self) -> String {
        if !self.on {
            "not announced (the switch is off)".into()
        } else if self.host.is_empty() {
            "starting".into()
        } else if self.probing {
            format!("{}: checking that the name is free", self.host)
        } else if !self.note.is_empty() {
            format!("{}: {}", self.host, self.note)
        } else {
            format!("{} at {}", self.host, self.addresses.join(", "))
        }
    }
}

pub fn write_status(dir: &Path, s: &Status) {
    let tmp = dir.join(format!("{STATUS_FILE}.tmp"));
    if let Ok(t) = serde_json::to_string(s) {
        if std::fs::write(&tmp, t).and_then(|_| std::fs::rename(&tmp, dir.join(STATUS_FILE))).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

pub fn read_status(dir: &Path) -> Status {
    std::fs::read_to_string(dir.join(STATUS_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-devname-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_default_is_on_on_a_desktop_and_off_in_cloud_mode_and_the_choice_wins() {
        assert!(effective(None, false) && !effective(None, true));
        assert!(!effective(Some(false), false) && effective(Some(true), true));
    }

    #[test]
    fn the_id_is_random_made_once_and_has_nothing_of_a_person_or_the_host_name() {
        let d = tmp("id");
        let a = label(&d);
        assert!(mdns::is_device_label(&a), "{a}");
        assert_eq!(label(&d), a, "kept");
        let host = std::fs::read_to_string("/etc/hostname").unwrap_or_default().trim().to_lowercase();
        let user = std::env::var("USER").unwrap_or_default().to_lowercase();
        for secret in [host, user].iter().filter(|s| s.len() > 2) {
            assert!(!a.contains(secret.as_str()), "{a} contains {secret}");
        }
        // a damaged or hand-edited file is replaced by a fresh random label
        std::fs::write(d.join(ID_FILE), "alice-laptop\n").unwrap();
        let b = label(&d);
        assert!(mdns::is_device_label(&b) && !b.contains("alice"));
        // two machines get different labels (24 random bits: a clash here would be a one in 16 million accident)
        let labels: std::collections::HashSet<String> = (0..20).map(|i| label(&tmp(&format!("n{i}")))).collect();
        assert!(labels.len() >= 19);
    }

    #[test]
    fn the_developer_service_exists_only_while_developer_mode_is_on() {
        assert!(services(false, 7979, None).is_empty());
        let s = services(true, 7979, Some(8443));
        assert_eq!((s[0].kind.as_str(), s[0].port, s[1].kind.as_str(), s[1].port), ("_iyakki-dev._tcp", 7979, "_iyakki-web._tcp", 8443));
    }

    #[test]
    fn the_status_is_kept_and_read_back_and_has_one_plain_sentence() {
        let d = tmp("st");
        assert!(!read_status(&d).on);
        let s = Status { on: true, host: "iyakki-0a1b2c.local".into(), probing: false, addresses: vec!["192.168.50.66".into()], services: vec![], note: String::new() };
        write_status(&d, &s);
        assert_eq!(read_status(&d), s);
        assert_eq!(s.line(), "iyakki-0a1b2c.local at 192.168.50.66");
        assert!(Status::default().line().contains("off"));
    }
}
