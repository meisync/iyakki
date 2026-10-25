//! Settings → Sound: the outputs of a made-up Mac mini 2018 (HDMI devices 3, 7, 8 only; monitor PDM-15T on the first),
//! Automatic and a chosen output, the test chime going to the right device (a stand-in `aplay` records its arguments),
//! IYAKKI's volume and mute, who may change them, and geometry and clicks at four screen sizes.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use std::path::{Path, PathBuf};

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
fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}
fn goto(a: &mut SettingsApp, cx: &AppCx, t: Tab) {
    a.go_to(t);
}
fn settle(a: &mut SettingsApp, cx: &AppCx) {
    for _ in 0..2000 {
        a.handle(&Event::Tick, cx);
        if !a.is_busy() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("job did not finish");
}
fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
    g
}
fn click(a: &mut SettingsApp, cx: &AppCx, g: &Gfx, label: &str) {
    let h = g.hits.iter().find(|h| h.label == label).or_else(|| g.hits.iter().find(|h| h.label.contains(label))).unwrap_or_else(|| panic!("no `{label}` among {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>()));
    let ev = h.ev.clone();
    a.handle(&ev, cx);
}

fn put(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

struct NoMixer;
impl iyakki_devctl::Control for NoMixer {
    fn set_link(&self, _: &str, _: bool) -> Result<(), String> {
        Ok(())
    }
    fn ipv4(&self) -> Vec<(String, String)> {
        vec![]
    }
    fn volume(&self, _: u32) -> Option<iyakki_devctl::Volume> {
        None
    }
    fn change_volume(&self, _: u32, _: Option<i32>) -> Result<iyakki_devctl::Volume, String> {
        Err("no mixer".into())
    }
    fn set_volume(&self, _: u32, _: u32) -> Result<iyakki_devctl::Volume, String> {
        Err("no mixer".into())
    }
    fn dhcp(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

/// The Mac mini 2018's sound card under a temp folder; `monitor`: PDM-15T is plugged into the first port.
fn mac_mini(tag: &str, monitor: bool) -> (DeviceEnv, PathBuf) {
    let d = temp_dir(&format!("{tag}-snd"));
    let proc_ = d.join("proc");
    put(&proc_, "asound/cards", " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n                      HDA Intel PCH at 0xa1210000 irq 136\n");
    for (dev, id) in [(3, "HDMI 0"), (7, "HDMI 1"), (8, "HDMI 2")] {
        put(&proc_, &format!("asound/card0/pcm{dev}p/info"), &format!("card: 0\ndevice: {dev}\nstream: PLAYBACK\nid: {id}\nname: {id}\n"));
    }
    for p in 0..9 {
        let t = if monitor && p == 6 { "monitor_present\t\t1\neld_valid\t\t1\nmonitor_name\t\tPDM-15T\nsad_count\t\t1\n" } else { "monitor_present\t\t0\neld_valid\t\t0\n" };
        put(&proc_, &format!("asound/card0/eld#2.{p}"), t);
    }
    std::fs::create_dir_all(d.join("sys")).unwrap();
    (DeviceEnv { roots: iyakki_devctl::Roots { sys: d.join("sys"), proc_ }, ctl: Arc::new(NoMixer) }, d)
}

/// A stand-in `aplay` that writes its arguments and what it was given into `dir`; the play command names it by path.
fn fake_aplay(cx: &AppCx, dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let p = bin.join("aplay");
    std::fs::write(&p, format!("#!/bin/sh\necho \"$@\" > {0}/args\ncat > {0}/played.wav\n", dir.display())).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    cx.services.settings.lock().unwrap().update(|s| s.voice.play_cmd = format!("{} -q", p.display())).unwrap();
}

fn rows(a: &mut SettingsApp, cx: &AppCx) -> Vec<DevRow> {
    a.hw.set_sound(cx.services.sound_settings());
    a.dev_rows()
}

fn select(a: &mut SettingsApp, cx: &AppCx, f: impl Fn(&DevRow) -> bool) {
    let i = rows(a, cx).iter().position(f).expect("the row is there");
    a.handle(&Event::Row(i), cx);
}

#[test]
fn the_mac_mini_lists_its_hdmi_outputs_and_plays_on_the_monitor_automatically() {
    let cx = admin("snd1");
    let (env, d) = mac_mini("snd1", true);
    let mut a = SettingsApp::new(&cx).with_hardware(env);
    goto(&mut a, &cx, Tab::Sound);
    let r = rows(&mut a, &cx);
    assert_eq!(r[0].title, "Sound output · Automatic");
    assert!(r[0].sub.contains("playing on HDMI: PDM-15T") && r[0].sub.contains("connected over HDMI/DP"), "{}", r[0].sub);
    assert_eq!(r[0].chips[0].0, "AUTO");
    assert_eq!(r[1].title, "IYAKKI volume");
    assert!(r[1].sub.contains("applied by IYAKKI for HDMI"), "{}", r[1].sub);
    let titles: Vec<&str> = r.iter().map(|x| x.title.as_str()).collect();
    for t in ["HDMI: PDM-15T", "HDMI 2 (no monitor)", "HDMI 3 (no monitor)"] {
        assert!(titles.contains(&t), "{t} in {titles:?}");
    }
    let pdm = r.iter().find(|x| x.title == "HDMI: PDM-15T").unwrap();
    assert_eq!(pdm.chips[0].0, "IN USE");
    assert!(pdm.sub.contains("plughw:0,3"), "{}", pdm.sub);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn an_administrator_chooses_an_output_it_is_saved_audited_and_automatic_comes_back() {
    let cx = admin("snd2");
    let (env, d) = mac_mini("snd2", true);
    let mut a = SettingsApp::new(&cx).with_hardware(env);
    goto(&mut a, &cx, Tab::Sound);
    key(&mut a, &cx, Key::Char('o'));
    assert!(a.has_modal(), "the list of outputs is open");
    // pick the second HDMI port from the list
    let item = "HDMI 2 (no monitor) · no monitor  [PCH:7]";
    a.apply_pick(&cx, PickKind::SoundOutput, item);
    a.modal = None;
    assert_eq!(cx.services.sound_settings().output, "PCH:7");
    assert!(status(&a).contains("Sound output: HDMI 2"), "{}", status(&a));
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("settings.sound.output") && l.contains("PCH:7")));
    let r = rows(&mut a, &cx);
    assert_eq!(r[0].title, "Sound output · chosen");
    assert_eq!(r[0].chips[0].0, "CHOSEN");
    assert_eq!(r.iter().find(|x| x.title.starts_with("HDMI 2")).unwrap().chips[0].0, "IN USE");
    // Enter on an output row uses it; Automatic from the list goes back to following what is connected
    select(&mut a, &cx, |x| x.title == "HDMI: PDM-15T");
    key(&mut a, &cx, Key::Enter);
    assert_eq!(cx.services.sound_settings().output, "PCH:3");
    a.apply_pick(&cx, PickKind::SoundOutput, "Automatic (follows what is connected)");
    assert_eq!(cx.services.sound_settings().output, "");
    assert!(status(&a).contains("Automatic, now HDMI: PDM-15T"), "{}", status(&a));
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn the_test_chime_goes_to_the_selected_output_or_the_one_in_use_with_the_device_after_aplay() {
    let cx = admin("snd3");
    let (env, d) = mac_mini("snd3", true);
    fake_aplay(&cx, &d);
    let mut a = SettingsApp::new(&cx).with_hardware(env);
    goto(&mut a, &cx, Tab::Sound);
    key(&mut a, &cx, Key::Char('t'));
    settle(&mut a, &cx);
    assert_eq!(status(&a), "Played a short chime on plughw:0,3.");
    assert_eq!(std::fs::read_to_string(d.join("args")).unwrap().trim(), "-D plughw:0,3 -q", "no shell, -D right after the program");
    let wav = iyakki_agent::voice::parse_wav(&std::fs::read(d.join("played.wav")).unwrap()).unwrap();
    assert!(wav.samples.len() > 8000 && iyakki_agent::voice::peak(&wav.samples) > 1000, "a real, gentle chime");
    // on the selected row's output
    select(&mut a, &cx, |x| x.title.starts_with("HDMI 3"));
    key(&mut a, &cx, Key::Char('t'));
    settle(&mut a, &cx);
    assert_eq!(std::fs::read_to_string(d.join("args")).unwrap().trim(), "-D plughw:0,8 -q");
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("settings.sound.test") && l.contains("plughw:0,8")));
    // a play command that names its own device keeps it for ordinary sound; Test sound on one output uses that output
    cx.services.settings.lock().unwrap().update(|s| s.voice.play_cmd = format!("{}/bin/aplay -q -D hw:5,0", d.display())).unwrap();
    select(&mut a, &cx, |x| x.title.starts_with("HDMI 2"));
    key(&mut a, &cx, Key::Char('t'));
    settle(&mut a, &cx);
    assert_eq!(std::fs::read_to_string(d.join("args")).unwrap().trim(), "-D plughw:0,7 -q");
    // muted: nothing plays, and it says why
    cx.services.update_sound(|s| s.muted = true).unwrap();
    key(&mut a, &cx, Key::Char('t'));
    settle(&mut a, &cx);
    assert!(status(&a).contains("muted"), "{}", status(&a));
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn without_a_monitor_the_mac_mini_says_there_is_no_output_and_the_test_explains() {
    let cx = admin("snd4");
    let (env, d) = mac_mini("snd4", false);
    fake_aplay(&cx, &d);
    let mut a = SettingsApp::new(&cx).with_hardware(env);
    goto(&mut a, &cx, Tab::Sound);
    let r = rows(&mut a, &cx);
    assert_eq!(r[0].chips[0].0, "NO OUTPUT");
    assert_eq!(r[0].sub, iyakki_agent::audioout::NO_OUTPUT);
    key(&mut a, &cx, Key::Char('t'));
    assert!(!a.is_busy());
    assert!(status(&a).contains("T2 Macs"), "{}", status(&a));
    assert!(!d.join("args").exists(), "nothing was played");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn iyakki_volume_and_mute_are_saved_from_keys_buttons_and_the_slider_and_only_by_administrators() {
    let cx = admin("snd5");
    let (env, d) = mac_mini("snd5", true);
    let mut a = SettingsApp::new(&cx).with_hardware(env.clone());
    goto(&mut a, &cx, Tab::Sound);
    key(&mut a, &cx, Key::Char('-'));
    assert_eq!(cx.services.sound_settings().volume_pct, 95);
    assert_eq!(status(&a), "IYAKKI volume 95%.");
    key(&mut a, &cx, Key::Char('m'));
    assert!(cx.services.sound_settings().muted);
    assert_eq!(status(&a), "IYAKKI sound muted.");
    assert_eq!(rows(&mut a, &cx)[1].chips[0].0, "muted");
    key(&mut a, &cx, Key::Char('+'));
    let s = cx.services.sound_settings();
    assert!(!s.muted && s.volume_pct == 100, "louder unmutes: {s:?}");
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("settings.sound.volume")));
    // the slider's track and its buttons
    let g = draw(&mut a, &cx, 1280, 720);
    let row = rows(&mut a, &cx).iter().position(|r| matches!(r.target, Target::SoftVolume)).unwrap();
    assert!(g.hits.iter().any(|h| h.ev == Event::Area(SLIDER_HW + row)), "the volume track takes clicks");
    let w = a.slider_w.iter().find(|(i, _)| *i == SLIDER_HW + row).unwrap().1;
    a.handle(&Event::AreaAt(SLIDER_HW + row, w * 0.4, 5.0), &cx);
    assert_eq!(cx.services.sound_settings().volume_pct, 40);
    a.handle(&Event::Sub(row as u8, Key::Char('+')), &cx);
    assert_eq!(cx.services.sound_settings().volume_pct, 45);
    // a person who is not an administrator changes nothing
    let mut ucx = admin("snd5u");
    ucx.services = cx.services.clone();
    ucx.session.user = "bob".into();
    ucx.session.role = iyakki_sec::policy::Role::User;
    ucx.session.policy = iyakki_sec::policy::Role::User.template();
    let mut b = SettingsApp::new(&ucx).with_hardware(env);
    b.tabs.push(Tab::Sound);
    b.tab = b.tabs.len() - 1;
    for k in ['-', 'm', 'o'] {
        key(&mut b, &ucx, Key::Char(k));
    }
    b.apply_pick(&ucx, PickKind::SoundOutput, "HDMI 3 (no monitor) · no monitor  [PCH:8]");
    let s = cx.services.sound_settings();
    assert!(s.volume_pct == 45 && !s.muted && s.output.is_empty(), "{s:?}");
    assert!(!b.has_modal());
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn the_sound_page_fits_and_its_buttons_rows_and_list_are_clickable_at_four_screen_sizes() {
    let cx = admin("snd6");
    let (env, d) = mac_mini("snd6", true);
    fake_aplay(&cx, &d);
    let mut a = SettingsApp::new(&cx).with_hardware(env);
    goto(&mut a, &cx, Tab::Sound);
    let buttons = ["Output", "Test sound", "Louder", "Quieter", "Mute", "Look again"];
    for (sw, sh) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let what = format!("at {sw}x{sh}");
        a.handle(&Event::Row(0), &cx);
        let g = draw(&mut a, &cx, sw, sh);
        let inside = |h: &crate::ui::Hit| h.rect.x >= -0.5 && h.rect.y >= 0.0 && h.rect.x + h.rect.w <= sw as f32 + 0.5 && h.rect.y + h.rect.h <= sh as f32 + 0.5;
        let bar: Vec<&crate::ui::Hit> = g.hits.iter().filter(|h| buttons.contains(&h.label.as_str())).collect();
        assert_eq!(bar.len(), buttons.len(), "{what}: every button: {:?}", bar.iter().map(|h| &h.label).collect::<Vec<_>>());
        for (i, x) in bar.iter().enumerate() {
            assert!(inside(x), "{what}: {} is outside the window {:?}", x.label, x.rect);
            for y in &bar[i + 1..] {
                let ox = (x.rect.x + x.rect.w).min(y.rect.x + y.rect.w) - x.rect.x.max(y.rect.x);
                let oy = (x.rect.y + x.rect.h).min(y.rect.y + y.rect.h) - x.rect.y.max(y.rect.y);
                assert!(ox <= 1.5 || oy <= 1.5, "{what}: {} and {} overlap", x.label, y.label);
            }
        }
        for needed in ["Sound output · Automatic", "IYAKKI volume"] {
            let h = g.hits.iter().find(|h| h.label == needed).unwrap_or_else(|| panic!("{what}: {needed} missing: {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>()));
            assert!(inside(h), "{what}: {needed} outside {:?}", h.rect);
        }
        // the last output row comes into view when it is selected (small screens leave rows above out)
        let last = rows(&mut a, &cx).iter().position(|r| r.title.starts_with("HDMI 3")).unwrap();
        a.handle(&Event::Row(last), &cx);
        let g = draw(&mut a, &cx, sw, sh);
        let h = g.hits.iter().find(|h| h.ev == Event::Row(last)).unwrap_or_else(|| panic!("{what}: the selected row is drawn"));
        assert!(inside(h) && h.rect.y + h.rect.h <= sh as f32 - 60.0, "{what}: the selected row is on the page {:?}", h.rect);
        // the Output button opens the list; its lines are on screen and picking one is applied
        a.handle(&Event::Row(0), &cx);
        let g = draw(&mut a, &cx, sw, sh);
        click(&mut a, &cx, &g, "Output");
        assert!(a.has_modal(), "{what}: the list is open");
        let g2 = draw(&mut a, &cx, sw, sh);
        for line in ["Automatic", "HDMI: PDM-15T", "HDMI 2 (no monitor)", "HDMI 3 (no monitor)"] {
            let h = g2.hits.iter().find(|h| h.label.contains(line)).unwrap_or_else(|| panic!("{what}: {line} missing from the list"));
            assert!(inside(h), "{what}: {line} outside {:?}", h.rect);
        }
        click(&mut a, &cx, &g2, "HDMI 2 (no monitor)");
        if a.has_modal() {
            key(&mut a, &cx, Key::Enter); // a picker that selects on the first click and opens on the second
        }
        assert_eq!(cx.services.sound_settings().output, "PCH:7", "{what}");
        assert!(!a.has_modal());
        cx.services.update_sound(|s| s.output.clear()).unwrap();
        // Test sound by click
        let g = draw(&mut a, &cx, sw, sh);
        click(&mut a, &cx, &g, "Test sound");
        settle(&mut a, &cx);
        assert!(status(&a).starts_with("Played a short chime on plughw:0,3"), "{what}: {}", status(&a));
    }
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn the_advanced_sound_rows_toggle_with_a_and_the_boost_and_the_hardware_answer_are_saved_per_card() {
    let cx = admin("snd-adv");
    let (env, d) = mac_mini("snd-adv", true);
    let mut a = SettingsApp::new(&cx).with_hardware(env);
    goto(&mut a, &cx, Tab::Sound);
    assert!(!rows(&mut a, &cx).iter().any(|r| matches!(r.target, Target::SoundPattern(false))));
    key(&mut a, &cx, Key::Char('A'));
    assert!(rows(&mut a, &cx).iter().any(|r| matches!(r.target, Target::SoundPattern(false))), "the stereo test is under Advanced");
    // boost: saved per card, capped, audited; 0 removes it
    a.sound_set_boost(&cx, "PCH", 6);
    assert_eq!(cx.services.sound_settings().boost_for("PCH"), 6);
    a.sound_set_boost(&cx, "PCH", 40);
    assert_eq!(cx.services.sound_settings().boost_for("PCH"), iyakki_agent::softvol::MAX_BOOST_DB);
    a.sound_set_boost(&cx, "PCH", 0);
    assert!(cx.services.sound_settings().boosts.is_empty());
    // "Did the second tone sound quieter?": y keeps the hardware control, n chooses the software volume
    a.hw_check_done("PCH".into(), Ok("Played.".into()));
    assert!(matches!(a.modal, Some(Modal::Confirm(ConfirmKind::SoundHwWorks(_), _))));
    a.handle(&Event::Key(Key::Char('n')), &cx);
    assert_eq!(cx.services.sound_settings().volume_mode_for("PCH"), iyakki_agent::audioout::VolumeMode::Software);
    a.hw_check_done("PCH".into(), Ok("Played.".into()));
    a.handle(&Event::Key(Key::Char('y')), &cx);
    assert_eq!(cx.services.sound_settings().volume_mode_for("PCH"), iyakki_agent::audioout::VolumeMode::Hardware);
    a.hw_check_done("PCH".into(), Ok("Played.".into()));
    a.handle(&Event::Key(Key::Esc), &cx);
    assert_eq!(cx.services.sound_settings().volume_mode_for("PCH"), iyakki_agent::audioout::VolumeMode::Hardware, "Esc changes nothing");
    let _ = std::fs::remove_dir_all(d);
}
