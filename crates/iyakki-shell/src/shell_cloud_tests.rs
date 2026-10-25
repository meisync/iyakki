//! Cloud mode in the shell: the class of every app, tiles, number keys, F-keys, the Fn keys, the Settings pages, the Devices
//! popover, the IYAKKI menu, About, the person's own layout and the whole screen at two sizes.

use super::tests::*;
use super::*;
use crate::apps::settings::Tab;
use iyakki_agent::cloudmode::{self, Class, Facts, Setting};

fn bare() -> cloudmode::CloudMode {
    cloudmode::cloud_mode(Setting::Auto, &Facts::bare())
}

fn forced() -> cloudmode::CloudMode {
    cloudmode::cloud_mode(Setting::On, &Facts::full())
}

fn signed_in(tag: &str, c: cloudmode::CloudMode) -> Shell {
    let (mut sh, _) = shell_with_users(tag);
    sh.set_cloud_for_test(c);
    login(&mut sh, "root", "rootpass12");
    sh
}

fn labels(sh: &Shell) -> Vec<&'static str> {
    sh.visible_tiles().iter().map(|t| t.label).collect()
}

const HARDWARE: [PApp; 6] = [PApp::Radio, PApp::Media, PApp::Tales, PApp::Voice, PApp::Camera, PApp::Disks];

#[test]
fn every_app_has_a_class_and_every_tile_is_hardware_or_remote_safe() {
    let mut hw = Vec::new();
    for a in PApp::ALL {
        match cloudmode::app_class(a) {
            Class::Hardware(_) => hw.push(a),
            Class::Remote => {}
        }
        // an app with no tile is the inbox (opened from the bell); everything else is on the grid or in the IYAKKI menu
        assert!(a == PApp::Inbox || TILES.iter().any(|t| t.app == a), "{a:?} has a tile");
    }
    hw.sort();
    let mut want = HARDWARE.to_vec();
    want.sort();
    assert_eq!(hw, want, "MEDIA, RADIO, TALES, VOICE, CAMERA and DISKS are the apps that need local hardware");
    for t in TILES.iter() {
        let kept = matches!(cloudmode::app_class(t.app), Class::Remote);
        assert_eq!(kept, !HARDWARE.contains(&t.app), "{}", t.label);
    }
}

#[test]
fn tiles_follow_the_mode() {
    let (mut sh, _) = shell_with_users("cloud-tiles");
    login(&mut sh, "root", "rootpass12");
    let all = labels(&sh);
    assert!(all.contains(&"MEDIA") && all.contains(&"RADIO") && all.contains(&"CAMERA") && all.contains(&"TALES") && all.contains(&"VOICE"), "desktop shows everything");
    sh.set_cloud_for_test(bare());
    let cloud = labels(&sh);
    for l in ["MEDIA", "RADIO", "CAMERA", "TALES", "VOICE", "DISKS"] {
        assert!(!cloud.contains(&l), "{l} is hidden");
    }
    for l in ["CHAT", "RAG", "MCP", "CLI", "SKILLS", "CALENDAR", "AI IDE", "EDIT", "DATA", "DASH", "FLOWS", "RUNS", "SRE PILOT", "SYSTEM", "STORE", "USERS", "USAGE", "NET SCAN", "MESH", "BROWSER", "SETTINGS"] {
        assert!(cloud.contains(&l), "{l} stays");
    }
    assert_eq!(cloud.len(), all.len() - 6);
    // auto on a machine that has sound and a camera but no screen: those stay, nothing else is hidden
    sh.set_cloud_for_test(cloudmode::cloud_mode(Setting::Auto, &Facts { display: false, ..Facts::full() }));
    assert_eq!(labels(&sh), all, "hardware that exists stays visible in auto");
    // on: hidden whatever the machine has
    sh.set_cloud_for_test(forced());
    assert_eq!(labels(&sh), cloud);
    // a machine with a camera but no sound card
    sh.set_cloud_for_test(cloudmode::cloud_mode(Setting::Auto, &Facts { audio: false, ..Facts::full() }));
    let v = labels(&sh);
    assert!(v.contains(&"CAMERA") && !v.contains(&"MEDIA") && !v.contains(&"RADIO") && !v.contains(&"TALES") && !v.contains(&"VOICE"));
    sh.set_cloud_for_test(cloudmode::CloudMode::desktop());
    assert_eq!(labels(&sh), all);
}

