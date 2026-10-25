//! Settings → Displays: who sees the page, the page without and with the shell driving the displays (a made-up card), editing
//! the draft, Apply with the 15-second trial, Keep and Go back, Identify, the Screen features switch and the page at four sizes.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use crate::backend_kms::Kms;
use crate::display::kms::FakeDrm;
use crate::display::outputs::{hub, now_ms, DisplayConfig, Placement, Request};

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

fn press(a: &mut SettingsApp, cx: &AppCx, c: char) {
    a.handle(&Event::Key(Key::Char(c)), cx);
}

fn goto(a: &mut SettingsApp, cx: &AppCx) {
    a.go_to(Tab::Displays);
}

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
    g
}

fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}

fn card() -> Kms<FakeDrm> {
    let edp = FakeDrm::conn(10, "eDP-1", true, &[(2560, 1600, 60), (1920, 1200, 60)], 0b011);
    let hdmi = FakeDrm::conn(11, "HDMI-A-1", true, &[(1920, 1080, 60), (1280, 720, 60)], 0b110);
    Kms::open(FakeDrm::new(3, vec![edp, hdmi]), DisplayConfig::default(), 4).unwrap()
}

fn publish(k: &Kms<FakeDrm>) {
    hub().set_status(k.status(now_ms()));
}

#[test]
fn only_administrators_have_the_page_and_it_sits_with_the_devices() {
    let cx = admin("sd1");
    let a = SettingsApp::new(&cx);
    assert!(a.tabs().contains(&Tab::Displays));
    assert_eq!(Tab::Displays.group(), "Display & sound");
    assert_eq!(Tab::Displays.label(), "Displays");
    let mut plain = cx_with_admin_store("sd1b");
    plain.session.policy = iyakki_sec::policy::Role::User.template();
    plain.session.role = iyakki_sec::policy::Role::User;
    let b = SettingsApp::new(&plain);
    assert!(!b.tabs().contains(&Tab::Displays));
}

#[test]
fn without_the_shell_driving_the_displays_the_page_lists_what_is_connected_and_says_why_nothing_changes() {
    let cx = admin("sd2");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx);
    hub().set_status(crate::display::outputs::Status {
        kms: false,
        why_not: "one display is connected".into(),
        outputs: vec![crate::display::outputs::OutView { name: "eDP-1".into(), connected: true, modes: vec![(2560, 1600, 0)], current: Some((2560, 1600)), pos: (0, 0), primary: false }],
        ..Default::default()
    });
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "display eDP-1"));
    press(&mut a, &cx, 'r');
    assert!(status(&a).contains("does not drive the displays") && status(&a).contains("one display is connected"), "{}", status(&a));
    press(&mut a, &cx, 'y');
    assert!(hub().take_requests().is_empty(), "nothing is asked of a loop that is not there");
    // Identify still works: a number on the one picture
    press(&mut a, &cx, 'i');
    assert!(hub().identifying(now_ms() + 100));
}

