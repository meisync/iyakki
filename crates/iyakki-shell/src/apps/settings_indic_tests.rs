//! Settings → Indic voices: the list, the approval card before any download, downloading with checksums, deleting, taking a voice
//! from a folder, switching the reading to the Indic voices, and the geometry of the page at four screen sizes. The files are fakes
//! pinned by a catalog.json written for the test; the server is a fake on this computer.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_agent::iyakki_proxy::{StreamHead, Streamer};
use iyakki_agent::indic::{self, models};
use super::indic_page::IndicAct;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

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
fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}
fn press(a: &mut SettingsApp, cx: &AppCx, c: char) {
    key(a, cx, Key::Char(c));
}
fn typ(a: &mut SettingsApp, cx: &AppCx, s: &str) {
    s.chars().for_each(|c| press(a, cx, c));
}
fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}
fn settle(a: &mut SettingsApp, cx: &AppCx) {
    for _ in 0..4000 {
        a.handle(&Event::Tick, cx);
        if !a.is_busy() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("job did not finish");
}
fn goto(a: &mut SettingsApp, cx: &AppCx) {
    while a.tab() != Tab::IndicVoices {
        key(a, cx, Key::Right);
    }
}
fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
    g
}
fn overlaps(a: R, b: R) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}
fn sha(b: &[u8]) -> String {
    models::sha256_hex(b)
}

/// A pinned fake bundle for Tamil (catalog.json in `root`); the files, by name.
fn bundle(root: &Path) -> HashMap<String, Vec<u8>> {
    let files: HashMap<String, Vec<u8>> = ["vocab.json", "hifigan.onnx", "fastpitch.onnx"].iter().enumerate().map(|(i, f)| (f.to_string(), (0..5000 + i * 700).map(|k| ((k * 13 + i) % 247) as u8).collect())).collect();
    let mut l = serde_json::Map::new();
    for (n, b) in &files {
        l.insert(n.clone(), serde_json::json!({"sha256": sha(b), "bytes": b.len()}));
    }
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("catalog.json"), serde_json::json!({"ta": l}).to_string()).unwrap();
    files
}

#[derive(Default)]
struct Fake {
    files: HashMap<String, Vec<u8>>,
    requests: Mutex<Vec<String>>,
}
impl Streamer for Fake {
    fn open(&self, url: &str, _headers: &[(String, String)]) -> Result<StreamHead, String> {
        self.requests.lock().unwrap().push(url.to_string());
        match self.files.get(url) {
            Some(b) => Ok(StreamHead { status: 200, headers: vec![], body: Box::new(std::io::Cursor::new(b.clone())) }),
            None => Ok(StreamHead { status: 404, headers: vec![], body: Box::new(std::io::empty()) }),
        }
    }
}

fn app(cx: &AppCx, tag: &str) -> (SettingsApp, std::path::PathBuf, HashMap<String, Vec<u8>>, Arc<Fake>) {
    let root = temp_dir(&format!("{tag}-models"));
    let files = bundle(&root);
    let f = Arc::new(Fake { files: files.iter().map(|(n, b)| (format!("https://models.example.org/indic/ta/{n}"), b.clone())).collect(), ..Fake::default() });
    let a = SettingsApp::new(cx).with_indic(f.clone(), root.clone(), false);
    (a, root, files, f)
}

