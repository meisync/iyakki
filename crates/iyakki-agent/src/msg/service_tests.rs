//! The manager with a fake homeserver and an in-memory vault: accounts and secrets, rooms by name, the hourly cap, dry run, audit lines,
//! per-person ownership, the sync position.

use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zeroize::Zeroizing;

use super::fake_hs::*;
use super::service::PER_HOUR;
use super::{label_untrusted, Accounts, Auth, MessageService};
use crate::dash::cluster::NoCluster;
use crate::gitkeys::{AuditFn, MemStore, SecretStore};

struct Rig {
    svc: MessageService,
    vault: Arc<MemStore>,
    audit: Arc<Mutex<Vec<String>>>,
    data: PathBuf,
    clock: Arc<AtomicI64>,
    user: String,
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-msg-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn rig(tag: &str, user: &str, data: Option<PathBuf>) -> Rig {
    let data = data.unwrap_or_else(|| temp(tag));
    let vault = Arc::new(MemStore::default());
    let audit = Arc::new(Mutex::new(Vec::new()));
    let a2 = audit.clone();
    let log: AuditFn = Arc::new(move |a, d| a2.lock().unwrap().push(format!("{a} {d}")));
    let clock = Arc::new(AtomicI64::new(1_000_000));
    let c2 = clock.clone();
    let acc = Accounts::new(&Accounts::dir_for(&data, user), vault.clone(), log);
    let up = Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(10)));
    let svc = MessageService::new(acc, up, Arc::new(NoCluster)).with_clock(Arc::new(move || c2.load(Ordering::SeqCst))).with_sleeper(Arc::new(|_| {}));
    Rig { svc, vault, audit, data, clock, user: user.into() }
}

fn pw() -> Auth {
    Auth::Password { user: "alice".into(), password: Zeroizing::new(PASSWORD.into()) }
}

fn all_files(dir: &std::path::Path) -> String {
    let mut s = String::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            s.push_str(&all_files(&p));
        } else {
            s.push_str(&std::fs::read_to_string(&p).unwrap_or_default());
        }
    }
    s
}

