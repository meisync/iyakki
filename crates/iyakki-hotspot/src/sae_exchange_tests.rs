// SPDX-License-Identifier: Apache-2.0
use super::*;

const AP: Mac = [2, 0, 0, 0, 0, 1];
const STA: Mac = [2, 0, 0, 0, 0, 9];

/// A repeatable byte source: SHA-256 style expansion of a counter, no clock and no entropy.
fn rng(seed: u8) -> impl FnMut(&mut [u8]) -> bool {
    let mut n: u32 = 0;
    move |b: &mut [u8]| {
        n += 1;
        let block = hmac_sha256(&[seed], &n.to_le_bytes());
        b.copy_from_slice(&block[..b.len()]);
        true
    }
}

fn pair(pw_ap: &[u8], pw_sta: &[u8]) -> (Sae, Sae) {
    (Sae::new(pw_ap, AP, STA, &mut rng(1)).unwrap(), Sae::new(pw_sta, STA, AP, &mut rng(2)).unwrap())
}

fn parts(b: SaeBody) -> ([u8; 32], [u8; 64]) {
    match b {
        SaeBody::Commit { scalar, element, .. } => (scalar, element),
        _ => panic!("not a commit"),
    }
}

#[test]
fn the_password_element_is_on_the_curve_and_the_same_from_both_sides() {
    let a = password_element(b"correct horse", &AP, &STA).unwrap();
    let b = password_element(b"correct horse", &STA, &AP).unwrap();
    assert_eq!(a, b);
    assert!(p256::point_on_curve(&a));
    assert_ne!(a, password_element(b"correct horsf", &AP, &STA).unwrap());
    assert_ne!(a, password_element(b"correct horse", &AP, &[2, 0, 0, 0, 0, 10]).unwrap());
}

#[test]
fn passwords_of_the_wrong_size_are_refused() {
    assert_eq!(password_element(b"", &AP, &STA), Err(SaeError::BadInput));
    assert_eq!(password_element(&[b'a'; 64], &AP, &STA), Err(SaeError::BadInput));
}

#[test]
fn kdf_has_the_length_asked_for_and_depends_on_every_input() {
    assert_eq!(kdf_sha256(b"k", "l", b"c", 384).len(), 48);
    assert_eq!(kdf_sha256(b"k", "l", b"c", 512).len(), 64);
    // the first 32 bytes of a 512-bit output differ from a 256-bit one: the length is part of the hashed data
    assert_ne!(kdf_sha256(b"k", "l", b"c", 512)[..32], kdf_sha256(b"k", "l", b"c", 256)[..]);
    assert_ne!(kdf_sha256(b"k", "l", b"c", 256), kdf_sha256(b"k", "m", b"c", 256));
    assert_ne!(kdf_sha256(b"k", "l", b"c", 256), kdf_sha256(b"j", "l", b"c", 256));
    // the first block is exactly HMAC(key, 0100 || label || context || 0001 for 256 bits as 0x0100)
    let mut m = vec![1, 0];
    m.extend_from_slice(b"lc");
    m.extend_from_slice(&256u16.to_le_bytes());
    assert_eq!(kdf_sha256(b"k", "l", b"c", 256), hmac_sha256(b"k", &m).to_vec());
}

#[test]
fn both_sides_with_the_same_password_derive_the_same_keys_and_accept_each_other() {
    let (mut ap, mut sta) = pair(b"hunter2hunter2", b"hunter2hunter2");
    let (as_, ae) = parts(ap.commit(&[]));
    let (ss, se) = parts(sta.commit(&[]));
    assert!(crate::sae::commit_bytes_plausible(&as_, &ae) && p256::point_on_curve(&ae));
    let ap_confirm = ap.on_commit(&ss, &se).unwrap();
    let sta_confirm = sta.on_commit(&as_, &ae).unwrap();
    let (c1, v1) = match ap_confirm {
        SaeBody::Confirm { send_confirm, confirm } => (send_confirm, confirm),
        _ => panic!(),
    };
    let (c2, v2) = match sta_confirm {
        SaeBody::Confirm { send_confirm, confirm } => (send_confirm, confirm),
        _ => panic!(),
    };
    assert_eq!((c1, c2), (1, 1));
    let k_sta = sta.on_confirm(c1, &v1).unwrap();
    let k_ap = ap.on_confirm(c2, &v2).unwrap();
    assert_eq!(k_ap, k_sta);
    assert_eq!(ap.state(), SaeState::Accepted);
    assert_ne!(k_ap.pmk, [0; 32]);
}

