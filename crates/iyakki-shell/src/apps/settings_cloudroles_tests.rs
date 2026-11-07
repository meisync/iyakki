//! Settings -> Cloud access: hidden from non-administrators, the first owner, adding people, namespaces and namespace members through
//! the `Roles` change functions, their refusals shown, removal asking first, the audit trail, the page at four sizes and the lists
//! surviving a reopen (they are a file).

use super::*;
use crate::apps::tests::cx_with_admin_store;
use super::cloudroles_page::{items, Item};
use iyakki_agent::cloudroles::{self, NsRole, Roles, TenantRole};

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}
fn typ(a: &mut SettingsApp, cx: &AppCx, text: &str) {
    for c in text.chars() {
        key(a, cx, Key::Char(c));
    }
}
fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}
fn page(cx: &AppCx) -> SettingsApp {
    let mut a = SettingsApp::new(cx);
    a.go_to(Tab::CloudRoles);
    a
}
fn file(cx: &AppCx) -> std::path::PathBuf {
    cloudroles::path(cx.services.dir())
}
fn on_disk(cx: &AppCx) -> Roles {
    cloudroles::load(&file(cx)).unwrap().expect("the lists are saved")
}
fn trail(cx: &AppCx) -> String {
    std::fs::read_to_string(cloudroles::audit_path(cx.services.dir())).unwrap_or_default()
}
/// Roles on disk where the signed-in account (`root`) has `role`, plus owner `olga`.
fn seed(cx: &AppCx, role: TenantRole) {
    let mut r = Roles::new("olga").unwrap();
    r.add_tenant_member("olga", "root", role).unwrap();
    cloudroles::save(&file(cx), &r).unwrap();
}
fn texts(g: &Gfx) -> String {
    g.layout_log.texts.iter().map(|t| t.3.clone()).collect::<Vec<_>>().join("\n")
}
fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
    g
}

#[test]
fn the_page_is_for_administrators_and_a_forced_request_changes_nothing() {
    let cx = cx_with_admin_store("cr1");
    assert!(SettingsApp::new(&cx).tabs().contains(&Tab::CloudRoles));
    let mut ucx = cx_with_admin_store("cr1u");
    ucx.session.user = "amy".into();
    ucx.session.role = iyakki_sec::policy::Role::User;
    ucx.session.policy = iyakki_sec::policy::Role::User.template();
    let mut a = SettingsApp::new(&ucx);
    assert!(!a.tabs().contains(&Tab::CloudRoles));
    a.tabs.push(Tab::CloudRoles);
    a.tab = a.tabs.len() - 1;
    key(&mut a, &ucx, Key::Char('o'));
    key(&mut a, &ucx, Key::Char('m'));
    assert!(!file(&ucx).exists(), "nothing was written");
    assert!(!a.has_modal());
    assert!(ucx.audit.recent(10).iter().any(|l| l.contains("settings.denied")));
}

#[test]
fn a_machine_without_roles_offers_make_me_the_owner_once() {
    let cx = cx_with_admin_store("cr2");
    let mut a = page(&cx);
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(texts(&g).contains("No roles on this machine yet"));
    assert!(g.hits.iter().any(|h| h.label == "Make me the owner"));
    key(&mut a, &cx, Key::Char('m'));
    assert!(status(&a).contains("no roles yet") && !a.has_modal(), "nothing can be added before the owner: {}", status(&a));
    key(&mut a, &cx, Key::Char('o'));
    let r = on_disk(&cx);
    assert_eq!(r.tenant_role("root"), Some(TenantRole::Owner));
    assert!(trail(&cx).contains("first owner"), "{}", trail(&cx));
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("settings.cloud_roles") && l.contains("first owner")));
    // once: the button is gone and the key refuses
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(!g.hits.iter().any(|h| h.label == "Make me the owner"));
    assert!(g.hits.iter().any(|h| h.label == "Add person"));
    key(&mut a, &cx, Key::Char('o'));
    assert!(status(&a).contains("already"), "{}", status(&a));
    assert_eq!(on_disk(&cx).tenant.len(), 1);
}

