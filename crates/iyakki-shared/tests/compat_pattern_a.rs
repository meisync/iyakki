//! F-CORE-23 / KI-720: golden files of the older pattern-A stores (docs/development/DATA-LAYOUT-AND-UPGRADE.md), written by hand from the
//! documented shape (not recorded from a release) and kept in `tests/compat/alpha7/`. Each is read by the current code; a field this release
//! does not know is ignored; a field a file lacks takes its default; a damaged file gives the defaults and is not trusted. Pattern A loses unknown
//! fields on the next save (finding F-9): that is asserted too, so the day a store moves to pattern S this test is changed on purpose.

use std::path::{Path, PathBuf};

use iyakki_shared::calendar::{CalendarStore, Repeat as CalRepeat};
use iyakki_shared::layout::Layout;
use iyakki_shared::media::{MediaStore, Repeat as PlayRepeat};
use iyakki_shared::modelhost::{Hosts, Preset};
use iyakki_shared::screensaver::{First, SaverConfig, Style};

fn golden(name: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/compat/alpha7").join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-compat-pa-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_screen_saver_choices_of_alpha7_are_read_and_a_later_field_or_a_damaged_file_is_handled() {
    let d = scratch("saver");
    std::fs::write(d.join("screensaver-amy.json"), golden("screensaver-amy-v0.json")).unwrap();
    let c = SaverConfig::load(&d, "amy");
    assert_eq!((c.first, c.after_min, c.then_off_min, c.battery_shorter, c.style, c.dim_pct), (First::Saver, 15, 5, false, Style::Starfield, 25));
    // a field a later release adds is ignored, a field this file lacks takes its default
    std::fs::write(d.join("screensaver-amy.json"), r#"{"style":"small","future":{"a":1}}"#).unwrap();
    let later = SaverConfig::load(&d, "amy");
    assert_eq!((later.style, later.after_min), (Style::Small, SaverConfig::default().after_min));
    // pattern A: the next save drops the field it did not know (F-9)
    later.save(&d, "amy").unwrap();
    assert!(!std::fs::read_to_string(d.join("screensaver-amy.json")).unwrap().contains("future"));
    // damaged: the defaults
    for bad in ["{ not json", "[1,2]", "", "{\"style\":\"from-the-future\"}"] {
        std::fs::write(d.join("screensaver-amy.json"), bad).unwrap();
        assert_eq!(SaverConfig::load(&d, "amy"), SaverConfig::default(), "{bad:?}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_home_layout_of_alpha7_is_read_with_its_groups_places_and_choices_and_a_damaged_file_gives_the_plain_one() {
    let d = scratch("layout");
    std::fs::write(d.join("layout-amy.json"), golden("layout-amy-v0.json")).unwrap();
    let l = Layout::load(&d, "amy");
    assert_eq!(l.groups.len(), 1);
    assert_eq!((l.groups[0].id.as_str(), l.groups[0].label.as_str()), ("g1", "School"));
    assert_eq!(l.placed.get("app:calculator").map(String::as_str), Some("g1"));
    assert_eq!(l.order["g1"], ["app:calculator"]);
    assert_eq!(l.app_groups["mathpad"].target, "g1");
    // saved again, it reads back as the same layout (the private group counter is kept)
    l.save(&d, "amy");
    assert_eq!(Layout::load(&d, "amy"), l);
    assert!(std::fs::read_to_string(d.join("layout-amy.json")).unwrap().contains("\"next_id\": 2"));
    // a missing field takes its default, an unknown one is ignored
    std::fs::write(d.join("layout-amy.json"), r#"{"groups":[{"id":"g9","label":"X"}],"future":1}"#).unwrap();
    assert_eq!(Layout::load(&d, "amy").groups.len(), 1);
    // damaged: the plain layout
    std::fs::write(d.join("layout-amy.json"), "{ cut").unwrap();
    assert_eq!(Layout::load(&d, "amy"), Layout::default());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_model_host_list_of_alpha7_is_read_with_defaults_for_fields_added_later_and_a_damaged_list_is_empty() {
    let d = scratch("hosts");
    std::fs::write(d.join("model-hosts.json"), golden("model-hosts-v0.json")).unwrap();
    let hosts = Hosts::new(&d);
    let all = hosts.list();
    assert_eq!(all.iter().map(|h| (h.id.as_str(), h.preset, h.port, h.in_cluster, h.public_ok)).collect::<Vec<_>>(), [("mac-studio", Preset::Mac, 11434, true, false), ("pc-gpu", Preset::Pc, 8080, false, false)]);
    assert_eq!(hosts.get("pc-gpu").unwrap().cluster_url(), "http://pc-gpu:8080/v1");
    // a new host goes on top of the old ones and the old ones are kept
    let mut extra = all[1].clone();
    extra.id = "pc-two".into();
    hosts.upsert(extra).unwrap();
    assert_eq!(hosts.list().len(), 3);
    std::fs::write(d.join("model-hosts.json"), "[{\"id\": 1}]").unwrap();
    assert!(hosts.list().is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_calendar_of_alpha7_is_read_with_its_events_and_alarms_and_what_it_lacks_takes_a_default_and_a_damaged_file_is_refused_not_emptied() {
    let d = scratch("calendar");
    std::fs::write(d.join("amy.json"), golden("calendar-amy-v0.json")).unwrap();
    let c = CalendarStore::new(&d);
    let ev = c.events("amy");
    assert_eq!(ev.iter().map(|e| (e.id.as_str(), e.title.as_str(), e.all_day, e.repeat, e.remind)).collect::<Vec<_>>(), [("e1", "Dentist", false, CalRepeat::None, None), ("e2", "Birthday", true, CalRepeat::Yearly, Some(60))]);
    assert_eq!((ev[1].notes.as_str(), ev[1].by.as_str()), ("bring the cake", "assistant"));
    let al = c.alarms("amy");
    assert_eq!((al.len(), al[0].label.as_str(), al[0].days, al[0].once_at, al[0].enabled), (1, "Wake up", 62, None, true));
    // a new event goes on top and the old ones stay
    c.add_event("amy", "Lunch", 1_790_200_000, 30, false, CalRepeat::None, "", "user", None).unwrap();
    assert_eq!(c.events("amy").len(), 3);
    // a field a later release adds is ignored; the next save drops it (F-9)
    std::fs::write(d.join("amy.json"), r#"{"events":[{"id":"e9","title":"X","start":1,"minutes":0,"all_day":false,"future":1}],"future":{"a":1}}"#).unwrap();
    assert_eq!(c.events("amy").len(), 1);
    c.add_event("amy", "Y", 2, 0, false, CalRepeat::None, "", "user", None).unwrap();
    assert!(!std::fs::read_to_string(d.join("amy.json")).unwrap().contains("future"));
    // a damaged calendar is not read as empty: the add is refused and the file is left alone
    std::fs::write(d.join("amy.json"), "{ cut").unwrap();
    assert!(c.add_event("amy", "Z", 3, 0, false, CalRepeat::None, "", "user", None).unwrap_err().contains("damaged"));
    assert_eq!(std::fs::read_to_string(d.join("amy.json")).unwrap(), "{ cut");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_media_choices_of_alpha7_are_read_and_a_missing_field_or_a_damaged_file_gives_the_defaults() {
    let d = scratch("media");
    std::fs::write(d.join("media-amy.json"), golden("media-amy-v0.json")).unwrap();
    let m = MediaStore::new(&d, "amy");
    let s = m.load();
    assert_eq!(s.folders.as_deref(), Some(&["/home/amy/Music".to_string(), "/home/amy/Podcasts".to_string()][..]));
    assert_eq!((s.playlists.len(), s.playlists[0].name.as_str(), s.playlists[0].items.len(), s.shuffle, s.repeat), (1, "Morning", 2, true, PlayRepeat::All));
    // saved again it reads back the same (the choices are not lost)
    m.save(&s).unwrap();
    assert_eq!(m.load().playlists, s.playlists);
    // no folders chosen yet: the default folders apply; a later field is ignored
    std::fs::write(d.join("media-amy.json"), r#"{"playlists":[],"shuffle":false,"repeat":"off","future":1}"#).unwrap();
    assert_eq!(m.folders(), iyakki_shared::media::default_folders("amy"));
    // damaged: the defaults
    for bad in ["{ cut", "[1]", "", r#"{"repeat":"sideways","playlists":[],"shuffle":false}"#] {
        std::fs::write(d.join("media-amy.json"), bad).unwrap();
        let g = m.load();
        assert!(g.folders.is_none() && g.playlists.is_empty() && !g.shuffle, "{bad:?}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_developer_tools_switch_the_remembered_folder_ways_and_the_installed_services_of_alpha7_are_read_and_damage_gives_the_defaults() {
    use iyakki_shared::hostfiles::{remembered_choice, Choice};
    let d = scratch("dev-files");
    // developer tools switch
    std::fs::write(d.join("devtools-amy.json"), golden("devtools-amy-v0.json")).unwrap();
    assert!(iyakki_shared::devtools::DevTools::new(&d).enabled("amy"));
    std::fs::write(d.join("devtools-amy.json"), r#"{"on":true,"future":1}"#).unwrap();
    assert!(iyakki_shared::devtools::DevTools::new(&d).enabled("amy"), "a later field is ignored");
    std::fs::write(d.join("devtools-amy.json"), "{ cut").unwrap();
    assert!(!iyakki_shared::devtools::DevTools::new(&d).enabled("amy"), "damaged: off");
    // remembered folder ways
    std::fs::write(d.join("host-choices-amy.json"), golden("host-choices-amy-v0.json")).unwrap();
    assert_eq!(remembered_choice(&d, "amy", Path::new("/mnt/photos")), Some(Choice::Link));
    assert_eq!(remembered_choice(&d, "amy", Path::new("/mnt/documents")), Some(Choice::Copy));
    assert_eq!(remembered_choice(&d, "amy", Path::new("/mnt/unknown")), None);
    std::fs::write(d.join("host-choices-amy.json"), r#"{"/mnt/photos":"teleport"}"#).unwrap();
    assert_eq!(remembered_choice(&d, "amy", Path::new("/mnt/photos")), None, "a way this release does not know is not used");
    std::fs::write(d.join("host-choices-amy.json"), "[1]").unwrap();
    assert_eq!(remembered_choice(&d, "amy", Path::new("/mnt/photos")), None);
    // installed services: fields the file lacks take their defaults, a later field is ignored, damage gives an empty list
    std::fs::write(d.join("installed-services-amy.json"), golden("installed-services-amy-v0.json")).unwrap();
    let list = iyakki_shared::platform_installed::load(&d, "amy");
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].id.as_str(), list[0].version.as_str(), list[0].created_namespace, list[0].connected, list[0].cluster.is_none(), list[0].workspaces.len(), list[0].note.as_str()), ("gitea-iyakki-gitea", "1.22.0", true, false, true, 1, ""));
    assert_eq!(list[0].vault, ["svc.gitea-iyakki-gitea.admin"]);
    let mut again = list[0].clone();
    again.id = "gitea-two".into();
    iyakki_shared::platform_installed::upsert(&d, "amy", again).unwrap();
    assert_eq!(iyakki_shared::platform_installed::load(&d, "amy").len(), 2);
    std::fs::write(d.join("installed-services-amy.json"), r#"[{"id":"x","future":{"a":1}}]"#).unwrap();
    assert_eq!(iyakki_shared::platform_installed::load(&d, "amy")[0].id, "x");
    std::fs::write(d.join("installed-services-amy.json"), "{ cut").unwrap();
    assert!(iyakki_shared::platform_installed::load(&d, "amy").is_empty());
    let _ = std::fs::remove_dir_all(&d);
}
