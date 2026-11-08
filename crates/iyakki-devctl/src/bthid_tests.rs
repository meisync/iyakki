use super::*;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

struct Chan {
    sent: Vec<Vec<u8>>,
    inbox: VecDeque<Recv>,
    /// replies produced when something is sent: called with the request
    reply: Option<Box<dyn FnMut(&[u8]) -> Vec<Recv>>>,
}

impl Chan {
    fn new() -> Chan {
        Chan { sent: vec![], inbox: VecDeque::new(), reply: None }
    }
}

impl Channel for Chan {
    fn send(&mut self, d: &[u8]) -> io::Result<()> {
        self.sent.push(d.to_vec());
        if let Some(f) = self.reply.as_mut() {
            self.inbox.extend(f(d));
        }
        Ok(())
    }
    fn recv(&mut self, _w: Duration) -> io::Result<Recv> {
        Ok(self.inbox.pop_front().unwrap_or(Recv::Timeout))
    }
}

#[derive(Clone, Default)]
struct FakeUhid(Arc<Mutex<Vec<Vec<u8>>>>);

impl Uhid for FakeUhid {
    fn write(&mut self, e: &[u8]) -> io::Result<()> {
        self.0.lock().unwrap().push(e.to_vec());
        Ok(())
    }
}

fn ty(e: &[u8]) -> u32 {
    u32::from_le_bytes(e[0..4].try_into().unwrap())
}

/// A keyboard's descriptor: boot keyboard, 8 modifier bits, 6 keys, LEDs.
const KBD: [u8; 8] = [0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0xc0, 0x00];

fn s(tag: u8, b: &[u8]) -> Vec<u8> {
    let mut v = if b.len() < 256 { vec![tag, b.len() as u8] } else { vec![tag + 1, (b.len() >> 8) as u8, b.len() as u8] };
    v.extend_from_slice(b);
    v
}

fn seq(items: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = items.concat();
    let mut v = vec![0x36, (body.len() >> 8) as u8, body.len() as u8];
    v.extend_from_slice(&body);
    v
}

fn u16e(n: u16) -> Vec<u8> {
    vec![0x09, (n >> 8) as u8, n as u8]
}

/// The attribute lists of one HID record: name, reconnect initiate and the descriptor.
fn record(name: &str, desc: &[u8]) -> Vec<u8> {
    seq(&[seq(&[
        u16e(0x0001),
        seq(&[vec![0x19, 0x11, 0x24]]),
        u16e(0x0100),
        s(0x25, name.as_bytes()),
        u16e(0x0205),
        vec![0x28, 1],
        u16e(0x0206),
        seq(&[seq(&[vec![0x08, 0x22], s(0x25, desc)])]),
    ])])
}

fn response(tid: u16, data: &[u8], cont: &[u8]) -> Vec<u8> {
    let mut p = (data.len() as u16).to_be_bytes().to_vec();
    p.extend_from_slice(data);
    p.push(cont.len() as u8);
    p.extend_from_slice(cont);
    let mut v = vec![0x07];
    v.extend_from_slice(&tid.to_be_bytes());
    v.extend_from_slice(&(p.len() as u16).to_be_bytes());
    v.extend_from_slice(&p);
    v
}

fn tid_of(req: &[u8]) -> u16 {
    u16::from_be_bytes([req[1], req[2]])
}

#[test]
fn data_elements_parse_nested_booleans_strings_and_hostile_input_is_an_error() {
    let (e, n) = parse_elem(&record("Kbd", &KBD)).unwrap();
    assert!(n > 20 && e.seq().unwrap().len() == 1);
    assert_eq!(parse_elem(&[0x28, 1]).unwrap().0, Elem::Bool(true));
    assert_eq!(parse_elem(&[0x08, 0x22]).unwrap().0, Elem::Uint(0x22));
    assert_eq!(parse_elem(&[0x10, 0xff]).unwrap().0, Elem::Int(-1));
    assert_eq!(parse_elem(&[0x19, 0x11, 0x24]).unwrap().0.uuid16(), Some(0x1124));
    assert_eq!(parse_elem(&[0x1a, 0, 0, 0x11, 0x24]).unwrap().0.uuid16(), Some(0x1124));
    assert_eq!(parse_elem(&[0x00]).unwrap().0, Elem::Nil);
    // longer than its data, empty, nested forever
    assert!(parse_elem(&[0x25, 9, 1]).is_err());
    assert!(parse_elem(&[]).is_err());
    let deep: Vec<u8> = std::iter::repeat([0x35u8, 0x02]).take(20).flatten().collect();
    assert!(parse_elem(&deep).is_err());
    for cut in 0..record("Kbd", &KBD).len() {
        let _ = parse_elem(&record("Kbd", &KBD)[..cut]);
    }
}

