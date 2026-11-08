//! Tests for `hwprofile` and `thiscomputertools` with fake hardware facts (no real DMI is read, no second real computer was used).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use iyakki_hwinfo::DmiIdentity;
use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::hwprofile::*;
use crate::services::Services;
use crate::settings::SettingsStore;
use crate::thiscomputertools as tools;

struct Fake(DmiIdentity);
impl HardwareFacts for Fake {
    fn dmi(&self) -> DmiIdentity {
        self.0.clone()
    }
}

fn dmi(v: &str, p: &str, ver: &str, b: &str) -> Fake {
    let o = |s: &str| if s.is_empty() { None } else { Some(s.to_string()) };
    Fake(DmiIdentity { sys_vendor: o(v), product_name: o(p), product_version: o(ver), board_name: o(b) })
}
fn mac() -> Fake {
    dmi("Apple Inc.", "Macmini8,1", "1.0", "Mac-7BA5B2DFE22DDD8C")
}
fn legion() -> Fake {
    dmi("LENOVO", "82WM", "Legion Pro 5 16ARX8", "LNVNB161216")
}
fn qemu() -> Fake {
    dmi("QEMU", "Standard PC (Q35 + ICH9, 2009)", "pc-q35-8.2", "")
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-hwprofile-{}-{}", std::process::id(), tag));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A settings store attached to `facts` in `dir`.
fn store(dir: &PathBuf, facts: &Fake, now: i64) -> SettingsStore {
    let mut st = SettingsStore::open(&dir.join("settings.json")).unwrap();
    let c = Computers::open(dir, facts, now, st.get());
    st.attach_computers(c);
    st
}

#[test]
fn hwprofile_the_identity_is_stable_differs_per_machine_and_a_vm_has_its_own() {
    let (a, b) = (identify(&mac()), identify(&mac()));
    assert_eq!(a, b);
    assert_eq!(a.id.len(), 10);
    assert_eq!(a.label, "Apple Macmini8,1");
    let l = identify(&legion());
    assert_eq!(l.label, "LENOVO 82WM Legion Pro 5 16ARX8");
    let q = identify(&qemu());
    assert!(a.id != l.id && a.id != q.id && l.id != q.id);
    assert_eq!(identify(&dmi("", "", "", "")).label, "This computer (no hardware name)");
    // vendor placeholders and spacing do not change the key
    assert_eq!(identify(&dmi("LENOVO", "82WM", "Legion  Pro 5 16ARX8", "")).id, identify(&dmi("lenovo", "82wm", "Legion Pro 5 16ARX8", "To Be Filled By O.E.M.")).id);
}

#[test]
fn hwprofile_the_pointer_table_is_valid_against_the_default_settings() {
    let v = serde_json::to_value(crate::settings::Settings::default()).unwrap();
    let mut seen = std::collections::BTreeSet::new();
    for i in PER_COMPUTER {
        assert!(v.pointer(i.pointer).is_some(), "{} is not in the default settings", i.pointer);
        assert!(seen.insert(i.pointer), "{} is listed twice", i.pointer);
        assert!(!i.why.is_empty());
    }
    // what stays with the person is not in the table
    for person in ["/chat", "/providers", "/voice/stt", "/voice/tts", "/time", "/sound/source_levels", "/vault", "/update_check"] {
        assert!(!PER_COMPUTER.iter().any(|i| i.pointer == person), "{person} belongs to the person");
    }
    assert_eq!(default_values().len(), PER_COMPUTER.len());
}

#[test]
fn hwprofile_the_first_computer_adopts_the_settings_and_the_overlay_is_applied_at_load() {
    let d = tmp("overlay");
    {
        let mut st = SettingsStore::open(&d.join("settings.json")).unwrap();
        st.update(|s| {
            s.sound.output = "PCH:3".into();
            s.keyboard.profile = "mac".into();
        })
        .unwrap();
    }
    let st = store(&d, &mac(), 100);
    assert_eq!(st.get().sound.output, "PCH:3");
    assert!(!st.computers().unwrap().new_computer, "the first computer is not announced");
    // a profile now exists; hand-edit the profile and the next start puts it over the global file
    let c = st.computers().unwrap().clone();
    let mut p = c.profile().unwrap();
    assert_eq!((p.boots, p.first_seen), (1, 100));
    p.values.insert("/sound/volume_pct".into(), json!(33));
    std::fs::write(c.dir().join(format!("{}.json", p.id)), serde_json::to_string(&p).unwrap()).unwrap();
    let st2 = store(&d, &mac(), 200);
    assert_eq!(st2.get().sound.volume_pct, 33);
    assert_eq!(st2.computers().unwrap().profile().unwrap().boots, 2);
    assert_eq!(std::fs::read_to_string(d.join("computers/current")).unwrap().trim(), identify(&mac()).id);
}

#[test]
fn hwprofile_a_per_computer_change_goes_to_the_profile_and_a_person_change_does_not() {
    let d = tmp("record");
    let mut st = store(&d, &mac(), 1);
    st.update(|s| s.sound.volume_pct = 21).unwrap();
    st.update(|s| s.time.zone = "Europe/Paris".into()).unwrap();
    let p = st.computers().unwrap().profile().unwrap();
    assert_eq!(p.values["/sound/volume_pct"], json!(21));
    assert!(!p.values.keys().any(|k| k.starts_with("/time")), "the time zone is the person's");
    // the global file keeps the last value as the fallback
    let g = SettingsStore::open(&d.join("settings.json")).unwrap();
    assert_eq!(g.get().sound.volume_pct, 21);
    assert_eq!(g.get().time.zone, "Europe/Paris");
}

#[test]
fn hwprofile_a_new_computer_starts_from_defaults_and_switching_back_restores() {
    let d = tmp("switch");
    let mut st = store(&d, &mac(), 1);
    st.update(|s| {
        s.sound.output = "PCH:3".into();
        s.sound.volume_pct = 40;
        s.keyboard.profile = "mac".into();
        s.time.zone = "Asia/Kolkata".into();
    })
    .unwrap();
    drop(st);
    let mut lg = store(&d, &legion(), 2);
    assert!(lg.computers().unwrap().new_computer);
    assert_eq!(lg.get().sound.output, "", "sound output automatic, not the Mac's card");
    assert_eq!(lg.get().keyboard.profile, "");
    assert_eq!(lg.get().sound.volume_pct, crate::settings::Settings::default().sound.volume_pct);
    assert_eq!(lg.get().time.zone, "Asia/Kolkata", "the time zone follows the person");
    lg.update(|s| s.sound.output = "HDMI:0".into()).unwrap();
    drop(lg);
    let m = store(&d, &mac(), 3);
    assert!(!m.computers().unwrap().new_computer);
    assert_eq!((m.get().sound.output.as_str(), m.get().sound.volume_pct, m.get().keyboard.profile.as_str()), ("PCH:3", 40, "mac"));
    let l2 = store(&d, &legion(), 4);
    assert_eq!(l2.get().sound.output, "HDMI:0");
    assert_eq!(m.computers().unwrap().list().len(), 2);
}

#[test]
fn hwprofile_a_new_computer_raises_exactly_one_inbox_notice() {
    struct Off;
    impl iyakki_proxy::Upstream for Off {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }
    let d = tmp("notice");
    let svc = Services::open(&d, Arc::new(Off), Arc::new(Off)).unwrap();
    let admins = vec!["amy".to_string()];
    let first = Computers::open(&d, &mac(), 1, svc.settings.lock().unwrap().get());
    assert!(!announce_new_computer(&svc, &first, &admins));
    let second = Computers::open(&d, &legion(), 2, svc.settings.lock().unwrap().get());
    assert!(announce_new_computer(&svc, &second, &admins));
    let again = Computers::open(&d, &legion(), 3, svc.settings.lock().unwrap().get());
    assert!(!announce_new_computer(&svc, &again, &admins));
    let _ = announce_new_computer(&svc, &second, &admins);
    let notes = svc.inbox.list("amy");
    let n: Vec<_> = notes.iter().filter(|n| n.title.starts_with("New computer")).collect();
    assert_eq!(n.len(), 1, "{notes:?}");
    assert!(n[0].title.contains("LENOVO 82WM") && n[0].body.contains("kept separately"));
    assert_eq!(n[0].link.as_ref().map(|l| (l.app.as_str(), l.id.as_str())), Some(("settings", "computer")));
}

#[test]
fn hwprofile_a_damaged_profile_falls_back_to_the_global_values_and_is_moved_aside() {
    let d = tmp("corrupt");
    let mut st = store(&d, &mac(), 1);
    st.update(|s| s.sound.volume_pct = 77).unwrap();
    let id = identify(&mac()).id;
    drop(st);
    std::fs::write(d.join(format!("computers/{id}.json")), "{ not json").unwrap();
    let st = store(&d, &mac(), 9);
    assert!(st.computers().unwrap().fell_back);
    assert_eq!(st.get().sound.volume_pct, 77, "no value is lost");
    let aside: Vec<_> = std::fs::read_dir(d.join("computers")).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains(".corrupt-")).collect();
    assert_eq!(aside.len(), 1);
    assert_eq!(st.computers().unwrap().profile().unwrap().values["/sound/volume_pct"], json!(77));
    // too large is damaged too
    drop(st);
    std::fs::write(d.join(format!("computers/{id}.json")), vec![b' '; 300_000]).unwrap();
    let st = store(&d, &mac(), 10);
    assert!(st.computers().unwrap().fell_back);
}

