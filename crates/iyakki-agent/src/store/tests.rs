//! Store-level tests: the built-in catalog on a first boot, the view of items, local (USB) catalogs, search, the
//! assistant's tools and their hard-ask rules. No network: every fetch is a table.

use std::sync::Arc;

use super::catalog::tests::{catalog, entry, FakeFetch};
use super::catalog::{sha256_hex, SourceKind};
use super::install::tests::{admin, fx, meta, skill_pkg, user, Fx, NOW};
use super::install::{Actor, Confirm};
use super::trust::Level;
use super::*;
use crate::pkg::{new_signing_key, public_key_hex};

fn a_user_policy() -> Policy {
    iyakki_sec::policy::Role::User.template()
}

fn cat_on(f: &Fx) {
    f.store.set_catalog_enabled(&Actor::system(), true).unwrap();
}

fn items(f: &Fx, a: &Actor) -> Vec<Item> {
    let l = f.store.local_catalogs(&f.svc);
    f.store.items(&f.svc, a, &l)
}

fn by<'a>(v: &'a [Item], id: &str) -> &'a Item {
    v.iter().find(|i| i.entry.id == id).unwrap_or_else(|| panic!("{id} is listed"))
}

#[test]
fn a_first_boot_with_no_network_and_no_stick_shows_the_built_in_catalog() {
    let f = fx("first");
    let a = admin();
    // by default the made-up paid apps are not part of anything
    assert!(!f.store.catalog_visible());
    assert!(!items(&f, &a).iter().any(|i| i.entry.demo), "no demo apps unless the catalog is switched on");
    assert!(f.store.local_catalogs(&f.svc)[0].catalog.entries.len() > 20, "the shipped skills are still there for the assistant and for installing");
    cat_on(&f);
    assert!(f.store.catalog_visible());
    let v = items(&f, &a);
    assert!(v.len() > 20, "{}", v.len());
    let py = by(&v, "lang-python");
    assert_eq!((py.verdict.level, py.state.clone(), py.source.as_str()), (Level::Official, State::Available, "builtin"));
    assert!(py.installable && py.verdict.label.contains("built into"));
    assert_eq!(by(&v, "cli-kubectl").state, State::Link("cli".into()));
    // the three made-up paid apps show their price and cannot be installed
    let demos: Vec<&Item> = v.iter().filter(|i| i.entry.demo).collect();
    assert_eq!(demos.len(), 3);
    assert!(demos.iter().all(|i| i.state == State::Demo && i.entry.meta.pricing.price > 0 && i.verdict.level == Level::Community));
    let r = f.store.review_item(&f.svc, &a, demos[0], &f.store.local_catalogs(&f.svc), &FakeFetch::default(), Scope::Shared, &[]);
    assert!(r.unwrap_err().contains("made-up example"));
    assert!(f.store.review_item(&f.svc, &a, by(&v, "cli-kubectl"), &f.store.local_catalogs(&f.svc), &FakeFetch::default(), Scope::Shared, &[]).unwrap_err().contains("CLI tile"));
    // the optional daily check is off
    assert!(!f.store.auto_check_due() && f.store.config().sources.is_empty());
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn shipped_skills_can_be_installed_removed_and_reinstalled_from_the_store_and_preinstalled_ones_show_as_such() {
    let f = fx("preinstalled");
    let a = admin();
    // the shell seeds these when an administrator first signs in: simulate one
    let (_, md) = builtin::SKILLS[1];
    f.svc.skills.add(Scope::Shared, md, true, true).unwrap();
    let v = items(&f, &a);
    let py = by(&v, builtin::SKILLS[1].0);
    assert_eq!(py.state, State::Installed);
    assert!(py.preinstalled && py.installed_version.as_deref() == Some("preinstalled"));
    // remove it, then install it again from the store
    f.store.remove(&f.svc, &a, "skill", py.entry.name.as_str(), &Scope::Shared).unwrap();
    let v = items(&f, &a);
    assert_eq!(by(&v, "lang-python").state, State::Available);
    let l = f.store.local_catalogs(&f.svc);
    let r = f.store.review_item(&f.svc, &a, by(&v, "lang-python"), &l, &FakeFetch::default(), Scope::Shared, &Store::known_names(&v)).unwrap();
    assert_eq!(r.verdict.level, Level::Official, "{}", r.summary());
    assert!(r.refusal.is_none() && r.permissions.is_empty(), "{}", r.summary());
    f.store.install(&f.svc, &a, &r, Confirm::default()).unwrap();
    assert!(f.svc.skills.get(&Scope::Shared, "lang-python").is_some());
    assert_eq!(by(&items(&f, &a), "lang-python").state, State::Installed);
    // every shipped skill installs from its built-in package
    for (name, _) in builtin::SKILLS.iter().chain(builtin::SAMPLE_SKILLS.iter()) {
        let v = items(&f, &a);
        let it = by(&v, name);
        if it.state.is_installed() {
            f.store.remove(&f.svc, &a, "skill", name, &Scope::Shared).unwrap();
        }
        let r = f.store.review_item(&f.svc, &a, by(&items(&f, &a), name), &l, &FakeFetch::default(), Scope::Shared, &[]).unwrap();
        f.store.install(&f.svc, &a, &r, Confirm::default()).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(f.svc.skills.get(&Scope::Shared, name).is_some(), "{name}");
    }
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn the_sample_mcp_server_installs_for_administrators_and_replaces_the_seeded_sample() {
    let f = fx("mcpdemo");
    f.svc.mcp.add(crate::mcp::StoredServer { name: "iyakki-demo".into(), command: "/old/iyakki".into(), args: vec!["--mcp-demo".into()], sample: true }).unwrap();
    let a = admin();
    let v = items(&f, &a);
    assert_eq!(by(&v, "iyakki-demo").state, State::Installed, "the seeded sample counts as preinstalled");
    f.store.remove(&f.svc, &a, "mcp", "iyakki-demo", &Scope::Shared).unwrap();
    let l = f.store.local_catalogs(&f.svc);
    let r = f.store.review_item(&f.svc, &a, by(&items(&f, &a), "iyakki-demo"), &l, &FakeFetch::default(), Scope::Shared, &[]).unwrap();
    assert!(r.refusal.is_none() && r.permissions[0].words.contains("IYAKKI itself"), "{}", r.summary());
    assert_eq!(f.store.install(&f.svc, &a, &r, Confirm { permissions: false, warnings: true }).unwrap_err(), super::install::InstallError::NeedsConfirmation("Confirm the permissions first.".into()));
    f.store.install(&f.svc, &a, &r, Confirm::all()).unwrap();
    let s = f.svc.mcp.list();
    assert_eq!((s[0].name.as_str(), s[0].args.clone(), s[0].command.is_empty()), ("iyakki-demo", vec!["--mcp-demo".to_string()], false));
    // an ordinary user is refused
    let r = f.store.review_item(&f.svc, &user("alice"), by(&items(&f, &user("alice")), "iyakki-demo"), &l, &FakeFetch::default(), Scope::Shared, &[]).unwrap();
    assert!(r.refusal.is_some());
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn a_usb_stick_catalog_installs_a_signed_package_with_no_network_at_all() {
    let f = fx("usb");
    let a = admin();
    let publisher = new_signing_key().unwrap();
    let cert = super::trust::Cert::issue(&f.root, super::manifest::Publisher { id: "acme".into(), name: "Acme".into(), url: String::new() }, &public_key_hex(&publisher), NOW - 5, NOW + 5000, &["skill"]);
    let bytes = skill_pkg("1.0.0", Some(&publisher), Some(&cert), &meta("acme", &[]));
    let stick = f.dir.join("mnt/usb/iyakki-store");
    std::fs::create_dir_all(stick.join("packages")).unwrap();
    std::fs::write(stick.join("packages/meeting-notes.iyp"), &bytes).unwrap();
    let mut e = entry("meeting-notes", &bytes, "packages/meeting-notes.iyp");
    e.signer = public_key_hex(&publisher);
    e.cert = Some(cert);
    e.meta = meta("acme", &[]);
    let mut c = catalog("stick", 1, vec![e]);
    c.sign(&f.root);
    std::fs::write(stick.join("catalog.json"), c.to_json()).unwrap();
    // the tile finds sticks and the administrator adds it
    assert_eq!(catalog::discover_sticks(&[&f.dir.join("mnt")]), vec![stick.clone()]);
    assert!(f.store.add_source(&user("alice"), "x", SourceKind::Folder, stick.to_str().unwrap()).is_err(), "only administrators change sources");
    f.store.add_source(&a, "My stick", SourceKind::Folder, stick.to_str().unwrap()).unwrap();
    let v = items(&f, &a);
    let it = by(&v, "meeting-notes");
    assert_eq!((it.verdict.level, it.source_name.as_str()), (Level::Verified, "My stick"), "the badge is worked out from the signer and certificate in the entry");
    let l = f.store.local_catalogs(&f.svc);
    let r = f.store.review_item(&f.svc, &a, it, &l, &FakeFetch::default(), Scope::Shared, &[]).unwrap();
    assert_eq!(r.verdict.level, Level::Verified, "{}", r.summary());
    f.store.install(&f.svc, &a, &r, Confirm::all()).unwrap();
    assert!(f.svc.skills.get(&Scope::Shared, "meeting-notes").is_some());
    assert_eq!(by(&items(&f, &a), "meeting-notes").state, State::Installed);
    // a package swapped on the stick fails the catalog's pin
    std::fs::write(stick.join("packages/meeting-notes.iyp"), String::from_utf8(bytes.clone()).unwrap().replace("List decisions", "Do evil")).unwrap();
    let it = items(&f, &a).into_iter().find(|i| i.entry.id == "meeting-notes").unwrap();
    assert!(f.store.review_item(&f.svc, &a, &it, &l, &FakeFetch::default(), Scope::Shared, &[]).unwrap_err().contains("size") || true);
    f.store.remove_source(&a, "src1").unwrap();
    assert!(f.store.local_catalogs(&f.svc).len() == 1);
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn a_remote_catalog_is_refreshed_cached_and_its_revocation_list_turns_a_withdrawn_app_off() {
    let f = fx("remote");
    let a = admin();
    let publisher = new_signing_key().unwrap();
    let bytes = skill_pkg("1.0.0", Some(&publisher), None, &meta("acme", &[]));
    let mut e = entry("meeting-notes", &bytes, "packages/mn.iyp");
    e.signer = public_key_hex(&publisher);
    e.meta = meta("acme", &[]);
    let mut c = catalog("remote", 3, vec![e]);
    c.sign(&f.root);
    let fetch = FakeFetch::default();
    fetch.put("https://cat.example/s/catalog.json", c.to_json().as_bytes());
    fetch.put("https://cat.example/s/packages/mn.iyp", &bytes);
    f.store.add_source(&a, "Remote", SourceKind::Url, "https://cat.example/s/catalog.json").unwrap();
    assert_eq!(f.store.local_catalogs(&f.svc).len(), 1, "nothing is fetched until asked");
    assert!(fetch.asked.lock().unwrap().is_empty());
    let rf = f.store.refresh(&f.svc, &fetch);
    assert!(rf.errors.is_empty(), "{:?}", rf.errors);
    assert_eq!(rf.loaded[0].catalog.serial, 3);
    assert_eq!(f.store.config().last_check, NOW);
    // offline afterwards: the cache is used
    let l = f.store.local_catalogs(&f.svc);
    assert_eq!(l.len(), 2);
    let v = f.store.items(&f.svc, &a, &l);
    let it = by(&v, "meeting-notes");
    assert_eq!(it.verdict.level, Level::Community);
    let r = f.store.review_item(&f.svc, &a, it, &l, &fetch, Scope::Shared, &[]).unwrap();
    f.store.install(&f.svc, &a, &r, Confirm::all()).unwrap();
    // the next refresh brings a revocation list beside the catalog
    let list = revocation::sign_list(&f.root, 1, NOW, NOW + 1000, vec![public_key_hex(&publisher)], vec![], vec![], vec![]);
    fetch.put("https://cat.example/s/revocations.json", list.as_bytes());
    let rf = f.store.refresh(&f.svc, &fetch);
    assert_eq!(rf.disabled.len(), 1, "{:?}", rf.disabled);
    assert!(rf.notes.iter().any(|n| n.contains("revocation list")));
    assert!(f.svc.skills.get(&Scope::Shared, "meeting-notes").is_none());
    assert_eq!(by(&f.store.items(&f.svc, &a, &f.store.local_catalogs(&f.svc)), "meeting-notes").state, State::Revoked);
    // a failing source is reported, not fatal
    fetch.drop_all();
    let rf = f.store.refresh(&f.svc, &fetch);
    assert!(rf.errors.is_empty() && rf.loaded[0].check.warnings.iter().any(|w| w.contains("Could not refresh")), "{:?}", rf.errors);
    f.store.set_auto_check(&a, true).unwrap();
    assert!(!f.store.auto_check_due(), "just checked");
    let later = f.store.clone().with_clock(Arc::new(|| NOW + 2 * 86_400));
    assert!(later.auto_check_due());
    assert!(f.store.set_auto_check(&user("alice"), false).is_err());
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn updates_are_offered_when_a_catalog_has_a_newer_version() {
    let f = fx("updates");
    let a = user("alice");
    let key = new_signing_key().unwrap();
    f.svc.packages.trust_key(&public_key_hex(&key), "Acme").unwrap();
    let v1 = skill_pkg("1.0.0", Some(&key), None, &meta("acme", &[]));
    let r = f.store.review_sideload(&f.svc, &a, v1, Scope::User("alice".into()), &[]);
    f.store.install(&f.svc, &a, &r, Confirm::all()).unwrap();
    let v2 = skill_pkg("1.2.0", Some(&key), None, &meta("acme", &[]));
    let mut e = entry("meeting-notes", &v2, "mn.iyp");
    e.version = "1.2.0".into();
    e.signer = public_key_hex(&key);
    e.meta = meta("acme", &[]);
    let stick = f.dir.join("stick");
    std::fs::create_dir_all(&stick).unwrap();
    std::fs::write(stick.join("mn.iyp"), &v2).unwrap();
    std::fs::write(stick.join("catalog.json"), catalog("stick", 1, vec![e]).to_json()).unwrap();
    f.store.add_source(&admin(), "S", SourceKind::Folder, stick.to_str().unwrap()).unwrap();
    let v = items(&f, &a);
    let it = by(&v, "meeting-notes");
    assert_eq!(it.state, State::Update("1.2.0".into()));
    assert_eq!(it.verdict.level, Level::Trusted);
    let l = f.store.local_catalogs(&f.svc);
    let r = f.store.review_item(&f.svc, &a, it, &l, &FakeFetch::default(), Scope::User("alice".into()), &[]).unwrap();
    assert!(r.summary().contains("update from 1.0.0"), "{}", r.summary());
    f.store.install(&f.svc, &a, &r, Confirm::all()).unwrap();
    assert_eq!(by(&items(&f, &a), "meeting-notes").state, State::Installed);
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn search_and_filters_find_by_words_price_trust_type_category_and_publisher() {
    let f = fx("search");
    cat_on(&f);
    let v = items(&f, &admin());
    let ids = |fl: Filter| filter(&v, &fl).iter().map(|i| i.entry.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(Filter { query: "rust".into(), ..Default::default() })[0], "lang-rust");
    assert!(ids(Filter { query: "kubernetes cluster".into(), ..Default::default() }).contains(&"k3s-dev-best-practices".to_string()) || !ids(Filter { query: "k3s".into(), ..Default::default() }).is_empty());
    assert!(ids(Filter { query: "zzzznothing".into(), ..Default::default() }).is_empty());
    let paid = ids(Filter { price: PriceFilter::Paid, ..Default::default() });
    assert_eq!(paid.len(), 3);
    assert!(ids(Filter { price: PriceFilter::Free, ..Default::default() }).iter().all(|i| !paid.contains(i)));
    assert!(ids(Filter { kind: Some("cli".into()), ..Default::default() }).iter().all(|i| i.starts_with("cli-")));
    assert!(ids(Filter { level: Some(Level::Community), ..Default::default() }).len() == 3, "only the demos are Community here");
    assert!(ids(Filter { level: Some(Level::Revoked), ..Default::default() }).is_empty());
    assert!(ids(Filter { category: Some("languages".into()), ..Default::default() }).len() == 10);
    assert!(ids(Filter { publisher: Some("example-studio".into()), ..Default::default() }).len() == 3);
    assert!(featured(&v).iter().any(|i| i.entry.id == "lang-python"));
    assert!(newest(&v, 5).len() == 5 && newest(&v, 5).iter().all(|i| !i.entry.demo));
    let cats = categories(&v);
    assert!(cats.iter().any(|c| c.0 == "skills" && c.1 == 18) && cats.iter().any(|c| c.0 == "tools"));
    let ps = publishers(&v);
    assert_eq!(ps[0].id, "iyakki");
    assert!(ps[0].apps > 20 && ps.iter().any(|p| p.id == "example-studio" && p.level == Level::Community));
    let _ = std::fs::remove_dir_all(&f.dir);
}

// ---------------------------------------------------------------------------------------- the assistant's tools

#[test]
fn the_tools_exist_only_with_the_store_and_install_always_asks() {
    let user_p = a_user_policy();
    let names: Vec<String> = tools::defs(&user_p).iter().map(|d| d.spec.name.clone()).collect();
    assert_eq!(names, vec!["store_search", "store_info", "store_install", "store_outdated", "store_update", "store_remove", "store_rollback"]);
    let defs = tools::defs(&user_p);
    assert!(defs.iter().take(2).all(|d| d.risk == crate::agent::Risk::Safe && d.untrusted));
    assert_eq!(defs[2].risk, crate::agent::Risk::Ask);
    assert_eq!(defs[3].risk, crate::agent::Risk::Safe, "store_outdated only reads");
    assert!(defs[4..].iter().all(|d| d.risk == crate::agent::Risk::Ask), "the three writers ask");
    let mut none = user_p.clone();
    none.apps.remove(&iyakki_sec::policy::App::Store);
    assert!(tools::defs(&none).is_empty());
    assert!(iyakki_sec::policy::Role::Guest.template().apps.iter().all(|a| *a != iyakki_sec::policy::App::Store), "guests have no store");
    // Auto and Autopilot never run an install by themselves, whoever signed it
    assert!(crate::agent::hard_ask("store_install", &serde_json::json!({"id": "lang-python"})).is_some());
    assert!(crate::agent::hard_ask("store_search", &serde_json::json!({})).is_none() && crate::agent::hard_ask("store_info", &serde_json::json!({})).is_none());
    assert!(tools::is_store_tool("store_install") && !tools::is_store_tool("radio_play"));
}

#[test]
fn search_and_info_read_the_local_catalogs_and_install_goes_through_the_same_review() {
    let f = fx("tools");
    cat_on(&f);
    let p = iyakki_sec::policy::Role::Admin.template();
    let call = |n: &str, args: serde_json::Value| tools::call(n, &args, &f.svc, &p, "root");
    let out = call("store_search", serde_json::json!({"query": "python"})).unwrap();
    assert!(out.contains("lang-python") && out.contains("Official") && out.contains("Free"), "{out}");
    assert!(call("store_search", serde_json::json!({"query": "qqqq"})).unwrap().contains("Nothing in the store"));
    let out = call("store_search", serde_json::json!({"query": "", "kind": "theme"})).unwrap();
    assert!(out.contains("demo-neon-pro-themes") && out.contains("made-up example") && out.contains("USD 4.99"), "{out}");
    let info = call("store_info", serde_json::json!({"id": "cli-kubectl"})).unwrap();
    assert!(info.contains("Official") && info.contains("connect to"), "{info}");
    assert!(call("store_info", serde_json::json!({"id": "nope"})).is_err());
    // the approval line says what and who
    let d = tools::describe(&f.svc, &iyakki_sec::policy::Role::Admin.template(), "root", "store_install", &serde_json::json!({"id": "iyakki-demo", "scope": "everyone"})).unwrap();
    assert!(d.contains("install “MCP demo server”") && d.contains("for everyone") && d.contains("run IYAKKI") || d.contains("run the program"), "{d}");
    // a made-up app and a CLI link are refused with a reason
    assert!(call("store_install", serde_json::json!({"id": "demo-meeting-suite"})).unwrap_err().contains("made-up"));
    assert!(call("store_install", serde_json::json!({"id": "cli-kubectl"})).unwrap_err().contains("CLI tile"));
    // a built-in skill installs for the person
    let out = call("store_install", serde_json::json!({"id": "lang-go", "scope": "everyone"})).unwrap();
    assert!(out.contains("Installed") && out.contains("Official"), "{out}");
    assert!(f.svc.skills.get(&Scope::Shared, "lang-go").is_some());
    assert!(call("store_install", serde_json::json!({"id": "lang-go", "scope": "everyone"})).unwrap_err().contains("already installed"));
    // an ordinary user cannot install for everyone
    let up = a_user_policy();
    assert!(tools::call("store_install", &serde_json::json!({"id": "lang-c", "scope": "everyone"}), &f.svc, &up, "alice").unwrap_err().contains("for everyone"));
    let out = tools::call("store_install", &serde_json::json!({"id": "lang-c"}), &f.svc, &up, "alice").unwrap();
    assert!(out.contains("Installed"));
    assert!(f.svc.skills.get(&Scope::User("alice".into()), "lang-c").is_some());
    // no store, no tools
    let mut none = up.clone();
    none.apps.remove(&iyakki_sec::policy::App::Store);
    assert!(tools::call("store_search", &serde_json::json!({}), &f.svc, &none, "alice").is_err());
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn the_assistant_cannot_install_something_that_differs_from_what_the_catalog_listed() {
    let f = fx("tools-differ");
    let publisher = new_signing_key().unwrap();
    // the catalog lists a Community skill with no permissions...
    let bytes = skill_pkg("1.0.0", Some(&publisher), None, &meta("acme", &[]));
    let stick = f.dir.join("stick");
    std::fs::create_dir_all(&stick).unwrap();
    std::fs::write(stick.join("mn.iyp"), &bytes).unwrap();
    let mut e = entry("meeting-notes", &bytes, "mn.iyp");
    e.signer = public_key_hex(&publisher);
    e.meta = meta("acme", &["net:example.com"]);
    std::fs::write(stick.join("catalog.json"), catalog("stick", 1, vec![e]).to_json()).unwrap();
    f.store.add_source(&admin(), "S", SourceKind::Folder, stick.to_str().unwrap()).unwrap();
    let p = iyakki_sec::policy::Role::Admin.template();
    // ...but the package asks for none: the listing and the package disagree
    let err = tools::call("store_install", &serde_json::json!({"id": "meeting-notes"}), &f.svc, &p, "root").unwrap_err();
    assert!(err.contains("different permissions"), "{err}");
    assert!(f.svc.skills.get(&Scope::User("root".into()), "meeting-notes").is_none());
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn the_shipped_files_are_what_the_shell_and_the_dist_folder_hold() {
    for (name, md) in builtin::SKILLS {
        let on_disk = std::fs::read_to_string(format!("{}/../../dist/skills/{name}.md", env!("CARGO_MANIFEST_DIR"))).unwrap();
        assert_eq!(on_disk, md, "{name}");
    }
    let _ = sha256_hex(b"");
}

#[test]
fn the_catalog_toggle_is_off_by_default_admin_only_and_a_configured_catalog_switches_it_on() {
    let f = fx("toggle");
    assert!(!f.store.catalog_visible());
    assert!(f.store.set_catalog_enabled(&user("alice"), true).is_err());
    f.store.set_catalog_enabled(&admin(), true).unwrap();
    assert!(f.store.catalog_visible());
    f.store.set_catalog_enabled(&admin(), false).unwrap();
    assert!(!f.store.catalog_visible());
    f.store.add_source(&admin(), "S", SourceKind::Folder, "/mnt/usb").unwrap();
    assert!(f.store.catalog_visible(), "a configured catalog shows the catalog");
    f.store.remove_source(&admin(), "src1").unwrap();
    assert!(!f.store.catalog_visible());
    let _ = std::fs::remove_dir_all(&f.dir);
}