#[test]
fn another_password_is_noticed_at_the_confirm() {
    let (mut ap, mut sta) = pair(b"right-password", b"wrong-password");
    let (as_, ae) = parts(ap.commit(&[]));
    let (ss, se) = parts(sta.commit(&[]));
    let SaeBody::Confirm { send_confirm, confirm } = sta.on_commit(&as_, &ae).unwrap() else { panic!() };
    ap.on_commit(&ss, &se).unwrap();
    assert_eq!(ap.on_confirm(send_confirm, &confirm), Err(SaeError::WrongPassword));
    assert_ne!(ap.state(), SaeState::Accepted);
}

#[test]
fn a_tampered_confirm_or_counter_is_refused() {
    let (mut ap, mut sta) = pair(b"hunter2hunter2", b"hunter2hunter2");
    let (as_, ae) = parts(ap.commit(&[]));
    let (ss, se) = parts(sta.commit(&[]));
    let SaeBody::Confirm { send_confirm, mut confirm } = sta.on_commit(&as_, &ae).unwrap() else { panic!() };
    ap.on_commit(&ss, &se).unwrap();
    assert_eq!(ap.on_confirm(send_confirm + 1, &confirm), Err(SaeError::WrongPassword));
    assert_eq!(ap.on_confirm(0xffff, &confirm), Err(SaeError::ConfirmExhausted));
    confirm[0] ^= 1;
    assert_eq!(ap.on_confirm(send_confirm, &confirm), Err(SaeError::WrongPassword));
}

#[test]
fn bad_commits_are_refused_and_the_steps_are_in_order() {
    let (mut ap, _) = pair(b"hunter2hunter2", b"hunter2hunter2");
    let (os, oe) = parts(ap.commit(&[]));
    assert_eq!(ap.on_commit(&os, &oe), Err(SaeError::Reflection));
    assert_eq!(ap.on_commit(&[0; 32], &oe), Err(SaeError::BadCommit));
    let mut one = [0u8; 32];
    one[31] = 1;
    assert_eq!(ap.on_commit(&one, &oe), Err(SaeError::BadCommit));
    let mut off = oe;
    off[63] ^= 1;
    let mut two = [0u8; 32];
    two[31] = 2;
    assert_eq!(ap.on_commit(&two, &off), Err(SaeError::BadCommit));
    assert_eq!(ap.on_confirm(1, &[0; 32]), Err(SaeError::OutOfOrder));
    assert!(ap.next_confirm().is_err());
}

#[test]
fn confirm_counters_go_up_and_each_one_is_accepted_by_the_peer() {
    let (mut ap, mut sta) = pair(b"hunter2hunter2", b"hunter2hunter2");
    let (as_, ae) = parts(ap.commit(&[]));
    let (ss, se) = parts(sta.commit(&[]));
    ap.on_commit(&ss, &se).unwrap();
    sta.on_commit(&as_, &ae).unwrap();
    let SaeBody::Confirm { send_confirm: c2, confirm: v2 } = ap.next_confirm().unwrap() else { panic!() };
    assert_eq!(c2, 2);
    assert!(sta.on_confirm(c2, &v2).is_ok());
}

#[test]
fn a_failing_random_source_is_an_error_not_a_weak_key() {
    assert_eq!(Sae::new(b"hunter2hunter2", AP, STA, &mut |_b: &mut [u8]| false).err(), Some(SaeError::BadInput));
    // all zero bytes are never in range
    assert_eq!(Sae::new(b"hunter2hunter2", AP, STA, &mut |b: &mut [u8]| { b.fill(0); true }).err(), Some(SaeError::BadInput));
}

