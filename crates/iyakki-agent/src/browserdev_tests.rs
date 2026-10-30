use super::*;
use crate::voicedesk::Mic;

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-browserdev-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn node(sys: &Path, n: u32, name: &str, index: Option<&str>) {
    let d = sys.join(format!("class/video4linux/video{n}"));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("name"), format!("{name}\n")).unwrap();
    if let Some(i) = index {
        std::fs::write(d.join("index"), format!("{i}\n")).unwrap();
    }
}

#[test]
fn cameras_are_the_picture_nodes_each_name_once_in_device_order() {
    let sys = tmp("cams");
    node(&sys, 4, "EMEET SmartCam Nova 4K: EMEET S", Some("0"));
    node(&sys, 5, "EMEET SmartCam Nova 4K: EMEET S", Some("1")); // metadata node
    node(&sys, 0, "FaceTime HD Camera (Built-in): ", Some("0"));
    node(&sys, 1, "FaceTime HD Camera (Built-in): ", None); // no index file: a picture node with the same name
    let c = list_cameras(&sys);
    assert_eq!(c.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["FaceTime HD Camera (Built-in)", "EMEET SmartCam Nova 4K: EMEET S"]);
    assert_eq!(c[0].id, "FaceTime HD Camera (Built-in):", "the id is the engine's name with the ends trimmed");
    assert!(list_cameras(&sys.join("nothing")).is_empty());
    let _ = std::fs::remove_dir_all(&sys);
}

#[test]
fn hidden_devices_are_never_picked_and_a_site_choice_that_is_gone_falls_back() {
    let cams = vec![Cam { id: "A".into(), label: "A".into() }, Cam { id: "B".into(), label: "B".into() }, Cam { id: "C".into(), label: "C".into() }];
    let mut p = DevicePrefs::default();
    assert_eq!(p.pick_cam(&cams, "").unwrap().id, "A", "automatic: the first");
    p.cam = "B".into();
    assert_eq!(p.pick_cam(&cams, "").unwrap().id, "B", "the default");
    assert_eq!(p.pick_cam(&cams, "C").unwrap().id, "C", "the site's own camera wins");
    assert_eq!(p.pick_cam(&cams, "Z").unwrap().id, "B", "a camera the site remembered but that is unplugged: the default");
    p.hide_cam("C");
    assert_eq!(p.pick_cam(&cams, "C").unwrap().id, "B", "a hidden camera is not used even when a site remembered it");
    p.hide_cam("B");
    assert!(p.cam.is_empty(), "hiding the default clears it");
    assert_eq!(p.pick_cam(&cams, "").unwrap().id, "A");
    p.hide_cam("A");
    assert!(p.pick_cam(&cams, "").is_none(), "every camera hidden: none, never a hidden one");
    assert!(p.usable_cams(&cams).is_empty());
    p.show_cam("A");
    assert_eq!(p.usable_cams(&cams).len(), 1);
}

#[test]
fn the_choices_are_saved_reloaded_and_a_damaged_file_means_defaults() {
    let d = tmp("save");
    let f = d.join("browser-devices-amy.json");
    let mut p = DevicePrefs::default();
    p.cam = "FaceTime HD Camera (Built-in):".into();
    p.hide_mic("plughw:0,1");
    p.hide_cam("Broken cam");
    p.save(&f).unwrap();
    assert_eq!(DevicePrefs::load(&f), p);
    std::fs::write(&f, "{ not json").unwrap();
    assert_eq!(DevicePrefs::load(&f), DevicePrefs::default());
    assert_eq!(DevicePrefs::load(&d.join("none.json")), DevicePrefs::default());
    // odd values are dropped, the lists are capped
    let many: Vec<String> = (0..200).map(|i| format!("\"cam{i}\"")).collect();
    let q = DevicePrefs::from_json(&format!("{{\"cam\":\"\\u0007bad\",\"hidden_cams\":[{}, \"\", \"x\\ny\"],\"hidden_mics\":[1, null, \"plughw:2,0\"]}}", many.join(",")));
    assert!(q.cam.is_empty() && q.hidden_cams.len() == 64 && !q.hidden_cams.contains("") && q.hidden_mics.len() == 1);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_plugins_get_the_lists_and_the_choices_and_an_empty_one_removes_the_file() {
    let d = tmp("share");
    let mut p = DevicePrefs::default();
    p.hide_cam("Broken cam");
    p.hide_mic("plughw:0,1");
    share_all(&d, &p, "FaceTime HD Camera (Built-in):", "plughw:1,0");
    let read = |f: &str| std::fs::read_to_string(d.join(f)).unwrap();
    assert_eq!(read(CAM_HIDDEN), "Broken cam\n");
    assert_eq!(read(MIC_HIDDEN), "plughw:0,1\n");
    assert_eq!(read(CAM_CHOICE), "FaceTime HD Camera (Built-in):\n");
    assert_eq!(read(crate::voicedesk::BROWSER_MIC_CHOICE), "plughw:1,0\n");
    p.show_cam("Broken cam");
    p.show_mic("plughw:0,1");
    share_all(&d, &p, "", "");
    for f in [CAM_HIDDEN, MIC_HIDDEN, CAM_CHOICE, crate::voicedesk::BROWSER_MIC_CHOICE] {
        assert!(!d.join(f).exists(), "{f} is gone");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn settings_rows_name_hidden_devices_even_when_they_are_unplugged() {
    let cams = vec![Cam { id: "Cover cam: x".into(), label: "Cover cam: x".into() }];
    let mics = vec![Mic { id: "plughw:1,0".into(), label: "USB Audio (USB)".into(), usb: true }];
    let mut p = DevicePrefs::default();
    p.hide_cam("Cover cam: x");
    p.hide_cam("Old camera:");
    p.hide_mic("plughw:1,0");
    p.hide_mic("plughw:9,9");
    let rows = p.hidden_rows(&cams, &mics);
    assert_eq!(rows.len(), 4);
    assert!(rows.iter().any(|r| r.0 == "camera" && r.2 == "Old camera"));
    assert!(rows.iter().any(|r| r.0 == "microphone" && r.2 == "USB Audio (USB)"));
    assert!(rows.iter().any(|r| r.0 == "microphone" && r.2 == "plughw:9,9"));
}
