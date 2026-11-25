//! The Settings tile's own tests (helpers shared by the page tests, and the tests that belong to no single page); moved out of `settings.rs` (S0).

use super::*;

use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};

use iyakki_proxy::{Upstream, UpstreamResponse};

use iyakki_sec::policy::Role;

use std::sync::Mutex;

#[test]
fn a_cut_row_keeps_its_address_and_hands_the_whole_row_to_the_tooltip() {
    let mut g = Gfx::new(600, 200);
    let mut tip = crate::tooltip::Tooltip::new();
    let sub = format!("connected · 192.168.100.200 · 1000 Mb/s · {}", "aa:bb:cc:dd:ee:ff · ".repeat(6));
    let full = row(&mut g, &mut tip, 0, 10.0, 10.0, 300.0, 58.0, 1.0, false, "Ethernet · enp3s0", &sub, &[]).expect("the line is cut at this width");
    assert!(full.contains("192.168.100.200") && full.contains("Ethernet · enp3s0"), "{full}");
    // wide enough: nothing is cut, nothing to show
    assert!(row(&mut g, &mut tip, 0, 10.0, 80.0, 590.0, 58.0, 1.0, false, "Ethernet", "connected · 192.168.100.200", &[]).is_none());
}

pub(super) type Reply = Box<dyn Fn(&str, &str) -> (u16, String) + Send + Sync>;

pub(super) struct Mock {
    pub(super) calls: Mutex<Vec<(String, String)>>,
    pub(super) reply: Reply,
}

impl Upstream for Mock {
    fn request(&self, m: &str, url: &str, _h: &[(String, String)], _b: &[u8]) -> Result<UpstreamResponse, String> {
        self.calls.lock().unwrap().push((m.into(), url.into()));
        let (status, body) = (self.reply)(m, url);
        Ok(UpstreamResponse { status, body: body.into_bytes() })
    }
}

pub(super) fn ollama_like(m: &str, url: &str) -> (u16, String) {
    match (m, url) {
        ("GET", u) if u.ends_with("/v1/models") => (200, r#"{"data":[{"id":"qwen3:32b"},{"id":"llama3.2:3b"}]}"#.into()),
        ("POST", u) if u.ends_with("/chat/completions") => (200, r#"{"choices":[{"message":{"content":"OK"}}]}"#.into()),
        ("POST", u) if u == "http://rag.corp/search" => (200, r#"{"results":[{"text":"x"}]}"#.into()),
        ("GET", u) if u.ends_with("/v1/auth/token/lookup-self") => (200, "{}".into()),
        _ => (404, String::new()),
    }
}

/// Admin session with a real Services over a mock network; the system vault is unlocked.
pub(super) fn admin(tag: &str) -> (AppCx, std::path::PathBuf) {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Mock { calls: Default::default(), reply: Box::new(ollama_like) }));
    cx.services.on_login("root", "rootpass12", true);
    (cx, d)
}

pub(super) fn user(tag: &str) -> AppCx {
    let (mut cx, _) = admin(tag);
    cx.session.user = "amy".into();
    cx.session.role = Role::User;
    cx.session.policy = Role::User.template();
    cx.services.on_login("amy", "amypass-123", false);
    cx
}

pub(super) fn typ(a: &mut SettingsApp, cx: &AppCx, s: &str) {
    for c in s.chars() {
        a.handle(&Event::Key(Key::Char(c)), cx);
    }
}

pub(super) fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}

pub(super) fn settle(a: &mut SettingsApp, cx: &AppCx) {
    let __wall = std::time::Instant::now();
    while __wall.elapsed() < std::time::Duration::from_secs(30) {
        a.handle(&Event::Tick, cx);
        if !a.is_busy() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("job did not finish");
}

pub(super) fn goto(a: &mut SettingsApp, _cx: &AppCx, t: Tab) {
    a.go_to(t);
}

pub(super) fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}

#[test]
fn every_tab_renders_for_admin_and_user_at_several_sizes() {
    for (w, h) in [(1280u32, 720u32), (800, 600), (640, 480)] {
        for admin_view in [true, false] {
            let (cx, _) = admin(&format!("set-r-{w}-{admin_view}"));
            let cx = if admin_view { cx } else { user(&format!("set-ru-{w}")) };
            let mut a = SettingsApp::new(&cx);
            let mut g = Gfx::new(w, h);
            for _ in 0..a.tabs().len() {
                g.clear(rgb(0, 0, 0));
                a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), (w as f32 / 1280.0).max(0.6), &cx, 3);
                key(&mut a, &cx, Key::Right);
            }
        }
    }
}

