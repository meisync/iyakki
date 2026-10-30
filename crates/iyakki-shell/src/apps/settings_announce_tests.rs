//! Settings → Network: the switch "Announce this device on the local network".

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};

struct Offline;
impl iyakki_agent::iyakki_proxy::Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_agent::iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}

#[test]
fn it_is_on_by_default_switches_off_and_on_with_a_reason_shown_and_is_audited() {
    let cx = admin("announce");
    assert_eq!(cx.services.announce_name_setting(), None);
    assert!(cx.services.announce_name_on(), "on by default on a desktop");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Network);
    assert!(a.announce_line(&cx).contains("ON") && a.announce_line(&cx).contains("iyakki-") && a.announce_line(&cx).contains("never tells who you are"));
    a.handle(&Event::Key(Key::Char('n')), &cx);
    assert!(!cx.services.announce_name_on() && cx.services.announce_name_setting() == Some(false));
    assert!(status(&a).contains("no longer announced"), "{}", status(&a));
    assert!(a.announce_line(&cx).contains("OFF"));
    a.handle(&Event::Key(Key::Char('n')), &cx);
    assert!(cx.services.announce_name_on());
    let s = status(&a);
    assert!(s.contains(".local") && s.contains("random name (not yours)") && s.contains("developer mode or the web service"), "{s}");
    // the name shown is the random one, not the host name or the account
    assert!(s.contains(&format!("{}.local", iyakki_agent::devname::label(cx.services.dir()))));
    assert!(!s.contains("root"), "no account name in it: {s}");
}

#[test]
fn an_ordinary_account_cannot_change_it() {
    let mut cx = admin("announce-user");
    cx.session.user = "amy".into();
    cx.session.policy = iyakki_sec::policy::Role::User.template();
    let mut a = SettingsApp::new(&cx);
    a.toggle_announce(&cx);
    assert_eq!(cx.services.announce_name_setting(), None);
}
