//! The helper's socket protocol, the peer check and the validation, over real Unix sockets in a temp directory, with the fake
//! system layer behind the server. Keys below are FAKE.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::Duration;

use iyakki_vpn::filter::FilterSet;
use iyakki_vpn::plan::{self, GuardBase, KillSwitchInput};
use iyakki_vpn::profile::Settings;

use super::*;
use crate::gitkeys::{AuditFn, MemStore};
use crate::vpn_mgr::{Manager, State};
use crate::vpn_mgr_tests::{conf, temp};
use crate::vpn_sys::FakeSys;

fn my_uid() -> u32 {
    // SAFETY: geteuid has no arguments and cannot fail.
    unsafe { libc::geteuid() }
}

struct Rig {
    helper: Helper,
    fake: Arc<FakeSys>,
    dir: PathBuf,
}

fn rig(tag: &str, policy: PeerPolicy) -> Rig {
    let dir = temp(tag);
    let path = dir.join("h.sock");
    let fake = Arc::new(FakeSys::new());
    fake.name("vpn.example.com", &["203.0.113.9"]);
    let l = bind_socket(&path, Some(my_uid())).unwrap();
    let helper = serve(l, path, policy, fake.clone()).unwrap();
    Rig { helper, fake, dir }
}

fn raw(path: &Path) -> (UnixStream, BufReader<UnixStream>) {
    let s = UnixStream::connect(path).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let r = BufReader::new(s.try_clone().unwrap());
    (s, r)
}

fn ask(w: &mut UnixStream, r: &mut BufReader<UnixStream>, line: &str) -> Value {
    w.write_all(line.as_bytes()).unwrap();
    w.write_all(b"\n").unwrap();
    let mut out = String::new();
    r.read_line(&mut out).unwrap();
    serde_json::from_str(&out).unwrap_or_else(|_| panic!("not json: {out:?}"))
}

