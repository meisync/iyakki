//! Tests of the STORE tile's management layer: updating, rolling back, pinning and removing from the Installed page, and the
//! person's own arrangement (tags, groups, titles, drag and drop). Drawn and clicked at the four screen sizes.

use super::*;
use iyakki_agent::pkg::{new_signing_key, public_key_hex, SigningKey};
use iyakki_agent::store::catalog::{sha256_hex, Catalog, Entry};
use iyakki_agent::store::manifest::{Meta, Pricing, Publisher};
use iyakki_agent::store::organize::Org;
use iyakki_sec::policy::Role;
use crate::ui::Nav;

fn meta(title: &str, perms: &[&str]) -> Meta {
    Meta { title: String::new(), summary: format!("{title} for the tests"), publisher: Publisher { id: "acme".into(), name: "Acme Ltd".into(), url: String::new() }, pricing: Pricing::default(), permissions: perms.iter().map(|s| s.to_string()).collect(), categories: vec!["work".into()], ..Default::default() }
}

fn signed_tool(name: &str, version: &str, caps: &[&str], k: &SigningKey, m: &Meta) -> String {
    let manifest = serde_json::json!({"name": name, "version": version, "license": "MIT", "description": "test tool", "schema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}, "capabilities": caps}).to_string();
    let t = iyakki_agent::pkg::build_tool(&manifest, include_str!("../../../../../../tests/fixtures/tool_echo.wat").as_bytes(), Some(k)).unwrap();
    iyakki_agent::pkg::resign(&t, Some(&m.to_json()), None, k).unwrap()
}

struct Pkg {
    kind: &'static str,
    name: &'static str,
    version: &'static str,
    text: String,
    meta: Meta,
}

/// Write (or rewrite) a folder catalog holding these packages.
fn publish(d: &std::path::Path, serial: u64, k: &SigningKey, pkgs: &[Pkg]) {
    std::fs::create_dir_all(d.join("packages")).unwrap();
    let mut entries = vec![];
    for p in pkgs {
        let file = format!("packages/{}-{}.iyp", p.name, p.version);
        std::fs::write(d.join(&file), &p.text).unwrap();
        entries.push(Entry { id: p.name.into(), kind: p.kind.into(), name: p.name.into(), version: p.version.into(), license: "MIT".into(), size: p.text.len() as u64, sha256: sha256_hex(p.text.as_bytes()), url: file, signer: public_key_hex(k), added: 1_800_000_000, meta: p.meta.clone(), ..Default::default() });
    }
    let c = Catalog { v: 1, id: "usb-stick".into(), name: "Test stick".into(), serial, entries, ..Default::default() };
    std::fs::write(d.join("catalog.json"), c.to_json()).unwrap();
}

fn skill(name: &'static str, version: &'static str, k: &SigningKey) -> Pkg {
    let m = meta(name, &[]);
    Pkg { kind: "skill", name, version, text: signed_skill(name, version, k, &m), meta: m }
}

fn tool(name: &'static str, version: &'static str, caps: &[&str], k: &SigningKey) -> Pkg {
    let m = meta(name, caps);
    Pkg { kind: "tool", name, version, text: signed_tool(name, version, caps, k, &m), meta: m }
}

/// Install a package for everyone the way the tile does after a review.
fn install(cx: &AppCx, p: &Pkg) {
    let st = Store::for_services(&cx.services);
    let a = Actor::from_policy(&cx.session.user, &cx.session.policy);
    let rv = st.review_sideload(&cx.services, &a, p.text.clone().into_bytes(), iyakki_agent::skills::Scope::Shared, &[]);
    st.install(&cx.services, &a, &rv, Confirm::all()).unwrap_or_else(|e| panic!("{e}: {}", rv.summary()));
}

struct NoFetch;
impl Fetch for NoFetch {
    fn get(&self, url: &str, _max: usize) -> Result<Vec<u8>, String> {
        Err(format!("offline: {url}"))
    }
}

/// Install the catalog's version of `name` for everyone, as the tile does after the review (so the store remembers where it came from).
fn install_cat(a: &mut StoreApp, cx: &AppCx, name: &str) {
    a.reload(cx);
    let item = a.all_items.iter().find(|i| i.entry.name == name).cloned().unwrap_or_else(|| panic!("{name} is in the catalog"));
    let actor = Actor::from_policy(&cx.session.user, &cx.session.policy);
    let r = a.store.review_item(&cx.services, &actor, &item, &a.loaded, &NoFetch, iyakki_agent::skills::Scope::Shared, &[]).unwrap();
    a.store.install(&cx.services, &actor, &r, Confirm::all()).unwrap_or_else(|e| panic!("{e}: {}", r.summary()));
    a.reload(cx);
}

fn world(tag: &str) -> (AppCx, SigningKey, std::path::PathBuf) {
    let cx = cx(tag, Role::Admin);
    let k = new_signing_key().unwrap();
    cx.services.packages.trust_key(&public_key_hex(&k), "Acme").unwrap();
    let d = crate::apps::tests::temp_dir(&format!("{tag}-stick")).join("iyakki-store");
    (cx, k, d)
}

#[allow(dead_code)]
fn center(g: &Gfx, ev: &Event) -> (i32, i32) {
    let h = g.hits.iter().rev().find(|h| h.ev == *ev).unwrap_or_else(|| panic!("no region for {ev:?}"));
    ((h.rect.x + h.rect.w / 2.0) as i32, (h.rect.y + h.rect.h / 2.0) as i32)
}

#[allow(dead_code)]
fn label_center(g: &Gfx, label: &str) -> (i32, i32) {
    let h = g.hits.iter().rev().find(|h| h.label == label).unwrap_or_else(|| panic!("no region labelled {label}: {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>()));
    ((h.rect.x + h.rect.w / 2.0) as i32, (h.rect.y + h.rect.h / 2.0) as i32)
}

fn has_label(g: &Gfx, label: &str) -> bool {
    g.hits.iter().any(|h| h.label == label)
}

fn select(a: &mut StoreApp, name: &str) {
    a.on_head = None;
    a.sel[1] = a.ifiltered().iter().position(|e| e.name == name).unwrap_or_else(|| panic!("{name} is listed"));
}

fn version_of(cx: &AppCx, kind: &str, name: &str) -> String {
    Store::for_services(&cx.services).records().get(&iyakki_agent::store::install::record_key(kind, &iyakki_agent::skills::Scope::Shared, name)).map(|r| r.version.clone()).unwrap_or_default()
}

#[test]
fn the_installed_page_offers_updates_and_update_all_reviews_skips_what_needs_a_decision_and_reports() {
    let (cx, k, d) = world("mg-all");
    publish(&d, 1, &k, &[tool("shout", "1.0.0", &["net:example.com"], &k), skill("weekly-report", "1.0.0", &k)]);
    let mut a = StoreApp::new(&cx);
    add_folder(&mut a, &cx, &d);
    install_cat(&mut a, &cx, "shout");
    install_cat(&mut a, &cx, "weekly-report");
    // the stick now has newer versions; the tool wants one permission more
    publish(&d, 2, &k, &[tool("shout", "1.1.0", &["net:example.com", "write:/tmp"], &k), skill("weekly-report", "1.1.0", &k)]);
    a.reload(&cx);
    a.switch(Section::Installed);
    assert_eq!(a.lives.iter().filter(|l| l.has_update()).count(), 2);
    for (w, h) in SIZES {
        let g = draw(&mut a, &cx, w, h);
        assert!(has_label(&g, "Updates 2") && has_label(&g, "Update all 2 (U)"), "{w}x{h}: {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
        all_inside(&g, w, h, "the installed page with updates");
        no_overlap(&g, w, h, |e| matches!(e, Event::Row(_) | Event::Tab(220..=240) | Event::Focus(_)), "rows, chips and the search box");
    }
    // the Updates chip narrows the list; every row with an update has an Update button
    click_ev(&mut a, &cx, 1280, 720, Event::Tab(manage::TAB_UPDATES));
    assert!(a.iupdates && a.ifiltered().len() == 2);
    let g = draw(&mut a, &cx, 1280, 720);
    assert_eq!(g.hits.iter().filter(|h| h.label == "Update").count(), 2);
    click_ev(&mut a, &cx, 1280, 720, Event::Tab(manage::TAB_UPDATES));
    assert!(!a.iupdates);
    // Update all: a review first, in every size
    click_ev(&mut a, &cx, 1280, 720, Event::Tab(manage::TAB_UPDATE_ALL));
    let Mode::Confirm(Ask::UpdateAll(rows)) = &a.mode else { panic!("review first") };
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|r| r.1.contains("asks for more") && !r.2) && rows.iter().any(|r| r.1.contains("1.0.0 -> 1.1.0") && r.2), "{rows:?}");
    for (w, h) in SIZES {
        let g = draw(&mut a, &cx, w, h);
        assert!(has_label(&g, "Update all (y)") && has_label(&g, "Cancel (Esc)"), "{w}x{h}");
        all_inside(&g, w, h, "the update-all review");
    }
    key(&mut a, &cx, Key::Esc);
    assert_eq!((version_of(&cx, "skill", "weekly-report"), version_of(&cx, "tool", "shout")), ("1.0.0".into(), "1.0.0".into()), "cancel changes nothing");
    key(&mut a, &cx, Key::Char('U'));
    key(&mut a, &cx, Key::Char('y'));
    assert!(a.job.is_some(), "one at a time, off the screen's thread");
    wait_job(&mut a, &cx);
    let Mode::Report(rep) = &a.mode else { panic!("{:?}", a.status_text()) };
    assert!(rep.summary().starts_with("1 updated, 1 skipped, 0 failed."), "{}", rep.summary());
    for (w, h) in SIZES {
        let g = draw(&mut a, &cx, w, h);
        assert!(has_label(&g, "Close (Esc)"), "{w}x{h}");
        all_inside(&g, w, h, "the update-all report");
    }
    key(&mut a, &cx, Key::Esc);
    assert!(matches!(a.mode, Mode::Browse));
    assert_eq!((version_of(&cx, "skill", "weekly-report"), version_of(&cx, "tool", "shout")), ("1.1.0".into(), "1.0.0".into()));
    assert!(a.status_text().unwrap().starts_with("1 updated"));
    // nothing left that needs no decision: Update all still lists the tool, as a skip
    key(&mut a, &cx, Key::Char('U'));
    assert!(matches!(&a.mode, Mode::Confirm(Ask::UpdateAll(r)) if r.len() == 1));
}

#[test]
fn updating_one_item_shows_the_new_permissions_applies_only_after_the_yes_and_can_be_rolled_back_and_pinned() {
    let (cx, k, d) = world("mg-one");
    publish(&d, 1, &k, &[tool("shout", "1.0.0", &["net:example.com"], &k)]);
    let mut a = StoreApp::new(&cx);
    add_folder(&mut a, &cx, &d);
    install_cat(&mut a, &cx, "shout");
    publish(&d, 2, &k, &[tool("shout", "1.1.0", &["net:example.com", "write:/tmp"], &k)]);
    a.reload(&cx);
    a.switch(Section::Installed);
    select(&mut a, "shout");
    key(&mut a, &cx, Key::Char('u'));
    wait_job(&mut a, &cx);
    let Mode::Review(rv) = &a.mode else { panic!("{:?}", a.status_text()) };
    let plan = rv.plan.clone().expect("a planned update");
    assert_eq!(plan.added.iter().map(|p| p.words.as_str()).collect::<Vec<_>>(), vec!["write files in /tmp"]);
    for (w, h) in SIZES {
        let g = draw(&mut a, &cx, w, h);
        assert!(has_label(&g, "Update and allow the new permissions (y)"), "{w}x{h}: {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
        all_inside(&g, w, h, "the update review");
    }
    key(&mut a, &cx, Key::Esc);
    assert_eq!(version_of(&cx, "tool", "shout"), "1.0.0");
    assert!(a.status_text().unwrap().contains("Nothing was changed"));
    // yes: applied in the background, checked, the old version kept
    key(&mut a, &cx, Key::Char('u'));
    wait_job(&mut a, &cx);
    key(&mut a, &cx, Key::Char('y'));
    wait_job(&mut a, &cx);
    assert_eq!(version_of(&cx, "tool", "shout"), "1.1.0", "{:?}", a.status_text());
    assert!(a.status_text().unwrap().contains("is now 1.1.0"));
    let l = a.life_of(&a.inst.iter().find(|e| e.name == "shout").unwrap().key.clone()).unwrap().clone();
    assert_eq!(l.previous.as_deref(), Some("1.0.0"));
    // the page says so and offers the way back
    select(&mut a, "shout");
    key(&mut a, &cx, Key::Enter);
    for (w, h) in SIZES {
        let g = draw(&mut a, &cx, w, h);
        assert!(has_label(&g, "Go back to 1.0.0 (b)") && has_label(&g, "Pin this version (p)"), "{w}x{h}: {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
        all_inside(&g, w, h, "the item page");
        no_overlap(&g, w, h, |e| matches!(e, Event::Tab(240..=248)), "the item page's buttons");
    }
    // go back: asks, then does it in the background and pins
    key(&mut a, &cx, Key::Char('b'));
    assert!(matches!(a.mode, Mode::Confirm(Ask::Rollback { .. })));
    key(&mut a, &cx, Key::Char('n'));
    assert_eq!(version_of(&cx, "tool", "shout"), "1.1.0");
    key(&mut a, &cx, Key::Char('b'));
    key(&mut a, &cx, Key::Char('y'));
    wait_job(&mut a, &cx);
    assert_eq!(version_of(&cx, "tool", "shout"), "1.0.0", "{:?}", a.status_text());
    assert!(a.status_text().unwrap().contains("pinned"));
    let key_ = a.inst.iter().find(|e| e.name == "shout").unwrap().key.clone();
    assert!(a.life_of(&key_).unwrap().pinned.is_some());
    // pinned: no Update button, the update says why; p unpins
    key(&mut a, &cx, Key::Char('u'));
    assert!(a.status_text().unwrap().contains("pinned at 1.0.0"), "{:?}", a.status_text());
    key(&mut a, &cx, Key::Char('p'));
    assert!(a.life_of(&key_).unwrap().pinned.is_none());
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(has_label(&g, "Update to 1.1.0 (u)") || has_label(&g, "Pin this version (p)"));
    // pin by key, once more, from the list
    key(&mut a, &cx, Key::Esc);
    select(&mut a, "shout");
    key(&mut a, &cx, Key::Char('p'));
    assert!(a.life_of(&key_).unwrap().pinned.is_some());
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label.starts_with("shout")) && !g.hits.iter().any(|h| h.label == "Update"), "a pinned item has no Update button");
    let log = std::fs::read_to_string(iyakki_agent::testdir::find("iyakki-shell", "mg-one").join("audit.log")).unwrap_or_default();
    assert!(log.contains("store.update") && log.contains("store.rollback"), "{log}");
}

#[test]
fn removing_shows_what_goes_and_what_stays_and_deleting_data_needs_the_name_typed() {
    let (cx, k, _d) = world("mg-rm");
    install(&cx, &skill("weekly-report", "1.0.0", &k));
    let data = cx.services.dir().join("store/appdata/skill/shared/weekly-report");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("notes.txt"), "keep me").unwrap();
    let mut a = StoreApp::new(&cx);
    select(&mut a, "weekly-report");
    key(&mut a, &cx, Key::Char('r'));
    let Mode::Confirm(Ask::RemovePlan(ui)) = &a.mode else { panic!("a plan, not just a question") };
    assert!(ui.plan.has_data() && ui.plan.removes[0].contains("skill file"));
    for (w, h) in SIZES {
        let g = draw(&mut a, &cx, w, h);
        assert!(has_label(&g, "Remove, keep data (y)") && has_label(&g, "Remove and delete data (x)") && has_label(&g, "Cancel (Esc)"), "{w}x{h}");
        all_inside(&g, w, h, "the removal dialog");
        no_overlap(&g, w, h, |e| matches!(e, Event::Key(_)), "its buttons");
    }
    // x asks for the name; a wrong one removes nothing
    key(&mut a, &cx, Key::Char('x'));
    for (w, h) in SIZES {
        let g = draw(&mut a, &cx, w, h);
        all_inside(&g, w, h, "typing the name");
        assert!(has_label(&g, "Cancel"), "{w}x{h}");
    }
    typed(&mut a, &cx, "weekly");
    key(&mut a, &cx, Key::Enter);
    assert!(a.status_text().unwrap().contains("not the name") && cx.services.skills.get(&iyakki_agent::skills::Scope::Shared, "weekly-report").is_some());
    typed(&mut a, &cx, "-report");
    key(&mut a, &cx, Key::Enter);
    assert!(a.status_text().unwrap().contains("Deleted its data folder"), "{:?}", a.status_text());
    assert!(!data.exists() && cx.services.skills.get(&iyakki_agent::skills::Scope::Shared, "weekly-report").is_none());
    // the default keeps the data
    install(&cx, &skill("weekly-report", "1.0.0", &k));
    std::fs::create_dir_all(&data).unwrap();
    a.reload(&cx);
    select(&mut a, "weekly-report");
    key(&mut a, &cx, Key::Char('r'));
    key(&mut a, &cx, Key::Char('y'));
    assert!(a.status_text().unwrap().contains("Kept its data"), "{:?}", a.status_text());
    assert!(data.exists());
    // Esc cancels
    install(&cx, &skill("weekly-report", "1.0.0", &k));
    a.reload(&cx);
    select(&mut a, "weekly-report");
    key(&mut a, &cx, Key::Char('r'));
    key(&mut a, &cx, Key::Esc);
    assert!(cx.services.skills.get(&iyakki_agent::skills::Scope::Shared, "weekly-report").is_some());
}

#[test]
fn the_mcp_skills_and_cli_tiles_lead_to_the_same_actions() {
    let (cx, k, _d) = world("mg-tiles");
    install(&cx, &skill("weekly-report", "1.0.0", &k));
    cx.services.mcp.add(iyakki_agent::mcp::StoredServer { name: "files".into(), command: "/usr/bin/files".into(), args: vec![], sample: false }).unwrap();
    let mut m = crate::apps::mcp::McpApp::new(&cx);
    m.handle(&Event::Key(Key::Char('u')), &cx);
    assert_eq!(m.take_request(), Some(("store".into(), "installed:mcp:files".into())));
    let mut sk = crate::apps::skills::SkillsApp::new(&cx);
    sk.handle(&Event::Key(Key::Char('u')), &cx);
    assert_eq!(sk.take_request().map(|r| r.0), Some("store".into()));
    // the STORE tile opens on that item's page
    let mut a = StoreApp::new(&cx);
    a.open_item("installed:mcp:files", &cx);
    assert!(matches!(a.mode, Mode::Detail) && a.iselected().unwrap().name == "files");
    let g = draw(&mut a, &cx, 1024, 600);
    assert!(has_label(&g, "Remove (r)"));
    a.open_item("installed:skill:nothing", &cx);
    assert!(a.status_text().unwrap().contains("not installed"));
    // the CLI tile: u on a tool that is not installed says so; b with nothing kept says so
    iyakki_agent::clis::write_fake_installed(&cx.services.clis, "kubectl", "#!/bin/sh\necho v1\n");
    let mut c = crate::apps::cli::CliApp::new(&cx);
    c.handle(&Event::Tab(1), &cx);
    let at = iyakki_agent::clis::CATALOG.iter().position(|d| d.id == "kubectl").unwrap();
    c.handle(&Event::Row(at), &cx);
    c.handle(&Event::Key(Key::Char('b')), &cx);
    assert!(c.status_text().unwrap().contains("no earlier version"), "{:?}", c.status_text());
    c.handle(&Event::Key(Key::Char('u')), &cx);
    let g = {
        let s = crate::shell::scale_for(1280, 720);
        let mut g = Gfx::new(1280, 720);
        g.clear(rgb(9, 11, 20));
        c.render(&mut g, R::new(0.0, 46.0 * s, 1280.0, 720.0 - 46.0 * s), s, &cx, 3);
        g
    };
    assert!(g.hits.iter().any(|h| h.label.to_lowercase().contains("install") || h.label.to_lowercase().contains("yes") || h.label.contains("Cancel")), "the update asks first");
}

// ------------------------------------------------------------------------------------------------ arranging

fn shift(nav: Nav) -> Key {
    Key::Nav { nav, shift: true, ctrl: false }
}

/// A tile with four skills installed, the list narrowed to skills so everything fits on screen.
fn arranged(tag: &str) -> (AppCx, StoreApp) {
    let (cx, k, _d) = world(tag);
    for n in ["alpha", "beta", "gamma", "delta"] {
        let p = match n {
            "alpha" => skill("alpha", "1.0.0", &k),
            "beta" => skill("beta", "1.0.0", &k),
            "gamma" => skill("gamma", "1.0.0", &k),
            _ => skill("delta", "1.0.0", &k),
        };
        install(&cx, &p);
    }
    let mut a = StoreApp::new(&cx);
    a.ifilter = Some(IType::Skill);
    (cx, a)
}

fn names(a: &StoreApp) -> Vec<String> {
    a.ifiltered().iter().map(|e| e.name.clone()).collect()
}

fn make_group(a: &mut StoreApp, cx: &AppCx, name: &str) {
    key(a, cx, Key::Char('n'));
    typed(a, cx, name);
    key(a, cx, Key::Enter);
    assert!(a.org.groups.iter().any(|g| g.name == name), "{:?}", a.status_text());
}

fn move_to_group(a: &mut StoreApp, cx: &AppCx, item: &str, group: &str) {
    select(a, item);
    key(a, cx, Key::Char('m'));
    let Mode::Menu(m) = &a.mode else { panic!("the move menu") };
    let at = m.rows.iter().position(|r| r.0 == group).unwrap_or_else(|| panic!("{group} in {:?}", m.rows.iter().map(|r| &r.0).collect::<Vec<_>>()));
    key(a, cx, Key::Char(char::from_digit(at as u32 + 1, 10).unwrap()));
    assert!(matches!(a.mode, Mode::Browse), "{:?}", a.status_text());
}

fn group_id(a: &StoreApp, name: &str) -> String {
    a.org.groups.iter().find(|g| g.name == name).unwrap().id.clone()
}

fn group_of(a: &StoreApp, item: &str) -> Option<String> {
    a.org.group_of(&format!("skill:{item}")).map(|g| g.name.clone())
}

#[test]
fn tags_titles_and_groups_are_edited_with_the_keyboard_complete_from_existing_tags_and_are_kept_per_account() {
    let (cx, mut a) = arranged("mg-org-keys");
    // tags: free text, several, with Tab completing from the tags that exist
    select(&mut a, "alpha");
    key(&mut a, &cx, Key::Char('t'));
    assert!(matches!(a.mode, Mode::Edit(_)));
    typed(&mut a, &cx, "Work, Dev tools");
    key(&mut a, &cx, Key::Enter);
    assert_eq!(a.org.tags_of("skill:alpha"), ["dev-tools", "work"].map(String::from));
    select(&mut a, "beta");
    key(&mut a, &cx, Key::Char('t'));
    typed(&mut a, &cx, "wo");
    let g = draw(&mut a, &cx, 800, 480);
    assert!(has_label(&g, "tag work"), "the suggestion is offered and clickable");
    key(&mut a, &cx, Key::Tab);
    key(&mut a, &cx, Key::Enter);
    assert_eq!(a.org.tags_of("skill:beta"), ["work"].map(String::from));
    // the tags show on the row and as filters; the tag dialog works at every size
    for (w, h) in SIZES {
        let g = draw(&mut a, &cx, w, h);
        assert!(has_label(&g, "tag work") && has_label(&g, "Tags (T)") && has_label(&g, "Group by: None (v)"), "{w}x{h}: {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
        all_inside(&g, w, h, "the installed page with tags");
        no_overlap(&g, w, h, |e| matches!(e, Event::Row(_) | Event::Tab(220..=232) | Event::Tab(2600..=2700) | Event::Focus(_)), "rows, chips and controls");
    }
    // your own title: shown in the list, the real name stays as the subtitle and on the page
    select(&mut a, "alpha");
    key(&mut a, &cx, Key::Char('e'));
    typed(&mut a, &cx, "Notes helper");
    key(&mut a, &cx, Key::Enter);
    let e = a.ifiltered().into_iter().find(|e| e.name == "alpha").unwrap().clone();
    assert_eq!(a.shown_title(&e), "Notes helper");
    assert!(a.row_sub(&e).starts_with("“alpha” · "), "{}", a.row_sub(&e));
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label.starts_with("Notes helper ")) && !g.hits.iter().any(|h| h.label.starts_with("alpha ")));
    key(&mut a, &cx, Key::Enter);
    draw(&mut a, &cx, 1280, 720);
    let text: Vec<String> = a.text.lines.iter().map(|l| l.0.clone()).collect();
    assert!(text.iter().any(|l| l == "Real name: alpha · id skill:alpha") && text.iter().any(|l| l == "Publisher: Acme Ltd") && text.iter().any(|l| l == "Your title: Notes helper"), "{text:?}");
    key(&mut a, &cx, Key::Esc);
    // too long a title is refused with the reason, an empty one goes back to the real title
    key(&mut a, &cx, Key::Char('e'));
    typed(&mut a, &cx, &"x".repeat(60));
    key(&mut a, &cx, Key::Enter);
    assert_eq!(a.org.alias_of("skill:alpha"), Some("Notes helper"), "{:?}", a.status_text());
    // groups: made, items moved by menu, listed in the person's order
    make_group(&mut a, &cx, "Work");
    make_group(&mut a, &cx, "Kids");
    assert!(a.status_text().unwrap().contains("Kids"));
    key(&mut a, &cx, Key::Char('n'));
    typed(&mut a, &cx, "work");
    key(&mut a, &cx, Key::Enter);
    assert!(a.status_text().unwrap().contains("already a group"), "{:?}", a.status_text());
    move_to_group(&mut a, &cx, "alpha", "Work");
    move_to_group(&mut a, &cx, "beta", "Work");
    move_to_group(&mut a, &cx, "gamma", "Kids");
    assert_eq!((group_of(&a, "alpha"), group_of(&a, "gamma"), group_of(&a, "delta")), (Some("Work".into()), Some("Kids".into()), None));
    // a new group from the move menu takes the item at once
    select(&mut a, "delta");
    key(&mut a, &cx, Key::Char('m'));
    key(&mut a, &cx, Key::Char('9'));
    assert!(matches!(a.mode, Mode::Menu(_)), "9 is beyond the list; nothing happened");
    let n = if let Mode::Menu(m) = &a.mode { m.rows.len() } else { 0 };
    key(&mut a, &cx, Key::Char(char::from_digit(n as u32, 10).unwrap()));
    assert!(matches!(a.mode, Mode::Edit(_)));
    typed(&mut a, &cx, "Later");
    key(&mut a, &cx, Key::Enter);
    assert_eq!(group_of(&a, "delta"), Some("Later".into()));
    // kept for this account only
    let again = StoreApp::new(&cx);
    assert_eq!((again.org.tags_of("skill:alpha").len(), again.org.groups.len(), again.org.alias_of("skill:alpha")), (2, 3, Some("Notes helper")));
    assert!(Org::load(Store::for_services(&cx.services).dir(), "alice").is_empty(), "another account has its own, empty, arrangement");
    assert!(Store::for_services(&cx.services).dir().join("org-root.json").exists());
    // what is no longer installed is forgotten
    let st = Store::for_services(&cx.services);
    st.remove(&cx.services, &Actor::system(), "skill", "gamma", &iyakki_agent::skills::Scope::Shared).unwrap();
    a.org.set_tags("skill:gamma", &["old".into()]).unwrap();
    a.org.save().unwrap();
    let again = StoreApp::new(&cx);
    assert!(again.org.group_of("skill:gamma").is_none() && again.org.tags_of("skill:gamma").is_empty(), "orphans are pruned");
    assert!(again.org.group_of("skill:alpha").is_some());
}

#[test]
fn a_title_of_your_own_never_hides_what_the_thing_is_in_the_page_the_list_or_the_install_review() {
    let (cx, k, d) = world("mg-alias");
    publish(&d, 1, &k, &[tool("shout", "1.0.0", &["net:example.com"], &k)]);
    let mut a = StoreApp::new(&cx);
    add_folder(&mut a, &cx, &d);
    install_cat(&mut a, &cx, "shout");
    a.switch(Section::Installed);
    select(&mut a, "shout");
    key(&mut a, &cx, Key::Char('e'));
    typed(&mut a, &cx, "Bank login helper");
    key(&mut a, &cx, Key::Enter);
    let e = a.iselected().unwrap();
    assert_eq!(a.shown_title(&e), "Bank login helper");
    assert!(a.row_sub(&e).contains("“shout”"), "the real name is the subtitle: {}", a.row_sub(&e));
    // the update review is about the package, not the title
    publish(&d, 2, &k, &[tool("shout", "1.1.0", &["net:example.com"], &k)]);
    a.reload(&cx);
    key(&mut a, &cx, Key::Char('u'));
    wait_job(&mut a, &cx);
    let Mode::Review(rv) = &a.mode else { panic!("{:?}", a.status_text()) };
    let sub = manage::review_sub(&rv.review);
    assert!(sub.contains("shout") && sub.contains("1.1.0") && !sub.contains("Bank"), "{sub}");
    assert_eq!((rv.review.name.as_str(), rv.review.verdict.publisher.as_ref().map(|p| p.name.as_str())), ("shout", None));
    assert!(!rv.review.title.contains("Bank") && !rv.review.summary().contains("Bank"), "{}", rv.review.summary());
    assert_eq!(rv.review.meta.publisher.name, "Acme Ltd");
    for (w, h) in SIZES {
        let g = draw(&mut a, &cx, w, h);
        all_inside(&g, w, h, "the review with a title of your own set");
    }
    key(&mut a, &cx, Key::Esc);
    // Discover shows the same arrangement, and the install review there is just as plain
    cat_on(&mut a, &cx);
    let it = a.items.iter().find(|i| i.entry.name == "shout").unwrap().clone();
    assert_eq!(a.item_sub(&it, true).split(" · ").next(), Some("“shout”"));
    let g = draw(&mut a, &cx, 1024, 600);
    assert!(g.hits.iter().any(|h| h.label.starts_with("Bank login helper ")), "{:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
}

/// Press an item row or heading the way the shell reports a real mouse press: the pointer is seen first, then the press.
fn press(a: &mut StoreApp, cx: &AppCx, g: &Gfx, ev: Event) -> (i32, i32) {
    let (x, y) = center(g, &ev);
    a.handle(&Event::MouseMove(x, y), cx);
    a.handle(&ev, cx);
    (x, y)
}

fn row_ord(a: &StoreApp, name: &str) -> usize {
    a.ifiltered().iter().position(|e| e.name == name).unwrap_or_else(|| panic!("{name}: {:?}", names(a)))
}

#[test]
fn dragging_moves_items_between_groups_reorders_inside_a_group_and_reorders_groups_but_a_click_never_drags() {
    let (cx, mut a) = arranged("mg-drag");
    make_group(&mut a, &cx, "Work");
    make_group(&mut a, &cx, "Kids");
    for n in ["alpha", "beta", "gamma"] {
        move_to_group(&mut a, &cx, n, "Work");
    }
    key(&mut a, &cx, Key::Char('v'));
    assert_eq!(a.iview, View::Groups);
    // the groups and what is in them, in the person's order
    assert_eq!(names(&a), ["alpha", "beta", "gamma", "delta"]);
    for (w, h) in SIZES {
        let g = draw(&mut a, &cx, w, h);
        assert!(has_label(&g, "group Work") && has_label(&g, "group Kids") && has_label(&g, "group Ungrouped"), "{w}x{h}: {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
        all_inside(&g, w, h, "the grouped list");
        no_overlap(&g, w, h, |e| matches!(e, Event::Row(_) | Event::Tab(4000..=4100)), "rows and headings");
    }
    // a click (even a wobbly one) opens the item and moves nothing
    let g = draw(&mut a, &cx, 1280, 720);
    let (x, y) = { let ev = Event::Row(row_ord(&a, "delta")); press(&mut a, &cx, &g, ev) };
    a.handle(&Event::MouseMove(x + 3, y + 3), &cx);
    assert!(a.drag.as_ref().is_some_and(|d| !d.active), "under the threshold it is still a click");
    a.handle(&Event::MouseUp(x + 3, y + 3), &cx);
    assert!(matches!(a.mode, Mode::Detail) && a.iselected().unwrap().name == "delta" && group_of(&a, "delta").is_none());
    key(&mut a, &cx, Key::Esc);
    // drag delta onto the Kids heading: it moves there, with a ghost and a highlight while it travels
    let g = draw(&mut a, &cx, 1280, 720);
    let (x, y) = { let ev = Event::Row(row_ord(&a, "delta")); press(&mut a, &cx, &g, ev) };
    let kids = a.heads().iter().position(|h| h.label == "Kids").unwrap();
    let (hx, hy) = center(&g, &Event::Tab(manage::TAB_HEAD + kids));
    a.handle(&Event::MouseMove(hx, hy), &cx);
    assert!(a.drag.as_ref().is_some_and(|d| d.active) && (hx - x).abs() + (hy - y).abs() >= manage::DRAG_THRESHOLD);
    for (w, h) in SIZES {
        let gg = draw(&mut a, &cx, w, h);
        all_inside(&gg, w, h, "the list while dragging");
    }
    draw(&mut a, &cx, 1280, 720);
    let before = a.org.clone();
    a.handle(&Event::MouseUp(hx, hy), &cx);
    assert_eq!(group_of(&a, "delta"), Some("Kids".into()), "{:?}", a.status_text());
    assert!(a.drag.is_none() && matches!(a.mode, Mode::Browse));
    assert_ne!(before.assign, a.org.assign);
    // reorder inside Work: gamma dropped on the upper half of alpha goes before it; on the lower half of beta after it
    let g = draw(&mut a, &cx, 1280, 720);
    { let ev = Event::Row(row_ord(&a, "gamma")); press(&mut a, &cx, &g, ev) };
    let ra = g.hits.iter().rev().find(|h| h.ev == Event::Row(row_ord(&a, "alpha"))).unwrap().rect;
    let (tx, ty) = ((ra.x + ra.w / 2.0) as i32, (ra.y + 3.0) as i32);
    a.handle(&Event::MouseMove(tx, ty), &cx);
    a.handle(&Event::MouseUp(tx, ty), &cx);
    assert_eq!(a.org.arrange(a.org.group_id_of("skill:gamma"), ["skill:alpha", "skill:beta", "skill:gamma"].map(String::from).to_vec()), ["skill:gamma", "skill:alpha", "skill:beta"].map(String::from));
    assert_eq!(&names(&a)[..3], ["gamma", "alpha", "beta"]);
    let g = draw(&mut a, &cx, 1280, 720);
    { let ev = Event::Row(row_ord(&a, "gamma")); press(&mut a, &cx, &g, ev) };
    let rb = g.hits.iter().rev().find(|h| h.ev == Event::Row(row_ord(&a, "beta"))).unwrap().rect;
    let (tx, ty) = ((rb.x + rb.w / 2.0) as i32, (rb.y + rb.h - 3.0) as i32);
    a.handle(&Event::MouseMove(tx, ty), &cx);
    a.handle(&Event::MouseUp(tx, ty), &cx);
    assert_eq!(&names(&a)[..3], ["alpha", "beta", "gamma"]);
    // reorder the groups: Kids dragged onto the upper half of Work's heading goes first
    let g = draw(&mut a, &cx, 1280, 720);
    let work = a.heads().iter().position(|h| h.label == "Work").unwrap();
    let kids = a.heads().iter().position(|h| h.label == "Kids").unwrap();
    press(&mut a, &cx, &g, Event::Tab(manage::TAB_HEAD + kids));
    let rw = g.hits.iter().rev().find(|h| h.ev == Event::Tab(manage::TAB_HEAD + work)).unwrap().rect;
    let (tx, ty) = ((rw.x + rw.w / 2.0) as i32, (rw.y + 2.0) as i32);
    a.handle(&Event::MouseMove(tx, ty), &cx);
    a.handle(&Event::MouseUp(tx, ty), &cx);
    assert_eq!(a.org.groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["Kids", "Work"]);
    // a click on a heading folds the group; the items vanish from the list but stay in the group
    let g = draw(&mut a, &cx, 1280, 720);
    let work = a.heads().iter().position(|h| h.label == "Work").unwrap();
    let (x, y) = press(&mut a, &cx, &g, Event::Tab(manage::TAB_HEAD + work));
    a.handle(&Event::MouseUp(x, y), &cx);
    assert!(a.org.group(&group_id(&a, "Work")).unwrap().collapsed);
    assert!(!names(&a).contains(&"alpha".to_string()) && group_of(&a, "alpha") == Some("Work".into()));
    let g = draw(&mut a, &cx, 800, 480);
    all_inside(&g, 800, 480, "a folded group");
    // dropping while not in My groups explains instead of guessing
    key(&mut a, &cx, Key::Char('v'));
    assert_eq!(a.iview, View::Category);
    let g = draw(&mut a, &cx, 1280, 720);
    let first_row = g.hits.iter().find(|h| matches!(h.ev, Event::Row(_))).unwrap().ev.clone();
    let (x, y) = press(&mut a, &cx, &g, first_row);
    a.handle(&Event::MouseMove(x + 40, y + 40), &cx);
    a.handle(&Event::MouseUp(x + 40, y + 40), &cx);
    assert!(a.status_text().unwrap().contains("My groups"), "{:?}", a.status_text());
    // a key during a drag cancels it
    let g = draw(&mut a, &cx, 1280, 720);
    let first_row = g.hits.iter().find(|h| matches!(h.ev, Event::Row(_))).unwrap().ev.clone();
    press(&mut a, &cx, &g, first_row);
    key(&mut a, &cx, Key::Esc);
    assert!(a.drag.is_none());
}

#[test]
fn everything_dragging_does_the_keyboard_does_too_move_menu_shift_arrows_and_the_heading_keys() {
    let (cx, mut a) = arranged("mg-kbd");
    make_group(&mut a, &cx, "Work");
    make_group(&mut a, &cx, "Kids");
    for n in ["alpha", "beta", "gamma"] {
        move_to_group(&mut a, &cx, n, "Work");
    }
    key(&mut a, &cx, Key::Char('v'));
    // Shift+Down / Shift+Up put an item in a different place inside its group
    select(&mut a, "alpha");
    key(&mut a, &cx, shift(Nav::Down));
    assert_eq!(&names(&a)[..3], ["beta", "alpha", "gamma"]);
    assert_eq!(a.iselected().unwrap().name, "alpha", "the cursor follows");
    key(&mut a, &cx, shift(Nav::Up));
    key(&mut a, &cx, shift(Nav::Up));
    assert_eq!(&names(&a)[..3], ["alpha", "beta", "gamma"]);
    // the cursor moves over headings: Up from the first item lands on its group's heading
    select(&mut a, "alpha");
    key(&mut a, &cx, Key::Up);
    assert_eq!(a.on_head, Some(Some(group_id(&a, "Work"))));
    key(&mut a, &cx, Key::Char('o'));
    assert!(a.status_text().unwrap().contains("Choose an item first"));
    // Enter folds, again unfolds; e renames; k changes the colour; Shift+Down moves the group down
    key(&mut a, &cx, Key::Enter);
    assert!(a.org.group(&group_id(&a, "Work")).unwrap().collapsed);
    key(&mut a, &cx, Key::Char(' '));
    assert!(!a.org.group(&group_id(&a, "Work")).unwrap().collapsed);
    let c0 = a.org.group(&group_id(&a, "Work")).unwrap().colour;
    key(&mut a, &cx, Key::Char('k'));
    assert_ne!(a.org.group(&group_id(&a, "Work")).unwrap().colour, c0);
    key(&mut a, &cx, Key::Char('e'));
    for _ in 0..4 {
        key(&mut a, &cx, Key::Backspace);
    }
    typed(&mut a, &cx, "Office");
    key(&mut a, &cx, Key::Enter);
    assert_eq!(a.org.groups[0].name, "Office");
    key(&mut a, &cx, shift(Nav::Down));
    assert_eq!(a.org.groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["Kids", "Office"]);
    let g = draw(&mut a, &cx, 1024, 600);
    all_inside(&g, 1024, 600, "the cursor on a heading");
    // x deletes the group after asking; its items stay, ungrouped
    key(&mut a, &cx, Key::Char('x'));
    assert!(matches!(a.mode, Mode::Confirm(Ask::DeleteGroup(..))));
    key(&mut a, &cx, Key::Esc);
    assert_eq!(a.org.groups.len(), 2);
    key(&mut a, &cx, Key::Char('x'));
    key(&mut a, &cx, Key::Char('y'));
    assert_eq!((a.org.groups.len(), group_of(&a, "alpha")), (1, None));
    assert!(names(&a).contains(&"alpha".to_string()));
    // the organise menu reaches the same things
    key(&mut a, &cx, Key::Char('g'));
    let Mode::Menu(m) = &a.mode else { panic!("the organise menu") };
    assert!(m.rows.iter().any(|r| r.0.starts_with("Group by: Category")) && m.rows.iter().any(|r| r.0.starts_with("Export")) && m.rows.iter().any(|r| r.0.starts_with("Delete group")));
    for (w, h) in SIZES {
        let g = draw(&mut a, &cx, w, h);
        all_inside(&g, w, h, "the organise menu");
    }
    key(&mut a, &cx, Key::Esc);
    // without My groups, Shift+arrows say so instead of doing something odd
    key(&mut a, &cx, Key::Char('v'));
    select(&mut a, "alpha");
    key(&mut a, &cx, shift(Nav::Down));
    assert!(a.status_text().unwrap().contains("My groups"), "{:?}", a.status_text());
}

#[test]
fn tags_groups_and_titles_filter_and_search_and_the_list_can_be_grouped_and_sorted_other_ways() {
    let (cx, mut a) = arranged("mg-filter");
    select(&mut a, "alpha");
    a.org.set_tags("skill:alpha", &["work".into(), "daily".into()]).unwrap();
    a.org.set_tags("skill:beta", &["work".into()]).unwrap();
    a.org.set_tags("skill:gamma", &["kids".into()]).unwrap();
    a.org.set_alias("skill:delta", Some("Zebra planner")).unwrap();
    let g1 = a.org.create_group("Evenings", 2).unwrap();
    a.org.move_item("skill:gamma", Some(&g1), None).unwrap();
    a.org_save();
    // chips: click a tag to filter, several narrow it down, click again to release
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(has_label(&g, "tag work") && has_label(&g, "tag daily") && has_label(&g, "tag kids"));
    let work = a.org.all_tags().iter().position(|t| t.0 == "work").unwrap();
    let daily = a.org.all_tags().iter().position(|t| t.0 == "daily").unwrap();
    click_ev(&mut a, &cx, 1280, 720, Event::Tab(manage::TAB_TAG_CHIP + work));
    assert_eq!(names(&a), ["alpha", "beta"]);
    click_ev(&mut a, &cx, 1280, 720, Event::Tab(manage::TAB_TAG_CHIP + daily));
    assert_eq!(names(&a), ["alpha"], "several tags narrow the list down");
    key(&mut a, &cx, Key::Esc);
    assert!(a.itags.is_empty(), "Esc clears the tag filters");
    a.ifilter = Some(IType::Skill);
    assert_eq!(names(&a).len(), 4);
    // search finds by tag, by group and by title of your own as well as by name
    for (q, want) in [("kids", vec!["gamma"]), ("evenings", vec!["gamma"]), ("zebra", vec!["delta"]), ("delta", vec!["delta"]), ("daily work", vec!["alpha"])] {
        key(&mut a, &cx, Key::Char('/'));
        typed(&mut a, &cx, q);
        assert_eq!(names(&a), want, "search for {q}");
        key(&mut a, &cx, Key::Esc);
    }
    // the tag manager: filter, rename (merging), delete
    key(&mut a, &cx, Key::Char('T'));
    for (w, h) in SIZES {
        let g = draw(&mut a, &cx, w, h);
        assert!(has_label(&g, "tag work") && has_label(&g, "Close (Esc)"), "{w}x{h}");
        all_inside(&g, w, h, "the tag manager");
    }
    key(&mut a, &cx, Key::Char(' '));
    assert_eq!(a.itags, ["work"]);
    key(&mut a, &cx, Key::Char('r'));
    for _ in 0..4 {
        key(&mut a, &cx, Key::Backspace);
    }
    typed(&mut a, &cx, "kids");
    key(&mut a, &cx, Key::Enter);
    assert_eq!((a.org.tags_of("skill:alpha").contains(&"kids".to_string()), a.org.tags_of("skill:alpha").contains(&"work".to_string())), (true, false), "{:?}", a.org.tags);
    assert!(matches!(a.mode, Mode::Tags(_)) && a.itags == ["kids"], "the filter follows the rename");
    key(&mut a, &cx, Key::Char('x'));
    assert!(matches!(a.mode, Mode::Confirm(Ask::DeleteTag(..))));
    key(&mut a, &cx, Key::Char('y'));
    assert!(a.org.all_tags().iter().all(|t| t.0 != "kids" && t.0 != "work") && a.itags.is_empty());
    key(&mut a, &cx, Key::Esc);
    // group by: every view has its headings
    a.org.set_tags("skill:alpha", &["work".into()]).unwrap();
    let heads = |a: &StoreApp| a.heads().iter().map(|h| h.label.clone()).collect::<Vec<_>>();
    assert!(a.heads().is_empty());
    key(&mut a, &cx, Key::Char('v'));
    assert_eq!(heads(&a), ["Evenings", "Ungrouped"]);
    key(&mut a, &cx, Key::Char('v'));
    assert_eq!(heads(&a), ["work"], "the publisher's categories, read only");
    key(&mut a, &cx, Key::Char('v'));
    assert_eq!(heads(&a), ["Skill"]);
    key(&mut a, &cx, Key::Char('v'));
    assert_eq!(heads(&a).len(), 1);
    key(&mut a, &cx, Key::Char('v'));
    assert_eq!(a.iview, View::Flat);
    assert_eq!(a.org.view, "flat");
    // the choice is remembered
    a.org.view = "kind".into();
    a.org.save().unwrap();
    assert_eq!(StoreApp::new(&cx).iview, View::Kind);
    // the grouping chip works by click
    click_ev(&mut a, &cx, 1280, 720, Event::Tab(manage::TAB_VIEW));
    assert_ne!(a.iview, View::Kind);
    // sort: updates first
    a.isort = Sort::Trust;
    key(&mut a, &cx, Key::Char('s'));
    assert_eq!(a.isort, Sort::Updates);
}

#[test]
fn the_arrangement_goes_to_a_file_and_back_on_another_machine() {
    let (cx, mut a) = arranged("mg-export");
    make_group(&mut a, &cx, "Work");
    move_to_group(&mut a, &cx, "alpha", "Work");
    a.org.set_tags("skill:alpha", &["daily".into()]).unwrap();
    a.org.set_alias("skill:alpha", Some("Notes")).unwrap();
    a.org_save();
    // export through the host's save dialog
    key(&mut a, &cx, Key::Char('g'));
    let Mode::Menu(m) = &a.mode else { panic!() };
    let at = m.rows.iter().position(|r| r.0.starts_with("Export")).unwrap();
    for _ in 0..at {
        key(&mut a, &cx, Key::Down);
    }
    key(&mut a, &cx, Key::Enter);
    let (name, bytes) = a.take_host_save().expect("a file to save");
    assert_eq!(name, "iyakki-organisation.json");
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("Work") && text.contains("Notes") && !text.contains("Acme"), "only the arrangement, nothing else");
    let path = cx.services.dir().join("exported.json");
    std::fs::write(&path, &text).unwrap();
    // another machine with the same things installed
    let (cx2, mut b) = arranged("mg-export-2");
    assert!(b.org.is_empty());
    key(&mut b, &cx2, Key::Char('g'));
    let Mode::Menu(m) = &b.mode else { panic!() };
    let at = m.rows.iter().position(|r| r.0.starts_with("Import an arrangement (merge)")).unwrap();
    for _ in 0..at {
        key(&mut b, &cx2, Key::Down);
    }
    key(&mut b, &cx2, Key::Enter);
    assert!(matches!(b.mode, Mode::Edit(_)));
    for _ in 0..5 {
        key(&mut b, &cx2, Key::Backspace);
    }
    typed(&mut b, &cx2, &path.display().to_string());
    key(&mut b, &cx2, Key::Enter);
    assert!(b.status_text().unwrap().starts_with("Merged"), "{:?}", b.status_text());
    assert_eq!((group_of(&b, "alpha"), b.org.alias_of("skill:alpha"), b.org.tags_of("skill:alpha").to_vec()), (Some("Work".into()), Some("Notes"), vec!["daily".to_string()]));
    // a file that is not an arrangement is refused and nothing changes
    std::fs::write(&path, "not json").unwrap();
    key(&mut b, &cx2, Key::Char('g'));
    let Mode::Menu(m) = &b.mode else { panic!() };
    let at = m.rows.iter().position(|r| r.0.starts_with("Import an arrangement (replace")).unwrap();
    for _ in 0..at {
        key(&mut b, &cx2, Key::Down);
    }
    key(&mut b, &cx2, Key::Enter);
    for _ in 0..5 {
        key(&mut b, &cx2, Key::Backspace);
    }
    typed(&mut b, &cx2, &path.display().to_string());
    key(&mut b, &cx2, Key::Enter);
    assert!(b.status_text().unwrap().contains("not an organisation file"), "{:?}", b.status_text());
    assert_eq!(group_of(&b, "alpha"), Some("Work".into()));
}

#[test]
fn discover_shows_titles_tags_and_groups_filters_by_them_and_takes_drops_on_group_headings() {
    let (cx, k, d) = world("mg-discover");
    publish(&d, 1, &k, &[skill("alpha", "1.0.0", &k), skill("beta", "1.0.0", &k)]);
    let mut a = StoreApp::new(&cx);
    add_folder(&mut a, &cx, &d);
    cat_on(&mut a, &cx);
    a.org.set_alias("skill:alpha", Some("Notes helper")).unwrap();
    a.org.set_tags("skill:beta", &["work".into()]).unwrap();
    let w = a.org.create_group("Work", 1).unwrap();
    a.org_save();
    a.filter = Filter { publisher: Some("acme".into()), ..Default::default() };
    // search by the person's own words
    key(&mut a, &cx, Key::Char('/'));
    typed(&mut a, &cx, "notes helper");
    assert_eq!(a.filtered().iter().map(|i| a.items[*i].entry.name.clone()).collect::<Vec<_>>(), ["alpha"]);
    key(&mut a, &cx, Key::Esc);
    a.filter = Filter { publisher: Some("acme".into()), ..Default::default() };
    // the tag chip filters the catalog
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(has_label(&g, "tag work") && has_label(&g, "Group by: None (v)") && has_label(&g, "Tags (T)"));
    click_ev(&mut a, &cx, 1280, 720, Event::Tab(manage::TAB_TAG_CHIP));
    assert_eq!(a.filtered().iter().map(|i| a.items[*i].entry.name.clone()).collect::<Vec<_>>(), ["beta"]);
    key(&mut a, &cx, Key::Esc);
    a.filter = Filter { publisher: Some("acme".into()), ..Default::default() };
    // group by My groups by key, then drag beta onto the Work heading
    key(&mut a, &cx, Key::Char('v'));
    assert_eq!(a.iview, View::Groups);
    for (wd, ht) in SIZES {
        let g = draw(&mut a, &cx, wd, ht);
        assert!(has_label(&g, "group Work") && has_label(&g, "group Ungrouped"), "{wd}x{ht}: {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
        all_inside(&g, wd, ht, "grouped Discover");
        no_overlap(&g, wd, ht, |e| matches!(e, Event::Row(_) | Event::Tab(4000..=4100)), "rows and headings");
    }
    let g = draw(&mut a, &cx, 1280, 720);
    let ord = a.rows().iter().filter_map(|r| if let Row::Item(i) = r { Some(*i) } else { None }).position(|i| a.items[i].entry.name == "beta").unwrap();
    press(&mut a, &cx, &g, Event::Row(ord));
    let hw = a.heads().iter().position(|h| h.label == "Work").unwrap();
    let (hx, hy) = center(&g, &Event::Tab(manage::TAB_HEAD + hw));
    a.handle(&Event::MouseMove(hx, hy), &cx);
    a.handle(&Event::MouseUp(hx, hy), &cx);
    assert_eq!(a.org.group_id_of("skill:beta"), Some(w.as_str()), "{:?}", a.status_text());
    // keys in Discover: # tags, m moves, e renames
    key(&mut a, &cx, Key::Char('#'));
    assert!(matches!(a.mode, Mode::Edit(_)));
    key(&mut a, &cx, Key::Esc);
    key(&mut a, &cx, Key::Char('m'));
    assert!(matches!(a.mode, Mode::Menu(_)));
    key(&mut a, &cx, Key::Esc);
    // w (not m) chooses for whom new installs are
    key(&mut a, &cx, Key::Char('w'));
    assert!(a.status_text().unwrap().contains("installs"), "{:?}", a.status_text());
}