#[test]
fn the_page_is_for_administrators_and_default_off() {
    let cx = admin("ind-vis");
    let mut a = SettingsApp::new(&cx);
    assert!(a.tabs().contains(&Tab::IndicVoices));
    goto(&mut a, &cx);
    assert_eq!(Tab::IndicVoices.label(), "Indic voices");
    assert!(cx.services.voice_settings().tts.is_none(), "default off: nothing is chosen until e is pressed");
    let g = draw(&mut a, &cx, 1280, 720);
    let rows = g.hits.iter().filter(|h| matches!(h.ev, Event::Row(_))).count();
    assert!(rows >= 3, "{rows} language rows at 1280x720");
    let mut ucx = admin("ind-user");
    ucx.session.user = "amy".into();
    ucx.session.role = iyakki_sec::policy::Role::User;
    ucx.session.policy = iyakki_sec::policy::Role::User.template();
    ucx.services.on_login("amy", "amypass-123", false);
    assert!(!SettingsApp::new(&ucx).tabs().contains(&Tab::IndicVoices), "a user does not get the page");
    // a user who reached it anyway changes nothing
    let mut u = SettingsApp::new(&ucx);
    u.tabs.push(Tab::IndicVoices);
    u.tab = u.tabs.len() - 1;
    for c in ['d', 'x', 'u', 'b', 'a', 'e', 'g', 'y', 'c'] {
        press(&mut u, &ucx, c);
        assert!(!u.has_modal() && !u.is_busy(), "{c}");
    }
    assert!(ucx.services.voice_settings().tts.is_none());
}

#[test]
fn nothing_is_downloaded_until_the_card_is_confirmed_and_the_card_names_host_and_size() {
    let cx = admin("ind-dl");
    let (mut a, root, files, fake) = app(&cx, "ind-dl");
    goto(&mut a, &cx);
    while a.indic_lang() != "ta" {
        key(&mut a, &cx, Key::Down);
    }
    // no address yet: an error, no card, no request
    press(&mut a, &cx, 'd');
    assert!(!a.has_modal() && status(&a).contains("no bundle address"), "{}", status(&a));
    // the address (an administrator's form)
    press(&mut a, &cx, 'b');
    assert!(a.has_modal());
    typ(&mut a, &cx, "https://models.example.org/indic/");
    key(&mut a, &cx, Key::Enter);
    assert!(!a.has_modal(), "{}", status(&a));
    assert_eq!(models::source(&root), "https://models.example.org/indic");
    // a plain-http public address is refused
    press(&mut a, &cx, 'b');
    for _ in 0..60 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "http://evil.example.com/x");
    key(&mut a, &cx, Key::Enter);
    assert!(status(&a).contains("https"), "{}", status(&a));
    key(&mut a, &cx, Key::Esc);
    // the card
    press(&mut a, &cx, 'd');
    let Some(Modal::Confirm(ConfirmKind::Indic(IndicAct::Download(l)), text)) = &a.modal else { panic!("a card: {}", status(&a)) };
    assert_eq!(l, "ta");
    let total: usize = files.values().map(Vec::len).sum();
    assert!(text.contains("models.example.org") && text.contains("Tamil") && text.contains("KB") && text.contains(&format!("{:.0} KB", total as f64 / 1024.0)), "{text}");
    assert!(text.contains("no licence") || text.contains("states no licence"), "the licence status is on the card: {text}");
    assert!(fake.requests.lock().unwrap().is_empty(), "nothing was fetched yet");
    // cancelling fetches nothing
    key(&mut a, &cx, Key::Esc);
    assert!(!a.has_modal() && fake.requests.lock().unwrap().is_empty());
    assert!(matches!(models::status(&root, "ta"), models::Status::NotInstalled));
    // confirming does
    press(&mut a, &cx, 'd');
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Yes, download"), "the card has its buttons");
    press(&mut a, &cx, 'y');
    assert!(a.is_busy());
    settle(&mut a, &cx);
    assert!(status(&a).contains("Tamil voice is installed"), "{}", status(&a));
    assert!(matches!(models::status(&root, "ta"), models::Status::Installed { .. }));
    assert_eq!(fake.requests.lock().unwrap().len(), 3);
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.indic.download") && l.contains("models.example.org")));
    // installed: pressing d again says so and fetches nothing
    press(&mut a, &cx, 'd');
    assert!(!a.has_modal() && status(&a).contains("already installed"));
    assert_eq!(fake.requests.lock().unwrap().len(), 3);
}

