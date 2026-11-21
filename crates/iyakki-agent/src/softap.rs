//! The access point's frame handling for a card that leaves authentication, association, the SAE exchange and the four-way
//! handshake to the computer (`ApMode::Software`, mac80211): F-HOTSPOT-22 and F-HOTSPOT-18. It is the one piece of code behind both
//! the radio test loop (`hotspot_radio`) and the product's runner (`wifi_ap_sw`), so the two cannot drift apart.
//!
//! Frames in, calls out: the management frames the kernel hands over (authentication, association request), the EAPOL packets and the
//! kernel's "station gone" events go in with a time; every call the kernel has to make (send a frame, add or remove a station, install
//! the pairwise key, authorize, send EAPOL) goes out through [`SoftIo`], which a fake implements in the tests and [`KernelIo`] implements
//! with the nl80211 calls (`ApControl`) and an EAPOL channel. Nothing here touches an address, DHCP, DNS or the firewall: those stay
//! the sharing manager's, on the interface the access point lives on.
//!
//! Security follows the beacon: WPA2 (open-system authentication, PSK handshake), WPA2 with WPA3 (a station may do either) or WPA3 only
//! (an association without a finished SAE exchange is refused, open authentication is not answered). SAE is plain (decision D-HOTSPOT-22,
//! default (a)): a station that insists on hash-to-element is refused by the exchange.
// SPDX-License-Identifier: Apache-2.0

use crate::hotspot_sae::{auth_config, SaeRadio, SaeStep};
use crate::nl80211_os::EapolChannel;
use iyakki_hotspot::handshake::{Action, AuthConfig, Authenticator, FailReason};
use iyakki_hotspot::nl80211::{ApControl, NetlinkTransport};
use iyakki_hotspot::rsn::{rsn_ie, SecurityMode};
use iyakki_hotspot::sae_exchange::SaeKeys;
use iyakki_hotspot::{eapol, kdf, mgmt, Mac};
use std::collections::HashMap;

/// What the access point needs from the kernel.
pub trait SoftIo {
    fn send_mgmt(&mut self, frame: &[u8]) -> Result<(), String>;
    fn add_station(&mut self, sta: Mac, aid: u16, listen: u16, rates: &[u8], capability: u16) -> Result<(), String>;
    fn remove_station(&mut self, sta: Mac, reason: u16) -> Result<(), String>;
    fn install_ptk(&mut self, sta: Mac, tk: &[u8; 16]) -> Result<(), String>;
    fn authorize(&mut self, sta: Mac) -> Result<(), String>;
    fn send_eapol(&mut self, sta: Mac, eapol: &[u8]) -> Result<(), String>;
    /// Fresh random bytes; false when the system source failed.
    fn random(&mut self, buf: &mut [u8]) -> bool;
}

#[derive(Debug, Clone)]
pub struct SoftApConfig {
    pub bssid: Mac,
    pub ssid: Vec<u8>,
    pub passphrase: String,
    pub band: mgmt::Band,
    pub security: SecurityMode,
    pub max_clients: usize,
    /// The group key already installed on the interface, repeated in message 3 of every handshake (key index 1).
    pub gtk: [u8; 16],
    /// The integrity group key for protected management frames (WPA3), key index 4.
    pub igtk: [u8; 16],
}

/// What happened, for the log and for whoever owns addresses and DHCP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoftEvent {
    Log(String),
    Associated(Mac),
    KeysInstalled(Mac),
    /// The port is open: the station may now be given an address.
    Authorized(Mac),
    Failed(Mac, String),
    /// The station is gone (the kernel said so, or the handshake failed).
    Left(Mac),
}

struct Sta {
    auth: Authenticator,
    aid: u16,
}

pub struct SoftAp {
    cfg: SoftApConfig,
    psk: [u8; 32],
    rsn: Vec<u8>,
    sae: SaeRadio,
    sae_keys: HashMap<Mac, SaeKeys>,
    stations: HashMap<Mac, Sta>,
}