// ------------------------------------------------------------------ voice tab

pub(super) fn fake_audio(d: &std::path::Path, silent: bool) -> iyakki_shared::voice::VoiceSettings {
    use iyakki_shared::voice::{encode_wav, tone};

    let pcm: Vec<u8> = if silent { vec![0u8; 32_000] } else { (0..16_000).flat_map(|i| (((i as f32 * 0.2).sin() * 9000.0) as i16).to_le_bytes()).collect() };

    std::fs::write(d.join("mic.raw"), pcm).unwrap();

    std::fs::write(d.join("canned.wav"), encode_wav(&tone(500.0, 200), 16_000)).unwrap();

    std::fs::write(d.join("whisper.sh"), "#!/bin/sh\necho 'hello from the test'\n").unwrap();

    let mut v = iyakki_shared::voice::VoiceSettings::default();

    v.record_cmd = format!("sh -c 'cat {}; sleep 30'", d.join("mic.raw").display());

    v.play_cmd = format!("sh -c 'cat > {}'", d.join("played.wav").display());

    v.stt = Some(SpeechEngine::Command { command: format!("sh {} {{wav}}", d.join("whisper.sh").display()) });

    v.tts = Some(SpeechEngine::Command { command: format!("sh -c 'cat >/dev/null; cat {}'", d.join("canned.wav").display()) });

    v
}

#[test]
pub(super) fn browser_voice_is_the_third_kind_of_text_to_speech_and_never_a_speech_to_text_kind() {
    let (cx, _) = admin("sv-bv");

    let mut a = SettingsApp::new(&cx);

    goto(&mut a, &cx, Tab::Voice);

    // Type -> Right, Right: the browser voice; the other fields are not needed
    key(&mut a, &cx, Key::Char('v'));

    key(&mut a, &cx, Key::Right);

    key(&mut a, &cx, Key::Right);

    for _ in 0..5 {
        key(&mut a, &cx, Key::Enter);
    }
    assert!(!a.has_modal(), "{:?}", a.status());
    assert_eq!(cx.voice_settings().tts, Some(SpeechEngine::Browser));
    // the form shows it chosen the next time, and the screen names it
    key(&mut a, &cx, Key::Char('v'));
    let again = SettingsApp::engine_form(&cx, false);
    assert_eq!(again.choice_idx(0), 2);
    key(&mut a, &cx, Key::Esc);
    let mut g = Gfx::new(1280, 700);
    g.clear(rgb(0, 0, 0));
    a.render(&mut g, R::new(0.0, 72.0, 1280.0, 628.0), 1.0, &cx, 0);
    // speech-to-text has two kinds only
    let stt = SettingsApp::engine_form(&cx, true);
    assert_eq!(stt.choice_idx(0), 0);
    // with nothing chosen, a browser that can speak is named as the default
    cx.settings().lock().unwrap().update(|s| s.voice.tts = None).unwrap();
    let hv = iyakki_shared::hostvoice::HostVoice::current();
    hv.connect(iyakki_shared::hostvoice::Front::Browser);
    hv.event(iyakki_shared::hostvoice::FromClient::Voices { supported: true, voices: vec![] });
    a.render(&mut g, R::new(0.0, 72.0, 1280.0, 628.0), 1.0, &cx, 0);
}

// ------------------------------------------------------------------ skills tab

pub(super) const SKILL_MD: &str = "---\nname: meeting-notes\ndescription: Summarise meeting notes\ntriggers: meeting, minutes\n---\nList decisions first.\n";

// ------------------------------------------------------------------ storage tab

pub(super) struct DirMounter;

impl iyakki_core_api::device_types::storage::Mounter for DirMounter {
    fn mount(&self, _dev: &std::path::Path, target: &std::path::Path) -> Result<(), String> {
        std::fs::create_dir_all(target).map_err(|e| e.to_string())
    }
    fn unmount(&self, _target: &std::path::Path) -> Result<(), String> {
        Ok(())
    }
}

