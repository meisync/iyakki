//! Internet sharing against a made-up machine: the state machine, the plan, the firewall table as golden text, the sockets with fakes
//! and on loopback, the files, and the command line.

use super::control::{self, Backend, MemBackend};
use super::fake::{FakeOs, RecordingMaker};
use super::io::{self, Ports};
use super::manager::{Inbound, Manager};
use super::plan::{make_plan, resolvers_from};
use super::rules::{describe, table_spec};
use super::*;
use iyakki_hotspot::dhcp;
use iyakki_hotspot::subnet::Cidr;
use std::net::{Ipv4Addr, SocketAddrV4};

fn cidr(s: &str) -> Cidr {
    Cidr::parse(s).unwrap()
}

/// eth0 is the uplink (192.168.1.5/24 with a default route); enx0 is a bare USB/Ethernet adapter.
fn machine() -> Vec<IfaceInfo> {
    let mut up = IfaceInfo::new("eth0", LinkKind::Ethernet);
    up.cidrs = vec![cidr("192.168.1.5/24")];
    up.default_route = true;
    vec![up, IfaceInfo::new("enx0", LinkKind::Usb)]
}

fn start_req(id: u64) -> Request {
    Request { id, on: true, via: Some("enx0".into()), ..Request::default() }
}

fn stop_req(id: u64) -> Request {
    Request { id, on: false, ..Request::default() }
}

fn pristine(os: &FakeOs) {
    let s = os.st();
    assert_eq!(s.forward, "0", "ip_forward is back to what it was");
    assert!(s.rules.is_none(), "the table is gone");
    assert!(s.zone.is_empty(), "the firewall zone is gone");
    assert!(s.addrs.is_empty(), "the address is gone");
    assert!(s.saved.is_none(), "the crash record is gone");
}

fn client_mac() -> [u8; 6] {
    [2, 0, 0, 0, 0, 7]
}

fn join(os: &FakeOs, name: &str) {
    let io = os.io();
    let mut g = io.lock().unwrap();
    g.inbound.push(Inbound::Dhcp(dhcp::client_packet(dhcp::DISCOVER, client_mac(), 1, Ipv4Addr::UNSPECIFIED, None, None, Some(name))));
}

fn request_address(m: &mut Manager, os: &mut FakeOs, req: &Request, name: &str) -> Ipv4Addr {
    join(os, name);
    m.tick(os, req);
    let offer = os.io().lock().unwrap().dhcp_out.last().cloned().expect("an offer");
    let (_, ip) = dhcp::reply_summary(&offer.0).unwrap();
    os.io().lock().unwrap().inbound.push(Inbound::Dhcp(dhcp::client_packet(dhcp::REQUEST, client_mac(), 1, Ipv4Addr::UNSPECIFIED, Some(ip), Some(Ipv4Addr::new(10, 42, 0, 1)), Some(name))));
    m.tick(os, req);
    ip
}

// ------------------------------------------------------------------ state machine

#[test]
fn start_sets_everything_up_in_order_and_stop_puts_it_all_back() {
    let mut os = FakeOs::new(machine());
    let mut m = Manager::new();
    let st = m.tick(&mut os, &start_req(1));
    assert_eq!(st.state, State::On, "{}", st.note);
    assert_eq!((st.uplink.as_str(), st.downstream.as_str(), st.subnet.as_str(), st.gateway.as_str()), ("eth0", "enx0", "10.42.0.0/24", "10.42.0.1"));
    assert!(st.isolate && st.idle_hours == 2);
    {
        let s = os.st();
        assert_eq!(s.calls, ["up enx0", "addr+ enx0 10.42.0.1/24", "io enx0 10.42.0.1", "rules+ iyakki_share", "zone enx0", "forward=1"], "forwarding is switched on last, after the rules");
        assert_eq!(s.forward, "1");
        assert_eq!(s.zone, ["enx0"]);
        assert_eq!(s.saved.as_ref().unwrap().previous_forward, "0");
    }
    let st = m.tick(&mut os, &stop_req(2));
    assert_eq!(st.state, State::Off);
    pristine(&os);
    assert_eq!(&os.st().calls[6..], ["forward=0", "rules-", "zone ", "addr- enx0 10.42.0.1/24"], "undone in reverse order");
    assert!(os.io().lock().unwrap().closed, "the sockets are closed");
    let a = os.st().audit.join("\n");
    assert!(a.contains("started") && a.contains("stopped"), "{a}");
}

#[test]
fn forwarding_that_was_already_on_stays_on_after_stop() {
    let mut os = FakeOs::new(machine());
    os.st().forward = "1".into();
    let mut m = Manager::new();
    m.tick(&mut os, &start_req(1));
    m.tick(&mut os, &stop_req(2));
    assert_eq!(os.st().forward, "1");
}

#[test]
fn the_subnet_avoids_the_uplink_and_every_other_link() {
    let mut ifs = machine();
    ifs[0].cidrs = vec![cidr("10.42.0.77/24")];
    let mut vpn = IfaceInfo::new("wg0", LinkKind::Other);
    vpn.cidrs = vec![cidr("192.168.43.2/24")];
    ifs.push(vpn);
    let p = make_plan(&ifs, &start_req(1), false, &[]).unwrap();
    assert_eq!(p.net.net, cidr("192.168.137.0/24"));
    assert_eq!(p.gateway(), Ipv4Addr::new(192, 168, 137, 1));
}

#[test]
fn an_uplink_that_takes_our_network_later_stops_sharing_with_one_clear_line() {
    let mut os = FakeOs::new(machine());
    let mut m = Manager::new();
    m.tick(&mut os, &start_req(1));
    os.st().ifs[0].cidrs.push(cidr("10.42.0.9/24"));
    os.advance_secs(3);
    let st = m.tick(&mut os, &start_req(1));
    assert_eq!(st.state, State::Failed);
    assert!(st.note.contains("No private network is free"), "{}", st.note);
    pristine(&os);
}