#[test]
fn the_anti_clogging_token_binds_to_the_station_and_the_secret() {
    let s = [7u8; 32];
    let t = anti_clogging_token(&s, &STA);
    assert!(anti_clogging_token_ok(&s, &STA, &t));
    assert!(!anti_clogging_token_ok(&s, &AP, &t));
    assert!(!anti_clogging_token_ok(&[8u8; 32], &STA, &t));
    assert!(!anti_clogging_token_ok(&s, &STA, &t[..8]));
}

// ------------------------------------------------------------------------------------ SAE into the four-way handshake

mod handshake {
    use super::*;
    use crate::eapol::{self, Akm, KeyFrame};
    use crate::fake_supplicant::Supplicant;
    use crate::handshake::{Action, AuthConfig, Authenticator, FailReason, State};
    use crate::rsn::{rsn_ie, SecurityMode};

    fn keys(pw_ap: &[u8], pw_sta: &[u8]) -> (SaeKeys, SaeKeys) {
        let (mut ap, mut sta) = pair(pw_ap, pw_sta);
        let (as_, ae) = parts(ap.commit(&[]));
        let (ss, se) = parts(sta.commit(&[]));
        let ap_c = ap.on_commit(&ss, &se).unwrap();
        let sta_c = sta.on_commit(&as_, &ae).unwrap();
        let (SaeBody::Confirm { send_confirm: c1, confirm: v1 }, SaeBody::Confirm { send_confirm: c2, confirm: v2 }) = (ap_c, sta_c) else { panic!() };
        let ka = ap.on_confirm(c2, &v2);
        let ks = sta.on_confirm(c1, &v1);
        match (ka, ks) {
            (Ok(a), Ok(s)) => (a, s),
            _ => panic!("the passwords differ"),
        }
    }

    fn sent(actions: &[Action]) -> Vec<Vec<u8>> {
        actions.iter().filter_map(|a| if let Action::Send(b) = a { Some(b.clone()) } else { None }).collect()
    }

    fn cfg(k: &SaeKeys, igtk: bool) -> AuthConfig {
        AuthConfig {
            pmk: k.pmk,
            aa: AP,
            spa: STA,
            anonce: [0x11; 32],
            gtk: [0x77; 16],
            gtk_id: 1,
            rsn_ie: rsn_ie(SecurityMode::Wpa3),
            first_replay: 1,
            akm: Akm::Sae,
            igtk: igtk.then_some((4, [0, 0, 0, 0, 0, 1], [0x5a; 16])),
            pmkid: Some(k.pmkid),
        }
    }

    #[test]
    fn a_wpa3_client_finishes_the_handshake_with_the_sae_keys_the_igtk_and_the_pmkid() {
        let (ka, ks) = keys(b"hunter2hunter2", b"hunter2hunter2");
        let beacon = rsn_ie(SecurityMode::Wpa3);
        let mut ap = Authenticator::new(cfg(&ka, true));
        let mut cl = Supplicant::new_sae(ks.pmk, AP, STA, [0x22; 32], beacon.clone(), beacon);
        let m1 = sent(&ap.start(0)).remove(0);
        assert_eq!(KeyFrame::decode(&m1).unwrap().key_info & 7, 3, "key descriptor version 3");
        let m2 = cl.on_frame(&m1).expect("m2");
        assert_eq!(cl.pmkid_seen(), Some(ka.pmkid));
        let acts = ap.on_frame(&m2, 10);
        let m3 = sent(&acts).remove(0);
        let m4 = cl.on_frame(&m3).expect("m4");
        let acts = ap.on_frame(&m4, 20);
        assert_eq!(ap.state(), State::Done);
        let Action::InstallPtk { tk, .. } = &acts[0] else { panic!("no key") };
        assert_eq!(*tk, cl.installed().unwrap().0);
        assert_eq!(cl.igtk(), Some((4, [0x5a; 16])));
        assert!(cl.saw_rsn_ie_match());
        // the group key exchange after it uses the same keys and the CMAC MIC
        let mut rk = ap.group_rekey([0x99; 16], 2).unwrap();
        let g1 = sent_rk(&rk.start(30)).remove(0);
        let g2 = cl.on_frame(&g1).expect("group message 2");
        assert_eq!(rk.on_frame(&g2), vec![crate::groupkey::RekeyAction::Confirmed]);
        assert_eq!(cl.installed().unwrap().1, [0x99; 16]);
    }