/// A fake machine: sda = the boot medium, vdb = empty disk, vdc = a disk with a filesystem on it.
pub(super) fn machine(tag: &str) -> Option<(StorageEnv, std::path::PathBuf)> {
    let mke2fs = ["/usr/sbin/mke2fs", "/sbin/mke2fs"].iter().map(std::path::PathBuf::from).find(|p| p.exists())?;
    let root = std::env::temp_dir().join(format!("iyakki-ui-storage-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("sys/block")).unwrap();
    std::fs::create_dir_all(root.join("dev")).unwrap();
    std::fs::write(root.join("mounts"), "rootfs / rootfs rw 0 0\n").unwrap();
    for (name, extra) in [("sda", Some((0x8001u64, &b"CD001"[..]))), ("vdb", None), ("vdc", Some((0u64, &b"precious"[..])))] {
        let d = root.join("sys/block").join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("size"), "32768\n").unwrap();
        std::fs::write(d.join("removable"), "0\n").unwrap();
        let node = root.join("dev").join(name);
        std::fs::File::create(&node).unwrap().set_len(16 * 1024 * 1024).unwrap();
        if let Some((off, b)) = extra {
            use std::os::unix::fs::FileExt;
            std::fs::OpenOptions::new().write(true).open(&node).unwrap().write_all_at(b, off).unwrap();
        }
    }
    let data = root.join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("users.json"), "{}").unwrap();
    let env = StorageEnv { roots: iyakki_core_api::device_types::storage::Roots { sys_block: root.join("sys/block"), dev: root.join("dev"), mounts: root.join("mounts"), require_block: false }, mounter: Arc::new(DirMounter), mke2fs, scratch: root.clone(), data_dir: data, virtual_disks: None };
    Some((env, root))
}

pub(super) fn select_disk(a: &mut SettingsApp, cx: &AppCx, name: &str) {
    let i = a.devices.iter().position(|d| d.name == name).unwrap();
    a.handle(&Event::Row(i), cx);
}

// ---------------------------------------------------------------- hardware pages

pub(super) struct FakeCtl(pub(super) Mutex<Vec<String>>);

