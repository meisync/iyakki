use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::net::{self, Ask};
use super::responder::{Dest, Out, Responder, State, Svc};
use super::wire::{self, Data, Message, Question, Record};
use super::*;

fn name(n: &str) -> Vec<u8> {
    let mut v = Vec::new();
    for l in n.split('.') {
        v.push(l.len() as u8);
        v.extend_from_slice(l.as_bytes());
    }
    v.push(0);
    v
}

fn a(x: u8) -> Ipv4Addr {
    Ipv4Addr::new(192, 168, 68, x)
}

fn dev_svc() -> Svc {
    Svc { kind: DEV_SERVICE.into(), port: 7979, txt: vec![] }
}

fn from(ip: Ipv4Addr, port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(ip), port)
}

/// A responder that is past probing and announcing.
fn running(label: &str) -> (Responder, u64) {
    let mut r = Responder::new(label, 0);
    r.set_addresses(vec![a(66)], vec![], 0);
    r.set_services(vec![dev_svc()], 0);
    let mut t = 0;
    while r.state() != State::Running && t < 10_000 {
        r.poll(t);
        t += 50;
    }
    assert_eq!(r.state(), State::Running);
    (r, t)
}

fn query(qs: Vec<Question>) -> Vec<u8> {
    wire::encode(&Message::query(qs)).unwrap()
}

fn q(n: &str, t: u16) -> Question {
    Question { name: n.into(), qtype: t, unicast: false }
}

fn decode_out(o: &Out) -> Message {
    wire::decode(&o.bytes).unwrap()
}

// ------------------------------------------------------------------------------------------------------------- wire

#[test]
fn a_query_written_by_hand_after_the_rfc_layout_is_read() {
    // header: id 0, flags 0, 1 question; _ipp._tcp.local PTR, class IN with the QU bit (RFC 6762 section 5.4)
    let mut p = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    p.extend(name("_ipp._tcp.local"));
    p.extend([0, 12, 0x80, 1]);
    let m = wire::decode(&p).unwrap();
    assert!(!m.is_response());
    assert_eq!(m.questions, vec![Question { name: "_ipp._tcp.local".into(), qtype: wire::TYPE_PTR, unicast: true }]);
}

#[test]
fn a_response_with_name_compression_is_read() {
    // the answer of RFC 6762 section 20 style: PTR _ipp._tcp.local -> "Printer._ipp._tcp.local", the target pointing back into the question
    let mut p = vec![0, 0, 0x84, 0, 0, 1, 0, 2, 0, 0, 0, 1];
    p.extend(name("_ipp._tcp.local")); // at offset 12
    p.extend([0, 12, 0, 1]);
    // answer 1: name = pointer to 12, PTR, ttl 4500, rdata = "Printer" + pointer to 12
    p.extend([0xC0, 12, 0, 12, 0, 1, 0, 0, 0x11, 0x94, 0, 10, 7]);
    p.extend(b"Printer");
    p.extend([0xC0, 12]);
    // answer 2: an A record for a name made of "host" + pointer to "local" (offset of "local" inside the first name = 12 + 5 + 5 = 22)
    p.extend([4]);
    p.extend(b"host");
    p.extend([0xC0, 22, 0, 1, 0x80, 1, 0, 0, 0, 120, 0, 4, 192, 168, 1, 7]);
    // additional: a TXT with two items
    p.extend([0xC0, 12, 0, 16, 0, 1, 0, 0, 0x11, 0x94, 0, 7, 3, b'a', b'=', b'1', 2, b'b', b'c']);
    let m = wire::decode(&p).unwrap();
    assert!(m.is_response());
    assert_eq!(m.answers[0].data, Data::Ptr("printer._ipp._tcp.local".into()), "names are lower-cased");
    assert_eq!(m.answers[0].ttl, 4500);
    assert_eq!(m.answers[1].name, "host.local");
    assert!(m.answers[1].flush && m.answers[1].data == Data::A(Ipv4Addr::new(192, 168, 1, 7)));
    assert_eq!(m.additional[0].data, Data::Txt(vec!["a=1".into(), "bc".into()]));
}

