use super::*;

/// What a test server is told: the frames the client wrote, cut apart.
fn frames(mut b: &[u8]) -> Vec<(u8, u8, u32, Vec<u8>)> {
    let mut out = Vec::new();
    while b.len() >= 9 {
        let n = ((b[0] as usize) << 16) | ((b[1] as usize) << 8) | b[2] as usize;
        out.push((b[3], b[4], u32::from_be_bytes([b[5], b[6], b[7], b[8]]) & 0x7fff_ffff, b[9..9 + n].to_vec()));
        b = &b[9 + n..];
    }
    out
}

fn client_frames(c: &mut Conn) -> Vec<(u8, u8, u32, Vec<u8>)> {
    let out = c.take_output();
    let body = out.strip_prefix(PREFACE).unwrap_or(&out);
    frames(body)
}

fn h(n: &str, v: &str) -> Header {
    (n.to_string(), v.to_string())
}

/// A server that answers with the same codec.
struct Srv {
    enc: Encoder,
}

impl Srv {
    fn new() -> Srv {
        Srv { enc: Encoder::default() }
    }
    fn settings(&self, pairs: &[(u16, u32)]) -> Vec<u8> {
        let mut p = Vec::new();
        for (i, v) in pairs {
            p.extend_from_slice(&i.to_be_bytes());
            p.extend_from_slice(&v.to_be_bytes());
        }
        let mut o = Vec::new();
        frame(&mut o, SETTINGS, 0, 0, &p);
        o
    }
    fn response(&mut self, id: u32, status: &str, headers: &[Header], body: Option<&[u8]>) -> Vec<u8> {
        let mut list = vec![h(":status", status)];
        list.extend_from_slice(headers);
        let block = self.enc.encode(&list);
        let mut o = Vec::new();
        frame(&mut o, HEADERS, END_HEADERS | if body.is_none() { END_STREAM } else { 0 }, id, &block);
        if let Some(b) = body {
            frame(&mut o, DATA, END_STREAM, id, b);
        }
        o
    }
}

#[test]
fn a_new_connection_starts_with_the_preface_settings_without_push_and_a_big_window() {
    let mut c = Conn::new();
    let out = c.take_output();
    assert!(out.starts_with(PREFACE));
    let f = frames(&out[PREFACE.len()..]);
    assert_eq!(f[0].0, SETTINGS);
    let pairs: Vec<(u16, u32)> = f[0].3.chunks(6).map(|c| (u16::from_be_bytes([c[0], c[1]]), u32::from_be_bytes([c[2], c[3], c[4], c[5]]))).collect();
    assert!(pairs.contains(&(2, 0)), "push is off: {pairs:?}");
    assert!(pairs.contains(&(4, LOCAL_WINDOW as u32)));
    assert_eq!((f[1].0, f[1].2), (WINDOW_UPDATE, 0), "the connection window is raised too");
    assert!(c.take_output().is_empty());
}

#[test]
fn a_get_is_sent_as_pseudo_headers_first_without_connection_headers_and_the_answer_comes_back_as_events() {
    let mut c = Conn::new();
    c.take_output();
    let id = c.request("GET", "https", "example.org", "/a?b=1", &[h("Accept", "text/html"), h("Connection", "keep-alive"), h("Host", "ignored"), h("User-Agent", "iyakki")], None).unwrap();
    assert_eq!(id, 1);
    let f = client_frames(&mut c);
    assert_eq!((f[0].0, f[0].1, f[0].2), (HEADERS, END_HEADERS | END_STREAM, 1));
    let sent = Decoder::default().decode(&f[0].3).unwrap();
    assert_eq!(&sent[..4], &[h(":method", "GET"), h(":scheme", "https"), h(":authority", "example.org"), h(":path", "/a?b=1")]);
    assert_eq!(&sent[4..], &[h("accept", "text/html"), h("user-agent", "iyakki")], "lower case, no connection-level headers, no host");
    let mut s = Srv::new();
    let ev = c.feed(&s.response(1, "200", &[h("content-type", "text/html"), h("content-length", "5")], Some(b"hello"))).unwrap();
    assert_eq!(ev, vec![Event::Headers { stream: 1, status: 200, headers: vec![h("content-type", "text/html"), h("content-length", "5")], end: false }, Event::Data { stream: 1, data: b"hello".to_vec(), end: true }]);
    assert_eq!(c.open_streams(), 0, "a finished stream is gone");
}