#[test]
fn hwprofile_a_category_can_move_between_per_computer_and_global() {
    let d = tmp("share");
    let mut mst = store(&d, &mac(), 1);
    mst.update(|s| s.keyboard.profile = "mac".into()).unwrap();
    let mc = mst.computers().unwrap().clone();
    drop(mst);
    // share the keyboard: changes go to the global file only, and another computer sees the shared value
    mc.set_shared(Category::Keyboard, true).unwrap();
    let mut mst = store(&d, &mac(), 2);
    mst.update(|s| s.keyboard.profile = "pc".into()).unwrap();
    assert_eq!(mst.computers().unwrap().profile().unwrap().values["/keyboard/profile"], json!("mac"), "a shared category is not recorded");
    drop(mst);
    let lg = store(&d, &legion(), 3);
    assert_eq!(lg.get().keyboard.profile, "pc", "shared: the Legion sees the global value");
    // back to per computer: the current values are captured
    let lc = lg.computers().unwrap().clone();
    lc.set_shared(Category::Keyboard, false).unwrap();
    assert!(!lc.shared().contains(&Category::Keyboard));
    assert_eq!(Category::parse("Keyboard and pointer"), Some(Category::Keyboard));
    assert_eq!(Category::parse("nope"), None);
}

#[test]
fn hwprofile_copy_from_and_reset_work_and_forget_removes() {
    let d = tmp("copy");
    let mut m = store(&d, &mac(), 1);
    m.update(|s| {
        s.sound.output = "PCH:3".into();
        s.keyboard.fn_mode = "media".into();
    })
    .unwrap();
    drop(m);
    let mut lg = store(&d, &legion(), 2);
    let c = lg.computers().unwrap().clone();
    let other = c.find_other("macmini").unwrap();
    assert_eq!(other.label, "Apple Macmini8,1");
    assert!(c.find_other("zzz").is_err() && c.find_other("").is_err());
    let mut res = Ok(());
    lg.update(|s| res = c.copy_from(&other.id, s)).unwrap();
    res.unwrap();
    assert_eq!((lg.get().sound.output.as_str(), lg.get().keyboard.fn_mode.as_str()), ("PCH:3", "media"));
    assert_eq!(c.profile().unwrap().values["/sound/output"], json!("PCH:3"));
    assert!(c.copy_from(&c.me.id.clone(), &mut lg.get().clone()).is_err(), "not from itself");
    let mut res = Ok(());
    lg.update(|s| res = c.reset(s)).unwrap();
    res.unwrap();
    assert_eq!(lg.get().sound.output, "");
    assert_eq!(c.profile().unwrap().values["/keyboard/fn_mode"], json!(""));
    assert!(c.forget(&c.me.id).is_err());
    c.forget(&other.id).unwrap();
    assert_eq!(c.list().len(), 1);
}

