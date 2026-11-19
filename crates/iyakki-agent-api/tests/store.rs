//! The store facets and their fake (F-SPLIT-03): defaults, recorded calls, the approval bound to the reviewed digest.

use iyakki_agent_api::{ComponentReview, FakeStoreFacets, NoStoreFacets, StoreComponents, StoreFacets, StoreHub, StoreUpdates, WizardLine};
use iyakki_sec::policy::Role;
use iyakki_shared::store_compapply::Review;
use iyakki_shared::store_updatecheck::{CheckSettings, Mode, Report};
use iyakki_shared::store_wizardhook::Pending;

fn review(digest: &str) -> ComponentReview {
    ComponentReview { store: "Lab store".into(), channel: "stable".into(), review: Review { text: "Install system components from Lab store".into(), digest: digest.into(), restart_machine: false, restart_core: true, parts: 2, download: 4096 } }
}

#[test]
fn a_handle_that_serves_nothing_answers_the_defaults_and_refuses_changes() {
    let n = NoStoreFacets;
    let admin = Role::Admin.template();
    assert!(n.tiles(&admin, "root").is_empty());
    assert!(n.add_dev_tile(&admin, "root", std::path::Path::new("/x")).is_err());
    assert!(!n.wasm_tiles_enabled());
    assert!(n.update_report().is_none() && !n.update_check_due(&CheckSettings::default()));
    assert!(n.pending_wizards("root").is_empty());
    assert_eq!(n.components_channel(), "stable");
    assert!(n.components_review("stable").is_err());
    assert!(n.components_apply(&review("d"), "root").is_err(), "nothing is installed by a core that does not serve the call");
}

#[test]
fn one_handle_serves_the_three_facets() {
    fn takes(_: &dyn StoreFacets) {}
    takes(&FakeStoreFacets::default());
    fn only_updates(u: &dyn StoreUpdates) -> bool {
        u.update_report().is_some()
    }
    let f = FakeStoreFacets::default();
    assert!(!only_updates(&f));
    f.set_report(Some(Report::default()));
    assert!(only_updates(&f));
}

#[test]
fn the_fake_records_every_call_by_name() {
    let f = FakeStoreFacets::default();
    f.set_due(true);
    let s = CheckSettings { mode: Mode::Daily, ..Default::default() };
    assert!(f.update_check_due(&s));
    let rep = f.update_check_run(&Role::Admin.template(), "root", &s, "0.1.0", "desktop");
    assert_eq!(rep.count(), 0);
    assert!(!f.update_check_due(&s), "a run clears the due flag");
    f.set_wizards(vec![WizardLine { pending: Pending { app: "desk".into(), wizard: "setup".into(), title: String::new(), trigger: "on_install".into(), from_version: None, queued_at: 1 }, outline: "asks 2 questions".into() }]);
    assert_eq!(f.pending_wizards("root").len(), 1);
    assert_eq!(f.calls(), ["update_check_due", "update_check_run(root, 0.1.0)", "update_check_due", "pending_wizards(root)"]);
}

#[test]
fn only_an_administrator_adds_a_dev_app_and_only_a_dev_app_is_removed() {
    use iyakki_shared::store_tilehub::{Caps, TileEntry};
    let entry = |name: &str, dev: bool| TileEntry { key: format!("dev/{name}"), name: name.into(), title: name.into(), icon: String::new(), icon_picture: None, group: "My apps".into(), version: "1".into(), publisher: "you".into(), trust: "unsigned dev app".into(), dev, dir: None, scope: iyakki_shared::skills::Scope::User("root".into()), caps: Caps::default(), wasm: false };
    let f = FakeStoreFacets::default();
    f.set_tiles(vec![entry("clock", true), entry("notes", false)]);
    let dir = std::path::Path::new("/work/clock");
    assert!(f.add_dev_tile(&Role::User.template(), "amy", dir).is_err());
    assert!(f.add_dev_tile(&Role::Admin.template(), "root", dir).is_ok());
    assert!(f.remove_dev_tile("root", "notes").is_err(), "an installed app is not a dev app");
    assert!(f.remove_dev_tile("root", "clock").is_ok());
    assert_eq!(f.tiles(&Role::Admin.template(), "root").len(), 1);
}

#[test]
fn a_components_install_is_refused_when_the_plan_is_not_the_one_that_was_reviewed() {
    let f = FakeStoreFacets::default();
    f.set_review(Some(review("digest-1")));
    let shown = f.components_review("stable").unwrap();
    assert_eq!(f.components_apply(&shown, "root").unwrap(), "Installed 2 part(s) from Lab store.");
    // the store's list changed after the review was shown: the old approval does not cover it
    f.set_review(Some(review("digest-2")));
    assert!(f.components_apply(&shown, "root").is_err());
    assert_eq!(f.applied(), ["digest-1"]);
}

#[test]
fn the_channel_is_one_of_three_and_is_kept() {
    let f = FakeStoreFacets::default();
    assert_eq!(f.components_channel(), "stable");
    assert!(f.components_set_channel("weekly").is_err());
    f.components_set_channel("beta").unwrap();
    assert_eq!(f.components_channel(), "beta");
}

#[test]
fn the_catalog_facet_answers_nothing_by_default_and_the_fake_records_what_was_asked() {
    use iyakki_agent_api::StoreCatalog;
    let n = NoStoreFacets;
    assert!(n.private_store_status("root").instance.is_none());
    assert!(n.permission_words("tile:storage", "root").is_none() && n.stick_stores().is_empty());
    assert!(n.scaffold_project("tile", "clock", std::path::Path::new("/w"), "clock").is_err());
    let f = FakeStoreFacets::default();
    assert_eq!(f.permission_words("tile:storage", "root").as_deref(), Some("tile permission storage"));
    assert!(f.permission_words("storage", "root").is_none());
    assert!(f.scaffold_project("tile", "clock", std::path::Path::new("/w"), "clock").is_ok());
    assert!(f.scaffold_project("rocket", "x", std::path::Path::new("/w"), "x").is_err());
    assert_eq!(f.scaffolded(), ["tile clock"]);
}