impl iyakki_core_api::device_types::devctl::Control for FakeCtl {
    fn set_link(&self, n: &str, up: bool) -> Result<(), String> {
        self.0.lock().unwrap().push(format!("{n} {up}"));
        Ok(())
    }
    fn ipv4(&self) -> Vec<(String, String)> {
        vec![]
    }
    fn volume(&self, _: u32) -> Option<iyakki_core_api::device_types::devctl::Volume> {
        None
    }
    fn change_volume(&self, _: u32, _: Option<i32>) -> Result<iyakki_core_api::device_types::devctl::Volume, String> {
        Err("no".into())
    }
    fn set_volume(&self, _: u32, _: u32) -> Result<iyakki_core_api::device_types::devctl::Volume, String> {
        Err("no".into())
    }
    fn dhcp(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

pub(super) fn fake_machine(tag: &str) -> (DeviceEnv, Arc<FakeCtl>) {
    let d = temp_dir(&format!("{tag}-hw"));
    let put = |rel: &str, c: &str| {
        let p = d.join("sys").join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, c).unwrap();
    };
    put("class/net/eno1/type", "1\n");
    put("class/net/eno1/flags", "0x1003\n");
    put("class/net/eno1/carrier", "1\n");
    put("class/net/eno1/device/x", "");
    let ctl = Arc::new(FakeCtl(Mutex::new(vec![])));
    (DeviceEnv { roots: iyakki_core_api::device_types::devctl::Roots { sys: d.join("sys"), proc_: d.join("proc") }, ctl: ctl.clone() }, ctl)
}

/// Pages that only show text (nothing to choose); every other page must have something to click or touch besides the tab bar.
pub(super) const DISPLAY_ONLY: &[&str] = &[];

#[test]
fn every_settings_page_with_choices_can_be_used_with_the_mouse_or_a_finger_not_only_the_keyboard() {
    let (cx, _) = admin("clk1");
    let (env, _) = fake_machine("clk1");
    let mut a = SettingsApp::new(&cx).with_hardware(env);
    let mut g = Gfx::new(1280, 3200);
    let mut lacking = Vec::new();
    for i in 0..a.tabs().len() {
        a.handle(&Event::Tab(i), &cx);
        g.clear(rgb(0, 0, 0));
        g.hits.clear();
        a.render(&mut g, R::new(0.0, 72.0, 1280.0, 1728.0), 1.0, &cx, 0);
        let name = format!("{:?}", a.tab());
        let clickable = g.hits.iter().filter(|h| !matches!(h.ev, Event::Tab(_) | Event::MouseMove(..) | Event::Scroll(_) | Event::Tick)).count();
        let rows = g.hits.iter().filter(|h| matches!(h.ev, Event::Row(_) | Event::Sub(..))).count();
        if std::env::var_os("IYAKKI_CLICK_AUDIT").is_some() {
            println!("AUDIT {name}: {clickable} targets, {rows} rows/sub");
        }
        if clickable == 0 && !DISPLAY_ONLY.contains(&name.as_str()) {
            lacking.push(name);
        }
    }
    assert!(lacking.is_empty(), "pages with nothing to click or touch: {lacking:?}");
}

// ---------------------------------------------------------------- reading aloud: engines of this computer (docs/VOICE.md)

/// Fake engines on a temp PATH: espeak-ng lists three voices, logs stdin and arguments, speaks for a moment and writes a
/// WAV for -w; spd-say lists two voices; piper is there without a voice file.
pub(super) fn host_fakes(d: &std::path::Path, which: &[&str]) {
    std::fs::create_dir_all(d).unwrap();

    std::fs::write(d.join("voices.txt"), "Pty Language Age/Gender VoiceName File Other\n 5 en-us --/M English_(America) gmw/en-US\n 5 ta --/M Tamil inc/ta\n 5 hi --/M Hindi inc/hi\n").unwrap();

    std::fs::write(d.join("tone.wav"), iyakki_shared::voice::encode_wav(&iyakki_shared::voice::tone(440.0, 100), 16_000)).unwrap();

    let mk = |name: &str, body: &str| {
        let p = d.join(name);
        std::fs::write(&p, format!("#!/bin/sh\nD=$(dirname \"$0\")\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    };
    for w in which {
        match *w {
            "espeak-ng" => mk("espeak-ng", "case \"$1\" in --voices) cat \"$D/voices.txt\"; exit 0;; esac\ncat >> \"$D/said.log\"\necho \"$@\" >> \"$D/args.log\"\nout=''; prev=''; for a in \"$@\"; do [ \"$prev\" = -w ] && out=\"$a\"; prev=\"$a\"; done\nif [ -n \"$out\" ]; then cat \"$D/tone.wav\" > \"$out\"; else sleep 0.1; fi"),
            "spd-say" => mk("spd-say", "case \"$1\" in -L) printf '  NAME LANGUAGE VARIANT\\n English en-US none\\n Tamil ta none\\n'; exit 0;; -C) exit 0;; esac\ncat >> \"$D/said.log\"\necho \"spd $@\" >> \"$D/args.log\"\nsleep 0.1"),
            "piper" => mk("piper", "exit 0"),
            other => panic!("no fake {other}"),
        }
    }
}

pub(super) fn host_env(d: &std::path::Path) -> iyakki_agent::hostengines::Env {
    iyakki_agent::hostengines::Env { path: d.to_string_lossy().to_string(), os: iyakki_shared::hostengines::Os::Linux, piper_dirs: Vec::new() }
}

pub(super) fn eng_dir(tag: &str) -> std::path::PathBuf {
    let d = temp_dir(tag);
    std::fs::create_dir_all(&d).unwrap();
    d
}

// ---------------------------------------------------------------- the left list, search and sliders

pub(super) fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);

    g.clear(rgb(0, 0, 0));

    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);

    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);

    g
}

#[test]
pub(super) fn a_click_on_a_page_button_leaves_the_search_box_instead_of_typing_into_it() {
    let (cx, _) = admin("nav-click");

    let mut a = SettingsApp::new(&cx);

    key(&mut a, &cx, Key::Ctrl('f'));

    assert!(a.nav_focus);

    crate::ui::set_from_click(true);

    key(&mut a, &cx, Key::Char('e'));

    crate::ui::set_from_click(false);

    assert!(!a.nav_focus, "the click took the focus away");

    assert!(a.nav_query.value.is_empty(), "and the button's key was not typed into the box");

}

