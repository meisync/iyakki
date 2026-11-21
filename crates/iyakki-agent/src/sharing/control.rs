//! The agent's part of `iyakki_shared::sharing::control`: the backend that reads and writes the real files and `/sys`.

use std::path::PathBuf;

use super::real;
use super::{IfaceInfo, Request, Status};
pub use iyakki_shared::sharing::control::*;
use iyakki_shared::sharing::control::{read_request as rr, read_status as rs, write_request as wr};


pub struct FileBackend {
    pub data: PathBuf,
}

impl Backend for FileBackend {
    fn interfaces(&self) -> Vec<IfaceInfo> {
        real::list_interfaces()
    }
    fn wifi_ap(&self) -> bool {
        real::wifi_ap_available()
    }
    fn wifi_ap_why(&self) -> Option<String> {
        real::wifi_ap_why()
    }
    fn nap_why(&self) -> Option<String> {
        real::nap_available().err()
    }
    fn status(&self) -> Status {
        rs(&self.data)
    }
    fn request(&self) -> Request {
        rr(&self.data)
    }
    fn submit(&self, r: &Request) -> Result<(), String> {
        wr(&self.data, r)
    }
    fn policy(&self) -> iyakki_shared::sharing::policy::Policy {
        iyakki_shared::sharing::policy::read(&self.data)
    }
    fn hotspot(&self) -> Option<(String, String)> {
        real::hotspot_credentials(&self.data).ok()
    }
    fn set_hotspot(&self, ssid: &str, passphrase: &str) -> Result<(), String> {
        real::set_hotspot_credentials(&self.data, ssid, passphrase)
    }
    fn hotspot_band(&self) -> String {
        real::hotspot_band(&self.data)
    }
    fn set_hotspot_band(&self, band: &str) -> Result<(), String> {
        real::set_hotspot_band(&self.data, band)
    }
    fn hotspot_country(&self) -> String {
        real::hotspot_country(&self.data)
    }
    fn set_hotspot_country(&self, country: &str) -> Result<(), String> {
        real::set_hotspot_country(&self.data, country)
    }
    fn hotspot_security(&self) -> String {
        real::hotspot_security(&self.data)
    }
    fn set_hotspot_security(&self, security: &str) -> Result<(), String> {
        real::set_hotspot_security(&self.data, security)
    }
    fn set_policy(&self, p: &iyakki_shared::sharing::policy::Policy, by: &str, is_admin: bool) -> Result<String, String> {
        iyakki_shared::sharing::policy::write(&self.data, p, by, is_admin, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0))
    }
}

