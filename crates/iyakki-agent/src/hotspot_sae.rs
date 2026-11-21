//! The computer's side of WPA3 for a card that leaves the SAE exchange to it (F-HOTSPOT-22, F-HOTSPOT-18): the authentication frames the
//! kernel hands over (registered management frames of type authentication) go to one [`SaeAp`] per station, the answers go back out with
//! `NL80211_CMD_FRAME`, and a station that proved the password gives its keys to the four-way handshake (`Akm::Sae`, with the integrity group key).
//!
//! Plain SAE only: the hash-to-element variant is not offered (decision D-HOTSPOT-22, default (a)), so the beacon announces SAE without it and
//! a station that insists on it is refused by the exchange itself. The state machine is pure (frames and a time in, steps out); [`deliver`]
//! sends the steps through any [`NetlinkTransport`], so the tests run it against a fake one.
// SPDX-License-Identifier: Apache-2.0

use iyakki_hotspot::eapol::Akm;
use iyakki_hotspot::handshake::AuthConfig;
use iyakki_hotspot::nl80211::{ApControl, NetlinkTransport, NlError};
use iyakki_hotspot::rsn::{rsn_ie, SecurityMode};
use iyakki_hotspot::sae::{encode_sae, parse_sae, SaeBody, ALGO_SAE};
use iyakki_hotspot::sae_ap::{SaeAction, SaeAp};
use iyakki_hotspot::sae_exchange::{SaeError, SaeKeys};
use iyakki_hotspot::Mac;
use std::collections::HashMap;

/// Exchanges that may run at once; a commit from another station is dropped until one ends (the radio cannot be made to do endless curve work).
pub const MAX_EXCHANGES: usize = 16;
/// From this many exchanges in progress a commit without a valid anti-clogging token is answered with a token request.
pub const BUSY_AT: usize = 4;
/// An exchange that has not finished after this long is forgotten.
pub const EXCHANGE_TIMEOUT_MS: u64 = 10_000;
/// The status of a refused authentication ("unspecified failure").
pub const STATUS_FAILURE: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaeStep {
    /// An authentication frame to send to the station.
    Frame(Vec<u8>),
    /// The station proved the password: start the four-way handshake with these keys.
    Keys { sta: Mac, keys: SaeKeys },
    /// The exchange failed; the station was told and its state is dropped.
    Failed { sta: Mac, error: SaeError },
}

pub struct SaeRadio {
    bssid: Mac,
    password: Vec<u8>,
    secret: [u8; 32],
    exchanges: HashMap<Mac, (SaeAp, u64)>,
}

impl SaeRadio {
    /// `secret` keys the anti-clogging tokens: fresh random bytes (see [`SaeRadio::rotate_secret`]).
    pub fn new(bssid: Mac, password: &[u8], secret: [u8; 32]) -> SaeRadio {
        SaeRadio { bssid, password: password.to_vec(), secret, exchanges: HashMap::new() }
    }

    /// New token secret; tokens made with the old one stop working, a station asks again.
    pub fn rotate_secret(&mut self, secret: [u8; 32]) {
        self.secret = secret;
    }

    pub fn in_progress(&self) -> usize {
        self.exchanges.len()
    }

    pub fn busy(&self) -> bool {
        self.exchanges.len() >= BUSY_AT
    }

    /// The station is gone (deauthenticated, removed): forget it.
    pub fn forget(&mut self, sta: &Mac) {
        self.exchanges.remove(sta);
    }

    /// Forgets exchanges that ran too long.
    pub fn tick(&mut self, now_ms: u64) {
        self.exchanges.retain(|_, (_, started)| now_ms.saturating_sub(*started) < EXCHANGE_TIMEOUT_MS);
    }

    /// Is this a frame for the SAE exchange (authentication algorithm 3)? Frames of other algorithms are not ours.
    pub fn is_sae(frame: &[u8]) -> bool {
        frame.len() >= 26 && u16::from_le_bytes([frame[24], frame[25]]) == ALGO_SAE
    }

