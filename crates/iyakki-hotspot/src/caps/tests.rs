//! Fixtures: hand-built answers of `NL80211_CMD_GET_WIPHY` (split dump) in the layout of linux/nl80211.h, shaped after what
//! `iw phy` prints for the card families named. No real kernel dump was available offline, so the CONTENTS (channel flags,
//! combinations) are illustrative reconstructions, not captures; the structure (attribute numbers, nesting) is the kernel's.

use super::*;
use crate::nl80211::{Msg, NLMSG_DONE};

const FAM: u16 = 0x1c;
const STA: u32 = IFTYPE_STATION;
const AP: u32 = IFTYPE_AP;

struct Spec {
    index: u32,
    name: &'static str,
    iftypes: Vec<u32>,
    /// (limits as (max, types), max_total, num_channels)
    combos: Vec<(Vec<(u32, Vec<u32>)>, u32, u32)>,
    /// (band id, channels as (freq, flags: d=disabled n=no-ir r=radar))
    bands: Vec<(u32, Vec<(u32, &'static str)>)>,
    ciphers: Vec<u32>,
    max_ap: Option<u32>,
    ext_bits: Vec<usize>,
    self_managed: bool,
    ap_sme: bool,
}

fn base(index: u32, name: &'static str) -> Spec {
    Spec { index, name, iftypes: vec![STA, AP, 6], combos: vec![], bands: vec![], ciphers: vec![CIPHER_TKIP, CIPHER_CCMP], max_ap: None, ext_bits: vec![], self_managed: false, ap_sme: false }
}

fn chans24(flags_12_13: &'static str, flags_14: &'static str) -> Vec<(u32, &'static str)> {
    let mut v: Vec<(u32, &'static str)> = (1..=11).map(|c| (2407 + 5 * c, "")).collect();
    v.push((2467, flags_12_13));
    v.push((2472, flags_12_13));
    v.push((2484, flags_14));
    v
}

fn chans5(world: bool) -> Vec<(u32, &'static str)> {
    // 36-48 normally open; 52-64 and 100-144 radar; 149-165 open. In the world domain everything is no-IR.
    let mut v = Vec::new();
    for c in [36, 40, 44, 48] {
        v.push((5000 + 5 * c, if world { "n" } else { "" }));
    }
    for c in [52, 56, 60, 64, 100, 104, 108, 112, 116, 120, 124, 128, 132, 136, 140, 144] {
        v.push((5000 + 5 * c, if world { "nr" } else { "r" }));
    }
    for c in [149, 153, 157, 161, 165] {
        v.push((5000 + 5 * c, if world { "n" } else { "" }));
    }
    v
}

/// Build the messages of one radio the way a split dump delivers it: the first message has the identity and types, the next
/// ones the bands (one band per message), the last the combinations, ciphers and features.
fn dump_of(specs: &[Spec]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut seq = 1;
    for s in specs {
        // message 1: identity and interface types
        let m1 = Msg::new(FAM, 2, seq, CMD_NEW_WIPHY).u32(A_WIPHY, s.index).strz(A_WIPHY_NAME, s.name).nest(A_SUPPORTED_IFTYPES, |m| s.iftypes.iter().fold(m, |m, t| m.flag(*t as u16)));
        out.extend(m1.finish());
        // one message per band
        for (id, chans) in &s.bands {
            let m = Msg::new(FAM, 2, seq, CMD_NEW_WIPHY).u32(A_WIPHY, s.index).nest(A_WIPHY_BANDS, |m| {
                m.nest(*id as u16, |b| {
                    b.nest(1, |f| {
                        chans.iter().enumerate().fold(f, |f, (i, (freq, fl))| {
                            f.nest(i as u16, |c| {
                                let mut c = c.u32(1, *freq).u32(6, 2000);
                                if fl.contains('d') {
                                    c = c.flag(2);
                                }
                                if fl.contains('n') {
                                    c = c.flag(3);
                                }
                                if fl.contains('r') {
                                    c = c.flag(5);
                                }
                                c
                            })
                        })
                    })
                })
            });
            out.extend(m.finish());
        }
        // last message: combinations, ciphers, limits, features
        let mut m = Msg::new(FAM, 2, seq, CMD_NEW_WIPHY).u32(A_WIPHY, s.index);
        if !s.combos.is_empty() {
            m = m.nest(A_IFACE_COMBINATIONS, |m| {
                s.combos.iter().enumerate().fold(m, |m, (i, (limits, total, chans))| {
                    m.nest(i as u16, |c| {
                        c.nest(1, |l| limits.iter().enumerate().fold(l, |l, (j, (max, types))| l.nest(j as u16, |x| x.u32(1, *max).nest(2, |t| types.iter().fold(t, |t, ty| t.flag(*ty as u16))))))
                            .u32(2, *total)
                            .u32(4, *chans)
                    })
                })
            });
        }
        let mut cs = Vec::new();
        for c in &s.ciphers {
            cs.extend_from_slice(&c.to_le_bytes());
        }
        m = m.bytes(A_CIPHER_SUITES, &cs);
        if let Some(n) = s.max_ap {
            m = m.u32(A_MAX_AP_ASSOC_STA, n);
        }
        if !s.ext_bits.is_empty() {
            let mut bits = vec![0u8; 10];
            for b in &s.ext_bits {
                bits[b / 8] |= 1 << (b % 8);
            }
            m = m.bytes(A_EXT_FEATURES, &bits);
        }
        if s.self_managed {
            m = m.flag(A_SELF_MANAGED_REG);
        }
        if s.ap_sme {
            m = m.u32(A_DEVICE_AP_SME, 1);
        }
        out.extend(m.finish());
        seq += 1;
    }
    let mut d = Vec::new();
    d.extend_from_slice(&20u32.to_le_bytes());
    d.extend_from_slice(&NLMSG_DONE.to_le_bytes());
    d.extend_from_slice(&2u16.to_le_bytes());
    d.extend_from_slice(&[0; 12]);
    out.extend(d);
    out
}

fn one(spec: Spec) -> PhyCaps {
    parse_phy_dump(&dump_of(&[spec])).expect("parsed").remove(0)
}

fn fixed_ap_flags(s: &mut Spec) {
    s.ext_bits = vec![EXT_CONTROL_PORT_OVER_NL80211];
}

/// Intel iwlwifi (SoftMAC, mac80211): `#{ managed } <= 2, #{ AP } <= 1, total <= 3, #channels <= 2`; world domain: 12-13 no-IR, 5 GHz no-IR.
fn intel_world() -> Spec {
    let mut s = base(0, "phy0");
    s.iftypes = vec![STA, AP, 6, 8, 9, 10];
    s.combos = vec![(vec![(2, vec![STA]), (1, vec![AP]), (1, vec![10])], 3, 2)];
    s.bands = vec![(BAND_2GHZ, chans24("n", "d")), (BAND_5GHZ, chans5(true))];
    s.ciphers = vec![CIPHER_TKIP, CIPHER_CCMP, CIPHER_GCMP];
    fixed_ap_flags(&mut s);
    s
}

/// Atheros AR9271 over USB (ath9k_htc): 2.4 GHz only, AP and client together on ONE channel.
fn ath9k_htc() -> Spec {
    let mut s = base(1, "phy1");
    s.iftypes = vec![STA, AP, 6, 8, 9, 1, 7];
    s.combos = vec![(vec![(2, vec![STA]), (8, vec![AP]), (1, vec![8]), (1, vec![9])], 8, 1)];
    s.bands = vec![(BAND_2GHZ, chans24("n", "d"))];
    s.max_ap = Some(8);
    s
}

/// MediaTek MT7612U (mt76): 2.4 and 5 GHz, AP and client together on one channel.
fn mt76() -> Spec {
    let mut s = base(2, "phy2");
    s.combos = vec![(vec![(2, vec![STA]), (8, vec![AP])], 8, 1)];
    s.bands = vec![(BAND_2GHZ, chans24("n", "d")), (BAND_5GHZ, chans5(false))];
    s.max_ap = Some(16);
    s
}

/// Broadcom brcmfmac (FullMAC): the driver does authentication and association; no AP PSK offload in this fixture.
fn brcmfmac() -> Spec {
    let mut s = base(3, "phy3");
    s.ap_sme = true;
    s.combos = vec![(vec![(2, vec![STA]), (1, vec![AP])], 3, 2)];
    s.bands = vec![(BAND_2GHZ, chans24("", "d")), (BAND_5GHZ, chans5(false))];
    s
}

fn no_ap_card() -> Spec {
    let mut s = base(4, "phy4");
    s.iftypes = vec![STA, 6];
    s.combos = vec![(vec![(1, vec![STA])], 1, 1)];
    s.bands = vec![(BAND_2GHZ, chans24("", "d"))];
    s
}

#[test]
fn the_dump_is_merged_per_radio_and_the_parts_are_read() {
    let caps = parse_phy_dump(&dump_of(&[intel_world(), ath9k_htc()])).unwrap();
    assert_eq!(caps.len(), 2);
    let i = &caps[0];
    assert_eq!((i.index, i.name.as_str()), (0, "phy0"));
    assert!(i.supports_iftype(AP) && i.supports_iftype(STA) && !i.supports_iftype(IFTYPE_AP_VLAN));
    assert_eq!(i.bands.len(), 2, "bands arrive in separate messages and are merged");
    assert_eq!(i.band(BAND_2GHZ).unwrap().channels.len(), 14);
    assert_eq!(i.band(BAND_2GHZ).unwrap().channels[5], Channel { freq: 2437, ..Default::default() });
    assert!(i.band(BAND_2GHZ).unwrap().channels[11].no_ir && i.band(BAND_2GHZ).unwrap().channels[13].disabled);
    assert!(i.band(BAND_5GHZ).unwrap().channels.iter().all(|c| c.no_ir));
    assert_eq!(i.combos.len(), 1);
    assert_eq!(i.combos[0].num_channels, 2);
    assert!(i.ciphers.contains(&CIPHER_CCMP) && i.ciphers.contains(&CIPHER_GCMP));
    assert!(i.ext_feature(EXT_CONTROL_PORT_OVER_NL80211) && !i.ext_feature(EXT_SAE_OFFLOAD_AP));
    assert_eq!(caps[1].max_ap_stations, Some(8));
    assert_eq!(Channel { freq: 2437, ..Default::default() }.number(), 6);
    assert_eq!(Channel { freq: 5180, ..Default::default() }.number(), 36);
}

#[test]
fn combinations_are_matched_by_type_count_and_total() {
    let c = &one(ath9k_htc()).combos[0];
    assert!(c.allows(&[AP]) && c.allows(&[AP, STA]) && c.allows(&[AP, AP, STA]));
    assert!(!c.allows(&[STA, STA, STA]) && !c.allows(&[9, 9]));
    let shared = IfComb { limits: vec![IfLimit { max: 1, types: vec![STA, AP] }], max_total: 1, num_channels: 1, sta_ap_bi_match: false };
    assert!(shared.allows(&[AP]) && shared.allows(&[STA]) && !shared.allows(&[AP, STA]));
}

#[test]
fn intel_in_the_world_domain_gets_2_4_ghz_channel_6_and_5_ghz_needs_a_country() {
    let caps = one(intel_world());
    let p = can_host_ap(&caps, &Want::new(BAND_2GHZ, "00")).unwrap();
    assert_eq!((p.channel, p.freq, p.width_mhz, p.mode), (6, 2437, 20, ApMode::Software));
    // no-IR 12 and 13 and the disabled 14 are never picked
    let e = can_host_ap(&caps, &Want::new(BAND_5GHZ, "00")).unwrap_err();
    assert_eq!(e.fix, Fix::SetCountry);
    assert!(e.reason.contains("country"), "{}", e.reason);
    // with a country set the radar-free 5 GHz channels open up: here the fixture is re-built as a country card
    let mut c2 = intel_world();
    c2.bands = vec![(BAND_2GHZ, chans24("", "d")), (BAND_5GHZ, chans5(false))];
    let p5 = can_host_ap(&one(c2), &Want::new(BAND_5GHZ, "DE")).unwrap();
    assert_eq!((p5.channel, p5.width_mhz), (36, 20), "the first channel without radar detection");
}

#[test]
fn ath9k_htc_hosts_an_access_point_and_a_client_on_the_same_channel() {
    let caps = one(ath9k_htc());
    let mut w = Want::new(BAND_2GHZ, "00");
    w.concurrent_sta = true;
    w.sta_channel_freq = Some(2412);
    let p = can_host_ap(&caps, &w).unwrap();
    assert!(p.follows_sta_channel);
    assert_eq!((p.channel, p.max_stations), (1, Some(8)), "the access point sits on the client's channel 1");
    w.sta_channel_freq = Some(2467);
    let e = can_host_ap(&caps, &w).unwrap_err();
    assert_eq!(e.fix, Fix::UseAnotherUplinkOrCard, "the client is on a no-IR channel: {}", e.reason);
    assert!(can_host_ap(&caps, &Want::new(BAND_5GHZ, "00")).unwrap_err().reason.contains("5 GHz"));
}

#[test]
fn mt76_offers_both_bands_and_picks_the_usual_channels() {
    let caps = one(mt76());
    assert_eq!(can_host_ap(&caps, &Want::new(BAND_2GHZ, "DE")).unwrap().channel, 6);
    let p5 = can_host_ap(&caps, &Want::new(BAND_5GHZ, "DE")).unwrap();
    assert_eq!(p5.channel, 36);
    assert!(!p5.follows_sta_channel);
}

#[test]
fn brcmfmac_is_card_managed_and_a_psk_offload_feature_changes_the_mode() {
    let mut s = brcmfmac();
    let p = can_host_ap(&one(brcmfmac()), &Want::new(BAND_2GHZ, "US")).unwrap();
    assert_eq!(p.mode, ApMode::CardManaged);
    s.ext_bits = vec![EXT_4WAY_HANDSHAKE_AP_PSK];
    assert_eq!(can_host_ap(&one(s), &Want::new(BAND_2GHZ, "US")).unwrap().mode, ApMode::PskOffload);
}

#[test]
fn a_card_without_access_point_mode_says_so() {
    let e = can_host_ap(&one(no_ap_card()), &Want::new(BAND_2GHZ, "DE")).unwrap_err();
    assert!(e.reason.contains("cannot work as an access point"), "{}", e.reason);
    assert_eq!(e.fix, Fix::None);
}

#[test]
fn a_radio_that_allows_only_one_interface_cannot_be_its_own_uplink() {
    let mut s = mt76();
    s.combos = vec![(vec![(1, vec![STA, AP])], 1, 1)];
    let caps = one(s);
    assert!(can_host_ap(&caps, &Want::new(BAND_2GHZ, "DE")).is_ok());
    let mut w = Want::new(BAND_2GHZ, "DE");
    w.concurrent_sta = true;
    let e = can_host_ap(&caps, &w).unwrap_err();
    assert_eq!(e.fix, Fix::UseAnotherUplinkOrCard);
    assert!(e.reason.contains("another uplink or another card"), "{}", e.reason);
}

#[test]
fn an_unconfigured_country_offers_to_set_one_but_a_self_managed_card_cannot() {
    let mut s = mt76();
    s.bands = vec![(BAND_2GHZ, chans24("n", "d").into_iter().map(|(f, _)| (f, "n")).collect())];
    let e = can_host_ap(&one(s), &Want::new(BAND_2GHZ, "00")).unwrap_err();
    assert_eq!(e.fix, Fix::SetCountry);
    let mut m = mt76();
    m.self_managed = true;
    m.bands = vec![(BAND_2GHZ, chans24("n", "d").into_iter().map(|(f, _)| (f, "n")).collect())];
    let e2 = can_host_ap(&one(m), &Want::new(BAND_2GHZ, "99")).unwrap_err();
    assert_eq!(e2.fix, Fix::None, "{}", e2.reason);
    assert!(e2.reason.contains("firmware"));
    let mut dfs = mt76();
    dfs.bands = vec![(BAND_5GHZ, chans5(false).into_iter().map(|(f, _)| (f, "r")).collect())];
    let e3 = can_host_ap(&one(dfs), &Want::new(BAND_5GHZ, "DE")).unwrap_err();
    assert!(e3.reason.contains("radar"), "{}", e3.reason);
}

#[test]
fn a_card_without_ccmp_or_without_any_cipher_is_refused() {
    let mut s = mt76();
    s.ciphers = vec![CIPHER_TKIP];
    assert!(can_host_ap(&one(s), &Want::new(BAND_2GHZ, "DE")).unwrap_err().reason.contains("CCMP"));
    let mut t = mt76();
    t.ciphers = vec![];
    assert!(can_host_ap(&one(t), &Want::new(BAND_2GHZ, "DE")).is_err());
}

#[test]
fn a_virtual_hwsim_like_radio_with_many_combinations_is_accepted() {
    let mut s = base(5, "phy5");
    s.iftypes = vec![STA, AP, 1, 4, 6, 7, 8, 9, 10];
    s.combos = vec![(vec![(1, vec![1, 7]), (1, vec![STA]), (8, vec![AP, 9])], 8, 1), (vec![(1, vec![STA]), (1, vec![10])], 2, 1)];
    s.bands = vec![(BAND_2GHZ, chans24("n", "d")), (BAND_5GHZ, chans5(true))];
    let caps = one(s);
    let mut w = Want::new(BAND_2GHZ, "00");
    w.concurrent_sta = true;
    assert_eq!(can_host_ap(&caps, &w).unwrap().channel, 6);
}

#[test]
fn the_requests_and_the_country_reply_have_the_documented_shape() {
    let r = get_wiphy_split_dump(FAM, 4);
    assert_eq!(crate::hex(&r), "18000000 1c00 0103 04000000 00000000 01000000 0400ae00".replace(' ', ""), "split flag is attribute 174");
    let s = req_set_reg(FAM, 5, "DE").unwrap();
    assert_eq!(crate::hex(&s), "1c000000 1c00 0500 05000000 00000000 1b000000 07002100 44450000".replace(' ', ""));
    assert!(req_set_reg(FAM, 5, "de").is_none() && req_set_reg(FAM, 5, "DEU").is_none());
    let reply = Msg::new(FAM, 0, 6, 31).bytes(A_REG_ALPHA2, b"00\0").finish();
    assert_eq!(parse_country(&reply), Some("00".to_string()));
    assert_eq!(parse_country(&[1, 2, 3]), None);
}

#[test]
fn the_parsers_never_panic_on_junk_and_the_no_card_reasons_differ() {
    let good = dump_of(&[intel_world()]);
    let mut x = 99u32;
    for n in 0..400 {
        let mut b = good.clone();
        for _ in 0..3 {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            let i = (x >> 8) as usize % b.len();
            b[i] = (x >> 24) as u8;
        }
        b.truncate(b.len() - (n % 7));
        let _ = parse_phy_dump(&b);
        let _ = parse_country(&b);
    }
    assert_ne!(no_card_reason(true), no_card_reason(false));
    assert!(no_card_reason(true).contains("firmware"));
}

#[test]
fn a_card_that_allows_two_channels_still_puts_the_access_point_on_the_clients_channel() {
    let caps = one(brcmfmac());
    let mut w = Want::new(BAND_2GHZ, "00");
    w.concurrent_sta = true;
    w.sta_channel_freq = Some(2417);
    let p = can_host_ap(&caps, &w).unwrap();
    assert!(p.follows_sta_channel);
    assert_eq!(p.channel, 2, "the client is on channel 2");
    w.sta_channel_freq = None;
    assert_eq!(can_host_ap(&caps, &w).unwrap().channel, 6, "client channel unknown: the usual pick");
}