#[test]
fn number_keys_count_the_tiles_that_are_shown() {
    let mut sh = signed_in("cloud-keys", bare());
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    let shown = sh.visible_tiles();
    for (i, t) in shown.iter().take(9).enumerate() {
        sh.handle(&Event::Key(Key::Char(char::from(b'1' + i as u8))));
        assert_eq!(sh.screen_name(), format!("app:{}", t.app.id()), "key {} opens {}", i + 1, t.label);
        sh.handle(&Event::Key(Key::Esc));
        assert_eq!(sh.screen_name(), "launcher");
    }
    // nothing opens a hidden app: not a key, not a jump, not a request from another app
    for a in HARDWARE {
        sh.open(a);
        assert_eq!(sh.screen_name(), "launcher", "{a:?} stays shut in cloud mode");
    }
}

#[test]
fn hidden_apps_have_no_f_key_or_jump() {
    let mut sh = signed_in("cloud-fkeys", bare());
    sh.jump_to("radio", "");
    sh.jump_to("camera", "");
    sh.jump_to("media", "");
    assert_eq!(sh.screen_name(), "launcher");
    sh.jump_to("chat", "");
    assert_eq!(sh.screen_name(), "app:chat", "a remote-safe app still opens");
}

#[test]
fn fn_keys_do_nothing_in_cloud_mode_but_the_power_keys_still_do() {
    let mut sh = signed_in("cloud-fn", bare());
    sh.handle(&Event::Fn(crate::fnkeys::FnKey::VolumeUp, true));
    sh.handle(&Event::Fn(crate::fnkeys::FnKey::BrightnessDown, true));
    sh.handle(&Event::Fn(crate::fnkeys::FnKey::PlayPause, true));
    assert!(sh.fnk.osd.is_none(), "no on-screen display");
    sh.handle(&Event::Fn(crate::fnkeys::FnKey::VolumeUp, false));
    sh.handle(&Event::Fn(crate::fnkeys::FnKey::Power, true));
    assert!(sh.fnk.power_card, "the power key still opens the power card");
    // a desktop shows the display for the same key
    let mut d = signed_in("cloud-fn-desk", cloudmode::CloudMode::desktop());
    d.handle(&Event::Fn(crate::fnkeys::FnKey::VolumeUp, true));
    assert!(d.fnk.osd.is_some());
}

#[test]
fn settings_pages_follow_the_mode() {
    let cx = crate::apps::tests::cx_with_admin_store("cloud-pages");
    let names = |c: cloudmode::CloudMode| -> Vec<&'static str> { SettingsApp::new(&cx).with_cloud(c).tabs().iter().map(|t| t.label()).collect() };
    let desk = names(cloudmode::CloudMode::desktop());
    for p in ["Sound", "System sounds", "Display", "Screen saver", "Keyboard", "Bluetooth", "Network", "Cloud mode"] {
        assert!(desk.contains(&p), "{p}");
    }
    let cloud = names(bare());
    for p in ["Sound", "System sounds", "Display", "Displays", "Screen saver", "Keyboard", "Bluetooth"] {
        assert!(!cloud.contains(&p), "{p} is hidden");
    }
    for p in ["Network", "Storage", "Time", "Models", "Cloud mode", "Browser"] {
        assert!(cloud.contains(&p), "{p} stays");
    }
    // Displays and Screen saver go with Display (the same hardware name hides all three)
    assert_eq!(cloud.len(), desk.len() - 7);
    // auto on a cloud image that has the hardware: every page stays; without a screen only Display goes
    let auto = names(cloudmode::cloud_mode(Setting::Auto, &Facts { variant: "cloud".into(), ..Facts::full() }));
    assert_eq!(auto, desk, "pages of hardware that exists stay");
    let noscreen = names(cloudmode::cloud_mode(Setting::Auto, &Facts { display: false, ..Facts::full() }));
    assert!(!noscreen.contains(&"Display") && !noscreen.contains(&"Displays") && !noscreen.contains(&"Screen saver") && noscreen.contains(&"Sound") && noscreen.contains(&"Keyboard"));
    assert!(!names(forced()).contains(&"Sound"));
    // every table entry is a page some Tab claims, and every Tab claims a name the table knows
    let claimed: Vec<&str> = [Tab::Sound, Tab::Display, Tab::Keyboard, Tab::Bluetooth].iter().filter_map(|t| t.cloud_page()).collect();
    let table: Vec<&str> = cloudmode::SETTINGS_PAGES.iter().map(|p| p.0).collect();
    assert_eq!(claimed, table);
}

