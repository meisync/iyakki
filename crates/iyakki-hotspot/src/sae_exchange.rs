//! The WPA3-SAE exchange itself (F-HOTSPOT-22; IEEE 802.11-2016 clause 12.4), for a card that leaves the handshake to the computer:
//! the password element by hunting and pecking on NIST P-256 (group 19), the commit (scalar and element), the shared secret, the
//! key derivation (KCK, PMK, PMKID) and the confirm hashes. The frames around it are in [`crate::sae`]; the curve arithmetic is
//! `iyakki_webcrypto::p256`. The same type serves either side of the exchange (the access point or, in tests, the station): the
//! password element is derived from the larger and the smaller address, so both sides get the same one.
//!
//! Timing: the search for the password element always runs 40 rounds (more only if none was found yet) and the point arithmetic is
//! not constant time (see `iyakki_webcrypto::p256`); a radio-range attacker measuring the response time of an access point can learn
//! a little about the password, as with every hunting-and-pecking implementation. Hash-to-element (status 126) is not implemented:
//! this access point does not announce it and a station that insists on it is refused (recorded in ALPHA-7-DECISIONS.md).
// SPDX-License-Identifier: Apache-2.0

use crate::sae::{SaeBody, CONFIRM_LEN, ELEMENT_LEN, GROUP_P256, SCALAR_LEN};
use crate::Mac;
use iyakki_webcrypto::kdf::hmac_sha256;
use iyakki_webcrypto::p256;

/// Rounds of the password-element search that always run (the standard's k for the elliptic-curve groups).
pub const PECK_ROUNDS: u32 = 40;
/// The send-confirm counter may not reach this value (the standard keeps 2^16 - 1 for "no more").
pub const SEND_CONFIRM_MAX: u16 = 0xffff;

/// KDF-Hash-Length of 12.4.5.2 (also the KDF-SHA256 of the pairwise key expansion): HMAC-SHA256 in counter mode over
/// `counter(2, little endian) || label || context || length-in-bits(2, little endian)`, cut to `bits` bits (a multiple of 8 here).
pub fn kdf_sha256(key: &[u8], label: &str, context: &[u8], bits: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(bits.div_ceil(256) * 32);
    let mut i: u16 = 1;
    while out.len() * 8 < bits {
        let mut m = Vec::with_capacity(4 + label.len() + context.len());
        m.extend_from_slice(&i.to_le_bytes());
        m.extend_from_slice(label.as_bytes());
        m.extend_from_slice(context);
        m.extend_from_slice(&(bits as u16).to_le_bytes());
        out.extend_from_slice(&hmac_sha256(key, &m));
        i += 1;
    }
    out.truncate(bits / 8);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaeError {
    /// The password is empty or longer than 63 bytes, or the random source failed.
    BadInput,
    /// No point was found for the password (never in practice: the search runs on until it finds one).
    NoPasswordElement,
    /// The peer's scalar is not in 2 ..= order-1, or its element is not on the curve.
    BadCommit,
    /// The peer sent our own commit back (a reflection attack).
    Reflection,
    /// The shared point came out as infinity.
    Degenerate,
    /// A step was taken out of order.
    OutOfOrder,
    /// The peer's confirm value is wrong: it used another password.
    WrongPassword,
    /// The peer's send-confirm counter is the reserved maximum.
    ConfirmExhausted,
}

/// What a finished exchange gives to the four-way handshake.
#[derive(Clone, PartialEq, Eq)]
pub struct SaeKeys {
    pub pmk: [u8; 32],
    pub pmkid: [u8; 16],
}

impl std::fmt::Debug for SaeKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SaeKeys(..)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaeState {
    /// Own commit made, the peer's not yet taken in.
    Committed,
    /// The peer's commit is in and the keys are derived; waiting for its confirm.
    Confirmed,
    /// The peer's confirm matched: both sides know the password.
    Accepted,
}

pub struct Sae {
    own: Mac,
    peer: Mac,
    pwe: [u8; 64],
    rand: [u8; 32],
    scalar: [u8; SCALAR_LEN],
    element: [u8; ELEMENT_LEN],
    state: SaeState,
    peer_scalar: [u8; SCALAR_LEN],
    peer_element: [u8; ELEMENT_LEN],
    kck: [u8; 32],
    keys: Option<SaeKeys>,
    send_confirm: u16,
}

impl std::fmt::Debug for Sae {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sae(..)")
    }
}