#[test]
fn a_file_with_the_wrong_checksum_is_refused_and_nothing_is_installed() {
    let cx = admin("ind-bad");
    let root = temp_dir("ind-bad-models");
    let mut files = bundle(&root);
    files.get_mut("fastpitch.onnx").unwrap()[9] ^= 0x55;
    let f = Arc::new(Fake { files: files.iter().map(|(n, b)| (format!("https://models.example.org/indic/ta/{n}"), b.clone())).collect(), ..Fake::default() });
    let mut a = SettingsApp::new(&cx).with_indic(f, root.clone(), false);
    goto(&mut a, &cx);
    models::set_source(&root, "https://models.example.org/indic").unwrap();
    while a.indic_lang() != "ta" {
        key(&mut a, &cx, Key::Down);
    }
    press(&mut a, &cx, 'd');
    press(&mut a, &cx, 'y');
    settle(&mut a, &cx);
    assert_eq!(a.status().unwrap().1, false);
    assert!(status(&a).contains("checksum") && status(&a).contains("fastpitch.onnx"), "{}", status(&a));
    assert!(!matches!(models::status(&root, "ta"), models::Status::Installed { .. }));
    assert!(!root.join("ta/fastpitch.onnx").exists());
}

#[test]
fn delete_asks_first_and_removes_only_that_language() {
    let cx = admin("ind-del");
    let (mut a, root, files, _) = app(&cx, "ind-del");
    std::fs::create_dir_all(root.join("ta")).unwrap();
    for (n, b) in &files {
        std::fs::write(root.join("ta").join(n), b).unwrap();
    }
    goto(&mut a, &cx);
    while a.indic_lang() != "ta" {
        key(&mut a, &cx, Key::Down);
    }
    press(&mut a, &cx, 'x');
    assert!(matches!(a.modal, Some(Modal::Confirm(ConfirmKind::Indic(IndicAct::Delete(_)), _))));
    key(&mut a, &cx, Key::Esc);
    assert!(matches!(models::status(&root, "ta"), models::Status::Installed { .. }), "cancel keeps it");
    press(&mut a, &cx, 'x');
    press(&mut a, &cx, 'y');
    assert!(status(&a).contains("deleted"), "{}", status(&a));
    assert_eq!(models::status(&root, "ta"), models::Status::NotInstalled);
    assert!(root.join("catalog.json").exists());
    press(&mut a, &cx, 'x');
    assert!(!a.has_modal() && status(&a).contains("not installed"));
}

#[test]
fn a_folder_or_usb_stick_installs_a_voice_after_checking_it() {
    let cx = admin("ind-usb");
    let (mut a, root, files, _) = app(&cx, "ind-usb");
    let stick = temp_dir("ind-usb-stick");
    std::fs::create_dir_all(stick.join("ta")).unwrap();
    for (n, b) in &files {
        std::fs::write(stick.join("ta").join(n), b).unwrap();
    }
    goto(&mut a, &cx);
    while a.indic_lang() != "ta" {
        key(&mut a, &cx, Key::Down);
    }
    press(&mut a, &cx, 'u');
    typ(&mut a, &cx, "/no/such/folder");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal(), "a folder that does not exist keeps the form open: {}", status(&a));
    key(&mut a, &cx, Key::Esc);
    press(&mut a, &cx, 'u');
    typ(&mut a, &cx, &stick.to_string_lossy());
    key(&mut a, &cx, Key::Enter);
    settle(&mut a, &cx);
    assert!(status(&a).contains("installed from the folder"), "{}", status(&a));
    assert!(matches!(models::status(&root, "ta"), models::Status::Installed { .. }));
    // a language this build has no bundle for cannot be installed that way
    while a.indic_lang() != "gu" {
        key(&mut a, &cx, Key::Down);
    }
    press(&mut a, &cx, 'u');
    assert!(!a.has_modal() && status(&a).contains("no bundle"), "{}", status(&a));
}

