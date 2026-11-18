use super::*;
use iyakki_agent_api::{StoreComponents, StoreHub, StoreUpdates};
use iyakki_sec::policy::Role;

struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

fn facets(tag: &str) -> (StoreFacetsImpl, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("iyakki-storefacets-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let svc = Services::open(&dir.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    (StoreFacetsImpl::new(svc), dir)
}

#[test]
fn a_fresh_machine_has_no_tiles_no_report_no_wizards_and_the_stable_channel() {
    let (f, dir) = facets("fresh");
    let admin = Role::Admin.template();
    assert!(f.tiles(&admin, "root").is_empty());
    assert!(!f.wasm_tiles_enabled(), "the experimental switch is off by default");
    assert!(f.update_report().is_none());
    assert!(f.pending_wizards("root").is_empty());
    assert!(f.update_announce_stores().is_empty());
    assert_eq!(f.components_channel(), "stable");
    assert!(f.components_check_lines().is_empty() && f.components_last_result().is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_report_the_check_kept_is_what_the_facet_reads_and_off_is_never_due() {
    let (f, dir) = facets("report");
    let rep = Report { checked_at: 1234, ..Default::default() };
    rep.save(f.svc.dir()).unwrap();
    assert_eq!(f.update_report().map(|r| r.checked_at), Some(1234));
    assert!(!f.update_check_due(&CheckSettings { mode: iyakki_shared::store_updatecheck::Mode::Off, ..Default::default() }));
    assert!(!f.update_check_due(&CheckSettings { mode: iyakki_shared::store_updatecheck::Mode::Manual, ..Default::default() }), "only the button looks");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_channel_is_validated_and_kept_in_the_data_folder() {
    let (f, dir) = facets("channel");
    assert!(f.components_set_channel("weekly").is_err());
    f.components_set_channel("nightly").unwrap();
    assert_eq!(f.components_channel(), "nightly");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn without_a_pinned_store_there_is_nothing_to_review_and_an_unknown_store_is_not_installed_from() {
    let (f, dir) = facets("nostore");
    assert!(f.components_review("stable").unwrap_err().starts_with("No store with a checked key"));
    let review = ComponentReview { store: "Ghost".into(), channel: "stable".into(), review: iyakki_shared::store_compapply::Review { text: String::new(), digest: "x".into(), restart_machine: false, restart_core: false, parts: 0, download: 0 } };
    assert!(f.components_apply(&review, "root").unwrap_err().contains("no longer a pinned store"));
    assert!(!f.svc.dir().join("components-installed.json").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_dev_app_needs_an_administrator() {
    let (f, dir) = facets("dev");
    let err = f.add_dev_tile(&Role::User.template(), "amy", &dir).unwrap_err();
    assert!(err.contains("Only an administrator"), "{err}");
    assert!(f.remove_dev_tile("amy", "nothing").is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_catalog_facet_words_a_permission_scaffolds_a_project_and_reads_its_version() {
    use iyakki_agent_api::StoreCatalog;
    let (f, dir) = facets("catalog");
    assert!(f.permission_words("tile:storage", "root").unwrap().contains("saved data"));
    assert!(f.permission_words("not a permission", "root").is_none());
    let st = f.private_store_status("root");
    assert!(st.instance.is_none() && st.added.is_none(), "a fresh machine has no private store");
    let root = dir.join("work");
    std::fs::create_dir_all(&root).unwrap();
    assert!(f.scaffold_project("rocket", "x", &root, "x").is_err(), "an unknown kind is refused");
    f.scaffold_project("skill", "meeting-notes", &root, "meeting-notes").unwrap();
    assert!(root.join("meeting-notes").is_dir());
    assert_eq!(f.project_version(&root.join("nothing-here")), "");
    let _ = std::fs::remove_dir_all(dir);
}
