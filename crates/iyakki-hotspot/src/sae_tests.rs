// SPDX-License-Identifier: Apache-2.0
use super::*;

const AP: Mac = [2, 0, 0, 0, 0, 1];
const STA: Mac = [2, 0, 0, 0, 0, 9];

fn to_ap(body: &SaeBody, status: u16) -> Vec<u8> {
    // a station frame: the same layout with the addresses the other way round
    let mut f = encode_sae(STA, AP, status, body);
    f[4..10].copy_from_slice(&AP);
    f[10..16].copy_from_slice(&STA);
    f
}

fn commit(token: &[u8]) -> SaeBody {
    SaeBody::Commit { group: GROUP_P256, token: token.to_vec(), scalar: [7; SCALAR_LEN], element: [9; ELEMENT_LEN] }
}

#[test]
fn a_commit_with_and_without_a_token_round_trips() {
    for t in [&[][..], &[1, 2, 3, 4][..]] {
        let f = parse_sae(&to_ap(&commit(t), 0), AP).unwrap();
        assert_eq!(f.body, commit(t));
        assert_eq!((f.from, f.to), (STA, AP));
    }
    assert!(parse_sae(&to_ap(&commit(&[]), STATUS_SAE_HASH_TO_ELEMENT), AP).is_some());
}

#[test]
fn a_confirm_round_trips_and_has_to_be_the_exact_size() {
    let c = SaeBody::Confirm { send_confirm: 1, confirm: [5; CONFIRM_LEN] };
    assert_eq!(parse_sae(&to_ap(&c, 0), AP).unwrap().body, c);
    let mut f = to_ap(&c, 0);
    f.push(0);
    assert!(parse_sae(&f, AP).is_none());
    f.truncate(f.len() - 3);
    assert!(parse_sae(&f, AP).is_none());
}

#[test]
fn the_token_request_and_the_group_refusal_carry_their_status() {
    let t = SaeBody::TokenRequest { group: GROUP_P256, token: vec![0xaa; 16] };
    let f = encode_sae(STA, AP, 0, &t);
    assert_eq!(u16::from_le_bytes([f[HEADER_LEN + 4], f[HEADER_LEN + 5]]), STATUS_SAE_ANTI_CLOGGING);
    let mut g = f.clone();
    g[4..10].copy_from_slice(&AP);
    g[10..16].copy_from_slice(&STA);
    assert_eq!(parse_sae(&g, AP).unwrap().body, t);
    let r = SaeBody::Rejected { status: STATUS_SAE_UNSUPPORTED_GROUP };
    assert_eq!(parse_sae(&to_ap(&r, 0), AP).unwrap().body, r);
}

#[test]
fn another_group_algorithm_address_or_size_is_refused() {
    let mut other = to_ap(&commit(&[]), 0);
    other[HEADER_LEN + 6] = 20;
    assert!(parse_sae(&other, AP).is_none(), "group 20 is not offered");
    let mut open = to_ap(&commit(&[]), 0);
    open[HEADER_LEN] = 0;
    assert!(parse_sae(&open, AP).is_none(), "not SAE");
    assert!(parse_sae(&to_ap(&commit(&[]), 0), [2, 0, 0, 0, 0, 2]).is_none(), "another bssid");
    assert!(parse_sae(&to_ap(&commit(&vec![1; TOKEN_MAX + 1]), 0), AP).is_none(), "token too long");
    for n in 0..HEADER_LEN + 6 + 2 + SCALAR_LEN + ELEMENT_LEN {
        let f = to_ap(&commit(&[]), 0);
        if n < f.len() {
            assert!(parse_sae(&f[..n], AP).is_none(), "cut at {n}");
        }
    }
}

#[test]
fn hostile_bytes_never_panic() {
    let mut x = 12345u32;
    for _ in 0..2000 {
        let n = (x % 120) as usize;
        let mut f = Vec::new();
        for _ in 0..n {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            f.push((x >> 24) as u8);
        }
        if f.len() >= 16 {
            f[0] = 0xb0;
            let at = HEADER_LEN.min(f.len() - 1);
            f[at] = 3;
        }
        let _ = parse_sae(&f, AP);
        let _ = parse_sa_query(&f, AP);
        let _ = parse_igtk_kde(&f);
    }
}

#[test]
fn a_scalar_of_zero_or_one_or_not_below_the_order_and_a_big_coordinate_are_refused() {
    let mut s = [0u8; 32];
    let e = [1u8; 64];
    assert!(!commit_bytes_plausible(&s, &e));
    s[31] = 1;
    assert!(!commit_bytes_plausible(&s, &e));
    s[31] = 2;
    assert!(commit_bytes_plausible(&s, &e));
    assert!(!commit_bytes_plausible(&[0xff; 32], &e));
    let mut big = e;
    big[..32].copy_from_slice(&[0xff; 32]);
    assert!(!commit_bytes_plausible(&s, &big));
    let mut big = e;
    big[32..].copy_from_slice(&[0xff; 32]);
    assert!(!commit_bytes_plausible(&s, &big));
}

#[test]
fn an_sa_query_request_is_answered_by_the_same_transaction_number_only() {
    let req = encode_sa_query(STA, AP, false, 0x1234);
    let q = parse_sa_query(&req, AP).unwrap();
    assert_eq!((q.response, q.transaction, q.to), (false, 0x1234, STA));
    let mut resp = encode_sa_query(AP, AP, true, 0x1234);
    resp[10..16].copy_from_slice(&STA);
    let r = parse_sa_query(&resp, AP).unwrap();
    assert!(sa_query_answers(0x1234, &r));
    assert!(!sa_query_answers(0x1235, &r));
    assert!(!sa_query_answers(0x1234, &q), "a request is not an answer");
    let mut bad = req.clone();
    bad[HEADER_LEN] = 9;
    assert!(parse_sa_query(&bad, AP).is_none());
    bad = req;
    bad.push(0);
    assert!(parse_sa_query(&bad, AP).is_none());
}

#[test]
fn the_igtk_item_has_the_standard_layout_and_round_trips() {
    let k = igtk_kde(4, [1, 2, 3, 4, 5, 6], &[0x11; 16]);
    assert_eq!(&k[..8], &[0xdd, 28, 0x00, 0x0f, 0xac, 9, 4, 0]);
    assert_eq!(k.len(), 30);
    assert_eq!(parse_igtk_kde(&k), Some((4, [1, 2, 3, 4, 5, 6], [0x11; 16])));
    assert!(parse_igtk_kde(&igtk_kde(3, [0; 6], &[0; 16])).is_none(), "key ids of an IGTK are 4 and 5");
    let mut short = k.clone();
    short.pop();
    assert!(parse_igtk_kde(&short).is_none());
}