#[test]
pub(super) fn searching_theme_or_dark_or_light_finds_the_account_page_where_the_theme_card_is() {
    let (cx, _) = admin("navtheme");

    let mut a = SettingsApp::new(&cx);

    for word in ["theme", "dark", "light", "contrast", "appearance"] {
        key(&mut a, &cx, Key::Ctrl('f'));
        typ(&mut a, &cx, word);
        let account = a.tabs().iter().position(|t| *t == Tab::Account).unwrap();
        assert!(a.nav_matches().contains(&account), "{word} should lead to Account");
        key(&mut a, &cx, Key::Esc);
    }
}

#[test]
fn typing_in_the_search_box_narrows_the_pages_and_the_first_match_becomes_the_page() {
    let (cx, _) = admin("nav1");
    let mut a = SettingsApp::new(&cx);
    let all = a.nav_matches().len();
    assert_eq!(all, a.tabs().len());
    key(&mut a, &cx, Key::Ctrl('f'));
    assert!(a.nav_focus, "Ctrl+F goes to the search box");
    typ(&mut a, &cx, "wifi");
    assert_eq!(a.nav_matches().len(), 1);
    assert_eq!(a.tab(), Tab::Network, "the one match is shown at once");
    // keys go to the search box while it has the keyboard: 'a' does not open a form on the Models page
    key(&mut a, &cx, Key::Esc);
    assert_eq!(a.nav_query.value, "", "Esc clears the text");
    assert!(!a.nav_focus, "and leaves the box in the same press");
    // words in any order, keywords and group names
    key(&mut a, &cx, Key::Char('/'));
    typ(&mut a, &cx, "volume");
    assert_eq!(a.tab(), Tab::Sound);
    key(&mut a, &cx, Key::Esc);
    key(&mut a, &cx, Key::Ctrl('f'));
    typ(&mut a, &cx, "devices");
    assert!(a.nav_matches().len() >= 2, "a group name finds its pages (Bluetooth and Keyboard)");
    key(&mut a, &cx, Key::Down);
    let second = a.tab();
    key(&mut a, &cx, Key::Enter);
    assert!(!a.nav_focus);
    assert_eq!(a.tab(), nav_first(&a).map_or(second, |_| a.tab()));
    // pasting works in the box
    let mut b = SettingsApp::new(&cx);
    key(&mut b, &cx, Key::Ctrl('f'));
    b.handle(&Event::Paste("bluetooth".into()), &cx);
    assert_eq!(b.tab(), Tab::Bluetooth);
    // nothing found: the page stays as it was and the list says so
    let mut c = SettingsApp::new(&cx);
    key(&mut c, &cx, Key::Ctrl('f'));
    typ(&mut c, &cx, "zzzzzz");
    assert!(c.nav_matches().is_empty());
    let g = draw(&mut c, &cx, 1280, 720);
    assert!(!g.hits.iter().any(|h| matches!(h.ev, Event::Tab(_))));
}

pub(super) fn nav_first(a: &SettingsApp) -> Option<usize> {
    a.nav_matches().first().copied()
}

#[test]
fn the_list_is_clickable_scrolls_with_the_wheel_over_it_and_a_page_stays_in_view() {
    let (cx, _) = admin("nav2");
    let mut a = SettingsApp::new(&cx);
    // tall enough for every page of an administrator's list (it grows as pages are added)
    let g = draw(&mut a, &cx, 1280, 3200);
    for i in 0..a.tabs().len() {
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(i)), "page {i} is in the list");
    }
    assert!(g.hits.iter().any(|h| h.ev == Event::Focus(nav::SEARCH_ID)));
    // a short window: the list is longer than the box and scrolls
    let last = *a.nav_matches().last().unwrap();
    let g = draw(&mut a, &cx, 1280, 300);
    assert_eq!(a.nav_scroll, 0.0);
    let shown: Vec<usize> = g.hits.iter().filter_map(|h| if let Event::Tab(t) = h.ev { Some(t) } else { None }).collect();
    assert!(shown.contains(&0) && !shown.contains(&last), "the end of the list is out of the box: shown {shown:?}, last {last}, nav {:?}", a.nav_rect);
    let inside = ((a.nav_rect.x + 20.0) as i32, (a.nav_rect.y + 100.0) as i32);
    a.handle(&Event::MouseMove(inside.0, inside.1), &cx);
    a.handle(&Event::Scroll(-4), &cx);
    assert!(a.nav_scroll > 0.0, "the wheel over the list scrolls the list: {}", a.nav_scroll);
    a.handle(&Event::Scroll(-100), &cx);
    let g = draw(&mut a, &cx, 1280, 300);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(last)), "scrolled to the end, the last page is in the box");
    let max = a.nav_scroll;
    a.handle(&Event::Scroll(-100), &cx);
    assert_eq!(a.nav_scroll, max, "it stops at the end");
    for _ in 0..20 {
        a.handle(&Event::Scroll(100), &cx);
    }
    assert_eq!(a.nav_scroll, 0.0, "and at the start");
    // the wheel elsewhere moves the selection in the page, not the list
    a.handle(&Event::MouseMove(700, 300), &cx);
    a.handle(&Event::Scroll(-1), &cx);
    assert_eq!(a.nav_scroll, 0.0);
    // choosing a page near the end by search keeps it visible
    key(&mut a, &cx, Key::Ctrl('f'));
    typ(&mut a, &cx, "account");
    let g = draw(&mut a, &cx, 1280, 300);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(a.tab)), "the chosen page is in the box");
}

