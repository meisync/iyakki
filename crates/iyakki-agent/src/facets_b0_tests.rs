//! The real b0 facets over the agent modules (F-SPLIT-03).

use std::sync::Arc;

use iyakki_agent_api::{AgentFacets};
use iyakki_shared::target::Target;

use crate::AgentApiImpl;

#[test]
fn the_real_agent_hands_out_the_real_quiet_system_and_registry_http() {
    let api = AgentApiImpl;
    assert_eq!(api.b0().quiet().system().own_pid(), std::process::id());
    // a registry on a port nobody listens on is refused by the real HTTP, not by the "no core" default
    let e = api.b0().ocipush().http().request("GET", "127.0.0.1:1", "/v2/", &[], &[]).unwrap_err();
    assert!(!e.starts_with("unavailable"), "{e}");
}

#[test]
fn the_real_target_facet_plans_and_records_a_change_in_a_folder() {
    let d = std::env::temp_dir().join(format!("iyakki-b0-target-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let api = AgentApiImpl;
    let plan = api.b0().target().plan_change(&d, "calc", Target::IyakkiTile);
    assert_eq!(plan.to, Target::IyakkiTile);
    let (done, msg) = api.b0().target().apply_change(&d, "calc", Target::IyakkiTile).unwrap();
    assert_eq!(done.to, Target::IyakkiTile);
    assert!(msg.contains("IYAKKI tile"));
    assert_eq!(iyakki_shared::target::read(&d), Some(Target::IyakkiTile));
    assert!(api.b0().target().apply_change(&d, "calc", Target::IyakkiTile).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_ide_browser_facet_is_offered_only_to_accounts_with_the_apps() {
    let api = AgentApiImpl;
    // an account without the apps never gets it; whether one WITH the apps gets it depends on a web engine being present, which other tests
    // switch on for the whole process (browsertools::assume_engine), so that half is not asserted here
    let mut none = iyakki_sec::policy::Policy::resolve(iyakki_sec::policy::Role::User, &Default::default());
    none.apps.clear();
    assert!(!api.b0().idebrowser().offered(&none));
    assert!(api.b0().idebrowser().take_requests("nobody", "nothing").is_empty());
    let _ = Arc::new(());
}

#[test]
fn the_runtime_an_item_needs_in_shared_matches_the_cli_catalogue() {
    use crate::clis::{def, Source};
    use iyakki_shared::quickinstall::Item;
    for item in Item::ALL {
        let (mut pip, mut npm) = (false, false);
        if let Some(d) = item.cli_id().and_then(def) {
            pip = matches!(d.source, Source::Pip(_));
            npm = matches!(d.source, Source::Npm(_));
        }
        assert_eq!(item.pip_tool(), pip, "{}", item.id());
        assert_eq!(item.npm_tool(), npm, "{}", item.id());
    }
}