static ACTIVE_TEST: Mutex<()> = Mutex::new(());

#[test]
fn hwprofile_the_gpu_choice_and_the_display_size_are_per_computer() {
    let _g = ACTIVE_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let d = tmp("extras");
    std::fs::write(d.join("lemonade-compute"), "cpu\n").unwrap();
    std::fs::write(d.join("ui-amy.json"), r#"{"size":3}"#).unwrap();
    let st = store(&d, &mac(), 1);
    activate(st.computers().unwrap());
    // the first computer adopted what the files held
    assert_eq!(crate::gpu_steps::lemonade_compute(&d), crate::gpu_steps::Compute::Cpu);
    assert_eq!(display_size_get(&d, "amy"), Some(Some(3)));
    drop(st);
    let st = store(&d, &legion(), 2);
    activate(st.computers().unwrap());
    assert_eq!(crate::gpu_steps::lemonade_compute(&d), crate::gpu_steps::Compute::Gpu, "a new computer is automatic, not the Mac's choice");
    assert_eq!(display_size_get(&d, "amy"), Some(None));
    display_size_set(&d, "amy", 1);
    crate::gpu_steps::set_lemonade_compute(&d, crate::gpu_steps::Compute::Cpu).unwrap();
    assert_eq!(display_size_get(&d, "amy"), Some(Some(1)));
    // a shared category uses the global file; another data folder is untouched
    st.computers().unwrap().set_shared(Category::Display, true).unwrap();
    assert_eq!(display_size_get(&d, "amy"), None);
    assert_eq!(display_size_get(&tmp("elsewhere"), "amy"), None);
    drop(st);
    let back = store(&d, &mac(), 3);
    activate(back.computers().unwrap());
    back.computers().unwrap().set_shared(Category::Display, false).unwrap();
    assert_eq!(display_size_get(&d, "amy"), Some(Some(3)));
    clear_active_for_test();
}

// ------------------------------------------------------------------------------------------------------------------ the tools

#[derive(Default)]
struct FakeBackend(Mutex<Vec<String>>);
impl tools::Backend for FakeBackend {
    fn status(&self) -> String {
        "Apple Macmini8,1".into()
    }
    fn set_sharing(&self, c: Category, s: bool) -> Result<String, String> {
        self.0.lock().unwrap().push(format!("share {} {s}", c.key()));
        Ok("ok".into())
    }
    fn copy_from(&self, c: &str) -> Result<String, String> {
        self.0.lock().unwrap().push(format!("copy {c}"));
        Ok("ok".into())
    }
    fn reset(&self) -> Result<String, String> {
        self.0.lock().unwrap().push("reset".into());
        Ok("ok".into())
    }
}
struct Nothing;
impl Tools for Nothing {
    fn defs(&self) -> Vec<ToolDef> {
        vec![]
    }
    fn call(&self, n: &str, _: &Value) -> Result<String, String> {
        Err(format!("no {n}"))
    }
    fn describe(&self, n: &str, _: &Value) -> String {
        n.into()
    }
}

#[test]
fn hwprofile_the_tools_status_is_free_and_every_change_asks() {
    let b = Arc::new(FakeBackend::default());
    let t = tools::ThisComputerTools::new(Arc::new(Nothing), b.clone());
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).map(|d| d.risk);
    assert_eq!(risk(tools::STATUS), Some(Risk::Safe));
    for n in [tools::SET_SHARING, tools::COPY_FROM, tools::RESET] {
        assert_eq!(risk(n), Some(Risk::Ask), "{n}");
        assert!(!t.pre_approved(n) && !t.approved_always(n) && t.grant_blocker(n, &json!({})).is_some(), "{n}");
    }
    assert_eq!(t.call(tools::STATUS, &json!({})).unwrap(), "Apple Macmini8,1");
    assert!(t.refusal(tools::SET_SHARING, &json!({"category": "toaster", "shared": true})).is_some());
    assert!(t.refusal(tools::SET_SHARING, &json!({"category": "sound"})).is_some());
    assert!(t.refusal(tools::COPY_FROM, &json!({})).is_some());
    t.call(tools::SET_SHARING, &json!({"category": "keyboard", "shared": true})).unwrap();
    t.call(tools::COPY_FROM, &json!({"computer": "legion"})).unwrap();
    t.call(tools::RESET, &json!({})).unwrap();
    assert_eq!(*b.0.lock().unwrap(), vec!["share keyboard true", "copy legion", "reset"]);
    assert!(t.describe(tools::SET_SHARING, &json!({"category": "sound", "shared": false})).contains("per computer"));
}