#[test]
fn the_devices_popover_leaves_out_hardware_rows() {
    use crate::devpop::{without_rows, Item, Jump, Level, Snapshot};
    let row = |t: &str| Item { title: t.into(), detail: String::new(), level: Level::Ok, jump: Jump::None, acts: vec![] };
    let snap = Snapshot { items: ["Ethernet", "Wi-Fi", "Sound", "Display", "Camera", "Bluetooth", "Keyboard and pointer", "Storage", "Battery"].iter().map(|t| row(t)).collect(), driver_help: vec![], at: 1 };
    let c = bare();
    let left: Vec<String> = without_rows(snap.clone(), &c.hidden_device_rows).items.into_iter().map(|i| i.title).collect();
    assert_eq!(left, ["Ethernet", "Wi-Fi", "Storage"]);
    let auto = cloudmode::cloud_mode(Setting::Auto, &Facts { audio: false, ..Facts::full() });
    let left: Vec<String> = without_rows(snap, &auto.hidden_device_rows).items.into_iter().map(|i| i.title).collect();
    assert!(!left.contains(&"Sound".to_string()) && left.contains(&"Camera".to_string()) && left.contains(&"Battery".to_string()));
}

#[test]
fn the_iyakki_menu_leaves_out_hardware_entries_and_their_keys() {
    use crate::brandmenu::{BrandMenu, Page};
    let mut m = BrandMenu::new();
    let titles = |m: &BrandMenu| m.shown().iter().map(|e| e.0).collect::<Vec<_>>();
    assert!(titles(&m).contains(&Page::Media) && titles(&m).contains(&Page::Mac));
    m.set_cloud(bare());
    assert!(!titles(&m).contains(&Page::Media) && !titles(&m).contains(&Page::Mac));
    for p in [Page::About, Page::Setup, Page::Settings] {
        assert!(titles(&m).contains(&p), "{p:?} stays");
    }
    m.toggle();
    assert_eq!(m.open_page(Page::Mac), crate::brandmenu::Outcome::Nothing, "no jump reaches a hidden page");
    m.handle_key(&Key::Char('m'));
    m.handle_key(&Key::Char('d'));
    assert!(m.shown().iter().all(|e| e.0 != Page::Media), "the letter keys of hidden entries do nothing");
    // a Mac keeps its drivers entry in automatic mode
    m.set_cloud(cloudmode::cloud_mode(Setting::Auto, &Facts { audio: false, ..Facts::full() }));
    assert!(titles(&m).contains(&Page::Mac) && !titles(&m).contains(&Page::Media));
}

#[test]
fn about_says_whether_cloud_mode_is_on_and_why() {
    let on = crate::about::cloud_fact(&bare());
    assert_eq!(on.0, "Cloud mode");
    assert!(on.1.starts_with("On.") && on.1.contains("no screen"), "{}", on.1);
    let off = crate::about::cloud_fact(&cloudmode::CloudMode::desktop());
    assert!(off.1.starts_with("Off."));
    let lines = crate::about::this_machine_with(Some(&forced()));
    assert!(lines.iter().any(|(t, _)| t.contains("Cloud mode") && t.contains("setting is On")), "About carries the row");
    assert!(!crate::about::this_machine().iter().any(|(t, _)| t.contains("Cloud mode")));
}

