//! Capability probe: can this Wi-Fi interface host a hotspot, and if not, why not? Pure code over the answer of the kernel.
//!
//! The principle (owner decision 2026-11-04): the hotspot is GENERIC. There is no code for a chip or a driver. Whatever
//! wireless interface the kernel's cfg80211/nl80211 already exposes is asked what it can do (`NL80211_CMD_GET_WIPHY` with the
//! split-dump flag), and the answer alone decides. The driver name is read from sysfs only to be shown to a person.
//!
//! * [`get_wiphy_split_dump`] / [`parse_phy_dump`]: the request and the parser (split dumps are merged per radio).
//! * [`get_reg_request`] / [`parse_country`] / [`req_set_reg`]: the regulatory country (`00` is the unconfigured world domain).
//! * [`can_host_ap`]: the decision, with ONE line of words for every "no" and, where a person can fix it, the fix.

use crate::nl80211::{attrs, genl, split_messages, Msg, NLMSG_DONE, NLMSG_ERROR, NLM_F_ACK, NLM_F_DUMP, NLM_F_REQUEST};

pub const CMD_GET_WIPHY: u8 = 1;
pub const CMD_NEW_WIPHY: u8 = 3;
pub const CMD_REQ_SET_REG: u8 = 27;
pub const CMD_GET_REG: u8 = 31;
const A_WIPHY: u16 = 1;
const A_WIPHY_NAME: u16 = 2;
const A_SUPPORTED_IFTYPES: u16 = 32;
const A_REG_ALPHA2: u16 = 33;
const A_WIPHY_BANDS: u16 = 22;
const A_CIPHER_SUITES: u16 = 57;
const A_IFACE_COMBINATIONS: u16 = 120;
const A_FEATURE_FLAGS: u16 = 143;
const A_DEVICE_AP_SME: u16 = 141;
const A_SPLIT_WIPHY_DUMP: u16 = 174;
const A_MAX_AP_ASSOC_STA: u16 = 202;
const A_SELF_MANAGED_REG: u16 = 216;
const A_EXT_FEATURES: u16 = 217;

pub const IFTYPE_STATION: u32 = 2;
pub const IFTYPE_AP: u32 = 3;
pub const IFTYPE_AP_VLAN: u32 = 4;
pub const IFTYPE_P2P_GO: u32 = 9;

pub const CIPHER_TKIP: u32 = 0x000f_ac02;
pub const CIPHER_CCMP: u32 = 0x000f_ac04;
pub const CIPHER_GCMP: u32 = 0x000f_ac08;

/// `enum nl80211_ext_feature_index` values that matter here.
pub const EXT_4WAY_HANDSHAKE_AP_PSK: usize = 50;
pub const EXT_SAE_OFFLOAD_AP: usize = 51;
pub const EXT_CONTROL_PORT_OVER_NL80211: usize = 26;
pub const EXT_CONTROL_PORT_OVER_TX_STATUS: usize = 48;
pub const EXT_VLAN_OFFLOAD: usize = 39;
/// `enum nl80211_feature_flags` bit: the kernel handles SAE in its own SME (userspace SAE possible).
pub const FEATURE_SAE: u32 = 1 << 5;

/// The 2.4 GHz band id in `NL80211_ATTR_WIPHY_BANDS` (5 GHz is 1, 6 GHz is 3).
pub const BAND_2GHZ: u32 = 0;
pub const BAND_5GHZ: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Channel {
    pub freq: u32,
    pub disabled: bool,
    pub no_ir: bool,
    pub radar: bool,
    pub indoor_only: bool,
    pub no_ht40_plus: bool,
    pub no_ht40_minus: bool,
    pub no_20mhz: bool,
}

impl Channel {
    /// The channel number (2.4 and 5 GHz).
    pub fn number(&self) -> u32 {
        match self.freq {
            2484 => 14,
            2412..=2472 => (self.freq - 2407) / 5,
            5000..=5895 => (self.freq - 5000) / 5,
            _ => 0,
        }
    }

