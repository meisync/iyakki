//! The security the hotspot offers in its beacons (F-HOTSPOT-04, F-HOTSPOT-18): the RSN element for WPA2-personal, WPA3-personal
//! (SAE) and the transition mode that offers both, with protected management frames (802.11w), and the choice between them from what
//! the card can do. This is only the description that goes into the beacon and probe response and the decision; the SAE exchange
//! itself (a card that runs it is told by `EXT_SAE_OFFLOAD_AP`) is not in this module, so the hotspot does not offer WPA3 unless the
//! card does the exchange.
// SPDX-License-Identifier: Apache-2.0

use crate::caps::{PhyCaps, EXT_SAE_OFFLOAD_AP};

const OUI: [u8; 3] = [0x00, 0x0f, 0xac];
const CCMP: u8 = 4;
const AKM_PSK: u8 = 2;
const AKM_SAE: u8 = 8;
/// BIP with CMAC-128: the cipher that protects broadcast management frames.
const BIP_CMAC_128: u8 = 6;
/// RSN capabilities: bit 6 management frame protection required, bit 7 capable.
const MFPR: u16 = 1 << 6;
const MFPC: u16 = 1 << 7;

/// What the network offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecurityMode {
    /// WPA2-personal (PSK, CCMP), management frames not protected: what every phone and laptop joins.
    Wpa2,
    /// WPA2 and WPA3 together: PSK and SAE, protection capable (not required), so older devices still join.
    Transition,
    /// WPA3-personal only: SAE, protection required.
    Wpa3,
}

impl SecurityMode {
    pub fn label(self) -> &'static str {
        match self {
            SecurityMode::Wpa2 => "WPA2",
            SecurityMode::Transition => "WPA2/WPA3",
            SecurityMode::Wpa3 => "WPA3",
        }
    }

    /// Management frame protection (802.11w): none, offered or required.
    pub fn pmf(self) -> &'static str {
        match self {
            SecurityMode::Wpa2 => "off",
            SecurityMode::Transition => "optional",
            SecurityMode::Wpa3 => "required",
        }
    }
}

/// The RSN element (id 0x30) for the mode.
pub fn rsn_ie(mode: SecurityMode) -> Vec<u8> {
    let (akms, caps): (&[u8], u16) = match mode {
        SecurityMode::Wpa2 => (&[AKM_PSK], 0),
        SecurityMode::Transition => (&[AKM_PSK, AKM_SAE], MFPC),
        SecurityMode::Wpa3 => (&[AKM_SAE], MFPC | MFPR),
    };
    let mut b = vec![1, 0];
    b.extend_from_slice(&[OUI[0], OUI[1], OUI[2], CCMP]);
    b.extend_from_slice(&[1, 0, OUI[0], OUI[1], OUI[2], CCMP]);
    b.extend_from_slice(&(akms.len() as u16).to_le_bytes());
    for a in akms {
        b.extend_from_slice(&[OUI[0], OUI[1], OUI[2], *a]);
    }
    b.extend_from_slice(&caps.to_le_bytes());
    if mode != SecurityMode::Wpa2 {
        // no PMKIDs, then the group management cipher
        b.extend_from_slice(&[0, 0, OUI[0], OUI[1], OUI[2], BIP_CMAC_128]);
    }
    let mut v = vec![0x30, b.len() as u8];
    v.extend(b);
    v
}

/// An RSN element taken apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RsnInfo {
    pub group: u8,
    pub pairwise: Vec<u8>,
    pub akms: Vec<u8>,
    pub caps: u16,
    pub group_mgmt: Option<u8>,
}

impl RsnInfo {
    pub fn pmf_capable(&self) -> bool {
        self.caps & MFPC != 0
    }
    pub fn pmf_required(&self) -> bool {
        self.caps & MFPR != 0
    }
}

/// Reads an RSN element whose suites all belong to the standard's own group (00-0f-ac). None when it is cut, has another group or
/// another version.
pub fn parse_rsn(ie: &[u8]) -> Option<RsnInfo> {
    if ie.len() < 2 || ie[0] != 0x30 || ie.len() != 2 + ie[1] as usize {
        return None;
    }
    let mut b = &ie[2..];
    let take = |b: &mut &[u8], n: usize| -> Option<Vec<u8>> {
        let (h, t) = b.split_at_checked(n)?;
        let v = h.to_vec();
        *b = t;
        Some(v)
    };
    if take(&mut b, 2)? != [1, 0] {
        return None;
    }
    let suite = |b: &mut &[u8]| -> Option<u8> {
        let s = take(b, 4)?;
        (s[..3] == OUI).then_some(s[3])
    };
    let group = suite(&mut b)?;
    let list = |b: &mut &[u8]| -> Option<Vec<u8>> {
        let n = u16::from_le_bytes(take(b, 2)?.try_into().ok()?) as usize;
        if n > 8 {
            return None;
        }
        (0..n).map(|_| suite(b)).collect()
    };
    let pairwise = list(&mut b)?;
    let akms = list(&mut b)?;
    let caps = u16::from_le_bytes(take(&mut b, 2)?.try_into().ok()?);
    let mut group_mgmt = None;
    if !b.is_empty() {
        let pmkids = u16::from_le_bytes(take(&mut b, 2)?.try_into().ok()?) as usize;
        take(&mut b, 16 * pmkids)?;
        if !b.is_empty() {
            group_mgmt = Some(suite(&mut b)?);
        }
    }
    Some(RsnInfo { group, pairwise, akms, caps, group_mgmt })
}

/// Which mode a card can serve, and which one the person asked for. WPA3 needs the card to run the SAE exchange itself (the host
/// side has no SAE); if it does not, the hotspot stays on WPA2 and the second value says why in plain words.
pub fn choose_security(c: &PhyCaps, wanted: SecurityMode) -> (SecurityMode, Option<&'static str>) {
    if wanted == SecurityMode::Wpa2 || c.ext_feature(EXT_SAE_OFFLOAD_AP) {
        return (wanted, None);
    }
    (SecurityMode::Wpa2, Some("this card cannot run the WPA3 exchange itself and this version cannot do it on the computer, so the hotspot uses WPA2"))
}

/// The first byte pair of the country element (id 7): the two letters and a space for "any environment", then the channel runs.
/// `runs` are (first channel, number of channels, maximum power in dBm).
pub fn country_ie(alpha2: &str, runs: &[(u8, u8, u8)]) -> Result<Vec<u8>, String> {
    if !valid_country(alpha2) {
        return Err("a country code is two capital letters, for example DE or US".into());
    }
    if runs.is_empty() || runs.len() > 20 {
        return Err("a country element lists 1 to 20 channel runs".into());
    }
    let mut b = alpha2.as_bytes().to_vec();
    b.push(b' ');
    for (first, n, dbm) in runs {
        b.extend_from_slice(&[*first, *n, *dbm]);
    }
    if b.len() % 2 == 1 {
        b.push(0);
    }
    let mut v = vec![7, b.len() as u8];
    v.extend(b);
    Ok(v)
}

/// Two capital ASCII letters; the world domain "00" and the unset "99" are not countries.
pub fn valid_country(c: &str) -> bool {
    c.len() == 2 && c.bytes().all(|b| b.is_ascii_uppercase())
}

#[cfg(test)]
#[path = "rsn_tests.rs"]
mod tests;