#[test]
fn losing_the_uplink_tears_everything_down() {
    let mut os = FakeOs::new(machine());
    let mut m = Manager::new();
    m.tick(&mut os, &start_req(1));
    os.st().ifs[0].default_route = false;
    os.advance_secs(3);
    let st = m.tick(&mut os, &start_req(1));
    assert_eq!(st.state, State::Failed);
    assert_eq!(st.note, "Sharing stopped: eth0 lost its route to the internet.");
    pristine(&os);
    // it does not come back by itself when the route returns; the person starts it again
    os.st().ifs[0].default_route = true;
    os.advance_secs(3);
    assert_eq!(m.tick(&mut os, &start_req(1)).state, State::Failed);
    assert_eq!(m.tick(&mut os, &start_req(2)).state, State::On);
}

#[test]
fn unplugging_the_shared_device_tears_everything_down() {
    let mut os = FakeOs::new(machine());
    let mut m = Manager::new();
    m.tick(&mut os, &start_req(1));
    os.st().ifs.retain(|i| i.name != "enx0");
    os.advance_secs(3);
    let st = m.tick(&mut os, &start_req(1));
    assert_eq!(st.state, State::Failed);
    assert!(st.note.contains("enx0 disappeared"), "{}", st.note);
    assert!(os.st().rules.is_none() && os.st().forward == "0");
}

#[test]
fn nobody_using_it_switches_it_off_after_the_chosen_hours_and_use_keeps_it_on() {
    let mut os = FakeOs::new(machine());
    let mut m = Manager::new();
    let mut req = start_req(1);
    req.idle_hours = 1;
    m.tick(&mut os, &req);
    os.advance_secs(1800);
    assert_eq!(m.tick(&mut os, &req).state, State::On);
    // traffic on the cable counts as use
    os.st().ifs[1].rx_bytes += 1_000_000;
    os.advance_secs(2000);
    assert_eq!(m.tick(&mut os, &req).state, State::On, "bytes moved 33 minutes ago start the hour again");
    os.advance_secs(1000);
    let st = m.tick(&mut os, &req);
    assert_eq!(st.state, State::On);
    assert!(st.off_in_secs.unwrap() < 3600);
    os.advance_secs(3600);
    let st = m.tick(&mut os, &req);
    assert_eq!(st.state, State::Off);
    assert!(st.note.contains("switched itself off after 1 h"), "{}", st.note);
    pristine(&os);
    assert!(os.st().audit.iter().any(|l| l.contains("stopped (idle)")));
    // an off request that is still on file does not start it again
    assert_eq!(m.tick(&mut os, &req).state, State::Off);
}

#[test]
fn a_device_that_holds_an_address_keeps_sharing_on_and_shows_in_the_list() {
    let mut os = FakeOs::new(machine());
    let mut m = Manager::new();
    let req = start_req(1);
    m.tick(&mut os, &req);
    let ip = request_address(&mut m, &mut os, &req, "laptop-2");
    assert!(ip >= Ipv4Addr::new(10, 42, 0, 10) && ip <= Ipv4Addr::new(10, 42, 0, 25));
    os.advance_secs(2);
    let st = m.tick(&mut os, &req);
    assert_eq!(st.clients.len(), 1);
    assert_eq!((st.clients[0].name.as_str(), st.clients[0].ip.as_str(), st.clients[0].mac.as_str()), ("laptop-2", ip.to_string().as_str(), "02:00:00:00:00:07"));
    assert!(st.clients[0].expires_in > 3000 && st.clients[0].expires_in <= 3600);
    assert!(os.st().audit.iter().any(|l| l.contains("device joined 02:00:00:00:00:07")));
    // two hours later the lease (one hour) has ended and nothing moved: it switches off
    os.advance_secs(3 * 3600);
    let st = m.tick(&mut os, &req);
    assert_eq!(st.state, State::Off, "{st:?}");
    assert!(os.st().audit.iter().any(|l| l.contains("device left")));
}

#[test]
fn a_crash_leaves_a_record_and_the_next_start_cleans_up_only_what_is_ours() {
    let mut os = FakeOs::new(machine());
    let mut m = Manager::new();
    m.tick(&mut os, &start_req(1));
    // the process dies here: a new manager finds the table, the zone, the address, forwarding on and the record
    drop(m);
    assert_eq!(os.st().forward, "1");
    let mut m2 = Manager::new();
    m2.recover(&mut os);
    pristine(&os);
    assert!(os.st().audit.iter().any(|l| l.contains("cleaned up")));
    // with nothing left behind, recover touches nothing
    let before = os.st().calls.len();
    m2.recover(&mut os);
    assert_eq!(os.st().calls.len(), before);
}

#[test]
fn a_stale_table_is_removed_before_a_new_session() {
    let mut os = FakeOs::new(machine());
    os.st().rules = Some(table_spec(&make_plan(&machine(), &start_req(1), false, &[]).unwrap()));
    let mut m = Manager::new();
    m.tick(&mut os, &start_req(1));
    assert_eq!(os.st().calls[0], "rules-");
}

#[test]
fn a_failed_start_undoes_the_steps_already_done_and_is_not_retried_until_the_request_changes() {
    for step in ["addr", "io", "rules", "forward"] {
        let mut os = FakeOs::new(machine());
        os.st().fail = Some(step);
        let mut m = Manager::new();
        let st = m.tick(&mut os, &start_req(1));
        assert_eq!(st.state, State::Failed, "{step}");
        assert!(!st.note.is_empty());
        pristine(&os);
        let n = os.st().calls.len();
        m.tick(&mut os, &start_req(1));
        assert_eq!(os.st().calls.len(), n, "{step}: no retry loop");
        os.st().fail = None;
        assert_eq!(m.tick(&mut os, &start_req(2)).state, State::On, "{step}: a new request tries again");
    }
}