/// The password element for this password and this pair of addresses (12.4.4.2.2, hunting and pecking).
pub fn password_element(password: &[u8], a: &Mac, b: &Mac) -> Result<[u8; 64], SaeError> {
    if password.is_empty() || password.len() > 63 {
        return Err(SaeError::BadInput);
    }
    let mut addrs = [0u8; 12];
    addrs[..6].copy_from_slice(std::cmp::max(a, b));
    addrs[6..].copy_from_slice(std::cmp::min(a, b));
    let prime = p256::prime();
    let mut found: Option<[u8; 64]> = None;
    let mut counter: u32 = 1;
    while counter <= PECK_ROUNDS || found.is_none() {
        if counter > 255 {
            break;
        }
        let mut data = password.to_vec();
        data.push(counter as u8);
        let seed = hmac_sha256(&addrs, &data);
        let value = kdf_sha256(&seed, "SAE Hunting and Pecking", &prime, 256);
        counter += 1;
        if value[..] >= prime[..] {
            continue;
        }
        let x: [u8; 32] = match value.try_into() {
            Ok(x) => x,
            Err(_) => continue,
        };
        // keep the first hit, but keep searching so the number of rounds does not depend on the password
        if found.is_none() {
            found = p256::lift_x(&x, seed[31] & 1 == 1);
        }
    }
    found.ok_or(SaeError::NoPasswordElement)
}

fn random_scalar(rng: &mut dyn FnMut(&mut [u8]) -> bool) -> Result<[u8; 32], SaeError> {
    for _ in 0..64 {
        let mut s = [0u8; 32];
        if !rng(&mut s) {
            return Err(SaeError::BadInput);
        }
        if p256::scalar_in_range(&s) {
            return Ok(s);
        }
    }
    Err(SaeError::BadInput)
}

impl Sae {
    /// Starts an exchange: derives the password element and makes this side's commit. `rng` fills the buffer with random bytes and says
    /// whether it could (tests give a fixed one; the product uses `iyakki_sec::random_fill`).
    pub fn new(password: &[u8], own: Mac, peer: Mac, rng: &mut dyn FnMut(&mut [u8]) -> bool) -> Result<Sae, SaeError> {
        let pwe = password_element(password, &own, &peer)?;
        loop {
            let rand = random_scalar(rng)?;
            let mask = random_scalar(rng)?;
            let scalar = p256::scalar_add(&rand, &mask).ok_or(SaeError::BadInput)?;
            // a scalar of 0 or 1 is not allowed on the wire: draw again
            if scalar[..31].iter().all(|b| *b == 0) && scalar[31] <= 1 {
                continue;
            }
            let masked = p256::point_mul(&mask, &pwe).ok_or(SaeError::Degenerate)?;
            let element = p256::point_neg(&masked).ok_or(SaeError::Degenerate)?;
            return Ok(Sae {
                own,
                peer,
                pwe,
                rand,
                scalar,
                element,
                state: SaeState::Committed,
                peer_scalar: [0; 32],
                peer_element: [0; 64],
                kck: [0; 32],
                keys: None,
                send_confirm: 0,
            });
        }
    }

    pub fn state(&self) -> SaeState {
        self.state
    }

    /// This side's commit body (group 19, with the anti-clogging token the peer asked for, if any).
    pub fn commit(&self, token: &[u8]) -> SaeBody {
        SaeBody::Commit { group: GROUP_P256, token: token.to_vec(), scalar: self.scalar, element: self.element }
    }

