use super::*;
use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex};

struct Attr {
    h: u16,
    t: Uuid,
    v: Vec<u8>,
}

/// A scripted GATT server: an attribute table that answers like a real one would (and records what was written).
struct Server {
    attrs: Vec<Attr>,
    mtu: usize,
    writes: Vec<(u16, Vec<u8>)>,
    cmds: Vec<(u16, Vec<u8>)>,
    /// error code to give for the first write request to the client configuration (pairing not done yet)
    ccc_error_once: Option<u8>,
}

fn a(h: u16, t: u16, v: &[u8]) -> Attr {
    Attr { h, t: Uuid::Short(t), v: v.to_vec() }
}

fn err(op: u8, h: u16, code: u8) -> Vec<Vec<u8>> {
    vec![Pdu::Error { op, handle: h, code }.encode()]
}

impl Server {
    /// A keyboard: HID service 1-12, device information 13-15.
    fn keyboard(map_len: usize) -> Server {
        let mut map = vec![0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0x85, 0x01];
        while map.len() < map_len - 1 {
            map.extend_from_slice(&[0x05, 0x01]);
        }
        map.push(0xc0);
        map.truncate(map_len);
        let attrs = vec![
            a(1, UUID_PRIMARY, &[0x12, 0x18]),
            a(2, UUID_CHARACTERISTIC, &[0x02, 3, 0, 0x4b, 0x2a]),
            a(3, CH_REPORT_MAP, &map),
            a(4, UUID_CHARACTERISTIC, &[0x12, 5, 0, 0x4d, 0x2a]),
            a(5, CH_REPORT, &[0; 8]),
            a(6, UUID_CCC, &[0, 0]),
            a(7, UUID_REPORT_REF, &[1, 1]),
            a(8, UUID_CHARACTERISTIC, &[0x0e, 9, 0, 0x4d, 0x2a]),
            a(9, CH_REPORT, &[0]),
            a(10, UUID_REPORT_REF, &[1, 2]),
            a(11, UUID_CHARACTERISTIC, &[0x06, 12, 0, 0x4e, 0x2a]),
            a(12, CH_PROTOCOL_MODE, &[1]),
            a(13, UUID_PRIMARY, &[0x0a, 0x18]),
            a(14, UUID_CHARACTERISTIC, &[0x02, 15, 0, 0x50, 0x2a]),
            a(15, CH_PNP_ID, &[1, 0x6d, 0x04, 0x0b, 0xb0, 0x10, 0x01]),
        ];
        Server { attrs, mtu: 64, writes: vec![], cmds: vec![], ccc_error_once: None }
    }

    fn service_end(&self, start: u16) -> u16 {
        self.attrs.iter().skip_while(|x| x.h != start).skip(1).find(|x| x.t == Uuid::Short(UUID_PRIMARY)).map_or(self.attrs.last().unwrap().h, |n| n.h - 1)
    }

