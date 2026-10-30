use super::*;
use crate::archive::tests::tmp;

fn iface(name: &str, kind: Kind, carrier: bool, ip: Option<&str>) -> Iface {
    Iface { name: name.into(), kind, carrier, ipv4: ip.map(|i| vec![i.parse().unwrap()]).unwrap_or_default(), gateway: None }
}

// ------------------------------------------------------------------------------------------------ interfaces

#[test]
fn interfaces_are_classified_from_name_driver_and_wireless_folder() {
    assert_eq!(classify("enp3s0", "r8169", false), Kind::Ethernet);
    assert_eq!(classify("eth0", "", false), Kind::Ethernet);
    assert_eq!(classify("wlp2s0", "iwlwifi", true), Kind::Wifi);
    assert_eq!(classify("thunderbolt0", "thunderbolt-net", false), Kind::Link);
    assert_eq!(classify("usb0", "cdc_ether", false), Kind::Link);
    assert_eq!(classify("enx0011223344", "cdc_ncm", false), Kind::Link);
    assert_eq!(classify("enx0011223344", "r8152", false), Kind::Link, "a USB network class adapter");
    assert_eq!(classify("enx0011223344", "xhci", false), Kind::Ethernet);
    assert_eq!(classify("lo", "", false), Kind::Other);
    assert_eq!(classify("docker0", "bridge", false), Kind::Other);
}

#[test]
fn default_gateways_are_read_from_the_routing_table() {
    let t = "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\neth0\t00000000\t0100A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\neth0\t0000A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n";
    assert_eq!(gateways_from_route(t), vec![("eth0".to_string(), "192.168.0.1".parse().unwrap())]);
}

// ------------------------------------------------------------------------------------------------ the offers, in order

fn ways(o: &[Offer]) -> Vec<Way> {
    o.iter().map(|x| x.way).collect()
}

#[test]
fn the_card_always_lists_the_four_ways_in_the_order_of_detection() {
    let o = offers(&Situation::default());
    assert_eq!(ways(&o), [Way::Ethernet, Way::LinkInternet, Way::Stick, Way::Push]);
    assert!(o.iter().all(|x| !x.ready && !x.line.is_empty()));
    assert!(recommended(&o).is_none());
}

#[test]
fn wifi_only_with_no_firmware_says_plug_in_a_cable_plainly() {
    let s = Situation { ifaces: vec![iface("wlp2s0", Kind::Wifi, false, None)], wifi_lacks_firmware: true, ..Default::default() };
    let o = offers(&s);
    assert!(o[0].line.starts_with("Plug in an Ethernet cable") && o[0].line.contains("Wi-Fi is the only network"), "{}", o[0].line);
}

#[test]
fn ethernet_with_internet_is_found_first_and_recommended() {
    let s = Situation { ifaces: vec![iface("enp3s0", Kind::Ethernet, true, Some("192.168.0.20")), iface("usb0", Kind::Link, true, None)], internet_on: vec!["enp3s0".into()], sticks: vec![PathBuf::from("/media/usb/firmware-extra")], devlink_up: true, ..Default::default() };
    let o = offers(&s);
    assert!(o[0].ready && o[0].line.contains("download the files now"));
    assert_eq!(recommended(&o).unwrap().way, Way::Ethernet);
    // Ethernet plugged in without internet is said so
    let s2 = Situation { ifaces: vec![iface("enp3s0", Kind::Ethernet, true, Some("169.254.1.2"))], ..Default::default() };
    assert!(!offers(&s2)[0].ready && offers(&s2)[0].line.contains("no internet yet"));
}

#[test]
fn a_cable_to_another_computer_needs_the_click_first_then_is_a_download() {
    let s = Situation { ifaces: vec![iface("thunderbolt0", Kind::Link, true, None)], ..Default::default() };
    let o = offers(&s);
    assert!(!o[1].ready && o[1].needs_click && o[1].line.contains("iyakki usb share-internet --apply"));
    assert_eq!(recommended(&o).unwrap().way, Way::LinkInternet);
    let s = Situation { internet_on: vec!["thunderbolt0".into()], ..s };
    let o = offers(&s);
    assert!(o[1].ready && !o[1].needs_click);
}

