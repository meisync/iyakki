//! The facets of lane b2 answer "unavailable" by default, the fake needs no agent, the recording fake notes what the UI asked (F-SPLIT-03).

use std::path::Path;

use iyakki_agent_api::{AgentApi, AgentFacets, AgentFacetsB2, DictFixFacet, FakeAgentApi, NtpFacet, ProfileStoreFacet, RecordingFacetsB2, StartupDiskFacet, WebGraphicsFacet};
use iyakki_shared::dictfix::{Context, Target};
use iyakki_shared::keyboard::KeyboardSettings;
use iyakki_shared::profilestore::Status;
use iyakki_shared::webgraphics::Mode;

#[test]
fn the_fake_agent_has_every_b2_facet_and_they_answer_unavailable() {
    fn takes(_: &dyn AgentApi) {}
    let f = FakeAgentApi::default();
    takes(&f);
    assert_eq!(f.profile_store().status(Path::new("/x"), "ann"), Status::Off);
    assert!(f.profile_store().protect_for(&(), "ann").is_err());
    assert!(!f.profile_store().is_protected(Path::new("/x"), "ann"));
    assert_eq!(f.profile_store().on_sign_out(Path::new("/x"), "ann"), Ok(false));
    assert_eq!(f.ntp().status().last_sync, 0);
    assert!(f.ntp().set_clock(1.0).is_err());
    assert!(f.startup_disk().first(Path::new("/x")).is_err());
    assert!(f.startup_disk().status_line(Path::new("/x"), false).is_empty());
    assert!(f.proc_owner().mine(&iyakki_core_api::sysinfo::SysRoots::real(), "ann", false).is_empty());
    let mut ran = false;
    f.proc_owner().as_user("ann", &mut || ran = true);
    assert!(ran, "the fake still runs the closure");
    assert!(!f.keyboard_apple().apple_state(Path::new("/x")).driver);
    assert_eq!(f.keyboard_apple().apply_apple(Path::new("/x"), &KeyboardSettings::default()), Ok(vec![]));
    assert_eq!(f.dict_fix().target(&()), Target::NoModel);
    assert!(f.dict_fix().set_enabled(&(), true).is_err());
    let fixed = f.dict_fix().fix_wording(&(), "hello", &Context::default());
    assert_eq!(fixed.text, "hello", "an unavailable correction keeps the words as heard");
    assert_eq!(f.web_graphics().load_mode(Path::new("/x")).0, Mode::Automatic);
    assert!(f.web_graphics().set_mode(Path::new("/x"), Mode::Software).is_err());
    assert!(f.web_graphics().take_pending().is_empty());
    assert_eq!(f.lemonade_compute().lemonade_compute(Path::new("/x")), iyakki_shared::gpu_steps::Compute::Gpu);
}

#[test]
fn the_recording_fake_notes_the_calls_the_ui_made() {
    let r = RecordingFacetsB2::default();
    assert_eq!(ProfileStoreFacet::status(&r, Path::new("/x"), "ann"), Status::Off);
    assert_eq!(StartupDiskFacet::register(&r, Path::new("/x")), Ok("registered".to_string()));
    assert!(WebGraphicsFacet::set_mode(&r, Path::new("/x"), Mode::Hardware).is_ok());
    assert_eq!(r.calls(), vec!["profile_store.status ann", "startup_disk.register", "web_graphics.set_mode hardware"]);
    // the defaults of the traits it does not override are still the unavailable ones
    assert!(NtpFacet::set_clock(&r, 1.0).is_err());
    assert_eq!(DictFixFacet::target(&r, &()), Target::NoModel);
    let _: &dyn AgentFacetsB2 = &FakeAgentApi::default();
}

#[test]
fn the_fake_agent_has_no_clusters_and_an_empty_gpu_page() {
    let f = FakeAgentApi::default();
    let role = iyakki_sec::policy::Role::Admin;
    assert!(f.cluster_registry().switch(Path::new("/x"), Path::new("/x"), "ann", role, "local").is_err());
    assert_eq!(f.cluster_registry().gpu_summary("local"), None);
    let run: std::sync::Arc<dyn iyakki_shared::wsexec::Runner> = f.exec().system();
    assert_eq!(f.cluster_registry().refresh_gpus(&run, &[], false), 0);
    assert!(f.gpu_page().quick_report(Path::new("/"), Path::new("/x")).gpus.is_empty());
    f.gpu_page().refresh_async(std::sync::Arc::new(()));
}
