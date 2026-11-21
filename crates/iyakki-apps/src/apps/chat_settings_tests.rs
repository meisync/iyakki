//! The shell-side Settings chat tools over a temporary data folder: policy (reads free, changes ask every time), the same files and
//! stores the pages use, and the secret rule for custom headers.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use iyakki_shared::agent::{Risk};
use iyakki_shared::agent::{Tools};
use iyakki_shared::browserheaders::{HeaderRule};
use iyakki_agent::browserheaders::{Store, Values};
use iyakki_shared::settingstools::{Entry};
use iyakki_shared::settingstools::{SettingsTools};
use iyakki_agent::settingstools::{Env};
use serde_json::json;

use super::*;

struct NoTools;
impl Tools for NoTools {
    fn defs(&self) -> Vec<iyakki_shared::agent::ToolDef> {
        vec![]
    }
    fn call(&self, name: &str, _args: &serde_json::Value) -> Result<String, String> {
        Err(format!("no tool {name}"))
    }
    fn describe(&self, name: &str, _args: &serde_json::Value) -> String {
        name.to_string()
    }
}

struct Mem(Mutex<HashMap<String, String>>);
impl Values for Mem {
    fn get(&self, id: &str) -> Result<Option<String>, String> {
        Ok(self.0.lock().unwrap().get(id).cloned())
    }
    fn set(&self, id: &str, v: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(id.into(), v.into());
        Ok(())
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(id);
        Ok(())
    }
}