    /// Takes in the peer's commit, derives the keys and returns the confirm to send back.
    pub fn on_commit(&mut self, scalar: &[u8; SCALAR_LEN], element: &[u8; ELEMENT_LEN]) -> Result<SaeBody, SaeError> {
        if self.state != SaeState::Committed {
            return Err(SaeError::OutOfOrder);
        }
        if !crate::sae::commit_bytes_plausible(scalar, element) || !p256::point_on_curve(element) {
            return Err(SaeError::BadCommit);
        }
        if *scalar == self.scalar && *element == self.element {
            return Err(SaeError::Reflection);
        }
        // K = rand * (peer-scalar * PWE + peer-element)
        let scaled = p256::point_mul(scalar, &self.pwe).ok_or(SaeError::BadCommit)?;
        let sum = p256::point_add(&scaled, element).ok_or(SaeError::Degenerate)?;
        let k = p256::point_mul(&self.rand, &sum).ok_or(SaeError::Degenerate)?;
        let keyseed = hmac_sha256(&[0u8; 32], &k[..32]);
        let ctx = p256::scalar_add(&self.scalar, scalar).ok_or(SaeError::BadCommit)?;
        let kp = kdf_sha256(&keyseed, "SAE KCK and PMK", &ctx, 512);
        self.kck.copy_from_slice(&kp[..32]);
        let mut pmk = [0u8; 32];
        pmk.copy_from_slice(&kp[32..]);
        let mut pmkid = [0u8; 16];
        pmkid.copy_from_slice(&ctx[..16]);
        self.keys = Some(SaeKeys { pmk, pmkid });
        self.peer_scalar = *scalar;
        self.peer_element = *element;
        self.state = SaeState::Confirmed;
        self.next_confirm()
    }

    fn confirm_value(&self, counter: u16, scalar: &[u8; 32], element: &[u8; 64], peer_scalar: &[u8; 32], peer_element: &[u8; 64]) -> [u8; CONFIRM_LEN] {
        let mut m = Vec::with_capacity(2 + 32 + 64 + 32 + 64);
        m.extend_from_slice(&counter.to_le_bytes());
        m.extend_from_slice(scalar);
        m.extend_from_slice(element);
        m.extend_from_slice(peer_scalar);
        m.extend_from_slice(peer_element);
        hmac_sha256(&self.kck, &m)
    }

    /// The next confirm to send (the counter goes up with every one; a retransmission asks for a new one).
    pub fn next_confirm(&mut self) -> Result<SaeBody, SaeError> {
        if self.state == SaeState::Committed {
            return Err(SaeError::OutOfOrder);
        }
        if self.send_confirm >= SEND_CONFIRM_MAX - 1 {
            return Err(SaeError::ConfirmExhausted);
        }
        self.send_confirm += 1;
        let confirm = self.confirm_value(self.send_confirm, &self.scalar, &self.element, &self.peer_scalar, &self.peer_element);
        Ok(SaeBody::Confirm { send_confirm: self.send_confirm, confirm })
    }

    /// Checks the peer's confirm. On a match the exchange is accepted and the keys are handed over once.
    pub fn on_confirm(&mut self, send_confirm: u16, confirm: &[u8; CONFIRM_LEN]) -> Result<SaeKeys, SaeError> {
        if self.state == SaeState::Committed {
            return Err(SaeError::OutOfOrder);
        }
        if send_confirm == SEND_CONFIRM_MAX {
            return Err(SaeError::ConfirmExhausted);
        }
        let want = self.confirm_value(send_confirm, &self.peer_scalar, &self.peer_element, &self.scalar, &self.element);
        if !crate::ct_eq(&want, confirm) {
            return Err(SaeError::WrongPassword);
        }
        self.state = SaeState::Accepted;
        self.keys.clone().ok_or(SaeError::OutOfOrder)
    }

    pub fn own(&self) -> Mac {
        self.own
    }
    pub fn peer(&self) -> Mac {
        self.peer
    }
}

/// An anti-clogging token for a station (12.4.6): a keyed hash of its address under a secret the access point changes now and then;
/// only a station that really sits at that address can send it back.
pub fn anti_clogging_token(secret: &[u8; 32], sta: &Mac) -> Vec<u8> {
    hmac_sha256(secret, sta)[..16].to_vec()
}

pub fn anti_clogging_token_ok(secret: &[u8; 32], sta: &Mac, token: &[u8]) -> bool {
    crate::ct_eq(&anti_clogging_token(secret, sta), token)
}

#[cfg(test)]
#[path = "sae_exchange_tests.rs"]
mod tests;
