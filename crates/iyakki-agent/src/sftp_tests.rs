//! Tests for the SFTP manager and its glue: keys, limits, firewall rules, expiry, the account-bound jail, the gate, the chat tool and a real
//! `sftp` client over loopback when this computer has one. (The packet codec, the jail and the session are tested in `iyakki-sshfs`.)

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent::Tools;
use iyakki_sshfs::testworld::{world, Log, World};
use crate::sftp::*;
use crate::sftp_fs::*;
use crate::sftp_proto::*;


// ------------------------------------------------------------------ keys, limits, firewall, manager

fn tmp(tag: &str) -> PathBuf {
    std::fs::canonicalize(crate::testdir::unique("sftpmgr", tag)).unwrap()
}

fn key_line(seed: u8, comment: &str) -> String {
    crate::sshkey::KeyPair::from_seed([seed; 32], comment).public_line()
}

#[test]
fn authorised_keys_are_checked_stored_privately_and_removed() {
    let d = tmp("keys");
    for bad in ["", "ssh-rsa AAAAB3Nza amy", "ssh-ed25519 !!!notbase64 x", "ssh-ed25519 AAAA", &format!("{}\nssh-ed25519 x", key_line(1, "a")), "-----BEGIN OPENSSH PRIVATE KEY-----", &"ssh-ed25519 ".repeat(100)] {
        assert!(parse_key_line(bad).is_err(), "{bad}");
    }
    let fp = add_key(&d, "amy", &key_line(1, "laptop")).unwrap();
    assert!(fp.starts_with("SHA256:"));
    assert_eq!(add_key(&d, "amy", &key_line(1, "again")).unwrap(), fp, "the same key twice is one key");
    add_key(&d, "amy", &key_line(2, "phone")).unwrap();
    assert_eq!(user_keys(&d, "amy").len(), 2);
    assert!(user_keys(&d, "bob").is_empty());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(d.join("sftp/keys/amy.pub")).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(add_key(&d, "../evil", &key_line(3, "x")).is_err() && add_key(&d, "A b", &key_line(3, "x")).is_err());
    assert_eq!(remove_key(&d, "amy", &fp).unwrap(), 1);
    assert!(remove_key(&d, "amy", &fp).is_err());
    assert_eq!(user_keys(&d, "amy").len(), 1);
    for i in 0..MAX_KEYS_PER_USER as u8 + 2 {
        let _ = add_key(&d, "cap", &key_line(10 + i, "k"));
    }
    assert_eq!(user_keys(&d, "cap").len(), MAX_KEYS_PER_USER);
    // the host key is made once and kept
    let h1 = host_key(&d).unwrap().fingerprint();
    assert_eq!(host_key(&d).unwrap().fingerprint(), h1);
    assert_eq!(std::fs::metadata(d.join("sftp/host_key")).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn many_wrong_passwords_block_an_address_for_a_while() {
    let mut l = Limiter::default();
    let ip: std::net::IpAddr = "192.168.1.5".parse().unwrap();
    let other: std::net::IpAddr = "192.168.1.6".parse().unwrap();
    for _ in 0..9 {
        l.failed(ip, 1000);
    }
    assert!(!l.blocked(ip, 1000));
    l.failed(ip, 1001);
    assert!(l.blocked(ip, 1002) && !l.blocked(other, 1002));
    assert!(l.blocked(ip, 1000 + 299) && !l.blocked(ip, 1001 + 300));
    // failures spread over more than the window never add up
    let mut l = Limiter::default();
    for i in 0..30 {
        l.failed(ip, 5000 + i * 400);
    }
    assert!(!l.blocked(ip, 5000 + 30 * 400));
}

#[test]
fn firewall_rules_are_for_the_local_network_with_an_expiry_and_replace_each_other() {
    let base = iyakki_fw::model::Config::default();
    let on = with_rules(base.clone(), Some((2222, 5000)));
    let ours: Vec<_> = on.rules.iter().filter(|r| r.comment == RULE_COMMENT).collect();
    assert_eq!(ours.len(), 4);
    for r in &ours {
        assert_eq!((r.ports.as_str(), r.proto, r.expires, r.kind), ("2222", iyakki_fw::model::Proto::Tcp, 5000, iyakki_fw::model::Kind::Cidr));
        assert!(LOCAL_NETWORKS.contains(&r.value.as_str()), "{}", r.value);
        iyakki_fw::validate::check_rule(r).unwrap();
    }
    assert!(on.only_opens_from(&base), "opening needs no confirmation step");
    let again = with_rules(on.clone(), Some((2323, 0)));
    assert_eq!(again.rules.iter().filter(|r| r.comment == RULE_COMMENT).count(), 4, "no pile-up");
    assert!(again.rules.iter().filter(|r| r.comment == RULE_COMMENT).all(|r| r.ports == "2323" && r.expires == 0));
    let off = with_rules(on, None);
    assert_eq!((off.rules, off.presets, off.enabled), (base.rules, base.presets, base.enabled), "closing leaves nothing behind");
}

struct FakeFw(Mutex<Vec<String>>, bool);
impl Firewall for FakeFw {
    fn open(&self, port: u16, expires: i64) -> Result<(), String> {
        if self.1 {
            return Err("refused".into());
        }
        self.0.lock().unwrap().push(format!("open {} {}", port != 0, expires != 0));
        Ok(())
    }
    fn close(&self) -> Result<(), String> {
        self.0.lock().unwrap().push("close".into());
        Ok(())
    }
}

struct FakeAuth {
    policy: iyakki_sec::policy::Policy,
    tries: Mutex<u32>,
}
impl iyakki_core_api::service::Authenticator for FakeAuth {
    fn login(&self, name: &str, password: &str) -> iyakki_core_api::error::ApiResult<iyakki_core_api::service::Principal> {
        *self.tries.lock().unwrap() += 1;
        if name == "amy" && password == "right-password-9" {
            Ok(iyakki_core_api::service::Principal { user: name.into(), role: "user".into(), admin: false, policy: self.policy.clone() })
        } else {
            Err(iyakki_core_api::error::ApiError::Denied("invalid username or password".into()))
        }
    }
    fn lookup(&self, user: &str) -> Option<iyakki_core_api::service::Principal> {
        (user == "amy").then(|| iyakki_core_api::service::Principal { user: user.into(), role: "user".into(), admin: false, policy: self.policy.clone() })
    }
}

struct Rig {
    sftp: Arc<Sftp>,
    fw: Arc<FakeFw>,
    audit: Log,
    w: World,
    data: PathBuf,
    auth: Arc<FakeAuth>,
}

fn rig(tag: &str) -> Rig {
    let (w, _j) = world(tag);
    let data = tmp(&format!("{tag}-data"));
    let mut policy = iyakki_sec::policy::Role::User.template();
    policy.fs_read = vec![w.home.display().to_string(), w.shared.display().to_string()];
    policy.fs_write = vec![w.home.display().to_string()];
    let auth = Arc::new(FakeAuth { policy, tries: Mutex::new(0) });
    let audit: Log = Arc::default();
    let a2 = audit.clone();
    let fw = Arc::new(FakeFw(Mutex::new(Vec::new()), false));
    let sftp = Arc::new(Sftp::new(&data, auth.clone(), Arc::new(move |e, d| a2.lock().unwrap().push(format!("{e} {d}"))), fw.clone()).with_home(w.home.clone()));
    Rig { sftp, fw, audit, w, data, auth }
}

#[test]
fn it_is_off_until_enabled_and_enabling_needs_a_way_to_log_in() {
    let r = rig("manager");
    let st = r.sftp.status();
    assert!(!st.running && st.config.users.is_empty() && !st.config.password_login);
    assert!(r.sftp.enable("amy").unwrap_err().contains("no account is allowed"));
    r.sftp.configure(|c| {
        c.users.push("amy".into());
        c.port = 0;
        Ok(())
    })
    .unwrap();
    assert!(r.sftp.enable("amy").unwrap_err().contains("nobody could log in"));
    assert!(r.fw.0.lock().unwrap().is_empty(), "nothing was opened");
    assert!(r.sftp.configure(|c| { c.port = 80; Ok(()) }).is_err() && r.sftp.configure(|c| { c.port = 8100; Ok(()) }).is_err());
    assert!(r.sftp.configure(|c| { c.users.push("A b".into()); Ok(()) }).is_err());
    add_key(&r.data, "amy", &key_line(1, "k")).unwrap();
    let st = r.sftp.enable("amy").unwrap();
    assert!(st.running && st.port != 0 && st.by == "amy");
    assert_eq!(r.fw.0.lock().unwrap().as_slice(), ["open true true".to_string()], "one firewall opening, with an expiry");
    assert!(r.sftp.enable("amy").is_err(), "twice is refused");
    // the port really answers and speaks SSH
    let mut c = std::net::TcpStream::connect(("127.0.0.1", st.port)).unwrap();
    let mut line = [0u8; 8];
    std::io::Read::read_exact(&mut c, &mut line).unwrap();
    assert_eq!(&line, b"SSH-2.0-");
    drop(c);
    assert!(r.sftp.disable("amy"));
    assert_eq!(r.fw.0.lock().unwrap().last().unwrap(), "close");
    // disable joined the listener, so the port is closed; a freed port can be taken at once by another test of this process (they bind
    // port 0), so a refusal at any try within two seconds is the proof
    let t0 = std::time::Instant::now();
    let mut refused = false;
    while t0.elapsed() < std::time::Duration::from_secs(2) {
        if std::net::TcpStream::connect(("127.0.0.1", st.port)).is_err() {
            refused = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(refused, "closed again");
    assert!(!r.sftp.disable("amy"));
    let a = r.audit.lock().unwrap().join("\n");
    assert!(a.contains("sftp.enable by amy") && a.contains("sftp.disable by amy"), "{a}");
}

#[test]
fn a_firewall_that_refuses_leaves_nothing_listening() {
    let mut r = rig("fwfail");
    let fw = Arc::new(FakeFw(Mutex::new(Vec::new()), true));
    let a2 = r.audit.clone();
    r.sftp = Arc::new(Sftp::new(&r.data, r.auth.clone(), Arc::new(move |e, d| a2.lock().unwrap().push(format!("{e} {d}"))), fw).with_home(r.w.home.clone()));
    r.sftp.configure(|c| {
        c.users.push("amy".into());
        c.port = 0;
        Ok(())
    })
    .unwrap();
    add_key(&r.data, "amy", &key_line(1, "k")).unwrap();
    assert!(r.sftp.enable("amy").unwrap_err().contains("firewall"));
    assert!(!r.sftp.status().running);
}

#[test]
fn the_gate_checks_the_allow_list_keys_and_passwords_and_never_logs_secrets() {
    let r = rig("gate");
    let limiter: Arc<Mutex<Limiter>> = Arc::default();
    let mut cfg = Config { users: vec!["amy".into()], ..Config::default() };
    let gate = |cfg: &Config| RealGate { dir: r.data.clone(), cfg: cfg.clone(), auth: r.auth.clone(), audit: Arc::new(|_, _| {}), limiter: limiter.clone(), home: Some(r.w.home.clone()) };
    use crate::ssh_server::Gate;
    let ip: std::net::IpAddr = "192.168.1.9".parse().unwrap();
    let k = crate::sshkey::KeyPair::from_seed([1; 32], "k");
    assert!(!gate(&cfg).key_ok("amy", &k.public_blob(), ip), "no key yet");
    add_key(&r.data, "amy", &k.public_line()).unwrap();
    assert!(gate(&cfg).key_ok("amy", &k.public_blob(), ip));
    assert!(!gate(&cfg).key_ok("amy", &crate::sshkey::KeyPair::from_seed([2; 32], "o").public_blob(), ip));
    assert!(!gate(&cfg).key_ok("bob", &k.public_blob(), ip), "not on the list");
    // passwords: off by default
    assert!(!gate(&cfg).password_ok("amy", "right-password-9", ip));
    assert_eq!(*r.auth.tries.lock().unwrap(), 0, "with passwords off the account code is not even asked");
    cfg.password_login = true;
    assert!(gate(&cfg).password_ok("amy", "right-password-9", ip));
    assert!(!gate(&cfg).password_ok("amy", "wrong", ip));
    assert!(!gate(&cfg).password_ok("bob", "right-password-9", ip), "an account off the list never gets in, even with a good password");
    for _ in 0..12 {
        gate(&cfg).password_ok("amy", "wrong", ip);
    }
    let before = *r.auth.tries.lock().unwrap();
    assert!(!gate(&cfg).password_ok("amy", "right-password-9", ip), "the address is blocked, even for the right password");
    assert_eq!(*r.auth.tries.lock().unwrap(), before, "and the account code is not asked while blocked");
    assert!(gate(&cfg).password_ok("amy", "right-password-9", "192.168.1.10".parse().unwrap()), "another address is not affected");
    // sessions: the account's rules decide
    let s = gate(&cfg).session("amy", ip);
    assert!(s.is_some());
    assert!(gate(&cfg).session("bob", ip).is_none());
}

#[test]
fn the_jail_of_an_account_follows_its_file_rules_and_hides_the_data_folder() {
    let r = rig("jailfor");
    let policy = r.auth.policy.clone();
    let j = jail_for(&policy, "amy", &r.data, Some(r.w.home.clone()));
    assert!(j.list("/").unwrap().iter().any(|e| e.0 == "a.txt"));
    assert!(j.open("/+shared/pub.txt", OPEN_READ, &Attrs::default()).is_ok());
    assert!(j.open("/+shared/pub.txt", OPEN_WRITE, &Attrs::default()).is_err(), "the policy gives read only there");
    assert!(j.open("/link_out/secret.txt", OPEN_READ, &Attrs::default()).is_err());
    // an account with no file rules sees an empty, unwritable home
    let mut none = policy.clone();
    none.fs_read.clear();
    none.fs_write.clear();
    let j = jail_for(&none, "amy", &r.data, Some(r.w.home.clone()));
    assert!(j.list("/").is_err() && j.open("/a.txt", OPEN_READ, &Attrs::default()).is_err());
    // a read-only account cannot write in its home
    let mut ro = policy;
    ro.fs_write.clear();
    let j = jail_for(&ro, "amy", &r.data, Some(r.w.home.clone()));
    assert!(j.open("/a.txt", OPEN_READ, &Attrs::default()).is_ok());
    assert_eq!(j.open("/z.txt", OPEN_WRITE | OPEN_CREAT, &Attrs::default()).err(), Some(FsErr::Denied));
    assert_eq!(j.remove("/a.txt"), Err(FsErr::Denied));
}

// ------------------------------------------------------------------ the chat tool

#[test]
fn the_chat_tool_reads_freely_and_asks_for_every_change() {
    use crate::sftptools::*;
    let r = rig("tool");
    let t = SftpTools::new(Arc::new(crate::assign::tests_support::NoTools), r.sftp.clone(), "amy");
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).map(|d| d.risk);
    assert_eq!(risk("sftp_status"), Some(crate::agent::Risk::Safe));
    assert_eq!(risk("sftp_set"), Some(crate::agent::Risk::Ask));
    assert!(!t.pre_approved("sftp_set") && !t.approved_always("sftp_set") && t.grant_blocker("sftp_set", &json!({})).is_some());
    let st = t.call("sftp_status", &json!({})).unwrap();
    assert!(st.contains("SFTP server: off") && st.contains("Host key: SHA256:") && st.contains("no account is allowed"), "{st}");
    // enabling without a login way is refused with the reason, at the card
    let e = json!({"action": "enable"});
    assert!(t.refusal("sftp_set", &e).unwrap().contains("no account is allowed"));
    t.call("sftp_set", &json!({"action": "allow_user", "user": "amy"})).unwrap();
    assert!(t.refusal("sftp_set", &e).unwrap().contains("nobody could log in"));
    // keys: public lines pass, private keys and passwords never
    let line = key_line(4, "me@laptop");
    assert!(t.preview("sftp_set", &json!({"action": "add_key", "key": line})).unwrap().contains("SHA256:"));
    assert!(t.refusal("sftp_set", &json!({"action": "add_key", "key": "-----BEGIN OPENSSH PRIVATE KEY-----\nabc"})).is_some());
    assert!(t.refusal("sftp_set", &json!({"action": "password_login", "value": "on", "password": "x"})).unwrap().contains("never send"));
    assert!(t.refusal("sftp_set", &json!({"action": "add_key", "key": "ssh-rsa AAAA x"})).is_some());
    assert!(t.refusal("sftp_set", &json!({"action": "allow_user", "user": "../x"})).is_some());
    assert!(t.refusal("sftp_set", &json!({"action": "set_port", "port": 80})).is_some());
    assert!(t.refusal("sftp_set", &json!({"action": "set_expiry", "expires": "forever"})).is_some());
    assert!(t.refusal("sftp_set", &json!({"action": "nope"})).is_some());
    t.call("sftp_set", &json!({"action": "add_key", "key": line})).unwrap();
    t.call("sftp_set", &json!({"action": "set_expiry", "expires": "1 hour"})).unwrap();
    t.call("sftp_set", &json!({"action": "set_port", "port": 2299})).unwrap();
    assert_eq!(r.sftp.config().port, 2299);
    // the card for enabling says who, what, from where and until when
    let card = t.preview("sftp_set", &e).unwrap();
    for needle in ["LOCAL NETWORK ONLY", "amy", "port 2299", "1 hour", "public keys only", "own file rules"] {
        assert!(card.contains(needle) || card.to_lowercase().contains(&needle.to_lowercase()), "{needle} in {card}");
    }
    let pw = t.preview("sftp_set", &json!({"action": "password_login", "value": "on"})).unwrap();
    assert!(pw.contains("ACCOUNT PASSWORD") && pw.contains("never stored"));
    // really enabling (port 0 for the test)
    r.sftp.configure(|c| {
        c.port = 0;
        Ok(())
    })
    .unwrap();
    let out = t.call("sftp_set", &e).unwrap();
    assert!(out.contains("SFTP server is ON") && out.contains("Local network only"), "{out}");
    assert!(t.call("sftp_status", &json!({})).unwrap().contains("SFTP server: ON"));
    assert!(t.refusal("sftp_set", &e).unwrap().contains("already running"));
    assert!(t.call("sftp_set", &json!({"action": "disable"})).unwrap().contains("turned off"));
    assert!(!r.sftp.status().running);
    assert!(t.call("sftp_set", &json!({"action": "remove_key", "fingerprint": "SHA256:nothing"})).is_err());
    // the prompt rule never asks for secrets
    assert!(PROMPT_RULE.contains("Never ask for or type a password or private key"));
}

#[test]
fn it_stops_by_itself_when_the_time_is_up_and_closes_the_firewall() {
    let r = rig("expiry");
    r.sftp
        .configure(|c| {
            c.users.push("amy".into());
            c.port = 0;
            c.expiry_secs = 1;
            Ok(())
        })
        .unwrap();
    add_key(&r.data, "amy", &key_line(1, "k")).unwrap();
    let st = r.sftp.enable("amy").unwrap();
    assert!(st.running && st.until > 0);
    assert!(status_lines(&r.data)[0].contains("ON at port"));
    std::thread::sleep(std::time::Duration::from_millis(2600));
    let st = r.sftp.status();
    assert!(!st.running, "the time is up");
    assert!(st.port == 0 || crate::testdir::port_is_closed(st.port));
    assert_eq!(r.fw.0.lock().unwrap().last().unwrap(), "close", "the firewall opening is removed");
    assert!(r.audit.lock().unwrap().iter().any(|l| l.starts_with("sftp.disable by expiry")));
    assert!(status_lines(&r.data)[0].contains("off"));
    // never: no end time
    r.sftp.configure(|c| {
        c.expiry_secs = 0;
        Ok(())
    })
    .unwrap();
    r.sftp.enable("amy").unwrap();
    assert_eq!(r.sftp.status().until, 0);
    r.sftp.disable("test");
    let lines = status_lines(&r.data).join("\n");
    assert!(lines.contains("keys only") && lines.contains("amy") && !lines.to_lowercase().contains("password:"), "{lines}");
}

// ------------------------------------------------------------------ a real client over loopback

fn have(p: &str) -> bool {
    std::process::Command::new(p).arg("-V").output().is_ok() || std::process::Command::new(p).arg("-?").output().is_ok()
}

fn run_sftp(r: &Rig, port: u16, extra: &[&str], envs: &[(&str, &str)], script: &str) -> (String, String, bool) {
    use std::io::Write;
    let mut cmd = std::process::Command::new("sftp");
    cmd.args(["-P", &port.to_string(), "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null", "-o", "LogLevel=ERROR", "-o", "ConnectTimeout=10"]).args(extra).arg("amy@127.0.0.1");
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.current_dir(&r.w.root).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.success())
}

/// A real OpenSSH `sftp` against our server: key login, an upload and a download, escapes, a refused unknown key, a password login and a
/// wrong password. Skipped (with a note) when this computer has no `sftp` or `ssh-keygen`.
#[test]
fn a_real_sftp_client_works_over_loopback_and_cannot_escape() {
    if !(have("ssh") && have("ssh-keygen") && std::process::Command::new("sftp").arg("-?").output().is_ok()) {
        eprintln!("no sftp client on this computer: the loopback test is skipped");
        return;
    }
    let r = rig("real");
    let keydir = tmp("real-keys");
    let gen = |name: &str| {
        let f = keydir.join(name);
        assert!(std::process::Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-C", name, "-f"]).arg(&f).status().unwrap().success());
        f
    };
    let (good, stranger) = (gen("good"), gen("stranger"));
    let publine = std::fs::read_to_string(keydir.join("good.pub")).unwrap();
    r.sftp
        .configure(|c| {
            c.users.push("amy".into());
            c.port = 0;
            c.password_login = true;
            Ok(())
        })
        .unwrap();
    add_key(&r.data, "amy", &publine).unwrap();
    let st = r.sftp.enable("amy").unwrap();
    let port = st.port;
    let id = |f: &PathBuf| ["-i".to_string(), f.display().to_string(), "-o".into(), "IdentitiesOnly=yes".into(), "-o".into(), "BatchMode=yes".into(), "-o".into(), "PreferredAuthentications=publickey".into()];
    let good_args = id(&good);
    let good_args: Vec<&str> = good_args.iter().map(String::as_str).collect();

    std::fs::write(r.w.root.join("local-up.txt"), "uploaded by a real client\n").unwrap();
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 253) as u8).collect();
    std::fs::write(r.w.root.join("local-big.bin"), &big).unwrap();
    let script = "pwd\nls\nput local-up.txt up.txt\nput local-big.bin big.bin\nget up.txt got.txt\nget big.bin got-big.bin\nmkdir newdir\nrename up.txt newdir/moved.txt\nls newdir\n\
                  cd /\nget /../../../outside/secret.txt leak1.txt\nget ../../outside/secret.txt leak2.txt\nget /link_out/secret.txt leak3.txt\nget /file_link leak4.txt\nls /link_out\nls /../outside\nput local-up.txt /link_out/planted.txt\nget /etc/passwd leak5.txt\nls /+shared\nget /+shared/pub.txt got-pub.txt\nput local-up.txt /+shared/nope.txt\nrm /a.txt\nrmdir newdir\nrm newdir/moved.txt\nrmdir newdir\nbye\n";
    let (out, err, _ok) = run_sftp(&r, port, &good_args, &[], script);
    eprintln!("--- sftp output ---\n{out}\n--- stderr ---\n{err}");
    assert!(out.contains("Remote working directory: /"), "{out}");
    assert!(out.contains("a.txt") && out.contains("+shared"), "{out}");
    assert_eq!(std::fs::read_to_string(r.w.root.join("got.txt")).unwrap(), "uploaded by a real client\n");
    assert_eq!(std::fs::read(r.w.root.join("got-big.bin")).unwrap(), big, "a 3 MB file crosses the encrypted channel intact");
    assert_eq!(std::fs::read(r.w.home.join("big.bin")).unwrap(), big);
    assert_eq!(std::fs::read_to_string(r.w.root.join("got-pub.txt")).unwrap(), "public");
    for leak in ["leak1.txt", "leak2.txt", "leak3.txt", "leak4.txt", "leak5.txt"] {
        assert!(!r.w.root.join(leak).exists(), "{leak} must not exist: nothing outside the scope can be fetched");
    }
    assert!(!r.w.outside.join("planted.txt").exists(), "nothing is written outside");
    assert!(!r.w.shared.join("nope.txt").exists(), "the read-only root stays read-only");
    assert!(!r.w.home.join("a.txt").exists(), "rm inside the scope works");
    assert!(!r.w.home.join("newdir").exists());
    assert_eq!(std::fs::read_to_string(r.w.outside.join("secret.txt")).unwrap(), "TOP SECRET");
    // refused: a key the server does not know
    let sid = id(&stranger);
    let sid: Vec<&str> = sid.iter().map(String::as_str).collect();
    let (_o, e, ok) = run_sftp(&r, port, &sid, &[], "ls\n");
    assert!(!ok && e.contains("Permission denied"), "{e}");
    // password login through the account code, via an askpass helper (the password is never on a command line)
    let ask = keydir.join("askpass.sh");
    std::fs::write(&ask, "#!/bin/sh\necho right-password-9\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ask, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let pw_args = ["-o", "PreferredAuthentications=password", "-o", "PubkeyAuthentication=no", "-o", "NumberOfPasswordPrompts=1"];
    let envs = [("SSH_ASKPASS", ask.to_str().unwrap()), ("SSH_ASKPASS_REQUIRE", "force"), ("DISPLAY", "none")];
    let (o, e, _) = run_sftp(&r, port, &pw_args, &envs, "ls\nbye\n");
    assert!(o.contains("+shared"), "password login should list the home: {o} {e}");
    std::fs::write(&ask, "#!/bin/sh\necho wrong-password\n").unwrap();
    let (o, e, ok) = run_sftp(&r, port, &pw_args, &envs, "ls\nbye\n");
    assert!(!ok && !o.contains("+shared") && e.contains("Permission denied"), "{o} {e}");
    // the audit trail has the logins and file changes, and no password
    let a = r.audit.lock().unwrap().join("\n");
    assert!(a.contains("sftp.login amy from 127.0.0.1") && a.contains("sftp.login_failed amy") && a.contains("sftp.open create /up.txt") && a.contains("sftp.rename /up.txt -> /newdir/moved.txt"), "{a}");
    assert!(!a.contains("right-password-9") && !a.contains("wrong-password") && !a.contains("TOP SECRET"), "{a}");
    r.sftp.disable("test");
}

/// Not a test: starts the server on a high port for a minute so a person can try `ssh -vvv` or `sftp` by hand
/// (`IYAKKI_SFTP_DEBUG_PORT=2299 cargo test -p iyakki-agent --lib sftp_by_hand -- --ignored --nocapture`).
#[test]
#[ignore]
fn sftp_by_hand() {
    let port: u16 = std::env::var("IYAKKI_SFTP_DEBUG_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(2299);
    let r = rig("hand");
    let line = std::env::var("IYAKKI_SFTP_DEBUG_KEY").ok().and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_else(|| key_line(1, "k"));
    r.sftp.configure(|c| {
        c.users.push("amy".into());
        c.port = port;
        c.password_login = true;
        Ok(())
    })
    .unwrap();
    add_key(&r.data, "amy", &line).unwrap();
    let st = r.sftp.enable("amy").unwrap();
    eprintln!("listening on {} home {}", st.port, r.w.home.display());
    std::thread::sleep(std::time::Duration::from_secs(150));
    r.sftp.disable("hand");
    eprintln!("{}", r.audit.lock().unwrap().join("\n"));
}