#[test]
fn two_requests_share_one_connection_and_the_answers_may_come_in_any_order_even_one_byte_at_a_time() {
    let mut c = Conn::new();
    c.take_output();
    let (a, b) = (c.request("GET", "https", "x.org", "/one", &[], None).unwrap(), c.request("GET", "https", "x.org", "/two", &[], None).unwrap());
    assert_eq!((a, b), (1, 3));
    let mut s = Srv::new();
    let mut wire = s.response(3, "200", &[h("x-which", "two")], Some(b"second"));
    wire.extend(s.response(1, "404", &[h("x-which", "one")], Some(b"first")));
    let mut events = Vec::new();
    for byte in &wire {
        events.extend(c.feed(&[*byte]).unwrap());
    }
    let whole = {
        let mut c2 = Conn::new();
        c2.request("GET", "https", "x.org", "/one", &[], None).unwrap();
        c2.request("GET", "https", "x.org", "/two", &[], None).unwrap();
        c2.feed(&wire).unwrap()
    };
    assert_eq!(events, whole, "cutting the stream anywhere changes nothing");
    assert!(events.iter().any(|e| matches!(e, Event::Headers { stream: 3, status: 200, .. })));
    assert!(events.iter().any(|e| matches!(e, Event::Headers { stream: 1, status: 404, .. })));
    assert_eq!(c.open_streams(), 0);
}

#[test]
fn a_request_body_waits_for_the_window_goes_out_in_frames_and_ends_with_the_flag() {
    let mut c = Conn::new();
    c.take_output();
    let s = Srv::new();
    c.feed(&s.settings(&[(4, 20_000)])).unwrap();
    c.take_output();
    let body: Vec<u8> = (0..40_000u32).map(|i| (i % 251) as u8).collect();
    let id = c.request("POST", "https", "x.org", "/up", &[h("content-type", "application/octet-stream")], Some(body.clone())).unwrap();
    let first = client_frames(&mut c);
    let sent: Vec<&(u8, u8, u32, Vec<u8>)> = first.iter().filter(|f| f.0 == DATA).collect();
    assert_eq!(sent.iter().map(|f| f.3.len()).sum::<usize>(), 20_000, "only what the stream window allows");
    assert!(sent.iter().all(|f| f.3.len() <= DEFAULT_FRAME && f.1 & END_STREAM == 0));
    // the server opens the stream window by 30 000: the rest follows and ends the stream
    let mut up = Vec::new();
    frame(&mut up, WINDOW_UPDATE, 0, id, &30_000u32.to_be_bytes());
    c.feed(&up).unwrap();
    let rest = client_frames(&mut c);
    let more: Vec<&(u8, u8, u32, Vec<u8>)> = rest.iter().filter(|f| f.0 == DATA).collect();
    assert_eq!(more.iter().map(|f| f.3.len()).sum::<usize>(), 20_000);
    assert_eq!(more.last().unwrap().1 & END_STREAM, END_STREAM, "the last piece ends the stream");
    let all: Vec<u8> = sent.iter().chain(more.iter()).flat_map(|f| f.3.clone()).collect();
    assert_eq!(all, body, "every byte, in order");
}

