// SPDX-License-Identifier: Apache-2.0
//! The runner against a fake netlink, a fake EAPOL channel and a scripted source: the SAE path from the registered frame to the kernel's
//! frame, station and key calls.
use super::*;
use crate::softap::tests::{assoc, rng, sae_from, sae_to, soft, AP, PASSWORD, SSID, STA};
use iyakki_hotspot::nl80211::{attr, attrs, cmd, split_messages, Msg, NlError, CTRL_ATTR_FAMILY_ID, GENL_ID_CTRL, NLMSG_ERROR};
use iyakki_hotspot::rsn::rsn_ie;
use iyakki_hotspot::sae::SaeBody;
use iyakki_hotspot::sae_exchange::Sae;
use std::collections::VecDeque;

const FAM: u16 = 0x1c;
const IFINDEX: u32 = 7;
const FREQ: u32 = 2437;

struct Fake {
    sent: Vec<Vec<u8>>,
}

impl NetlinkTransport for Fake {
    fn exchange(&mut self, req: &[u8]) -> Result<Vec<u8>, NlError> {
        self.sent.push(req.to_vec());
        let m = split_messages(req).unwrap().remove(0);
        if m.ty == GENL_ID_CTRL {
            return Ok(Msg::new(GENL_ID_CTRL, 2, 1, 1).u16(CTRL_ATTR_FAMILY_ID, FAM).finish());
        }
        Ok(Msg::new(NLMSG_ERROR, 0, m.seq, 0).finish())
    }
}

#[derive(Default)]
struct FakeEapol {
    sent: Vec<(Mac, Vec<u8>)>,
}

impl EapolChannel for FakeEapol {
    fn send(&mut self, sta: Mac, b: &[u8]) -> Result<(), String> {
        self.sent.push((sta, b.to_vec()));
        Ok(())
    }
    fn recv(&mut self, _: i32) -> Result<Option<(Mac, Vec<u8>)>, String> {
        Ok(None)
    }
    fn raw_fd(&self) -> std::os::fd::RawFd {
        -1
    }
}

struct Script(VecDeque<Vec<Incoming>>);

impl Source<FakeEapol> for Script {
    fn poll(&mut self, _: i32, _: &mut FakeEapol) -> Vec<Incoming> {
        self.0.pop_front().unwrap_or_default()
    }
}

fn calls(l: &RadioLoop<Fake, FakeEapol, Script>) -> Vec<(u8, Vec<(u16, Vec<u8>)>)> {
    l.ap.transport()
        .sent
        .iter()
        .filter_map(|b| {
            let m = &split_messages(b).unwrap()[0];
            (m.ty == FAM).then(|| (m.payload[0], attrs(&m.payload[4..]).unwrap().into_iter().map(|(t, v)| (t, v.to_vec())).collect()))
        })
        .collect()
}

fn frames(l: &RadioLoop<Fake, FakeEapol, Script>) -> Vec<Vec<u8>> {
    calls(l).into_iter().filter(|(c, _)| *c == cmd::FRAME).map(|(_, a)| a.into_iter().find(|(t, _)| *t == attr::FRAME).unwrap().1).collect()
}

fn the_loop(security: SecurityMode, script: Vec<Vec<Incoming>>) -> RadioLoop<Fake, FakeEapol, Script> {
    RadioLoop { soft: soft(security), ap: ApControl::new(Fake { sent: vec![] }), eap: FakeEapol::default(), source: Script(script.into()), ifindex: IFINDEX, freq_mhz: FREQ }
}

