//! The tethering session against a fake phone: the set-up exchange, frames both ways, filters, refusals, silence, a phone that hangs
//! up, hostile packets in the middle of a session, and the plain reasons. No radio, no TAP.

use super::*;
use crate::bnep::*;
use std::collections::VecDeque;

const ME: Mac = [0x02, 0, 0, 0, 0, 0x01];
const PHONE: Mac = [0xaa, 0xbb, 0xcc, 0, 0, 0x02];

/// A phone that answers the BNEP set-up with `answer` (`None`: silence) and then echoes what it receives.
struct FakePhone {
    answer: Option<u16>,
    /// what the phone says next (packets) and what the computer's TAP says next (frames)
    from_phone: VecDeque<Vec<u8>>,
    from_tap: VecDeque<Vec<u8>>,
    /// the phone hangs up when this many events were handed out
    close_after: Option<usize>,
    handed: usize,
    // what the computer did
    to_phone: Vec<Vec<u8>>,
    to_tap: Vec<Vec<u8>>,
    stop: Option<Arc<AtomicBool>>,
    /// the person disconnects once nothing is left to say (off for tests that wait for silence or a hang-up)
    autostop: bool,
    /// the computer's frame is seen before the phone's answer
    tap_first: bool,
}

impl FakePhone {
    fn new(answer: Option<u16>) -> FakePhone {
        FakePhone { answer, from_phone: VecDeque::new(), from_tap: VecDeque::new(), close_after: None, handed: 0, to_phone: vec![], to_tap: vec![], stop: None, autostop: true, tap_first: false }
    }
}

impl PanLink for FakePhone {
    fn wait(&mut self, _t: Duration) -> io::Result<LinkEv> {
        self.handed += 1;
        if self.close_after.is_some_and(|n| self.handed > n) {
            return Ok(LinkEv::Closed);
        }
        if self.tap_first {
            if let Some(f) = self.from_tap.pop_front() {
                return Ok(LinkEv::Tap(f));
            }
        }
        if let Some(p) = self.from_phone.pop_front() {
            return Ok(LinkEv::Phone(p));
        }
        if let Some(f) = self.from_tap.pop_front() {
            return Ok(LinkEv::Tap(f));
        }
        // nothing left to say: the person disconnects (or the test ends the session)
        if let (true, Some(s)) = (self.autostop, &self.stop) {
            s.store(true, Ordering::SeqCst);
        }
        std::thread::sleep(Duration::from_millis(2));
        Ok(LinkEv::Idle)
    }

    fn to_phone(&mut self, packet: &[u8]) -> io::Result<()> {
        self.to_phone.push(packet.to_vec());
        // the set-up request is answered the way a phone does
        if packet.first() == Some(&1) && packet.get(1) == Some(&1) {
            if let Some(code) = self.answer {
                self.from_phone.push_front(setup_response(code));
            }
        }
        Ok(())
    }

    fn to_tap(&mut self, frame: &[u8]) -> io::Result<()> {
        self.to_tap.push(frame.to_vec());
        Ok(())
    }
}

fn arp_frame() -> Vec<u8> {
    [&ME[..], &PHONE, &[0x08, 0x06], &[1, 2, 3, 4]].concat()
}

fn session(p: &mut FakePhone, wait_ms: u64) -> (End, bool) {
    let stop = Arc::new(AtomicBool::new(false));
    p.stop = Some(stop.clone());
    let mut came_up = false;
    let end = run(p, ME, PHONE, &stop, Duration::from_millis(wait_ms), &mut || came_up = true);
    (end, came_up)
}

#[test]
fn the_computer_asks_for_the_phones_network_service_as_a_panu_and_brings_the_link_up_on_yes() {
    let mut p = FakePhone::new(Some(0));
    let (end, up) = session(&mut p, 500);
    assert_eq!(end, End::Stopped);
    assert!(up, "DHCP is started when the phone says yes");
    assert_eq!(p.to_phone[0], setup_request(UUID_NAP, UUID_PANU), "NAP is the phone's service, PANU is ours");
}