#[test]
fn every_refusal_is_one_plain_line() {
    let ask = |ifs: Vec<IfaceInfo>, r: Request| {
        let mut os = FakeOs::new(ifs);
        let mut m = Manager::new();
        let st = m.tick(&mut os, &r);
        assert_eq!(st.state, State::Failed);
        assert!(os.st().calls.is_empty(), "nothing was touched");
        st.note
    };
    let mut no_route = machine();
    no_route[0].default_route = false;
    assert!(ask(no_route, start_req(1)).starts_with("There is no internet to share"));
    let mut busy = machine();
    busy[1].cidrs = vec![cidr("172.30.0.4/24")];
    assert!(ask(busy, start_req(1)).contains("enx0 is busy"));
    assert!(ask(machine(), Request { uplink: Some("eth0".into()), via: Some("eth0".into()), ..start_req(1) }).contains("two different devices"));
    assert!(ask(machine(), Request { via: Some("nope0".into()), ..start_req(1) }).contains("no network device called nope0"));
    assert!(ask(machine(), Request { idle_hours: 100, ..start_req(1) }).contains("between 1 and 72"));
    assert!(ask(machine(), Request { via: Some("lo".into()), ..start_req(1) }).contains("no network device"));
    let mut wifi = machine();
    wifi.push(IfaceInfo::new("wlan1", LinkKind::Wifi));
    assert_eq!(ask(wifi, Request { via: Some("wlan1".into()), ..start_req(1) }), "Wi-Fi hotspot: not available on this build.");
    let mut two = machine();
    two.push(IfaceInfo::new("enx1", LinkKind::Ethernet));
    assert!(ask(two, Request { via: None, ..start_req(1) }).contains("choose one"));
    assert!(ask(vec![machine().remove(0)], Request { via: None, ..start_req(1) }).contains("plug in an Ethernet"));
    let mut loopback = machine();
    loopback.push(IfaceInfo::new("docker0", LinkKind::Other));
    assert!(ask(loopback, Request { via: Some("docker0".into()), ..start_req(1) }).contains("cannot be used to share"));
}

#[test]
fn the_only_free_cable_is_chosen_when_none_is_named_and_the_uplink_is_never_the_cable() {
    let p = make_plan(&machine(), &Request { via: None, ..start_req(1) }, false, &[]).unwrap();
    assert_eq!((p.uplink.as_str(), p.down.as_str()), ("eth0", "enx0"));
    // a USB uplink with the route is not offered as the downstream
    let mut ifs = machine();
    ifs[0].kind = LinkKind::Usb;
    let p = make_plan(&ifs, &Request { via: None, ..start_req(1) }, false, &[]).unwrap();
    assert_eq!(p.down, "enx0");
    assert!(make_plan(&ifs, &Request { uplink: Some("enx0".into()), via: Some("enx0".into()), ..start_req(1) }, false, &[]).is_err());
}

#[test]
fn a_changed_request_restarts_the_session_with_the_new_settings() {
    let mut os = FakeOs::new(machine());
    let mut m = Manager::new();
    m.tick(&mut os, &start_req(1));
    let st = m.tick(&mut os, &Request { idle_hours: 5, isolate: false, ..start_req(2) });
    assert_eq!((st.state, st.idle_hours, st.isolate), (State::On, 5, false));
    assert_eq!(os.st().opened.len(), 2);
    assert!(os.st().rules.as_ref().unwrap().chains[1].rules.iter().all(|r| r.len() != 3 || !format!("{r:?}").contains("IifIs(Exact(\"enx0\")), OifIs(Exact(\"enx0\"))")), "isolation rule gone");
}

#[test]
fn dns_queries_are_forwarded_answers_matched_and_the_gateway_name_is_answered_here() {
    let mut os = FakeOs::new(machine());
    os.st().resolvers = vec![Ipv4Addr::new(192, 168, 1, 1)];
    let mut m = Manager::new();
    let req = start_req(1);
    m.tick(&mut os, &req);
    let q = iyakki_hotspot::dns::build_query(0xbeef, "example.org", 1);
    let own = iyakki_hotspot::dns::build_query(0x0102, GATEWAY_NAME, 1);
    let io = os.io();
    io.lock().unwrap().inbound.push(Inbound::DnsQuery { src: Ipv4Addr::new(10, 42, 0, 10), sport: 5353, data: q });
    io.lock().unwrap().inbound.push(Inbound::DnsQuery { src: Ipv4Addr::new(10, 42, 0, 10), sport: 5354, data: own });
    // from outside the network: dropped
    io.lock().unwrap().inbound.push(Inbound::DnsQuery { src: Ipv4Addr::new(192, 168, 1, 99), sport: 5353, data: iyakki_hotspot::dns::build_query(1, "x.org", 1) });
    m.tick(&mut os, &req);
    let (up, own_answers) = {
        let g = io.lock().unwrap();
        (g.dns_up.clone(), g.dns_client.clone())
    };
    assert_eq!(up.len(), 1);
    assert_eq!(u16::from_be_bytes([up[0][0], up[0][1]]), 0x1234, "the resolver sees a fresh id, not the client's");
    assert_eq!(own_answers.len(), 1);
    assert_eq!(own_answers[0].1, 5354);
    assert!(own_answers[0].2.ends_with(&[10, 42, 0, 1]), "iyakki.lan is the gateway");
    let mut answer = up[0].clone();
    answer[2] |= 0x80;
    io.lock().unwrap().inbound.push(Inbound::DnsAnswer(answer));
    m.tick(&mut os, &req);
    let g = io.lock().unwrap();
    let back = g.dns_client.iter().find(|c| c.1 == 5353).expect("the answer went back to the client");
    assert_eq!(u16::from_be_bytes([back.2[0], back.2[1]]), 0xbeef, "the client's own id is restored");
}

#[test]
fn the_audit_lines_never_hold_anything_secret_and_the_session_record_holds_no_secret() {
    let mut os = FakeOs::new(machine());
    let mut m = Manager::new();
    let req = start_req(1);
    m.tick(&mut os, &req);
    request_address(&mut m, &mut os, &req, "phone");
    m.tick(&mut os, &stop_req(2));
    let all = os.st().audit.join("\n").to_lowercase();
    for bad in ["passphrase", "password", "key", "token", "secret"] {
        assert!(!all.contains(bad), "{bad} in {all}");
    }
    let rec = serde_json::to_string(&Saved { downstream: "enx0".into(), address: Ipv4Addr::new(10, 42, 0, 1), prefix: 24, previous_forward: "0".into() }).unwrap();
    assert!(!rec.to_lowercase().contains("pass"));
}

// ------------------------------------------------------------------ firewall