#[test]
fn people_namespaces_and_namespace_members_are_added_through_the_model_and_logged() {
    let cx = cx_with_admin_store("cr3");
    let mut a = page(&cx);
    key(&mut a, &cx, Key::Char('o'));
    // add a person with a role
    key(&mut a, &cx, Key::Char('m'));
    assert!(a.has_modal(), "the name is asked for");
    typ(&mut a, &cx, "alice admin");
    key(&mut a, &cx, Key::Enter);
    assert!(!a.has_modal());
    assert_eq!(on_disk(&cx).tenant_role("alice"), Some(TenantRole::Admin), "{}", status(&a));
    key(&mut a, &cx, Key::Char('m'));
    typ(&mut a, &cx, "bob");
    key(&mut a, &cx, Key::Enter);
    assert_eq!(on_disk(&cx).tenant_role("bob"), Some(TenantRole::Member), "the role defaults to member");
    // a namespace: the maker administers it
    key(&mut a, &cx, Key::Char('n'));
    typ(&mut a, &cx, "web");
    key(&mut a, &cx, Key::Enter);
    assert_eq!(on_disk(&cx).ns_role("web", "root"), Some(NsRole::Admin), "{}", status(&a));
    // choose the namespace row and add a member to it
    let list = items(&on_disk(&cx), "root");
    let at = list.iter().position(|i| matches!(i, Item::Ns(n, _) if n == "web")).unwrap();
    a.sel[a.tab] = at;
    key(&mut a, &cx, Key::Char('a'));
    typ(&mut a, &cx, "bob viewer");
    key(&mut a, &cx, Key::Enter);
    assert_eq!(on_disk(&cx).ns_role("web", "bob"), Some(NsRole::Viewer), "{}", status(&a));
    // a namespace member who is not in the tenant is refused with the model's words
    key(&mut a, &cx, Key::Char('a'));
    typ(&mut a, &cx, "zed");
    key(&mut a, &cx, Key::Enter);
    assert!(status(&a).contains("not a member of the tenant"), "{}", status(&a));
    assert_eq!(on_disk(&cx).ns_role("web", "zed"), None);
    // the trail and the app audit log have a line for every change
    let t = trail(&cx);
    for want in ["first owner", "made alice", "made bob", "namespace web: made by root", "made bob Viewer"] {
        assert!(t.contains(want), "{want} in {t}");
    }
    assert!(t.lines().all(|l| l.split('\t').next().unwrap().parse::<u64>().is_ok()), "each line starts with a time: {t}");
    let log = cx.audit.recent(40);
    assert!(log.iter().filter(|l| l.contains("settings.cloud_roles ") || l.contains("settings.cloud_roles\t") || l.contains("settings.cloud_roles:")).count() >= 4, "{log:?}");
    assert!(log.iter().any(|l| l.contains("settings.cloud_roles.refused") && l.contains("not a member of the tenant")), "{log:?}");
    // a bad role word keeps nothing
    key(&mut a, &cx, Key::Char('m'));
    typ(&mut a, &cx, "carl boss");
    key(&mut a, &cx, Key::Enter);
    assert!(status(&a).contains("not a role"), "{}", status(&a));
    assert_eq!(on_disk(&cx).tenant_role("carl"), None);
    // Esc closes the question
    key(&mut a, &cx, Key::Char('m'));
    key(&mut a, &cx, Key::Esc);
    assert!(!a.has_modal());
}

#[test]
fn what_the_model_refuses_is_shown_and_nothing_changes() {
    let cx = cx_with_admin_store("cr4");
    seed(&cx, TenantRole::Member);
    let mut a = page(&cx);
    let before = on_disk(&cx);
    key(&mut a, &cx, Key::Char('m'));
    typ(&mut a, &cx, "eve");
    key(&mut a, &cx, Key::Enter);
    assert!(status(&a).contains("Refused") && status(&a).contains("RoleTooLow"), "a member cannot add: {}", status(&a));
    key(&mut a, &cx, Key::Char('n'));
    typ(&mut a, &cx, "secret");
    key(&mut a, &cx, Key::Enter);
    assert!(status(&a).contains("Refused"), "{}", status(&a));
    assert_eq!(on_disk(&cx), before);
    assert!(trail(&cx).is_empty(), "a refusal is not a change");
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("settings.cloud_roles.refused")));
    // a plain member sees their own role, not the member list
    let g = draw(&mut a, &cx, 1280, 720);
    let t = texts(&g);
    assert!(t.contains("root: member of this tenant") && !t.contains("olga"), "{t}");
    // an administrator cannot make an owner
    seed(&cx, TenantRole::Admin);
    let mut b = page(&cx);
    key(&mut b, &cx, Key::Char('m'));
    typ(&mut b, &cx, "eve owner");
    key(&mut b, &cx, Key::Enter);
    assert!(status(&b).contains("only an owner makes an owner"), "{}", status(&b));
    // a person outside the tenant sees no access and nothing else
    let mut r = Roles::new("olga").unwrap();
    r.create_namespace("olga", "hidden").unwrap();
    cloudroles::save(&file(&cx), &r).unwrap();
    let mut c = page(&cx);
    let g = draw(&mut c, &cx, 1280, 720);
    let t = texts(&g);
    assert!(t.contains("not a member of this tenant") && !t.contains("hidden") && !t.contains("olga"), "{t}");
}

