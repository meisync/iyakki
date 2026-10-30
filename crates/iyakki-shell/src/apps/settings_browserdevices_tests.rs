//! Settings -> Browser -> the cameras and microphones marked "don't use": listed with a button each, a number key brings one back, the
//! plugin files follow, nothing is shown when nothing is hidden, and the buttons fit at the usual sizes.

use super::*;
use crate::apps::browser_media::devprefs_path;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_agent::browserdev::{DevicePrefs, CAM_HIDDEN, MIC_HIDDEN};

struct Offline;
impl iyakki_agent::iyakki_proxy::Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_agent::iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
    g
}

#[test]
fn hidden_devices_are_listed_and_a_number_key_brings_one_back() {
    let share = temp_dir("bdev-share");
    std::env::set_var("IYAKKI_MIC_DIR", &share);
    let cx = admin("bdev1");
    let a = &mut SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    // nothing hidden: no title, no buttons
    assert!(!draw(a, &cx, 1920, 1080).hits.iter().any(|h| h.ev == Event::Key(Key::Char('1'))));
    let mut p = DevicePrefs::default();
    p.hide_cam("Cover cam: x");
    p.hide_mic("plughw:9,9");
    p.save(&devprefs_path(&cx)).unwrap();
    for (w, h) in [(2560u32, 1600u32), (1920, 1080), (1280, 720)] {
        let g = draw(a, &cx, w, h);
        let rects: Vec<R> = ['1', '2'].iter().filter_map(|c| g.hits.iter().find(|x| x.ev == Event::Key(Key::Char(*c))).map(|x| x.rect)).collect();
        assert_eq!(rects.len(), 2, "{w}x{h}: one button per hidden device");
        assert!(rects.iter().all(|r| r.x >= 0.0 && r.x + r.w <= w as f32 && r.y + r.h <= h as f32), "{w}x{h}: on screen {rects:?}");
        assert!(!(rects[0].y < rects[1].y + rects[1].h && rects[1].y < rects[0].y + rects[0].h), "{w}x{h}: the buttons do not overlap");
    }
    // the first row is the camera
    a.handle(&Event::Key(Key::Char('1')), &cx);
    let q = DevicePrefs::load(&devprefs_path(&cx));
    assert!(q.hidden_cams.is_empty() && q.hidden_mics.contains("plughw:9,9"), "{q:?}");
    assert!(!share.join(CAM_HIDDEN).exists() && std::fs::read_to_string(share.join(MIC_HIDDEN)).unwrap() == "plughw:9,9\n", "the plugin files follow");
    // now the microphone is the first row
    a.handle(&Event::Key(Key::Char('1')), &cx);
    assert_eq!(DevicePrefs::load(&devprefs_path(&cx)), DevicePrefs::default());
    assert!(!share.join(MIC_HIDDEN).exists());
    assert!(!draw(a, &cx, 1920, 1080).hits.iter().any(|h| h.ev == Event::Key(Key::Char('1'))), "the section is gone when nothing is hidden");
    // a key for a row that is not there does nothing
    a.handle(&Event::Key(Key::Char('7')), &cx);
    let _ = std::fs::remove_dir_all(&share);
}