#[test]
fn the_request_asks_for_the_hid_service_and_every_attribute_and_carries_the_continuation() {
    let r = sdp_request(7, &[1, 2]);
    assert_eq!(r[0], 0x06);
    assert_eq!(&r[1..3], &[0, 7]);
    assert_eq!(&r[5..10], &[0x35, 0x03, 0x19, 0x11, 0x24]);
    assert_eq!(&r[r.len() - 3..], &[2, 1, 2]);
    assert_eq!(u16::from_be_bytes([r[3], r[4]]) as usize, r.len() - 5);
}

#[test]
fn the_descriptor_name_and_reconnect_flag_are_read_over_two_fragments() {
    let all = record("Magic Keyboard", &KBD);
    let (a, b) = all.split_at(20);
    let (a, b) = (a.to_vec(), b.to_vec());
    let mut ch = Chan::new();
    let mut step = 0;
    ch.reply = Some(Box::new(move |req| {
        let t = tid_of(req);
        step += 1;
        vec![Recv::Data(if step == 1 { response(t, &a, &[0xaa]) } else { response(t, &b, &[]) })]
    }));
    let rec = fetch_hid_record(&mut ch, Duration::from_secs(1)).unwrap();
    assert_eq!(rec.descriptor, KBD);
    assert_eq!(rec.name.as_deref(), Some("Magic Keyboard"));
    assert_eq!(rec.reconnect_initiate, Some(true));
    assert_eq!(ch.sent.len(), 2);
    assert_eq!(*ch.sent[1].last().unwrap(), 0xaa, "the second request carries the continuation state");
}

#[test]
fn a_device_without_a_hid_record_a_refusal_a_silent_device_and_a_closed_channel_are_plain_errors() {
    let mut ch = Chan::new();
    ch.reply = Some(Box::new(|req| vec![Recv::Data(response(tid_of(req), &seq(&[]), &[]))]));
    assert!(matches!(fetch_hid_record(&mut ch, Duration::from_secs(1)), Err(HidError::NoHidService)));
    let mut ch = Chan::new();
    ch.reply = Some(Box::new(|req| vec![Recv::Data(vec![0x01, req[1], req[2], 0, 2, 0, 3])]));
    let e = fetch_hid_record(&mut ch, Duration::from_secs(1)).unwrap_err().to_string();
    assert!(e.contains("refused (code 3)"), "{e}");
    assert!(matches!(fetch_hid_record(&mut Chan::new(), Duration::from_secs(1)), Err(HidError::Timeout)));
    let mut ch = Chan::new();
    ch.reply = Some(Box::new(|_| vec![Recv::Closed]));
    assert!(matches!(fetch_hid_record(&mut ch, Duration::from_secs(1)), Err(HidError::Closed)));
    // an answer to another request, and an oversized descriptor
    let mut ch = Chan::new();
    ch.reply = Some(Box::new(|_| vec![Recv::Data(response(99, &[], &[]))]));
    assert!(fetch_hid_record(&mut ch, Duration::from_secs(1)).unwrap_err().to_string().contains("another request"));
    assert!(parse_hid_record(&record("x", &vec![1u8; 5000])).is_err(), "a descriptor the kernel cannot take");
}

