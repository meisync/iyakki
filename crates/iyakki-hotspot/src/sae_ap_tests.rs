// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::sae::{parse_sa_query, parse_sae};
use crate::sae_exchange::Sae;
use iyakki_webcrypto::kdf::hmac_sha256;

const AP: Mac = [2, 0, 0, 0, 0, 1];
const STA: Mac = [2, 0, 0, 0, 0, 9];

fn rng(seed: u8) -> impl FnMut(&mut [u8]) -> bool {
    let mut n: u32 = 0;
    move |b: &mut [u8]| {
        n += 1;
        b.copy_from_slice(&hmac_sha256(&[seed], &n.to_le_bytes())[..b.len()]);
        true
    }
}

/// A frame the station sends: the same layout as the access point's with the addresses the other way round.
fn from_sta(body: &SaeBody) -> SaeFrame {
    let mut f = encode_sae(STA, AP, 0, body);
    f[4..10].copy_from_slice(&AP);
    f[10..16].copy_from_slice(&STA);
    parse_sae(&f, AP).unwrap()
}

fn to_sta(b: &[u8]) -> SaeBody {
    parse_sae(&{
        let mut f = b.to_vec();
        // an access point frame is addressed to the station; read it as if it came to the access point
        f[4..10].copy_from_slice(&AP);
        f[10..16].copy_from_slice(&STA);
        f
    }, AP)
    .unwrap()
    .body
}

fn frames(a: &[SaeAction]) -> Vec<Vec<u8>> {
    a.iter().filter_map(|x| if let SaeAction::Send(b) = x { Some(b.clone()) } else { None }).collect()
}

#[test]
fn a_station_with_the_password_gets_its_keys_after_commit_and_confirm() {
    let mut ap = SaeAp::new(b"hunter2hunter2", AP, STA, [1; 32]);
    let mut sta = Sae::new(b"hunter2hunter2", STA, AP, &mut rng(5)).unwrap();
    let acts = ap.on_frame(&from_sta(&sta.commit(&[])), false, &mut rng(6));
    let out = frames(&acts);
    assert_eq!(out.len(), 2, "commit then confirm");
    let SaeBody::Commit { scalar, element, .. } = to_sta(&out[0]) else { panic!() };
    let SaeBody::Confirm { send_confirm, confirm } = to_sta(&out[1]) else { panic!() };
    let SaeBody::Confirm { send_confirm: sc, confirm: sv } = sta.on_commit(&scalar, &element).unwrap() else { panic!() };
    sta.on_confirm(send_confirm, &confirm).unwrap();
    let acts = ap.on_frame(&from_sta(&SaeBody::Confirm { send_confirm: sc, confirm: sv }), false, &mut rng(7));
    assert!(matches!(&acts[..], [SaeAction::Keys(_)]));
    assert!(ap.is_done());
    // nothing more is taken after it
    assert!(ap.on_frame(&from_sta(&sta.commit(&[])), false, &mut rng(8)).is_empty());
}

#[test]
fn a_station_with_another_password_fails_at_its_confirm() {
    let mut ap = SaeAp::new(b"right-password", AP, STA, [1; 32]);
    let mut sta = Sae::new(b"wrong-password", STA, AP, &mut rng(5)).unwrap();
    let out = frames(&ap.on_frame(&from_sta(&sta.commit(&[])), false, &mut rng(6)));
    let SaeBody::Commit { scalar, element, .. } = to_sta(&out[0]) else { panic!() };
    let c = sta.on_commit(&scalar, &element).unwrap();
    let acts = ap.on_frame(&from_sta(&c), false, &mut rng(7));
    assert_eq!(acts, vec![SaeAction::Failed(SaeError::WrongPassword)]);
    assert!(!ap.is_done());
}