#[test]
fn editing_the_draft_and_applying_asks_the_display_loop_and_the_trial_can_be_kept_or_undone() {
    let cx = admin("sd3");
    let mut k = card();
    publish(&k);
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx);
    draw(&mut a, &cx, 1280, 720);
    // select the monitor, pick another resolution, put it below, make it the main display
    a.handle(&Event::Key(Key::Down), &cx);
    press(&mut a, &cx, 'r');
    assert!(status(&a).contains("HDMI-A-1: 1280x720"), "{}", status(&a));
    press(&mut a, &cx, 'a');
    assert!(status(&a).contains("below"), "{}", status(&a));
    press(&mut a, &cx, 'y');
    let reqs = hub().take_requests();
    assert_eq!(reqs.len(), 1);
    let Request::Apply(cfg) = reqs[0].clone() else { panic!("{reqs:?}") };
    assert_eq!(cfg.mode_of("HDMI-A-1"), Some((1280, 720)));
    assert_eq!(cfg.placement_of("HDMI-A-1"), Placement::Below);
    // the loop applies it; the page shows the trial
    let t0 = now_ms();
    k.process_requests(reqs, t0);
    hub().set_status(k.status(t0));
    let g = draw(&mut a, &cx, 1280, 720);
    let btn = a.displays_buttons();
    assert!(btn.iter().any(|b| b.0 == "Keep") && btn.iter().any(|b| b.0 == "Go back"));
    assert!(hub().status().trial_secs == Some(15));
    let _ = g;
    // keep
    press(&mut a, &cx, 'k');
    assert_eq!(hub().take_requests(), vec![Request::Keep]);
    k.process_requests(vec![Request::Keep], t0 + 1000);
    assert_eq!(k.take_kept().len(), 1);
    hub().set_status(k.status(t0 + 1000));
    assert!(!a.displays_buttons().iter().any(|b| b.0 == "Keep"));
    // another try, then go back
    press(&mut a, &cx, 'm');
    press(&mut a, &cx, 'y');
    let reqs = hub().take_requests();
    k.process_requests(reqs, t0 + 2000);
    hub().set_status(k.status(t0 + 2000));
    assert!(k.status(t0 + 2000).notes.iter().any(|n| n.contains("Mirror needs")), "these two displays share no resolution, so mirror falls back to extend and says so");
    press(&mut a, &cx, 'v');
    assert_eq!(hub().take_requests(), vec![Request::Revert]);
    k.process_requests(vec![Request::Revert], t0 + 3000);
    assert_eq!(k.specs().len(), 2);
    // with nothing changed Apply says so
    hub().set_status(k.status(t0 + 3000));
    draw(&mut a, &cx, 1280, 720);
    press(&mut a, &cx, 'y');
    assert!(status(&a).contains("Nothing has changed"), "{}", status(&a));
}

#[test]
fn the_main_display_choice_is_sent_and_the_screen_features_switch_cycles_and_is_stored() {
    let cx = admin("sd4");
    let k = card();
    publish(&k);
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx);
    draw(&mut a, &cx, 1280, 720);
    a.handle(&Event::Key(Key::Down), &cx);
    press(&mut a, &cx, 'p');
    press(&mut a, &cx, 'y');
    let Request::Apply(cfg) = hub().take_requests().remove(0) else { panic!() };
    assert_eq!(cfg.primary.as_deref(), Some("HDMI-A-1"));
    let dir = cx.services.dir().to_path_buf();
    assert_eq!(crate::memgate::read_switch(&dir), crate::memgate::Switch::Auto);
    press(&mut a, &cx, 's');
    assert_eq!(crate::memgate::read_switch(&dir), crate::memgate::Switch::On);
    assert!(status(&a).contains("Always on"));
    press(&mut a, &cx, 's');
    assert_eq!(crate::memgate::read_switch(&dir), crate::memgate::Switch::Off);
    press(&mut a, &cx, 's');
    assert_eq!(crate::memgate::read_switch(&dir), crate::memgate::Switch::Auto);
}

#[test]
fn the_page_draws_with_the_memory_line_the_rows_and_the_arrangement_at_four_sizes() {
    let cx = admin("sd5");
    let k = card();
    publish(&k);
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx);
    for (w, h) in [(1280u32, 720u32), (800, 480), (1920, 1080), (640, 400)] {
        let g = draw(&mut a, &cx, w, h);
        assert!(g.hits.iter().any(|h| h.label == "display eDP-1") && g.hits.iter().any(|h| h.label == "display HDMI-A-1"), "{w}x{h}");
        for hit in g.hits.iter().filter(|h| h.label.starts_with("display ")) {
            assert!(hit.rect.x >= 0.0 && hit.rect.x + hit.rect.w <= w as f32 + 0.5, "{w}x{h}: {:?}", hit.rect);
        }
    }
    let (line, sw) = a.disp_gate(&cx);
    assert!(line.starts_with("Layouts:") || line.contains("Screen features"), "{line}");
    assert_eq!(sw, crate::memgate::Switch::Auto);
    assert_eq!(a.hint().is_empty(), false);
}
