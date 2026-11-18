use super::*;
use crate::caps::PhyCaps;
use crate::{eapol, hex};

#[test]
fn the_wpa2_element_is_the_one_the_handshake_already_checks() {
    assert_eq!(rsn_ie(SecurityMode::Wpa2), eapol::rsn_ie_ccmp_psk());
}

#[test]
fn wpa3_and_the_transition_mode_have_the_standards_bytes() {
    // version 1, group CCMP, one pairwise CCMP, one AKM SAE (00-0f-ac:8), capabilities MFPC|MFPR = 0x00c0, no PMKIDs, BIP-CMAC-128
    assert_eq!(hex(&rsn_ie(SecurityMode::Wpa3)), "301a0100000fac040100000fac040100000fac08c0000000000fac06");
    // PSK and SAE, capabilities MFPC = 0x0080
    assert_eq!(hex(&rsn_ie(SecurityMode::Transition)), "301e0100000fac040100000fac040200000fac02000fac0880000000000fac06");
}

#[test]
fn an_element_is_read_back_and_damage_is_noticed() {
    for m in [SecurityMode::Wpa2, SecurityMode::Transition, SecurityMode::Wpa3] {
        let i = parse_rsn(&rsn_ie(m)).unwrap();
        assert_eq!((i.group, i.pairwise.clone()), (4, vec![4]));
        assert_eq!(i.akms, match m { SecurityMode::Wpa2 => vec![2], SecurityMode::Transition => vec![2, 8], SecurityMode::Wpa3 => vec![8] });
        assert_eq!((i.pmf_capable(), i.pmf_required()), (m != SecurityMode::Wpa2, m == SecurityMode::Wpa3), "{m:?}");
        assert_eq!(i.group_mgmt, if m == SecurityMode::Wpa2 { None } else { Some(6) });
    }
    let good = rsn_ie(SecurityMode::Wpa3);
    for n in 0..good.len() {
        // every cut is refused or read as a shorter whole element, never a panic
        let _ = parse_rsn(&good[..n]);
    }
    assert!(parse_rsn(&good[..good.len() - 1]).is_none());
    let mut other_oui = good.clone();
    other_oui[4] = 0x50;
    assert!(parse_rsn(&other_oui).is_none());
    let mut v2 = good.clone();
    v2[2] = 2;
    assert!(parse_rsn(&v2).is_none());
    assert!(parse_rsn(&[]).is_none() && parse_rsn(&[0x30]).is_none() && parse_rsn(&[0xdd, 0]).is_none());
    // a count that promises more suites than there are
    let mut big = good.clone();
    big[8] = 200;
    assert!(parse_rsn(&big).is_none());
    // hostile bytes never panic
    let mut s = 99u32;
    for n in 0..3000 {
        let len = n % 40;
        let v: Vec<u8> = (0..len).map(|_| { s = s.wrapping_mul(1664525).wrapping_add(1013904223); (s >> 24) as u8 }).collect();
        let mut w = vec![0x30, len as u8];
        w.extend(&v);
        let _ = parse_rsn(&w);
    }
}

#[test]
fn wpa3_is_offered_only_when_the_card_runs_the_exchange_itself() {
    let mut c = PhyCaps::default();
    assert_eq!(choose_security(&c, SecurityMode::Wpa2), (SecurityMode::Wpa2, None));
    let (m, why) = choose_security(&c, SecurityMode::Wpa3);
    assert_eq!(m, SecurityMode::Wpa2);
    assert!(why.unwrap().contains("uses WPA2"));
    assert_eq!(choose_security(&c, SecurityMode::Transition).0, SecurityMode::Wpa2);
    c.ext_features = vec![0; EXT_SAE_OFFLOAD_AP / 8 + 1];
    c.ext_features[EXT_SAE_OFFLOAD_AP / 8] |= 1 << (EXT_SAE_OFFLOAD_AP % 8);
    assert!(c.ext_feature(EXT_SAE_OFFLOAD_AP));
    assert_eq!(choose_security(&c, SecurityMode::Wpa3), (SecurityMode::Wpa3, None));
    assert_eq!(choose_security(&c, SecurityMode::Transition), (SecurityMode::Transition, None));
    assert_eq!((SecurityMode::Wpa3.label(), SecurityMode::Wpa3.pmf(), SecurityMode::Transition.pmf(), SecurityMode::Wpa2.pmf()), ("WPA3", "required", "optional", "off"));
}

#[test]
fn the_country_element_has_the_two_letters_a_space_the_runs_and_even_padding() {
    // 5 GHz in Germany: channels 36 to 48 and 52 to 64, 23 dBm and 20 dBm
    let ie = country_ie("DE", &[(36, 4, 23), (52, 4, 20)]).unwrap();
    assert_eq!(hex(&ie), "070a44452024041734041400");
    assert_eq!(ie.len() as u8 - 2, ie[1]);
    let odd = country_ie("US", &[(1, 11, 30)]).unwrap();
    assert_eq!(odd, vec![7, 6, b'U', b'S', b' ', 1, 11, 30]);
    assert!(country_ie("de", &[(1, 1, 1)]).is_err() && country_ie("DEU", &[(1, 1, 1)]).is_err() && country_ie("00", &[(1, 1, 1)]).is_err());
    assert!(country_ie("DE", &[]).is_err() && country_ie("DE", &[(1, 1, 1); 21]).is_err());
    assert!(valid_country("GB") && !valid_country("g") && !valid_country("1A") && !valid_country(""));
}