#[test]
fn frames_go_both_ways_the_computers_as_general_ethernet_and_the_phones_to_the_tap() {
    let mut p = FakePhone::new(Some(0));
    let out = [&PHONE[..], &ME, &[0x08, 0x00], &[9, 9, 9]].concat();
    p.from_tap.push_back(out.clone());
    // the phone sends one frame in the compressed form (no addresses) and one general frame
    p.from_phone.push_back(vec![0x02, 0x08, 0x06, 1, 2, 3, 4]);
    let (end, _) = session(&mut p, 500);
    assert_eq!(end, End::Stopped);
    assert_eq!(p.to_phone[1], [&[0u8][..], &out].concat(), "general Ethernet: type 0, then the frame as it was");
    assert_eq!(p.to_tap, vec![arp_frame()], "the compressed frame got both addresses back");
}

#[test]
fn frames_the_computer_sends_before_the_phone_said_yes_are_dropped() {
    let mut p = FakePhone::new(Some(0));
    p.from_tap.push_back(arp_frame());
    p.tap_first = true; // the answer arrives after the tap frame was seen
    let (end, up) = session(&mut p, 500);
    assert_eq!((end, up), (End::Stopped, true));
    assert_eq!(p.to_phone.len(), 1, "only the set-up request was sent: {:?}", p.to_phone);
}

#[test]
fn the_phones_filter_requests_are_answered_with_success_and_garbage_does_not_end_the_link() {
    let mut p = FakePhone::new(Some(0));
    p.from_phone.push_back(vec![1, 3, 0, 4, 8, 0, 8, 0]); // network type filter
    p.from_phone.push_back(vec![1, 5, 0, 0]); // multicast filter
    p.from_phone.push_back(vec![0xff, 0xff, 0xff]); // nonsense
    p.from_phone.push_back(vec![1, 0x44]); // an unknown control message
    p.from_phone.push_back(vec![]); // empty
    p.from_phone.push_back(vec![2, 8]); // a short frame
    p.from_phone.push_back(vec![0x82, 8, 6, 0x80, 255, 1]); // extension headers that run past the end
    let (end, _) = session(&mut p, 500);
    assert_eq!(end, End::Stopped, "still up after all of it");
    assert!(p.to_phone.contains(&vec![1, 4, 0, 0]), "network filter: success");
    assert!(p.to_phone.contains(&vec![1, 6, 0, 0]), "multicast filter: success");
    assert!(p.to_phone.contains(&not_understood(0x44)));
}

#[test]
fn a_refusal_comes_back_with_its_code_and_tethering_off_has_a_plain_text() {
    let mut p = FakePhone::new(Some(4));
    let (end, up) = session(&mut p, 500);
    assert_eq!(end, End::Refused(4));
    assert!(!up);
    assert!(refusal_text(4).contains("Bluetooth tethering"), "{}", refusal_text(4));
    assert!(refusal_text(1).contains("PAN") && refusal_text(99).contains("99"));
}

#[test]
fn a_silent_phone_ends_with_no_answer_after_the_wait() {
    let mut p = FakePhone::new(None);
    p.autostop = false;
    let (end, up) = session(&mut p, 40);
    assert_eq!((end, up), (End::NoAnswer, false));
    assert!(NO_ANSWER.contains("Bluetooth tethering"));
}

#[test]
fn a_phone_that_hangs_up_before_and_after_the_answer_is_told_apart() {
    let mut early = FakePhone::new(None);
    early.close_after = Some(0);
    early.autostop = false;
    assert_eq!(session(&mut early, 500).0, End::NoAnswer);
    let mut late = FakePhone::new(Some(0));
    late.close_after = Some(3);
    late.autostop = false;
    assert_eq!(session(&mut late, 500), (End::Lost, true));
}

#[test]
fn connect_failures_say_what_to_do() {
    assert!(connect_reason(libc::ECONNREFUSED).contains("Switch on Bluetooth tethering"));
    assert!(connect_reason(libc::EACCES).contains("Forget the phone"));
    assert!(connect_reason(libc::ETIMEDOUT).contains("Unlock"));
    assert!(connect_reason(libc::EHOSTUNREACH).contains("closer"));
    assert!(connect_reason(libc::ENODEV).contains("no Bluetooth controller"));
    assert!(connect_reason(libc::EAFNOSUPPORT).contains("Power the controller"));
    assert!(connect_reason(libc::EIO).starts_with("Could not connect to the phone"));
}

