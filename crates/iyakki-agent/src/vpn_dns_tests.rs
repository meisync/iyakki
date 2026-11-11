//! The DNS forwarder's sockets, on loopback, with fake upstream servers. No mark is set (that needs administrator rights) and
//! nothing leaves the machine.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_vpn::dnsroute::{Action, Config, Event, Forwarder};
use iyakki_vpn::dnswire::{build_query, build_response, parse_response, TYPE_A};
use iyakki_vpn::filter::FilterSet;
use iyakki_vpn::net::Context;

use super::*;

struct Sink {
    f: Mutex<Forwarder>,
    routes: Mutex<Vec<IpAddr>>,
}

impl Sink {
    fn new() -> Arc<Sink> {
        let ctx = Context { cluster: vec![], lan: vec![], tunnel: vec![] };
        let mut f = Forwarder::new(Config::new(true, vec![], ctx), FilterSet::parse("*.corp.example.com").unwrap());
        let _ = f.handle(Event::TunnelState(true));
        Arc::new(Sink { f: Mutex::new(f), routes: Mutex::new(vec![]) })
    }
}

impl EventSink for Sink {
    fn event(&self, ev: Event, _now: u64) -> Vec<Action> {
        let acts = self.f.lock().unwrap().handle(ev);
        let mut io = Vec::new();
        for a in acts {
            match a {
                Action::RouteAdd { ip, .. } => self.routes.lock().unwrap().push(ip),
                Action::RouteDel { ip } => self.routes.lock().unwrap().retain(|r| *r != ip),
                Action::Note(_) => {}
                o => io.push(o),
            }
        }
        io
    }
}

/// A fake UDP DNS server that answers every A query with one address; `spoof` makes it answer from a different socket.
struct FakeUdp {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    seen: Arc<Mutex<usize>>,
    h: Option<std::thread::JoinHandle<()>>,
}

impl FakeUdp {
    fn start(answer: &str, spoof: bool) -> FakeUdp {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        s.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        let addr = s.local_addr().unwrap();
        let other = UdpSocket::bind("127.0.0.1:0").unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(0usize));
        let (st, sn) = (stop.clone(), seen.clone());
        let ip: IpAddr = answer.parse().unwrap();
        let h = std::thread::spawn(move || {
            let mut buf = [0u8; 2048];
            while !st.load(Ordering::SeqCst) {
                if let Ok((n, from)) = s.recv_from(&mut buf) {
                    *sn.lock().unwrap() += 1;
                    let r = build_response(&buf[..n], 0, &[(ip, 120)]).unwrap();
                    let _ = if spoof { other.send_to(&r, from) } else { s.send_to(&r, from) };
                }
            }
        });
        FakeUdp { addr, stop, seen, h: Some(h) }
    }
}

impl Drop for FakeUdp {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.h.take() {
            let _ = h.join();
        }
    }
}

fn ask_udp(to: SocketAddr, name: &str, id: u16) -> Option<Vec<u8>> {
    ask_udp_times(to, name, id, 1)
}

/// UDP: a busy machine (or a flood of datagrams just before) can drop or delay one question; a client asks again.
fn ask_udp_times(to: SocketAddr, name: &str, id: u16, times: usize) -> Option<Vec<u8>> {
    let c = UdpSocket::bind("127.0.0.1:0").unwrap();
    c.set_read_timeout(Some(Duration::from_millis(1500))).unwrap();
    let mut buf = [0u8; 2048];
    for _ in 0..times {
        c.send_to(&build_query(id, name, TYPE_A), to).unwrap();
        if let Ok((n, _)) = c.recv_from(&mut buf) {
            return Some(buf[..n].to_vec());
        }
    }
    None
}

fn first_addr(reply: &[u8]) -> IpAddr {
    parse_response(reply).unwrap().answers[0].ip
}

#[test]
fn nameservers_are_read_from_resolv_conf_and_odd_lines_are_skipped() {
    let t = "# c\nsearch lan\nnameserver 192.168.1.1\nnameserver fe80::1\nnameserver 192.168.1.1\nnameserver bogus\nnameserver 0.0.0.0\nnameserver 224.0.0.1\nnameserver 9.9.9.9\nnameserver 1.1.1.1\n";
    let v = parse_nameservers(t);
    assert_eq!(v.len(), 3, "{v:?}");
    assert_eq!(v[0], "192.168.1.1:53".parse().unwrap());
    assert!(parse_nameservers("").is_empty());
}

#[test]
fn the_forwarder_listens_on_loopback_only() {
    let b = Bound::bind(0).unwrap();
    assert!(b.addrs().len() >= 2);
    assert!(b.addrs().iter().all(|a| a.ip().is_loopback()), "{:?}", b.addrs());
    assert!(b.addrs().iter().all(|a| a.port() == b.port));
}