#[test]
fn a_busy_access_point_asks_for_a_token_and_accepts_the_commit_that_returns_it() {
    let mut ap = SaeAp::new(b"hunter2hunter2", AP, STA, [1; 32]);
    let sta = Sae::new(b"hunter2hunter2", STA, AP, &mut rng(5)).unwrap();
    let acts = ap.on_frame(&from_sta(&sta.commit(&[])), true, &mut rng(6));
    let out = frames(&acts);
    assert_eq!(out.len(), 1);
    let SaeBody::TokenRequest { token, .. } = to_sta(&out[0]) else { panic!("a token request") };
    // a wrong token is asked again
    assert_eq!(frames(&ap.on_frame(&from_sta(&sta.commit(&[0; 16])), true, &mut rng(6))).len(), 1);
    let acts = ap.on_frame(&from_sta(&sta.commit(&token)), true, &mut rng(6));
    assert_eq!(frames(&acts).len(), 2);
}

#[test]
fn another_group_is_answered_with_status_77_and_other_stations_are_ignored() {
    let mut ap = SaeAp::new(b"hunter2hunter2", AP, STA, [1; 32]);
    let sta = Sae::new(b"hunter2hunter2", STA, AP, &mut rng(5)).unwrap();
    let SaeBody::Commit { scalar, element, token, .. } = sta.commit(&[]) else { panic!() };
    let mut f = from_sta(&SaeBody::Commit { group: GROUP_P256, token, scalar, element });
    f.body = SaeBody::Commit { group: 20, token: vec![], scalar, element };
    let out = frames(&ap.on_frame(&f, false, &mut rng(6)));
    assert_eq!(parse_sae_status(&out[0]), STATUS_SAE_UNSUPPORTED_GROUP);
    let mut other = from_sta(&sta.commit(&[]));
    other.from = [2, 0, 0, 0, 0, 77];
    assert!(ap.on_frame(&other, false, &mut rng(6)).is_empty());
}

fn parse_sae_status(f: &[u8]) -> u16 {
    u16::from_le_bytes([f[24 + 4], f[24 + 5]])
}

#[test]
fn a_repeated_commit_gets_a_fresh_confirm_and_a_confirm_before_any_commit_is_ignored() {
    let mut ap = SaeAp::new(b"hunter2hunter2", AP, STA, [1; 32]);
    let sta = Sae::new(b"hunter2hunter2", STA, AP, &mut rng(5)).unwrap();
    assert!(ap.on_frame(&from_sta(&SaeBody::Confirm { send_confirm: 1, confirm: [0; 32] }), false, &mut rng(6)).is_empty());
    ap.on_frame(&from_sta(&sta.commit(&[])), false, &mut rng(6));
    let again = frames(&ap.on_frame(&from_sta(&sta.commit(&[])), false, &mut rng(6)));
    let SaeBody::Confirm { send_confirm, .. } = to_sta(&again[0]) else { panic!() };
    assert_eq!(send_confirm, 2);
}

#[test]
fn the_sa_query_is_repeated_answered_or_times_out() {
    let (mut t, first) = SaQueryTimer::start(AP, STA, 0x1234, 0);
    let SaQueryAction::Send(f) = first else { panic!() };
    assert_eq!(parse_sa_query(&f, AP).unwrap().transaction, 0x1234);
    assert_eq!(t.tick(100), None);
    assert!(matches!(t.tick(200), Some(SaQueryAction::Send(_))));
    // a response with another number, or from another station, does not count
    let wrong = SaQuery { from: STA, to: AP, response: true, transaction: 1 };
    assert_eq!(t.on_response(&wrong), None);
    assert_eq!(t.on_response(&SaQuery { from: [9; 6], ..SaQuery { transaction: 0x1234, ..wrong } }), None);
    assert_eq!(t.on_response(&SaQuery { transaction: 0x1234, ..wrong }), Some(SaQueryAction::KeepAssociation));
    assert_eq!(t.tick(5000), None);
    let (mut t2, _) = SaQueryTimer::start(AP, STA, 7, 0);
    assert_eq!(t2.tick(1000), Some(SaQueryAction::AcceptNew));
    assert_eq!(t2.tick(2000), None);
}