#[test]
fn moved_tiles_stay_hidden_and_empty_groups_hide_as_before() {
    let mut sh = signed_in("cloud-layout", bare());
    let radio = iyakki_agent::layout::sys_key("RADIO");
    sh.layout.placed.insert(radio, Group::Home.id().to_string());
    let home = sh.visible_tiles().into_iter().filter(|t| sh.tile_gid(t) == Group::Home.id()).map(|t| t.label).collect::<Vec<_>>();
    assert!(!home.contains(&"RADIO"), "a hidden tile moved to Home stays hidden: {home:?}");
    // Web & Media holds BROWSER, which is remote-safe, so the group stays; move BROWSER away and the group has nothing to show
    let chips = |sh: &Shell| sh.groups().iter().map(|x| x.label().to_string()).collect::<Vec<_>>();
    assert!(chips(&sh).contains(&Group::WebMedia.label().to_string()));
    sh.layout.placed.insert(iyakki_agent::layout::sys_key("BROWSER"), Group::Home.id().to_string());
    assert!(!chips(&sh).contains(&Group::WebMedia.label().to_string()), "a group whose tiles are all hidden or moved away is not shown");
    sh.set_cloud_for_test(cloudmode::CloudMode::desktop());
    assert!(chips(&sh).contains(&Group::WebMedia.label().to_string()), "on a desktop the hidden tiles bring the group back");
}

#[test]
fn the_whole_shell_draws_in_cloud_mode_at_two_sizes() {
    for (w, h) in [(1280u32, 720u32), (800, 480)] {
        let mut sh = signed_in(&format!("cloud-draw-{w}"), bare());
        let mut g = Gfx::new(w, h);
        sh.render(&mut g);
        let blank = Gfx::new(w, h);
        assert_ne!(g.rgba(), blank.rgba(), "{w}x{h}: the launcher is drawn");
        // every shown tile has a hit rectangle inside the window; the hidden ones are not drawn
        assert!(sh.hits().iter().all(|hit| hit.label != "RADIO" && hit.label != "MEDIA" && hit.label != "CAMERA"), "{w}x{h}");
        // the devices popover, the IYAKKI menu, Settings and a page of it all draw
        sh.handle(&Event::Key(Key::F(8)));
        sh.render(&mut g);
        sh.handle(&Event::Key(Key::Esc));
        sh.handle(&Event::Key(Key::F(10)));
        sh.render(&mut g);
        sh.handle(&Event::Key(Key::Esc));
        let cx = crate::apps::tests::cx_with_admin_store(&format!("cloud-set-{w}"));
        let mut app = SettingsApp::new(&cx).with_cloud(bare());
        while app.tab() != Tab::Cloud {
            app.handle(&Event::Key(Key::Right), &cx);
        }
        let mut g2 = Gfx::new(w, h);
        app.render(&mut g2, R::new(0.0, 0.0, w as f32, h as f32), scale_for(w, h), &cx, 0);
    }
}

#[test]
fn a_sound_card_that_appears_late_brings_the_sound_apps_back() {
    // signed in before the card exists: automatic Cloud mode hides the apps that need sound
    let mut sh = signed_in("cloud-late", cloudmode::cloud_mode(Setting::Auto, &Facts { audio: false, ..Facts::full() }));
    let early = labels(&sh);
    for l in ["RADIO", "MEDIA", "TALES", "VOICE"] {
        assert!(!early.contains(&l), "{l} hidden without a sound card");
    }
    // the card registers: the next look at the machine brings them back
    assert!(sh.refresh_cloud(cloudmode::CloudMode::desktop()), "it changed");
    let later = labels(&sh);
    for l in ["RADIO", "MEDIA", "TALES", "VOICE"] {
        assert!(later.contains(&l), "{l} is back");
    }
    assert!(!sh.refresh_cloud(cloudmode::CloudMode::desktop()), "no change the second time");
}
