//! Settings -> Remote access -> Machines for flows: add, replace and forget a machine from the screen, the same file and the same checks as the
//! assistant's tools, the audit lines, and that a token never lands in the file.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};

struct Off;
impl iyakki_proxy::Upstream for Off {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("no network in tests".into())
    }
}

fn page(tag: &str) -> (AppCx, SettingsApp) {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Off));
    cx.services.on_login("root", "rootpass12", true);
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Remote);
    (cx, a)
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}
fn type_in(a: &mut SettingsApp, cx: &AppCx, s: &str) {
    (0..90).for_each(|_| key(a, cx, Key::Backspace));
    for c in s.chars() {
        key(a, cx, Key::Char(c));
    }
}
fn say(a: &SettingsApp) -> String {
    a.status().map(|(t, _)| t.clone()).unwrap_or_default()
}
fn add(a: &mut SettingsApp, cx: &AppCx, name: &str, url: &str, vault: &str) {
    key(a, cx, Key::Char('M'));
    assert!(a.has_modal(), "M opens the form");
    type_in(a, cx, name);
    key(a, cx, Key::Tab);
    type_in(a, cx, url);
    key(a, cx, Key::Tab);
    type_in(a, cx, vault);
    key(a, cx, Key::Enter);
}
fn lines(a: &SettingsApp, cx: &AppCx) -> String {
    let mut l = Vec::new();
    a.machine_lines(cx, &mut l);
    l.iter().map(|x| x.0.clone()).collect::<Vec<_>>().join("\n")
}

#[test]
fn a_machine_is_added_listed_replaced_and_forgotten_from_the_screen_and_the_token_is_only_ever_a_vault_name() {
    let (cx, mut a) = page("mach-flow");
    assert!(lines(&a, &cx).contains("No machine is saved"));
    add(&mut a, &cx, "office", "https://office.example:8443", "office-token");
    assert!(say(&a).contains("Saved the machine office at https://office.example:8443"), "{}", say(&a));
    let text = lines(&a, &cx);
    assert!(text.contains("office  https://office.example:8443") && text.contains("vault entry office-token"), "{text}");
    // the very file the assistant's tools and the flow step read
    let m = iyakki_agent::flows::machines::Machines::new(&cx.data_dir().join("flows").join("root"));
    assert_eq!(m.get("office").unwrap().token, "office-token");
    // replacing keeps one entry; a second machine sorts after
    add(&mut a, &cx, "office", "https://office.example:9443", "office-token2");
    add(&mut a, &cx, "home", "https://home.example", "home-token");
    let names: Vec<String> = m.list().into_iter().map(|x| x.name).collect();
    assert_eq!(names, ["home", "office"]);
    assert_eq!(m.get("office").unwrap().url, "https://office.example:9443");
    let audit = std::fs::read_to_string(cx.audit.path()).unwrap_or_default();
    assert!(audit.contains("flow.remote_machine") && audit.contains("saved office https://office.example:8443"), "{audit}");
    assert!(!std::fs::read_to_string(cx.data_dir().join("flows").join("root").join("remote-machines.json")).unwrap().contains("rootpass12"));
    // forget
    key(&mut a, &cx, Key::Char('X'));
    type_in(&mut a, &cx, "home");
    key(&mut a, &cx, Key::Enter);
    assert!(say(&a).contains("Forgot the machine home"), "{}", say(&a));
    assert_eq!(m.list().len(), 1);
    key(&mut a, &cx, Key::Char('X'));
    type_in(&mut a, &cx, "ghost");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal() || say(&a).contains("No machine has that name"), "{}", say(&a));
}

#[test]
fn bad_names_addresses_and_a_pasted_token_are_refused_and_nothing_is_saved() {
    let (cx, mut a) = page("mach-bad");
    for (name, url, vault) in [("Bad Name", "https://a.example", "tok"), ("ok", "http://a.example", "tok"), ("ok", "https://a.example/path?x=1", "tok"), ("ok", "https://user@a.example", "tok"), ("ok", "https://a.example", "has space or $ecret!"), ("ok", "https://a.example", "")] {
        add(&mut a, &cx, name, url, vault);
        assert!(a.has_modal(), "{name} {url} {vault:?} stays on the form with its message");
        key(&mut a, &cx, Key::Esc);
    }
    let m = iyakki_agent::flows::machines::Machines::new(&cx.data_dir().join("flows").join("root"));
    assert!(m.list().is_empty(), "nothing was saved");
    assert!(lines(&a, &cx).contains("never the token"));
}