    /// Handles one authentication frame from the air. Anything that does not parse, is not for this access point or is not SAE gives no
    /// steps; nothing a station sends can panic or grow the state past [`MAX_EXCHANGES`].
    pub fn on_auth_frame(&mut self, frame: &[u8], now_ms: u64, rng: &mut dyn FnMut(&mut [u8]) -> bool) -> Vec<SaeStep> {
        self.tick(now_ms);
        if !Self::is_sae(frame) {
            return Vec::new();
        }
        let Some(f) = parse_sae(frame, self.bssid) else { return Vec::new() };
        let busy = self.busy();
        let sta = f.from;
        if !self.exchanges.contains_key(&sta) {
            if self.exchanges.len() >= MAX_EXCHANGES || !matches!(f.body, SaeBody::Commit { .. }) {
                return Vec::new();
            }
            self.exchanges.insert(sta, (SaeAp::new(&self.password, self.bssid, sta, self.secret), now_ms));
        }
        let Some((ap, _)) = self.exchanges.get_mut(&sta) else { return Vec::new() };
        let mut steps = Vec::new();
        let mut end = false;
        for a in ap.on_frame(&f, busy, rng) {
            match a {
                SaeAction::Send(b) => steps.push(SaeStep::Frame(b)),
                SaeAction::Keys(keys) => {
                    steps.push(SaeStep::Keys { sta, keys });
                    end = true;
                }
                SaeAction::Failed(error) => {
                    // a refusal frame, so the station stops waiting
                    steps.push(SaeStep::Frame(encode_sae(sta, self.bssid, STATUS_FAILURE, &SaeBody::Rejected { status: STATUS_FAILURE })));
                    steps.push(SaeStep::Failed { sta, error });
                    end = true;
                }
            }
        }
        // a token request leaves no state behind: the exchange starts when the commit comes back with the token
        if !ap.started() {
            end = true;
        }
        if end {
            self.exchanges.remove(&sta);
        }
        steps
    }
}

/// The four-way handshake of a station that did SAE: WPA3 key management, the PMKID of the exchange, the integrity group key for
/// protected management frames. `mode` is what the beacon announces (the same element goes into message 3).
pub fn auth_config(keys: &SaeKeys, aa: Mac, spa: Mac, anonce: [u8; 32], gtk: [u8; 16], igtk: (u16, [u8; 6], [u8; 16]), mode: SecurityMode) -> AuthConfig {
    AuthConfig { pmk: keys.pmk, aa, spa, anonce, gtk, gtk_id: 1, rsn_ie: rsn_ie(mode), first_replay: 1, akm: Akm::Sae, igtk: Some(igtk), pmkid: Some(keys.pmkid) }
}

/// Sends the frames of the steps on the access point's channel through the kernel and returns what the four-way handshake has to start
/// with. A frame that could not be sent is reported in `failed` (the exchange then times out on the station's side).
pub struct Delivered {
    pub keys: Vec<(Mac, SaeKeys)>,
    pub failed: Vec<String>,
}

pub fn deliver<T: NetlinkTransport>(ap: &mut ApControl<T>, ifindex: u32, freq_mhz: u32, steps: Vec<SaeStep>) -> Delivered {
    let mut out = Delivered { keys: Vec::new(), failed: Vec::new() };
    for s in steps {
        match s {
            SaeStep::Frame(b) => {
                if let Err(e) = ap.send_mgmt(ifindex, freq_mhz, &b) {
                    out.failed.push(describe(&e));
                }
            }
            SaeStep::Keys { sta, keys } => out.keys.push((sta, keys)),
            SaeStep::Failed { .. } => {}
        }
    }
    out
}

fn describe(e: &NlError) -> String {
    format!("sending the authentication frame failed: {e}")
}

#[cfg(test)]
#[path = "hotspot_sae_tests.rs"]
mod tests;