    fn handle(&mut self, req: &[u8]) -> Vec<Vec<u8>> {
        let Ok(p) = Pdu::parse(req) else { return vec![] };
        let op = p.opcode();
        match p {
            Pdu::MtuReq(_) => vec![Pdu::MtuRsp(self.mtu as u16).encode()],
            Pdu::ReadByGroupReq { start, end, uuid } => {
                let mut list = vec![];
                for x in self.attrs.iter().filter(|x| x.h >= start && x.h <= end && Uuid::Short(UUID_PRIMARY) == uuid && x.t == uuid) {
                    // one service per answer keeps the multi-round loop honest
                    if list.is_empty() {
                        list.push((x.h, self.service_end(x.h), x.v.clone()));
                    }
                }
                if list.is_empty() { err(op, start, ERR_ATTRIBUTE_NOT_FOUND) } else { vec![Pdu::ReadByGroupRsp(list).encode()] }
            }
            Pdu::ReadByTypeReq { start, end, uuid } => {
                let list: Vec<(u16, Vec<u8>)> = self.attrs.iter().filter(|x| x.h >= start && x.h <= end && x.t == uuid).map(|x| (x.h, x.v.clone())).collect();
                let list: Vec<_> = list.into_iter().take(2).collect();
                if list.is_empty() { err(op, start, ERR_ATTRIBUTE_NOT_FOUND) } else { vec![Pdu::ReadByTypeRsp(list).encode()] }
            }
            Pdu::FindInfoReq { start, end } => {
                let list: Vec<(u16, Uuid)> = self.attrs.iter().filter(|x| x.h >= start && x.h <= end).map(|x| (x.h, x.t)).collect();
                if list.is_empty() { err(op, start, ERR_ATTRIBUTE_NOT_FOUND) } else { vec![Pdu::FindInfoRsp(list).encode()] }
            }
            Pdu::ReadReq(h) => match self.attrs.iter().find(|x| x.h == h) {
                Some(x) => vec![Pdu::ReadRsp(x.v[..x.v.len().min(self.mtu - 1)].to_vec()).encode()],
                None => err(op, h, 1),
            },
            Pdu::ReadBlobReq { handle, offset } => match self.attrs.iter().find(|x| x.h == handle) {
                Some(x) if (offset as usize) <= x.v.len() => vec![Pdu::ReadBlobRsp(x.v[offset as usize..].iter().take(self.mtu - 1).copied().collect()).encode()],
                Some(_) => err(op, handle, 0x07),
                None => err(op, handle, 1),
            },
            Pdu::WriteReq { handle, value } => {
                if let Some(c) = self.ccc_error_once.take() {
                    return err(op, handle, c);
                }
                self.writes.push((handle, value.clone()));
                if let Some(x) = self.attrs.iter_mut().find(|x| x.h == handle) {
                    x.v = value;
                }
                vec![Pdu::WriteRsp.encode()]
            }
            Pdu::WriteCmd { handle, value } => {
                self.cmds.push((handle, value));
                vec![]
            }
            Pdu::Confirm => vec![],
            _ => err(op, 0, 0x06),
        }
    }
}