#[test]
fn every_record_type_survives_encode_and_decode() {
    let m = Message {
        id: 7,
        flags: 0x8400,
        questions: vec![Question { name: "iyakki-0a1b2c.local".into(), qtype: wire::TYPE_ANY, unicast: true }],
        answers: vec![
            Record { name: "iyakki-0a1b2c.local".into(), rtype: wire::TYPE_A, flush: true, ttl: 120, data: Data::A(a(66)) },
            Record { name: "iyakki-0a1b2c.local".into(), rtype: wire::TYPE_AAAA, flush: true, ttl: 120, data: Data::Aaaa("fd00::66".parse().unwrap()) },
            Record { name: "_iyakki-dev._tcp.local".into(), rtype: wire::TYPE_PTR, flush: false, ttl: 4500, data: Data::Ptr("iyakki-0a1b2c._iyakki-dev._tcp.local".into()) },
        ],
        authority: vec![Record { name: "x.local".into(), rtype: wire::TYPE_SRV, flush: true, ttl: 120, data: Data::Srv { priority: 1, weight: 2, port: 7979, target: "iyakki-0a1b2c.local".into() } }],
        additional: vec![Record { name: "x.local".into(), rtype: wire::TYPE_TXT, flush: true, ttl: 4500, data: Data::Txt(vec!["v=1".into()]) }, Record { name: "y.local".into(), rtype: 99, flush: false, ttl: 1, data: Data::Other(vec![1, 2, 3]) }],
    };
    assert_eq!(wire::decode(&wire::encode(&m).unwrap()).unwrap(), m);
}

#[test]
fn bad_packets_are_errors_and_never_panics() {
    let mut good = vec![0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0];
    good.extend(name("iyakki-0a1b2c.local"));
    good.extend([0, 1, 0, 1, 0, 0, 0, 120, 0, 4, 10, 0, 0, 1]);
    assert!(wire::decode(&good).is_ok());
    // truncated at every length
    for cut in 0..good.len() {
        assert!(wire::decode(&good[..cut]).is_err(), "cut at {cut}");
    }
    // a pointer to itself, to a later place, and two names that point at each other (the second points forward, so it is refused)
    for bad in [[0xC0u8, 12], [0xC0, 40], [0xC0, 13]] {
        let mut p = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        p.extend(bad);
        p.extend([0, 1, 0, 1]);
        assert!(wire::decode(&p).unwrap_err().contains("pointer"), "{bad:?}");
    }
    // a long chain of names that each point back at the one before ends at the jump limit
    let mut p = vec![0, 0, 0, 0, 0, 20, 0, 0, 0, 0, 0, 0];
    p.extend([3, b'a', b'b', b'c', 0, 0, 1, 0, 1]); // question 1: the name is at offset 12
    let mut prev = 12u16;
    for _ in 1..20 {
        let here = p.len() as u16;
        p.extend([0xC0 | (prev >> 8) as u8, prev as u8, 0, 1, 0, 1]);
        prev = here;
    }
    assert!(wire::decode(&p).unwrap_err().contains("too many"));
    // a label of 64 bytes (the length byte 0x40 is a reserved label type) and a name over 255 bytes
    let mut p = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 64];
    p.extend(vec![b'a'; 64]);
    p.extend([0, 0, 1, 0, 1]);
    assert!(wire::decode(&p).is_err());
    let mut p = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    for _ in 0..10 {
        p.push(60);
        p.extend(vec![b'a'; 60]);
    }
    p.extend([0, 0, 1, 0, 1]);
    assert!(wire::decode(&p).unwrap_err().contains("too long"));
    // counts that promise more records than the packet holds or than the cap allows
    assert!(wire::decode(&[0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]).is_err());
    assert!(wire::decode(&[0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]).is_err());
    // an A record whose length is not 4 is kept as bytes, not trusted as an address
    let mut p = vec![0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0];
    p.extend([0, 1, 0, 1, 0, 0, 0, 1, 0, 2, 9, 9]);
    assert!(matches!(wire::decode(&p).unwrap().answers[0].data, Data::Other(_)));
    // a record length that runs past the end
    let mut p = vec![0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0];
    p.extend([0, 1, 0, 1, 0, 0, 0, 1, 0, 200, 9, 9]);
    assert!(wire::decode(&p).is_err());
    // the encoder refuses a label over 63 bytes and a packet over the size cap
    let long = Message::query(vec![q(&format!("{}.local", "a".repeat(64)), 1)]);
    assert!(wire::encode(&long).is_err());
    let many = Message { answers: (0..200).map(|i| Record { name: format!("h{i}.local"), rtype: 1, flush: false, ttl: 1, data: Data::A(a(1)) }).collect(), ..Default::default() };
    assert!(wire::encode(&many).is_err());
}

