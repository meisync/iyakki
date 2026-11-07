//! Framing tests of the real nl80211 transport against a fake kernel: a datagram socket pair, with a thread that answers
//! the way the generic netlink controller and nl80211 do. No netlink, no root and no Wi-Fi card needed.
//! The tests that need the real kernel (virtual radios) are `#[ignore]`d: see `hotspot_radio_tests.rs`.

use super::*;
use iyakki_hotspot::nl80211::{ApControl, Msg, NlError};
use std::os::unix::net::UnixDatagram;

fn seq_of(req: &[u8]) -> u32 {
    u32::from_le_bytes(req[8..12].try_into().unwrap())
}

fn ack(seq: u32, errno: i32, request: &[u8], ext: Option<&str>) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&(-errno).to_le_bytes());
    p.extend_from_slice(&request[..16.min(request.len())]);
    if let Some(t) = ext {
        // the kernel repeats the whole request, then adds attributes (type 1 = message)
        p.extend_from_slice(&request[16..]);
        let mut a = t.as_bytes().to_vec();
        a.push(0);
        p.extend_from_slice(&((4 + a.len()) as u16).to_le_bytes());
        p.extend_from_slice(&1u16.to_le_bytes());
        p.extend_from_slice(&a);
        while p.len() % 4 != 0 {
            p.push(0);
        }
    }
    let mut m = Vec::new();
    m.extend_from_slice(&((16 + p.len()) as u32).to_le_bytes());
    m.extend_from_slice(&NLMSG_ERROR.to_le_bytes());
    m.extend_from_slice(&0u16.to_le_bytes());
    m.extend_from_slice(&seq.to_le_bytes());
    m.extend_from_slice(&0u32.to_le_bytes());
    m.extend_from_slice(&p);
    m
}

fn done(seq: u32) -> Vec<u8> {
    let mut m = Vec::new();
    m.extend_from_slice(&20u32.to_le_bytes());
    m.extend_from_slice(&NLMSG_DONE.to_le_bytes());
    m.extend_from_slice(&2u16.to_le_bytes());
    m.extend_from_slice(&seq.to_le_bytes());
    m.extend_from_slice(&0u32.to_le_bytes());
    m.extend_from_slice(&0u32.to_le_bytes());
    m
}

fn family_reply(seq: u32) -> Vec<u8> {
    Msg::new(nl80211::GENL_ID_CTRL, 0, seq, 1)
        .strz(nl80211::CTRL_ATTR_FAMILY_NAME, "nl80211")
        .u16(nl80211::CTRL_ATTR_FAMILY_ID, 0x1c)
        .nest(CTRL_ATTR_MCAST_GROUPS, |g| g.nest(1, |x| x.strz(1, "config").u32(2, 4)).nest(2, |x| x.strz(1, "mlme").u32(2, 7)))
        .finish()
}

fn event(cmd: u8, ifindex: u32, sta: Mac) -> Vec<u8> {
    Msg::new(0x1c, 0, 0, cmd).u32(nl80211::attr::IFINDEX, ifindex).bytes(nl80211::attr::MAC, &sta).finish()
}

/// A socket under test and the thread that plays the kernel: `handler` gets each request and returns the datagrams to send back.
fn with_kernel(timeout_ms: u64, handler: impl Fn(&[u8]) -> Vec<Vec<u8>> + Send + 'static) -> (Nl80211Socket, std::thread::JoinHandle<()>) {
    let (a, b) = UnixDatagram::pair().unwrap();
    b.set_read_timeout(Some(Duration::from_millis(1500))).unwrap();
    let t = std::thread::spawn(move || {
        let mut buf = vec![0u8; 65536];
        while let Ok(n) = b.recv(&mut buf) {
            for d in handler(&buf[..n]) {
                let _ = b.send(&d);
            }
        }
    });
    (Nl80211Socket::from_fd(OwnedFd::from(a), Duration::from_millis(timeout_ms)), t)
}