#[test]
fn hwprofile_the_real_operations_go_through_the_settings_store() {
    struct Off;
    impl iyakki_proxy::Upstream for Off {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }
    let _g = ACTIVE_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let d = tmp("realops");
    let svc = Services::open(&d, Arc::new(Off), Arc::new(Off)).unwrap();
    assert!(copy_from(&svc, "x").is_err(), "no computer attached yet");
    {
        let mut st = svc.settings.lock().unwrap();
        let c = Computers::open(&d, &mac(), 1, st.get());
        st.attach_computers(c);
        st.update(|s| s.sound.output = "PCH:3".into()).unwrap();
        let c = Computers::open(&d, &legion(), 2, st.get());
        st.attach_computers(c);
    }
    assert_eq!(svc.settings.lock().unwrap().get().sound.output, "");
    copy_from(&svc, "Macmini").unwrap();
    assert_eq!(svc.settings.lock().unwrap().get().sound.output, "PCH:3");
    reset(&svc).unwrap();
    assert_eq!(svc.settings.lock().unwrap().get().sound.output, "");
    set_sharing(&svc, Category::Sound, true).unwrap();
    assert!(computers_of(&svc).unwrap().shared().contains(&Category::Sound));
    assert!(status_text(&computers_of(&svc).unwrap()).contains("Sound: shared by all computers"));
    forget(&svc, "Macmini").unwrap();
    assert_eq!(computers_of(&svc).unwrap().list().len(), 1);
}