#[test]
fn uhid_events_have_the_kernels_layout() {
    let c = uhid_create2("Kbd", "AA:BB:CC:DD:EE:FF", 0x1234, 0x5678, &KBD);
    assert_eq!(c.len(), UHID_EVENT_SIZE);
    assert_eq!(UHID_EVENT_SIZE, 4376);
    assert_eq!(ty(&c), UHID_CREATE2);
    assert_eq!(&c[4..7], b"Kbd");
    assert_eq!(&c[196..213], b"AA:BB:CC:DD:EE:FF");
    assert_eq!(u16::from_le_bytes([c[260], c[261]]), 8, "rd_size");
    assert_eq!(u16::from_le_bytes([c[262], c[263]]), 5, "BUS_BLUETOOTH");
    assert_eq!(u32::from_le_bytes(c[264..268].try_into().unwrap()), 0x1234);
    assert_eq!(u32::from_le_bytes(c[268..272].try_into().unwrap()), 0x5678);
    assert_eq!(&c[280..288], &KBD);
    let long = "n".repeat(300);
    let c = uhid_create2(&long, "", 0, 0, &KBD);
    assert_eq!(c[4 + 127], 0, "the name is cut and stays zero terminated");
    let i = uhid_input2(&[1, 0, 4, 0, 0, 0, 0, 0, 0]);
    assert_eq!((ty(&i), u16::from_le_bytes([i[4], i[5]])), (UHID_INPUT2, 9));
    assert_eq!(&i[6..15], &[1, 0, 4, 0, 0, 0, 0, 0, 0]);
    assert_eq!(ty(&uhid_destroy()), UHID_DESTROY);
    let g = uhid_get_report_reply(5, 5);
    assert_eq!((ty(&g), u32::from_le_bytes(g[4..8].try_into().unwrap()), u16::from_le_bytes([g[8], g[9]])), (UHID_GET_REPORT_REPLY, 5, 5));
}

#[test]
fn kernel_events_are_understood() {
    let mut o = vec![0u8; UHID_EVENT_SIZE];
    o[0..4].copy_from_slice(&UHID_OUTPUT.to_le_bytes());
    o[4] = 1;
    o[5] = 2;
    o[4100..4102].copy_from_slice(&2u16.to_le_bytes());
    assert_eq!(parse_uhid(&o), Some(UhidEvent::Output(vec![1, 2])));
    let mut g = vec![0u8; 16];
    g[0..4].copy_from_slice(&UHID_GET_REPORT.to_le_bytes());
    g[4..8].copy_from_slice(&9u32.to_le_bytes());
    assert_eq!(parse_uhid(&g), Some(UhidEvent::GetReport { id: 9 }));
    assert_eq!(parse_uhid(&UHID_OPEN.to_le_bytes()), Some(UhidEvent::Open));
    assert_eq!(parse_uhid(&[1, 2]), None);
    assert_eq!(parse_uhid(&UHID_OUTPUT.to_le_bytes()), None, "a truncated output event is ignored");
}

fn open() -> (Session<FakeUhid>, FakeUhid, Chan) {
    let u = FakeUhid::default();
    let mut ctl = Chan::new();
    let s = Session::open(u.clone(), "Kbd", "AA:BB:CC:DD:EE:FF", 0, 0, &KBD, &mut ctl).unwrap();
    assert_eq!(ctl.sent, vec![vec![0x71]], "report protocol is asked for");
    (s, u, ctl)
}

#[test]
fn reports_reach_the_kernel_leds_reach_the_keyboard_and_report_requests_get_an_error_at_once() {
    let (mut s, u, _ctl) = open();
    assert_eq!(ty(&u.0.lock().unwrap()[0]), UHID_CREATE2);
    let rep = [0xa1, 1, 0, 4, 0, 0, 0, 0, 0];
    assert_eq!(s.on_interrupt(&rep).unwrap(), Flow::Continue);
    assert_eq!(s.on_interrupt(&[0xa3, 1]).unwrap(), Flow::Continue, "feature reports are not input");
    assert_eq!(s.on_interrupt(&[0xa1]).unwrap(), Flow::Continue, "an empty report is dropped");
    {
        let w = u.0.lock().unwrap();
        assert_eq!(w.len(), 2);
        assert_eq!(&w[1][6..14], &rep[1..9]);
    }
    let mut intr = Chan::new();
    s.on_uhid(UhidEvent::Output(vec![1, 2]), &mut intr).unwrap();
    assert_eq!(intr.sent, vec![vec![0xa2, 1, 2]]);
    s.on_uhid(UhidEvent::GetReport { id: 3 }, &mut intr).unwrap();
    s.on_uhid(UhidEvent::SetReport { id: 4 }, &mut intr).unwrap();
    s.on_uhid(UhidEvent::Open, &mut intr).unwrap();
    let w = u.0.lock().unwrap();
    assert_eq!((ty(&w[2]), ty(&w[3])), (UHID_GET_REPORT_REPLY, UHID_SET_REPORT_REPLY));
    assert_eq!(w.len(), 4);
    assert_eq!(s.reports, 1);
}

