//! The `adb` side of developer mode without a network: scopes (granting, dependencies, old key files, who may grant), the
//! scope matrix over every route, the debug reads against a made-up machine, files (sandbox, links, secrets, chunks, resume,
//! limits), input, sessions and the shell grant. The same routes run over TLS in `crates/iyakki-remote/tests/devmode_adb.rs`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use iyakki_sec::policy::Role;
use serde_json::{json, Value};

use super::api::{self, Ctx, Out, ROUTES};
use super::debug::Paths;
use super::input::InputEv;
use super::grants::ShellStatus;
use super::scopes::{self, Scope};
use super::*;

const NOW: i64 = 1_800_000_000;

pub(super) struct Rig {
    pub dir: PathBuf,
    pub dm: DevMode,
    pub paths: Paths,
    pub clock: Arc<AtomicI64>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn wr(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// A device: data folder, switched-on developer mode, and a made-up /proc and /sys.
pub(super) fn rig(tag: &str) -> Rig {
    let m = crate::sysinfo::tests::machine(&format!("adb-{tag}"));
    let dir = std::env::temp_dir().join(format!("iyakki-adb-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let clock = Arc::new(AtomicI64::new(NOW));
    let c = clock.clone();
    let dm = DevMode::open(&dir).with_clock(Arc::new(move || c.load(Ordering::SeqCst)));
    dm.enable(&Role::Admin.template(), "root", 0).unwrap();
    let (proc, sys) = (m.proc.clone(), m.sys.clone());
    wr(&sys.join("class/dmi/id/sys_vendor"), "Apple Inc.\n");
    wr(&sys.join("class/dmi/id/product_name"), "MacBookPro14,1\n");
    wr(&sys.join("class/dmi/id/bios_version"), "MBP141.88Z\n");
    wr(&proc.join("modules"), "snd_hda_intel 1 0 - Live 0x0\nbrcmfmac 2 0 - Live 0x0\ni915 3 1 - Live 0x0\n");
    wr(&proc.join("sys/kernel/tainted"), "4097\n");
    wr(&proc.join("sys/kernel/osrelease"), "6.12.1-iyakki\n");
    wr(&proc.join("version"), "Linux version 6.12.1-iyakki (build@host) #1 SMP\n");
    wr(&proc.join("cmdline"), "quiet root=/dev/sda2 ro\n");
    wr(&proc.join("asound/cards"), " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n                      HDA Intel PCH at 0xdf240000\n");
    wr(&proc.join("mounts"), "/dev/sda2 / ext4 rw 0 0\nproc /proc proc rw 0 0\n/dev/sda1 /boot vfat ro 0 0\n");
    wr(&sys.join("block/sda/size"), "125000000\n");
    wr(&sys.join("block/sda/device/model"), "USB SanDisk 3.2\n");
    wr(&sys.join("block/sda/removable"), "1\n");
    wr(&sys.join("block/loop0/size"), "8\n");
    wr(&sys.join("class/drm/card0/device/vendor"), "0x8086\n");
    wr(&sys.join("class/drm/card0/device/device"), "0x5916\n");
    wr(&sys.join("class/drm/card0-eDP-1/status"), "connected\n");
    wr(&sys.join("class/drm/card0-HDMI-A-1/status"), "disconnected\n");
    std::fs::create_dir_all(sys.join("firmware/efi")).unwrap();
    wr(&dir.join("kmsg"), "6,1,1000,-;Linux version 6.12.1-iyakki\n6,2,2000,-;[iyakki INFO ] shell started\n  SUBSYSTEM=x\n4,3,3000,-;brcmfmac 0000:03:00.0: Direct firmware load for brcm/brcmfmac43602-pcie.bin failed with error -2\n6,4,4000,-;[iyakki WARN ] token=ghp_abcdefghijklmnopqrstuvwxyz0123456789ABCD leaked\n6,5,5000,-;usb 1-1: new high-speed USB device\n");
    wr(&dir.join("boot.log"), "splash: fb0 1920x1200\nsplash: stopped\n");
    wr(&dir.join("release"), "VERSION=0.1.0-alpha.1\nVARIANT=macweb\nBUILD_DATE=2026-10-21\nGIT_DESCRIBE=v0.1.0-alpha.1-3-gabc\n");
    let paths = Paths { sys: crate::sysinfo::SysRoots { proc, sys, passwd: m.passwd.clone() }, kmsg: dir.join("kmsg"), boot_log: dir.join("boot.log"), release: dir.join("release"), addrs: Arc::new(|| vec![("eth0".into(), "192.0.2.10".into())]) };
    Rig { dir, dm, paths, clock }
}

pub(super) fn key(name: &str, account: &str, scopes: &[Scope]) -> DevKey {
    DevKey { name: name.into(), key: format!("{:0>64}", name.len()), account: account.into(), added: NOW, added_by: "root".into(), last_used: 0, scopes: scopes.iter().map(|s| s.id().to_string()).collect() }
}

pub(super) fn call(r: &Rig, k: &DevKey, policy: &iyakki_sec::policy::Policy, method: &str, path: &str, q: &[(&str, &str)], body: &[u8]) -> Option<Out> {
    let audit = |_: &str, _: &str| {};
    let tail = |n: usize| vec!["1 root login ok".to_string(); n.min(3)];
    let cx = Ctx { dm: &r.dm, key: k, policy, role: policy_role(policy), hostname: "iyakki-test", paths: &r.paths, audit_tail: &tail, audit: &audit, firewall_file: r.dir.join("firewall.json"), home: Some({ let h = r.dir.join("acct-home"); std::fs::create_dir_all(&h).unwrap(); h }), svc: None, ext: None, own_port: 7979, power: &|_| {}, boot: None, release: "", volatile: false, data_free: None, kube: None };
    let qs = |n: &str| q.iter().find(|(k, _)| *k == n).map(|(_, v)| v.to_string()).unwrap_or_default();
    api::handle(&cx, method, path, &qs, body)
}

fn policy_role(p: &iyakki_sec::policy::Policy) -> &'static str {
    if p.manage_settings { "admin" } else { "user" }
}

fn jv(o: &Out) -> Value {
    serde_json::from_slice(&o.body).unwrap_or(Value::Null)
}

fn admin() -> iyakki_sec::policy::Policy {
    Role::Admin.template()
}

fn userp() -> iyakki_sec::policy::Policy {
    Role::User.template()
}

#[test]
fn scopes_are_granted_per_computer_by_an_administrator_with_their_dependencies_and_old_keys_hold_only_install() {
    let r = rig("scopes");
    let adm = admin();
    // a key file written before scopes existed
    wr(&r.dir.join("devmode/keys.json"), r#"[{"name":"old","key":"aa","account":"root","added":1,"added_by":"root"}]"#);
    assert_eq!(r.dm.keys()[0].scopes, vec!["install"], "an old key holds only the default scope");
    assert_eq!(r.dm.scopes_of("old").unwrap(), vec![Scope::Install]);
    assert!(r.dm.set_scope(&userp(), "bob", "old", Scope::Debug, true).is_err(), "only administrators grant");
    assert!(r.dm.set_scope(&adm, "root", "nobody", Scope::Debug, true).is_err());
    assert!(r.dm.set_scope(&adm, "root", "old", Scope::FilesSystem, true).unwrap_err().contains("files first"));
    r.dm.set_scope(&adm, "root", "old", Scope::Files, true).unwrap();
    r.dm.set_scope(&adm, "root", "old", Scope::FilesSystem, true).unwrap();
    let k = r.dm.set_scope(&adm, "root", "old", Scope::Debug, true).unwrap();
    assert_eq!(k.scope_text(), "install, debug, files, files:system");
    // taking files away takes what depends on it away too
    let k = r.dm.set_scope(&adm, "root", "old", Scope::Files, false).unwrap();
    assert_eq!(k.scope_text(), "install, debug");
    assert!(r.dm.activity(20).iter().any(|a| a.action == "devmode.scope.grant" && a.detail.contains("old files")));
    assert_eq!(Scope::parse("Files:System"), Some(Scope::FilesSystem));
    assert_eq!(Scope::parse("root"), None);
    // shell includes power
    assert!(key("x", "root", &[Scope::Shell]).has(Scope::Power) && !key("x", "root", &[Scope::Power]).has(Scope::Shell));
    // the pairing card: scopes chosen while the code is open end up on the new key
    let p = r.dm.start_pairing(&adm, "root").unwrap();
    assert_eq!(p.scopes, vec!["install"]);
    assert!(r.dm.set_pairing_scope(&userp(), "bob", Scope::Debug, true).is_err());
    assert_eq!(r.dm.set_pairing_scope(&adm, "root", Scope::Debug, true).unwrap(), vec!["install", "debug"]);
    let fp = "AB:CD";
    let sk = crate::pkg::new_signing_key().unwrap();
    let pubhex = crate::pkg::public_key_hex(&sk);
    let proof = auth::pair_proof(&auth::pair_key(&p.code, fp).unwrap(), fp, &pubhex, "laptop");
    let (nk, _) = r.dm.complete_pairing(fp, "laptop", &pubhex, &proof).unwrap();
    assert_eq!(nk.scopes, vec!["install", "debug"]);
    assert!(r.dm.activity(5).iter().any(|a| a.action == "devmode.pair" && a.detail.contains("scopes: install, debug")), "the pairing line says what was granted");
}

/// Every route x every scope: refused (403, naming the scope) without it, not refused with it.
#[test]
fn the_scope_matrix_every_route_is_refused_without_its_scope_and_allowed_with_it() {
    let r = rig("matrix");
    let a = admin();
    let mut checked = 0;
    for (m, p, need) in ROUTES {
        let Some(need) = need else { continue };
        for s in Scope::ALL {
            // the other scopes alone (and every scope but this route's own): denied
            let mut held: Vec<Scope> = vec![s];
            if s == Scope::FilesSystem {
                held.push(Scope::Files);
            }
            if s == Scope::InputPassword {
                held.push(Scope::Input);
            }
            let k = key("laptop", "root", &held);
            let o = call(&r, &k, &a, m, p, &[], b"{}");
            let allowed = held.iter().any(|h| h == need || (*need == Scope::Power && *h == Scope::Shell));
            match (&o, allowed) {
                (Some(o), false) => {
                    assert_eq!(o.status, 403, "{m} {p} with only {}: {}", s.id(), String::from_utf8_lossy(&o.body));
                    assert!(String::from_utf8_lossy(&o.body).contains(need.id()), "{m} {p}: the refusal names the scope: {}", String::from_utf8_lossy(&o.body));
                }
                (None, false) => panic!("{m} {p} must refuse a computer without {}", need.id()),
                (Some(o), true) => assert!(!(o.status == 403 && String::from_utf8_lossy(&o.body).contains("scope")), "{m} {p} with {}: {}", s.id(), String::from_utf8_lossy(&o.body)),
                (None, true) => {} // a route served by the listener itself (install, uninstall...)
            }
            checked += 1;
        }
        // no scope at all
        let none = key("laptop", "root", &[]);
        match call(&r, &none, &a, m, p, &[], b"{}") {
            Some(o) => assert_eq!(o.status, 403, "{m} {p} with no scope"),
            None => panic!("{m} {p} must refuse a computer with no scope"),
        }
    }
    assert!(checked >= ROUTES.len() * 5);
    // routes that do not exist are not mine
    assert!(call(&r, &key("l", "root", &Scope::ALL), &a, "GET", "/v1/dev/nothing", &[], b"").is_none());
    assert!(api::scope_of("GET", "/v1/dev/whoami") == Some(None) && api::scope_of("DELETE", "/v1/dev/info").is_none());
    // every refusal was logged as refused
    let _ = call(&r, &key("laptop", "root", &[]), &a, "GET", "/v1/dev/info", &[], b"");
    assert!(r.dm.activity(5).iter().any(|x| !x.ok && x.action == "dev.info" && x.detail.contains("debug")));
}

#[test]
fn a_user_account_cannot_do_what_only_administrators_may_even_with_the_scope() {
    let r = rig("roles");
    let u = userp();
    let k = key("laptop", "bob", &Scope::ALL);
    for p in ["/v1/dev/dmesg", "/v1/dev/syslog"] {
        let o = call(&r, &k, &u, "GET", p, &[], b"").unwrap();
        assert_eq!(o.status, 403, "{p}");
    }
    // the processes are only its own
    let o = call(&r, &k, &u, "GET", "/v1/dev/ps", &[], b"").unwrap();
    assert_eq!(o.status, 200);
    assert_eq!(jv(&o)["all"], json!(false));
    assert!(jv(&o)["processes"].as_array().unwrap().is_empty(), "bob owns no process on the made-up machine");
    let o = call(&r, &key("laptop", "root", &Scope::ALL), &admin(), "GET", "/v1/dev/ps", &[], b"").unwrap();
    assert_eq!(jv(&o)["processes"].as_array().unwrap().len(), 2);
    // scopes that need an administrator account
    for s in [Scope::Shell, Scope::Update, Scope::Forward, Scope::Power, Scope::FilesSystem] {
        assert!(scopes::check(&key("l", "bob", &Scope::ALL), &u, s).unwrap_err().contains("administrator"), "{}", s.id());
        assert!(scopes::check(&key("l", "root", &Scope::ALL), &admin(), s).is_ok());
    }
    assert!(scopes::check(&key("l", "bob", &[Scope::Debug]), &u, Scope::Debug).is_ok());
    assert!(scopes::check(&key("l", "bob", &[Scope::Debug]), &u, Scope::Files).is_err());
}

#[test]
fn info_ps_dmesg_logs_and_status_read_the_made_up_machine_and_hide_credentials() {
    let r = rig("debug");
    let k = key("laptop", "root", &[Scope::Debug]);
    let a = admin();
    let v = jv(&call(&r, &k, &a, "GET", "/v1/dev/info", &[], b"").unwrap());
    assert_eq!(v["device"]["product"], "MacBookPro14,1");
    assert_eq!(v["device"]["vendor"], "Apple Inc.");
    assert_eq!(v["image"]["variant"], "macweb");
    assert_eq!(v["image"]["version"], "0.1.0-alpha.1");
    assert_eq!(v["kernel"]["release"], "6.12.1-iyakki");
    assert_eq!(v["kernel"]["modules"], 3);
    assert_eq!(v["kernel"]["tainted"], 4097);
    assert_eq!(v["kernel"]["taint"], json!(["proprietary module loaded", "out-of-tree module loaded"]));
    assert_eq!(v["uptime_secs"], 93784);
    assert_eq!(v["memory"]["total"], 8_000_000u64 * 1024);
    assert_eq!(v["firmware"]["uefi"], true);
    assert_eq!(v["firmware"]["missing_files"], json!(["brcm/brcmfmac43602-pcie.bin"]), "the missing WiFi firmware is named");
    assert_eq!(v["disks"].as_array().unwrap().len(), 1, "loop devices are not disks");
    assert_eq!(v["disks"][0]["bytes"], 125_000_000u64 * 512);
    assert_eq!(v["disks"][0]["removable"], true);
    assert_eq!(v["mounts"].as_array().unwrap().len(), 2, "only real file systems");
    assert_eq!(v["mounts"][1]["ro"], true);
    assert_eq!(v["network"][0]["address"], "192.0.2.10");
    assert_eq!(v["gpu"][0]["connected"], json!(["card0-eDP-1"]));
    assert!(v["sound"][0].as_str().unwrap().contains("HDA-Intel"));
    assert_eq!(v["hostname"], "iyakki-test");
    // kernel log: all, since, grep, newest kept, no continuation lines
    let d = jv(&call(&r, &k, &a, "GET", "/v1/dev/dmesg", &[], b"").unwrap());
    assert_eq!(d["records"].as_array().unwrap().len(), 5);
    assert_eq!(d["next"], 5);
    let d = jv(&call(&r, &k, &a, "GET", "/v1/dev/dmesg", &[("since", "3")], b"").unwrap());
    assert_eq!(d["records"].as_array().unwrap().len(), 2);
    let d = jv(&call(&r, &k, &a, "GET", "/v1/dev/dmesg", &[("grep", "USB")], b"").unwrap());
    assert_eq!(d["records"].as_array().unwrap().len(), 1);
    let d = jv(&call(&r, &k, &a, "GET", "/v1/dev/dmesg", &[("max", "2")], b"").unwrap());
    assert_eq!(d["records"][0]["seq"], 4, "the newest records are kept");
    let all = String::from_utf8_lossy(&call(&r, &k, &a, "GET", "/v1/dev/dmesg", &[], b"").unwrap().body).to_string();
    assert!(!all.contains("ghp_"), "a credential in the log is hidden: {all}");
    assert!(all.contains("hidden"));
    // system logs
    let s = jv(&call(&r, &k, &a, "GET", "/v1/dev/syslog", &[("src", "shell")], b"").unwrap());
    assert!(s["text"].as_str().unwrap().contains("shell started") && !s["text"].as_str().unwrap().contains("ghp_") && !s["text"].as_str().unwrap().contains("Linux version"));
    let b = jv(&call(&r, &k, &a, "GET", "/v1/dev/syslog", &[("src", "boot")], b"").unwrap());
    assert!(b["text"].as_str().unwrap().contains("splash: stopped"));
    let au = jv(&call(&r, &k, &a, "GET", "/v1/dev/syslog", &[("src", "audit")], b"").unwrap());
    assert!(au["text"].as_str().unwrap().contains("login ok"));
    assert_eq!(call(&r, &k, &a, "GET", "/v1/dev/syslog", &[("src", "../../etc/shadow")], b"").unwrap().status, 422);
    // services table
    let st = jv(&call(&r, &k, &a, "GET", "/v1/dev/status", &[], b"").unwrap());
    let names: Vec<&str> = st["services"].as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap()).collect();
    for n in ["k3s (dev cluster)", "Lemonade (local models)", "Firewall", "Shared folders", "Sound", "Graphics"] {
        assert!(names.contains(&n), "{n} in {names:?}");
    }
    assert_eq!(st["services"].as_array().unwrap().iter().find(|x| x["name"] == "Sound").unwrap()["state"], "ready");
    // nothing here wrote anything but the activity log
    assert!(r.dm.activity(20).iter().any(|x| x.action == "dev.info" && x.ok));
    // a missing machine does not crash anything
    let mut empty = r.paths.clone();
    empty.sys.proc = r.dir.join("nope");
    empty.kmsg = r.dir.join("nokmsg");
    let _ = super::debug::info(&empty, "x");
    assert!(super::debug::kmsg(&empty, 0, "", 10).is_empty());
}

fn sb(r: &Rig) -> (files::Sandbox, PathBuf, PathBuf) {
    let home = std::fs::canonicalize(&r.dir).unwrap().join("h/home");
    let work = std::fs::canonicalize(&r.dir).unwrap().join("h/ws");
    let sys = std::fs::canonicalize(&r.dir).unwrap().join("h/etc");
    let data = std::fs::canonicalize(&r.dir).unwrap().join("h/data");
    for d in [&home, &work, &sys, &data, &data.join("vault"), &data.join("devmode")] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(sys.join("hostname"), "iyakki\n").unwrap();
    std::fs::write(sys.join("shadow"), "root:$6$hash::\n").unwrap();
    std::fs::write(sys.join("ssh_host_ed25519_key"), "PRIVATE").unwrap();
    std::fs::write(data.join("vault/system.afv"), "secret").unwrap();
    std::fs::write(data.join("devmode/keys.json"), "[]").unwrap();
    std::fs::write(data.join("settings.json"), "{}").unwrap();
    (files::Sandbox::with_roots(&home, &[home.clone(), work.clone()], &[sys.clone(), data.clone()], &[data.join("vault"), data.join("devmode")]), home, sys)
}

fn push(sb: &files::Sandbox, path: &str, data: &[u8], force: bool) -> Result<Value, (u16, Value)> {
    files::put(sb, path, 0, data.len() as u64, &auth::sha256_hex(data), force, None, data)
}

#[test]
fn files_stay_inside_the_sandbox_links_and_dot_dot_do_not_escape_and_secrets_are_never_readable() {
    let r = rig("files");
    let (s, home, sys) = sb(&r);
    // inside: push, list, stat, pull, remove
    assert!(push(&s, "notes/a.txt", b"hello", false).unwrap()["done"].as_bool().unwrap());
    assert_eq!(std::fs::read(home.join("notes/a.txt")).unwrap(), b"hello");
    assert_eq!(files::get(&s, "~/notes/a.txt", 0, 100).unwrap(), b"hello");
    assert_eq!(files::get(&s, "notes/a.txt", 2, 100).unwrap(), b"llo", "a chunk from an offset");
    let l = files::ls(&s, "notes").unwrap();
    assert_eq!(l["entries"][0]["name"], "a.txt");
    assert_eq!(files::stat(&s, "notes/a.txt", true).unwrap()["sha256"], auth::sha256_hex(b"hello"));
    // an existing file needs force
    assert_eq!(push(&s, "notes/a.txt", b"two", false).unwrap_err().0, 409);
    assert!(push(&s, "notes/a.txt", b"two", true).is_ok());
    assert_eq!(std::fs::read(home.join("notes/a.txt")).unwrap(), b"two");
    // escapes
    for bad in ["../../etc/passwd", "/etc/passwd", "notes/../../../x", "/", "~/../ws2/x"] {
        assert!(files::get(&s, bad, 0, 10).is_err(), "{bad}");
        assert!(push(&s, bad, b"x", true).is_err(), "{bad}");
        assert!(files::rm(&s, bad, true).is_err(), "{bad}");
        assert!(files::mkdir(&s, bad).is_err(), "{bad}");
    }
    assert!(!home.parent().unwrap().join("x").exists());
    // a link to the outside is followed before the check, so reading and writing through it is refused
    let outside = r.dir.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("f"), "outside").unwrap();
    std::os::unix::fs::symlink(&outside, home.join("link")).unwrap();
    assert!(files::get(&s, "link/f", 0, 10).is_err());
    assert!(files::ls(&s, "link").is_err());
    assert!(push(&s, "link/new", b"x", true).is_err());
    assert!(!outside.join("new").exists());
    // a dangling link must not let a write create its target
    std::os::unix::fs::symlink(outside.join("created"), home.join("dangling")).unwrap();
    assert!(push(&s, "dangling", b"x", true).is_err());
    assert!(!outside.join("created").exists(), "nothing was created through the link");
    // a link to a file inside is fine to read, never written through
    std::os::unix::fs::symlink(home.join("notes/a.txt"), home.join("inside")).unwrap();
    assert_eq!(files::get(&s, "inside", 0, 10).unwrap(), b"two");
    // the sandbox roots themselves are not removed; a folder needs -r
    assert!(files::rm(&s, "~", true).is_err());
    assert!(files::rm(&s, "notes", false).is_err());
    assert!(files::rm(&s, "notes", true).is_ok() && !home.join("notes").exists());
    assert!(std::fs::symlink_metadata(home.join("link")).is_ok(), "removing a folder removes links as links");
    files::rm(&s, "link", false).unwrap();
    assert!(outside.join("f").exists(), "the target of a link survives its removal");
    // system places: readable, never writable, secrets hidden
    assert_eq!(files::get(&s, &format!("{}/hostname", sys.display()), 0, 100).unwrap(), b"iyakki\n");
    assert!(push(&s, &format!("{}/hostname", sys.display()), b"x", true).unwrap_err().1["error"].as_str().unwrap().contains("read-only"));
    assert!(files::rm(&s, &format!("{}/hostname", sys.display()), false).is_err());
    for secret in ["shadow", "ssh_host_ed25519_key"] {
        let e = files::get(&s, &format!("{}/{secret}", sys.display()), 0, 100).unwrap_err();
        assert!(e.contains("secrets"), "{secret}: {e}");
    }
    let listing = files::ls(&s, &sys.display().to_string()).unwrap().to_string();
    assert!(listing.contains("hostname") && !listing.contains("shadow") && !listing.contains("ssh_host"), "{listing}");
    let data = sys.parent().unwrap().join("data");
    assert!(files::get(&s, &format!("{}/settings.json", data.display()), 0, 10).is_ok());
    for secret in ["vault/system.afv", "devmode/keys.json"] {
        assert!(files::get(&s, &format!("{}/{secret}", data.display()), 0, 10).unwrap_err().contains("secrets"), "{secret}");
    }
    // a secret by name inside the home is hidden too
    std::fs::write(home.join("id_ed25519"), "k").unwrap();
    assert!(files::get(&s, "id_ed25519", 0, 10).is_err());
    assert!(!files::ls(&s, "~").unwrap().to_string().contains("id_ed25519"));
    // without the system scope the sandbox has no read-only places at all
    let plain = files::Sandbox::with_roots(&home, &[home.clone()], &[], &[]);
    assert!(files::get(&plain, &format!("{}/hostname", sys.display()), 0, 10).is_err());
}

#[test]
fn transfers_are_chunked_checked_resumable_and_bounded() {
    let r = rig("chunks");
    let (s, home, _) = sb(&r);
    let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    let sha = auth::sha256_hex(&data);
    let put = |off: u64, chunk: &[u8], sha: &str| files::put(&s, "big.bin", off, data.len() as u64, sha, false, Some(0o640), chunk);
    // first chunk
    assert_eq!(put(0, &data[..40_000], &sha).unwrap()["have"], 40_000);
    assert!(!home.join("big.bin").exists(), "nothing appears under the real name before the end");
    assert_eq!(files::stat(&s, "big.bin", false).unwrap()["part_size"], 40_000, "the device says how much it kept: that is the resume point");
    // wrong offsets are refused and say where to continue
    let (code, v) = put(10_000, &data[10_000..20_000], &sha).unwrap_err();
    assert_eq!((code, v["have"].clone()), (409, json!(40_000)));
    // resume
    assert_eq!(put(40_000, &data[40_000..90_000], &sha).unwrap()["have"], 90_000);
    let done = put(90_000, &data[90_000..], &sha).unwrap();
    assert_eq!(done["done"], true);
    assert_eq!(std::fs::read(home.join("big.bin")).unwrap(), data);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(home.join("big.bin")).unwrap().permissions().mode() & 0o777, 0o640);
    assert!(!home.join("big.bin.iyakki-part").exists());
    // a wrong checksum drops the partial file and leaves an existing file alone
    std::fs::write(home.join("keep.bin"), "old").unwrap();
    let (code, v) = files::put(&s, "keep.bin", 0, 3, &auth::sha256_hex(b"zzz"), true, None, b"new").unwrap_err();
    assert_eq!(code, 422);
    assert!(v["error"].as_str().unwrap().contains("checksum"));
    assert_eq!(std::fs::read(home.join("keep.bin")).unwrap(), b"old");
    assert!(!home.join("keep.bin.iyakki-part").exists());
    // limits
    assert!(files::put(&s, "x", 0, files::MAX_FILE + 1, &sha, false, None, b"").unwrap_err().1["error"].as_str().unwrap().contains("more than"));
    assert!(files::put(&s, "x", 0, 10, &sha, false, None, &vec![0u8; 11]).is_err(), "more than announced");
    assert!(files::put(&s, "x", 0, (files::MAX_CHUNK + 1) as u64, &sha, false, None, &vec![0u8; files::MAX_CHUNK + 1]).is_err(), "chunk too big");
    assert!(files::put(&s, "x", 0, 1, "nothex", false, None, b"a").is_err());
    assert!(files::put(&s, "x", 0, 1, &"g".repeat(64), false, None, b"a").is_err());
    assert_eq!(files::get(&s, "big.bin", 0, usize::MAX).unwrap().len(), 100_000, "a pull returns at most a chunk and here the whole file");
    let huge = vec![7u8; files::MAX_CHUNK + 1000];
    std::fs::write(home.join("huge.bin"), &huge).unwrap();
    assert_eq!(files::get(&s, "huge.bin", 0, usize::MAX).unwrap().len(), files::MAX_CHUNK);
    // a folder is not a file; a leftover partial file is never listed
    std::fs::create_dir_all(home.join("d")).unwrap();
    assert_eq!(push(&s, "d", b"x", true).unwrap_err().0, 409);
    assert!(files::get(&s, "d", 0, 10).is_err());
    files::put(&s, "half.bin", 0, 10, &sha, false, None, b"12345").unwrap();
    assert!(!files::ls(&s, "~").unwrap().to_string().contains("iyakki-part"));
}

#[test]
fn files_over_the_api_need_the_scope_log_what_they_did_and_never_put_contents_in_the_log() {
    let r = rig("filesapi");
    let a = admin();
    let k = key("laptop", "fileacct", &[Scope::Files]);
    let body = b"top secret payload";
    let sha = auth::sha256_hex(body);
    let o = call(&r, &k, &a, "POST", "/v1/dev/files/put", &[("path", "~/dev-test/a.txt"), ("offset", "0"), ("total", &body.len().to_string()), ("sha256", &sha)], body).unwrap();
    assert_eq!(o.status, 200, "{}", String::from_utf8_lossy(&o.body));
    let path = jv(&o)["path"].as_str().unwrap().to_string();
    let got = call(&r, &k, &a, "GET", "/v1/dev/files/get", &[("path", &path)], b"").unwrap();
    assert_eq!(got.body, body);
    assert_eq!(call(&r, &k, &a, "GET", "/v1/dev/files/get", &[("path", "/etc/hostname")], b"").unwrap().status, 403, "system files need files:system");
    assert_eq!(call(&r, &k, &a, "POST", "/v1/dev/files/rm", &[("path", "~/dev-test"), ("recursive", "1")], b"").unwrap().status, 200);
    let log: String = r.dm.activity(50).iter().map(|x| x.line()).collect::<Vec<_>>().join("\n");
    assert!(log.contains("dev.files.put") && log.contains("push ") && log.contains("remove ") && log.contains("pull "), "{log}");
    assert!(!log.contains("top secret"), "contents are never logged");
    assert!(log.contains("NO ") && log.contains("refused"), "the refusal is logged: {log}");
    // files:system adds read-only access to /etc for an administrator account
    let k2 = key("laptop2", "fileacct", &[Scope::Files, Scope::FilesSystem]);
    assert_eq!(call(&r, &k2, &a, "GET", "/v1/dev/files/get", &[("path", "/etc/hostname")], b"").unwrap().status, if Path::new("/etc/hostname").exists() { 200 } else { 403 });
    assert_eq!(call(&r, &k2, &a, "GET", "/v1/dev/files/get", &[("path", "/etc/shadow")], b"").unwrap().status, 403, "never the shadow file");
    assert_eq!(call(&r, &k2, &a, "POST", "/v1/dev/files/mkdir", &[("path", "/etc/iyakki-dev-test")], b"").unwrap().status, 403, "system places are read-only");
    // a user account with files:system gets nothing extra
    let k3 = key("laptop3", "fileacct", &[Scope::Files, Scope::FilesSystem]);
    assert_eq!(call(&r, &k3, &userp(), "GET", "/v1/dev/files/get", &[("path", "/etc/hostname")], b"").unwrap().status, 403);
}

#[test]
fn input_events_are_validated_queued_in_order_bounded_and_refused_on_a_password_field() {
    let r = rig("input");
    let a = admin();
    let k = key("laptop", "root", &[Scope::Input]);
    let send = |k: &DevKey, b: Value| call(&r, k, &a, "POST", "/v1/dev/input", &[], b.to_string().as_bytes()).unwrap();
    assert_eq!(send(&k, json!({"kind":"tap","x":100,"y":200})).status, 200);
    assert_eq!(send(&k, json!({"kind":"key","name":"Enter"})).status, 200);
    assert_eq!(send(&k, json!({"kind":"text","text":"hunter2 and more"})).status, 200);
    assert_eq!(send(&k, json!({"kind":"swipe","x1":1,"y1":2,"x2":3,"y2":4,"ms":200})).status, 200);
    assert_eq!(input::take(&r.dir), vec![InputEv::Tap(100, 200), InputEv::Key("Enter".into()), InputEv::Text("hunter2 and more".into()), InputEv::Swipe(1, 2, 3, 4, 200)]);
    assert!(input::take(&r.dir).is_empty());
    for bad in [json!({"kind":"tap","x":-1,"y":2}), json!({"kind":"tap","x":99999,"y":2}), json!({"kind":"tap"}), json!({"kind":"key","name":"Ctrl+Alt+Del"}), json!({"kind":"key","name":""}), json!({"kind":"text","text":""}), json!({"kind":"text","text":"x".repeat(1001)}), json!({"kind":"mouse"}), json!({"kind":"swipe","x1":1,"y1":1,"x2":2,"y2":2,"ms":0})] {
        assert_eq!(send(&k, bad.clone()).status, 422, "{bad}");
    }
    assert_eq!(call(&r, &k, &a, "POST", "/v1/dev/input", &[], b"not json").unwrap().status, 400);
    assert!(input::take(&r.dir).is_empty(), "nothing invalid was queued");
    // the queue is bounded
    for _ in 0..input::MAX_QUEUE {
        assert_eq!(send(&k, json!({"kind":"key","name":"a"})).status, 200);
    }
    assert_eq!(send(&k, json!({"kind":"key","name":"a"})).status, 429);
    input::take(&r.dir);
    // a password field has focus: refused unless input:password
    input::set_secret_focus(&r.dir, true);
    let o = send(&k, json!({"kind":"text","text":"x"}));
    assert_eq!(o.status, 403);
    assert!(String::from_utf8_lossy(&o.body).contains("password"));
    assert!(input::take(&r.dir).is_empty());
    let kp = key("laptop", "root", &[Scope::Input, Scope::InputPassword]);
    assert_eq!(send(&kp, json!({"kind":"text","text":"x"})).status, 200);
    input::set_secret_focus(&r.dir, false);
    input::take(&r.dir);
    assert_eq!(send(&k, json!({"kind":"key","name":"Ctrl+c"})).status, 200);
    // typed text is logged by length only
    let log: String = r.dm.activity(600).iter().map(|x| x.line()).collect::<Vec<_>>().join("\n");
    assert!(!log.contains("hunter2") && log.contains("text of 16 characters") && log.contains("tap 100 200"), "{log}");
}

#[test]
fn sessions_end_when_a_computer_is_revoked_a_scope_is_taken_away_or_developer_mode_goes_off() {
    let r = rig("sessions");
    let adm = admin();
    wr(&r.dir.join("devmode/keys.json"), &serde_json::to_string(&vec![key("laptop", "root", &[Scope::Install, Scope::Forward, Scope::Shell]), key("mac", "root", &[Scope::Install, Scope::Forward])]).unwrap());
    let now = r.dm.now();
    let a = sessions::open(&r.dir, "laptop", "root", Scope::Forward, "forward 6443", now).unwrap();
    let b = sessions::open(&r.dir, "laptop", "root", Scope::Shell, "shell", now).unwrap();
    let c = sessions::open(&r.dir, "mac", "root", Scope::Forward, "forward 13305", now).unwrap();
    assert_eq!(sessions::list(&r.dir).len(), 3);
    assert!(!a.ended() && !b.ended() && !c.ended());
    // taking the forward scope away ends that computer's forwards only
    r.dm.set_scope(&adm, "root", "laptop", Scope::Forward, false).unwrap();
    assert!(a.ended() && !b.ended() && !c.ended());
    // revoking ends everything of the computer at once
    r.dm.revoke(&adm, "root", "laptop").unwrap();
    assert!(b.ended() && !c.ended());
    // an administrator disconnects one session from the list
    let id = sessions::list(&r.dir).iter().find(|s| s.computer == "mac").unwrap().id;
    assert!(sessions::end(&r.dir, id) && c.ended() && !sessions::end(&r.dir, 999_999));
    drop((a, b, c));
    assert!(sessions::list(&r.dir).is_empty(), "closed sessions leave the list");
    // switching developer mode off ends them all
    let d = sessions::open(&r.dir, "mac", "root", Scope::Forward, "forward 1", now).unwrap();
    r.dm.disable(&adm, "root").unwrap();
    assert!(d.ended());
    drop(d);
    // bounded per computer
    r.dm.enable(&adm, "root", 0).unwrap();
    let held: Vec<_> = (0..sessions::MAX_PER_COMPUTER).map(|i| sessions::open(&r.dir, "mac", "root", Scope::Forward, &format!("f{i}"), now).unwrap()).collect();
    assert!(sessions::open(&r.dir, "mac", "root", Scope::Forward, "one too many", now).is_err());
    drop(held);
}

#[test]
fn automatic_approval_is_chosen_by_an_administrator_for_hours_and_never_covers_root_power_or_a_revoked_computer() {
    let r = rig("autoshell");
    let adm = admin();
    wr(&r.dir.join("devmode/keys.json"), &serde_json::to_string(&vec![key("laptop", "root", &[Scope::Install, Scope::Shell, Scope::Power]), key("mac", "bob", &[Scope::Install, Scope::Shell]), key("plain", "root", &[Scope::Install])]).unwrap());
    let laptop = r.dm.keys().into_iter().find(|k| k.name == "laptop").unwrap();
    let mac = r.dm.keys().into_iter().find(|k| k.name == "mac").unwrap();
    // off by default: a request waits for the person at the screen
    assert_eq!(r.dm.shell_request(&mac, false, 5, "").unwrap(), ShellStatus::Pending);
    // only an administrator switches it on, and only for a computer that holds the shell scope
    assert!(r.dm.shell_auto_on(&userp(), "bob", "mac", 4).is_err());
    assert!(r.dm.shell_auto_on(&adm, "root", "plain", 4).is_err());
    assert!(r.dm.shell_auto_on(&adm, "root", "nobody", 4).is_err());
    let a = r.dm.shell_auto_on(&adm, "root", "mac", 100_000).unwrap();
    assert_eq!(a.until - r.dm.now(), grants::AUTO_MAX_HOURS as i64 * 3600, "capped");
    // the next request is granted at once, as a plain shell, for the minutes asked
    match r.dm.shell_request(&mac, false, 7, "").unwrap() {
        ShellStatus::Granted(g) => assert_eq!((g.as_root, g.once, g.expires - r.dm.now(), g.by.as_str()), (false, false, 420, "automatic approval")),
        s => panic!("expected a grant, got {s:?}"),
    }
    assert!(r.dm.shell_pending().is_empty(), "no card is left waiting");
    // root is always asked, whoever the account is; the other computers are unaffected
    r.dm.shell_auto_on(&adm, "root", "laptop", 4).unwrap();
    r.dm.shell_end(&adm, "root", "laptop").unwrap();
    match r.dm.shell_request(&laptop, true, 5, "").unwrap() {
        ShellStatus::Pending => {}
        s => panic!("a root request must wait for approval, got {s:?}"),
    }
    assert!(r.dm.shell_pending()[0].as_root);
    // power is never covered
    r.dm.shell_end(&adm, "root", "laptop").unwrap();
    assert_eq!(r.dm.grant_request(&laptop, "power", false, 5, "").unwrap(), ShellStatus::Pending);
    // it ends by itself
    r.clock.store(NOW + 4 * 3600 + 1, Ordering::SeqCst);
    assert!(!r.dm.shell_auto_active("laptop") && r.dm.shell_auto_list().iter().any(|x| x.computer == "mac"));
    r.clock.store(NOW + grants::AUTO_MAX_HOURS as i64 * 3600, Ordering::SeqCst);
    assert!(r.dm.shell_auto_list().is_empty());
    // the administrator switches it off, a revoke forgets it, and developer mode off stops it
    r.clock.store(NOW, Ordering::SeqCst);
    r.dm.shell_auto_on(&adm, "root", "mac", 4).unwrap();
    r.dm.shell_auto_off(&adm, "root", "mac").unwrap();
    assert!(!r.dm.shell_auto_active("mac"));
    r.dm.shell_auto_on(&adm, "root", "mac", 4).unwrap();
    r.dm.revoke(&adm, "root", "mac").unwrap();
    assert!(!r.dm.shell_auto_list().iter().any(|x| x.computer == "mac"), "a revoked computer is forgotten");
    r.dm.shell_auto_on(&adm, "root", "laptop", 4).unwrap();
    r.dm.disable(&adm, "root").unwrap();
    assert!(!r.dm.shell_auto_active("laptop"), "developer mode off: asked again");
}

#[test]
fn the_shell_grant_is_asked_for_approved_for_minutes_or_one_session_denied_expired_and_shown_in_the_bar() {
    let r = rig("grant");
    let adm = admin();
    wr(&r.dir.join("devmode/keys.json"), &serde_json::to_string(&vec![key("laptop", "root", &[Scope::Install, Scope::Shell]), key("mac", "bob", &[Scope::Install, Scope::Shell]), key("plain", "root", &[Scope::Install])]).unwrap());
    let laptop = r.dm.keys().into_iter().find(|k| k.name == "laptop").unwrap();
    assert_eq!(r.dm.shell_status("laptop"), ShellStatus::None);
    assert!(r.dm.shell_banner().is_none());
    // a request is pending; nobody can run anything yet
    assert_eq!(r.dm.shell_request(&laptop, true, 30, "debug the audio\nchain").unwrap(), ShellStatus::Pending);
    assert_eq!(r.dm.shell_status("laptop"), ShellStatus::Pending);
    assert_eq!(r.dm.shell_pending()[0].why, "debug the audio chain");
    assert_eq!(r.dm.shell_banner().as_deref(), Some("SHELL REQUEST from laptop"));
    assert!(r.dm.shell_grant("laptop").is_none());
    // only an administrator approves; a computer without the scope cannot be approved
    assert!(r.dm.shell_approve(&userp(), "bob", "laptop", 10).is_err());
    assert!(r.dm.shell_approve(&adm, "root", "nobody", 10).is_err());
    let g = r.dm.shell_approve(&adm, "root", "laptop", 10).unwrap();
    assert_eq!((g.as_root, g.once, g.expires), (true, false, NOW + 600), "asked as root by the root account: the grant says so");
    assert_eq!(r.dm.shell_banner().as_deref(), Some("SHELL ACCESS by laptop"));
    assert!(r.dm.shell_pending().is_empty());
    // it expires
    r.clock.store(NOW + 599, Ordering::SeqCst);
    assert!(r.dm.shell_grant("laptop").is_some());
    r.clock.store(NOW + 600, Ordering::SeqCst);
    assert!(r.dm.shell_grant("laptop").is_none() && r.dm.shell_banner().is_none(), "expired: no grant, no banner");
    // as root is only for the root account, whatever is asked
    let mac = r.dm.keys().into_iter().find(|k| k.name == "mac").unwrap();
    r.dm.shell_request(&mac, true, 5, "").unwrap();
    assert!(!r.dm.shell_pending()[0].as_root, "a non-root account never gets root");
    // deny
    r.dm.shell_deny(&adm, "root", "mac").unwrap();
    assert_eq!(r.dm.shell_status("mac"), ShellStatus::Denied);
    assert!(r.dm.shell_approve(&adm, "root", "mac", 5).is_err(), "a denied request is gone");
    // a request that nobody answers times out
    r.dm.shell_request(&mac, false, 5, "").unwrap();
    r.clock.fetch_add(grants::REQUEST_SECS, Ordering::SeqCst);
    assert_eq!(r.dm.shell_status("mac"), ShellStatus::None);
    assert!(r.dm.shell_approve(&adm, "root", "mac", 5).unwrap_err().contains("no request waiting"));
    // one session: lasts until its session closes
    r.dm.shell_request(&mac, false, 5, "").unwrap();
    let g = r.dm.shell_approve(&adm, "root", "mac", 0).unwrap();
    assert!(g.once);
    r.dm.shell_session_closed("mac");
    assert!(r.dm.shell_grant("mac").is_none(), "the grant ended with the session");
    // minutes are capped; ending early kills the sessions; revoke and switching off remove grants
    r.dm.shell_request(&laptop, false, 100_000, "").unwrap();
    assert_eq!(r.dm.shell_pending()[0].minutes, grants::MAX_MINUTES);
    let g = r.dm.shell_approve(&adm, "root", "laptop", 100_000).unwrap();
    assert_eq!(g.expires - r.dm.now(), grants::MAX_MINUTES as i64 * 60);
    let sess = sessions::open(&r.dir, "laptop", "root", Scope::Shell, "shell", r.dm.now()).unwrap();
    r.dm.shell_end(&adm, "root", "laptop").unwrap();
    assert!(sess.ended() && r.dm.shell_grant("laptop").is_none());
    r.dm.shell_request(&laptop, false, 5, "").unwrap();
    r.dm.shell_approve(&adm, "root", "laptop", 5).unwrap();
    r.dm.revoke(&adm, "root", "laptop").unwrap();
    assert!(r.dm.shell_grant("laptop").is_none());
    r.dm.shell_request(&mac, false, 5, "").unwrap();
    r.dm.shell_approve(&adm, "root", "mac", 5).unwrap();
    r.dm.disable(&adm, "root").unwrap();
    assert!(r.dm.shell_grants().is_empty() && r.dm.shell_banner().is_none());
    let log: String = r.dm.activity(100).iter().map(|x| x.line()).collect::<Vec<_>>().join("\n");
    assert!(log.contains("dev.shell.request") && log.contains("devmode.shell.grant") && log.contains("devmode.shell.deny") && log.contains("devmode.shell.end"));
}

#[test]
fn commands_run_as_the_accounts_own_system_user_and_root_only_for_the_root_account_with_an_as_root_grant() {
    use super::shellexec::identity;
    let passwd = "root:x:0:0:root:/root:/bin/sh\nalice:x:1001:1001::/home/alice:/bin/sh\n# comment\nbob:x:1002:1002::/home/bob:/bin/sh\n";
    let tmp = Path::new("/tmp/h");
    // root, with the grant saying so
    let i = identity("root", true, passwd, 0, tmp).unwrap();
    assert!(i.root && i.uid.is_none());
    // root without it: refused; another account asking "as root" never gets it
    assert!(identity("root", false, passwd, 0, tmp).unwrap_err().contains("as root"));
    let i = identity("alice", true, passwd, 0, tmp).unwrap();
    assert!(!i.root && i.uid == Some(1001) && i.gid == Some(1001) && i.home == Path::new("/home/alice"), "{i:?}");
    // a process that is not root cannot change user: it stays itself
    assert_eq!(identity("alice", false, passwd, 1000, tmp).unwrap().uid, None);
    // an account with no system user gets the least-privileged one when the process is root, never root
    let i = identity("carol", true, passwd, 0, tmp).unwrap();
    assert!(!i.root && i.uid == Some(65534) && i.user == "nobody", "{i:?}");
    // a broken passwd line is not a user
    assert!(identity("alice", false, "alice:x:abc:1::/h:/bin/sh\n", 0, tmp).is_err());
}