#[test]
fn removal_asks_first_says_what_happens_and_the_model_keeps_the_last_owner() {
    let cx = cx_with_admin_store("cr5");
    let mut r = Roles::new("root").unwrap();
    r.add_tenant_member("root", "bob", TenantRole::Member).unwrap();
    r.create_namespace("root", "web").unwrap();
    r.add_namespace_member("root", "web", "bob", NsRole::Editor).unwrap();
    cloudroles::save(&file(&cx), &r).unwrap();
    let mut a = page(&cx);
    let list = items(&r, "root");
    let bob = list.iter().position(|i| *i == Item::Tenant("bob".into(), TenantRole::Member)).unwrap();
    a.sel[a.tab] = bob;
    key(&mut a, &cx, Key::Char('x'));
    assert!(a.has_modal(), "asked first");
    assert_eq!(on_disk(&cx).tenant_role("bob"), Some(TenantRole::Member), "not removed yet");
    let g = draw(&mut a, &cx, 1280, 720);
    let t = texts(&g);
    assert!(t.contains("leave every namespace") || t.contains("They leave every namespace"), "{t}");
    assert!(t.contains("to be rotated"), "{t}");
    assert!(!t.contains("were rotated") && !t.contains("has been rotated"), "the page does not claim a rotation: {t}");
    assert!(g.hits.iter().any(|h| h.label == "Yes, remove") && g.hits.iter().any(|h| h.label == "Cancel"));
    // another key cancels
    key(&mut a, &cx, Key::Char('n'));
    assert!(!a.has_modal());
    assert_eq!(on_disk(&cx).tenant_role("bob"), Some(TenantRole::Member));
    // y removes the person from the tenant and every namespace
    key(&mut a, &cx, Key::Char('x'));
    key(&mut a, &cx, Key::Char('y'));
    let r = on_disk(&cx);
    assert_eq!(r.tenant_role("bob"), None);
    assert_eq!(r.ns_role("web", "bob"), None);
    assert!(trail(&cx).contains("removed bob"), "{}", trail(&cx));
    // the last owner: the model's message is shown
    let me = items(&r, "root").iter().position(|i| *i == Item::Tenant("root".into(), TenantRole::Owner)).unwrap();
    a.sel[a.tab] = me;
    key(&mut a, &cx, Key::Char('x'));
    key(&mut a, &cx, Key::Char('y'));
    assert!(status(&a).contains("the last owner cannot be removed"), "{}", status(&a));
    assert_eq!(on_disk(&cx).tenant_role("root"), Some(TenantRole::Owner));
    // the last administrator of a namespace
    let list = items(&on_disk(&cx), "root");
    let m = list.iter().position(|i| matches!(i, Item::NsMember(_, w, _) if w == "root")).unwrap();
    a.sel[a.tab] = m;
    key(&mut a, &cx, Key::Char('x'));
    key(&mut a, &cx, Key::Char('y'));
    assert!(status(&a).contains("last administrator of a namespace"), "{}", status(&a));
    // deleting a namespace asks too
    let list = items(&on_disk(&cx), "root");
    a.sel[a.tab] = list.iter().position(|i| matches!(i, Item::Ns(n, _) if n == "web")).unwrap();
    key(&mut a, &cx, Key::Char('d'));
    assert!(a.has_modal() && on_disk(&cx).namespaces.contains_key("web"));
    key(&mut a, &cx, Key::Char('y'));
    assert!(!on_disk(&cx).namespaces.contains_key("web"), "{}", status(&a));
}

