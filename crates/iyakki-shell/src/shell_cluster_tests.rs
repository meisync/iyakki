//! The dev cluster at the shell level: a cluster that goes down or a disk that fills up becomes a warning in the administrator's INBOX
//! (once), ordinary accounts are not told, and sign-in/out hands the vault resolver to the cluster tools and takes it away again.

use super::tests::*;
use super::*;
use iyakki_agent::k3sdev::{Machine, Svc};
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

fn install_failing(sh: &Shell) -> Arc<iyakki_agent::k3sdev::K3sDev> {
    let k = sh.services_for_demo().k3sdev.clone();
    let l = k.layout();
    std::fs::create_dir_all(l.root.join("bin")).unwrap();
    std::fs::write(l.bin(), "#!/bin/sh\necho 'boom: out of memory'\nexit 9\n").unwrap();
    std::fs::set_permissions(l.bin(), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(l.state(), "{\"backend\":\"native\",\"version\":\"v1.31.1+k3s1\",\"autostart\":true}").unwrap();
    let m = Machine { k3s_yaml: l.root.join("nope/k3s.yaml"), lib_dir: l.root.join("nope/lib"), bins: vec![], units: vec![], proc_dir: l.root.join("nope/proc") };
    k.assume(true, true, false, false, m, Duration::from_millis(5), Duration::from_millis(10));
    k.assume_connected(None);
    k
}

#[test]
fn a_cluster_that_goes_down_and_a_full_disk_are_warnings_in_the_administrators_inbox_once_and_users_are_not_told() {
    let (mut sh, _) = shell_with_users("clusteralerts");
    login(&mut sh, "root", "rootpass12");
    let k = install_failing(&sh);
    // the supervisor's audit sink is the shell's audit log from sign-in on
    k.start("root").unwrap();
    for _ in 0..400 {
        if matches!(k.state(), Svc::Failed(_)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(matches!(k.state(), Svc::Failed(_)), "{:?}", k.state());
    for _ in 0..65 {
        sh.handle(&Event::Tick);
    }
    let notes = sh.services_for_demo().inbox.list("root");
    assert!(notes.iter().any(|n| n.title == "The dev cluster keeps failing and was stopped" && n.body.contains("stopped trying")), "the give-up is announced too");
    let down: Vec<_> = notes.iter().filter(|n| n.title == "The dev cluster stopped and was restarted").collect();
    assert_eq!(down.len(), 1, "{:?}", notes.iter().map(|n| n.title.clone()).collect::<Vec<_>>());
    assert!(down[0].body.contains("boom: out of memory") && down[0].kind == iyakki_agent::inbox::Kind::Warning, "{:?}", down[0]);
    assert!(sh.toast_text().is_some_and(|t| t.contains("The dev cluster")), "{:?}", sh.toast_text());
    // taken: not again at the next look
    for _ in 0..65 {
        sh.handle(&Event::Tick);
    }
    assert_eq!(sh.services_for_demo().inbox.list("root").iter().filter(|n| n.title == "The dev cluster stopped and was restarted").count(), 1);
    // a nearly full disk
    k.refresh_health();
    k.check_disk_for_test(91);
    for _ in 0..65 {
        sh.handle(&Event::Tick);
    }
    let full: Vec<_> = sh.services_for_demo().inbox.list("root").into_iter().filter(|n| n.title.contains("91% full")).collect();
    assert_eq!(full.len(), 1);
    assert!(full[0].body.contains("85%"));
    // an ordinary account is not told: the alert waits for an administrator
    key(&mut sh, Key::Char('o'));
    login(&mut sh, "alice", "alicepass1");
    k.check_disk_for_test(0);
    k.check_disk_for_test(95);
    for _ in 0..65 {
        sh.handle(&Event::Tick);
    }
    assert!(sh.services_for_demo().inbox.list("alice").is_empty());
}