#[test]
fn dropping_the_session_removes_the_input_device_and_a_bad_descriptor_is_refused() {
    let (s, u, _c) = open();
    drop(s);
    assert_eq!(ty(u.0.lock().unwrap().last().unwrap()), UHID_DESTROY);
    let mut c = Chan::new();
    assert!(Session::open(FakeUhid::default(), "x", "", 0, 0, &[], &mut c).is_err());
    assert!(Session::open(FakeUhid::default(), "x", "", 0, 0, &vec![0; 5000], &mut c).is_err());
}

struct Script(VecDeque<Ev>);
impl Source for Script {
    fn wait(&mut self, _t: Duration) -> io::Result<Ev> {
        Ok(self.0.pop_front().unwrap_or(Ev::Idle))
    }
}

#[test]
fn a_running_session_ends_when_the_link_is_lost_unplugged_or_stopped() {
    let stop = AtomicBool::new(false);
    let (mut s, u, _c) = open();
    let mut intr = Chan::new();
    let mut src = Script(VecDeque::from([Ev::Interrupt(vec![0xa1, 1, 2]), Ev::Idle, Ev::ChannelClosed]));
    assert_eq!(run_session(&mut s, &mut intr, &mut src, &stop), End::LinkLost);
    assert_eq!(u.0.lock().unwrap().len(), 2);
    let (mut s, _u, _c) = open();
    let mut src = Script(VecDeque::from([Ev::Control(vec![0x15])]));
    assert_eq!(run_session(&mut s, &mut intr, &mut src, &stop), End::Unplugged);
    let (mut s, _u, _c) = open();
    let mut src = Script(VecDeque::from([Ev::Control(vec![0x00])]));
    stop.store(true, Ordering::SeqCst);
    assert_eq!(run_session(&mut s, &mut intr, &mut src, &stop), End::Stopped);
}

#[test]
fn a_reconnect_waits_longer_each_time_and_gives_up_after_a_few_tries() {
    let d: Vec<u64> = (0..6).map(|a| retry_delay(a).map(|d| d.as_secs()).unwrap_or(0)).collect();
    assert_eq!(d, vec![3, 10, 30, 60, 60, 0]);
}

fn key_event(hint: u8, addr: [u8; 6], val: u8) -> Vec<u8> {
    let mut p = vec![hint];
    p.extend_from_slice(&addr);
    p.extend_from_slice(&[0, 4]);
    p.extend_from_slice(&[val; 16]);
    p.push(0);
    p
}

#[test]
fn keys_the_kernel_says_to_keep_are_stored_and_loaded_back_after_a_restart() {
    let mut st = Store::default();
    assert!(!st.note_link_key(&key_event(0, [1, 2, 3, 4, 5, 6], 9)), "store_hint 0: a one-time key is not kept");
    assert!(st.note_link_key(&key_event(1, [1, 2, 3, 4, 5, 6], 9)));
    assert!(st.note_link_key(&key_event(1, [1, 2, 3, 4, 5, 6], 7)), "a new key replaces the old one");
    assert!(!st.note_link_key(&[1, 2]));
    assert_eq!(st.link_keys.len(), 1);
    let p = st.link_keys_params().unwrap();
    assert_eq!(&p[0..3], &[0, 1, 0]);
    assert_eq!(p.len(), 3 + 25);
    assert_eq!(&p[3..9], &[1, 2, 3, 4, 5, 6]);
    assert_eq!(&p[11..27], &[7u8; 16]);
    let mut ltk = vec![1u8, 1, 2, 3, 4, 5, 6, 1, 1, 0, 16];
    ltk.extend_from_slice(&[0u8; 26]);
    assert_eq!(ltk.len(), 37);
    assert!(st.note_ltk(&ltk));
    assert_eq!(st.ltk_params().unwrap().len(), 2 + 36);
    // the file text round-trips and a damaged file is an empty store
    assert_eq!(Store::parse(&st.to_text()), st);
    assert_eq!(Store::parse("{not json"), Store::default());
    assert!(Store::default().link_keys_params().is_none());
    // Load Link Keys and Load Long Term Keys go to the controller
    struct T(Vec<Vec<u8>>, VecDeque<Vec<u8>>);
    impl Transport for T {
        fn send(&mut self, p: &[u8]) -> io::Result<()> {
            self.0.push(p.to_vec());
            let op = u16::from_le_bytes([p[0], p[1]]);
            let mut r = vec![0x01, 0x00, p[2], p[3], 3, 0, p[0], p[1], 0];
            r[4] = 3;
            self.1.push_back(r);
            let _ = op;
            Ok(())
        }
        fn recv(&mut self, _w: Duration) -> io::Result<Option<Vec<u8>>> {
            Ok(self.1.pop_front())
        }
    }
    let mut m = Mgmt::new(T(vec![], VecDeque::new()));
    assert_eq!(load_keys(&mut m, 0, &st).unwrap(), 2);
    let ops: Vec<u16> = m.transport().0.iter().map(|p| u16::from_le_bytes([p[0], p[1]])).collect();
    assert_eq!(ops, vec![OP_LOAD_LINK_KEYS, OP_LOAD_LTKS]);
}