fn env(tag: &str, admin: bool) -> (Env, Arc<Mutex<Vec<String>>>) {
    let dir = std::env::temp_dir().join(format!("iyakki-chatsettings-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let audits = Arc::new(Mutex::new(Vec::new()));
    let a2 = audits.clone();
    let settings = Arc::new(Mutex::new(iyakki_shared::settings::SettingsStore::open(&dir.join("settings.json")).unwrap()));
    (Env { dir, user: "ada".into(), admin, settings: iyakki_shared::settings_ref::SettingsRef::of_store(&settings), audit: Arc::new(move |a, d| a2.lock().unwrap().push(format!("{a} {d}"))) }, audits)
}

fn tools(e: &Env, entries: Vec<Entry>) -> SettingsTools {
    SettingsTools::new(Arc::new(NoTools), e.admin, entries)
}

fn policy(t: &SettingsTools, reads: &[&str], changes: &[&str]) {
    let defs = t.defs();
    for n in reads {
        assert_eq!(defs.iter().find(|d| d.spec.name == *n).unwrap_or_else(|| panic!("{n}")).risk, Risk::Safe);
    }
    for n in changes {
        assert_eq!(defs.iter().find(|d| d.spec.name == *n).unwrap_or_else(|| panic!("{n}")).risk, Risk::Ask);
        assert!(t.grant_blocker(n, &json!({})).is_some() && !t.approved_always(n), "{n}");
    }
}

#[test]
fn keyboard_choices_are_checked_saved_in_the_machine_settings_and_applied() {
    let (e, audits) = env("kbd", true);
    let sys = e.dir.join("sys");
    std::fs::create_dir_all(&sys).unwrap();
    let t = tools(&e, keyboard_entries(&e, sys));
    policy(&t, &["keyboard_status"], &["keyboard_set"]);
    assert!(t.refusal("keyboard_set", &json!({"repeat_delay_ms": 5})).is_some(), "out of range");
    assert!(t.refusal("keyboard_set", &json!({"profile": "dvorak"})).is_some());
    assert!(t.call("keyboard_set", &json!({})).is_err());
    assert!(t.preview("keyboard_set", &json!({"repeat_rate": 20})).unwrap().contains("20 per second"));
    let out = t.call("keyboard_set", &json!({"repeat_delay_ms": 300, "repeat_rate": 20, "trackpad_natural": false})).unwrap();
    assert!(out.contains("delay 300 ms, rate 20"), "{out}");
    let k = e.settings.lock().unwrap().get().keyboard.clone();
    assert_eq!((k.repeat_delay_ms, k.repeat_rate, k.trackpad_natural), (300, 20, Some(false)));
    assert!(t.call("keyboard_status", &json!({})).unwrap().contains("rate 20"));
    assert!(t.call("keyboard_status", &json!({})).unwrap().contains("three-finger drag: off"));
    t.call("keyboard_set", &json!({"three_finger_drag": true})).unwrap();
    assert!(e.settings.lock().unwrap().get().keyboard.three_finger_drag_on());
    assert!(t.call("keyboard_status", &json!({})).unwrap().contains("three-finger drag: on"));
    t.call("keyboard_set", &json!({"three_finger_drag": false})).unwrap();
    // the Home swipe: on with three fingers until chosen otherwise; checked, previewed, saved, applied and audited like the rest
    assert!(t.call("keyboard_status", &json!({})).unwrap().contains("Home swipe between groups: on (3 fingers"));
    assert!(t.refusal("keyboard_set", &json!({"home_swipe_fingers": 4})).is_some(), "only two or three fingers");
    assert!(t.refusal("keyboard_set", &json!({"home_swipe_fingers": "all"})).is_some());
    assert!(t.preview("keyboard_set", &json!({"home_swipe": false})).unwrap().contains("Home swipe between groups: off"));
    t.call("keyboard_set", &json!({"home_swipe": false, "home_swipe_fingers": 2})).unwrap();
    let k = e.settings.lock().unwrap().get().keyboard.clone();
    assert!(!k.home_swipe_on() && k.home_swipe_fingers_n() == 2);
    assert!(t.call("keyboard_status", &json!({})).unwrap().contains("Home swipe between groups: off (2 fingers"));
    t.call("keyboard_set", &json!({"home_swipe": true, "home_swipe_fingers": 3})).unwrap();
    assert!(audits.lock().unwrap().iter().any(|a| a.starts_with("settings.keyboard")));
    let _ = std::fs::remove_dir_all(&e.dir);
}

#[test]
fn pointer_speed_is_read_free_changed_by_asking_checked_saved_applied_and_audited() {
    let _shared = crate::apps::settings::live_input(); // reads the process-wide input state after the tool applied it: holds the input lock
    let (e, audits) = env("pointer", true);
    let t = tools(&e, pointer_entries(&e));
    policy(&t, &["pointer_speed_status"], &["pointer_speed_set"]);
    assert!(t.call("pointer_speed_status", &json!({})).unwrap().contains("Mouse speed: 100%. Touchpad speed: 100%"));
    for bad in [json!({"mouse_pct": 24}), json!({"mouse_pct": 301}), json!({"touchpad_pct": 0}), json!({"mouse_pct": "fast"}), json!({"mouse_pct": -5})] {
        assert!(t.refusal("pointer_speed_set", &bad).is_some(), "{bad}");
    }
    assert!(t.call("pointer_speed_set", &json!({})).is_err(), "nothing named");
    assert!(t.preview("pointer_speed_set", &json!({"mouse_pct": 150})).unwrap().contains("Mouse speed: 150%"));
    let out = t.call("pointer_speed_set", &json!({"mouse_pct": 150, "touchpad_pct": 80})).unwrap();
    assert!(out.contains("Mouse speed: 150%") && out.contains("Touchpad speed: 80%"), "{out}");
    let k = e.settings.lock().unwrap().get().keyboard.clone();
    assert_eq!((k.pointer_speed_pct, k.touchpad_speed_pct), (150, 80));
    assert_eq!((crate::input::mouse_speed_pct(), crate::input::pad_config().speed_pct), (150, 80), "applied at once");
    crate::input::request_pointer_speed(100, 100);
    assert!(audits.lock().unwrap().iter().any(|a| a.starts_with("settings.pointer_speed")));
    let _ = std::fs::remove_dir_all(&e.dir);
}

#[test]
fn time_zone_and_clock_face_changes_ask_and_bad_zones_are_refused() {
    let (e, audits) = env("time", true);
    let t = tools(&e, time_entries(&e));
    policy(&t, &["time_status"], &["time_set", "clock_face_set"]);
    assert!(t.refusal("time_set", &json!({"zone": "Mars/Olympus"})).is_some());
    assert!(t.call("time_set", &json!({})).is_err());
    let out = t.call("time_set", &json!({"zone": "Europe/Paris", "set_from_network": false})).unwrap();
    assert!(out.contains("Europe/Paris") && out.contains("network: no"), "{out}");
    assert_eq!(e.settings.lock().unwrap().get().time.zone, "Europe/Paris");
    assert!(!e.settings.lock().unwrap().get().time.zone_auto, "a chosen zone switches detection off");
    t.call("clock_face_set", &json!({"hour12": true})).unwrap();
    assert_eq!(iyakki_uikit::uisize::load_clock(&e.dir, "ada"), (true, false));
    assert!(audits.lock().unwrap().iter().any(|a| a.starts_with("settings.time ")));
    // a person who is not an administrator sees only the clock face and the status of the zone
    let (p, _) = env("time2", false);
    let t2 = tools(&p, time_entries(&p));
    assert!(t2.call("time_set", &json!({"zone": "Europe/Paris"})).is_err());
    assert!(t2.defs().iter().all(|d| d.spec.name != "time_set"));
    let _ = std::fs::remove_dir_all(&e.dir);
    let _ = std::fs::remove_dir_all(&p.dir);
}

#[test]
fn header_rules_show_no_value_and_cannot_be_added_by_chat() {
    let (e, _) = env("hdr", false);
    let values = Arc::new(Mem(Mutex::new(HashMap::new())));
    let store = Store::new(&crate::apps::browser_headers::rules_path_for(&e.dir, "ada"), values.clone());
    let rule = HeaderRule { name: "Staging token".into(), header: "X-Api-Key".into(), domains: "api.example.org".into(), enabled: true, ..Default::default() };
    let saved = store.add(rule, "s3cret-value").unwrap();
    let t = tools(&e, header_entries(&e, values.clone()));
    policy(&t, &["browser_headers_list"], &["browser_headers_enable", "browser_headers_delete"]);
    assert!(t.defs().iter().all(|d| !d.spec.name.contains("add")), "no way to add one with a value");
    let list = t.call("browser_headers_list", &json!({})).unwrap();
    assert!(list.contains("Staging token") && list.contains("********") && !list.contains("s3cret"), "{list}");
    t.call("browser_headers_enable", &json!({"rule": "staging token", "on": false})).unwrap();
    assert!(!store.list()[0].enabled);
    assert!(t.call("browser_headers_enable", &json!({"rule": "nope", "on": true})).is_err());
    t.call("browser_headers_delete", &json!({"rule": saved.id})).unwrap();
    assert!(store.list().is_empty());
    assert!(values.0.lock().unwrap().is_empty(), "the vault value goes with the rule");
    let _ = std::fs::remove_dir_all(&e.dir);
}

#[test]
fn browser_plugins_are_added_off_switched_on_and_removed_by_chat_with_asks_and_none_by_default() {
    let (e, audits) = env("plug", false);
    let t = tools(&e, plugin_entries(&e));
    policy(&t, &["browser_plugins_list"], &["browser_plugin_add", "browser_plugin_enable", "browser_plugin_remove"]);
    assert!(t.call("browser_plugins_list", &json!({})).unwrap().contains("No browser plug-ins"));
    // a script with no sites, or one for a whole domain ending, is refused
    assert!(t.call("browser_plugin_add", &json!({"name": "A", "kind": "script", "text": "1"})).is_err());
    assert!(t.call("browser_plugin_add", &json!({"name": "A", "kind": "script", "sites": ["*.com"], "text": "1"})).is_err());
    assert!(t.call("browser_plugin_add", &json!({"name": "A", "kind": "nope", "text": "1"})).is_err());
    t.call("browser_plugin_add", &json!({"name": "Dark", "kind": "style", "sites": ["example.com"], "text": "body{background:#000}"})).unwrap();
    let list = t.call("browser_plugins_list", &json!({})).unwrap();
    assert!(list.contains("Dark") && list.contains("off") && list.contains("example.com"), "{list}");
    t.call("browser_plugin_enable", &json!({"plugin": "dark", "on": true})).unwrap();
    assert!(t.call("browser_plugins_list", &json!({})).unwrap().contains("(on"));
    assert!(t.call("browser_plugin_enable", &json!({"plugin": "nope", "on": true})).is_err());
    t.call("browser_plugin_remove", &json!({"plugin": "pl1"})).unwrap();
    assert!(t.call("browser_plugins_list", &json!({})).unwrap().contains("No browser plug-ins"));
    assert!(audits.lock().unwrap().iter().filter(|a| a.starts_with("settings.browser.plugins")).count() >= 3);
    let _ = std::fs::remove_dir_all(&e.dir);
}

#[test]
fn home_layout_groups_are_made_renamed_deleted_and_the_layout_reset() {
    let (e, _) = env("layout", false);
    let t = tools(&e, layout_entries(&e));
    policy(&t, &["home_layout_list"], &["home_layout_group"]);
    t.call("home_layout_group", &json!({"action": "create", "name": "Garden"})).unwrap();
    assert!(Layout::load(&e.dir, "ada").groups.iter().any(|g| g.label == "Garden"));
    assert!(t.call("home_layout_group", &json!({"action": "create", "name": "garden"})).is_err(), "name taken");
    t.call("home_layout_group", &json!({"action": "rename", "group": "Garden", "name": "Yard"})).unwrap();
    assert!(t.call("home_layout_list", &json!({})).unwrap().contains("Yard"));
    assert!(t.call("home_layout_group", &json!({"action": "rename", "group": "home", "name": "X"})).is_err(), "built-in groups keep their names");
    t.call("home_layout_group", &json!({"action": "delete", "group": "Yard"})).unwrap();
    assert!(Layout::load(&e.dir, "ada").groups.is_empty());
    t.call("home_layout_group", &json!({"action": "reset"})).unwrap();
    assert!(t.call("home_layout_group", &json!({"action": "explode"})).is_err());
    let _ = std::fs::remove_dir_all(&e.dir);
}

#[test]
fn only_an_administrator_sets_or_clears_the_default_layout_for_new_accounts_by_chat() {
    let (e, _) = env("layoutdef-user", false);
    let t = tools(&e, layout_entries(&e));
    policy(&t, &["home_layout_list"], &["home_layout_group"]);
    assert!(t.call("home_layout_group", &json!({"action": "set_default_for_new_accounts"})).is_err());
    assert!(!Layout::has_default(&e.dir));
    let _ = std::fs::remove_dir_all(&e.dir);
    let (e, _) = env("layoutdef-admin", true);
    let t = tools(&e, layout_entries(&e));
    policy(&t, &["home_layout_list"], &["home_layout_group"]);
    t.call("home_layout_group", &json!({"action": "create", "name": "Garden"})).unwrap();
    t.call("home_layout_group", &json!({"action": "set_default_for_new_accounts"})).unwrap();
    assert_eq!(Layout::load(&e.dir, "someone-new").groups[0].label, "Garden");
    t.call("home_layout_group", &json!({"action": "clear_default_for_new_accounts"})).unwrap();
    assert!(Layout::load(&e.dir, "someone-new").is_default());
    let _ = std::fs::remove_dir_all(&e.dir);
}

#[test]
fn ads_and_trackers_tools_show_the_state_change_it_like_the_page_and_refuse_bad_input() {
    use super::super::browser_block as bb;
    let cx = crate::apps::tests::cx_with_admin_store("chatblk1");
    let (e, audits) = env("chatblk1", true);
    let t = tools(&e, block_entries(&cx, &e));
    policy(&t, &["browser_block_status"], &["browser_block_set", "browser_block_download", "browser_block_update", "browser_block_remove", "browser_block_allow"]);
    // nothing downloaded: the status says so and nothing is blocked
    let st = t.call("browser_block_status", &json!({})).unwrap();
    assert!(st.contains("is on") && st.contains("No filter list is downloaded"), "{st}");
    // the switch is the page's switch, and is audited
    t.call("browser_block_set", &json!({"on": false})).unwrap();
    assert!(!bb::load(&cx).enabled);
    assert!(t.call("browser_block_status", &json!({})).unwrap().contains("is off"));
    t.call("browser_block_set", &json!({"on": true})).unwrap();
    assert!(bb::load(&cx).enabled);
    assert!(t.call("browser_block_set", &json!({})).is_err());
    // a site is allowed and blocked again; bad host names are refused
    t.call("browser_block_allow", &json!({"site": "Example.ORG", "allow": true})).unwrap();
    assert!(bb::load(&cx).allow.contains("example.org"));
    assert!(t.call("browser_block_status", &json!({})).unwrap().contains("Allowed sites: example.org"));
    t.call("browser_block_allow", &json!({"site": "example.org", "allow": false})).unwrap();
    assert!(bb::load(&cx).allow.is_empty());
    assert!(t.call("browser_block_allow", &json!({"site": "bad host/../", "allow": true})).is_err());
    assert!(t.call("browser_block_allow", &json!({"site": "", "allow": true})).is_err());
    // a download needs a known list or an https address; nothing is started for bad input
    assert!(t.call("browser_block_download", &json!({})).is_err());
    assert!(t.call("browser_block_download", &json!({"list": "nonsense"})).is_err());
    assert!(t.call("browser_block_download", &json!({"url": "http://insecure.example/list.txt"})).is_err());
    assert!(bb::job_status().is_empty() || bb::job_status().starts_with("Done"), "no job was started by the refused calls");
    // the card names the address or the list
    assert!(t.describe("browser_block_download", &json!({"url": "https://lists.example/x.txt"})).contains("https://lists.example/x.txt"));
    assert!(t.describe("browser_block_download", &json!({"list": "ublock-filters"})).contains("ublock-filters"));
    // removing with nothing downloaded says so; with a list on disk it removes it
    assert_eq!(t.call("browser_block_remove", &json!({})).unwrap(), "There were no lists.");
    let src = std::sync::Arc::new(OneList);
    let store = bb::store(&cx);
    assert!(bb::run_job(&store, src.as_ref(), &bb::Job::AddKnown("ublock-filters".into()), 5).starts_with("added uBlock filters"));
    let st = t.call("browser_block_status", &json!({})).unwrap();
    assert!(st.contains("uBlock filters [ublock-filters]") && st.contains("1 rules") && st.contains("GPL-3.0") && st.contains("raw.githubusercontent.com"), "{st}");
    assert_eq!(t.call("browser_block_remove", &json!({})).unwrap(), "Removed 1 list.");
    assert!(store.list().is_empty());
    let log = audits.lock().unwrap().join("\n");
    assert!(log.contains("Block ads and trackers: off") && log.contains("example.org allowed") && log.contains("Removed the filter lists"), "{log}");
}

struct OneList;
impl iyakki_web::blocker_lists::ListSource for OneList {
    fn get(&self, _url: &str, _max: usize) -> Result<Vec<u8>, String> {
        Ok(b"||ads.example.com^\n".to_vec())
    }
}

#[test]
fn the_display_tools_are_free_and_look_again_reads_a_display_with_no_usable_edid() {
    let (e, _) = env("display", false);
    let drm = e.dir.join("drm");
    let c = drm.join("card0-DP-1");
    std::fs::create_dir_all(&c).unwrap();
    std::fs::write(c.join("status"), "connected\n").unwrap();
    std::fs::write(c.join("edid"), b"").unwrap();
    std::fs::write(c.join("modes"), "1920x1080\n640x480\n").unwrap();
    let t = tools(&e, display_entries(&e, drm));
    policy(&t, &["displays_status", "displays_look_again"], &[]);
    assert_eq!(t.call("displays_status", &json!({})).unwrap(), "DP-1: no usable EDID");
    assert!(t.call("displays_look_again", &json!({})).unwrap().contains("Looking at the displays again: DP-1: no usable EDID"));
    let _ = std::fs::remove_dir_all(&e.dir);
}

#[test]
fn the_screen_size_tool_asks_checks_the_size_against_what_the_display_offers_saves_it_per_person_and_turns_on_driving_one_screen() {
    use crate::memgate::{read_switch, Switch};
    let (e, audits) = env("dispsize", true);
    let drm = e.dir.join("drm");
    let c = drm.join("card0-HDMI-A-1");
    std::fs::create_dir_all(&c).unwrap();
    std::fs::write(c.join("status"), "connected\n").unwrap();
    std::fs::write(c.join("edid"), b"").unwrap();
    std::fs::write(c.join("modes"), "1920x1080\n1280x720\n").unwrap();
    let t = tools(&e, display_entries(&e, drm.clone()));
    policy(&t, &["displays_status"], &["displays_size_set"]);
    // refused: no such display, a size it does not offer; nothing is saved and the switch stays
    let e1 = t.call("displays_size_set", &json!({"display": "DP-9", "width": 1280, "height": 720})).unwrap_err();
    assert!(e1.contains("no connected display") && e1.contains("HDMI-A-1"), "{e1}");
    let e2 = t.call("displays_size_set", &json!({"display": "HDMI-A-1", "width": 3840, "height": 2160})).unwrap_err();
    assert!(e2.contains("does not offer 3840x2160") && e2.contains("1920x1080, 1280x720"), "{e2}");
    assert_eq!(crate::display::outputs::load(&e.dir, "ada").mode_of("HDMI-A-1"), None);
    assert_eq!(read_switch(&e.dir), Switch::Auto);
    // accepted: kept for this person, one screen so the shell is asked to drive it, logged
    let ok = t.call("displays_size_set", &json!({"display": "hdmi-a-1", "width": 1280, "height": 720})).unwrap();
    assert!(ok.contains("Saved: HDMI-A-1 at 1280x720") && ok.contains("Always on"), "{ok}");
    assert_eq!(crate::display::outputs::load(&e.dir, "ada").mode_of("HDMI-A-1"), Some((1280, 720)));
    assert_eq!(crate::display::outputs::load(&e.dir, "bob").mode_of("HDMI-A-1"), None, "another person is untouched");
    assert_eq!(read_switch(&e.dir), Switch::On);
    assert!(audits.lock().unwrap().iter().any(|a| a == "settings.display.resolution HDMI-A-1 1280x720"));
    // switched off on this computer: refused with the way out
    crate::memgate::write_switch(&e.dir, Switch::Off).unwrap();
    assert!(t.call("displays_size_set", &json!({"display": "HDMI-A-1", "width": 1920, "height": 1080})).unwrap_err().contains("switched off"));
    // an account that is not an administrator never reaches it
    let (e2, _) = env("dispsize2", false);
    let t2 = tools(&e2, display_entries(&e2, drm));
    assert!(t2.call("displays_size_set", &json!({"display": "HDMI-A-1", "width": 1280, "height": 720})).unwrap_err().contains("administrator"));
    let _ = std::fs::remove_dir_all(&e.dir);
    let _ = std::fs::remove_dir_all(&e2.dir);
}

#[test]
fn known_bad_site_and_certificate_tools_show_change_and_refuse_bad_input() {
    let _turn = CERT_FAKE_TURN.lock().unwrap_or_else(|e| e.into_inner());
    use super::super::browser_badsites as bs;
    let _turn = bs::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let cx = crate::apps::tests::cx_with_admin_store("chatbad1");
    let (e, audits) = env("chatbad1", true);
    let t = tools(&e, safe_browsing_entries(&cx, &e));
    let _ = std::fs::remove_file(bs::list_path(&cx));
    let _pin = bs::Pin::to(&cx);
    policy(&t, &["browser_badsites_status", "browser_certificate_view"], &["browser_badsites_set", "browser_badsites_add", "browser_badsites_remove", "browser_badsites_download"]);
    let st = t.call("browser_badsites_status", &json!({})).unwrap();
    assert!(st.contains("is on and holds 0 sites") && st.contains("urlhaus"), "{st}");
    t.call("browser_badsites_add", &json!({"site": "Evil.EXAMPLE", "reason": "phishing"})).unwrap();
    assert!(bs::load_list(&cx).check_host("a.evil.example").is_some());
    assert!(Guard_listed("a.evil.example"), "the shared guard has it at once");
    assert!(t.call("browser_badsites_status", &json!({})).unwrap().contains("- evil.example: phishing"));
    assert!(t.call("browser_badsites_add", &json!({"site": "http://evil.example/x"})).is_err());
    assert!(t.call("browser_badsites_add", &json!({"site": "com"})).is_err());
    assert!(t.call("browser_badsites_add", &json!({})).is_err());
    t.call("browser_badsites_set", &json!({"on": false})).unwrap();
    assert!(!Guard_listed("a.evil.example") && t.call("browser_badsites_status", &json!({})).unwrap().contains("is off"));
    t.call("browser_badsites_set", &json!({"on": true})).unwrap();
    assert!(t.call("browser_badsites_remove", &json!({"site": "nothere.example"})).is_err());
    t.call("browser_badsites_remove", &json!({"site": "evil.example"})).unwrap();
    assert!(!Guard_listed("a.evil.example"));
    t.call("browser_badsites_add", &json!({"site": "x.example"})).unwrap();
    assert!(t.call("browser_badsites_remove", &json!({"site": "all"})).unwrap().contains("1 removed"));
    // downloads need a known list or an https address
    assert!(t.call("browser_badsites_download", &json!({})).is_err());
    assert!(t.call("browser_badsites_download", &json!({"list": "nonsense"})).is_err());
    assert!(t.call("browser_badsites_download", &json!({"url": "http://insecure.example/l.txt"})).is_err());
    // the certificate view: refuses bad names and ports, shows what the probe found
    assert!(t.call("browser_certificate_view", &json!({"host": "bad host/../"})).is_err());
    assert!(t.call("browser_certificate_view", &json!({"host": "a.example", "port": 0})).is_err());
    assert!(t.call("browser_certificate_view", &json!({"host": "a.example"})).is_err(), "no probe result in a test: an error, not a panic");
    *CERT_FAKE.lock().unwrap() = Some(iyakki_proxy::certview::View { host: "shop.example".into(), subject: "CN=shop.example".into(), issuer: "CN=Some Authority".into(), not_before: "2026-01-01".into(), not_after: "2027-01-01".into(), days_left: 90, sha256: "AA:BB".into(), names_match: true, trust: Err("UnknownIssuer".into()), sent: 1, self_issued: false, warnings: vec!["It does not lead to a trusted authority (UnknownIssuer).".into()] });
    let out = t.call("browser_certificate_view", &json!({"host": "Shop.Example."})).unwrap();
    *CERT_FAKE.lock().unwrap() = None;
    assert!(out.contains("Issued by: CN=Some Authority") && out.contains("Trusted authority: NO") && out.contains("Warnings:"), "{out}");
    assert!(audits.lock().unwrap().iter().any(|a| a.contains("Known-bad site list")));
    let _ = std::fs::remove_file(bs::list_path(&cx));
    bs::forget_loaded();
}

#[allow(non_snake_case)]
fn Guard_listed(host: &str) -> bool {
    iyakki_web::badsites::Guard::global().check(host).is_some()
}
