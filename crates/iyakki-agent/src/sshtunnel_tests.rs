//! SSH tunnels: the profile rules, the file, the command line and its allow-list, the words for the client's errors, and the manager with
//! a fake client (no network, no SSH server, no root). What a real OpenSSH does with these arguments is checked on a machine with a host
//! (docs/SSH-TUNNELS.md, "What is verified").

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::gitkeys::{GitKeys, MemStore, Session};
use crate::ssh::HostKey;
use crate::sshtunnel::*;
use crate::sshtunnel_mgr::*;

fn temp(tag: &str) -> PathBuf {
    let base = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into());
    let mut r = [0u8; 3];
    iyakki_sec::random_fill(&mut r).unwrap();
    let d = Path::new(&base).join(format!("tn-{tag}-{:02x}{:02x}{:02x}", r[0], r[1], r[2]));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn host_key(seed: &str) -> HostKey {
    HostKey { pattern: String::new(), kind: "ssh-ed25519".into(), b64: crate::story::base64_encode(format!("fake-host-key-{seed}").as_bytes()) }
}

struct Rig {
    m: Arc<Manager>,
    runner: Arc<FakeRunner>,
    ports: Arc<FakePorts>,
    keys: Arc<GitKeys>,
    store: Arc<MemStore>,
    log: Arc<Mutex<Vec<String>>>,
    shown: Arc<Mutex<HashMap<(String, u16), Vec<HostKey>>>>,
    dir: PathBuf,
}

fn rig(tag: &str) -> Rig {
    let dir = temp(tag);
    let store = Arc::new(MemStore::default());
    let log = Arc::new(Mutex::new(Vec::new()));
    let l2 = log.clone();
    let audit: crate::gitkeys::AuditFn = Arc::new(move |a, b| l2.lock().unwrap().push(format!("{a} {b}")));
    let keys = Arc::new(GitKeys::new(&dir, store.clone(), audit.clone(), Arc::new(Session::default())));
    let shown: Arc<Mutex<HashMap<(String, u16), Vec<HostKey>>>> = Arc::new(Mutex::new(HashMap::new()));
    let s2 = shown.clone();
    let probe: crate::gitjobs::Probe = Arc::new(move |h, p| s2.lock().unwrap().get(&(h.to_string(), p)).cloned().ok_or_else(|| format!("{h}:{p} sent no host key")));
    let runner = Arc::new(FakeRunner::default());
    let ports = Arc::new(FakePorts::default());
    let m = Arc::new(Manager::new(&dir, keys.clone(), probe, runner.clone(), ports.clone(), audit).with_pause(Arc::new(|_| {})));
    Rig { m, runner, ports, keys, store, log, shown, dir }
}

impl Rig {
    /// The host presents this key and the person has trusted it.
    fn trusted(&self, host: &str, port: u16) {
        let k = host_key(host);
        self.shown.lock().unwrap().insert((host.into(), port), vec![k.clone()]);
        self.keys.known_hosts().trust(host, port, &[k]).unwrap();
    }
    fn key(&self, id: &str) {
        self.keys.generate(id, "fake@example.invalid").unwrap();
    }
    fn add(&self, name: &str, port: u16, enabled: bool) -> Tunnel {
        let t = self.m.save(Tunnel { enabled, ..sample(name, "work", port) }, false).unwrap();
        t
    }
}

fn sample(name: &str, key: &str, port: u16) -> Tunnel {
    Tunnel { name: name.into(), host: "bastion.example.invalid".into(), port: 22, user: "alice".into(), key: key.into(), kind: Kind::LocalForward, local_port: port, target_host: "intranet.example.invalid".into(), target_port: 8080, ..Default::default() }
}

fn ready(tag: &str) -> Rig {
    let r = rig(tag);
    r.key("work");
    r.trusted("bastion.example.invalid", 22);
    r
}

// ------------------------------------------------------------------ the profile

#[test]
fn a_profile_is_checked_field_by_field() {
    let ok = Tunnel { id: "t1".into(), ..sample("Docs", "work", 8080) };
    assert_eq!(ok.validate(), Ok(()));
    let bad = |f: &dyn Fn(&mut Tunnel)| {
        let mut t = ok.clone();
        f(&mut t);
        assert!(t.validate().is_err(), "{t:?}");
    };
    bad(&|t| t.name = "".into());
    bad(&|t| t.name = "x".repeat(41));
    bad(&|t| t.name = "a\nb".into());
    bad(&|t| t.host = "".into());
    bad(&|t| t.host = "-oProxyCommand=id".into());
    bad(&|t| t.host = "a b".into());
    bad(&|t| t.host = "a;b".into());
    bad(&|t| t.host = "host\n.example".into());
    bad(&|t| t.host = "::1".into());
    bad(&|t| t.host = "h".repeat(254));
    bad(&|t| t.port = 0);
    bad(&|t| t.user = "".into());
    bad(&|t| t.user = "-oLocalCommand=id".into());
    bad(&|t| t.user = "a@b".into());
    bad(&|t| t.key = "".into());
    bad(&|t| t.key = "../x".into());
    bad(&|t| t.local_port = 80);
    bad(&|t| t.local_port = 0);
    bad(&|t| t.target_host = "".into());
    bad(&|t| t.target_host = "x y".into());
    bad(&|t| t.target_port = 0);
    bad(&|t| t.id = "../x".into());
    bad(&|t| t.jump = vec![Hop { host: "j.example.invalid".into(), port: 22, user: "".into() }]);
    bad(&|t| t.jump = vec![Hop { host: "j;x".into(), port: 22, user: "u".into() }]);
    bad(&|t| t.jump = vec![Hop { host: "j.example.invalid".into(), port: 22, user: "u".into() }; MAX_JUMPS + 1]);
    let mut socks = Tunnel { kind: Kind::DynamicSocks, target_host: String::new(), target_port: 0, ..ok.clone() };
    assert_eq!(socks.validate(), Ok(()));
    socks.target_host = "x.example.invalid".into();
    assert!(socks.validate().is_err(), "a SOCKS proxy has no single target");
}