#[test]
fn the_family_is_resolved_once_with_its_multicast_groups() {
    let count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let c2 = count.clone();
    let (mut s, _k) = with_kernel(1000, move |req| {
        c2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(&req[4..6], &nl80211::GENL_ID_CTRL.to_le_bytes(), "the first question goes to the controller");
        vec![family_reply(seq_of(req))]
    });
    let f = s.family().unwrap();
    assert_eq!(f.id, 0x1c);
    assert_eq!(f.group("mlme"), Some(7));
    assert_eq!(f.group("config"), Some(4));
    assert_eq!(f.group("scan"), None);
    assert_eq!(s.family().unwrap(), f);
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1, "the second call is answered from memory");
}

#[test]
fn replies_are_matched_by_sequence_number_and_other_messages_become_events() {
    let sta = [2, 1, 2, 3, 4, 5];
    let (mut s, _k) = with_kernel(1000, move |req| {
        let seq = seq_of(req);
        // an event first, a reply of someone else's request, then our acknowledgement
        vec![event(nl80211::cmd::NEW_STATION, 9, sta), ack(seq.wrapping_add(77), 0, req, None), ack(seq, 0, req, None)]
    });
    let req = nl80211::stop_ap(0x1c, 5, 9);
    let r = s.exchange(&req).unwrap();
    assert_eq!(nl80211::check_ack(&r), Ok(()));
    assert_eq!(seq_of(&r), 5, "only our own reply is returned");
    // both foreign messages were kept, in order
    let e1 = s.next_event(100).unwrap().unwrap();
    assert_eq!(nl80211::parse_event(&e1), Some(nl80211::Event::Station { ifindex: 9, sta, added: true }));
    let e2 = s.next_event(100).unwrap().unwrap();
    assert_eq!(seq_of(&e2), 5 + 77);
    assert_eq!(s.next_event(10).unwrap(), None);
}

#[test]
fn a_dump_is_collected_up_to_the_done_message() {
    let (mut s, _k) = with_kernel(1000, |req| {
        let seq = seq_of(req);
        let part = |n: u32| Msg::new(0x1c, NLM_F_MULTI_FOR_TESTS, seq, nl80211::cmd::NEW_WIPHY).u32(nl80211::attr::WIPHY, n).strz(nl80211::attr::WIPHY_NAME, &format!("phy{n}")).nest(nl80211::attr::SUPPORTED_IFTYPES, |m| m.flag(2).flag(3)).finish();
        // two parts in one datagram, one in the next, then done
        let mut first = part(0);
        first.extend(part(1));
        vec![first, part(2), done(seq)]
    });
    let req = nl80211::get_wiphy_dump(0x1c, 3);
    let r = s.exchange(&req).unwrap();
    let w = nl80211::parse_wiphy_dump(&r).unwrap();
    assert_eq!(w.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["phy0", "phy1", "phy2"]);
    assert!(w.iter().all(|x| x.supports_ap()));
}

const NLM_F_MULTI_FOR_TESTS: u16 = 2;

#[test]
fn a_refusal_carries_the_errno_and_the_kernels_own_words() {
    let (mut s, _k) = with_kernel(1000, |req| vec![ack(seq_of(req), 22, req, Some("Beacon head too short"))]);
    let req = nl80211::start_ap(0x1c, 8, &nl80211::ApParams { ifindex: 3, ssid: b"X".to_vec(), freq_mhz: 2437, beacon_interval_tu: 100, dtim_period: 2, beacon_head: vec![1], beacon_tail: vec![], probe_resp: vec![], inactivity_secs: 300, pmk: None });
    let r = s.exchange(&req).unwrap();
    assert_eq!(nl80211::check_ack(&r), Err(NlError::Kernel(22)));
    assert_eq!(s.last_ext_ack(), Some("Beacon head too short"));
}

#[test]
fn silence_ends_in_a_timeout_and_garbage_is_reported() {
    let (mut s, _k) = with_kernel(120, |_| vec![]);
    let t = Instant::now();
    let e = s.exchange(&nl80211::stop_ap(0x1c, 1, 2)).unwrap_err();
    assert!(matches!(e, NlError::Io(ref m) if m.contains("did not answer")), "{e:?}");
    assert!(t.elapsed() >= Duration::from_millis(100) && t.elapsed() < Duration::from_secs(2));
    let (mut s2, _k2) = with_kernel(500, |_| vec![vec![1, 2, 3]]);
    assert_eq!(s2.exchange(&nl80211::stop_ap(0x1c, 1, 2)), Err(NlError::Malformed));
}