#[test]
fn an_unjoined_namespace_shows_its_name_only_and_a_tenant_owner_can_join_it_on_the_record() {
    let cx = cx_with_admin_store("cr6");
    let mut r = Roles::new("root").unwrap();
    r.add_tenant_member("root", "mia", TenantRole::Member).unwrap();
    r.create_namespace("root", "mine").unwrap();
    r.add_namespace_member("root", "mine", "mia", NsRole::Viewer).unwrap();
    r.namespaces.entry("theirs".into()).or_default().insert("mia".into(), NsRole::Admin);
    cloudroles::save(&file(&cx), &r).unwrap();
    let mut a = page(&cx);
    let g = draw(&mut a, &cx, 1280, 720);
    let t = texts(&g);
    assert!(t.contains("theirs") && t.contains("the name only"), "{t}");
    assert!(t.contains("    mia"), "members of a joined namespace are listed: {t}");
    let list = items(&r, "root");
    assert!(!list.iter().any(|i| matches!(i, Item::NsMember(n, ..) if n == "theirs")), "no members of a namespace this account is not in");
    a.sel[a.tab] = list.iter().position(|i| matches!(i, Item::Ns(n, None) if n == "theirs")).unwrap();
    key(&mut a, &cx, Key::Char('a'));
    typ(&mut a, &cx, "root");
    key(&mut a, &cx, Key::Enter);
    assert_eq!(on_disk(&cx).ns_role("theirs", "root"), Some(NsRole::Editor), "{}", status(&a));
    assert!(trail(&cx).contains("JOINED"), "the join is recorded: {}", trail(&cx));
    // someone else cannot be added by a tenant owner who is not in it
    let mut b = page(&cx);
    let mut r = on_disk(&cx);
    r.namespaces.get_mut("theirs").unwrap().remove("root");
    cloudroles::save(&file(&cx), &r).unwrap();
    let list = items(&r, "root");
    b.sel[b.tab] = list.iter().position(|i| matches!(i, Item::Ns(n, None) if n == "theirs")).unwrap();
    key(&mut b, &cx, Key::Char('a'));
    typ(&mut b, &cx, "mia viewer");
    key(&mut b, &cx, Key::Enter);
    assert!(status(&b).contains("only the namespace's administrators"), "{}", status(&b));
}

#[test]
fn the_lists_survive_reopening_and_the_mouse_works() {
    let cx = cx_with_admin_store("cr7");
    let mut a = page(&cx);
    key(&mut a, &cx, Key::Char('o'));
    for n in ["alpha", "beta"] {
        key(&mut a, &cx, Key::Char('n'));
        typ(&mut a, &cx, n);
        key(&mut a, &cx, Key::Enter);
    }
    drop(a);
    let mut b = page(&cx);
    let g = draw(&mut b, &cx, 1280, 720);
    let t = texts(&g);
    assert!(t.contains("root: owner of this tenant") && t.contains("alpha") && t.contains("beta"), "{t}");
    assert!(t.contains("Audit trail") && t.contains("namespace beta: made by root"), "{t}");
    // a click on a row chooses it; the buttons are clickable and send their key
    let row = g.hits.iter().find(|h| h.label == "beta").expect("rows are clickable");
    let ev = row.ev.clone();
    b.handle(&ev, &cx);
    assert!(matches!(items(&on_disk(&cx), "root").get(b.sel()), Some(Item::Ns(n, _)) if n == "beta"), "{:?}", b.sel());
    let add = g.hits.iter().find(|h| h.label == "New namespace").expect("button");
    b.handle(&add.ev.clone(), &cx);
    assert!(b.has_modal());
    typ(&mut b, &cx, "gamma");
    let g = draw(&mut b, &cx, 1280, 720);
    let ok = g.hits.iter().find(|h| h.label == "Add").expect("the Add button while typing");
    b.handle(&ok.ev.clone(), &cx);
    assert!(on_disk(&cx).namespaces.contains_key("gamma"), "{}", status(&b));
    // paste goes into the field
    key(&mut b, &cx, Key::Char('n'));
    b.handle(&Event::Paste("delta".into()), &cx);
    key(&mut b, &cx, Key::Enter);
    assert!(on_disk(&cx).namespaces.contains_key("delta"));
}