#[test]
fn random_and_mutated_packets_never_panic() {
    let mut seed = 0x1234_5678_9abc_def0u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let (mut r, _) = running("iyakki-0a1b2c");
    let base = wire::encode(&Message::response(vec![Record { name: "iyakki-0a1b2c.local".into(), rtype: 1, flush: true, ttl: 120, data: Data::A(a(66)) }], vec![])).unwrap();
    let base_q = query(vec![q("_iyakki-dev._tcp.local", wire::TYPE_PTR), q("iyakki-0a1b2c.local", wire::TYPE_A)]);
    for i in 0..3000u64 {
        let mut p = if i % 3 == 0 { (0..(next() % 200) as usize).map(|_| next() as u8).collect() } else if i % 3 == 1 { base.clone() } else { base_q.clone() };
        for _ in 0..(next() % 4) {
            if !p.is_empty() {
                let at = (next() as usize) % p.len();
                p[at] = next() as u8;
            }
        }
        if next() % 5 == 0 && !p.is_empty() {
            p.truncate((next() as usize) % p.len());
        }
        let _ = wire::decode(&p);
        let _ = r.on_packet(&p, from(a(20 + (i % 50) as u8), 5353), 100_000 + i * 200);
    }
    assert!(r.dropped > 0, "the broken ones were counted");
}

// ------------------------------------------------------------------------------------------------------- responder

#[test]
fn it_probes_three_times_then_announces_twice_and_then_is_quiet() {
    let mut r = Responder::new("iyakki-0a1b2c", 0);
    assert!(r.poll(10_000).is_empty(), "no address yet: nothing to probe or announce");
    r.set_addresses(vec![a(66)], vec![], 10_000);
    r.set_services(vec![dev_svc()], 10_000);
    let (mut probes, mut announces, mut t) = (0, 0, 10_000u64);
    let mut probe_times = vec![];
    while t < 20_000 {
        for o in r.poll(t) {
            assert_eq!(o.dest, Dest::Multicast);
            let m = decode_out(&o);
            if m.is_response() {
                announces += 1;
                assert!(m.answers.iter().any(|x| x.data == Data::A(a(66))) && m.answers.iter().any(|x| matches!(&x.data, Data::Ptr(p) if p == "iyakki-0a1b2c._iyakki-dev._tcp.local")));
                assert!(m.additional.iter().any(|x| matches!(&x.data, Data::Srv { port: 7979, target, .. } if target == "iyakki-0a1b2c.local")));
            } else {
                probes += 1;
                probe_times.push(t);
                assert_eq!(m.questions[0].name, "iyakki-0a1b2c.local");
                assert!(m.authority.iter().any(|x| x.data == Data::A(a(66))), "the tie-break data is in the probe");
            }
        }
        t += 10;
    }
    assert_eq!((probes, announces), (3, 2));
    assert!(probe_times.windows(2).all(|w| (240..=270).contains(&(w[1] - w[0]))), "250 ms apart: {probe_times:?}");
    assert_eq!(r.state(), State::Running);
    assert!(r.poll(30_000).is_empty());
}

#[test]
fn another_machine_that_owns_the_name_makes_this_one_take_a_suffix() {
    let mut r = Responder::new("iyakki-0a1b2c", 0);
    r.set_addresses(vec![a(66)], vec![], 0);
    r.poll(300);
    // a response from somebody else with the same name and another address
    let theirs = wire::encode(&Message::response(vec![Record { name: "iyakki-0a1b2c.local".into(), rtype: 1, flush: true, ttl: 120, data: Data::A(a(99)) }], vec![])).unwrap();
    r.on_packet(&theirs, from(a(99), 5353), 400);
    assert_eq!(r.host(), "iyakki-0a1b2c-2.local");
    assert_eq!(r.state(), State::Probing, "probing starts again for the new name");
    // and again: -3
    r.on_packet(&theirs.iter().map(|b| *b).collect::<Vec<_>>(), from(a(99), 5353), 500);
    let theirs2 = wire::encode(&Message::response(vec![Record { name: "iyakki-0a1b2c-2.local".into(), rtype: 1, flush: true, ttl: 120, data: Data::A(a(98)) }], vec![])).unwrap();
    r.on_packet(&theirs2, from(a(98), 5353), 600);
    assert_eq!(r.host(), "iyakki-0a1b2c-3.local");
    // the same address is not a conflict (a reflected copy of our own record)
    let ours = wire::encode(&Message::response(vec![Record { name: "iyakki-0a1b2c-3.local".into(), rtype: 1, flush: true, ttl: 120, data: Data::A(a(66)) }], vec![])).unwrap();
    r.on_packet(&ours, from(a(98), 5353), 700);
    assert_eq!(r.host(), "iyakki-0a1b2c-3.local");
}