#[test]
fn use_for_reading_switches_the_engine_and_back_and_the_gender_follows() {
    let cx = admin("ind-use");
    let (mut a, root, _, _) = app(&cx, "ind-use");
    goto(&mut a, &cx);
    press(&mut a, &cx, 'e');
    let Some(SpeechEngine::Host { engine, program, voice }) = cx.services.voice_settings().tts else { panic!("{}", status(&a)) };
    assert_eq!((engine.as_str(), program.as_deref(), voice.as_deref()), ("indic-tts", Some("http://127.0.0.1:8189"), Some("female")));
    press(&mut a, &cx, 'g');
    assert!(matches!(cx.services.voice_settings().tts, Some(SpeechEngine::Host { voice: Some(ref v), .. }) if v == "male"));
    // the server address
    press(&mut a, &cx, 'a');
    for _ in 0..40 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "http://192.168.1.20:8189");
    key(&mut a, &cx, Key::Enter);
    assert!(matches!(cx.services.voice_settings().tts, Some(SpeechEngine::Host { program: Some(ref p), voice: Some(ref v), .. }) if p == "http://192.168.1.20:8189" && v == "male"), "{}", status(&a));
    assert_eq!(models::server(&root), "http://192.168.1.20:8189");
    // a bad address changes nothing
    press(&mut a, &cx, 'a');
    for _ in 0..40 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "ftp://nope");
    key(&mut a, &cx, Key::Enter);
    assert!(status(&a).contains("http"), "{}", status(&a));
    key(&mut a, &cx, Key::Esc);
    assert_eq!(models::server(&root), "http://192.168.1.20:8189");
    // the reader's status names it and its server, and the speech engine label does too
    let label = cx.services.voice_settings().tts.unwrap().label();
    assert!(label.contains("Indic neural voice (local)") && label.contains("192.168.1.20"), "{label}");
    // and off again
    press(&mut a, &cx, 'e');
    assert!(cx.services.voice_settings().tts.is_none());
    assert!(cx.audit.recent(30).iter().any(|l| l.contains("settings.voice.tts")));
}

fn fake_voice_server() -> String {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let body = if req.starts_with("GET /v1/voices") { br#"{"voices":[{"id":"ta-female"},{"id":"ta-male"},{"id":"hi-female"}]}"#.to_vec() } else { b"no".to_vec() };
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = s.write_all(&body);
        }
    });
    format!("http://127.0.0.1:{port}")
}

#[test]
fn check_server_reports_the_languages_and_a_dead_server_in_words() {
    let cx = admin("ind-chk");
    let (mut a, root, _, _) = app(&cx, "ind-chk");
    goto(&mut a, &cx);
    models::set_server(&root, &fake_voice_server()).unwrap();
    press(&mut a, &cx, 'c');
    settle(&mut a, &cx);
    assert!(status(&a).contains("3 voices") && status(&a).contains("ta, hi"), "{}", status(&a));
    models::set_server(&root, "http://127.0.0.1:1").unwrap();
    press(&mut a, &cx, 'c');
    settle(&mut a, &cx);
    assert_eq!(a.status().unwrap().1, false);
    assert!(status(&a).contains("cannot be reached"), "{}", status(&a));
    // the test voice says what is missing instead of playing silence
    press(&mut a, &cx, 'y');
    settle(&mut a, &cx);
    assert!(status(&a).contains("cannot be reached") && status(&a).contains("Nothing was played"), "{}", status(&a));
}

