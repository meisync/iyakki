//! Re-finding a moved device by its announced key id, the saved list (duplicates, stale entries, rename, prune) and the device's side
//! (what removing a pairing takes away). All with fakes: no network, no real device.

use std::cell::{Cell, RefCell};

use iyakki_core_local::netscan::mdns::net::Found;
use iyakki_sec::policy::Role;

use super::client::{self, Device, Home};
use super::cli_devices;
use super::DevMode;

const NOW: i64 = 1_800_000_000;
const DAY: i64 = 86_400;

fn fp(seed: u8) -> String {
    (0..32u8).map(|i| format!("{:02X}", i.wrapping_mul(7).wrapping_add(seed))).collect::<Vec<_>>().join(":")
}

fn dev(name: &str, host: &str, seed: u8, account: &str, seen_days_ago: i64) -> Device {
    Device { name: name.into(), host: host.into(), addr: String::new(), port: 7979, fingerprint: fp(seed), account: account.into(), paired_at: NOW - 100 * DAY, seen_at: NOW - seen_days_ago * DAY }
}

fn home(tag: &str) -> Home {
    let d = std::env::temp_dir().join(format!("iyakki-pairing-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    Home::at(&d)
}

/// TLS with a pin: only `good` shows the pinned certificate, `other` is a different IYAKKI, the rest do not answer.
fn tls(good: &'static str, other: &'static str) -> impl FnMut(&str) -> Result<String, String> {
    move |ip: &str| {
        if ip == good {
            Ok(format!("answered by {ip}"))
        } else if ip == other {
            Err("the device's certificate changed (it is 00:11, the one paired was AA:BB): if the device was reinstalled, pair again".into())
        } else {
            Err(format!("cannot reach {ip}:7979: timed out"))
        }
    }
}

#[test]
fn a_device_paired_by_address_is_found_again_by_its_key_id_and_the_new_address_is_saved_without_a_new_code() {
    let h = home("moved");
    let d = dev("macbook", "192.0.2.82", 1, "admin", 1);
    h.add(d.clone()).unwrap();
    let asked = RefCell::new(Vec::new());
    let by_key = |id: &str| {
        asked.borrow_mut().push(id.to_string());
        vec!["192.0.2.100".to_string()]
    };
    let mut notes = Vec::new();
    let r = client::reach_by_key(&d, Some(&h), &|_| panic!("no name to look up"), &by_key, &mut tls("192.0.2.100", "none"), &mut |n| notes.push(n.to_string())).unwrap();
    assert_eq!(r, "answered by 192.0.2.100");
    assert_eq!(notes, vec!["device moved: 192.0.2.82 -> 192.0.2.100".to_string()]);
    assert_eq!(asked.borrow().as_slice(), [d.key_id()], "only the short key id is asked for");
    assert_eq!(h.device(Some("macbook")).unwrap().host, "192.0.2.100", "saved");
    // it moved again: found again the same way, still one entry
    let d2 = h.device(Some("macbook")).unwrap();
    client::reach_by_key(&d2, Some(&h), &|_| vec![], &|_| vec!["192.0.2.107".into()], &mut tls("192.0.2.107", "none"), &mut |_| {}).unwrap();
    assert_eq!(h.devices().len(), 1);
    assert_eq!(h.device(None).unwrap().host, "192.0.2.107");
    // the saved address answers: no scan at all
    let n = Cell::new(0);
    client::reach_by_key(&h.device(None).unwrap(), Some(&h), &|_| vec![], &|_| { n.set(n.get() + 1); vec![] }, &mut tls("192.0.2.107", "none"), &mut |_| {}).unwrap();
    assert_eq!(n.get(), 0);
}

#[test]
fn a_different_key_at_the_old_address_or_in_a_forged_announcement_is_never_trusted() {
    let h = home("wrongkey");
    let d = dev("macbook", "192.0.2.82", 1, "admin", 1);
    h.add(d.clone()).unwrap();
    // another device took the old address and nothing announces our key: the error says the certificate changed, nothing is saved
    let mut notes = Vec::new();
    let e = client::reach_by_key(&d, Some(&h), &|_| vec![], &|_| vec![], &mut tls("none", "192.0.2.82"), &mut |n| notes.push(n.to_string())).unwrap_err();
    assert!(e.contains("certificate changed") || e.contains("no device announcing"), "{e}");
    assert!(notes.is_empty() && h.device(None).unwrap().host == "192.0.2.82");
    // somebody announces our key id from an address whose certificate is another one: refused, nothing saved
    let e = client::reach_by_key(&d, Some(&h), &|_| vec![], &|_| vec!["192.0.2.99".into()], &mut tls("none", "192.0.2.99"), &mut |n| notes.push(n.to_string())).unwrap_err();
    assert!(e.contains("certificate changed"), "{e}");
    assert!(notes.is_empty() && h.device(None).unwrap().host == "192.0.2.82", "a forged announcement changes nothing");
    // nobody found at all: the error names the old address problem and the missing announcement
    let e = client::reach_by_key(&d, None, &|_| vec![], &|_| vec![], &mut tls("none", "none"), &mut |_| {}).unwrap_err();
    assert!(e.contains("timed out") && e.contains("no device announcing this device's key"), "{e}");
    // a DNS name chosen by the person is left alone: no scan, no replacement
    let mut named = d.clone();
    named.host = "box.example.com".into();
    assert!(client::reach_by_key(&named, None, &|_| panic!("no lookup"), &|_| panic!("no scan"), &mut |h| Ok::<_, String>(h.to_string()), &mut |_| {}).is_ok());
    assert!(client::reach_by_key(&named, None, &|_| panic!("no lookup"), &|_| panic!("no scan"), &mut |_| Err::<String, _>("down".into()), &mut |_| {}).is_err());
}

#[test]
fn a_device_paired_by_name_also_falls_back_to_its_key_when_the_name_is_not_answered() {
    let h = home("byname");
    let mut d = dev("mac", "iyakki-0a1b2c.local", 2, "admin", 1);
    d.addr = "192.0.2.5".into();
    h.add(d.clone()).unwrap();
    let mut notes = Vec::new();
    client::reach_by_key(&d, Some(&h), &|_| vec![], &|_| vec!["192.0.2.6".into()], &mut tls("192.0.2.6", "none"), &mut |n| notes.push(n.to_string())).unwrap();
    assert_eq!(notes, vec!["device moved: 192.0.2.5 -> 192.0.2.6".to_string()]);
    let saved = h.device(None).unwrap();
    assert_eq!((saved.host.as_str(), saved.addr.as_str()), ("iyakki-0a1b2c.local", "192.0.2.6"), "the name stays, the address is updated");
}

fn found(host: &str, addrs: &[&str], kind: &str, txt: &[&str]) -> Found {
    Found { host: host.into(), addrs: addrs.iter().map(|a| a.parse().unwrap()).collect(), services: vec![(kind.into(), 7979)], txt: txt.iter().map(|t| t.to_string()).collect() }
}

#[test]
fn only_announcements_with_this_key_id_on_the_developer_service_give_addresses() {
    let id = auth_id(1);
    let list = vec![
        found("iyakki-aaaaaa.local", &["192.0.2.100", "fe80::1", "fd00::5"], "_iyakki-dev._tcp", &["v=1", &format!("fp={}", id.to_lowercase())]),
        found("iyakki-bbbbbb.local", &["192.0.2.101"], "_iyakki-dev._tcp", &["v=1", "fp=FFFFFFFFFFFFFFFF"]),
        found("iyakki-cccccc.local", &["192.0.2.102"], "_iyakki-dev._tcp", &["v=1"]),
        found("iyakki-dddddd.local", &["192.0.2.103"], "_iyakki-web._tcp", &["fp=".to_string().as_str(), &format!("fp={id}")]),
    ];
    assert_eq!(client::addresses_for_key(&list, &id), vec!["192.0.2.100".to_string(), "fd00::5".to_string()], "link-local, other keys, no key and the web service are left out");
    assert!(client::addresses_for_key(&list, "").is_empty() && client::addresses_for_key(&list, "AB").is_empty(), "a short or empty id matches nothing");
}

fn auth_id(seed: u8) -> String {
    super::auth::key_id(&fp(seed))
}

#[test]
fn the_list_marks_duplicates_and_stale_entries_and_shows_last_seen_address_and_time() {
    let h = home("list");
    h.add(dev("mb-old", "192.0.2.82", 1, "admin", 40)).unwrap();
    h.add(dev("mb", "192.0.2.107", 1, "admin", 0)).unwrap();
    h.add(dev("gone", "192.0.2.9", 3, "admin", 30)).unwrap();
    let lines = cli_devices::list_lines(&h, NOW);
    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains("mb-old") && lines[0].contains("DUPLICATE of mb") && lines[0].contains("40 days ago at 192.0.2.82"), "{}", lines[0]);
    assert!(lines[1].contains("just now at 192.0.2.107") && !lines[1].contains("DUPLICATE"), "{}", lines[1]);
    assert!(lines[2].contains("STALE?") && lines[2].contains("30 days"), "{}", lines[2]);
    assert!(lines[0].starts_with('*'), "the default keeps its star");
}

#[test]
fn prune_merges_exact_duplicates_and_removes_only_stale_entries_that_do_not_answer_and_asks_first() {
    let h = home("prune");
    h.add(dev("a", "192.0.2.82", 1, "admin", 40)).unwrap(); // the default, an old address of the same device as b
    h.add(dev("b", "192.0.2.100", 1, "admin", 1)).unwrap();
    h.add(dev("c", "192.0.2.9", 3, "admin", 40)).unwrap(); // old and dead
    h.add(dev("d", "192.0.2.10", 4, "admin", 40)).unwrap(); // old but answers (switched off earlier, now back)
    h.add(dev("e", "192.0.2.11", 5, "admin", 2)).unwrap(); // dead but used two days ago: kept
    h.add(dev("f", "192.0.2.12", 1, "other", 40)).unwrap(); // same device, another account: not a duplicate
    assert_eq!(h.default_name(), "a");
    let asked = RefCell::new(Vec::new());
    let alive = |d: &Device| {
        asked.borrow_mut().push(d.name.clone());
        d.name == "d" || d.name == "f"
    };
    let plan = cli_devices::prune_plan(&h, NOW, &alive);
    assert_eq!(plan.names(), vec!["a".to_string(), "c".to_string()]);
    let mut asked = asked.borrow().clone();
    asked.sort();
    assert_eq!(asked, vec!["c", "d", "f"], "a duplicate, a recent entry and the kept one are not even probed");
    // without --yes it asks; "no" removes nothing
    let mut said = Vec::new();
    let mut q = Vec::new();
    cli_devices::prune(&h, false, NOW, &alive, &mut |s| { q.push(s.to_string()); false }, &mut |l| said.push(l.to_string())).unwrap();
    assert_eq!(h.devices().len(), 6);
    assert!(q[0].contains("Remove them?") && said.iter().any(|l| l.contains("a  duplicate of b")) && said.iter().any(|l| l.contains("c  does not answer")), "{said:?}");
    // --yes removes without asking, the default moves to the entry that stays
    let mut said = Vec::new();
    cli_devices::prune(&h, true, NOW, &alive, &mut |_| panic!("--yes does not ask"), &mut |l| said.push(l.to_string())).unwrap();
    let names: Vec<String> = h.devices().into_iter().map(|d| d.name).collect();
    assert_eq!(names, vec!["b", "d", "e", "f"]);
    assert_eq!(h.default_name(), "b");
    // nothing left to do
    let mut said = Vec::new();
    cli_devices::prune(&h, true, NOW, &alive, &mut |_| panic!(), &mut |l| said.push(l.to_string())).unwrap();
    assert!(said[0].starts_with("Nothing to remove"));
}

#[test]
fn rename_forget_and_a_fresh_pairing_that_replaces_the_old_addresses_of_the_same_device() {
    let h = home("rename");
    h.add(dev("a", "192.0.2.82", 1, "admin", 5)).unwrap();
    h.add(dev("b", "192.0.2.50", 2, "admin", 5)).unwrap();
    h.rename("a", "macbook").unwrap();
    assert_eq!(h.default_name(), "macbook", "the default follows the new name");
    assert!(h.rename("macbook", "b").unwrap_err().contains("already paired"));
    assert!(h.rename("nobody", "x").is_err() && h.rename("macbook", "bad name").is_err());
    h.remove("b").unwrap();
    assert_eq!(h.devices().len(), 1);
    // paired again at a new address under a new name: the old entry of the same device and account is replaced
    let fresh = dev("macbook-2", "192.0.2.107", 1, "admin", 0);
    h.add(fresh.clone()).unwrap();
    assert_eq!(h.replace_duplicates_of(&fresh).unwrap(), vec!["macbook".to_string()]);
    assert_eq!(h.devices().len(), 1);
    assert_eq!(h.default_name(), "macbook-2");
}

#[test]
fn the_device_answers_are_noted_but_the_file_is_not_rewritten_on_every_request() {
    let h = home("seen");
    let mut d = dev("a", "192.0.2.82", 1, "admin", 3);
    d.seen_at = 1;
    h.add(d).unwrap();
    h.mark_seen("a");
    let first = h.device(None).unwrap().seen_at;
    assert!(first > 1_700_000_000, "stamped with the real clock: {first}");
    h.mark_seen("a");
    assert_eq!(h.device(None).unwrap().seen_at, first, "within five minutes nothing is written again");
}

// ---------------------------------------------------------------------------------------------- the device's side

fn rig(tag: &str) -> (DevMode, std::path::PathBuf) {
    let d = std::env::temp_dir().join(format!("iyakki-pairing-dev-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("devmode")).unwrap();
    let dm = DevMode::open(&d);
    dm.enable(&Role::Admin.template(), "root", 0).unwrap();
    let keys: Vec<super::DevKey> = [("laptop", 7u8, 1_700_000_000), ("old-mac", 8, 1_600_000_000)]
        .into_iter()
        .map(|(n, k, t)| super::DevKey { name: n.into(), key: format!("{:0>64}", k), account: "root".into(), added: t, added_by: "root".into(), last_used: t + 5, scopes: vec!["install".into(), "shell".into(), "debug".into()] })
        .collect();
    std::fs::write(d.join("devmode/keys.json"), serde_json::to_string(&keys).unwrap()).unwrap();
    (dm, d)
}

#[test]
fn removing_a_pairing_says_what_it_takes_away_ends_its_windows_and_leaves_nothing_for_a_new_computer_of_the_same_name() {
    let (dm, d) = rig("remove");
    let admin = Role::Admin.template();
    let g = super::sessions::open(&d, "laptop", "root", super::scopes::Scope::Shell, "shell", dm.now() - 600).unwrap();
    dm.set_core_enabled(&admin, "root", true).unwrap();
    dm.set_core_scope(&admin, "root", "laptop", super::contract::GRANTABLE[0], true).unwrap();
    let p = dm.paired_computer("laptop").unwrap();
    assert_eq!(p.shell_windows(), 1);
    let q = p.removal_text();
    assert!(q.contains("install, debug, shell") || q.contains("shell"), "{q}");
    assert!(q.contains("1 open shell window") && q.contains("core contract scopes") && q.contains("pair again"), "{q}");
    assert_eq!(dm.pairings().iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["old-mac", "laptop"], "oldest first");
    // a user may not
    assert!(dm.remove_pairing(&Role::User.template(), "amy", "laptop").unwrap_err().contains("administrator"));
    assert!(dm.remove_pairing(&admin, "root", "ghost").is_err());
    let said = dm.remove_pairing(&admin, "root", "laptop").unwrap();
    assert!(said.contains("Removed laptop") && said.contains("1 open shell window"), "{said}");
    assert!(g.ended(), "its open shell window ended at once");
    assert!(dm.paired_computer("laptop").is_none() && dm.paired_computer("old-mac").is_some());
    assert!(dm.activity(10).iter().any(|a| a.action == "devmode.revoke"));
    // a new computer paired under the same name starts with no contract grant (the old one was dropped with the pairing)
    let k = super::DevKey { name: "laptop".into(), key: format!("{:0>64}", 9), account: "root".into(), added: 1, added_by: "root".into(), last_used: 0, scopes: vec!["install".into()] };
    let mut keys = dm.keys();
    keys.push(k);
    std::fs::write(d.join("devmode/keys.json"), serde_json::to_string(&keys).unwrap()).unwrap();
    assert!(dm.core_scopes("laptop").is_empty(), "{:?}", dm.core_scopes("laptop"));
    let _ = std::fs::remove_dir_all(d);
}