    fn sent_rk(a: &[crate::groupkey::RekeyAction]) -> Vec<Vec<u8>> {
        a.iter().filter_map(|x| if let crate::groupkey::RekeyAction::Send(b) = x { Some(b.clone()) } else { None }).collect()
    }

    #[test]
    fn without_protected_management_frames_message_3_has_no_igtk() {
        let (ka, ks) = keys(b"hunter2hunter2", b"hunter2hunter2");
        let beacon = rsn_ie(SecurityMode::Wpa3);
        let mut ap = Authenticator::new(cfg(&ka, false));
        let mut cl = Supplicant::new_sae(ks.pmk, AP, STA, [0x22; 32], beacon.clone(), beacon);
        let m1 = sent(&ap.start(0)).remove(0);
        let m2 = cl.on_frame(&m1).unwrap();
        let m3 = sent(&ap.on_frame(&m2, 10)).remove(0);
        cl.on_frame(&m3).unwrap();
        assert_eq!(cl.igtk(), None);
    }

    #[test]
    fn a_client_whose_element_differs_from_the_beacon_is_refused_as_a_downgrade() {
        let (ka, ks) = keys(b"hunter2hunter2", b"hunter2hunter2");
        let beacon = rsn_ie(SecurityMode::Wpa3);
        let mut ap = Authenticator::new(cfg(&ka, true));
        let mut cl = Supplicant::new_sae(ks.pmk, AP, STA, [0x22; 32], rsn_ie(SecurityMode::Transition), beacon);
        let m1 = sent(&ap.start(0)).remove(0);
        let m2 = cl.on_frame(&m1).unwrap();
        assert_eq!(ap.on_frame(&m2, 10), vec![Action::Failed(FailReason::BadRsnIe)]);
    }

    #[test]
    fn a_pre_shared_key_frame_is_not_taken_for_an_sae_one() {
        // a WPA2 client (version 2, HMAC-SHA1) sending message 2 to a WPA3 access point is ignored, not authorized
        let (ka, ks) = keys(b"hunter2hunter2", b"hunter2hunter2");
        let mut ap = Authenticator::new(cfg(&ka, true));
        let mut cl = Supplicant::new(ks.pmk, AP, STA, [0x22; 32], eapol::rsn_ie_ccmp_psk());
        let m1 = sent(&ap.start(0)).remove(0);
        let m2 = cl.on_frame(&m1).unwrap();
        assert!(ap.on_frame(&m2, 10).is_empty());
        assert_eq!(ap.state(), State::WaitM2);
    }

    #[test]
    fn the_pmkid_entry_of_a_client_is_cut_off_and_checked() {
        let beacon = rsn_ie(SecurityMode::Wpa3);
        let mut with = beacon.clone();
        with.truncate(with.len() - 6);
        with.extend_from_slice(&[1, 0]);
        with.extend_from_slice(&[0xab; 16]);
        with.extend_from_slice(&beacon[beacon.len() - 4..]);
        with[1] = (with.len() - 2) as u8;
        let (bare, id) = eapol::split_rsn_pmkid(&with);
        assert_eq!(bare, beacon);
        assert_eq!(id, Some([0xab; 16]));
        assert_eq!(eapol::split_rsn_pmkid(&beacon), (beacon.clone(), None));
        assert_eq!(eapol::split_rsn_pmkid(&[0x30, 2, 1]), (vec![0x30, 2, 1], None));
    }
}