#[test]
fn geometry_and_click_paths_hold_at_four_sizes() {
    let cx = admin("ind-geo");
    let (mut a, root, files, _) = app(&cx, "ind-geo");
    std::fs::create_dir_all(root.join("ta")).unwrap();
    for (n, b) in &files {
        std::fs::write(root.join("ta").join(n), b).unwrap();
    }
    models::set_source(&root, "https://models.example.org/indic").unwrap();
    goto(&mut a, &cx);
    for sel_lang in ["ta", "gu", "mni"] {
        while a.indic_lang() != sel_lang {
            let k = if a.sel() < indic::LANGS.iter().position(|l| l.code == sel_lang).unwrap() { Key::Down } else { Key::Up };
                key(&mut a, &cx, k);
        }
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            let g = draw(&mut a, &cx, w, h);
            let sc = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
            let keys: Vec<(char, R)> = g.hits.iter().filter_map(|x| if let Event::Key(Key::Char(c)) = x.ev { Some((c, x.rect)) } else { None }).collect();
            for c in ['d', 'x', 'u', 'b', 'a', 'e', 'g', 'y', 'c', 'k'] {
                assert!(keys.iter().any(|k| k.0 == c), "{w}x{h}: button {c}");
            }
            for (i, (ca, ra)) in keys.iter().enumerate() {
                assert!(ra.x >= 0.0 && ra.x + ra.w <= w as f32 && ra.y + ra.h <= h as f32 - 28.0 * sc, "{w}x{h}: {ca} inside and clear of the status line");
                for (cb, rb) in keys.iter().skip(i + 1) {
                    assert!(!overlaps(*ra, *rb), "{w}x{h}: {ca} and {cb} overlap");
                }
                for r in g.hits.iter().filter(|h| matches!(h.ev, Event::Row(_))) {
                    assert!(!overlaps(*ra, r.rect), "{w}x{h}: button {ca} covers a language row");
                }
            }
            let rows: Vec<(usize, R)> = g.hits.iter().filter_map(|h| if let Event::Row(i) = h.ev { Some((i, h.rect)) } else { None }).collect();
            let sel = a.sel();
            assert!(rows.iter().any(|(i, _)| *i == sel), "{w}x{h}: the chosen language ({sel_lang}) is on screen");
            for (_, r) in &rows {
                assert!(r.x >= 0.0 && r.x + r.w <= w as f32 && r.y >= 0.0 && r.y + r.h <= h as f32 - 28.0 * sc, "{w}x{h}: row inside the screen and above the status line: {r:?}");
            }
            for (i, (_, ra)) in rows.iter().enumerate() {
                for (_, rb) in rows.iter().skip(i + 1) {
                    assert!(!overlaps(*ra, *rb), "{w}x{h}: language rows overlap");
                }
            }
            // clicking a row chooses it
            let (idx, _) = rows[rows.len() / 2];
            let ev = g.hits.iter().find(|h| h.ev == Event::Row(idx)).unwrap().ev.clone();
            a.handle(&ev, &cx);
            assert_eq!(a.sel(), idx);
            while a.indic_lang() != sel_lang {
                let k = if a.sel() < indic::LANGS.iter().position(|l| l.code == sel_lang).unwrap() { Key::Down } else { Key::Up };
                key(&mut a, &cx, k);
            }
        }
    }
    // clicking Download opens the card, and the card's buttons are on screen at every size
    while a.indic_lang() != "hi" {
        key(&mut a, &cx, Key::Up);
    }
    let g = draw(&mut a, &cx, 1280, 720);
    let ev = g.hits.iter().find(|h| h.ev == Event::Key(Key::Char('d'))).unwrap().ev.clone();
    a.handle(&ev, &cx);
    assert!(matches!(a.modal, Some(Modal::Confirm(ConfirmKind::Indic(IndicAct::Download(_)), _))), "clicking Download opens the approval card: {}", status(&a));
    let g = draw(&mut a, &cx, 800, 480);
    assert!(g.hits.iter().any(|h| h.label == "Yes, download"), "the card's buttons are there at 800x480");
    key(&mut a, &cx, Key::Esc);
    while a.indic_lang() != "ta" {
        key(&mut a, &cx, Key::Up);
    }
    // Tamil is installed in this test: delete opens a card
    let g = draw(&mut a, &cx, 1280, 720);
    let ev = g.hits.iter().find(|h| h.ev == Event::Key(Key::Char('x'))).unwrap().ev.clone();
    a.handle(&ev, &cx);
    assert!(a.has_modal());
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let g = draw(&mut a, &cx, w, h);
        for lbl in ["Yes, delete", "Cancel"] {
            let hit = g.hits.iter().find(|x| x.label == lbl).unwrap_or_else(|| panic!("{w}x{h}: {lbl}"));
            assert!(hit.rect.x >= 0.0 && hit.rect.x + hit.rect.w <= w as f32 && hit.rect.y + hit.rect.h <= h as f32, "{w}x{h}: {lbl} on screen");
        }
    }
}
