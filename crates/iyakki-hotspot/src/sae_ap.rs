//! The access point's side of one station's SAE authentication, from frame to keys (F-HOTSPOT-22): reads the parsed authentication
//! frames of [`crate::sae`], runs [`crate::sae_exchange::Sae`], answers with commit and confirm frames, asks for an anti-clogging token
//! when the access point is busy, and hands the PMK and PMKID to the four-way handshake when the station's confirm is right. Also the
//! SA Query bookkeeping of 802.11w ([`SaQueryTimer`]). A pure state machine: no socket, no clock (the caller passes the time).
// SPDX-License-Identifier: Apache-2.0

use crate::sae::{encode_sae, encode_sa_query, SaQuery, SaeBody, SaeFrame, GROUP_P256, STATUS_SAE_UNSUPPORTED_GROUP};
use crate::sae_exchange::{anti_clogging_token, anti_clogging_token_ok, Sae, SaeError, SaeKeys};
use crate::Mac;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaeAction {
    /// A management frame to send to the station.
    Send(Vec<u8>),
    /// The station proved the password: start the four-way handshake with these keys.
    Keys(SaeKeys),
    /// The exchange failed; the caller drops the station's state (and may count it for a ban).
    Failed(SaeError),
}

pub struct SaeAp {
    password: Vec<u8>,
    bssid: Mac,
    sta: Mac,
    secret: [u8; 32],
    sae: Option<Sae>,
    done: bool,
}

impl SaeAp {
    /// `secret` keys the anti-clogging tokens (fresh random bytes, changed now and then by the caller).
    pub fn new(password: &[u8], bssid: Mac, sta: Mac, secret: [u8; 32]) -> SaeAp {
        SaeAp { password: password.to_vec(), bssid, sta, secret, sae: None, done: false }
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Handles one parsed frame from the station. `busy` is true when the access point is under load (many exchanges in progress):
    /// a commit without a valid token is then answered with a token request instead of the expensive work.
    pub fn on_frame(&mut self, f: &SaeFrame, busy: bool, rng: &mut dyn FnMut(&mut [u8]) -> bool) -> Vec<SaeAction> {
        if f.from != self.sta || f.bssid != self.bssid || self.done {
            return vec![];
        }
        match &f.body {
            SaeBody::Commit { group, token, scalar, element } => {
                if *group != GROUP_P256 {
                    return vec![SaeAction::Send(encode_sae(self.sta, self.bssid, STATUS_SAE_UNSUPPORTED_GROUP, &SaeBody::Rejected { status: STATUS_SAE_UNSUPPORTED_GROUP }))];
                }
                if self.sae.is_some() {
                    // a repeated commit: the station did not get our answer; send the confirm again with a higher counter
                    return self.sae.as_mut().and_then(|s| s.next_confirm().ok()).map(|b| vec![SaeAction::Send(encode_sae(self.sta, self.bssid, 0, &b))]).unwrap_or_default();
                }
                if busy && !anti_clogging_token_ok(&self.secret, &self.sta, token) {
                    let body = SaeBody::TokenRequest { group: GROUP_P256, token: anti_clogging_token(&self.secret, &self.sta) };
                    return vec![SaeAction::Send(encode_sae(self.sta, self.bssid, 0, &body))];
                }
                let mut sae = match Sae::new(&self.password, self.bssid, self.sta, rng) {
                    Ok(s) => s,
                    Err(e) => return vec![SaeAction::Failed(e)],
                };
                let confirm = match sae.on_commit(scalar, element) {
                    Ok(c) => c,
                    Err(e) => return vec![SaeAction::Failed(e)],
                };
                let out = vec![SaeAction::Send(encode_sae(self.sta, self.bssid, 0, &sae.commit(&[]))), SaeAction::Send(encode_sae(self.sta, self.bssid, 0, &confirm))];
                self.sae = Some(sae);
                out
            }
            SaeBody::Confirm { send_confirm, confirm } => {
                let Some(sae) = self.sae.as_mut() else { return vec![] };
                match sae.on_confirm(*send_confirm, confirm) {
                    Ok(keys) => {
                        self.done = true;
                        vec![SaeAction::Keys(keys)]
                    }
                    Err(e) => vec![SaeAction::Failed(e)],
                }
            }
            SaeBody::TokenRequest { .. } | SaeBody::Rejected { .. } => vec![],
        }
    }
}

/// Retransmissions of an SA Query request before the old association is believed gone (802.11 dot11AssociationSAQueryMaximumTimeout
/// is 1 s with a retry every 200 ms here).
pub const SA_QUERY_RETRY_MS: u64 = 200;
pub const SA_QUERY_TIMEOUT_MS: u64 = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaQueryAction {
    Send(Vec<u8>),
    /// The station answered the query: the new association request was forged or stale; keep the old association.
    KeepAssociation,
    /// No answer in time: the station is gone, accept the new association.
    AcceptNew,
}

/// A station that is associated sends a new association request (maybe a forged one): the access point asks the old association with
/// SA Query requests before it lets the request through (802.11w).
pub struct SaQueryTimer {
    bssid: Mac,
    sta: Mac,
    transaction: u16,
    next_send: u64,
    deadline: u64,
    finished: bool,
}

impl SaQueryTimer {
    /// Starts the query and returns it with the first request. `transaction` is random.
    pub fn start(bssid: Mac, sta: Mac, transaction: u16, now_ms: u64) -> (SaQueryTimer, SaQueryAction) {
        let t = SaQueryTimer { bssid, sta, transaction, next_send: now_ms + SA_QUERY_RETRY_MS, deadline: now_ms + SA_QUERY_TIMEOUT_MS, finished: false };
        let first = SaQueryAction::Send(encode_sa_query(sta, bssid, false, transaction));
        (t, first)
    }

    pub fn on_response(&mut self, r: &SaQuery) -> Option<SaQueryAction> {
        if self.finished || r.from != self.sta || !crate::sae::sa_query_answers(self.transaction, r) {
            return None;
        }
        self.finished = true;
        Some(SaQueryAction::KeepAssociation)
    }

    pub fn tick(&mut self, now_ms: u64) -> Option<SaQueryAction> {
        if self.finished {
            return None;
        }
        if now_ms >= self.deadline {
            self.finished = true;
            return Some(SaQueryAction::AcceptNew);
        }
        if now_ms >= self.next_send {
            self.next_send = now_ms + SA_QUERY_RETRY_MS;
            return Some(SaQueryAction::Send(encode_sa_query(self.sta, self.bssid, false, self.transaction)));
        }
        None
    }
}

#[cfg(test)]
#[path = "sae_ap_tests.rs"]
mod tests;
