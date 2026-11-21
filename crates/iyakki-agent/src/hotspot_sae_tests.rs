// SPDX-License-Identifier: Apache-2.0
use super::*;
use iyakki_hotspot::handshake::Authenticator;
use iyakki_hotspot::nl80211::{attr, cmd, split_messages, attrs, Msg, GENL_ID_CTRL, CTRL_ATTR_FAMILY_ID, NLMSG_ERROR};
use iyakki_hotspot::sae_exchange::Sae;
use iyakki_webcrypto::kdf::hmac_sha256;

const AP: Mac = [2, 0, 0, 0, 0, 1];
const STA: Mac = [2, 0, 0, 0, 0, 9];
const STA2: Mac = [2, 0, 0, 0, 0, 10];
const FAM: u16 = 0x1c;
const PASSWORD: &[u8] = b"hunter2hunter2";

fn rng(seed: u8) -> impl FnMut(&mut [u8]) -> bool {
    let mut n: u32 = 0;
    move |b: &mut [u8]| {
        n += 1;
        b.copy_from_slice(&hmac_sha256(&[seed], &n.to_le_bytes())[..b.len()]);
        true
    }
}

/// What the kernel would hand the program: a station's frame to the access point.
fn from(sta: Mac, body: &SaeBody) -> Vec<u8> {
    let mut f = encode_sae(sta, AP, 0, body);
    f[4..10].copy_from_slice(&AP);
    f[10..16].copy_from_slice(&sta);
    f
}

/// What a station reads out of a frame the access point sent it.
fn to_station(b: &[u8], sta: Mac) -> SaeBody {
    let mut f = b.to_vec();
    f[4..10].copy_from_slice(&AP);
    f[10..16].copy_from_slice(&sta);
    parse_sae(&f, AP).unwrap().body
}

/// A netlink transport that records every request and acknowledges it (or refuses).
struct Fake {
    sent: Vec<Vec<u8>>,
    errno: i32,
}

impl NetlinkTransport for Fake {
    fn exchange(&mut self, req: &[u8]) -> Result<Vec<u8>, NlError> {
        self.sent.push(req.to_vec());
        let m = split_messages(req).unwrap().remove(0);
        if m.ty == GENL_ID_CTRL {
            return Ok(Msg::new(GENL_ID_CTRL, 2, 1, 1).u16(CTRL_ATTR_FAMILY_ID, FAM).finish());
        }
        let mut ack = Msg::new(NLMSG_ERROR, 0, m.seq, 0).finish();
        ack[16..20].copy_from_slice(&(-self.errno).to_le_bytes());
        Ok(ack)
    }
}

fn radio() -> SaeRadio {
    SaeRadio::new(AP, PASSWORD, [1; 32])
}

/// The authentication frames that went out as `NL80211_CMD_FRAME`, with the interface and frequency they went on.
fn sent_frames(c: &ApControl<Fake>) -> Vec<(u32, u32, Vec<u8>)> {
    c.transport()
        .sent
        .iter()
        .filter_map(|b| {
            let m = &split_messages(b).unwrap()[0];
            if m.ty != FAM || m.payload[0] != cmd::FRAME {
                return None;
            }
            let a = attrs(&m.payload[4..]).unwrap();
            let get = |t: u16| a.iter().find(|(x, _)| *x == t).map(|(_, v)| v.to_vec()).unwrap();
            Some((u32::from_le_bytes(get(attr::IFINDEX).try_into().unwrap()), u32::from_le_bytes(get(attr::WIPHY_FREQ).try_into().unwrap()), get(attr::FRAME)))
        })
        .collect()
}