#[test]
fn the_address_to_open_and_the_route_are_in_words() {
    let t = Tunnel { id: "t1".into(), jump: vec![Hop { host: "gate.example.invalid".into(), port: 2222, user: "bob".into() }], ..sample("Docs", "work", 8123) };
    assert_eq!(t.address(), "http://127.0.0.1:8123/");
    assert_eq!(Tunnel { kind: Kind::DynamicSocks, ..t.clone() }.address(), "socks5h://127.0.0.1:8123");
    assert_eq!(t.route_text(), "gate.example.invalid > alice@bastion.example.invalid:22");
    assert_eq!(t.hops(), vec![("gate.example.invalid".to_string(), 2222), ("bastion.example.invalid".to_string(), 22)]);
}

// ------------------------------------------------------------------ the file

#[test]
fn the_file_has_no_secret_is_private_and_keeps_names_and_ports_apart() {
    let r = ready("store");
    let t = r.add("Docs", 8080, true);
    assert_eq!(t.id, "t1");
    let path = r.dir.join("tunnels.json");
    let text = std::fs::read_to_string(&path).unwrap();
    let private = r.store.map.lock().unwrap().get("ssh.work").unwrap().clone();
    assert!(!text.contains("PRIVATE") && !text.contains(private.lines().nth(1).unwrap_or("zzz")), "no key text in the file");
    assert!(text.contains("\"key\": \"work\""), "the key is a name only");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    // a second tunnel: its own id, sorted by name
    let b = r.m.save(sample("Alpha", "work", 8081), false).unwrap();
    assert_eq!(b.id, "t2");
    assert_eq!(r.m.tunnels().iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), vec!["Alpha", "Docs"]);
    // the same name (any case) and the same port are refused, in words
    assert!(r.m.save(sample("docs", "work", 8090), false).unwrap_err().contains("already a tunnel called Docs"));
    assert!(r.m.save(sample("Third", "work", 8080), false).unwrap_err().contains("already used by the tunnel Docs"));
    // a key that does not exist
    assert!(r.m.save(sample("Fourth", "ghost", 8095), false).unwrap_err().contains("no key called ghost"));
    // damaged ids in the file are dropped on load
    std::fs::write(&path, r#"[{"id":"../../x","name":"evil"}]"#).unwrap();
    assert!(r.m.tunnels().is_empty());
}

#[test]
fn there_is_a_limit_on_the_number_of_tunnels() {
    let r = ready("limit");
    for i in 0..MAX_TUNNELS {
        r.m.save(sample(&format!("n{i}"), "work", 20000 + i as u16), false).unwrap();
    }
    assert!(r.m.save(sample("one more", "work", 29999), false).unwrap_err().contains("at most 32"));
}

// ------------------------------------------------------------------ the command line

