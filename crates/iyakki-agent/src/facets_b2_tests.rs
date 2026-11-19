//! The real facets of lane b2 delegate to the agent's modules (F-SPLIT-03): each is checked against a made-up folder or the module's own answer.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_agent_api::{AgentFacetsB2};
use iyakki_shared::keyboard::KeyboardSettings;
use iyakki_shared::profilestore::Status;
use iyakki_shared::webgraphics::Mode;

use crate::agentapi_gen::AgentApiImpl;

fn tmp(tag: &str) -> PathBuf {
    let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-facets-b2-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_real_profile_store_facet_reads_the_data_folder() {
    let d = tmp("profile");
    let a = AgentApiImpl;
    assert_eq!(a.profile_store().status(&d, "ann"), Status::Off);
    assert!(!a.profile_store().is_protected(&d, "ann"));
    assert_eq!(a.profile_store().on_sign_out(&d, "ann"), Ok(false));
    assert!(a.profile_store().import_sealed(&d, "ann", &d.join("none"), "not a key", 1).is_err(), "a key that is no key is refused before any file is touched");
    // a wrong handle is refused, not a crash
    assert!(a.profile_store().protect_for(&(), "ann").is_err());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_real_ntp_facet_is_the_modules_status() {
    let a = AgentApiImpl;
    a.ntp().reset_status();
    assert_eq!(a.ntp().status(), crate::ntp::status());
}

#[test]
fn the_real_keyboard_facet_reads_and_writes_the_kernels_files_under_a_made_up_sys() {
    let d = tmp("kbd");
    let a = AgentApiImpl;
    assert!(!a.keyboard_apple().apple_state(&d).driver, "no driver folder, no driver");
    let p = crate::keyboard::apple_param(&d, "fnmode");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, "1\n").unwrap();
    assert!(a.keyboard_apple().apple_state(&d).driver);
    let k = KeyboardSettings { fn_mode: "fkeys".into(), ..Default::default() };
    let done = a.keyboard_apple().apply_apple(&d, &k).unwrap();
    assert_eq!(done, vec!["hid_apple fnmode = 2".to_string()]);
    assert_eq!(std::fs::read_to_string(&p).unwrap().trim(), "2");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_real_web_graphics_facet_keeps_the_choice_in_the_data_folder() {
    let d = tmp("graphics");
    let a = AgentApiImpl;
    assert_eq!(a.web_graphics().load_mode(&d).0, Mode::Automatic);
    a.web_graphics().set_mode(&d, Mode::Software).unwrap();
    assert_eq!(a.web_graphics().load_mode(&d).0, Mode::Software);
    assert_eq!(crate::webgraphics::load_mode(&d).0, Mode::Software);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_real_proc_owner_facet_runs_the_closure_for_the_person() {
    let a = AgentApiImpl;
    let mut seen = None;
    a.proc_owner().as_user("ann", &mut || seen = crate::procowner::current_user());
    assert_eq!(seen.as_deref(), Some("ann"));
}

#[test]
fn the_real_dict_fix_facet_refuses_a_handle_that_is_not_the_services() {
    let a = AgentApiImpl;
    assert_eq!(a.dict_fix().target(&()), iyakki_shared::dictfix::Target::NoModel);
    assert!(a.dict_fix().set_enabled(&(), true).is_err());
}

#[test]
fn the_real_tool_wrap_leaves_the_tools_alone_for_a_wrong_handle() {
    let a = AgentApiImpl;
    let inner: Arc<dyn iyakki_shared::agent::Tools> = Arc::new(crate::extagent::NoTools);
    let wrapped = a.tool_wraps().linux_appimage_tools(inner.clone(), Arc::new(()), iyakki_sec::policy::Role::User.template(), "ann".into(), Arc::new(|_, _| {}));
    assert_eq!(wrapped.defs().len(), inner.defs().len(), "no services handle, no extra tools");
}

#[test]
fn the_lemonade_choice_is_the_modules() {
    let d = tmp("lemonade");
    let a = AgentApiImpl;
    assert_eq!(a.lemonade_compute().lemonade_compute(&d), crate::gpu_steps::lemonade_compute(&d));
    let _ = std::fs::remove_dir_all(&d);
    let _: &Path = Path::new("/");
}

#[test]
fn the_real_cluster_facet_switches_through_the_registry_and_mirrors_the_choice() {
    let d = tmp("clusters");
    let a = AgentApiImpl;
    let role = iyakki_sec::policy::Role::Admin;
    let e = a.cluster_registry().switch(&d, &d, "ann", role, "local").unwrap();
    assert_eq!(e.id, "local");
    assert_eq!(crate::clusters::Registry::load(&d, "ann").active, "local");
    assert!(a.cluster_registry().switch(&d, &d, "ann", iyakki_sec::policy::Role::Guest, "local").is_err(), "a guest cannot switch");
    assert!(a.cluster_registry().switch(&d, &d, "ann", role, "nope").is_err());
    assert_eq!(a.cluster_registry().gpu_summary("never-seen"), None);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_real_gpu_page_facet_reads_files_under_a_made_up_root_and_ignores_a_wrong_handle() {
    let d = tmp("gpupage");
    let a = AgentApiImpl;
    let r = a.gpu_page().quick_report(&d, &d);
    assert!(r.gpus.is_empty(), "no PCI devices in an empty root");
    assert_eq!(r.pods_note, "looking...");
    a.gpu_page().refresh_async(Arc::new(()));
    let _ = std::fs::remove_dir_all(&d);
}