#[test]
fn the_page_draws_at_four_sizes_without_text_leaving_the_screen() {
    let cx = cx_with_admin_store("cr8");
    let mut r = Roles::new("root").unwrap();
    for i in 0..12 {
        r.add_tenant_member("root", &format!("person-with-a-rather-long-name-{i}"), TenantRole::Member).unwrap();
    }
    for i in 0..4 {
        r.create_namespace("root", &format!("namespace-number-{i}-with-a-long-name-for-the-width-check")).unwrap();
    }
    cloudroles::save(&file(&cx), &r).unwrap();
    for i in 0..8 {
        cloudroles::append_audit(&cloudroles::audit_path(cx.services.dir()), 100 + i, &cloudroles::Change(format!("tenant: root did a change number {i} with some words to make the line long enough to need cutting at narrow widths"))).unwrap();
    }
    let mut a = page(&cx);
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        for step in 0..3 {
            match step {
                1 => key(&mut a, &cx, Key::End),
                2 => {
                    key(&mut a, &cx, Key::Home);
                    key(&mut a, &cx, Key::Char('x'));
                }
                _ => {}
            }
            let g = draw(&mut a, &cx, w, h);
            for (_, rect, clip, text) in &g.layout_log.texts {
                let right = clip.map_or(rect.x + rect.w, |c| (rect.x + rect.w).min(c.x + c.w));
                let bottom = clip.map_or(rect.y + rect.h, |c| (rect.y + rect.h).min(c.y + c.h));
                assert!(rect.x >= 0.0 && right <= w as f32 + 0.5 && bottom <= h as f32 + 0.5, "{w}x{h} step {step}: {text:?} at {rect:?} leaves the screen");
            }
            assert!(g.hits.iter().any(|x| matches!(x.ev, Event::Row(_))), "{w}x{h} step {step}: rows are there");
            if step == 0 {
                assert!(texts(&g).contains("Shared vault: built-in (not externally reviewed)"), "{w}x{h}: the provider and its label are shown");
            }
            for x in &g.hits {
                let r = x.rect;
                assert!(r.w > 0.0 && r.h > 0.0 && r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: {} {r:?} outside the screen", x.label);
            }
            if step == 2 {
                assert!(a.has_modal() && g.hits.iter().any(|x| x.label == "Yes, remove"), "{w}x{h}: the question has its buttons");
                key(&mut a, &cx, Key::Char('n'));
            }
        }
        // the chosen row stays in view when the cursor goes to the end
        key(&mut a, &cx, Key::End);
        let g = draw(&mut a, &cx, w, h);
        assert!(g.hits.iter().any(|x| x.ev == Event::Row(a.sel())), "{w}x{h}: the chosen row is drawn");
        key(&mut a, &cx, Key::Home);
    }
}

#[test]
fn the_page_names_an_external_provider_and_a_setting_that_cannot_be_used() {
    let cx = cx_with_admin_store("cr9");
    seed(&cx, TenantRole::Owner);
    let mut a = page(&cx);
    let dir = cx.services.dir().to_path_buf();
    iyakki_agent::sharedvault::Setting { v: 1, provider: iyakki_agent::sharedvault::Provider::External, external: Some(iyakki_agent::sharedvault::ExternalRef { address: "https://kms.example.invalid".into(), mount: "secret".into(), path: "iyakki/acme".into(), namespace: None, token_secret: "shared-vault-token".into() }) }.save(&dir, "root").unwrap();
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let t = texts(&draw(&mut a, &cx, w, h));
        assert!(t.contains("Shared vault: external system") && t.contains("not externally reviewed"), "{w}x{h}: {t}");
        assert!(!t.contains("kms.example.invalid") && !t.contains("shared-vault-token"), "{w}x{h}: no address or token name on the page");
    }
    std::fs::write(iyakki_agent::sharedvault::Setting::path(&dir), "{ damaged").unwrap();
    let t = texts(&draw(&mut a, &cx, 800, 480));
    assert!(t.contains("setting unusable") && t.contains("not externally reviewed"), "{t}");
}