fn kh() -> PathBuf {
    PathBuf::from("/data/ssh/u/home/.ssh/known_hosts")
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

#[test]
fn the_local_forward_command_is_exactly_this() {
    let t = Tunnel { id: "t1".into(), ..sample("Docs", "work", 8080) };
    let a = build_args(&t, &kh()).unwrap();
    let mut want = s(&["-N", "-T"]);
    for o in [
        "BatchMode=yes", "StrictHostKeyChecking=yes", "GlobalKnownHostsFile=/dev/null", "UpdateHostKeys=no", "VerifyHostKeyDNS=no", "PreferredAuthentications=publickey",
        "PubkeyAuthentication=yes", "PasswordAuthentication=no", "KbdInteractiveAuthentication=no", "IdentitiesOnly=no", "ForwardAgent=no", "ForwardX11=no", "PermitLocalCommand=no",
        "ControlMaster=no", "ControlPath=none", "ExitOnForwardFailure=yes", "ServerAliveInterval=15", "ServerAliveCountMax=3", "ConnectTimeout=15", "LogLevel=ERROR",
        "UserKnownHostsFile=/data/ssh/u/home/.ssh/known_hosts",
    ] {
        want.push("-o".into());
        want.push(o.into());
    }
    want.extend(s(&["-p", "22", "-L", "127.0.0.1:8080:intranet.example.invalid:8080", "--", "alice@bastion.example.invalid"]));
    assert_eq!(a, want);
    assert_eq!(build_args(&t, &kh()).unwrap(), a, "pure: the same profile, the same line");
}

#[test]
fn socks_and_a_jump_chain_are_built_from_validated_fields() {
    let t = Tunnel {
        id: "t2".into(),
        kind: Kind::DynamicSocks,
        target_host: String::new(),
        target_port: 0,
        jump: vec![Hop { host: "Gate.example.invalid".into(), port: 2222, user: "bob".into() }, Hop { host: "gate2.example.invalid".into(), port: 22, user: "carol".into() }],
        ..sample("Everything", "work", 1080)
    };
    let a = build_args(&t, &kh()).unwrap();
    let at = |flag: &str| a.iter().position(|x| x == flag).map(|i| a[i + 1].clone());
    assert_eq!(at("-D").as_deref(), Some("127.0.0.1:1080"));
    assert_eq!(at("-J").as_deref(), Some("bob@Gate.example.invalid:2222,carol@gate2.example.invalid:22"));
    assert!(!a.contains(&"-L".to_string()));
    assert_eq!(a.last().unwrap(), "alice@bastion.example.invalid");
}

#[test]
fn no_profile_can_make_a_dangerous_command() {
    let base = Tunnel { id: "t1".into(), ..sample("Docs", "work", 8080) };
    for evil in ["-oProxyCommand=touch /x", "a;id", "a b", "a\nb", "$(id)", "`id`", "a|b", "a'b", "a\"b", "%h", "~/x", "--help", "-F/etc/passwd"] {
        for f in [
            |t: &mut Tunnel, e: &str| t.host = e.into(),
            |t: &mut Tunnel, e: &str| t.user = e.into(),
            |t: &mut Tunnel, e: &str| t.target_host = e.into(),
            |t: &mut Tunnel, e: &str| t.jump = vec![Hop { host: e.into(), port: 22, user: "u".into() }],
            |t: &mut Tunnel, e: &str| t.jump = vec![Hop { host: "j.example.invalid".into(), port: 22, user: e.into() }],
        ] {
            let mut t = base.clone();
            f(&mut t, evil);
            assert!(build_args(&t, &kh()).is_err(), "{evil:?} must be refused: {t:?}");
        }
    }
    // a path with a space or a quote cannot be passed as the known hosts file
    assert!(build_args(&base, Path::new("/data/a b/known_hosts")).is_err());
    assert!(build_args(&base, Path::new("/data/a\"b/known_hosts")).is_err());
    // whatever is built never contains these options or flags
    let a = build_args(&base, &kh()).unwrap();
    for w in &a {
        let l = w.to_ascii_lowercase();
        for bad in ["proxycommand", "localcommand=", "include", "identityfile", "controlpersist", "remoteforward", "password="] {
            assert!(!l.starts_with(bad), "{w}");
        }
        assert!(!["-F", "-S", "-R", "-i", "-W", "-w", "-f", "-A", "-X", "-g"].contains(&w.as_str()), "{w}");
        assert!(!w.contains("0.0.0.0"), "{w}");
    }
}

#[test]
fn the_allow_list_refuses_everything_that_is_not_one_of_its_shapes() {
    let t = Tunnel { id: "t1".into(), ..sample("Docs", "work", 8080) };
    let good = build_args(&t, &kh()).unwrap();
    assert_eq!(check_args(&good, &kh()), Ok(()));
    let with = |extra: &[&str]| {
        let mut a = good.clone();
        let at = a.len() - 2;
        for (i, e) in extra.iter().enumerate() {
            a.insert(at + i, e.to_string());
        }
        a
    };
    for extra in [
        &["-o", "ProxyCommand=nc x 1"][..], &["-o", "LocalCommand=id"], &["-o", "Include=/x"], &["-o", "IdentityFile=/x"], &["-o", "ProxyJump=x"], &["-o", "PermitLocalCommand=yes"],
        &["-o", "StrictHostKeyChecking=no"], &["-o", "StrictHostKeyChecking=accept-new"], &["-o", "BatchMode=no"], &["-o", "UserKnownHostsFile=/etc/other"], &["-o", "ServerAliveInterval=0"],
        &["-o", "RemoteForward=1 x:2"], &["-o", "GatewayPorts=yes"], &["-o", "NoName"], &["-F", "/x"], &["-S", "/x"], &["-R", "9:x:9"], &["-W", "x:1"], &["-i", "/k"], &["-E", "/log"],
        &["-w", "0:0"], &["-f"], &["-A"], &["-X"], &["-Y"], &["-g"], &["-M"], &["-t"], &["-v"], &["-l", "root"], &["-D", "127.0.0.1:2000"], &["-L", "127.0.0.1:2000:x:1"], &["-p", "0"],
    ] {
        assert!(check_args(&with(extra), &kh()).is_err(), "{extra:?} must be refused");
    }
    // a bind to anything but 127.0.0.1
    for bad in [
        s(&["-N", "-D", "0.0.0.0:1080", "--", "a@b"]),
        s(&["-N", "-D", "1080", "--", "a@b"]),
        s(&["-N", "-D", "*:1080", "--", "a@b"]),
        s(&["-N", "-D", "localhost:1080", "--", "a@b"]),
        s(&["-N", "-D", "127.0.0.1:80", "--", "a@b"]),
    ] {
        assert!(check_args(&bad, &kh()).is_err(), "{bad:?}");
    }
    let swap = |from: &str, to: &str| good.iter().map(|x| x.replace(from, to)).collect::<Vec<_>>();
    assert!(check_args(&swap("-L", "-R"), &kh()).is_err());
    assert!(check_args(&swap("127.0.0.1:8080", "0.0.0.0:8080"), &kh()).is_err());
    assert!(check_args(&swap("127.0.0.1:8080", "[::]:8080"), &kh()).is_err());
    // the destination is last, once
    let mut late = good.clone();
    late.push("-N".into());
    assert!(check_args(&late, &kh()).is_err());
    // missing pieces
    assert!(check_args(&good[..good.len() - 2], &kh()).is_err(), "no destination");
    assert!(check_args(&s(&["-N", "--", "a@b"]), &kh()).is_err(), "no forward and no known hosts file");
    // the known hosts file must be the person's own
    assert!(check_args(&good, Path::new("/other/known_hosts")).is_err());
}

// ------------------------------------------------------------------ the words

#[test]
fn the_texts_of_openssh_become_plain_words() {
    let cases: &[(&str, Fault)] = &[
        ("@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\nOffending ED25519 key in /x/known_hosts:1\nHost key verification failed.", Fault::HostKeyChanged),
        ("No ED25519 host key is known for bastion.example.invalid and you have requested strict checking.\nHost key verification failed.", Fault::NotTrusted),
        ("Host key verification failed.", Fault::NotTrusted),
        ("alice@bastion.example.invalid: Permission denied (publickey).", Fault::PermissionDenied),
        ("Received disconnect from 192.0.2.1 port 22:2: Too many authentication failures", Fault::PermissionDenied),
        ("ssh: connect to host bastion.example.invalid port 22: Connection refused", Fault::Refused),
        ("ssh: connect to host bastion.example.invalid port 22: Connection timed out", Fault::Timeout),
        ("ssh: connect to host bastion.example.invalid port 22: Operation timed out", Fault::Timeout),
        ("ssh: connect to host 192.0.2.1 port 22: No route to host", Fault::NoRoute),
        ("ssh: connect to host 192.0.2.1 port 22: Network is unreachable", Fault::NoRoute),
        ("ssh: Could not resolve hostname bastion.example.invalid: Name or service not known", Fault::NoName),
        ("bind [127.0.0.1]:8080: Address already in use\nchannel_setup_fwd_listener_tcpip: cannot listen to port: 8080\nCould not request local forwarding.", Fault::AddressInUse),
        ("Timeout, server 192.0.2.1 not responding.", Fault::Dropped),
        ("Connection to 192.0.2.1 closed by remote host.", Fault::Dropped),
        ("client_loop: send disconnect: Broken pipe", Fault::Dropped),
        ("kex_exchange_identification: read: Connection reset by peer", Fault::Dropped),
        ("something nobody has seen", Fault::Other),
        ("", Fault::Other),
    ];
    for (text, want) in cases {
        assert_eq!(classify(text), *want, "{text}");
    }
    // changed key and a login failure in one run: the key is what the person must hear about
    assert_eq!(classify("REMOTE HOST IDENTIFICATION HAS CHANGED\nPermission denied (publickey)."), Fault::HostKeyChanged);
    // only what can pass by itself is retried
    for f in [Fault::NotTrusted, Fault::HostKeyChanged, Fault::PermissionDenied, Fault::AddressInUse, Fault::KeyUnavailable] {
        assert!(!f.retryable() && !f.words().is_empty(), "{f:?}");
    }
    for f in [Fault::Refused, Fault::Timeout, Fault::NoRoute, Fault::NoName, Fault::Dropped, Fault::Other] {
        assert!(f.retryable(), "{f:?}");
    }
    assert_eq!(ago(5), "just now");
    assert_eq!(ago(300), "5 min");
    assert_eq!(ago(7300), "2 h");
    assert_eq!(ago(200_000), "2 days");
}

// ------------------------------------------------------------------ ports

#[test]
fn a_taken_port_is_reported_and_an_empty_one_is_picked() {
    let r = ready("ports");
    // empty: the system picks and it is kept
    let a = r.m.save(sample("Auto", "work", 0), false).unwrap();
    assert_eq!(a.local_port, 40000);
    let b = r.m.save(sample("Auto two", "work", 0), false).unwrap();
    assert_eq!(b.local_port, 40001, "two tunnels never get the same port");
    // another tunnel owns it
    assert!(r.m.save(sample("Clash", "work", 40000), false).unwrap_err().contains("already used by the tunnel Auto"));
    // another program owns it: never silently reused
    r.ports.taken.lock().unwrap().insert(9000);
    let e = r.m.save(sample("Busy", "work", 9000), false).unwrap_err();
    assert!(e.contains("another program") && e.contains("9000"), "{e}");
    // a port the OS would give but another (disabled) tunnel keeps is skipped
    r.ports.taken.lock().unwrap().clear();
    let c = r.m.save(sample("Auto three", "work", 0), false).unwrap();
    assert_eq!(c.local_port, 40002);
    // editing keeps the tunnel's own port; changing to a busy one is refused and the old one stays
    let mut e = r.m.get(&a.id).unwrap();
    e.target_port = 9090;
    assert_eq!(r.m.save(e, false).unwrap().local_port, 40000);
    r.ports.taken.lock().unwrap().insert(9100);
    let mut e = r.m.get(&a.id).unwrap();
    e.local_port = 9100;
    assert!(r.m.save(e, false).is_err());
    assert_eq!(r.m.get(&a.id).unwrap().local_port, 40000);
}

#[test]
fn a_running_tunnel_may_be_edited_although_its_own_port_is_busy() {
    let r = ready("ownport");
    let t = r.add("Docs", 8080, true);
    r.m.tick(100);
    r.m.tick(100);
    assert!(matches!(r.m.status(&t.id).state, State::Connected { .. }));
    r.ports.taken.lock().unwrap().insert(8080); // our own client listens there
    let mut e = r.m.get(&t.id).unwrap();
    e.name = "Docs renamed".into();
    assert!(r.m.save(e, false).is_ok(), "the port is ours");
    assert_eq!(r.runner.alive(), 0, "an edited tunnel stops; the next tick starts it with the new profile");
    r.m.tick(101);
    assert_eq!(r.runner.alive(), 1);
}

// ------------------------------------------------------------------ enabled, disabled

#[test]
fn a_disabled_tunnel_is_kept_and_never_started() {
    let r = ready("disabled");
    let t = r.add("Docs", 8080, false);
    for n in 0..5 {
        r.m.tick(100 + n);
    }
    assert!(r.runner.launches.lock().unwrap().is_empty());
    assert_eq!(r.m.status(&t.id).state, State::Stopped);
    assert_eq!(r.m.status(&t.id).line(false, 100), "disabled");
    assert_eq!(r.m.tunnels().len(), 1, "kept");
}

#[test]
fn enabling_starts_it_and_disabling_stops_it_again() {
    let r = ready("enable");
    let t = r.add("Docs", 8080, false);
    r.m.set_enabled(&t.id, true).unwrap();
    r.m.tick(100);
    r.m.tick(100);
    assert_eq!(r.m.status(&t.id).state, State::Connected { since: 100 });
    assert_eq!(r.runner.alive(), 1);
    let launches = r.runner.launches.lock().unwrap().clone();
    assert_eq!(launches.len(), 1);
    assert_eq!(launches[0].0, t.id);
    assert_eq!(launches[0].2, 8080);
    assert_eq!(launches[0].3, "work", "the key is passed by name; its private half goes to the agent, never to the command line");
    check_args(&launches[0].1, &r.keys.known_hosts_path()).unwrap();
    assert_eq!(r.m.status(&t.id).line(true, 100 + 600), "connected, 10 min ago");
    r.m.set_enabled(&t.id, false).unwrap();
    assert_eq!(r.runner.alive(), 0);
    r.m.tick(200);
    assert_eq!(r.runner.launches.lock().unwrap().len(), 1, "not started again");
    assert!(!r.m.get(&t.id).unwrap().enabled);
    // the file says so, so it also holds after a restart of the device
    assert!(std::fs::read_to_string(r.dir.join("tunnels.json")).unwrap().contains("\"enabled\": false"));
}

#[test]
fn several_tunnels_run_at_once_with_their_own_state() {
    let r = ready("several");
    let a = r.add("A", 8001, true);
    let b = r.add("B", 8002, true);
    let c = r.add("C", 8003, false);
    r.runner.plan(Plan::Connect);
    r.runner.plan(Plan::Fail("ssh: connect to host bastion.example.invalid port 22: Connection refused".into()));
    r.m.tick(100);
    assert_eq!(r.runner.launches.lock().unwrap().len(), 2, "both enabled ones started, the disabled one did not");
    r.m.tick(101);
    assert!(matches!(r.m.status(&a.id).state, State::Connected { since: 101 }));
    let sb = r.m.status(&b.id);
    assert_eq!((sb.state, sb.fault), (State::Stopped, Some(Fault::Refused)), "B failed on its own and did not take A down");
    assert_eq!(r.m.status(&c.id).state, State::Stopped);
    assert_eq!(r.runner.alive(), 1);
}

// ------------------------------------------------------------------ reconnecting

#[test]
fn a_dropped_connection_comes_back_after_a_growing_pause() {
    let r = ready("backoff");
    let t = r.add("Docs", 8080, true);
    r.m.tick(1000);
    r.m.tick(1000);
    assert!(matches!(r.m.status(&t.id).state, State::Connected { .. }));
    r.runner.drop_all("Timeout, server 192.0.2.1 not responding.");
    r.m.tick(1010);
    let s = r.m.status(&t.id);
    assert_eq!((s.state, s.fault, s.retry_at), (State::Stopped, Some(Fault::Dropped), Some(1012)), "first pause after a connected run is short: {s:?}");
    assert!(s.error.unwrap().contains("dropped"));
    r.m.tick(1011);
    assert_eq!(r.runner.launches.lock().unwrap().len(), 1, "not before the pause is over");
    // it keeps failing: 2, 4, 8, 16 … seconds
    let mut now = 1012;
    let mut waits = vec![];
    for _ in 0..5 {
        r.runner.plan(Plan::Fail("ssh: connect to host bastion.example.invalid port 22: Connection refused".into()));
        r.m.tick(now); // starts
        r.m.tick(now); // sees it ended
        let s = r.m.status(&t.id);
        waits.push(s.retry_at.unwrap() - now);
        now = s.retry_at.unwrap();
    }
    assert_eq!(waits, vec![4, 8, 16, 32, 64]);
    // and never above five minutes
    for _ in 0..8 {
        r.runner.plan(Plan::Fail("Connection timed out".into()));
        r.m.tick(now);
        r.m.tick(now);
        let s = r.m.status(&t.id);
        assert!(s.retry_at.unwrap() - now <= BACKOFF_MAX);
        now = s.retry_at.unwrap();
    }
    // it connects at last and the count starts over
    r.m.tick(now);
    r.m.tick(now);
    assert!(matches!(r.m.status(&t.id).state, State::Connected { .. }));
    assert_eq!(r.m.status(&t.id).attempts, 0);
    assert_eq!(r.m.status(&t.id).error, None);
}

#[test]
fn faults_that_need_the_person_are_not_retried() {
    for (text, fault) in [
        ("Permission denied (publickey).", Fault::PermissionDenied),
        ("WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!\nHost key verification failed.", Fault::HostKeyChanged),
        ("bind [127.0.0.1]:8080: Address already in use\nCould not request local forwarding.", Fault::AddressInUse),
    ] {
        let r = ready("hard");
        let t = r.add("Docs", 8080, true);
        r.runner.plan(Plan::Fail(text.into()));
        r.m.tick(100);
        r.m.tick(100);
        let s = r.m.status(&t.id);
        assert_eq!((s.fault, s.needs_you, s.retry_at), (Some(fault), true, None), "{text}");
        for n in 0..50 {
            r.m.tick(101 + n * 100);
        }
        assert_eq!(r.runner.launches.lock().unwrap().len(), 1, "{fault:?}: one try, then it waits for you");
        assert_eq!(s.line(true, 200), "stopped: needs you");
        // Retry (or edit, or toggling) starts it again
        r.m.retry(&t.id);
        r.m.tick(10_000);
        r.m.tick(10_000);
        assert_eq!(r.runner.launches.lock().unwrap().len(), 2);
        assert!(matches!(r.m.status(&t.id).state, State::Connected { .. }));
    }
}

#[test]
fn a_client_that_never_listens_times_out() {
    let r = ready("hang");
    let t = r.add("Docs", 8080, true);
    r.runner.plan(Plan::Hang);
    r.m.tick(100);
    assert_eq!(r.m.status(&t.id).state, State::Connecting);
    r.m.tick(100 + CONNECT_LIMIT);
    assert_eq!(r.m.status(&t.id).state, State::Connecting, "still within the limit");
    r.m.tick(101 + CONNECT_LIMIT);
    let s = r.m.status(&t.id);
    assert_eq!((s.state, s.fault), (State::Stopped, Some(Fault::Timeout)));
    assert_eq!(r.runner.alive(), 0, "the stuck client is killed");
    assert!(s.retry_at.is_some());
}

#[test]
fn a_client_that_cannot_start_is_a_message_not_a_crash() {
    let r = ready("spawn");
    let t = r.add("Docs", 8080, true);
    r.runner.plan(Plan::SpawnError("no SSH client was found. Install it.".into()));
    r.m.tick(100);
    let s = r.m.status(&t.id);
    assert_eq!(s.error.as_deref(), Some("no SSH client was found. Install it."));
    assert_eq!(s.state, State::Stopped);
}

// ------------------------------------------------------------------ host keys and keys

#[test]
fn an_untrusted_host_is_never_connected_to_and_a_trusted_one_is() {
    let r = rig("untrusted");
    r.key("work");
    let t = r.add("Docs", 8080, true);
    r.m.tick(100);
    let s = r.m.status(&t.id);
    assert_eq!((s.fault, s.needs_you), (Some(Fault::NotTrusted), true));
    assert!(s.error.unwrap().contains("trust"));
    assert!(r.runner.launches.lock().unwrap().is_empty(), "no client was started");
}

#[test]
fn the_fingerprint_is_shown_and_only_a_confirmed_one_is_trusted() {
    let r = rig("trust");
    r.key("work");
    let k = host_key("bastion");
    r.shown.lock().unwrap().insert(("bastion.example.invalid".into(), 22), vec![k.clone()]);
    let t = r.add("Docs", 8080, true);
    let reports = r.m.check_hosts(&t);
    assert_eq!(reports.len(), 1);
    let text = reports[0].text();
    assert!(text.contains("not trusted yet") && text.contains(&k.fingerprint()), "{text}");
    // a wrong fingerprint trusts nothing
    assert!(r.m.trust("bastion.example.invalid", 22, "SHA256:wrong").unwrap_err().contains("nothing was trusted"));
    assert!(r.keys.known_hosts().for_host("bastion.example.invalid", 22).is_empty());
    // the right one does
    r.m.trust("bastion.example.invalid", 22, &k.fingerprint()).unwrap();
    assert_eq!(r.keys.known_hosts().for_host("bastion.example.invalid", 22).len(), 1);
    assert!(r.m.check_hosts(&t)[0].text().contains("is trusted"));
    r.m.tick(100);
    r.m.tick(100);
    assert!(matches!(r.m.status(&t.id).state, State::Connected { .. }));
    assert!(r.log.lock().unwrap().iter().any(|l| l.starts_with("sshtunnel.trust ") && l.contains(&k.fingerprint())));
}

#[test]
fn a_changed_host_key_is_a_hard_refusal_that_trust_does_not_override() {
    let r = ready("changed");
    let t = r.add("Docs", 8080, true);
    let other = host_key("someone-else");
    r.shown.lock().unwrap().insert(("bastion.example.invalid".into(), 22), vec![other.clone()]);
    let text = r.m.check_hosts(&t)[0].text();
    assert!(text.contains("CHANGED") && text.contains("Nothing was sent"), "{text}");
    let e = r.m.trust("bastion.example.invalid", 22, &other.fingerprint()).unwrap_err();
    assert!(e.contains("CHANGED"), "{e}");
    assert_eq!(r.keys.known_hosts().for_host("bastion.example.invalid", 22)[0].b64, host_key("bastion.example.invalid").b64, "the recorded key stays");
    let rep = r.m.test(&t.id, 100);
    assert!(!rep.ok && rep.untrusted.is_empty() && rep.lines[0].contains("CHANGED"));
    assert!(r.runner.launches.lock().unwrap().is_empty());
    // only forgetting it on purpose, then trusting again, accepts the new key
    assert_eq!(r.m.forget_host("bastion.example.invalid", 22).unwrap(), 1);
    r.m.trust("bastion.example.invalid", 22, &other.fingerprint()).unwrap();
}

#[test]
fn every_host_of_a_jump_chain_must_be_trusted() {
    let r = ready("jump");
    let t = r.m.save(Tunnel { jump: vec![Hop { host: "gate.example.invalid".into(), port: 2222, user: "bob".into() }], enabled: true, ..sample("Deep", "work", 8080) }, false).unwrap();
    r.m.tick(100);
    assert_eq!(r.m.status(&t.id).fault, Some(Fault::NotTrusted), "the first hop is unknown");
    r.trusted("gate.example.invalid", 2222);
    r.m.retry(&t.id);
    r.m.tick(101);
    r.m.tick(101);
    assert!(matches!(r.m.status(&t.id).state, State::Connected { .. }));
    let sent = r.runner.launches.lock().unwrap()[0].1.clone();
    assert!(sent.contains(&"bob@gate.example.invalid:2222".to_string()));
}

#[test]
fn a_key_that_cannot_be_opened_stops_the_tunnel_with_words() {
    let r = ready("nokey");
    let t = r.add("Docs", 8080, true);
    r.store.map.lock().unwrap().remove("ssh.work");
    r.m.tick(100);
    let s = r.m.status(&t.id);
    assert_eq!((s.fault, s.needs_you), (Some(Fault::KeyUnavailable), true));
    assert!(s.error.unwrap().contains("key could not be opened"));
    assert!(r.runner.launches.lock().unwrap().is_empty());
}

#[test]
fn an_ssh_config_in_the_private_home_makes_tunnels_refuse_to_run() {
    let r = ready("config");
    let t = r.add("Docs", 8080, true);
    std::fs::write(r.dir.join("home").join(".ssh").join("config"), "Host *\n  ProxyCommand id\n").unwrap();
    r.m.tick(100);
    assert!(r.m.status(&t.id).error.unwrap().contains(".ssh/config"));
    assert!(r.runner.launches.lock().unwrap().is_empty());
}

// ------------------------------------------------------------------ delete, keys

#[test]
fn deleting_removes_the_key_made_for_it_and_its_known_hosts_line() {
    let r = rig("delete");
    let k = host_key("b");
    r.shown.lock().unwrap().insert(("bastion.example.invalid".into(), 22), vec![k.clone()]);
    r.keys.known_hosts().trust("bastion.example.invalid", 22, &[k]).unwrap();
    let t = r.m.save(Tunnel { enabled: true, ..sample("Docs", "", 8080) }, true).unwrap();
    assert!(t.own_key && t.key == "tunnel-docs", "{t:?}");
    assert!(r.store.map.lock().unwrap().contains_key("ssh.tunnel-docs"));
    r.m.tick(100);
    assert_eq!(r.runner.alive(), 1);
    let done = r.m.remove(&t.id).unwrap();
    assert_eq!(r.runner.alive(), 0, "stopped first");
    assert!(r.m.tunnels().is_empty());
    assert!(!r.store.map.lock().unwrap().contains_key("ssh.tunnel-docs"), "the vault entry is gone");
    assert!(r.keys.known_hosts().for_host("bastion.example.invalid", 22).is_empty(), "so is the known_hosts line");
    let said = done.join(" | ");
    assert!(said.contains("the tunnel Docs") && said.contains("tunnel-docs") && said.contains("trusted key"), "{said}");
    assert!(r.m.remove(&t.id).is_err(), "already gone");
}

#[test]
fn deleting_keeps_a_shared_key_and_a_host_another_tunnel_still_uses() {
    let r = ready("shared");
    let a = r.add("A", 8001, false);
    let b = r.add("B", 8002, false);
    r.m.remove(&a.id).unwrap();
    assert!(r.store.map.lock().unwrap().contains_key("ssh.work"), "a key you chose is yours; it stays");
    assert_eq!(r.keys.known_hosts().for_host("bastion.example.invalid", 22).len(), 1, "B still needs the host");
    r.m.remove(&b.id).unwrap();
    assert!(r.store.map.lock().unwrap().contains_key("ssh.work"));
    assert!(r.keys.known_hosts().for_host("bastion.example.invalid", 22).is_empty());
}

#[test]
fn a_new_key_is_rolled_back_when_the_tunnel_cannot_be_saved() {
    let r = ready("rollback");
    r.add("Docs", 8080, false);
    let before = r.store.map.lock().unwrap().len();
    // same name: refused after the key would have been made
    let e = r.m.save(sample("docs", "", 8090), true).unwrap_err();
    assert!(e.contains("already a tunnel"), "{e}");
    assert_eq!(r.store.map.lock().unwrap().len(), before, "no orphan key in the vault");
}

#[test]
fn editing_a_tunnel_with_its_own_key_keeps_the_key_tied_to_it() {
    let r = ready("ownkey");
    let t = r.m.save(sample("Docs", "", 8080), true).unwrap();
    let mut e = r.m.get(&t.id).unwrap();
    e.target_port = 9999;
    let s = r.m.save(e, false).unwrap();
    assert!(s.own_key, "still its own key");
    // pointing it at a shared key lets go of the ownership but does not delete the shared key; the old own key goes
    let mut e = r.m.get(&t.id).unwrap();
    e.key = "work".into();
    let s = r.m.save(e, false).unwrap();
    assert!(!s.own_key);
    assert!(!r.store.map.lock().unwrap().contains_key("ssh.tunnel-docs"));
    assert!(r.store.map.lock().unwrap().contains_key("ssh.work"));
}

// ------------------------------------------------------------------ the test button

#[test]
fn test_connects_for_real_waits_for_the_port_and_stops_again() {
    let r = ready("test-ok");
    let t = r.add("Docs", 8080, false);
    r.runner.plan(Plan::ConnectAfter(3));
    let rep = r.m.test(&t.id, 100);
    assert!(rep.ok, "{rep:?}");
    assert!(rep.lines.last().unwrap().contains("http://127.0.0.1:8080/"));
    assert_eq!(r.runner.alive(), 0, "a test leaves nothing running");
    assert_eq!(r.m.status(&t.id).state, State::Stopped);
}

#[test]
fn test_says_why_in_words_for_each_kind_of_failure() {
    for (text, word) in [
        ("Permission denied (publickey).", "refused the key"),
        ("ssh: connect to host bastion.example.invalid port 22: Connection refused", "refused"),
        ("ssh: connect to host bastion.example.invalid port 22: Connection timed out", "did not answer"),
        ("bind [127.0.0.1]:8080: Address already in use", "already used"),
    ] {
        let r = ready("test-fail");
        let t = r.add("Docs", 8080, false);
        r.runner.plan(Plan::Fail(text.into()));
        let rep = r.m.test(&t.id, 100);
        assert!(!rep.ok && rep.lines.last().unwrap().contains(word), "{text}: {rep:?}");
    }
    let r = ready("test-hang");
    let t = r.add("Docs", 8080, false);
    r.runner.plan(Plan::Hang);
    let rep = r.m.test(&t.id, 100);
    assert!(!rep.ok && rep.lines.last().unwrap().contains("did not answer"));
    assert_eq!(r.runner.alive(), 0);
    // a busy local port is told before anything is started
    r.ports.taken.lock().unwrap().insert(8080);
    let rep = r.m.test(&t.id, 100);
    assert!(!rep.ok && rep.lines.last().unwrap().contains("already used"));
}

#[test]
fn test_lists_the_hosts_that_need_trust_with_their_fingerprints() {
    let r = rig("test-trust");
    r.key("work");
    let k = host_key("b");
    r.shown.lock().unwrap().insert(("bastion.example.invalid".into(), 22), vec![k.clone()]);
    let t = r.add("Docs", 8080, false);
    let rep = r.m.test(&t.id, 100);
    assert!(!rep.ok);
    assert_eq!(rep.untrusted.len(), 1);
    assert_eq!(rep.untrusted[0].2[0].fingerprint(), k.fingerprint());
    assert!(r.runner.launches.lock().unwrap().is_empty(), "no login before the host is trusted");
    // an unreachable host is an error line, not a panic
    r.shown.lock().unwrap().clear();
    let rep = r.m.test(&t.id, 100);
    assert!(!rep.ok && rep.lines[0].contains("sent no host key"));
}

// ------------------------------------------------------------------ the Browser seam

struct ByDomain;
impl SiteRouter for ByDomain {
    fn tunnel_for(&self, url: &str) -> Option<String> {
        url.contains(".corp.example.invalid").then(|| "t1".to_string())
    }
}

#[test]
fn the_browser_seam_answers_only_for_a_connected_tunnel() {
    let r = ready("route");
    let t = r.add("Docs", 8080, true);
    assert_eq!(t.id, "t1");
    assert_eq!(r.m.route(&NoSiteRouter, "https://wiki.corp.example.invalid/"), None, "direct by default");
    assert_eq!(r.m.route(&ByDomain, "https://wiki.corp.example.invalid/"), None, "not connected yet: the page is not sent to a dead port");
    r.m.tick(100);
    r.m.tick(100);
    let route = r.m.route(&ByDomain, "https://wiki.corp.example.invalid/").unwrap();
    assert_eq!((route.id.as_str(), route.name.as_str(), route.kind, route.address.as_str()), ("t1", "Docs", Kind::LocalForward, "http://127.0.0.1:8080/"));
    assert_eq!(r.m.route(&ByDomain, "https://example.invalid/"), None, "everything else direct");
    r.m.set_enabled("t1", false).unwrap();
    assert_eq!(r.m.route(&ByDomain, "https://wiki.corp.example.invalid/"), None);
}

// ------------------------------------------------------------------ the log and the supervisor

#[test]
fn the_audit_log_has_names_and_never_key_material() {
    let r = ready("audit");
    let t = r.add("Docs", 8080, true);
    r.m.tick(100);
    r.m.tick(100);
    r.runner.drop_all("Connection reset by peer");
    r.m.tick(101);
    r.m.remove(&t.id).unwrap();
    let log = r.log.lock().unwrap().join("\n");
    for w in ["sshtunnel.save Docs", "sshtunnel.start Docs", "sshtunnel.connected Docs", "sshtunnel.stopped Docs", "sshtunnel.delete Docs"] {
        assert!(log.contains(w), "{w} in {log}");
    }
    let private = r.store.map.lock().unwrap().get("ssh.work").unwrap().clone();
    for line in private.lines().filter(|l| l.len() > 20) {
        assert!(!log.contains(line), "key material in the log");
    }
}

#[test]
fn the_supervisor_starts_enabled_tunnels_by_itself_and_stops_them_when_dropped() {
    let r = ready("super");
    let t = r.add("Docs", 8080, true);
    let sup = r.m.supervise();
    let mut up = false;
    for _ in 0..60 {
        if matches!(r.m.status(&t.id).state, State::Connected { .. }) {
            up = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(up, "started without anyone calling tick");
    drop(sup);
    assert_eq!(r.runner.alive(), 0);
}