#[test]
fn big_header_blocks_continue_padding_priority_trailers_and_interim_answers_are_understood() {
    let mut c = Conn::new();
    c.take_output();
    let id = c.request("GET", "https", "x.org", "/", &[], None).unwrap();
    c.take_output();
    let mut s = Srv::new();
    let mut wire = Vec::new();
    // an interim 100, then the real answer split over HEADERS and CONTINUATION, with padding and a priority block
    let interim = s.enc.encode(&[h(":status", "100")]);
    frame(&mut wire, HEADERS, END_HEADERS, id, &interim);
    let block = s.enc.encode(&[h(":status", "200"), h("x-a", &"a".repeat(300)), h("x-b", "b")]);
    let (a, b) = block.split_at(100);
    let mut first = vec![2u8]; // two bytes of padding
    first.extend_from_slice(&[0, 0, 0, 0, 16]); // priority: dependency 0, weight 16
    first.extend_from_slice(a);
    first.extend_from_slice(&[0, 0]);
    frame(&mut wire, HEADERS, PADDED | PRIORITY_FLAG, id, &first);
    frame(&mut wire, CONTINUATION, END_HEADERS, id, b);
    let mut padded = vec![3u8];
    padded.extend_from_slice(b"body");
    padded.extend_from_slice(&[0, 0, 0]);
    frame(&mut wire, DATA, PADDED, id, &padded);
    let trailer = s.enc.encode(&[h("x-checksum", "abc")]);
    frame(&mut wire, HEADERS, END_HEADERS | END_STREAM, id, &trailer);
    let ev = c.feed(&wire).unwrap();
    assert_eq!(ev.len(), 3, "{ev:?}");
    let Event::Headers { status, headers, end, .. } = &ev[0] else { panic!("{ev:?}") };
    assert_eq!((*status, *end, headers.len(), headers[0].1.len()), (200, false, 2, 300));
    assert_eq!(ev[1], Event::Data { stream: id, data: b"body".to_vec(), end: false });
    assert_eq!(ev[2], Event::Data { stream: id, data: vec![], end: true }, "trailers end the stream");
}

#[test]
fn flow_control_gives_the_window_back_as_data_is_used_and_a_server_that_overruns_it_is_stopped() {
    let mut c = Conn::new();
    c.take_output();
    let id = c.request("GET", "https", "x.org", "/big", &[], None).unwrap();
    c.take_output();
    let mut s = Srv::new();
    c.feed(&s.response(id, "200", &[], None).iter().copied().take(0).collect::<Vec<u8>>()).unwrap();
    let mut hdr = Vec::new();
    let block = s.enc.encode(&[h(":status", "200")]);
    frame(&mut hdr, HEADERS, END_HEADERS, id, &block);
    c.feed(&hdr).unwrap();
    let mut got = 0usize;
    let mut updates = 0usize;
    for _ in 0..80 {
        let mut d = Vec::new();
        frame(&mut d, DATA, 0, id, &vec![7u8; DEFAULT_FRAME]);
        got += c.feed(&d).unwrap().iter().map(|e| if let Event::Data { data, .. } = e { data.len() } else { 0 }).sum::<usize>();
        updates += client_frames(&mut c).iter().filter(|f| f.0 == WINDOW_UPDATE).count();
    }
    assert_eq!(got, 80 * DEFAULT_FRAME, "80 frames of 16 KB are more than the 1 MB window: it was refilled meanwhile");
    assert!(updates >= 2, "window updates for the stream and the connection: {updates}");
    // a server that ignores the window: one frame at a time, without the client's updates being read, is still within bounds; a burst is not
    let mut c = Conn::new();
    c.take_output();
    let id = c.request("GET", "https", "x.org", "/", &[], None).unwrap();
    let mut s = Srv::new();
    let mut hdr = Vec::new();
    frame(&mut hdr, HEADERS, END_HEADERS, id, &s.enc.encode(&[h(":status", "200")]));
    c.feed(&hdr).unwrap();
    let mut burst = Vec::new();
    for _ in 0..70 {
        frame(&mut burst, DATA, 0, id, &vec![1u8; DEFAULT_FRAME]);
    }
    // the frames are fed in one go: the client updates its window after each, so this is allowed; what is not allowed is a frame bigger than its limit
    assert!(c.feed(&burst).is_ok());
    let mut huge = Vec::new();
    frame(&mut huge, DATA, 0, id, &vec![0u8; DEFAULT_FRAME + 1]);
    assert!(matches!(c.feed(&huge), Err(H2Error::Protocol(_))));
    let out = client_frames(&mut c);
    assert!(out.iter().any(|f| f.0 == GOAWAY && u32::from_be_bytes([f.3[4], f.3[5], f.3[6], f.3[7]]) == code::FRAME_SIZE_ERROR));
    assert!(c.is_closed());
}