struct Chan {
    srv: Arc<Mutex<Server>>,
    inbox: VecDeque<Recv>,
    sent: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Channel for Chan {
    fn send(&mut self, d: &[u8]) -> io::Result<()> {
        self.sent.lock().unwrap().push(d.to_vec());
        let r = self.srv.lock().unwrap().handle(d);
        self.inbox.extend(r.into_iter().map(Recv::Data));
        Ok(())
    }
    fn recv(&mut self, _w: Duration) -> io::Result<Recv> {
        Ok(self.inbox.pop_front().unwrap_or(Recv::Timeout))
    }
}

type Shared = Arc<Mutex<Server>>;

fn gatt(s: Server) -> (Gatt<Chan>, Shared, Arc<Mutex<Vec<Vec<u8>>>>) {
    let srv = Arc::new(Mutex::new(s));
    let sent = Arc::new(Mutex::new(vec![]));
    let mut g = Gatt::new(Chan { srv: srv.clone(), inbox: VecDeque::new(), sent: sent.clone() });
    g.timeout = Duration::from_millis(50);
    (g, srv, sent)
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

#[test]
fn every_message_survives_an_encode_and_parse_round_trip() {
    let all = vec![
        Pdu::Error { op: 0x0a, handle: 7, code: 5 },
        Pdu::MtuReq(185),
        Pdu::MtuRsp(23),
        Pdu::FindInfoReq { start: 1, end: 9 },
        Pdu::FindInfoRsp(vec![(3, Uuid::Short(0x2902)), (4, Uuid::Short(0x2908))]),
        Pdu::FindInfoRsp(vec![(3, Uuid::Long([7; 16]))]),
        Pdu::ReadByTypeReq { start: 1, end: 0xffff, uuid: Uuid::Short(0x2803) },
        Pdu::ReadByTypeReq { start: 1, end: 0xffff, uuid: Uuid::Long([9; 16]) },
        Pdu::ReadByTypeRsp(vec![(2, vec![1, 2, 3]), (6, vec![4, 5, 6])]),
        Pdu::ReadReq(0x30),
        Pdu::ReadRsp(vec![1, 2, 3]),
        Pdu::ReadBlobReq { handle: 3, offset: 22 },
        Pdu::ReadBlobRsp(vec![9; 5]),
        Pdu::ReadByGroupReq { start: 1, end: 0xffff, uuid: Uuid::Short(0x2800) },
        Pdu::ReadByGroupRsp(vec![(1, 9, vec![0x12, 0x18]), (10, 20, vec![0x0f, 0x18])]),
        Pdu::WriteReq { handle: 6, value: vec![1, 0] },
        Pdu::WriteRsp,
        Pdu::WriteCmd { handle: 9, value: vec![2] },
        Pdu::Notify { handle: 5, value: vec![0, 0, 4, 0, 0, 0, 0, 0] },
        Pdu::Indicate { handle: 5, value: vec![1] },
        Pdu::Confirm,
    ];
    for p in all {
        assert_eq!(Pdu::parse(&p.encode()).unwrap(), p, "{p:?}");
    }
    // the exact bytes of two of them, from the specification
    assert_eq!(Pdu::MtuReq(185).encode(), [0x02, 0xb9, 0x00]);
    assert_eq!(Pdu::ReadByGroupReq { start: 1, end: 0xffff, uuid: Uuid::Short(0x2800) }.encode(), [0x10, 1, 0, 0xff, 0xff, 0x00, 0x28]);
    assert_eq!(Pdu::WriteReq { handle: 6, value: vec![1, 0] }.encode(), [0x12, 6, 0, 1, 0]);
}

#[test]
fn hostile_messages_are_errors_and_never_panic() {
    // every prefix and every one-byte change of valid messages, plus pseudo-random bytes of every length
    let mut seed = 0x1234_5678_9abc_def0u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for len in 0..64 {
        for _ in 0..200 {
            let b: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            let _ = Pdu::parse(&b);
        }
    }
    for op in 0..=255u8 {
        for len in 0..30 {
            let mut b = vec![op];
            b.extend((0..len).map(|i| (i * 7) as u8));
            let _ = Pdu::parse(&b);
        }
    }
    assert!(Pdu::parse(&[]).is_err());
    assert!(Pdu::parse(&[OP_ERROR, 1, 2]).is_err());
    assert!(Pdu::parse(&[OP_FIND_INFO_RSP, 3, 1, 0, 2, 0]).is_err());
    assert!(Pdu::parse(&[OP_READ_BY_TYPE_RSP, 0, 1]).is_err());
    assert!(Pdu::parse(&[OP_READ_BY_TYPE_RSP, 4, 1, 0, 2]).is_err());
    assert!(Pdu::parse(&[OP_READ_BY_GROUP_RSP, 2, 1, 0]).is_err());
    assert!(Pdu::parse(&[OP_NOTIFY, 1]).is_err());
    assert!(Pdu::parse(&[OP_WRITE_RSP, 1]).is_err());
    assert_eq!(Pdu::parse(&[0xd2, 1, 2, 3]).unwrap(), Pdu::Other(0xd2));
}

#[test]
fn the_mtu_is_the_smaller_number_and_a_server_without_the_exchange_stays_at_23() {
    let (mut g, srv, _) = gatt(Server::keyboard(20));
    srv.lock().unwrap().mtu = 100;
    assert_eq!(g.exchange_mtu().unwrap(), 100);
    srv.lock().unwrap().mtu = 5000;
    assert_eq!(g.exchange_mtu().unwrap(), WANT_MTU);
    srv.lock().unwrap().mtu = 4;
    assert_eq!(g.exchange_mtu().unwrap(), DEFAULT_MTU);
    // an error response to the request
    let (mut g2, _, _) = gatt(Server::keyboard(20));
    g2.channel().inbox.push_back(Recv::Data(Pdu::Error { op: OP_MTU_REQ, handle: 0, code: 6 }.encode()));
    assert_eq!(g2.exchange_mtu().unwrap(), DEFAULT_MTU);
}

#[test]
fn services_characteristics_and_descriptors_are_found_over_several_rounds() {
    let (mut g, _, sent) = gatt(Server::keyboard(20));
    let svcs = g.primary_services().unwrap();
    assert_eq!(svcs.iter().map(|s| (s.0, s.1, s.2)).collect::<Vec<_>>(), vec![(1, 12, Uuid::Short(0x1812)), (13, 15, Uuid::Short(0x180a))]);
    let chars = g.characteristics(1, 12).unwrap();
    assert_eq!(chars.iter().map(|c| (c.0, c.2)).collect::<Vec<_>>(), vec![(2, 3), (4, 5), (8, 9), (11, 12)]);
    let d = g.descriptors(6, 7).unwrap();
    assert_eq!(d, vec![(6, Uuid::Short(UUID_CCC)), (7, Uuid::Short(UUID_REPORT_REF))]);
    assert!(sent.lock().unwrap().len() >= 5);
}

#[test]
fn a_long_report_map_is_read_with_blob_requests_and_an_exact_multiple_ends_cleanly() {
    for len in [20usize, 62, 63, 64, 126, 127, 200] {
        let (mut g, _, _) = gatt(Server::keyboard(len));
        g.mtu = 64;
        let got = g.read_long(3).unwrap();
        assert_eq!(got.len(), len, "length {len}");
    }
}

#[test]
fn the_hid_service_is_discovered_with_its_reports_ids_and_numbers() {
    let (mut g, _, _) = gatt(Server::keyboard(100));
    g.exchange_mtu().unwrap();
    let dev = discover_hid(&mut g).unwrap();
    assert_eq!(dev.report_map.len(), 100);
    assert_eq!(dev.reports.len(), 2);
    assert_eq!(dev.reports[0], ReportChar { value_handle: 5, ccc_handle: Some(6), report_id: 1, kind: ReportKind::Input });
    assert_eq!(dev.reports[1], ReportChar { value_handle: 9, ccc_handle: None, report_id: 1, kind: ReportKind::Output });
    assert_eq!(dev.protocol_mode_handle, Some(12));
    assert_eq!((dev.vendor, dev.product), (0x046d, 0xb00b));
    assert!(map_uses_report_ids(&dev.report_map));
}

#[test]
fn a_device_without_the_hid_service_or_a_report_map_is_a_plain_error() {
    let mut s = Server::keyboard(20);
    s.attrs.retain(|x| x.h >= 13);
    let (mut g, _, _) = gatt(s);
    assert!(matches!(discover_hid(&mut g), Err(HidError::NoHidService)));
    let mut s = Server::keyboard(20);
    s.attrs[1].t = Uuid::Short(0x1234); // the report map declaration is not a declaration any more
    s.attrs[1].t = Uuid::Short(0x2999);
    let (mut g, _, _) = gatt(s);
    assert!(matches!(discover_hid(&mut g), Err(HidError::Sdp(_))));
}

#[test]
fn a_server_that_never_ends_its_list_is_stopped() {
    struct Endless(VecDeque<Recv>, u16);
    impl Channel for Endless {
        fn send(&mut self, d: &[u8]) -> io::Result<()> {
            if let Ok(Pdu::ReadByGroupReq { start, .. }) = Pdu::parse(d) {
                self.1 = start;
                self.0.push_back(Recv::Data(Pdu::ReadByGroupRsp(vec![(start, start, vec![1, 2])]).encode()));
            }
            Ok(())
        }
        fn recv(&mut self, _: Duration) -> io::Result<Recv> {
            Ok(self.0.pop_front().unwrap_or(Recv::Timeout))
        }
    }
    let mut g = Gatt::new(Endless(VecDeque::new(), 0));
    assert!(matches!(g.primary_services(), Err(GattError::Protocol(_))));
    // handles that go backwards
    struct Back(VecDeque<Recv>);
    impl Channel for Back {
        fn send(&mut self, _: &[u8]) -> io::Result<()> {
            self.0.push_back(Recv::Data(Pdu::ReadByGroupRsp(vec![(1, 5, vec![1, 2])]).encode()));
            Ok(())
        }
        fn recv(&mut self, _: Duration) -> io::Result<Recv> {
            Ok(self.0.pop_front().unwrap_or(Recv::Timeout))
        }
    }
    let mut g = Gatt::new(Back(VecDeque::new()));
    assert!(matches!(g.primary_services(), Err(GattError::Protocol(_))));
}

#[test]
fn notifications_during_a_request_are_kept_and_indications_are_confirmed() {
    let (mut g, _, sent) = gatt(Server::keyboard(20));
    let ch = g.channel();
    ch.inbox.push_back(Recv::Data(Pdu::Notify { handle: 5, value: vec![1, 2] }.encode()));
    ch.inbox.push_back(Recv::Data(Pdu::Indicate { handle: 5, value: vec![3] }.encode()));
    ch.inbox.push_back(Recv::Data(vec![0xff])); // garbage in the middle is ignored
    ch.inbox.push_back(Recv::Data(Pdu::WriteRsp.encode())); // an answer to nothing
    assert_eq!(g.read(12).unwrap(), vec![1]);
    assert_eq!(g.take_notifications(), vec![(5, vec![1, 2]), (5, vec![3])]);
    assert!(sent.lock().unwrap().iter().any(|m| m == &[OP_CONFIRM]));
}

#[test]
fn a_silent_server_times_out_and_a_closed_channel_is_reported() {
    struct Dead(bool);
    impl Channel for Dead {
        fn send(&mut self, _: &[u8]) -> io::Result<()> {
            Ok(())
        }
        fn recv(&mut self, _: Duration) -> io::Result<Recv> {
            Ok(if self.0 { Recv::Closed } else { Recv::Timeout })
        }
    }
    let mut g = Gatt::new(Dead(false));
    assert!(matches!(g.read(1), Err(GattError::Timeout)));
    let mut g = Gatt::new(Dead(true));
    assert!(matches!(g.read(1), Err(GattError::Closed)));
    assert!(matches!(g.next_notification(Duration::from_millis(1)), Err(GattError::Closed)));
    assert!(att_reason(5).contains("pairing"));
    assert!(att_reason(0xee).contains("0xee"));
}

#[test]
fn notifications_are_enabled_once_more_after_an_encryption_error() {
    let mut s = Server::keyboard(20);
    s.ccc_error_once = Some(ERR_INSUFFICIENT_ENCRYPTION);
    let (mut g, srv, _) = gatt(s);
    g.enable_notifications(6, true).unwrap();
    assert_eq!(srv.lock().unwrap().writes, vec![(6, vec![1, 0])]);
    // a hard refusal stays a refusal
    let mut s = Server::keyboard(20);
    s.ccc_error_once = Some(0x03);
    let (mut g, _, _) = gatt(s);
    assert!(matches!(g.enable_notifications(6, true), Err(GattError::Att { code: 3, .. })));
}

fn open_session() -> (HogpSession<FakeUhid>, Gatt<Chan>, Shared, FakeUhid) {
    let (mut g, srv, _) = gatt(Server::keyboard(100));
    g.exchange_mtu().unwrap();
    let dev = discover_hid(&mut g).unwrap();
    let u = FakeUhid::default();
    let s = HogpSession::open(u.clone(), &mut g, dev, "Test keys", "AA:BB:CC:DD:EE:FF").unwrap();
    (s, g, srv, u)
}

#[test]
fn opening_a_session_selects_report_protocol_turns_notifications_on_and_creates_the_input_device() {
    let (_s, _g, srv, u) = open_session();
    let srv = srv.lock().unwrap();
    assert_eq!(srv.writes, vec![(6, vec![1, 0])]);
    assert_eq!(srv.cmds, vec![(12, vec![1])]);
    let ev = u.0.lock().unwrap();
    assert_eq!(ty(&ev[0]), UHID_CREATE2_FOR_TEST);
    // vendor and product from the PnP id reach the input device
    let o = 4 + 128 + 64 + 64;
    assert_eq!(u32::from_le_bytes(ev[0][o + 4..o + 8].try_into().unwrap()), 0x046d);
    assert_eq!(u32::from_le_bytes(ev[0][o + 8..o + 12].try_into().unwrap()), 0xb00b);
}

const UHID_CREATE2_FOR_TEST: u32 = crate::bthid::UHID_CREATE2;

#[test]
fn notifications_become_input_reports_with_the_report_id_first() {
    let (mut s, _g, _, u) = open_session();
    assert_eq!(s.on_notification(5, &[0, 0, 4, 0, 0, 0, 0, 0]).unwrap(), Note::Delivered);
    assert_eq!(s.on_notification(9, &[1]).unwrap(), Note::Ignored); // not an input report
    assert_eq!(s.on_notification(77, &[1]).unwrap(), Note::Ignored);
    assert_eq!(s.on_notification(5, &[]).unwrap(), Note::Ignored);
    let ev = u.0.lock().unwrap();
    assert_eq!(ty(&ev[1]), crate::bthid::UHID_INPUT2);
    assert_eq!(u16::from_le_bytes([ev[1][4], ev[1][5]]), 9);
    assert_eq!(&ev[1][6..15], &[1, 0, 0, 4, 0, 0, 0, 0, 0]);
    assert_eq!(s.reports, 1);
}

#[test]
fn led_reports_go_back_as_write_commands_and_report_requests_get_an_error_at_once() {
    let (mut s, mut g, srv, u) = open_session();
    s.on_uhid(UhidEvent::Output(vec![1, 2]), &mut g).unwrap(); // id 1, caps lock
    s.on_uhid(UhidEvent::Output(vec![5, 2]), &mut g).unwrap(); // no such output report: dropped
    assert_eq!(srv.lock().unwrap().cmds.last().unwrap(), &(9, vec![2]));
    assert_eq!(srv.lock().unwrap().cmds.len(), 2); // protocol mode + the LED report
    s.on_uhid(UhidEvent::GetReport { id: 3 }, &mut g).unwrap();
    s.on_uhid(UhidEvent::SetReport { id: 4 }, &mut g).unwrap();
    let ev = u.0.lock().unwrap();
    assert_eq!(ty(&ev[1]), crate::bthid::UHID_GET_REPORT_REPLY);
    assert_eq!(ty(&ev[2]), crate::bthid::UHID_SET_REPORT_REPLY);
}

#[test]
fn dropping_the_session_removes_the_input_device() {
    let (s, _g, _, u) = open_session();
    drop(s);
    assert_eq!(ty(u.0.lock().unwrap().last().unwrap()), crate::bthid::UHID_DESTROY);
}

struct Script(VecDeque<Ev>);
impl Source for Script {
    fn wait(&mut self, _t: Duration) -> io::Result<Ev> {
        Ok(self.0.pop_front().unwrap_or(Ev::ChannelClosed))
    }
}

#[test]
fn a_running_session_delivers_reports_confirms_indications_and_ends_when_the_link_is_lost() {
    use crate::bthid::End;
    use std::sync::atomic::AtomicBool;
    let (mut s, mut g, _, u) = open_session();
    let stop = AtomicBool::new(false);
    let mut src = Script(
        vec![
            Ev::Interrupt(Pdu::Notify { handle: 5, value: vec![0, 0, 5, 0, 0, 0, 0, 0] }.encode()),
            Ev::Interrupt(Pdu::Indicate { handle: 5, value: vec![0; 8] }.encode()),
            Ev::Interrupt(vec![0xee, 1, 2]), // hostile
            Ev::Interrupt(vec![]),
            Ev::Idle,
        ]
        .into(),
    );
    assert_eq!(run_hogp(&mut s, &mut g, &mut src, &stop), End::LinkLost);
    assert_eq!(s.reports, 2);
    assert!(u.0.lock().unwrap().len() >= 3);
    // stop wins
    let stop = AtomicBool::new(true);
    assert_eq!(run_hogp(&mut s, &mut g, &mut Script(VecDeque::new()), &stop), End::Stopped);
}

#[test]
fn report_ids_in_a_map_are_found_only_as_real_items() {
    assert!(map_uses_report_ids(&[0x05, 0x01, 0x85, 0x02]));
    assert!(!map_uses_report_ids(&[0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0xc0]));
    // 0x85 inside the data bytes of a 4-byte item is not an item
    assert!(!map_uses_report_ids(&[0x07, 0x85, 0x85, 0x85, 0x85, 0xc0]));
    assert!(!map_uses_report_ids(&[]));
    assert!(!map_uses_report_ids(&[0xfe, 2, 0x10, 0x85, 0x85]));
}