#[test]
fn two_machines_probing_at_once_the_lower_data_gives_way() {
    let probe = |addr: Ipv4Addr| {
        wire::encode(&Message { questions: vec![Question { name: "iyakki-0a1b2c.local".into(), qtype: wire::TYPE_ANY, unicast: true }], authority: vec![Record { name: "iyakki-0a1b2c.local".into(), rtype: 1, flush: false, ttl: 120, data: Data::A(addr) }], ..Default::default() }).unwrap()
    };
    let mut low = Responder::new("iyakki-0a1b2c", 0);
    low.set_addresses(vec![a(10)], vec![], 0);
    low.on_packet(&probe(a(200)), from(a(200), 5353), 100);
    assert_eq!(low.host(), "iyakki-0a1b2c-2.local", "the other one has the later data: this one yields");
    let mut high = Responder::new("iyakki-0a1b2c", 0);
    high.set_addresses(vec![a(200)], vec![], 0);
    high.on_packet(&probe(a(10)), from(a(10), 5353), 100);
    assert_eq!(high.host(), "iyakki-0a1b2c.local", "this one wins and keeps the name");
}

#[test]
fn a_stranger_claiming_the_name_of_a_running_device_does_not_rename_it() {
    let (mut r, t) = running("iyakki-0a1b2c");
    let theirs = wire::encode(&Message::response(vec![Record { name: "iyakki-0a1b2c.local".into(), rtype: 1, flush: true, ttl: 120, data: Data::A(a(99)) }], vec![])).unwrap();
    r.on_packet(&theirs, from(a(99), 5353), t + 100);
    assert_eq!(r.host(), "iyakki-0a1b2c.local", "paired computers keep finding it");
    let outs = r.poll(t + 100);
    assert!(outs.iter().any(|o| decode_out(o).answers.iter().any(|x| x.data == Data::A(a(66)))), "it defends the name by announcing again");
}

#[test]
fn the_renames_have_a_limit() {
    let mut r = Responder::new("iyakki-0a1b2c", 0);
    r.set_addresses(vec![a(66)], vec![], 0);
    for i in 0..20u32 {
        let n = r.host();
        let theirs = wire::encode(&Message::response(vec![Record { name: n, rtype: 1, flush: true, ttl: 120, data: Data::A(a(99)) }], vec![])).unwrap();
        r.on_packet(&theirs, from(a(99), 5353), 100 + i as u64);
    }
    assert_eq!(r.host(), "iyakki-0a1b2c-9.local", "eight renames, then it stays");
}

#[test]
fn a_changed_address_is_announced_at_once_and_the_old_one_gets_a_goodbye() {
    let (mut r, t) = running("iyakki-0a1b2c");
    assert!(r.set_addresses(vec![a(67)], vec![], t + 10));
    assert!(!r.set_addresses(vec![a(67)], vec![], t + 11), "no change, no packet");
    let outs = r.poll(t + 10);
    let msgs: Vec<Message> = outs.iter().map(decode_out).collect();
    let bye = msgs.iter().flat_map(|m| &m.answers).find(|x| x.ttl == 0).expect("goodbye");
    assert_eq!(bye.data, Data::A(a(66)));
    let ann = msgs.iter().flat_map(|m| &m.answers).find(|x| x.ttl > 0 && matches!(x.data, Data::A(_))).expect("announcement");
    assert_eq!(ann.data, Data::A(a(67)));
    assert!(ann.flush, "cache-flush: old data is replaced in the neighbours' caches");
    // the second announcement follows a second later; then quiet
    assert!(r.poll(t + 500).is_empty());
    assert!(!r.poll(t + 1100).is_empty());
    assert!(r.poll(t + 5000).is_empty());
    assert_eq!(r.state(), State::Running);
    // a query now gets the new address only
    let outs = r.on_packet(&query(vec![q("iyakki-0a1b2c.local", wire::TYPE_A)]), from(a(20), 5353), t + 6000);
    let m = decode_out(&outs[0]);
    assert_eq!(m.answers.len(), 1);
    assert_eq!(m.answers[0].data, Data::A(a(67)));
}