#[test]
fn the_control_layer_works_through_the_real_transport_framing() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(u16, u8)>::new()));
    let s2 = seen.clone();
    let (s, _k) = with_kernel(1000, move |req| {
        let seq = seq_of(req);
        let ty = u16::from_le_bytes([req[4], req[5]]);
        s2.lock().unwrap().push((ty, req[16]));
        if ty == nl80211::GENL_ID_CTRL {
            vec![family_reply(seq)]
        } else {
            vec![ack(seq, 0, req, None)]
        }
    });
    let mut c = ApControl::new(SharedNl::new(s));
    c.set_ap_mode(4).unwrap();
    c.stop_ap(4).unwrap();
    let v = seen.lock().unwrap().clone();
    assert_eq!(v, vec![(nl80211::GENL_ID_CTRL, 3), (0x1c, nl80211::cmd::SET_INTERFACE), (0x1c, nl80211::cmd::STOP_AP)]);
}

#[test]
fn a_registered_frame_event_survives_while_a_request_is_in_flight() {
    // the frame event arrives between a request and its acknowledgement: it must not be lost and must not be taken for the reply
    let frame = vec![0xb0u8; 30];
    let f2 = frame.clone();
    let (mut s, _k) = with_kernel(1000, move |req| {
        if u16::from_le_bytes([req[4], req[5]]) == nl80211::GENL_ID_CTRL {
            return vec![family_reply(seq_of(req))];
        }
        let ev = Msg::new(0x1c, 0, 0, nl80211::cmd::FRAME).u32(nl80211::attr::IFINDEX, 6).u32(nl80211::attr::WIPHY_FREQ, 2437).bytes(nl80211::attr::FRAME, &f2).finish();
        vec![ev, ack(seq_of(req), 0, req, None)]
    });
    s.run(|f, q| nl80211::stop_ap(f, q, 6)).unwrap();
    let e = s.next_event(100).unwrap().expect("the frame event was kept");
    assert_eq!(nl80211::parse_event(&e), Some(nl80211::Event::Frame { ifindex: 6, freq_mhz: 2437, frame }));
}

#[test]
fn the_family_reply_parser_never_panics_on_junk() {
    let mut x = 12345u32;
    for n in 0..300 {
        let b: Vec<u8> = (0..(n % 90)).map(|_| { x = x.wrapping_mul(1664525).wrapping_add(1013904223); (x >> 24) as u8 }).collect();
        let _ = parse_family(&b);
        let _ = ext_ack_text(&b);
        let _ = cut(&b);
    }
}

/// Needs the virtual-radio VM (KVM, qemu, the small test kernel); prints PASS/FAIL per check and fails on any FAIL.
/// Run: `cargo test -p iyakki-agent --lib hotspot_works_on_virtual_radios -- --ignored --nocapture`
#[test]
#[ignore = "boots a QEMU microVM with virtual radios: crates/iyakki-hotspot/tools/hwsim-test.sh"]
fn hotspot_works_on_virtual_radios() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../iyakki-hotspot/tools/hwsim-test.sh");
    let out = std::process::Command::new("bash").arg(root).output().expect("bash");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    println!("{text}");
    assert!(out.status.success() && !text.contains("FAIL  "), "a virtual-radio check failed");
}

/// Read-only, no rights needed, but only meaningful on a machine that has a Wi-Fi card: the real kernel answers the probe.
/// Run: `cargo test -p iyakki-agent --lib the_real_kernel_answers_the_capability_probe -- --ignored --nocapture`
#[test]
#[ignore = "asks the real kernel (read-only); needs a Wi-Fi radio on the machine"]
fn the_real_kernel_answers_the_capability_probe() {
    let mut s = Nl80211Socket::open(Duration::from_secs(3)).expect("netlink");
    let p = probe(&mut s).expect("probe");
    println!("{p:#?}");
    assert!(!p.phys.is_empty(), "no radio on this machine");
    assert!(p.phys.iter().all(|c| !c.bands.is_empty() && !c.iftypes.is_empty()));
}