/// What to answer the association request of a station that finished the SAE exchange: CCMP for both ciphers, key management SAE (8),
/// protected management frames announced by the station (WPA3 requires them).
pub fn sae_assoc_status(r: &mgmt::AssocRequest, ssid: &[u8], stations_now: usize, max_clients: usize) -> u16 {
    if r.ssid != ssid {
        return mgmt::STATUS_UNSPECIFIED;
    }
    let Some(info) = r.rsn_ie.as_deref().and_then(iyakki_hotspot::rsn::parse_rsn) else { return mgmt::STATUS_CIPHER_REJECTED };
    if info.group != 4 || info.pairwise != [4] || !info.pmf_capable() {
        return mgmt::STATUS_CIPHER_REJECTED;
    }
    if info.akms != [8] {
        return STATUS_INVALID_AKMP;
    }
    if stations_now >= max_clients {
        return mgmt::STATUS_AP_FULL;
    }
    mgmt::STATUS_OK
}

/// "Invalid AKMP": the key management the station named is not the one it authenticated with.
pub const STATUS_INVALID_AKMP: u16 = 43;

pub fn mac_text(m: &Mac) -> String {
    m.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
}

impl SoftAp {
    /// `token_secret` keys the SAE anti-clogging tokens (fresh random bytes).
    pub fn new(cfg: SoftApConfig, token_secret: [u8; 32]) -> SoftAp {
        let psk = kdf::pmk_from_passphrase(&cfg.passphrase, &cfg.ssid);
        let rsn = if cfg.security == SecurityMode::Wpa2 { eapol::rsn_ie_ccmp_psk() } else { rsn_ie(cfg.security) };
        let sae = SaeRadio::new(cfg.bssid, cfg.passphrase.as_bytes(), token_secret);
        SoftAp { cfg, psk, rsn, sae, sae_keys: HashMap::new(), stations: HashMap::new() }
    }

    /// The element the beacon, the probe answer and message 3 carry.
    pub fn rsn_element(&self) -> &[u8] {
        &self.rsn
    }

    pub fn station_count(&self) -> usize {
        self.stations.len()
    }

    pub fn sae_in_progress(&self) -> usize {
        self.sae.in_progress()
    }

    /// A management frame the kernel handed over (authentication or association request).
    pub fn on_mgmt(&mut self, io: &mut dyn SoftIo, frame: &[u8], now_ms: u64) -> Vec<SoftEvent> {
        let mut ev = Vec::new();
        let Some(h) = mgmt::parse_header(frame) else { return ev };
        let wpa3 = self.cfg.security != SecurityMode::Wpa2;
        if h.fc0 == mgmt::SUBTYPE_AUTH && wpa3 && SaeRadio::is_sae(frame) {
            let mut rng = |b: &mut [u8]| io.random(b);
            let steps = self.sae.on_auth_frame(frame, now_ms, &mut rng);
            for s in steps {
                match s {
                    SaeStep::Frame(b) => {
                        if let Err(e) = io.send_mgmt(&b) {
                            ev.push(SoftEvent::Log(format!("sending the authentication frame failed: {e}")));
                        }
                    }
                    SaeStep::Keys { sta, keys } => {
                        ev.push(SoftEvent::Log(format!("SAE finished for {}", mac_text(&sta))));
                        self.sae_keys.insert(sta, keys);
                    }
                    SaeStep::Failed { sta, error } => ev.push(SoftEvent::Log(format!("SAE failed for {}: {error:?}", mac_text(&sta)))),
                }
            }
        } else if h.fc0 == mgmt::SUBTYPE_AUTH && self.cfg.security == SecurityMode::Wpa3 {
            ev.push(SoftEvent::Log(format!("open authentication from {} ignored (WPA3 only)", mac_text(&h.sa))));
        } else if h.fc0 == mgmt::SUBTYPE_AUTH {
            if let Some(resp) = mgmt::auth_response(frame, self.cfg.bssid) {
                if let Err(e) = io.send_mgmt(&resp) {
                    ev.push(SoftEvent::Log(format!("sending the authentication answer failed: {e}")));
                }
            }
        } else if h.fc0 == mgmt::SUBTYPE_ASSOC_REQ {
            self.on_assoc(io, frame, now_ms, &mut ev);
        }
        ev
    }