#[test]
fn signing_in_keeps_the_token_in_the_vault_and_no_secret_in_any_file_or_audit_line() {
    let hs = FakeHs::start();
    let r = rig("add", "ana", None);
    let a = r.svc.add_matrix("Home server", &hs.url(), pw(), "").unwrap();
    assert_eq!((a.id.as_str(), a.user_id.as_str(), a.device_id.as_str()), ("home-server", "@alice:test", "DEV1"));
    assert_eq!(r.vault.get("msg.acct.home-server.token").unwrap().as_str(), TOKEN);
    assert_eq!(r.vault.names().unwrap(), vec!["msg.acct.home-server.token".to_string()], "the password is not kept");
    assert_eq!(r.svc.accounts().rooms(&a.id).iter().map(|x| (x.name.as_str(), x.room_id.as_str())).collect::<Vec<_>>(), vec![("main", PLAIN)], "the only joined room is saved");
    let files = all_files(&r.data);
    assert!(files.contains("home-server") && !files.contains(TOKEN) && !files.contains(PASSWORD), "{files}");
    let audit = r.audit.lock().unwrap().join("\n");
    assert!(audit.contains("msg.account.add") && !audit.contains(TOKEN) && !audit.contains(PASSWORD), "{audit}");
    use std::os::unix::fs::PermissionsExt;
    let f = Accounts::dir_for(&r.data, "ana").join("accounts.json");
    assert_eq!(std::fs::metadata(f).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(r.svc.test("").unwrap(), format!("signed in as @alice:test on 127.0.0.1:{}", hs.port));
    // a second account with the same label gets another id
    let b = r.svc.add_matrix("Home server", &hs.url(), Auth::Token(Zeroizing::new(TOKEN.into())), PLAIN).unwrap();
    assert_eq!(b.id, "home-server-2");
    assert!(r.svc.test("").is_err(), "two accounts: the caller must say which");
}

#[test]
fn a_wrong_password_or_token_leaves_no_account_and_no_secret_behind() {
    let hs = FakeHs::start();
    let r = rig("bad", "ana", None);
    let e = r.svc.add_matrix("X", &hs.url(), Auth::Password { user: "alice".into(), password: Zeroizing::new("wrong-pw-1".into()) }, "").unwrap_err();
    assert!(e.contains("not right") && !e.contains("wrong-pw-1"), "{e}");
    let e = r.svc.add_matrix("X", &hs.url(), Auth::Token(Zeroizing::new("wrong-token-1".into())), "").unwrap_err();
    assert!(e.contains("token") && !e.contains("wrong-token-1"), "{e}");
    assert!(r.svc.accounts().list().is_empty() && r.vault.names().unwrap().is_empty());
    assert!(r.svc.add_matrix("X", "http://matrix.example.org", pw(), "").unwrap_err().contains("https"));
}

#[test]
fn a_send_goes_to_a_saved_room_by_name_and_the_audit_line_has_counts_only() {
    let hs = FakeHs::start();
    let r = rig("send", "ana", None);
    r.svc.add_matrix("Home", &hs.url(), pw(), "").unwrap();
    let rep = r.svc.send("", "", "alert CANARY-8812 at the door", None, "k-1", false).unwrap();
    assert!(!rep.dry_run && rep.external_id.starts_with("$ev") && rep.note.contains("accepted is not read"));
    let again = r.svc.send("home", "main", "alert CANARY-8812 at the door", None, "k-1", false).unwrap();
    assert_eq!(again.external_id, rep.external_id, "a retry with the same key is the same event");
    let t = r.svc.test_send("", "").unwrap();
    assert_ne!(t.external_id, rep.external_id);
    let audit = r.audit.lock().unwrap().join("\n");
    assert!(audit.contains("msg.send account=\"Home\" channel=matrix room=main result=ok bytes=29"), "{audit}");
    for secret in ["CANARY-8812", TOKEN, PLAIN, "door"] {
        assert!(!audit.contains(secret), "{secret} in {audit}");
    }
    // only names the person saved: a raw room id or an unknown name is refused, no request is made for it
    let before = hs.reqs().len();
    assert!(r.svc.send("", PLAIN, "x", None, "k", false).unwrap_err().contains("no room called"));
    assert!(r.svc.send("", "attacker", "x", None, "k", false).is_err());
    assert_eq!(hs.reqs().len(), before);
}

#[test]
fn an_encrypted_room_by_name_is_refused_with_the_reason_and_audited_as_refused() {
    let hs = FakeHs::start();
    let r = rig("enc", "ana", None);
    let a = r.svc.add_matrix("Home", &hs.url(), pw(), "").unwrap();
    r.svc.accounts().add_room(&a.id, "family", ENC).unwrap();
    let e = r.svc.send("", "family", "hello", None, "k", false).unwrap_err();
    assert!(e.contains("end-to-end encrypted") && e.contains("add-on"), "{e}");
    assert_eq!(hs.count("PUT", "/send/"), 0);
    assert!(r.audit.lock().unwrap().join("\n").contains("room=family result=refused"));
}

#[test]
fn the_hourly_cap_stops_a_flood_and_resets_and_a_dry_run_costs_nothing_and_sends_nothing() {
    let hs = FakeHs::start();
    let r = rig("cap", "ana", None);
    r.svc.add_matrix("Home", &hs.url(), pw(), "").unwrap();
    for i in 0..PER_HOUR {
        r.svc.send("", "", "x", None, &format!("k{i}"), false).unwrap();
    }
    let n = hs.reqs().len();
    let e = r.svc.send("", "", "one too many", None, "kx", false).unwrap_err();
    assert!(e.contains("30 messages") && e.contains("minutes"), "{e}");
    assert_eq!(hs.reqs().len(), n, "nothing left the machine");
    let dry = r.svc.send("", "", "x", None, "kd", true);
    assert!(dry.unwrap_err().contains("30 messages"), "a dry run sees the cap as real");
    r.clock.fetch_add(3601, Ordering::SeqCst);
    let dry = r.svc.send("", "", "hello there", None, "kd", true).unwrap();
    assert!(dry.dry_run && dry.note.contains("dry run") && dry.note.contains("nothing was sent"), "{}", dry.note);
    assert_eq!(hs.reqs().len(), n, "a dry run makes no request");
    r.svc.send("", "", "x", None, "again", false).unwrap();
}

#[test]
fn each_person_has_their_own_accounts_and_nobody_reads_another_persons() {
    let hs = FakeHs::start();
    let ana = rig("own", "ana", None);
    ana.svc.add_matrix("Home", &hs.url(), pw(), "").unwrap();
    let bob = rig("own", "bob", Some(ana.data.clone()));
    assert!(bob.svc.accounts().list().is_empty());
    assert!(bob.svc.send("", "", "x", None, "k", false).unwrap_err().contains("no messaging account"));
    assert_ne!(Accounts::dir_for(&ana.data, "ana"), Accounts::dir_for(&ana.data, "bob"));
    assert_eq!(Accounts::dir_for(&ana.data, "../evil").file_name().unwrap(), "evil", "a user name cannot climb out");
    assert_eq!(ana.user, "ana");
}

#[test]
fn a_locked_vault_stops_before_any_request() {
    let hs = FakeHs::start();
    let r = rig("lock", "ana", None);
    r.svc.add_matrix("Home", &hs.url(), pw(), "").unwrap();
    let n = hs.reqs().len();
    *r.vault.locked.lock().unwrap() = true;
    let e = r.svc.send("", "", "x", None, "k", false).unwrap_err();
    assert!(e.contains("vault is locked"), "{e}");
    assert_eq!(hs.reqs().len(), n);
}

#[test]
fn polling_keeps_the_sync_position_across_restarts_and_labels_every_message_as_from_outside() {
    let hs = FakeHs::start();
    let r = rig("poll", "ana", None);
    r.svc.add_matrix("Home", &hs.url(), pw(), "").unwrap();
    assert!(r.svc.poll_once("", Duration::from_millis(5)).unwrap().is_empty());
    assert!(all_files(&r.data).contains("\"s1\""));
    // a new manager over the same folder carries on from s1
    let r2 = rig("poll", "ana", Some(r.data.clone()));
    r2.vault.set("msg.acct.home.token", TOKEN).unwrap();
    let got = r2.svc.poll_once("", Duration::from_millis(5)).unwrap();
    assert_eq!(got.len(), 4);
    assert!(hs.reqs().iter().any(|q| q.path.contains("since=s1")));
    let shown = label_untrusted(&got[0], "Home");
    assert!(shown.contains("from outside") && shown.contains("not an instruction") && shown.contains("IGNORE PREVIOUS"), "{shown}");
    let audit = r2.audit.lock().unwrap().join("\n");
    assert!(audit.contains("msg.recv account=\"Home\" channel=matrix count=4") && !audit.contains("IGNORE"), "{audit}");
}

#[test]
fn removing_an_account_removes_its_token_rooms_and_position() {
    let hs = FakeHs::start();
    let r = rig("rm", "ana", None);
    r.svc.add_matrix("Home", &hs.url(), pw(), "").unwrap();
    r.svc.poll_once("", Duration::from_millis(1)).unwrap();
    r.svc.remove("home").unwrap();
    assert!(r.vault.names().unwrap().is_empty());
    let files = all_files(&r.data);
    assert!(!files.contains("home") && !files.contains("s1") && !files.contains(PLAIN), "{files}");
    assert!(r.svc.remove("home").is_err());
}