#[test]
fn a_wpa3_station_finishes_the_exchange_through_the_kernels_frame_calls_and_the_keys_start_a_wpa3_handshake() {
    let mut r = radio();
    let mut nl = ApControl::new(Fake { sent: vec![], errno: 0 });
    let mut client = Sae::new(PASSWORD, STA, AP, &mut rng(5)).unwrap();
    // the station's commit arrives; the access point's commit and confirm go out through the kernel on the right interface and channel
    let steps = r.on_auth_frame(&from(STA, &client.commit(&[])), 1000, &mut rng(6));
    let d = deliver(&mut nl, 7, 2437, steps);
    assert!(d.keys.is_empty() && d.failed.is_empty());
    let out = sent_frames(&nl);
    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|(i, f, _)| *i == 7 && *f == 2437));
    let SaeBody::Commit { scalar, element, .. } = to_station(&out[0].2, STA) else { panic!("commit first") };
    let SaeBody::Confirm { send_confirm, confirm } = to_station(&out[1].2, STA) else { panic!("then confirm") };
    let mine = client.on_commit(&scalar, &element).unwrap();
    client.on_confirm(send_confirm, &confirm).unwrap();
    // the station's confirm ends it: the keys come out, equal to the station's own, and nothing is kept
    let steps = r.on_auth_frame(&from(STA, &mine), 1100, &mut rng(7));
    let d = deliver(&mut nl, 7, 2437, steps);
    assert_eq!(d.keys.len(), 1);
    assert_eq!(r.in_progress(), 0);
    let (sta, keys) = &d.keys[0];
    assert_eq!(*sta, STA);
    // the four-way handshake then starts as WPA3: protected management frames announced, message 1 carries the PMKID
    let cfg = auth_config(keys, AP, STA, [3; 32], [4; 16], (4, [0; 6], [5; 16]), SecurityMode::Wpa3);
    assert_eq!((cfg.akm, cfg.pmkid, cfg.igtk.is_some()), (Akm::Sae, Some(keys.pmkid), true));
    assert_eq!(cfg.rsn_ie, rsn_ie(SecurityMode::Wpa3));
    let mut auth = Authenticator::new(cfg);
    let first = auth.start(0);
    assert!(!first.is_empty());
    assert!(first.iter().any(|a| matches!(a, iyakki_hotspot::handshake::Action::Send(b) if b.windows(16).any(|w| w == keys.pmkid))), "message 1 names the PMKID of the exchange");
}

#[test]
fn a_wrong_password_is_refused_with_a_frame_and_leaves_no_state() {
    let mut r = SaeRadio::new(AP, b"right-password", [1; 32]);
    let mut client = Sae::new(b"wrong-password", STA, AP, &mut rng(5)).unwrap();
    let steps = r.on_auth_frame(&from(STA, &client.commit(&[])), 0, &mut rng(6));
    let frames: Vec<&Vec<u8>> = steps.iter().filter_map(|s| if let SaeStep::Frame(b) = s { Some(b) } else { None }).collect();
    let SaeBody::Commit { scalar, element, .. } = to_station(frames[0], STA) else { panic!() };
    let mine = client.on_commit(&scalar, &element).unwrap();
    let steps = r.on_auth_frame(&from(STA, &mine), 10, &mut rng(7));
    assert!(matches!(&steps[..], [SaeStep::Frame(_), SaeStep::Failed { sta, error: SaeError::WrongPassword }] if *sta == STA), "{steps:?}");
    let SaeStep::Frame(b) = &steps[0] else { panic!() };
    // algorithm 3, sequence 1, status 1
    assert_eq!((&b[24..26], &b[26..28], &b[28..30]), (&3u16.to_le_bytes()[..], &1u16.to_le_bytes()[..], &STATUS_FAILURE.to_le_bytes()[..]));
    assert_eq!(r.in_progress(), 0);
}

#[test]
fn frames_that_are_not_for_this_exchange_do_nothing_and_never_grow_the_state() {
    let mut r = radio();
    let client = Sae::new(PASSWORD, STA, AP, &mut rng(5)).unwrap();
    let good = from(STA, &client.commit(&[]));
    // open authentication (algorithm 0), garbage and every truncation of a good frame
    let mut open = good.clone();
    open[24..26].copy_from_slice(&0u16.to_le_bytes());
    assert!(!SaeRadio::is_sae(&open) && r.on_auth_frame(&open, 0, &mut rng(6)).is_empty());
    assert!(r.on_auth_frame(&[], 0, &mut rng(6)).is_empty() && r.on_auth_frame(&[0xb0; 30], 0, &mut rng(6)).is_empty());
    for n in 0..good.len() {
        assert!(r.on_auth_frame(&good[..n], 0, &mut rng(6)).is_empty(), "a frame cut at {n}");
    }
    // a frame for another access point
    let mut other = good.clone();
    other[16..22].copy_from_slice(&[9; 6]);
    assert!(r.on_auth_frame(&other, 0, &mut rng(6)).is_empty());
    // a confirm from a station that never sent a commit
    assert!(r.on_auth_frame(&from(STA2, &SaeBody::Confirm { send_confirm: 1, confirm: [0; 32] }), 0, &mut rng(6)).is_empty());
    assert_eq!(r.in_progress(), 0);
}

