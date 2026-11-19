//! The trace of the first screens (F-SPLIT-03, docs/development/UI-RPC-DESIGN.md section 6): a recording `ServicesHandle` sits under the shell while it draws
//! the launcher and opens the Settings overview, and the exact list of `ServicesHandle` methods those two screens call is compared with the list the
//! remote handle has to serve first. A method that appears here and is not in the list fails the test (the list only grows by decision); a method in
//! the list that the screens no longer call also fails, so the list never goes stale.

use super::tests::*;
use super::*;
use iyakki_agent_api::RecordingServices;

/// Calls on the handles these methods return (`b4()`, `radio()`, `mcp()`, `store_facets()`) and on the members (`settings()`, `screen()`, `camera()`) are not
/// counted here: they are the facet traits and member-store traits the remote handle serves next to these.
/// The `ServicesHandle` methods the launcher and the Settings overview call, sorted. Update by running the test: it prints the new list.
const FIRST_SCREENS: &[&str] = &["b4", "camera", "dir", "mcp", "radio", "screen", "settings", "sound_settings", "store_facets"];

fn trace() -> std::collections::BTreeMap<String, usize> {
    let (mut sh, _) = shell_with_users("trace");
    login(&mut sh, "root", "rootpass12");
    assert_eq!(sh.screen_name(), "launcher");
    let rec = Arc::new(RecordingServices::new(sh.env.services.clone()));
    sh.env.services = rec.clone();
    let mut g = Gfx::new(1280, 720);
    for _ in 0..3 {
        sh.render(&mut g);
    }
    assert!(sh.open_for_demo("settings"));
    for _ in 0..3 {
        sh.render(&mut g);
    }
    rec.tally()
}

#[test]
fn the_launcher_and_the_settings_overview_call_exactly_these_services_methods() {
    let t = trace();
    let names: Vec<&str> = t.keys().map(String::as_str).collect();
    println!("TRACE {} methods: {:?}", names.len(), names);
    assert_eq!(names, FIRST_SCREENS, "the screens call a different set of services methods than the remote handle serves first");
}