#[test]
fn without_a_network_the_stick_then_the_push_are_offered() {
    let s = Situation { sticks: vec![PathBuf::from("/media/usb/firmware-extra")], devlink_up: true, ..Default::default() };
    let o = offers(&s);
    assert_eq!(recommended(&o).unwrap().way, Way::Stick);
    assert!(o[3].ready && o[3].line.contains("You still approve the install on this screen"));
    let s = Situation { devlink_up: true, ..Default::default() };
    assert_eq!(recommended(&offers(&s)).unwrap().way, Way::Push);
}

#[test]
fn sticks_are_found_by_their_folder_not_by_guessing() {
    let root = tmp("fwx-net-stick");
    let usb = root.join("USB");
    std::fs::create_dir_all(usb.join("firmware-extra/intel")).unwrap();
    std::fs::create_dir_all(root.join("OTHER/photos")).unwrap();
    std::fs::create_dir_all(root.join("THIRD/linux-firmware-20261019")).unwrap();
    let found = find_sticks(&[root.clone()]);
    assert_eq!(found, vec![root.join("THIRD/linux-firmware-20261019"), usb.join("firmware-extra")]);
    let _ = std::fs::remove_dir_all(&root);
}

// ------------------------------------------------------------------------------------------------ the session over the cable

#[derive(Default)]
struct FakeNet {
    ran: Vec<String>,
    fail_on: Option<String>,
    dns: Vec<(Ipv4Addr, String, Vec<Ipv4Addr>)>,
    hosts: String,
    reachable: bool,
    audit: Vec<String>,
}

impl NetEnv for FakeNet {
    fn run(&mut self, argv: &[String]) -> Result<(), String> {
        let l = argv.join(" ");
        self.ran.push(l.clone());
        if self.fail_on.as_ref().is_some_and(|f| l.contains(f)) {
            return Err("RTNETLINK answers: Operation not permitted".into());
        }
        Ok(())
    }
    fn resolve(&mut self, server: Ipv4Addr, name: &str) -> Vec<Ipv4Addr> {
        self.dns.iter().find(|d| d.0 == server && d.1 == name).map(|d| d.2.clone()).unwrap_or_default()
    }
    fn read_hosts(&self) -> String {
        self.hosts.clone()
    }
    fn write_hosts(&mut self, text: &str) -> Result<(), String> {
        self.hosts = text.to_string();
        Ok(())
    }
    fn reaches(&mut self, _h: &str) -> bool {
        self.reachable
    }
    fn audit(&mut self, l: &str) {
        self.audit.push(l.to_string());
    }
}

fn net_with_dns() -> FakeNet {
    FakeNet { hosts: "127.0.0.1 localhost\n".into(), reachable: true, dns: vec![(SHARE_HOST, "git.kernel.org".into(), vec!["203.0.113.7".parse().unwrap()])], ..Default::default() }
}

#[test]
fn the_session_adds_host_routes_only_and_no_default_route() {
    let mut n = net_with_dns();
    let (s, log) = start_session(&mut n, "thunderbolt0", &["git.kernel.org"], false).unwrap();
    let all = n.ran.join("\n");
    assert!(all.contains("ip addr add 10.77.0.2/30 dev thunderbolt0"), "{all}");
    assert!(all.contains("ip route add 203.0.113.7/32 via 10.77.0.1 dev thunderbolt0"), "{all}");
    assert!(all.contains("ip route add 9.9.9.9/32 via 10.77.0.1"), "resolvers are routed through the cable too");
    assert!(!all.contains("default"), "no default route for the rest of the system: {all}");
    assert!(n.hosts.contains("203.0.113.7 git.kernel.org # iyakki-firmware-session") && n.hosts.contains("localhost"));
    assert!(log.iter().any(|l| l.contains("no default route")) && n.audit[0].starts_with("firmware.link-internet start thunderbolt0"));
    // stop: everything the session added goes, the rest of the hosts file stays
    stop_session(&mut n, &s).unwrap();
    assert_eq!(n.hosts, "127.0.0.1 localhost\n");
    assert!(n.ran.iter().any(|c| c == "ip route del 203.0.113.7/32 via 10.77.0.1 dev thunderbolt0 metric 50") || n.ran.iter().any(|c| c.starts_with("ip route del 203.0.113.7/32")));
    assert!(n.ran.iter().any(|c| c.starts_with("ip addr del 10.77.0.2/30 dev thunderbolt0")), "{:?}", n.ran);
    assert!(n.audit.last().unwrap().starts_with("firmware.link-internet stop"));
}