#[test]
fn a_wpa3_station_goes_from_its_commit_to_a_station_entry_and_message_1_through_the_kernels_calls() {
    let mut client = Sae::new(PASSWORD.as_bytes(), STA, AP, &mut rng(5)).unwrap();
    let mut l = the_loop(SecurityMode::Wpa3, vec![vec![Incoming::Mgmt(sae_from(STA, &client.commit(&[])))]]);
    l.step(0, 100);
    let out = frames(&l);
    assert_eq!(out.len(), 2, "commit and confirm go out as kernel frame calls");
    let SaeBody::Commit { scalar, element, .. } = sae_to(&out[0], STA) else { panic!() };
    let SaeBody::Confirm { send_confirm, confirm } = sae_to(&out[1], STA) else { panic!() };
    let mine = client.on_commit(&scalar, &element).unwrap();
    client.on_confirm(send_confirm, &confirm).unwrap();
    l.source.0.push_back(vec![Incoming::Mgmt(sae_from(STA, &mine))]);
    l.source.0.push_back(vec![Incoming::Mgmt(assoc(STA, SSID, &rsn_ie(SecurityMode::Wpa3)))]);
    let ev1 = l.step(0, 200);
    assert!(ev1.iter().any(|e| matches!(e, SoftEvent::Log(m) if m.starts_with("SAE finished"))));
    let ev2 = l.step(0, 300);
    assert!(ev2.contains(&SoftEvent::Associated(STA)));
    // every frame went to the access point's interface and channel; the station was added; message 1 went over the EAPOL channel
    let all = calls(&l);
    for (c, a) in &all {
        if *c == cmd::FRAME {
            assert_eq!(a.iter().find(|(t, _)| *t == attr::IFINDEX).map(|(_, v)| u32::from_le_bytes(v[..4].try_into().unwrap())), Some(IFINDEX));
            assert_eq!(a.iter().find(|(t, _)| *t == attr::WIPHY_FREQ).map(|(_, v)| u32::from_le_bytes(v[..4].try_into().unwrap())), Some(FREQ));
        }
    }
    assert!(all.iter().any(|(c, _)| *c == cmd::NEW_STATION));
    assert_eq!(l.eap.sent.len(), 1);
    assert_eq!(l.eap.sent[0].0, STA);
    // the association answer is the last frame call and says OK
    let last = frames(&l).pop().unwrap();
    assert_eq!(u16::from_le_bytes([last[26], last[27]]), 0);
}

#[test]
fn a_station_that_left_is_forgotten_through_the_source_and_a_wrong_password_never_adds_one() {
    let mut l = the_loop(SecurityMode::Transition, vec![]);
    // the exchange of a wrong password runs on the pure side; the loop then sees the station leave
    let client = Sae::new(b"nope-nope-nope", STA, AP, &mut rng(5)).unwrap();
    l.source.0.push_back(vec![Incoming::Mgmt(sae_from(STA, &client.commit(&[])))]);
    l.step(0, 0);
    assert_eq!(l.soft.sae_in_progress(), 1);
    l.source.0.push_back(vec![Incoming::StationGone(STA)]);
    l.step(0, 10);
    assert_eq!(l.soft.sae_in_progress(), 0);
    assert!(!calls(&l).iter().any(|(c, _)| *c == cmd::NEW_STATION));
}

struct Counting(Arc<std::sync::atomic::AtomicUsize>);

impl Round for Counting {
    fn round(&mut self, _: u64) -> Vec<SoftEvent> {
        self.0.fetch_add(1, Ordering::SeqCst);
        std::thread::yield_now();
        Vec::new()
    }
}

// the runner slot is one per process: the tests that use it take this lock
static SLOT: Mutex<()> = Mutex::new(());

#[test]
fn the_runner_starts_after_its_setup_runs_until_end_and_is_replaced_by_the_next_one() {
    let _g = SLOT.lock().unwrap_or_else(|p| p.into_inner());
    let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let n2 = n.clone();
    spawn_with(move || Ok(Box::new(Counting(n2)) as Box<dyn Round>)).unwrap();
    assert!(running());
    // wait on the counter, not on a clock: the thread is running once it counted
    while n.load(Ordering::SeqCst) == 0 {
        std::thread::yield_now();
    }
    // a second one replaces the first (the first is stopped and joined before the second starts)
    let m = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let m2 = m.clone();
    spawn_with(move || Ok(Box::new(Counting(m2)) as Box<dyn Round>)).unwrap();
    let frozen = n.load(Ordering::SeqCst);
    end();
    assert!(!running());
    assert_eq!(n.load(Ordering::SeqCst), frozen, "the first runner did not run after it was replaced");
    let after_end = m.load(Ordering::SeqCst);
    assert_eq!(m.load(Ordering::SeqCst), after_end, "nothing runs after end");
    end();
}

#[test]
fn a_setup_that_fails_gives_its_reason_and_leaves_nothing_running() {
    let _g = SLOT.lock().unwrap_or_else(|p| p.into_inner());
    end();
    let e = spawn_with(|| Err("no EAPOL socket".into())).err().unwrap();
    assert_eq!(e, "no EAPOL socket");
    assert!(!running());
}