    fn on_assoc(&mut self, io: &mut dyn SoftIo, frame: &[u8], now_ms: u64, ev: &mut Vec<SoftEvent>) {
        let bssid = self.cfg.bssid;
        let Some(r) = mgmt::parse_assoc_request(frame, bssid) else { return };
        let status = match self.sae_keys.contains_key(&r.sta) {
            // a station that finished the SAE exchange names the SAE key management, not the pre-shared key one
            true => sae_assoc_status(&r, &self.cfg.ssid, self.stations.len(), self.cfg.max_clients),
            false => mgmt::assoc_status(&r, &self.cfg.ssid, self.stations.len(), self.cfg.max_clients),
        };
        ev.push(SoftEvent::Log(format!("association request from {} status {status}", mac_text(&r.sta))));
        let refuse = |io: &mut dyn SoftIo, status: u16| {
            let _ = io.send_mgmt(&mgmt::assoc_response(r.sta, bssid, self.cfg.band, status, 0));
        };
        if status != mgmt::STATUS_OK {
            refuse(io, status);
            return;
        }
        let sae_station = self.sae_keys.remove(&r.sta);
        if self.cfg.security == SecurityMode::Wpa3 && sae_station.is_none() {
            // no finished SAE exchange for this station: refused
            refuse(io, mgmt::STATUS_UNSPECIFIED);
            return;
        }
        if self.stations.remove(&r.sta).is_some() {
            let _ = io.remove_station(r.sta, mgmt::REASON_UNSPECIFIED);
        }
        let used: Vec<u16> = self.stations.values().map(|s| s.aid).collect();
        let Some(aid) = mgmt::free_aid(&used) else { return };
        if let Err(e) = io.add_station(r.sta, aid, r.listen_interval, &r.rates, r.capability) {
            ev.push(SoftEvent::Log(format!("adding the station failed: {e}")));
            return;
        }
        if let Err(e) = io.send_mgmt(&mgmt::assoc_response(r.sta, bssid, self.cfg.band, mgmt::STATUS_OK, aid)) {
            ev.push(SoftEvent::Log(format!("sending the association answer failed: {e}")));
        }
        let mut anonce = [0u8; 32];
        if !io.random(&mut anonce) {
            let _ = io.remove_station(r.sta, mgmt::REASON_UNSPECIFIED);
            ev.push(SoftEvent::Log("no random bytes for the handshake; the station is dropped".into()));
            return;
        }
        let c = &self.cfg;
        let cfg4 = match &sae_station {
            Some(k) => auth_config(k, bssid, r.sta, anonce, c.gtk, (crate::wifi_ap_sw::IGTK_IDX as u16, [0; 6], c.igtk), c.security),
            None => AuthConfig { pmk: self.psk, aa: bssid, spa: r.sta, anonce, gtk: c.gtk, gtk_id: 1, rsn_ie: self.rsn.clone(), first_replay: 1, akm: eapol::Akm::Psk, igtk: None, pmkid: None },
        };
        let mut auth = Authenticator::new(cfg4);
        let first = auth.start(now_ms);
        self.stations.insert(r.sta, Sta { auth, aid });
        ev.push(SoftEvent::Associated(r.sta));
        ev.push(SoftEvent::Log(format!("associated {} aid {aid}, handshake started", mac_text(&r.sta))));
        self.apply(io, r.sta, first, ev);
    }

    /// An EAPOL packet from a station.
    pub fn on_eapol(&mut self, io: &mut dyn SoftIo, sta: Mac, bytes: &[u8], now_ms: u64) -> Vec<SoftEvent> {
        let mut ev = Vec::new();
        if let Some(s) = self.stations.get_mut(&sta) {
            let a = s.auth.on_frame(bytes, now_ms);
            self.apply(io, sta, a, &mut ev);
        }
        ev
    }

    /// The kernel says the station is gone: everything kept for it is forgotten.
    pub fn on_station_removed(&mut self, sta: Mac) -> Vec<SoftEvent> {
        self.sae.forget(&sta);
        self.sae_keys.remove(&sta);
        if self.stations.remove(&sta).is_some() {
            vec![SoftEvent::Left(sta)]
        } else {
            Vec::new()
        }
    }