#[test]
fn a_tunnel_name_is_asked_of_the_vpn_dns_and_its_address_becomes_a_route_before_the_answer_returns() {
    let vpn = FakeUdp::start("203.0.113.77", false);
    let sys = FakeUdp::start("93.184.216.34", false);
    let sink = Sink::new();
    let b = Bound::bind(0).unwrap();
    let port = b.port;
    let run = start(b, Upstreams { system: vec![sys.addr], vpn: vec![vpn.addr] }, None, sink.clone()).unwrap();
    let to: SocketAddr = ([127, 0, 0, 1], port).into();

    let r = ask_udp(to, "app.corp.example.com", 0x1234).expect("an answer");
    assert_eq!(u16::from_be_bytes([r[0], r[1]]), 0x1234, "the program's own id comes back");
    assert_eq!(first_addr(&r), "203.0.113.77".parse::<IpAddr>().unwrap());
    assert_eq!(sink.routes.lock().unwrap().clone(), vec!["203.0.113.77".parse::<IpAddr>().unwrap()], "the route was learned");
    assert_eq!(*vpn.seen.lock().unwrap(), 1);
    assert_eq!(*sys.seen.lock().unwrap(), 0, "a tunnel name never goes to the system resolver");

    let r = ask_udp(to, "www.example.org", 0x77).expect("an answer");
    assert_eq!(first_addr(&r), "93.184.216.34".parse::<IpAddr>().unwrap());
    assert_eq!(sink.routes.lock().unwrap().len(), 1, "an ordinary name teaches no route");
    assert_eq!(*vpn.seen.lock().unwrap(), 1);
    drop(run);
}

#[test]
fn a_reply_from_somewhere_other_than_the_asked_server_is_ignored() {
    let vpn = FakeUdp::start("203.0.113.78", true);
    let sink = Sink::new();
    let b = Bound::bind(0).unwrap();
    let port = b.port;
    let run = start(b, Upstreams { system: vec![], vpn: vec![vpn.addr] }, None, sink.clone()).unwrap();
    let r = ask_udp(([127, 0, 0, 1], port).into(), "x.corp.example.com", 9);
    assert!(r.is_none(), "an answer from a stranger is dropped; the program times out instead");
    assert!(sink.routes.lock().unwrap().is_empty());
    drop(run);
}

#[test]
fn tcp_clients_are_served_and_the_upstream_is_asked_over_tcp() {
    // a fake DNS-over-TCP server
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let vpn_addr = l.local_addr().unwrap();
    let t = std::thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        let mut n = [0u8; 2];
        s.read_exact(&mut n).unwrap();
        let mut q = vec![0u8; u16::from_be_bytes(n) as usize];
        s.read_exact(&mut q).unwrap();
        let r = build_response(&q, 0, &[("203.0.113.90".parse().unwrap(), 300)]).unwrap();
        let mut out = (r.len() as u16).to_be_bytes().to_vec();
        out.extend_from_slice(&r);
        s.write_all(&out).unwrap();
    });
    let sink = Sink::new();
    let b = Bound::bind(0).unwrap();
    let port = b.port;
    let run = start(b, Upstreams { system: vec![], vpn: vec![vpn_addr] }, None, sink.clone()).unwrap();

    let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let q = build_query(0xabcd, "db.corp.example.com", TYPE_A);
    let mut out = (q.len() as u16).to_be_bytes().to_vec();
    out.extend_from_slice(&q);
    c.write_all(&out).unwrap();
    let mut n = [0u8; 2];
    c.read_exact(&mut n).unwrap();
    let mut r = vec![0u8; u16::from_be_bytes(n) as usize];
    c.read_exact(&mut r).unwrap();
    assert_eq!(u16::from_be_bytes([r[0], r[1]]), 0xabcd);
    assert_eq!(first_addr(&r), "203.0.113.90".parse::<IpAddr>().unwrap());
    assert_eq!(sink.routes.lock().unwrap().len(), 1);
    t.join().unwrap();
    drop(run);
}

#[test]
fn junk_and_oversized_tcp_input_is_dropped_and_the_loop_keeps_serving() {
    let vpn = FakeUdp::start("203.0.113.79", false);
    let sink = Sink::new();
    let b = Bound::bind(0).unwrap();
    let port = b.port;
    let run = start(b, Upstreams { system: vec![], vpn: vec![vpn.addr] }, None, sink.clone()).unwrap();
    // a length that is too large, a length that is too small, a closed connection, and a flood of short datagrams
    for len in [60000u16, 3u16] {
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.write_all(&len.to_be_bytes()).unwrap();
        c.write_all(&[0u8; 8]).unwrap();
        c.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        let mut b = [0u8; 4];
        assert!(matches!(c.read(&mut b), Ok(0) | Err(_)));
    }
    drop(TcpStream::connect(("127.0.0.1", port)).unwrap());
    let u = UdpSocket::bind("127.0.0.1:0").unwrap();
    for i in 0..300u16 {
        let _ = u.send_to(&[i as u8; 5], ("127.0.0.1", port));
        let _ = u.send_to(&[0xff; 40], ("127.0.0.1", port));
    }
    let r = ask_udp_times(([127, 0, 0, 1], port).into(), "ok.corp.example.com", 5, 3).expect("still serving");
    assert_eq!(first_addr(&r), "203.0.113.79".parse::<IpAddr>().unwrap());
    drop(run);
}

#[test]
fn dropping_the_runner_stops_the_thread_and_frees_the_port() {
    let sink = Sink::new();
    // a port below the range that bind(0) and outgoing connections use: the number is bound again after the drop, and another test must not be able to take it between
    let port = crate::testdir::free_port();
    let b = Bound::bind(port).unwrap();
    let t0 = std::time::Instant::now();
    let run = start(b, Upstreams::default(), None, sink).unwrap();
    drop(run);
    assert!(t0.elapsed() < Duration::from_secs(2), "stops within a poll interval");
    Bound::bind(port).expect("the port is free again");
}