#[test]
fn forgetting_removes_the_device_and_its_keys_and_names_are_found_by_address_or_name() {
    let mut st = Store::default();
    let k = |a: &str, n: &str| Known { address: a.into(), addr_type: 0, name: n.into(), kind: "keyboard".into(), descriptor: hex(&KBD), vendor: 0, product: 0, auto: true };
    st.upsert(k("01:02:03:04:05:06", "Kbd"));
    st.upsert(k("01:02:03:04:05:06", "Kbd renamed"));
    assert_eq!(st.devices.len(), 1);
    assert_eq!(st.device("01:02:03:04:05:06").unwrap().name, "Kbd renamed");
    st.note_link_key(&key_event(1, [6, 5, 4, 3, 2, 1], 1));
    assert!(st.forget("01:02:03:04:05:06"));
    assert!(st.devices.is_empty() && st.link_keys.is_empty());
    assert!(!st.forget("01:02:03:04:05:06"));
    assert_eq!(unhex(&hex(&KBD)).unwrap(), KBD);
    assert_eq!(unhex("zz"), None);
    let v = |a: &str, n: &str| DeviceView { address: a.into(), addr_type: 0, name: n.into(), kind: "mouse".into(), state: LinkState::Disconnected };
    let l = vec![v("01:02:03:04:05:06", "Mouse"), v("0A:02:03:04:05:06", "Mouse"), v("0B:02:03:04:05:06", "Pad")];
    assert_eq!(find_device(&l, "pad").unwrap().address, "0B:02:03:04:05:06");
    assert_eq!(find_device(&l, "0a:02:03:04:05:06").unwrap().name, "Mouse");
    assert!(find_device(&l, "mouse").unwrap_err().contains("more than one"));
    assert!(find_device(&l, "").is_err() && find_device(&l, "nope").is_err());
}

#[test]
fn every_failure_has_a_plain_reason() {
    for (e, w) in [(libc::EHOSTUNREACH, "switch it on"), (libc::ECONNREFUSED, "Forget"), (libc::ENETDOWN, "power the controller on"), (libc::EPERM, "administrator")] {
        assert!(connect_reason(e).contains(w), "{e}");
    }
    assert!(HidError::NoHidService.to_string().contains("keyboard, mouse or game pad"));
    assert!(HidError::Connect(libc::ETIMEDOUT).to_string().contains("did not answer"));
    let d = DeviceView { address: "AA:BB:CC:DD:EE:FF".into(), addr_type: 0, name: String::new(), kind: "keyboard".into(), state: LinkState::Failed(connect_reason(libc::ETIMEDOUT)) };
    assert_eq!(d.title(), "AA:BB:CC:DD:EE:FF");
    assert!(d.line().starts_with("keyboard · AA:BB:CC:DD:EE:FF · not connected: the device did not answer"));
    assert!(is_hid_kind(btmgmt::Kind::Gamepad) && !is_hid_kind(btmgmt::Kind::Headphones));
}