#[test]
fn stopping_sends_goodbyes_for_every_record_and_then_nothing() {
    let (mut r, t) = running("iyakki-0a1b2c");
    let outs = r.stop();
    let recs: Vec<Record> = outs.iter().flat_map(|o| decode_out(o).answers).collect();
    assert_eq!(recs.len(), 4, "A, PTR, SRV, TXT");
    assert!(recs.iter().all(|x| x.ttl == 0));
    assert!(r.poll(t + 10_000).is_empty());
    assert!(r.on_packet(&query(vec![q("iyakki-0a1b2c.local", 1)]), from(a(20), 5353), t + 20_000).is_empty());
    // a responder that never got past probing has nothing to take back
    let mut p = Responder::new("iyakki-0a1b2c", 0);
    p.set_addresses(vec![a(66)], vec![], 0);
    assert!(p.stop().is_empty());
}

#[test]
fn a_service_change_is_announced_and_a_removed_service_gets_a_goodbye() {
    let (mut r, t) = running("iyakki-0a1b2c");
    // developer mode off: the service goes
    r.set_services(vec![], t + 10);
    let recs: Vec<Record> = r.poll(t + 10).iter().flat_map(|o| decode_out(o).answers).collect();
    assert!(recs.iter().filter(|x| x.ttl == 0).count() >= 3, "{recs:?}");
    let outs = r.on_packet(&query(vec![q("_iyakki-dev._tcp.local", wire::TYPE_PTR)]), from(a(20), 5353), t + 3000);
    assert!(outs.is_empty(), "not offered any more: no answer");
}

#[test]
fn only_its_own_names_are_answered_and_the_answer_has_what_a_client_needs() {
    let (mut r, t) = running("iyakki-0a1b2c");
    let src = from(a(20), 5353);
    for other in ["printer.local", "iyakki-ffffff.local", "_ipp._tcp.local", "_http._tcp.local", "iyakki-0a1b2c.example.com", "local", "_services._dns-sd._udp.local.x"] {
        assert!(r.on_packet(&query(vec![q(other, wire::TYPE_ANY)]), src, t + 10_000).is_empty(), "{other}");
    }
    // a response is never answered, and never forwarded
    assert!(r.on_packet(&wire::encode(&Message::response(vec![], vec![])).unwrap(), src, t + 10_000).is_empty());
    // a host name question: A only
    let outs = r.on_packet(&query(vec![q("IYAKKI-0A1B2C.LOCAL", wire::TYPE_A)]), src, t + 20_000);
    assert_eq!(outs.len(), 1);
    assert_eq!(outs[0].dest, Dest::Multicast);
    let m = decode_out(&outs[0]);
    assert!(m.is_response() && m.answers.len() == 1 && m.answers[0].data == Data::A(a(66)));
    // the service type: the PTR with the SRV, TXT and the address alongside
    let outs = r.on_packet(&query(vec![q("_iyakki-dev._tcp.local", wire::TYPE_PTR)]), src, t + 30_000);
    let m = decode_out(&outs[0]);
    assert!(matches!(&m.answers[0].data, Data::Ptr(p) if p == "iyakki-0a1b2c._iyakki-dev._tcp.local"));
    assert!(m.additional.iter().any(|x| matches!(x.data, Data::Srv { port: 7979, .. })) && m.additional.iter().any(|x| matches!(x.data, Data::Txt(_))) && m.additional.iter().any(|x| x.data == Data::A(a(66))));
    // the meta query lists the service types
    let outs = r.on_packet(&query(vec![q("_services._dns-sd._udp.local", wire::TYPE_PTR)]), src, t + 40_000);
    assert!(matches!(&decode_out(&outs[0]).answers[0].data, Data::Ptr(p) if p == "_iyakki-dev._tcp.local"));
    // the TXT does not carry a name of a person
    let txt = decode_out(&r.on_packet(&query(vec![q("iyakki-0a1b2c._iyakki-dev._tcp.local", wire::TYPE_TXT)]), src, t + 50_000)[0]).answers[0].data.clone();
    assert_eq!(txt, Data::Txt(vec!["v=1".into()]));
}