#[test]
fn use_for_everything_is_a_choice_and_adds_a_low_priority_default_route() {
    let mut n = net_with_dns();
    let (s, _) = start_session(&mut n, "usb0", &["git.kernel.org"], true).unwrap();
    assert!(n.ran.iter().any(|c| c.contains("route add default via 10.77.0.1 dev usb0 metric 700")));
    assert!(session_undo(&s).iter().any(|c| c.join(" ").contains("route del default")));
}

#[test]
fn a_failed_step_or_an_unreachable_host_removes_what_was_added() {
    let mut n = net_with_dns();
    n.fail_on = Some("route add 203.0.113.7".into());
    let e = start_session(&mut n, "usb0", &["git.kernel.org"], false).unwrap_err();
    assert!(e.contains("Operation not permitted"), "{e}");
    assert!(n.ran.iter().any(|c| c.contains("addr del")), "undone: {:?}", n.ran);
    assert_eq!(n.hosts, "127.0.0.1 localhost\n");
    let mut n = net_with_dns();
    n.reachable = false;
    let e = start_session(&mut n, "usb0", &["git.kernel.org"], false).unwrap_err();
    assert!(e.contains("not reachable through the cable") && !n.hosts.contains("iyakki-firmware-session"), "{e}");
    let mut n = net_with_dns();
    n.dns.clear();
    assert!(start_session(&mut n, "usb0", &["git.kernel.org"], false).unwrap_err().contains("iyakki usb share-internet --apply"));
}

#[test]
fn the_dns_helpers_encode_a_query_and_read_the_answer() {
    let q = dns_query(0x1234, "git.kernel.org").unwrap();
    assert_eq!(&q[..4], &[0x12, 0x34, 0x01, 0x00]);
    assert!(dns_query(1, "bad name.example").is_none() && dns_query(1, "a..b").is_none());
    // an answer: header (response, 1 question, 2 answers), the question, two A records with a name pointer
    let mut a = vec![0x12, 0x34, 0x81, 0x80, 0, 1, 0, 2, 0, 0, 0, 0];
    a.extend_from_slice(&q[12..]);
    for ip in [[203, 0, 113, 7], [203, 0, 113, 8]] {
        a.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4]);
        a.extend_from_slice(&ip);
    }
    assert_eq!(dns_parse_a(0x1234, &a), vec![Ipv4Addr::new(203, 0, 113, 7), Ipv4Addr::new(203, 0, 113, 8)]);
    assert!(dns_parse_a(0x9999, &a).is_empty(), "another id");
    assert!(dns_parse_a(0x1234, &a[..20]).is_empty() || dns_parse_a(0x1234, &a[..20]).len() < 2, "a short packet gives no more than it holds");
    let mut bad = a.clone();
    bad[3] |= 3; // NXDOMAIN
    assert!(dns_parse_a(0x1234, &bad).is_empty());
}

// ------------------------------------------------------------------------------------------------ the other computer: share its internet

fn link_table() -> Vec<Iface> {
    vec![iface("enp0s31f6", Kind::Ethernet, true, Some("192.168.0.5")), iface("wlp3s0", Kind::Wifi, true, Some("192.168.1.9")), iface("thunderbolt0", Kind::Link, true, None)]
}

