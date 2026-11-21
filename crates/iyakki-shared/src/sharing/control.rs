//! How the shell and the command line talk to the sharing service: two small files in `<data>/share/`. The service (`real::start_system`)
//! reads `request.json` and writes `status.json`. Nobody but root can write the folder, which is the permission check for the command
//! line; the Settings page checks the administrator role before it writes. Neither file holds a secret.

use super::{IfaceInfo, Request, Status};
use std::path::Path;
use std::sync::Mutex;

pub use crate::sharing::dir;

fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn read_request(data: &Path) -> Request {
    std::fs::read_to_string(dir(data).join("request.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn write_request(data: &Path, r: &Request) -> Result<(), String> {
    write_atomic(&dir(data).join("request.json"), &serde_json::to_string(r).map_err(|e| e.to_string())?)
}

pub fn read_status(data: &Path) -> Status {
    std::fs::read_to_string(dir(data).join("status.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn write_status(data: &Path, s: &Status) -> Result<(), String> {
    write_atomic(&dir(data).join("status.json"), &serde_json::to_string(s).map_err(|e| e.to_string())?)
}

/// The request for "start", built from the one before it (the id goes up so the service sees a change).
pub fn start_request(prev: &Request, uplink: Option<String>, via: Option<String>, idle_hours: u8, isolate: bool, allow_lan: bool, by: &str, by_admin: bool) -> Request {
    // the hotspot is not a device that exists yet: choosing it asks the service to make it
    let wifi = via.as_deref() == Some(crate::wifi_ap::AP_IFACE);
    // the Bluetooth access point is made by the service in the same way
    let nap = via.as_deref() == Some(iyakki_devctl::BT_NAP_IFACE);
    let via = if wifi || nap { None } else { via };
    Request { id: prev.id + 1, on: true, uplink, via, idle_hours, isolate, allow_lan, by: by.into(), by_admin, wifi, nap }
}

/// The request for "let phones connect over Bluetooth": the access point is made by the service, so there is no `via`.
pub fn start_nap_request(prev: &Request, uplink: Option<String>, idle_hours: u8, isolate: bool, allow_lan: bool, by: &str, by_admin: bool) -> Request {
    start_request(prev, uplink, Some(iyakki_devctl::BT_NAP_IFACE.into()), idle_hours, isolate, allow_lan, by, by_admin)
}

/// The request for "start a Wi-Fi hotspot": the access point is made by the service, so there is no `via`.
pub fn start_wifi_request(prev: &Request, uplink: Option<String>, idle_hours: u8, isolate: bool, allow_lan: bool, by: &str, by_admin: bool) -> Request {
    start_request(prev, uplink, Some(crate::wifi_ap::AP_IFACE.into()), idle_hours, isolate, allow_lan, by, by_admin)
}

pub fn stop_request(prev: &Request, by: &str) -> Request {
    Request { id: prev.id + 1, on: false, by: by.into(), ..prev.clone() }
}

/// What a page or command needs from the machine; the real one reads the files and `/sys`, tests use [`MemBackend`].
pub trait Backend: Send + Sync {
    fn interfaces(&self) -> Vec<IfaceInfo>;
    fn wifi_ap(&self) -> bool;
    /// Why the Wi-Fi hotspot is not offered, in words (`None`: it is, or this backend cannot say).
    fn wifi_ap_why(&self) -> Option<String> {
        None
    }
    /// Why letting phones connect over Bluetooth is not offered, in words (`None`: it is). The default says it is not built into this backend.
    fn nap_why(&self) -> Option<String> {
        Some(super::view::NAP_NOT_BUILT.into())
    }
    fn status(&self) -> Status;
    fn request(&self) -> Request;
    fn submit(&self, r: &Request) -> Result<(), String>;
    fn policy(&self) -> super::policy::Policy;
    /// The hotspot's network name and password (made on first use). `None` where they cannot be read (not root, or a made-up machine).
    fn hotspot(&self) -> Option<(String, String)> {
        None
    }
    /// Change the hotspot's name and password (checked first).
    fn set_hotspot(&self, _ssid: &str, _passphrase: &str) -> Result<(), String> {
        Err("not available here".into())
    }
    /// The hotspot's band: "auto", "2.4" or "5".
    fn hotspot_band(&self) -> String {
        "auto".into()
    }
    fn set_hotspot_band(&self, _band: &str) -> Result<(), String> {
        Err("not available here".into())
    }
    /// The regulatory country the hotspot asks for (two capital letters), empty when none is chosen.
    fn hotspot_country(&self) -> String {
        String::new()
    }
    fn set_hotspot_country(&self, _country: &str) -> Result<(), String> {
        Err("not available here".into())
    }
    /// The hotspot's security: "wpa2", "transition" or "wpa3".
    fn hotspot_security(&self) -> String {
        "wpa2".into()
    }
    fn set_hotspot_security(&self, _security: &str) -> Result<(), String> {
        Err("not available here".into())
    }
    fn set_policy(&self, p: &super::policy::Policy, by: &str, is_admin: bool) -> Result<String, String>;
}

/// An in-memory machine for the page tests: `submit` only stores the request; `set_status` plays the service.
pub struct MemBackend {
    inner: Mutex<(Vec<IfaceInfo>, bool, Status, Request, super::policy::Policy)>,
    /// The hotspot's name and password (checked the same way as the real ones).
    hot: Mutex<(String, String)>,
    /// What the radios said when the hotspot is not offered.
    why: Mutex<Option<String>>,
    /// What Bluetooth said when letting phones connect is not offered (`None`: it is).
    nap: Mutex<Option<String>>,
}

impl MemBackend {
    pub fn new(ifs: Vec<IfaceInfo>, wifi_ap: bool) -> MemBackend {
        MemBackend { inner: Mutex::new((ifs, wifi_ap, Status::default(), Request::default(), super::policy::Policy::default())), hot: Mutex::new(("IYAKKI-TEST".into(), "first password".into())), why: Mutex::new(None), nap: Mutex::new(Some(super::view::NAP_NOT_BUILT.into())) }
    }
    pub fn set_wifi_why(&self, why: Option<&str>) {
        *self.why.lock().unwrap_or_else(|e| e.into_inner()) = why.map(str::to_string);
    }
    pub fn set_nap_why(&self, why: Option<&str>) {
        *self.nap.lock().unwrap_or_else(|e| e.into_inner()) = why.map(str::to_string);
    }
    pub fn set_status(&self, s: Status) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).2 = s;
    }
    pub fn set_policy_raw(&self, p: super::policy::Policy) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).4 = p;
    }
    pub fn set_interfaces(&self, ifs: Vec<IfaceInfo>) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).0 = ifs;
    }
}