#[test]
fn the_asker_that_already_knows_the_answer_gets_none_and_one_that_asks_by_unicast_gets_it_alone() {
    let (mut r, t) = running("iyakki-0a1b2c");
    let src = from(a(20), 5353);
    let known = Record { name: "_iyakki-dev._tcp.local".into(), rtype: wire::TYPE_PTR, flush: false, ttl: 4000, data: Data::Ptr("iyakki-0a1b2c._iyakki-dev._tcp.local".into()) };
    let asking = Message { questions: vec![q("_iyakki-dev._tcp.local", wire::TYPE_PTR)], answers: vec![known.clone()], ..Default::default() };
    assert!(r.on_packet(&wire::encode(&asking).unwrap(), src, t + 10_000).is_empty());
    let stale = Message { answers: vec![Record { ttl: 100, ..known }], ..asking };
    assert!(!r.on_packet(&wire::encode(&stale).unwrap(), src, t + 20_000).is_empty(), "a nearly expired one is sent again");
    // QU bit: unicast to the asker
    let qu = wire::encode(&Message::query(vec![Question { name: "iyakki-0a1b2c.local".into(), qtype: 1, unicast: true }])).unwrap();
    let outs = r.on_packet(&qu, src, t + 30_000);
    assert_eq!(outs[0].dest, Dest::Unicast(src));
}

#[test]
fn a_legacy_unicast_query_is_answered_to_its_port_with_its_id_and_a_short_life() {
    let (mut r, t) = running("iyakki-0a1b2c");
    let src = from(a(20), 51234);
    let mut m = Message::query(vec![q("iyakki-0a1b2c.local", wire::TYPE_A)]);
    m.id = 0x4242;
    let outs = r.on_packet(&wire::encode(&m).unwrap(), src, t + 10);
    assert_eq!(outs[0].dest, Dest::Unicast(src));
    let ans = decode_out(&outs[0]);
    assert_eq!(ans.id, 0x4242);
    assert_eq!(ans.questions, m.questions, "the question is repeated");
    assert!(ans.answers[0].ttl <= 10 && !ans.answers[0].flush);
}

#[test]
fn a_flood_of_queries_is_cut_off_per_source_and_in_all() {
    let (mut r, t) = running("iyakki-0a1b2c");
    let qu = wire::encode(&Message::query(vec![Question { name: "iyakki-0a1b2c.local".into(), qtype: 1, unicast: true }])).unwrap();
    let now = t + 100_000;
    let answered: usize = (0..100).map(|_| r.on_packet(&qu, from(a(20), 5353), now).len()).sum();
    assert_eq!(answered, 10, "ten a second from one source");
    assert!(r.rate_limited >= 90);
    // many sources at the same moment: 40 responses per 10 seconds in all
    let mut total = answered;
    for i in 0..100u8 {
        total += r.on_packet(&qu, from(Ipv4Addr::new(192, 168, 68, 100 + i), 5353), now + 1).len();
    }
    assert_eq!(total, 40);
    // a second later, the budget of the window is still spent; after ten seconds it is back
    assert!(r.on_packet(&qu, from(a(21), 5353), now + 2000).is_empty());
    assert!(!r.on_packet(&qu, from(a(21), 5353), now + 10_500).is_empty());
    // the same multicast record is not sent twice within a second
    let mq = query(vec![q("iyakki-0a1b2c.local", 1)]);
    let t2 = now + 60_000;
    assert_eq!(r.on_packet(&mq, from(a(30), 5353), t2).len(), 1);
    assert!(r.on_packet(&mq, from(a(31), 5353), t2 + 500).is_empty());
    assert_eq!(r.on_packet(&mq, from(a(32), 5353), t2 + 1500).len(), 1);
    // big queries are not worth the work
    let mut big = query(vec![q("iyakki-0a1b2c.local", 1)]);
    big.resize(3000, 0);
    assert!(r.on_packet(&big, from(a(40), 5353), t2 + 5000).is_empty());
}