// ---- what stands in the way before any connection is tried

use crate::btview::Controller;

fn ctl(configured: bool, address: &str, powered: bool, bredr: bool) -> Controller {
    Controller { index: 0, configured, address: address.into(), version: Some(9), manufacturer: 15, supported: if bredr { crate::btmgmt::SET_BREDR } else { 0 }, current: if powered { crate::btmgmt::SET_POWERED } else { 0 }, name: "x".into() }
}

fn sw(soft: bool, hard: bool) -> crate::Rfkill {
    crate::Rfkill { id: "0".into(), name: "hci0".into(), kind: "bluetooth".into(), soft, hard }
}

#[test]
fn every_blocker_has_its_own_plain_sentence_and_a_ready_controller_has_none() {
    let good = ctl(true, "5C:F7:E6:01:02:03", true, true);
    assert_eq!(blocker(&Ok(vec![good.clone()]), true, &[sw(false, false)]), None);
    assert!(blocker(&Ok(vec![]), false, &[]).unwrap().contains("no Bluetooth controller"));
    assert!(blocker(&Err("x".into()), false, &[]).unwrap().contains("no Bluetooth controller"));
    assert!(blocker(&Err("boom".into()), true, &[]).unwrap().contains("cannot be asked (boom)"));
    assert!(blocker(&Ok(vec![]), true, &[]).unwrap().contains("not set up"));
    assert!(blocker(&Ok(vec![good.clone()]), true, &[sw(true, false)]).unwrap().contains("On/off row"), "the radio blocked by On/off");
    assert!(blocker(&Ok(vec![good.clone()]), true, &[sw(false, true)]).unwrap().contains("key or switch"));
    assert!(blocker(&Ok(vec![ctl(false, "00:00:00:00:00:00", false, true)]), true, &[]).unwrap().contains("firmware"));
    assert!(blocker(&Ok(vec![ctl(true, "5C:F7:E6:01:02:03", false, true)]), true, &[]).unwrap().contains("Power"));
    assert!(blocker(&Ok(vec![ctl(true, "5C:F7:E6:01:02:03", true, false)]), true, &[]).unwrap().contains("Low Energy"));
}

#[test]
fn the_sharing_page_gets_one_line_about_the_controller() {
    let d = std::env::temp_dir().join(format!("bt-pan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let r = crate::Roots { sys: d.clone(), proc_: d.join("proc") };
    assert!(controller_state(&r).unwrap_err().contains("no Bluetooth controller"));
    std::fs::create_dir_all(d.join("class/bluetooth/hci0")).unwrap();
    assert_eq!(controller_state(&r), Ok("hci0".to_string()));
    std::fs::create_dir_all(d.join("class/rfkill/rfkill0")).unwrap();
    std::fs::write(d.join("class/rfkill/rfkill0/type"), "bluetooth\n").unwrap();
    std::fs::write(d.join("class/rfkill/rfkill0/soft"), "1\n").unwrap();
    std::fs::write(d.join("class/rfkill/rfkill0/hard"), "0\n").unwrap();
    assert!(controller_state(&r).unwrap_err().contains("On/off"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn frames_for_the_interface_have_no_bluetooth_headers_and_short_ones_are_ignored() {
    let f = Frame { dst: [1; 6], src: [2; 6], proto: 0x0806, payload: vec![7, 8] };
    assert_eq!(ethernet_bytes(&f), [&[1u8; 6][..], &[2u8; 6], &[8, 6], &[7, 8]].concat());
    assert_eq!(frame_of(&ethernet_bytes(&f)), Some(f));
    assert_eq!(frame_of(&[0; 13]), None);
}

#[test]
fn nothing_is_connected_at_the_start_and_disconnecting_says_so() {
    let t = RealTether { inner: Mutex::new(Inner { view: TetherView::idle(), stop: None }) };
    assert_eq!(t.view().state, TetherState::Idle);
    assert_eq!(t.disconnect().unwrap_err(), "No phone is connected.");
}