const GOLDEN: &str = "\
table inet iyakki_share
  chain input filter hook input priority -10 policy accept
    iif enx0 udp dport 67 accept
    iif enx0 udp dport 53 daddr 10.42.0.1/32 accept
    iif enx0 drop
  chain forward filter hook forward priority -10 policy accept
    iif eth0 oif enx0 ct state established,related accept
    iif enx0 oif enx0 drop
    iif enx0 oif eth0 daddr 10.0.0.0/8 drop
    iif enx0 oif eth0 daddr 172.16.0.0/12 drop
    iif enx0 oif eth0 daddr 192.168.0.0/16 drop
    iif enx0 oif eth0 daddr 169.254.0.0/16 drop
    iif enx0 oif eth0 daddr 100.64.0.0/10 drop
    iif enx0 oif eth0 saddr 10.42.0.0/24 accept
    iif enx0 drop
    oif enx0 drop
  chain postrouting nat hook postrouting priority 100 policy accept
    oif eth0 saddr 10.42.0.0/24 masquerade
";

fn plan_for(r: &Request) -> super::plan::Plan {
    let mut ifs = machine();
    ifs[0].cidrs = vec![cidr("192.168.1.5/24")];
    make_plan(&ifs, r, false, &[]).unwrap()
}

#[test]
fn the_firewall_table_is_exactly_this() {
    assert_eq!(describe(&table_spec(&plan_for(&start_req(1)))), GOLDEN);
}

#[test]
fn isolation_and_the_local_network_switch_change_only_their_own_rules() {
    let open = describe(&table_spec(&plan_for(&Request { isolate: false, allow_lan: true, ..start_req(1) })));
    assert!(!open.contains("iif enx0 oif enx0 drop"));
    assert!(!open.contains("daddr 10.0.0.0/8 drop"));
    assert!(open.contains("iif enx0 oif eth0 saddr 10.42.0.0/24 accept") && open.contains("iif enx0 drop\n    oif enx0 drop"), "the closing drops stay");
}

#[test]
fn the_table_encodes_with_the_typed_writer_and_removes_only_its_own_table() {
    let spec = table_spec(&plan_for(&start_req(1)));
    let (bytes, acks) = iyakki_fw::nl::tablegen::encode_apply(&spec, 1).unwrap();
    assert!(bytes.windows(12).any(|w| w == b"iyakki_share"));
    assert_eq!(acks, 3 + 3 + (3 + 10 + 1));
    assert!(iyakki_fw::nl::tablegen::encode_remove(TABLE, 1).is_ok());
    assert!(iyakki_fw::nl::tablegen::encode_remove("iyakki_fw", 1).is_err(), "never the firewall's own table");
    let mut bad = plan_for(&start_req(1));
    bad.down = "a\"b".into();
    assert!(iyakki_fw::nl::tablegen::encode_apply(&table_spec(&bad), 1).is_err(), "an odd interface name never reaches a rule");
}

// ------------------------------------------------------------------ sockets

#[test]
fn dhcp_and_dns_are_bound_to_the_shared_interface_only() {
    let mut mk = RecordingMaker::default();
    io::open(&mut mk, "enx0", Ipv4Addr::new(10, 42, 0, 1), &[Ipv4Addr::new(9, 9, 9, 9)], Ports::default()).unwrap();
    assert_eq!(mk.binds.len(), 3);
    assert_eq!(mk.binds[0], (SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 67), Some("enx0".into()), true), "DHCP: port 67, this interface, may broadcast");
    assert_eq!(mk.binds[1], (SocketAddrV4::new(Ipv4Addr::new(10, 42, 0, 1), 53), Some("enx0".into()), false), "DNS: only the gateway address, this interface");
    assert_eq!(mk.binds[2].1, None, "the resolver socket follows the routes");
    assert!(mk.binds.iter().all(|b| b.0.port() != 22 && b.0.port() != 80), "no other port is opened");
    let mut busy = RecordingMaker { fail_port: Some(67), ..Default::default() };
    let e = io::open(&mut busy, "enx0", Ipv4Addr::new(10, 42, 0, 1), &[], Ports::default()).err().unwrap();
    assert!(e.contains("DHCP port 67"), "{e}");
}

fn free_port() -> u16 {
    crate::testdir::free_port()
}

#[test]
fn a_real_dhcp_discover_on_loopback_gets_an_offer_and_the_ack() {
    let ports = Ports { dhcp_server: free_port(), dhcp_client: free_port(), dns: free_port(), upstream_dns: 53, broadcast: Ipv4Addr::LOCALHOST };
    let mut os = FakeOs::new(machine());
    os.st().loop_ports = Some(ports);
    let mut m = Manager::new();
    let req = start_req(1);
    assert_eq!(m.tick(&mut os, &req).state, State::On);
    let client = std::net::UdpSocket::bind(("127.0.0.1", ports.dhcp_client)).unwrap();
    client.set_read_timeout(Some(std::time::Duration::from_millis(100))).unwrap();
    let srv = ("127.0.0.1", ports.dhcp_server);
    let recv = |m: &mut Manager, os: &mut FakeOs| -> Vec<u8> {
        for _ in 0..40 {
            m.tick(os, &req);
            let mut buf = [0u8; 1500];
            if let Ok((n, _)) = client.recv_from(&mut buf) {
                return buf[..n].to_vec();
            }
        }
        panic!("no answer on the loopback socket");
    };
    client.send_to(&dhcp::client_packet(dhcp::DISCOVER, client_mac(), 9, Ipv4Addr::UNSPECIFIED, None, None, Some("loop")), srv).unwrap();
    let offer = recv(&mut m, &mut os);
    let (t, ip) = dhcp::reply_summary(&offer).unwrap();
    assert_eq!(t, dhcp::OFFER);
    assert!(ip.octets()[..3] == [10, 42, 0]);
    client.send_to(&dhcp::client_packet(dhcp::REQUEST, client_mac(), 9, Ipv4Addr::UNSPECIFIED, Some(ip), Some(Ipv4Addr::new(10, 42, 0, 1)), Some("loop")), srv).unwrap();
    let ack = recv(&mut m, &mut os);
    assert_eq!(dhcp::reply_summary(&ack), Some((dhcp::ACK, ip)));
    os.advance_secs(3);
    assert_eq!(m.tick(&mut os, &req).clients.len(), 1);
}

// ------------------------------------------------------------------ small pure pieces, files, command line