#[test]
fn the_name_is_random_and_never_made_from_a_person_or_a_host_name() {
    let l = label_from([0x0a, 0x1b, 0x2c]);
    assert_eq!(l, "iyakki-0a1b2c");
    assert!(is_device_label(&l) && !is_device_label("iyakki-alice1") && !is_device_label("laptop") && !is_device_label("iyakki-0a1b2c-2"));
    assert!(is_local_name("iyakki-0a1b2c.local") && is_local_name("Box.LOCAL.") && !is_local_name("192.168.1.2") && !is_local_name("example.com") && !is_local_name(".local") && !is_local_name("a b.local"));
}

// ------------------------------------------------------------------------------------------ resolver on loopback

/// A responder behind a real UDP socket on 127.0.0.1 (the stand-in for a device on the network).
struct Fake {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Fake {
    fn start(label: &str, v4: Ipv4Addr, svcs: Vec<Svc>) -> Fake {
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
        let addr = sock.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let (s2, label) = (stop.clone(), label.to_string());
        let thread = std::thread::spawn(move || {
            let mut r = Responder::new(&label, 0);
            r.set_addresses(vec![v4], vec![], 0);
            r.set_services(svcs, 0);
            let mut t = 0;
            while r.state() != State::Running {
                r.poll(t);
                t += 100;
            }
            let mut buf = vec![0u8; 9000];
            while !s2.load(Ordering::SeqCst) {
                if let Ok((n, from)) = sock.recv_from(&mut buf) {
                    t += 1000;
                    for o in r.on_packet(&buf[..n], from, t) {
                        if let Dest::Unicast(to) = o.dest {
                            let _ = sock.send_to(&o.bytes, to);
                        }
                    }
                }
            }
        });
        Fake { addr, stop, thread: Some(thread) }
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn ask_fake(f: &Fake) -> Ask {
    Ask { timeout: Duration::from_millis(600), dests: vec![f.addr], allow_loopback: true }
}

#[test]
fn the_resolver_finds_a_name_and_the_scan_lists_the_device_with_its_services() {
    let f = Fake::start("iyakki-0a1b2c", Ipv4Addr::new(192, 168, 68, 66), vec![dev_svc(), Svc { kind: WEB_SERVICE.into(), port: 8443, txt: vec![] }]);
    let ips = net::resolve("IYAKKI-0A1B2C.local", &ask_fake(&f));
    assert_eq!(ips, vec![IpAddr::V4(Ipv4Addr::new(192, 168, 68, 66))]);
    assert!(net::resolve("someone-else.local", &ask_fake(&f)).is_empty(), "other names are not answered");
    let found = net::scan(&ask_fake(&f));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].host, "iyakki-0a1b2c.local");
    assert_eq!(found[0].addrs, vec![IpAddr::V4(Ipv4Addr::new(192, 168, 68, 66))]);
    assert_eq!(found[0].services, vec![("_iyakki-dev._tcp".to_string(), 7979), ("_iyakki-web._tcp".to_string(), 8443)]);
}

#[test]
fn answers_with_public_addresses_or_from_the_wrong_place_are_thrown_away() {
    // a device that claims a public address: nothing is returned (the local network only)
    let f = Fake::start("iyakki-0a1b2c", Ipv4Addr::new(8, 8, 8, 8), vec![dev_svc()]);
    assert!(net::resolve("iyakki-0a1b2c.local", &ask_fake(&f)).is_empty());
    assert!(net::scan(&ask_fake(&f)).iter().all(|d| d.addrs.is_empty()));
    // without allow_loopback even a private address from 127.0.0.1 is refused (the sender must be on the local network)
    let g = Fake::start("iyakki-0a1b2c", Ipv4Addr::new(192, 168, 68, 66), vec![dev_svc()]);
    let strict = Ask { allow_loopback: false, ..ask_fake(&g) };
    assert!(net::resolve("iyakki-0a1b2c.local", &strict).is_empty());
    // nobody there: an empty answer after the time, not an error
    let t = std::time::Instant::now();
    let none = Ask { timeout: Duration::from_millis(200), dests: vec!["127.0.0.1:9".parse().unwrap()], allow_loopback: true };
    assert!(net::resolve("iyakki-0a1b2c.local", &none).is_empty() && t.elapsed() < Duration::from_secs(2));
}

#[test]
fn private_and_unique_local_addresses_are_the_ones_announced() {
    assert!(net::is_ula("fd12:3456::1".parse::<Ipv6Addr>().unwrap()) && net::is_ula("fc00::1".parse::<Ipv6Addr>().unwrap()));
    assert!(!net::is_ula("fe80::1".parse::<Ipv6Addr>().unwrap()) && !net::is_ula("2001:db8::1".parse::<Ipv6Addr>().unwrap()) && !net::is_ula("::1".parse::<Ipv6Addr>().unwrap()));
    let (v4, v6) = net::interfaces();
    assert!(v4.iter().all(|i| crate::net::is_private(i.v4) && !i.iface.starts_with("lo")) && v6.iter().all(|a| net::is_ula(*a)), "{v4:?}");
    let nets = vec![net::IfAddr { iface: "eth0".into(), v4: a(66), prefix: 24 }];
    assert!(net::on_link(IpAddr::V4(a(5)), &nets) && !net::on_link(IpAddr::V4(Ipv4Addr::new(192, 168, 69, 5)), &nets) && !net::on_link(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), &nets));
}