    /// Handshake resends and time-outs.
    pub fn tick(&mut self, io: &mut dyn SoftIo, now_ms: u64) -> Vec<SoftEvent> {
        let mut ev = Vec::new();
        self.sae.tick(now_ms);
        let due: Vec<(Mac, Vec<Action>)> = self.stations.iter_mut().map(|(m, s)| (*m, s.auth.tick(now_ms))).filter(|(_, a)| !a.is_empty()).collect();
        for (m, a) in due {
            self.apply(io, m, a, &mut ev);
        }
        ev
    }

    fn apply(&mut self, io: &mut dyn SoftIo, sta: Mac, list: Vec<Action>, ev: &mut Vec<SoftEvent>) {
        for a in list {
            match a {
                Action::Send(b) => {
                    if let Err(e) = io.send_eapol(sta, &b) {
                        ev.push(SoftEvent::Log(format!("EAPOL send to {} failed: {e}", mac_text(&sta))));
                    }
                }
                Action::InstallPtk { peer, tk } => match io.install_ptk(peer, &tk) {
                    Ok(()) => {
                        ev.push(SoftEvent::KeysInstalled(peer));
                        ev.push(SoftEvent::Log(format!("pairwise key installed for {}", mac_text(&peer))));
                    }
                    Err(e) => ev.push(SoftEvent::Log(format!("installing the pairwise key failed: {e}"))),
                },
                Action::Authorized => match io.authorize(sta) {
                    Ok(()) => {
                        ev.push(SoftEvent::Authorized(sta));
                        ev.push(SoftEvent::Log(format!("authorized {}", mac_text(&sta))));
                    }
                    Err(e) => ev.push(SoftEvent::Log(format!("authorizing failed: {e}"))),
                },
                Action::Failed(why) => {
                    let text = match why {
                        FailReason::WrongPassphrase => "wrong passphrase",
                        FailReason::Timeout => "timeout",
                        FailReason::BadRsnIe => "bad RSN element",
                    };
                    ev.push(SoftEvent::Log(format!("handshake failed for {}: {text}", mac_text(&sta))));
                    ev.push(SoftEvent::Failed(sta, text.to_string()));
                    if self.stations.remove(&sta).is_some() {
                        ev.push(SoftEvent::Left(sta));
                    }
                    let _ = io.remove_station(sta, mgmt::REASON_4WAY_TIMEOUT);
                }
            }
        }
    }
}

/// The real [`SoftIo`]: nl80211 calls through any transport (a fake one in the tests) and an EAPOL channel.
pub struct KernelIo<'a, T: NetlinkTransport, E: EapolChannel> {
    pub ap: &'a mut ApControl<T>,
    pub eap: &'a mut E,
    pub ifindex: u32,
    pub freq_mhz: u32,
}

fn random_os(buf: &mut [u8]) -> bool {
    iyakki_sec::random_fill(buf).is_ok()
}

impl<T: NetlinkTransport, E: EapolChannel> SoftIo for KernelIo<'_, T, E> {
    fn send_mgmt(&mut self, frame: &[u8]) -> Result<(), String> {
        self.ap.send_mgmt(self.ifindex, self.freq_mhz, frame).map_err(|e| e.to_string())
    }
    fn add_station(&mut self, sta: Mac, aid: u16, listen: u16, rates: &[u8], capability: u16) -> Result<(), String> {
        self.ap.add_station(self.ifindex, sta, aid, listen, rates, capability).map_err(|e| format!("NEW_STATION: {e}"))
    }
    fn remove_station(&mut self, sta: Mac, reason: u16) -> Result<(), String> {
        self.ap.remove_station(self.ifindex, sta, reason).map_err(|e| format!("DEL_STATION: {e}"))
    }
    fn install_ptk(&mut self, sta: Mac, tk: &[u8; 16]) -> Result<(), String> {
        self.ap.install_ptk(self.ifindex, sta, tk).map_err(|e| format!("NEW_KEY: {e}"))
    }
    fn authorize(&mut self, sta: Mac) -> Result<(), String> {
        self.ap.authorize_station(self.ifindex, sta).map_err(|e| format!("SET_STATION: {e}"))
    }
    fn send_eapol(&mut self, sta: Mac, eapol: &[u8]) -> Result<(), String> {
        self.eap.send(sta, eapol)
    }
    fn random(&mut self, buf: &mut [u8]) -> bool {
        random_os(buf)
    }
}

#[cfg(test)]
#[path = "softap_tests.rs"]
pub(crate) mod tests;