#[test]
fn the_socket_is_private_and_a_stale_or_foreign_path_is_handled() {
    let r = rig("sock", PeerPolicy::only(my_uid()));
    let mode = std::fs::metadata(&r.helper.path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    // a second helper on the same path is refused while the first answers
    assert_eq!(bind_socket(&r.helper.path, None).unwrap_err().kind(), std::io::ErrorKind::AddrInUse);
    // a regular file at the path is never removed
    let f = r.dir.join("not-a-socket");
    std::fs::write(&f, "x").unwrap();
    assert!(bind_socket(&f, None).is_err());
    assert!(f.exists());
    // a stale socket (nobody listening) is replaced
    let p = r.dir.join("stale.sock");
    drop(UnixListener::bind(&p).unwrap());
    assert!(p.exists());
    bind_socket(&p, None).unwrap();
}

#[test]
fn a_peer_that_is_not_on_the_list_is_closed_on_and_nothing_runs() {
    let r = rig("deny", PeerPolicy::only(my_uid().wrapping_add(4242)));
    let remote = RemoteSys::new(&r.helper.path);
    let e = remote.ip(&["link".into(), "add".into(), "dev".into(), IFNAME.into(), "type".into(), "wireguard".into()]).unwrap_err();
    assert!(e.contains("did not answer") || e.contains("not reachable"), "{e}");
    assert!(r.fake.calls().is_empty(), "the system layer was never touched: {:?}", r.fake.calls());
    assert!(r.helper.log.lock().unwrap().iter().any(|l| l.contains("peer not allowed")));
    // an empty list allows nobody either
    let r2 = rig("deny2", PeerPolicy::default());
    assert!(RemoteSys::new(&r2.helper.path).guard_present() == false && r2.fake.calls().is_empty());
    assert!(PeerPolicy::default().allows(&PeerCred { pid: 1, uid: 0, gid: 0 }) == false);
    // the kernel's record is what is checked: our own connection reports our own id
    let (s, _) = raw(&r.helper.path);
    assert_eq!(peer_cred(&s).unwrap().uid, my_uid());
}

#[test]
fn every_operation_round_trips_to_the_system_layer() {
    let r = rig("ops", PeerPolicy::only(my_uid()));
    let c = RemoteSys::new(&r.helper.path);
    assert_eq!(c.net_info().unwrap().physical_if, "eth0");
    assert_eq!(c.resolve("vpn.example.com").unwrap(), vec!["203.0.113.9".parse::<IpAddr>().unwrap()]);
    assert!(c.resolve("bad host;rm").is_err());
    c.ip(&plan::link_add_args(IFNAME).unwrap()).unwrap();
    assert!(c.link_exists(IFNAME) && !c.link_exists("eth9"));
    // keys cross the socket and arrive as the same bytes
    let k = Secret::new(vec![0x11; 32]);
    let psk = Secret::new(vec![0x33; 32]);
    let pubk = PublicKey([0x22; 32]);
    let allowed = [Cidr::parse("0.0.0.0/0").unwrap()];
    let mut p = PeerSet::new(&pubk, &allowed);
    p.preshared = Some(&psk);
    p.endpoint = Some("203.0.113.9:51820".parse().unwrap());
    p.keepalive = Some(25);
    let dev = DeviceSet { ifname: IFNAME, private_key: Some(&k), listen_port: None, fwmark: None, replace_peers: true, peers: vec![p] };
    c.wg_configure(&dev).unwrap();
    assert_eq!(r.fake.st.lock().unwrap().last_private.as_deref(), Some(&[0x11u8; 32][..]));
    r.fake.st.lock().unwrap().handshake = Some(1234);
    let st = c.wg_status(IFNAME).unwrap();
    assert_eq!(st.peers.len(), 1);
    assert_eq!(st.peers[0].last_handshake, Some(1234));
    assert_eq!(st.peers[0].public_key, pubk);
    assert_eq!(st.peers[0].endpoint, Some("203.0.113.9:51820".parse().unwrap()));
    // resolv.conf and its backup
    assert_eq!(c.read_resolv().as_deref(), Some("nameserver 192.168.1.1\n"));
    c.write_backup("nameserver 192.168.1.1\n").unwrap();
    c.write_resolv("nameserver 10.8.0.1\n").unwrap();
    assert_eq!(c.read_backup().as_deref(), Some("nameserver 192.168.1.1\n"));
    c.drop_backup();
    assert!(c.read_backup().is_none());
    // the guard and the DNS capture keep their typed content (the reasons are display text and stay behind)
    let phys = ["eth0".to_string()];
    let lan = [Cidr::parse("192.168.1.0/24").unwrap()];
    let cluster = [Cidr::parse("10.42.0.0/16").unwrap()];
    let eps = [("203.0.113.9".parse().unwrap(), 51820, plan::Proto::Udp)];
    let base = GuardBase { ifname: IFNAME, physical_ifs: &phys, endpoints: &eps, lan: &lan, cluster: &cluster, cluster_if_prefixes: &["cni"], dev_port: 7979 };
    let spec = plan::kill_switch(&KillSwitchInput { base, block_dns_outside: false }).unwrap();
    c.install_guard(&spec).unwrap();
    assert!(c.guard_present());
    {
        let got = r.fake.st.lock().unwrap().guard.clone().unwrap();
        assert_eq!(got.rules.len(), spec.rules.len());
        assert!(got.rules.iter().zip(&spec.rules).all(|(a, b)| a.m == b.m && a.verdict == b.verdict));
    }
    c.guard_set("198.51.100.7".parse().unwrap(), Some(60), true).unwrap();
    assert!(r.fake.st.lock().unwrap().guard_members.contains("198.51.100.7"));
    c.guard_set("198.51.100.7".parse().unwrap(), None, false).unwrap();
    c.remove_guard();
    assert!(!c.guard_present());
    c.install_dns_capture(&plan::dns_capture(15353).unwrap()).unwrap();
    assert_eq!(r.fake.st.lock().unwrap().capture.as_ref().map(|c| c.listen_port), Some(15353));
    c.remove_dns_capture();
    assert!(r.fake.st.lock().unwrap().capture.is_none());
    c.ip(&plan::link_del_args(IFNAME).unwrap()).unwrap();
    assert!(r.fake.clean());
}

#[test]
fn the_helper_runs_only_the_ip_commands_the_plans_produce() {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    for ok in [
        plan::link_add_args(IFNAME).unwrap(),
        plan::link_del_args(IFNAME).unwrap(),
        plan::link_up_args(IFNAME, None).unwrap(),
        plan::link_up_args(IFNAME, Some(1380)).unwrap(),
        plan::addr_add_args(IFNAME, &Cidr::parse("10.8.0.2/24").unwrap()).unwrap(),
        plan::Route { dest: Cidr::parse("0.0.0.0/1").unwrap(), via: None, dev: IFNAME.into() }.ip_args(true),
        plan::Route { dest: Cidr::parse("203.0.113.9/32").unwrap(), via: Some("192.168.1.1".parse().unwrap()), dev: "eth0".into() }.ip_args(false),
    ] {
        assert!(validate_ip_args(&ok).is_ok(), "{ok:?}");
    }
    for bad in [
        s(&[]),
        s(&["link", "add", "dev", "eth0", "type", "wireguard"]),
        s(&["link", "del", "dev", "eth0"]),
        s(&["link", "set", "dev", "eth0", "down"]),
        s(&["link", "add", "dev", IFNAME, "type", "bridge"]),
        s(&["address", "add", "10.0.0.1/8", "dev", "eth0"]),
        s(&["route", "add", "default", "dev", "eth0"]),
        s(&["route", "add", "0.0.0.0/0", "dev", IFNAME]),
        s(&["route", "add", "::/0", "dev", IFNAME]),
        s(&["route", "add", "10.0.0.0/8", "dev", "eth0;reboot"]),
        s(&["-batch", "/tmp/x"]),
        s(&["netns", "exec", "x", "sh"]),
        s(&["route", "flush", "all"]),
        s(&["link", "set", "dev", IFNAME, "mtu", "99999", "up"]),
        s(&["route", "add", "10.0.0.0/8", "via", "not-an-ip", "dev", "eth0"]),
        s(&["route", "add", "10.0.0.0/8", "dev", &"a".repeat(80)]),
    ] {
        assert!(validate_ip_args(&bad).is_err(), "{bad:?}");
    }
    // and the helper applies the same rule: nothing reaches the system layer
    let fake = FakeSys::new();
    for bad in [s(&["link", "del", "dev", "eth0"]), s(&["route", "add", "default", "dev", "eth0"])] {
        assert!(!dispatch(&fake, Req::Ip { args: bad }).ok);
    }
    assert!(fake.calls().is_empty());
}

#[test]
fn other_requests_are_checked_again_by_the_helper() {
    let fake = FakeSys::new();
    let wire = |ifname: &str, key: Option<&str>| WgWire { ifname: ifname.into(), private_key: key.map(String::from), listen_port: None, fwmark: None, replace_peers: true, peers: vec![] };
    assert!(!dispatch(&fake, Req::WgConfigure { dev: wire("eth0", None) }).ok, "only the VPN interface");
    assert!(!dispatch(&fake, Req::WgConfigure { dev: wire(IFNAME, Some("short")) }).ok, "a key must be 32 bytes");
    assert!(!dispatch(&fake, Req::WgStatus { ifname: "eth0".into() }).ok);
    assert!(!dispatch(&fake, Req::WriteResolv { text: "x".repeat(5000) }).ok);
    assert!(!dispatch(&fake, Req::WriteBackup { text: "x".repeat(5000) }).ok);
    assert!(!dispatch(&fake, Req::Resolve { host: "a b".into() }).ok);
    assert!(!dispatch(&fake, Req::LinkExists { ifname: "../x".into() }).data_true());
    assert!(fake.calls().iter().all(|c| !c.contains("configure") && !c.contains("resolv write")), "{:?}", fake.calls());
}

impl Resp {
    fn data_true(&self) -> bool {
        self.ok && self.value == json!(true)
    }
}

#[test]
fn garbage_is_answered_with_an_error_and_an_overlong_line_ends_the_connection() {
    let r = rig("garbage", PeerPolicy::only(my_uid()));
    let (mut w, mut rd) = raw(&r.helper.path);
    let v = ask(&mut w, &mut rd, "this is not json");
    assert_eq!(v["ok"], json!(false));
    let v = ask(&mut w, &mut rd, r#"{"op":"no_such_op"}"#);
    assert_eq!(v["ok"], json!(false));
    let v = ask(&mut w, &mut rd, r#"{"op":"ping"}"#);
    assert_eq!((v["ok"].clone(), v["value"].clone()), (json!(true), json!("pong")), "the connection still works");
    // a request that arrives in two pieces is put together
    w.write_all(br#"{"op":"pi"#).unwrap();
    std::thread::sleep(Duration::from_millis(700));
    w.write_all(b"ng\"}\n").unwrap();
    let mut out = String::new();
    rd.read_line(&mut out).unwrap();
    assert!(out.contains("pong"), "{out}");
    // too long: closed
    let (mut w2, mut r2) = raw(&r.helper.path);
    let big = vec![b'a'; MAX_LINE + 10];
    let _ = w2.write_all(&big);
    let mut sink = Vec::new();
    let _ = r2.read_to_end(&mut sink);
    assert!(sink.is_empty(), "no answer to an over-long line; the connection just ends");
    // and the helper is still there for the next client
    assert!(RemoteSys::new(&r.helper.path).call(&Req::Ping).is_ok());
}

#[test]
fn the_client_reconnects_after_the_helper_restarts() {
    let dir = temp("restart");
    let path = dir.join("h.sock");
    let fake = Arc::new(FakeSys::new());
    let h1 = serve(bind_socket(&path, None).unwrap(), path.clone(), PeerPolicy::only(my_uid()), fake.clone()).unwrap();
    let c = RemoteSys::new(&path);
    assert!(c.call(&Req::Ping).is_ok());
    drop(h1);
    assert!(c.call(&Req::Ping).is_err(), "nobody home");
    let _h2 = serve(bind_socket(&path, None).unwrap(), path.clone(), PeerPolicy::only(my_uid()), fake).unwrap();
    assert!(c.call(&Req::Ping).is_ok(), "a fresh connection is made");
}

#[test]
fn a_key_never_reaches_the_helper_log_or_an_error_text() {
    let r = rig("canary", PeerPolicy::only(my_uid()));
    let c = RemoteSys::new(&r.helper.path);
    let k = Secret::new(b"CANARYCANARYCANARYCANARYCANARY12".to_vec());
    let pubk = PublicKey([0x22; 32]);
    let allowed = [Cidr::parse("10.0.0.0/8").unwrap()];
    // a configure that the system layer refuses: the error comes back, the key does not
    r.fake.fail("wg configure");
    let dev = DeviceSet { ifname: IFNAME, private_key: Some(&k), listen_port: None, fwmark: None, replace_peers: true, peers: vec![PeerSet::new(&pubk, &allowed)] };
    let e = c.wg_configure(&dev).unwrap_err();
    let b64k = b64::encode(k.expose());
    let everything = format!("{e}{:?}{:?}", r.helper.log.lock().unwrap(), r.fake.calls());
    assert!(!everything.contains(&b64k) && !everything.contains("CANARY"), "{everything}");
    assert!(r.helper.log.lock().unwrap().contains(&"wg_configure".to_string()));
}

#[test]
fn the_manager_runs_unchanged_over_the_helper_and_leaves_nothing_behind() {
    let r = rig("manager", PeerPolicy::only(my_uid()));
    let dir = temp("manager-data");
    let audit: AuditFn = Arc::new(|_, _| {});
    let m = Manager::new(&dir, Arc::new(MemStore::default()), Arc::new(RemoteSys::new(&r.helper.path)), audit);
    let id = m.import("Office", conf("0.0.0.0/0, ::/0", "vpn.example.com:51820").as_bytes()).unwrap().profile.id;
    m.set_settings(&id, Settings { kill_switch: true, ..Settings::full() }, 1000).unwrap();
    m.set_enabled(&id, true, 1000).unwrap();
    assert_eq!(m.status(&id).state, State::Connecting, "{:?}", m.status(&id));
    {
        let s = r.fake.st.lock().unwrap();
        assert!(s.links.contains(IFNAME) && s.guard.is_some());
        assert!(s.routes.contains(&format!("0.0.0.0/1 dev {IFNAME}")));
        assert_eq!(s.last_private.as_deref(), Some(&[0x11u8; 32][..]), "the vault's key reached the kernel stand-in");
    }
    r.fake.st.lock().unwrap().handshake = Some(1000);
    m.tick(1001);
    assert!(matches!(m.status(&id).state, State::Connected { .. }), "status over the socket: {:?}", m.status(&id));
    // a hybrid profile with a learned route works the same way
    m.set_enabled(&id, false, 1002).unwrap();
    m.set_settings(&id, Settings::hybrid(FilterSet::parse("198.51.100.0/24").unwrap()), 1002).unwrap();
    m.set_enabled(&id, true, 1003).unwrap();
    assert!(r.fake.st.lock().unwrap().guard.is_some(), "the fail-closed guard went through the helper");
    m.set_enabled(&id, false, 1004).unwrap();
    assert!(r.fake.clean(), "disable through the helper removes everything");
}

#[test]
fn the_helper_setting_is_off_unless_it_was_turned_on() {
    let dir = temp("setting");
    assert!(!helper_enabled(&dir));
    assert_eq!(read_setting(&dir).socket, DEFAULT_SOCKET);
    write_setting(&dir, &HelperSetting { enabled: true, socket: "/run/x/h.sock".into() }).unwrap();
    assert!(helper_enabled(&dir));
    assert_eq!(read_setting(&dir).socket, "/run/x/h.sock");
    std::fs::write(dir.join("helper.json"), "garbage").unwrap();
    assert!(!helper_enabled(&dir), "an unreadable setting means off");
    assert_eq!(std::fs::metadata(dir.join("helper.json")).unwrap().permissions().mode() & 0o777, 0o600);
}