#[test]
fn only_so_many_exchanges_run_at_once_and_old_ones_are_forgotten() {
    let mut r = radio();
    let mut next = 0u8;
    let mut start = |r: &mut SaeRadio, now: u64| {
        next += 1;
        let sta: Mac = [2, 0, 0, 0, 1, next];
        let c = Sae::new(PASSWORD, sta, AP, &mut rng(next)).unwrap();
        r.on_auth_frame(&from(sta, &c.commit(&[])), now, &mut rng(6))
    };
    // below the busy mark every commit is answered with commit and confirm (the token comes with a busy radio)
    for _ in 0..BUSY_AT {
        assert_eq!(start(&mut r, 0).len(), 2);
    }
    assert!(r.busy());
    // busy: the next commit without a token gets a token request and no state
    let steps = start(&mut r, 0);
    assert_eq!(steps.len(), 1);
    assert_eq!(r.in_progress(), BUSY_AT);
    // a station with a token is let in up to the cap; past it nothing is answered
    let mut kept = BUSY_AT;
    while kept < MAX_EXCHANGES {
        let sta: Mac = [2, 0, 0, 0, 2, kept as u8];
        let c = Sae::new(PASSWORD, sta, AP, &mut rng(100 + kept as u8)).unwrap();
        let tok = iyakki_hotspot::sae_exchange::anti_clogging_token(&[1; 32], &sta);
        assert_eq!(r.on_auth_frame(&from(sta, &c.commit(&tok)), 0, &mut rng(6)).len(), 2);
        kept += 1;
    }
    assert_eq!(r.in_progress(), MAX_EXCHANGES);
    let sta: Mac = [2, 0, 0, 0, 3, 1];
    let c = Sae::new(PASSWORD, sta, AP, &mut rng(1)).unwrap();
    let tok = iyakki_hotspot::sae_exchange::anti_clogging_token(&[1; 32], &sta);
    assert!(r.on_auth_frame(&from(sta, &c.commit(&tok)), 0, &mut rng(6)).is_empty(), "past the cap nothing is started");
    // the exchanges that never finish are forgotten after their time; a gone station is forgotten at once
    r.forget(&[2, 0, 0, 0, 1, 1]);
    assert_eq!(r.in_progress(), MAX_EXCHANGES - 1);
    r.tick(EXCHANGE_TIMEOUT_MS - 1);
    assert_eq!(r.in_progress(), MAX_EXCHANGES - 1);
    r.tick(EXCHANGE_TIMEOUT_MS);
    assert_eq!(r.in_progress(), 0);
    // a new secret makes the old tokens useless
    r.rotate_secret([9; 32]);
    assert!(r.on_auth_frame(&from(sta, &c.commit(&tok)), EXCHANGE_TIMEOUT_MS, &mut rng(6)).len() == 2, "not busy any more: no token needed");
}

#[test]
fn a_frame_the_kernel_refuses_is_reported_and_the_keys_still_come_through() {
    let mut nl = ApControl::new(Fake { sent: vec![], errno: 22 });
    let keys = SaeKeys { pmk: [1; 32], pmkid: [2; 16] };
    let d = deliver(&mut nl, 7, 2437, vec![SaeStep::Frame(vec![1, 2, 3]), SaeStep::Keys { sta: STA, keys: keys.clone() }, SaeStep::Failed { sta: STA2, error: SaeError::WrongPassword }]);
    assert_eq!(d.failed.len(), 1);
    assert!(d.failed[0].contains("sending the authentication frame failed"), "{:?}", d.failed);
    assert_eq!(d.keys, vec![(STA, keys)]);
}

#[test]
fn the_registration_and_the_frame_call_are_the_kernels_own_messages() {
    let mut nl = ApControl::new(Fake { sent: vec![], errno: 0 });
    nl.register_mgmt(7, iyakki_hotspot::nl80211::FRAME_AUTH).unwrap();
    nl.register_mgmt(7, iyakki_hotspot::nl80211::FRAME_ASSOC_REQ).unwrap();
    nl.send_mgmt(7, 5180, &[0xb0, 0]).unwrap();
    let cmds: Vec<u8> = nl.transport().sent.iter().filter_map(|b| { let m = &split_messages(b).unwrap()[0]; (m.ty == FAM).then(|| m.payload[0]) }).collect();
    assert_eq!(cmds, vec![cmd::REGISTER_FRAME, cmd::REGISTER_FRAME, cmd::FRAME]);
    let reg = &split_messages(&nl.transport().sent[1]).unwrap()[0];
    let a = attrs(&reg.payload[4..]).unwrap();
    assert_eq!(a.iter().find(|(t, _)| *t == attr::FRAME_TYPE).unwrap().1, &0x00b0u16.to_le_bytes());
    assert_eq!(sent_frames(&nl), vec![(7, 5180, vec![0xb0, 0])]);
}