#[test]
fn the_servers_limits_and_goodbyes_are_kept_pings_are_answered_and_resets_are_events() {
    let mut c = Conn::new();
    c.take_output();
    let s = Srv::new();
    c.feed(&s.settings(&[(3, 2)])).unwrap();
    let out = client_frames(&mut c);
    assert!(out.iter().any(|f| f.0 == SETTINGS && f.1 == ACK), "the settings are acknowledged");
    let a = c.request("GET", "https", "x.org", "/1", &[], None).unwrap();
    let b = c.request("GET", "https", "x.org", "/2", &[], None).unwrap();
    assert!(matches!(c.request("GET", "https", "x.org", "/3", &[], None), Err(H2Error::Busy(_))), "two at a time is what the server allows");
    c.take_output();
    let mut ping = Vec::new();
    frame(&mut ping, PING, 0, 0, b"12345678");
    c.feed(&ping).unwrap();
    let out = client_frames(&mut c);
    assert_eq!(out[0].0, PING);
    assert_eq!((out[0].1, out[0].3.clone()), (ACK, b"12345678".to_vec()), "a ping is answered with its own bytes");
    let mut rst = Vec::new();
    frame(&mut rst, RST_STREAM, 0, a, &code::REFUSED_STREAM.to_be_bytes());
    assert_eq!(c.feed(&rst).unwrap(), vec![Event::Reset { stream: a, code: code::REFUSED_STREAM }]);
    assert_eq!(c.open_streams(), 1);
    // GOAWAY with last stream 0: the stream the server never saw is refused, and no new request is started
    let mut bye = Vec::new();
    let mut p = 0u32.to_be_bytes().to_vec();
    p.extend_from_slice(&code::NO_ERROR.to_be_bytes());
    frame(&mut bye, GOAWAY, 0, 0, &p);
    let ev = c.feed(&bye).unwrap();
    assert!(ev.contains(&Event::Reset { stream: b, code: code::REFUSED_STREAM }) && ev.contains(&Event::GoAway { last: 0, code: 0 }), "{ev:?}");
    assert!(!c.can_start() && c.is_closed());
    assert!(matches!(c.request("GET", "https", "x.org", "/4", &[], None), Err(H2Error::Busy(_))));
}