impl Backend for MemBackend {
    fn nap_why(&self) -> Option<String> {
        self.nap.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn wifi_ap_why(&self) -> Option<String> {
        self.why.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn interfaces(&self) -> Vec<IfaceInfo> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).0.clone()
    }
    fn wifi_ap(&self) -> bool {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).1
    }
    fn status(&self) -> Status {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).2.clone()
    }
    fn request(&self) -> Request {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).3.clone()
    }
    fn submit(&self, r: &Request) -> Result<(), String> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).3 = r.clone();
        Ok(())
    }
    fn policy(&self) -> super::policy::Policy {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).4.clone()
    }
    fn hotspot(&self) -> Option<(String, String)> {
        Some(self.hot.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }
    fn set_hotspot(&self, ssid: &str, passphrase: &str) -> Result<(), String> {
        iyakki_hotspot::settings::validate_ssid(ssid).map_err(|e| format!("the network name: {e:?}"))?;
        iyakki_hotspot::settings::validate_passphrase(passphrase).map_err(|e| format!("the password: {e:?}"))?;
        *self.hot.lock().unwrap_or_else(|e| e.into_inner()) = (ssid.into(), passphrase.into());
        Ok(())
    }
    fn set_policy(&self, p: &super::policy::Policy, by: &str, is_admin: bool) -> Result<String, String> {
        if !is_admin {
            return Err("Only administrators can change the sharing policy.".into());
        }
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let line = format!("share policy changed by {by}: before [{}] after [{}]", g.4.summary(), p.summary());
        g.4 = super::policy::Policy { changed_by: by.into(), changed_at: 1, ..p.clone() };
        Ok(line)
    }
}