    /// May an access point start here without radar detection and without waiting for a regulatory domain?
    pub fn usable_for_ap(&self) -> bool {
        !self.disabled && !self.no_ir && !self.radar && !self.no_20mhz
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Band {
    pub id: u32,
    pub channels: Vec<Channel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IfLimit {
    pub max: u32,
    pub types: Vec<u32>,
}

/// One allowed set of interfaces: "at most `max` of these types" for each limit, `max_total` interfaces, `num_channels` channels.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IfComb {
    pub limits: Vec<IfLimit>,
    pub max_total: u32,
    pub num_channels: u32,
    pub sta_ap_bi_match: bool,
}

impl IfComb {
    /// Can interfaces of exactly these types exist together in this combination?
    pub fn allows(&self, wanted: &[u32]) -> bool {
        if wanted.len() as u32 > self.max_total {
            return false;
        }
        fn place(limits: &mut [u32], types: &[Vec<u32>], wanted: &[u32]) -> bool {
            let Some((first, rest)) = wanted.split_first() else { return true };
            for i in 0..limits.len() {
                if limits[i] > 0 && types[i].contains(first) {
                    limits[i] -= 1;
                    if place(limits, types, rest) {
                        return true;
                    }
                    limits[i] += 1;
                }
            }
            false
        }
        let mut left: Vec<u32> = self.limits.iter().map(|l| l.max).collect();
        let types: Vec<Vec<u32>> = self.limits.iter().map(|l| l.types.clone()).collect();
        place(&mut left, &types, wanted)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PhyCaps {
    pub index: u32,
    pub name: String,
    pub iftypes: Vec<u32>,
    pub combos: Vec<IfComb>,
    pub bands: Vec<Band>,
    pub ciphers: Vec<u32>,
    pub max_ap_stations: Option<u32>,
    pub feature_flags: u32,
    pub ext_features: Vec<u8>,
    /// The card's firmware decides the regulatory domain (the country cannot be set from here).
    pub self_managed_reg: bool,
    /// The driver does authentication and association of an access point itself (FullMAC style).
    pub ap_sme: bool,
    /// Filled in by the caller from sysfs, for the explanation only; never used to decide anything.
    pub driver: Option<String>,
}

impl PhyCaps {
    pub fn supports_iftype(&self, t: u32) -> bool {
        self.iftypes.contains(&t)
    }
    pub fn ext_feature(&self, bit: usize) -> bool {
        self.ext_features.get(bit / 8).is_some_and(|b| b & (1 << (bit % 8)) != 0)
    }
    pub fn band(&self, id: u32) -> Option<&Band> {
        self.bands.iter().find(|b| b.id == id)
    }
}

// ---- requests ----

/// A dump of all radios with the split-dump flag (without it the kernel sends the old, shortened form).
pub fn get_wiphy_split_dump(family: u16, seq: u32) -> Vec<u8> {
    Msg::new(family, NLM_F_REQUEST | NLM_F_DUMP, seq, CMD_GET_WIPHY).flag(A_SPLIT_WIPHY_DUMP).finish()
}

/// The regulatory domain in force (the country, or `00` for the world domain).
pub fn get_reg_request(family: u16, seq: u32) -> Vec<u8> {
    Msg::new(family, NLM_F_REQUEST | NLM_F_ACK, seq, CMD_GET_REG).finish()
}

/// Ask the kernel to use a country's rules (same as `iw reg set DE`). The country is two capital letters.
pub fn req_set_reg(family: u16, seq: u32, alpha2: &str) -> Option<Vec<u8>> {
    let b = alpha2.as_bytes();
    if b.len() != 2 || !b.iter().all(|c| c.is_ascii_uppercase()) {
        return None;
    }
    let mut v = b.to_vec();
    v.push(0);
    Some(Msg::new(family, NLM_F_REQUEST | NLM_F_ACK, seq, CMD_REQ_SET_REG).bytes(A_REG_ALPHA2, &v).finish())
}

// ---- parsers ----

fn u32_of(b: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(b.try_into().ok()?))
}

fn parse_channel(a: &[(u16, &[u8])]) -> Option<Channel> {
    let mut c = Channel::default();
    let mut have = false;
    for (t, v) in a {
        match t {
            1 => {
                c.freq = u32_of(v)?;
                have = true;
            }
            2 => c.disabled = true,
            3 => c.no_ir = true,
            5 => c.radar = true,
            9 => c.no_ht40_minus = true,
            10 => c.no_ht40_plus = true,
            14 => c.indoor_only = true,
            16 => c.no_20mhz = true,
            _ => {}
        }
    }
    have.then_some(c)
}

fn parse_combos(b: &[u8]) -> Option<Vec<IfComb>> {
    let mut out = Vec::new();
    for (_, one) in attrs(b)? {
        let mut c = IfComb { num_channels: 1, ..Default::default() };
        for (t, v) in attrs(one)? {
            match t {
                1 => {
                    for (_, l) in attrs(v)? {
                        let mut lim = IfLimit::default();
                        for (lt, lv) in attrs(l)? {
                            match lt {
                                1 => lim.max = u32_of(lv)?,
                                2 => lim.types = attrs(lv)?.iter().map(|(ty, _)| *ty as u32).collect(),
                                _ => {}
                            }
                        }
                        c.limits.push(lim);
                    }
                }
                2 => c.max_total = u32_of(v)?,
                3 => c.sta_ap_bi_match = true,
                4 => c.num_channels = u32_of(v)?,
                _ => {}
            }
        }
        out.push(c);
    }
    Some(out)
}

/// All radios out of the answer to [`get_wiphy_split_dump`]. The parts of one radio are merged; unknown attributes are skipped;
/// a damaged answer gives `None`.
pub fn parse_phy_dump(reply: &[u8]) -> Option<Vec<PhyCaps>> {
    let mut out: Vec<PhyCaps> = Vec::new();
    for m in split_messages(reply)? {
        if m.ty == NLMSG_DONE || m.ty == NLMSG_ERROR {
            continue;
        }
        let (c, a) = genl(m.payload)?;
        if c != CMD_NEW_WIPHY {
            continue;
        }
        let a = attrs(a)?;
        let Some(index) = a.iter().find(|(t, _)| *t == A_WIPHY).and_then(|(_, v)| u32_of(v)) else { continue };
        let pos = match out.iter().position(|p| p.index == index) {
            Some(p) => p,
            None => {
                out.push(PhyCaps { index, ..Default::default() });
                out.len() - 1
            }
        };
        let p = &mut out[pos];
        for (t, v) in &a {
            match *t {
                A_WIPHY_NAME => p.name = String::from_utf8_lossy(v.split(|b| *b == 0).next().unwrap_or(&[])).into_owned(),
                A_SUPPORTED_IFTYPES => p.iftypes = attrs(v)?.iter().map(|(ty, _)| *ty as u32).collect(),
                A_IFACE_COMBINATIONS => p.combos = parse_combos(v)?,
                A_CIPHER_SUITES => p.ciphers = v.chunks_exact(4).filter_map(u32_of).collect(),
                A_MAX_AP_ASSOC_STA => p.max_ap_stations = u32_of(v),
                A_FEATURE_FLAGS => p.feature_flags = u32_of(v).unwrap_or(0),
                A_EXT_FEATURES => p.ext_features = v.to_vec(),
                A_SELF_MANAGED_REG => p.self_managed_reg = true,
                A_DEVICE_AP_SME => p.ap_sme = true,
                A_WIPHY_BANDS => {
                    for (band_id, band) in attrs(v)? {
                        let mut chans = Vec::new();
                        for (bt, bv) in attrs(band)? {
                            if bt == 1 {
                                for (_, f) in attrs(bv)? {
                                    if let Some(ch) = parse_channel(&attrs(f)?) {
                                        chans.push(ch);
                                    }
                                }
                            }
                        }
                        match p.bands.iter_mut().find(|b| b.id == band_id as u32) {
                            Some(b) => b.channels.extend(chans),
                            None => p.bands.push(Band { id: band_id as u32, channels: chans }),
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Some(out)
}

/// The country out of the answer to [`get_reg_request`] (`00` = world domain, `99` = the card decides).
pub fn parse_country(reply: &[u8]) -> Option<String> {
    for m in split_messages(reply)? {
        let Some((_, a)) = genl(m.payload) else { continue };
        if m.ty == NLMSG_ERROR || m.ty == NLMSG_DONE {
            continue;
        }
        let a = attrs(a)?;
        if let Some((_, v)) = a.iter().find(|(t, _)| *t == A_REG_ALPHA2) {
            let s: String = v.iter().take_while(|b| **b != 0).map(|b| *b as char).collect();
            if s.len() == 2 {
                return Some(s);
            }
        }
    }
    None
}

// ---- the decision ----

/// Who runs the authentication, association and 4-way handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApMode {
    /// SoftMAC (mac80211): our authenticator does it (`mgmt`, `handshake`).
    Software,
    /// The card does the handshake and takes the PSK (`NL80211_EXT_FEATURE_4WAY_HANDSHAKE_AP_PSK`). Not built yet.
    PskOffload,
    /// The card does everything of the access point by itself (FullMAC with AP SME) and offers no PSK hand-over. Not built yet.
    CardManaged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Want {
    pub band: u32,
    /// The same radio must also stay connected as a Wi-Fi client (its uplink).
    pub concurrent_sta: bool,
    /// The regulatory country now in force (`00` unconfigured).
    pub country: String,
    /// When the radio also runs as a client, the channel it is on (the access point must follow it if the radio has one channel).
    pub sta_channel_freq: Option<u32>,
}

impl Want {
    pub fn new(band: u32, country: &str) -> Want {
        Want { band, concurrent_sta: false, country: country.to_string(), sta_channel_freq: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApPlan {
    pub freq: u32,
    pub channel: u32,
    pub width_mhz: u32,
    pub mode: ApMode,
    /// The radio has one channel for all its interfaces: the access point is on the client's channel.
    pub follows_sta_channel: bool,
    pub max_stations: Option<u32>,
}

/// What a person can do about a "no".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fix {
    SetCountry,
    UseAnotherUplinkOrCard,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// One line, plain words, for the Settings page.
    pub reason: String,
    pub fix: Fix,
}

fn no(reason: &str, fix: Fix) -> Result<ApPlan, Refusal> {
    Err(Refusal { reason: reason.to_string(), fix })
}

/// Why there is no card to offer at all (nothing came back from the kernel). `wifi_hardware_seen` is true when the bus lists
/// a Wi-Fi device (a PCI network controller, a USB wireless dongle) although no radio exists.
pub fn no_card_reason(wifi_hardware_seen: bool) -> &'static str {
    if wifi_hardware_seen {
        "A Wi-Fi device is present but has no driver or no firmware loaded; it cannot be used until its firmware is installed."
    } else {
        "No Wi-Fi card was found; plug in a Wi-Fi adapter."
    }
}

fn pick_channel(chans: &[Channel], band: u32) -> Option<&Channel> {
    let ok: Vec<&Channel> = chans.iter().filter(|c| c.usable_for_ap()).collect();
    if band == BAND_2GHZ {
        for pref in [6, 1, 11] {
            if let Some(c) = ok.iter().find(|c| c.number() == pref) {
                return Some(c);
            }
        }
        // 12 and 13 are not offered even when open: some phones refuse them
        return ok.iter().find(|c| c.number() <= 11).copied();
    }
    ok.first().copied()
}

/// Can this radio host a hotspot on `want.band`? On success the plan names the channel and who runs the handshake; on failure
/// one line of words and, if a person can fix it, the fix.
pub fn can_host_ap(caps: &PhyCaps, want: &Want) -> Result<ApPlan, Refusal> {
    if !caps.supports_iftype(IFTYPE_AP) {
        return no("This Wi-Fi card cannot work as an access point (its driver does not offer that mode).", Fix::None);
    }
    if !caps.ciphers.is_empty() && !caps.ciphers.contains(&CIPHER_CCMP) {
        return no("This card does not support the encryption (CCMP) that a safe hotspot needs.", Fix::None);
    }
    if caps.ciphers.is_empty() {
        return no("This card does not report any encryption support.", Fix::None);
    }
    // interface combinations: an access point alone, or together with a client
    let wanted: Vec<u32> = if want.concurrent_sta { vec![IFTYPE_AP, IFTYPE_STATION] } else { vec![IFTYPE_AP] };
    let comb = caps.combos.iter().filter(|c| c.allows(&wanted)).max_by_key(|c| c.num_channels);
    if comb.is_none() {
        return if want.concurrent_sta && caps.combos.iter().any(|c| c.allows(&[IFTYPE_AP])) {
            no("This card cannot be an access point and a Wi-Fi client at the same time; use another uplink or another card.", Fix::UseAnotherUplinkOrCard)
        } else if caps.combos.is_empty() {
            // an old driver without combinations: the kernel allows one interface at a time, which is enough for an access point alone
            if want.concurrent_sta {
                no("This card cannot be an access point and a Wi-Fi client at the same time; use another uplink or another card.", Fix::UseAnotherUplinkOrCard)
            } else {
                plan_channel(caps, want, None)
            }
        } else {
            no("This card's driver does not allow an access point in this setup.", Fix::None)
        };
    }
    plan_channel(caps, want, comb)
}

fn plan_channel(caps: &PhyCaps, want: &Want, comb: Option<&IfComb>) -> Result<ApPlan, Refusal> {
    let mode = if caps.ap_sme {
        if caps.ext_feature(EXT_4WAY_HANDSHAKE_AP_PSK) {
            ApMode::PskOffload
        } else {
            ApMode::CardManaged
        }
    } else {
        ApMode::Software
    };
    // the radio says it can use two channels at once, but a client on one channel and an access point on another make cards
    // like the Broadcom ones miss frames: when the client's channel is known, the access point always sits on it
    let follows = want.concurrent_sta && (want.sta_channel_freq.is_some() || comb.is_some_and(|cb| cb.num_channels < 2));
    let Some(band) = caps.band(want.band).filter(|b| !b.channels.is_empty()) else {
        return no(if want.band == BAND_2GHZ { "This card has no 2.4 GHz band." } else { "This card has no 5 GHz band." }, Fix::None);
    };
    let chosen = if follows {
        // the radio has one channel: the access point must sit on the client's channel, and it must be allowed there
        match want.sta_channel_freq.and_then(|f| band.channels.iter().find(|c| c.freq == f)) {
            Some(c) if c.usable_for_ap() => Some(c),
            Some(_) => return no("The Wi-Fi client of this card is on a channel where an access point is not allowed; use another uplink or another card.", Fix::UseAnotherUplinkOrCard),
            None => pick_channel(&band.channels, want.band),
        }
    } else {
        pick_channel(&band.channels, want.band)
    };
    match chosen {
        Some(c) => Ok(ApPlan { freq: c.freq, channel: c.number(), width_mhz: 20, mode, follows_sta_channel: follows, max_stations: caps.max_ap_stations }),
        None => {
            let only_waiting = band.channels.iter().any(|c| !c.disabled && c.no_ir && !c.radar);
            if want.country == "00" && !caps.self_managed_reg {
                no("No channel may be used until a country is chosen; set the country and try again.", Fix::SetCountry)
            } else if caps.self_managed_reg && only_waiting {
                no("The card's own firmware decides the country rules and allows no access point on this band here.", Fix::None)
            } else {
                no("No channel on this band can host an access point in this country (only radar-checked or disabled channels).", Fix::None)
            }
        }
    }
}

#[cfg(test)]
mod tests;