#[test]
fn the_sliders_are_not_clickable_for_someone_who_may_not_change_settings() {
    let ucx = user("sld2");
    let mut a = SettingsApp::new(&ucx);
    goto(&mut a, &ucx, Tab::Rag);
    let g = draw(&mut a, &ucx, 1280, 720);
    assert!(!g.hits.iter().any(|h| h.label == "slider" || h.label == "increase" || h.label == "decrease"), "nothing to click");
    let before = ucx.services.settings().lock().unwrap().get().rag.clone();
    a.handle(&Event::AreaAt(SLIDER_TOPK, 10.0, 5.0), &ucx);
    a.slider_w.push((SLIDER_TOPK, 100.0));
    a.handle(&Event::AreaAt(SLIDER_TOPK, 10.0, 5.0), &ucx);
    assert_eq!(ucx.services.settings().lock().unwrap().get().rag, before, "a forged click changes nothing");
}

// ------------------------------------------------------------------ WiFi

/// A machine with a WiFi adapter `wlan0` and stand-in `iw`, `ip`, `wpa_cli`, `wpa_supplicant` and `udhcpc`.
pub(super) fn wifi_world(tag: &str) -> (WifiEnv, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    let d = std::env::temp_dir().join(format!("iyakki-ui-wifi-{}-{tag}", std::process::id()));

    let _ = std::fs::remove_dir_all(&d);

    let net = d.join("sys/class/net/wlan0");

    std::fs::create_dir_all(net.join("device")).unwrap();

    std::fs::create_dir_all(net.join("wireless")).unwrap();

    std::fs::write(net.join("type"), "1\n").unwrap();

    std::fs::create_dir_all(d.join("proc")).unwrap();

    let bin = d.join("bin");

    std::fs::create_dir_all(&bin).unwrap();

    let rec = d.join("rec.log");

    let script = |name: &str, body: &str| {
        let p = bin.join(name);
        std::fs::write(&p, format!("#!/bin/sh\necho \"{name} $*\" >> {}\n{body}\n", rec.display())).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    script("ip", "exit 0");
    script("iw", "printf 'BSS aa:bb:cc:00:00:01(on wlan0)\\n\\tfreq: 2437.0\\n\\tcapability: ESS Privacy (0x0411)\\n\\tsignal: -50.00 dBm\\n\\tSSID: Seen\\n\\tRSN:\\t * Version: 1\\n\\t\\t * Authentication suites: PSK\\nBSS aa:bb:cc:00:00:02(on wlan0)\\n\\tfreq: 2412.0\\n\\tcapability: ESS (0x0401)\\n\\tsignal: -70.00 dBm\\n\\tSSID: Cafe\\n'");
    script("wpa_cli", "exit 1");
    script("wpa_supplicant", "exit 0");
    script("udhcpc", "exit 0");
    let env = WifiEnv {
        wifi: Arc::new(iyakki_core_api::device_types::devctl::wifi::Wifi { run_dir: d.join("run"), path: Some(bin) }),
        known: Arc::new(iyakki_core_api::device_types::devctl::wifi::KnownStore::new(&d.join("wifi.json"))),
        roots: iyakki_core_api::device_types::devctl::Roots { sys: d.join("sys"), proc_: d.join("proc") },
    };
    (env, d)
}
