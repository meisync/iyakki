// #[cfg(test)] -- test code, included by store_setup.rs under #[cfg(test)].
//! Store -> Setup and the data science toolkit: the popup says where the toolkit stands, and Verify and Upgrade appear only when they make
//! sense, for an administrator. Fakes only: the self-test is refused for want of the Linux apps sandbox (nothing runs, nothing is fetched).
use super::*;
use crate::apps::store_port::CxPort;
use crate::apps::store::{Section, StoreApp};
use crate::apps::{App, AppCx};
use iyakki_sec::policy::Role;

fn rig(tag: &str, role: Role, installed: bool) -> (StoreApp, AppCx) {
    let cx = super::super::tests::cx(tag, role);
    let mut a = StoreApp::new(&cx);
    let mut f = Facts::default();
    for it in Item::ALL {
        f.presence.push((it, if installed && it == Item::DataScience { Presence::Installed("1 files".into()) } else { Presence::Missing }));
    }
    a.setup.set_machine(Machine { ram_mb: 16_000, ..Default::default() });
    a.setup.set_facts(f);
    let data = cx.data_dir().join("qi");
    a.setup.launch = Some(qi::Launch { exe: "/usr/bin/true".into(), data: data.clone() });
    if installed {
        let site = iyakki_shared::datasci::site_dir(&data);
        std::fs::create_dir_all(&site).unwrap();
        std::fs::create_dir_all(iyakki_shared::datasci::python_bin(&data).parent().unwrap()).unwrap();
        std::fs::write(iyakki_shared::datasci::python_bin(&data), "#!fake").unwrap();
        std::fs::write(iyakki_shared::datasci::dir(&data).join("state.json"), r#"{"files":["numpy 1.0 abc"],"selftest":"ok earlier"}"#).unwrap();
    }
    a.handle(&Event::Tab(4), &cx);
    assert!(a.section == Section::Setup);
    a.setup.enter(&CxPort(&cx));
    (a, cx)
}

fn idx() -> usize {
    Item::ALL.iter().position(|x| *x == Item::DataScience).unwrap()
}

fn labels(a: &StoreApp) -> Vec<String> {
    a.setup.popup_button_list(Item::DataScience).into_iter().filter(|(_, on)| *on).map(|(l, _)| l).collect()
}

#[test]
fn verify_and_upgrade_are_offered_to_an_administrator_for_an_installed_toolkit_that_differs_from_the_pins() {
    let (mut a, cx) = rig("ds-buttons", Role::Admin, true);
    a.setup.open_popup(Item::DataScience);
    a.setup.refresh_toolkit(&CxPort(&cx));
    // the record has one file the table does not pin: behind this IYAKKI
    assert_eq!(a.setup.ds_state.as_ref().unwrap().0, "upgrade");
    let l = labels(&a);
    assert!(l.iter().any(|x| x.starts_with("Verify")) && l.iter().any(|x| x.starts_with("Upgrade")), "{l:?}");
    // a standard user gets neither, and is told in words when it tries
    let (mut u, cxu) = rig("ds-user", Role::User, true);
    u.setup.open_popup(Item::DataScience);
    u.setup.refresh_toolkit(&CxPort(&cxu));
    assert!(!labels(&u).iter().any(|x| x.starts_with("Verify") || x.starts_with("Upgrade")));
    u.setup.verify_toolkit(&CxPort(&cxu));
    assert!(u.setup.note.as_ref().is_some_and(|n| !n.1 && n.0.contains("administrator")));
    // not installed: neither
    let (mut m, cxm) = rig("ds-missing", Role::Admin, false);
    m.setup.open_popup(Item::DataScience);
    m.setup.refresh_toolkit(&CxPort(&cxm));
    assert_eq!(m.setup.ds_state.as_ref().unwrap().0, "none");
    assert!(!labels(&m).iter().any(|x| x.starts_with("Verify") || x.starts_with("Upgrade")));
}

#[test]
fn verify_without_the_sandbox_says_why_and_is_audited_and_upgrade_needs_the_state_to_say_so() {
    let (mut a, cx) = rig("ds-verify", Role::Admin, true);
    a.setup.open_popup(Item::DataScience);
    a.setup.refresh_toolkit(&CxPort(&cx));
    a.handle(&Event::Tab(VERIFY + idx()), &cx);
    let note = a.setup.note.clone().unwrap();
    assert!(!note.1 && note.0.contains("Linux apps"), "{note:?}");
    let log = std::fs::read_to_string(cx.audit.path_buf()).unwrap_or_default();
    assert!(log.contains("quickinstall.datasci.verify"));
    // upgrade only when the record says the pins differ
    a.setup.ds_state = Some(("current".into(), String::new()));
    a.handle(&Event::Tab(UPGRADE + idx()), &cx);
    assert!(a.setup.note.as_ref().is_some_and(|n| !n.1 && n.0.contains("nothing to upgrade")));
}