#[test]
fn resolvers_come_from_the_machine_ipv4_only_at_most_three() {
    let t = "# x\nnameserver 127.0.0.53\nnameserver fe80::1\nnameserver 1.1.1.1\nnameserver 1.1.1.1\nnameserver 9.9.9.9\nnameserver 8.8.8.8\nnameserver 8.8.4.4\nsearch lan\n";
    assert_eq!(resolvers_from(t), [Ipv4Addr::new(127, 0, 0, 53), Ipv4Addr::new(1, 1, 1, 1), Ipv4Addr::new(9, 9, 9, 9)]);
    assert!(resolvers_from("").is_empty());
    let p = make_plan(&machine(), &start_req(1), false, &[]).unwrap();
    assert_eq!(p.resolvers, super::plan::FALLBACK_RESOLVERS, "public resolvers only when the machine names none");
}

#[test]
fn link_kinds_come_from_names_and_drivers() {
    use super::real::kind_of;
    assert_eq!(kind_of("enp3s0", "r8169", false), LinkKind::Ethernet);
    assert_eq!(kind_of("thunderbolt0", "thunderbolt-net", false), LinkKind::Thunderbolt);
    assert_eq!(kind_of("usb0", "cdc_subset", false), LinkKind::Usb);
    assert_eq!(kind_of("enx00e04c680001", "r8152", false), LinkKind::Usb);
    assert_eq!(kind_of("wlp3s0", "iwlwifi", true), LinkKind::Wifi);
    assert_eq!(kind_of("docker0", "bridge", false), LinkKind::Other);
    assert!(valid_iface("enx00e04c680001") && !valid_iface("a b") && !valid_iface("eth0:1") && !valid_iface(&"x".repeat(16)));
}