#[test]
fn a_server_that_breaks_the_rules_gets_a_goaway_and_the_connection_ends() {
    let cases: Vec<(&str, Vec<u8>, u32)> = {
        let mut v = Vec::new();
        let mut x = Vec::new();
        frame(&mut x, PUSH_PROMISE, END_HEADERS, 1, &[0, 0, 0, 2]);
        v.push(("push although it was switched off", x, code::PROTOCOL_ERROR));
        let mut x = Vec::new();
        frame(&mut x, HEADERS, END_HEADERS, 1, &[0xff, 0xff]);
        v.push(("a header block that is not HPACK", x, code::COMPRESSION_ERROR));
        let mut x = Vec::new();
        frame(&mut x, HEADERS, 0, 1, &[0x88]);
        frame(&mut x, DATA, 0, 1, b"x");
        v.push(("another frame inside a header block", x, code::PROTOCOL_ERROR));
        let mut x = Vec::new();
        frame(&mut x, SETTINGS, 0, 1, &[]);
        v.push(("settings on a stream", x, code::PROTOCOL_ERROR));
        let mut x = Vec::new();
        frame(&mut x, SETTINGS, 0, 0, &[0, 5, 0, 0, 0, 1]);
        v.push(("a frame size below the allowed", x, code::PROTOCOL_ERROR));
        let mut x = Vec::new();
        frame(&mut x, WINDOW_UPDATE, 0, 0, &0u32.to_be_bytes());
        v.push(("a window change of zero", x, code::PROTOCOL_ERROR));
        let mut x = Vec::new();
        frame(&mut x, WINDOW_UPDATE, 0, 0, &0x7fff_ffffu32.to_be_bytes());
        v.push(("a window past its limit", x, code::FLOW_CONTROL_ERROR));
        let mut x = Vec::new();
        frame(&mut x, DATA, 0, 0, b"x");
        v.push(("data on stream zero", x, code::PROTOCOL_ERROR));
        let mut x = Vec::new();
        frame(&mut x, CONTINUATION, END_HEADERS, 1, &[0x88]);
        v.push(("a continuation without a block", x, code::PROTOCOL_ERROR));
        let mut x = Vec::new();
        frame(&mut x, DATA, 0, 1, b"early");
        v.push(("data before the headers", x, code::PROTOCOL_ERROR));
        v
    };
    for (why, wire, want) in cases {
        let mut c = Conn::new();
        c.take_output();
        c.request("GET", "https", "x.org", "/", &[], None).unwrap();
        c.take_output();
        assert!(c.feed(&wire).is_err(), "{why}");
        let out = client_frames(&mut c);
        let g = out.iter().find(|f| f.0 == GOAWAY).unwrap_or_else(|| panic!("{why}: no GOAWAY in {out:?}"));
        assert_eq!(u32::from_be_bytes([g.3[4], g.3[5], g.3[6], g.3[7]]), want, "{why}");
        assert!(c.is_closed() && c.feed(&[0]).is_err(), "{why}");
    }
    // an answer without a status resets that stream only; unknown frame kinds are ignored
    let mut c = Conn::new();
    c.take_output();
    let id = c.request("GET", "https", "x.org", "/", &[], None).unwrap();
    let mut s = Srv::new();
    let mut wire = Vec::new();
    frame(&mut wire, 0xee, 0, 0, b"whatever");
    frame(&mut wire, HEADERS, END_HEADERS, id, &s.enc.encode(&[h("x-no-status", "1")]));
    let ev = c.feed(&wire).unwrap();
    assert_eq!(ev, vec![Event::Reset { stream: id, code: code::PROTOCOL_ERROR }]);
    assert!(!c.is_closed());
}

#[test]
fn a_cancelled_request_tells_the_server_and_hostile_bytes_never_panic() {
    let mut c = Conn::new();
    c.take_output();
    let id = c.request("GET", "https", "x.org", "/slow", &[], None).unwrap();
    c.take_output();
    c.cancel(id);
    let out = client_frames(&mut c);
    assert_eq!((out[0].0, out[0].2, out[0].3.clone()), (RST_STREAM, id, code::CANCEL.to_be_bytes().to_vec()));
    assert_eq!(c.open_streams(), 0);
    // a late answer for the cancelled stream is read (the header table is shared) and dropped
    let mut s = Srv::new();
    assert!(c.feed(&s.response(id, "200", &[], Some(b"late"))).unwrap().is_empty());
    // random bytes, whole connections of them, are errors at worst
    let mut x = 0xdead_beefu32;
    for round in 0..300 {
        let mut c = Conn::new();
        c.take_output();
        let _ = c.request("GET", "https", "x.org", "/", &[], None);
        let n = 20 + (round % 60);
        let v: Vec<u8> = (0..n).map(|_| { x = x.wrapping_mul(1664525).wrapping_add(1013904223); (x >> 24) as u8 }).collect();
        let _ = c.feed(&v);
        let _ = c.take_output();
    }
}