#[test]
fn the_announcer_runs_on_a_test_port_and_stops_with_goodbyes() {
    // port 0 would not be 5353: use a high port; the machine may have no private interface (then it announces nothing but still runs)
    let mut an = net::Announcer::start_on("iyakki-0a1b2c", vec![dev_svc()], 45353).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let st = an.status();
    assert!(st.running && st.host == "iyakki-0a1b2c.local");
    assert!(st.v4.iter().all(|a| crate::net::is_private(*a)), "only private addresses: {st:?}");
    an.stop();
    assert!(!an.status().running);
}

// ------------------------------------------------------------------------------------------------------------- extra names

#[test]
fn an_extra_local_name_is_answered_with_the_device_address_and_goodbyes_follow_its_removal() {
    let (mut r, t) = running("iyakki-0a1b2c");
    // only well-formed `.local` names other than the device's own are kept
    assert!(r.set_extra_names(vec!["Store.local".into(), "iyakki-0a1b2c.local".into(), "example.org".into(), "*.local".into(), "bad..local".into(), ".local".into()], t));
    assert_eq!(r.extra_names(), ["store.local"]);
    assert!(!r.set_extra_names(vec!["store.local".into()], t), "the same list is no change");
    // it is announced (twice, a second apart) with the device's address
    let mut announced = Vec::new();
    let mut now = t;
    for _ in 0..60 {
        for o in r.poll(now) {
            announced.push(decode_out(&o));
        }
        now += 50;
    }
    assert!(announced.iter().any(|m| m.answers.iter().any(|x| x.name == "store.local" && x.data == Data::A(a(66)))), "{announced:?}");
    // a question for it is answered, by the A record, with the device address
    let outs = r.on_packet(&query(vec![q("store.local", wire::TYPE_A)]), from(a(5), 5353), now + 5000);
    assert_eq!(outs.len(), 1);
    let m = decode_out(&outs[0]);
    assert_eq!(m.answers.len(), 1);
    assert_eq!((m.answers[0].name.as_str(), &m.answers[0].data), ("store.local", &Data::A(a(66))));
    // a name that is not ours is still ignored in silence
    assert!(r.on_packet(&query(vec![q("other.local", wire::TYPE_A)]), from(a(5), 5353), now + 6000).is_empty());
    // removing the name sends a goodbye (time to live 0) for it
    assert!(r.set_extra_names(vec![], now + 7000));
    let mut bye = Vec::new();
    for k in 0..4 {
        for o in r.poll(now + 7000 + k * 50) {
            bye.push(decode_out(&o));
        }
    }
    assert!(bye.iter().any(|m| m.answers.iter().any(|x| x.name == "store.local" && x.ttl == 0)), "{bye:?}");
    assert!(r.on_packet(&query(vec![q("store.local", wire::TYPE_A)]), from(a(6), 5353), now + 20_000).is_empty(), "no longer answered");
}

#[test]
fn stopping_gives_a_goodbye_for_the_extra_names_too() {
    let (mut r, t) = running("iyakki-0a1b2c");
    r.set_extra_names(vec!["store.local".into()], t);
    let bye: Vec<Message> = r.stop().iter().map(decode_out).collect();
    assert!(bye.iter().flat_map(|m| &m.answers).any(|x| x.name == "store.local" && x.ttl == 0));
}