#[test]
fn request_and_status_files_round_trip_and_junk_reads_as_off() {
    let d = std::env::temp_dir().join(format!("iyakki-share-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    assert_eq!(control::read_request(&d), Request::default());
    assert_eq!(control::read_status(&d).state, State::Off);
    let r = control::start_request(&Request::default(), Some("eth0".into()), Some("enx0".into()), 4, true, false, "tester", true);
    control::write_request(&d, &r).unwrap();
    assert_eq!(control::read_request(&d), r);
    assert_eq!(r.id, 1);
    assert_eq!(control::stop_request(&r, "tester").id, 2);
    std::fs::write(control::dir(&d).join("status.json"), "{ not json").unwrap();
    assert_eq!(control::read_status(&d), Status::default());
    let s = Status { state: State::On, uplink: "eth0".into(), clients: vec![Client { name: "a".into(), ip: "10.42.0.10".into(), mac: "aa".into(), expires_in: 5 }], ..Status::default() };
    control::write_status(&d, &s).unwrap();
    assert_eq!(control::read_status(&d), s);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_status_line_says_it_in_plain_words() {
    assert_eq!(Status::default().line(), "Sharing is off.");
    let s = Status { state: State::On, uplink: "eth0".into(), downstream: "enx0".into(), subnet: "10.42.0.0/24".into(), off_in_secs: Some(6480), clients: vec![Client { name: "a".into(), ip: "i".into(), mac: "m".into(), expires_in: 1 }], ..Status::default() };
    assert_eq!(s.line(), "Sharing eth0 through enx0 on 10.42.0.0/24 (1 device). Switches off in 1 h 48 min if nobody uses it.");
    assert_eq!(human_duration(59), "59 s");
    assert_eq!(human_duration(7200), "2 h");
}

fn args(s: &str) -> Vec<String> {
    s.split_whitespace().map(String::from).collect()
}

#[test]
fn the_command_line_parses_and_refuses_what_it_does_not_know() {
    use super::cli::{parse, Cmd};
    assert_eq!(parse(&args("start")), Ok(Cmd::Start { uplink: None, via: None, hours: 2, isolate: true, allow_lan: false }));
    assert_eq!(parse(&args("start --uplink eth0 --via enx0 --hours 12 --allow-lan --no-isolation")), Ok(Cmd::Start { uplink: Some("eth0".into()), via: Some("enx0".into()), hours: 12, isolate: false, allow_lan: true }));
    assert_eq!(parse(&args("stop")), Ok(Cmd::Stop));
    assert_eq!(parse(&args("status")), Ok(Cmd::Status));
    assert_eq!(parse(&args("list")), Ok(Cmd::List));
    for bad in ["", "start --hours 0", "start --hours 73", "start --hours x", "start --via", "start --via --hours", "start --bogus", "stop --via x", "status --allow-lan", "restart"] {
        assert!(parse(&args(bad)).is_err(), "{bad:?} must be refused");
    }
    assert!(parse(&args("start --hours 99")).unwrap_err().contains("1 to 72"));
}

#[test]
fn the_command_line_explains_a_mistake_before_it_asks_the_service() {
    let b = MemBackend::new(machine(), false);
    let mut out = Vec::new();
    let rc = super::cli::run(&b, super::cli::Cmd::Start { uplink: None, via: Some("nope0".into()), hours: 2, isolate: true, allow_lan: false }, &mut |l| out.push(l.to_string()), 0);
    assert_eq!(rc, 2);
    assert!(out[0].contains("no network device called nope0"), "{out:?}");
    assert_eq!(b.request(), Request::default(), "nothing was written");
}

#[test]
fn the_command_line_writes_the_request_and_prints_the_answer_of_the_service() {
    let b = MemBackend::new(machine(), false);
    b.set_status(Status { state: State::On, uplink: "eth0".into(), downstream: "enx0".into(), subnet: "10.42.0.0/24".into(), gateway: "10.42.0.1".into(), request_id: 1, ..Status::default() });
    let mut out = Vec::new();
    let rc = super::cli::run(&b, super::cli::Cmd::Start { uplink: None, via: Some("enx0".into()), hours: 3, isolate: true, allow_lan: false }, &mut |l| out.push(l.to_string()), 0);
    assert_eq!(rc, 0, "{out:?}");
    assert!(b.request().on && b.request().idle_hours == 3 && b.request().via.as_deref() == Some("enx0"));
    assert!(out.iter().any(|l| l.contains("give the devices on that cable access to your internet")));
    assert!(out.iter().any(|l| l.contains("Sharing eth0 through enx0")));
    // no service answers: say so instead of hanging
    let b2 = MemBackend::new(machine(), false);
    let mut out2 = Vec::new();
    assert_eq!(super::cli::run(&b2, super::cli::Cmd::Stop, &mut |l| out2.push(l.to_string()), 0), 1);
    assert!(out2[0].contains("did not answer"));
}

#[test]
fn the_page_lists_name_and_state_and_says_when_wifi_is_missing() {
    let mut ifs = machine();
    ifs.push(IfaceInfo::new("thunderbolt0", LinkKind::Thunderbolt));
    ifs[1].carrier = false;
    ifs.push(IfaceInfo::new("wlan1", LinkKind::Wifi));
    ifs.push(IfaceInfo::new("docker0", LinkKind::Other));
    let c = super::view::choices(&ifs, false);
    assert_eq!(c.uplinks.iter().map(|u| (u.name.as_str(), u.usable)).collect::<Vec<_>>(), [("eth0", true), ("bluetooth-panu", false)]);
    assert_eq!(c.downs.iter().map(|u| (u.name.as_str(), u.usable)).collect::<Vec<_>>(), [("eth0", false), ("enx0", true), ("thunderbolt0", true), ("bluetooth-nap", false)]);
    assert!(c.downs[3].why.contains("not built yet"), "{}", c.downs[3].why);
    assert!(c.downs[1].label.contains("no cable") && c.downs[2].label.contains("cable connected") && c.downs[0].label.contains("in use"));
    assert_eq!(c.wifi_line.as_deref(), Some("Wi-Fi hotspot: not available on this build"));
    let with_ap = super::view::choices(&ifs, true);
    assert!(with_ap.wifi_line.is_none());
    assert!(with_ap.downs.iter().any(|d| d.name == crate::wifi_ap::AP_IFACE && d.usable && d.label.starts_with("Wi-Fi hotspot")), "the hotspot is one entry of its own");
    assert!(!with_ap.downs.iter().any(|d| d.name == "wlan1"), "a Wi-Fi client interface is never offered as the cable");
}

#[test]
fn the_older_single_device_flow_uses_the_same_typed_writer() {
    let t = describe(&super::rules::legacy_table("usb0", "wlp3s0"));
    assert_eq!(t, "table inet iyakki_share\n  chain forward filter hook forward priority -10 policy accept\n    iif usb0 oif wlp3s0 accept\n    iif wlp3s0 oif usb0 ct state established,related accept\n  chain post nat hook postrouting priority 100 policy accept\n    iif usb0 oif wlp3s0 masquerade\n");
    assert!(iyakki_fw::nl::tablegen::encode_apply(&super::rules::legacy_table("usb0", "wlp3s0"), 1).is_ok());
    assert!(iyakki_fw::nl::tablegen::encode_apply(&super::rules::legacy_table("usb0; reboot", "wlp3s0"), 1).is_err(), "a name that could smuggle anything is refused by the writer");
    assert!(super::real::legacy_apply("usb0; reboot", "eth0").is_err());
}

// ------------------------------------------------------------------ policy and Bluetooth

use super::policy::{self, Policy, Who};

#[test]
fn the_policy_matrix_role_by_kind_by_allow() {
    for (who, admin, user) in [(Who::Admins, true, false), (Who::Users, true, true), (Who::Nobody, false, false)] {
        for kind in [LinkKind::Ethernet, LinkKind::Usb, LinkKind::Thunderbolt, LinkKind::Wifi, LinkKind::BluetoothNap, LinkKind::BluetoothPanu] {
            let mut p = Policy::default();
            match policy::policy_kind(kind) {
                policy::PolicyKind::Wired => p.wired = who,
                policy::PolicyKind::Wifi => p.wifi = who,
                policy::PolicyKind::Bluetooth => p.bluetooth = who,
            }
            assert_eq!(p.may_start(kind, true).is_ok(), admin, "{kind:?} {who:?} admin");
            assert_eq!(p.may_start(kind, false).is_ok(), user, "{kind:?} {who:?} user");
        }
    }
    // the kinds are independent: wired allowed, hotspot blocked
    let p = Policy { wired: Who::Users, wifi: Who::Nobody, ..Policy::default() };
    assert!(p.may_start(LinkKind::Ethernet, false).is_ok() && p.may_start(LinkKind::Wifi, true).is_err());
    let d = Policy::default();
    assert!(d.may_start(LinkKind::Ethernet, true).is_ok() && d.may_start(LinkKind::Ethernet, false).is_err(), "the default: administrators only");
    assert_eq!(Policy { wired: Who::Nobody, ..Policy::default() }.may_start(LinkKind::Usb, true).unwrap_err().line(), "Internet sharing is turned off by your administrator.");
}

#[test]
fn a_user_request_is_refused_by_the_service_under_the_default_policy_and_allowed_when_users_may() {
    let mut os = FakeOs::new(machine());
    os.st().policy = Policy::default();
    let mut m = Manager::new();
    let st = m.tick(&mut os, &start_req(1));
    assert_eq!(st.state, State::Failed);
    assert!(st.note.contains("only be started by an administrator"), "{}", st.note);
    assert!(os.st().calls.is_empty());
    assert_eq!(m.tick(&mut os, &Request { by_admin: true, ..start_req(2) }).state, State::On);
}

#[test]
fn a_running_session_stops_within_seconds_when_the_policy_changes_and_it_is_audited() {
    let mut os = FakeOs::new(machine());
    let mut m = Manager::new();
    assert_eq!(m.tick(&mut os, &start_req(1)).state, State::On);
    os.st().policy.wired = Who::Nobody;
    os.advance_secs(1);
    assert_eq!(m.tick(&mut os, &start_req(1)).state, State::On, "looked at every two seconds, not every tick");
    os.advance_secs(2);
    let st = m.tick(&mut os, &start_req(1));
    assert_eq!(st.state, State::Failed);
    assert_eq!(st.note, "Internet sharing is turned off by your administrator.");
    pristine(&os);
    assert!(os.st().audit.iter().any(|l| l.contains("stopped (the administrator's policy no longer allows it)")));
    // removing the person's permission (admins only now) stops a session a user started
    let mut os = FakeOs::new(machine());
    let mut m = Manager::new();
    m.tick(&mut os, &start_req(1));
    os.st().policy.wired = Who::Admins;
    os.advance_secs(3);
    assert_eq!(m.tick(&mut os, &start_req(1)).state, State::Failed);
    // an administrator's session survives the same change
    let mut os = FakeOs::new(machine());
    let mut m = Manager::new();
    let r = Request { by_admin: true, ..start_req(1) };
    m.tick(&mut os, &r);
    os.st().policy.wired = Who::Admins;
    os.advance_secs(3);
    assert_eq!(m.tick(&mut os, &r).state, State::On);
}

#[test]
fn the_limits_hard_maximum_names_and_device_count_are_enforced() {
    let mut os = FakeOs::new(machine());
    os.st().policy.max_hours = 1;
    let mut m = Manager::new();
    let r = Request { idle_hours: 72, ..start_req(1) };
    m.tick(&mut os, &r);
    for _ in 0..7 {
        os.st().ifs[1].rx_bytes += 100_000; // busy all the time: the idle switch-off never fires
        os.advance_secs(600);
        m.tick(&mut os, &r);
    }
    let st = m.status().clone();
    assert_eq!(st.state, State::Failed, "{st:?}");
    assert!(st.note.contains("at most 1 h per session"), "{}", st.note);
    pristine(&os);
    // allowed interface names
    let mut os = FakeOs::new(machine());
    os.st().policy.allowed_downstreams = vec!["thunderbolt0".into()];
    let st = Manager::new().tick(&mut os, &start_req(1));
    assert!(st.note.contains("Sharing through enx0 is not allowed"), "{}", st.note);
    let mut os = FakeOs::new(machine());
    os.st().policy.allowed_uplinks = vec!["wlan9".into()];
    assert!(Manager::new().tick(&mut os, &start_req(1)).note.contains("internet of eth0 is not allowed"));
    // device limit shrinks the pool
    let mut p = plan_for(&start_req(1));
    super::plan::apply_policy(&mut p, &Policy { wired: Who::Users, max_clients: 3, ..Policy::default() }, &start_req(1)).unwrap();
    assert_eq!(p.pool, 3);
}

#[test]
fn bluetooth_kinds_report_not_available_from_their_probe_and_cannot_start() {
    assert_eq!(LinkKind::BluetoothNap.probe(true).unwrap_err().line(), "Bluetooth tethering: not available on this build.");
    assert!(LinkKind::BluetoothPanu.probe(false).is_err() && LinkKind::Ethernet.probe(false).is_ok() && LinkKind::Wifi.probe(false).is_err());
    let mut ifs = machine();
    ifs.push(IfaceInfo::new("bnep0", LinkKind::BluetoothNap));
    let e = make_plan(&ifs, &Request { via: Some("bnep0".into()), ..start_req(1) }, false, &[]).unwrap_err();
    assert_eq!(e, ShareError::BluetoothNotAvailable);
    // a bridged downstream (several ports, one address) is part of the plan's shape
    assert!(plan_for(&start_req(1)).members.is_empty());
}

#[test]
fn the_policy_file_is_written_by_administrators_only_and_audited_with_before_and_after() {
    let d = std::env::temp_dir().join(format!("iyakki-share-policy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    assert_eq!(policy::read(&d), Policy::default());
    let p = Policy { wired: Who::Users, wifi: Who::Nobody, max_hours: 4, ..Policy::default() };
    assert!(policy::write(&d, &p, "amy", false, 5).unwrap_err().contains("Only administrators"));
    assert_eq!(policy::read(&d), Policy::default());
    let line = policy::write(&d, &p, "root", true, 99).unwrap();
    assert!(line.contains("before [wired admins, wifi admins") && line.contains("after [wired users, wifi nobody") && line.contains("max hours 4"), "{line}");
    let back = policy::read(&d);
    assert_eq!((back.wired, back.wifi, back.changed_by.as_str(), back.changed_at), (Who::Users, Who::Nobody, "root", 99));
    assert!(std::fs::read_to_string(control::dir(&d).join("audit.log")).unwrap().contains("share policy changed by root"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_command_line_refuses_with_the_reason_and_edits_the_policy() {
    use super::cli::{parse, run, Cmd};
    let b = MemBackend::new(machine(), false);
    b.set_policy_raw(Policy { wired: Who::Nobody, ..Policy::default() });
    let mut out = Vec::new();
    let rc = run(&b, Cmd::Start { uplink: None, via: Some("enx0".into()), hours: 2, isolate: true, allow_lan: false }, &mut |l| out.push(l.to_string()), 0);
    assert_eq!(rc, 2);
    assert_eq!(out[0], "error: Internet sharing is turned off by your administrator.");
    assert!(!b.request().on);
    assert_eq!(parse(&args("policy --wired users --max-hours 3")), Ok(Cmd::Policy(vec![("wired".into(), "users".into()), ("max-hours".into(), "3".into())])));
    assert!(parse(&args("policy --wired everyone")).is_err() && parse(&args("policy --max-hours x")).is_err() && parse(&args("policy --bogus 1")).is_err());
    let mut out = Vec::new();
    assert_eq!(run(&b, Cmd::Policy(vec![("wired".into(), "users".into())]), &mut |l| out.push(l.to_string()), 0), 0);
    assert!(out[0].contains("before [wired nobody") && out[0].contains("after [wired users"), "{out:?}");
    assert_eq!(b.policy().wired, Who::Users);
}

#[test]
fn a_wifi_hotspot_makes_its_access_point_first_shares_through_it_and_removes_it_with_the_session() {
    let os0 = FakeOs::new(machine());
    os0.st().wifi_ap = true;
    let mut os = os0.clone();
    let mut m = Manager::new();
    let req = Request { id: 1, on: true, wifi: true, ..Request::default() };
    let st = m.tick(&mut os, &req);
    assert_eq!(st.state, State::On, "{}", st.note);
    assert_eq!(st.downstream, crate::wifi_ap::AP_IFACE);
    assert_eq!(st.uplink, "eth0");
    {
        let s = os.st();
        assert_eq!(s.calls.iter().filter(|c| *c == "wifi_ap_start").count(), 1);
        assert!(s.calls.iter().position(|c| c == "wifi_ap_start") < s.calls.iter().position(|c| c.starts_with("forward=1")), "the access point exists before forwarding starts");
    }
    let st = m.tick(&mut os, &Request { id: 2, on: false, ..Request::default() });
    assert_eq!(st.state, State::Off);
    let s = os.st();
    assert_eq!(s.calls.iter().filter(|c| *c == "wifi_ap_stop").count(), 1, "{:?}", s.calls);
    assert!(!s.ifs.iter().any(|i| i.name == crate::wifi_ap::AP_IFACE), "the access point interface is gone");
}

#[test]
fn a_hotspot_that_cannot_start_says_why_and_a_failed_share_takes_the_access_point_down_again() {
    let os0 = FakeOs::new(machine());
    os0.st().wifi_ap = true;
    os0.st().wifi_ap_error = Some("the card refused");
    let mut os = os0.clone();
    let mut m = Manager::new();
    let st = m.tick(&mut os, &Request { id: 1, on: true, wifi: true, ..Request::default() });
    assert_eq!(st.state, State::Failed);
    assert!(st.note.contains("the card refused"), "{}", st.note);
    // the access point starts but the share fails afterwards (no internet anywhere): it is taken down again
    let os0 = FakeOs::new(vec![IfaceInfo::new("eth0", LinkKind::Ethernet)]);
    os0.st().wifi_ap = true;
    let mut os = os0.clone();
    let mut m = Manager::new();
    let st = m.tick(&mut os, &Request { id: 1, on: true, wifi: true, ..Request::default() });
    assert_eq!(st.state, State::Failed);
    assert_eq!(os.st().calls.iter().filter(|c| *c == "wifi_ap_stop").count(), 1);
}

#[test]
fn a_client_wifi_interface_is_never_shared_only_the_hotspot_this_computer_made() {
    let mut ifs = machine();
    ifs.push(IfaceInfo::new("wlan1", LinkKind::Wifi));
    let r = Request { id: 1, on: true, via: Some("wlan1".into()), ..Request::default() };
    assert!(matches!(make_plan(&ifs, &r, true, &[]), Err(ShareError::NotShareable(_))));
    ifs.push(IfaceInfo::new(crate::wifi_ap::AP_IFACE, LinkKind::Wifi));
    let r = Request { id: 1, on: true, via: Some(crate::wifi_ap::AP_IFACE.into()), ..Request::default() };
    assert!(make_plan(&ifs, &r, true, &[]).is_ok());
}

#[test]
fn the_ranges_the_routing_table_sends_through_a_link_are_read_and_default_and_link_local_ones_are_not() {
    // /proc/net/route text: wg0 routes 10.42.0.0/16 (little-endian hex), eth0 has the default route, its own /24 and a link-local range
    let text = "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n\
                eth0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
                eth0\t0001A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n\
                eth0\t0000FEA9\t00000000\t0001\t0\t0\t1000\t0000FFFF\t0\t0\t0\n\
                wg0\t00002A0A\t00000000\t0001\t0\t0\t50\t0000FFFF\t0\t0\t0\n\
                wg0\t00002A0A\t00000000\t0001\t0\t0\t50\t0000FFFF\t0\t0\t0\n\
                bad\tzzzz\t00000000\t0001\t0\t0\t50\t0000FFFF\t0\t0\t0\n\
                odd\t00002B0A\t00000000\t0001\t0\t0\t50\t00FF00FF\t0\t0\t0\n";
    let r = super::plan::routed_ranges_from(text);
    assert_eq!(r, vec![("eth0".to_string(), cidr("192.168.1.0/24")), ("wg0".to_string(), cidr("10.42.0.0/16"))]);
    assert!(super::plan::routed_ranges_from("").is_empty());
}

#[test]
fn the_hotspot_network_avoids_a_range_only_a_route_names() {
    // a VPN routes 10.42.0.0/16 and 192.168.43.0/24 although no address of this machine is in them
    let mut ifs = machine();
    let mut vpn = IfaceInfo::new("wg0", LinkKind::Other);
    vpn.routed = vec![cidr("10.42.0.0/16"), cidr("192.168.43.0/24")];
    ifs.push(vpn);
    let p = make_plan(&ifs, &start_req(1), false, &[]).unwrap();
    assert_eq!(p.net.net, cidr("192.168.137.0/24"), "the first two usual networks are taken by the routes");
    // the same machine without the routes would have taken the first usual network
    let p = make_plan(&machine(), &start_req(1), false, &[]).unwrap();
    assert_eq!(p.net.net, cidr("10.42.0.0/24"));
}

#[test]
fn a_huge_routed_range_leaves_the_networks_outside_it() {
    let mut ifs = machine();
    let mut vpn = IfaceInfo::new("wg0", LinkKind::Other);
    vpn.routed = vec![cidr("10.0.0.0/8")];
    ifs.push(vpn);
    let p = make_plan(&ifs, &start_req(1), false, &[]).unwrap();
    assert_eq!(p.net.net, cidr("192.168.43.0/24"));
}

#[test]
fn a_connected_phone_is_an_uplink_like_a_cable_and_never_a_cable_to_share_on() {
    assert_eq!(super::real::kind_of("iyakbt0", "", false), LinkKind::BluetoothPanu);
    let mut ifs = machine();
    let mut bt = IfaceInfo::new("iyakbt0", LinkKind::BluetoothPanu);
    bt.default_route = true;
    ifs.push(bt);
    let c = super::view::choices(&ifs, false);
    let up: Vec<_> = c.uplinks.iter().map(|u| (u.name.as_str(), u.usable)).collect();
    assert!(up.contains(&("iyakbt0", true)), "{up:?}");
    assert!(!up.iter().any(|u| u.0 == "bluetooth-panu"), "the placeholder makes way for the real link: {up:?}");
    assert!(c.uplinks.iter().any(|u| u.name == "iyakbt0" && u.label.contains("use a phone's tethering") && u.label.contains("has internet")));
    assert!(!c.downs.iter().any(|d| d.name == "iyakbt0"), "the phone's link is a source, never the cable that is shared");
    // without the link the entry stays and says what to do
    let c = super::view::choices(&machine(), false);
    let p = c.uplinks.iter().find(|u| u.name == "bluetooth-panu").unwrap();
    assert!(!p.usable && !p.why.is_empty());
}