#[test]
fn the_cable_and_the_uplink_are_picked_from_a_fake_interface_table() {
    let t = link_table();
    assert_eq!(pick_link(&t, None).unwrap(), "thunderbolt0");
    assert_eq!(pick_link(&t, Some("enp0s31f6")).unwrap(), "enp0s31f6");
    assert!(pick_link(&t, Some("nope0")).unwrap_err().contains("nope0"));
    let mut two = t.clone();
    two.push(iface("usb0", Kind::Link, true, None));
    assert!(pick_link(&two, None).unwrap_err().contains("--device"));
    assert!(pick_link(&t[..2], None).unwrap_err().contains("no cable link"));
    let gws = vec![("thunderbolt0".to_string(), Ipv4Addr::new(10, 0, 0, 1)), ("wlp3s0".to_string(), Ipv4Addr::new(192, 168, 1, 1))];
    assert_eq!(pick_uplink(&gws, "thunderbolt0", None).unwrap(), "wlp3s0");
    assert_eq!(pick_uplink(&gws, "thunderbolt0", Some("enp0s31f6")).unwrap(), "enp0s31f6");
    assert!(pick_uplink(&gws, "thunderbolt0", Some("thunderbolt0")).is_err());
    assert!(pick_uplink(&gws[..1], "thunderbolt0", None).unwrap_err().contains("--uplink"));
}

#[test]
fn the_share_commands_name_the_cable_only_and_undo_restores_forwarding() {
    let s = Share { link: "thunderbolt0".into(), uplink: "wlp3s0".into() };
    let c = share_commands(&s).unwrap();
    let all = c.join("\n");
    assert!(c[0] == "ip addr add 10.77.0.1/30 dev thunderbolt0" && all.contains("sysctl -w net.ipv4.ip_forward=1"));
    assert!(all.contains("masquerade") && all.contains("iifname \"thunderbolt0\" oifname \"wlp3s0\""));
    assert!(!all.contains("flush") && !all.contains("iptables -F"), "no existing rule is touched");
    assert!(all.contains("table ip iyakki_share"), "everything lives in one table of our own");
    let u = unshare_commands(&s, "0").unwrap();
    assert!(u[0] == "nft delete table ip iyakki_share" && u.contains(&"sysctl -w net.ipv4.ip_forward=0".to_string()) && u.last().unwrap().starts_with("ip addr del 10.77.0.1/30"));
    assert!(unshare_commands(&s, "2").is_err());
    // names that could smuggle a second command are refused
    assert!(share_commands(&Share { link: "usb0; reboot".into(), uplink: "wlp3s0".into() }).is_err());
    assert!(share_commands(&Share { link: "usb0".into(), uplink: "a\"b".into() }).is_err());
    assert!(dns_hints(&s)[0].contains("optional") && OTHER_HOSTS.contains("Internet Sharing") && OTHER_HOSTS.contains("Windows"));
}

#[test]
fn applying_stops_at_the_first_failing_command_and_says_which() {
    let s = Share { link: "usb0".into(), uplink: "eth0".into() };
    let cmds = share_commands(&s).unwrap();
    let mut ran: Vec<String> = Vec::new();
    let r = apply(&mut |c| {
        ran.push(c.to_string());
        if c.starts_with("sysctl") { Err("permission denied (run as root)".into()) } else { Ok(()) }
    }, &cmds);
    assert!(r.unwrap_err().contains("sysctl"));
    assert_eq!(ran.len(), 3, "it stopped there: {ran:?}");
}

#[test]
fn the_interface_for_an_address_is_the_most_specific_route() {
    // default via eth0; a host route for 203.0.113.7 via usb0 (the session); a local net on eth0
    let t = "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n\
eth0\t00000000\t0100A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
eth0\t0000A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n\
usb0\t077100CB\t0100000A\t0003\t0\t0\t50\tFFFFFFFF\t0\t0\t0\n";
    assert_eq!(route_iface_for(t, "203.0.113.7".parse().unwrap()).as_deref(), Some("usb0"));
    assert_eq!(route_iface_for(t, "198.51.100.9".parse().unwrap()).as_deref(), Some("eth0"));
    assert_eq!(route_iface_for(t, "192.168.0.9".parse().unwrap()).as_deref(), Some("eth0"));
}
