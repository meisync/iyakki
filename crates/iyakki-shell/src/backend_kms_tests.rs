use super::*;
use crate::display::kms::FakeDrm;
use crate::display::outputs::Placement;
use crate::memgate::{features_with, DisplayMode, MemFacts, Switch, GIB};

fn card() -> FakeDrm {
    let edp = FakeDrm::conn(10, "eDP-1", true, &[(2560, 1600, 60), (1920, 1200, 60)], 0b0011);
    let hdmi = FakeDrm::conn(11, "HDMI-A-1", true, &[(1920, 1080, 60), (1280, 720, 60), (2560, 1600, 30)], 0b0110);
    FakeDrm::new(3, vec![edp, hdmi])
}

fn solid(w: usize, h: usize, rgba: [u8; 4]) -> Vec<u8> {
    rgba.iter().copied().cycle().take(w * h * 4).collect()
}

#[test]
fn opening_builds_two_buffers_per_output_sets_every_mode_and_tells_the_shell_the_displays() {
    let k = Kms::open(card(), DisplayConfig::default(), 4).unwrap();
    assert_eq!(k.plan().outputs.len(), 2);
    let specs = k.specs();
    assert_eq!((specs[0].name.as_str(), specs[0].w, specs[0].h, specs[0].x), ("eDP-1", 2560, 1600, 0));
    assert_eq!((specs[1].name.as_str(), specs[1].w, specs[1].h, specs[1].x), ("HDMI-A-1", 1920, 1080, 2560));
    let mut k = k;
    let d = k.drm();
    assert_eq!(d.shown.len(), 2, "both CRTCs show something");
    assert_eq!(d.log.iter().filter(|l| l.starts_with("create_buffer")).count(), 4);
    assert!(d.shown.values().any(|s| s.2 == (2560, 1600)) && d.shown.values().any(|s| s.2 == (1920, 1080)));
    assert_eq!(k.generation, 1);
}

#[test]
fn the_rgba_frame_becomes_xrgb_with_the_cards_pitch() {
    let src = [10u8, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 1, 2, 3, 255];
    let mut dst = vec![0xEEu8; 2 * 16];
    rgba_to_xrgb(&src, 2, 2, &mut dst, 16);
    assert_eq!(&dst[0..8], &[30, 20, 10, 255, 60, 50, 40, 255]);
    assert_eq!(&dst[8..16], &[0xEE; 8], "the padding of a row is left alone");
    assert_eq!(&dst[16..24], &[90, 80, 70, 255, 3, 2, 1, 255]);
}

#[test]
fn a_frame_is_drawn_on_the_back_buffer_and_flipped_and_a_card_that_refuses_flips_gets_setcrtc() {
    let mut k = Kms::open(card(), DisplayConfig::default(), 4).unwrap();
    let (w, h) = (1920usize, 1080usize);
    k.present(1, &solid(w, h, [200, 100, 50, 255]), w, h, None).unwrap();
    let flips = k.drm().flips;
    assert_eq!(flips, 1);
    // the pixel is B G R X in the buffer now on show
    let crtc = k.plan().outputs[1].crtc;
    let fb = k.drm().shown[&crtc].0;
    let buf = *k.drm().fb_of.get(&fb).unwrap();
    let px = k.drm().pixels(&buf).unwrap();
    assert_eq!(&px[0..4], &[50, 100, 200, 255]);
    // the next frame uses the other buffer
    k.present(1, &solid(w, h, [1, 2, 3, 255]), w, h, None).unwrap();
    let fb2 = k.drm().shown[&crtc].0;
    assert_ne!(fb, fb2, "two buffers take turns");
    // a card that cannot flip: SETCRTC every time, and the picture still arrives
    k.drm().pageflip_supported = false;
    k.present(1, &solid(w, h, [9, 9, 9, 255]), w, h, None).unwrap();
    let fb3 = k.drm().shown[&crtc].0;
    assert_ne!(fb2, fb3);
    assert!(k.drm().log.iter().filter(|l| l.starts_with("set_crtc")).count() >= 3);
    // a frame of the wrong size is refused, not drawn out of bounds
    assert!(k.present(1, &solid(100, 100, [0; 4]), 100, 100, None).is_err());
    assert!(k.present(5, &[], 0, 0, None).is_err());
}

#[test]
fn the_pointer_is_drawn_into_the_buffer_of_the_display_it_is_on() {
    let mut k = Kms::open(card(), DisplayConfig::default(), 4).unwrap();
    let (w, h) = (1920usize, 1080usize);
    k.present(1, &solid(w, h, [0, 0, 255, 255]), w, h, Some((100, 100, 1))).unwrap();
    let crtc = k.plan().outputs[1].crtc;
    let fb = k.drm().shown[&crtc].0;
    let buf = *k.drm().fb_of.get(&fb).unwrap();
    let px = k.drm().pixels(&buf).unwrap().to_vec();
    let at = |x: usize, y: usize| -> [u8; 4] { let i = y * buf.pitch as usize + x * 4; [px[i], px[i + 1], px[i + 2], px[i + 3]] };
    assert_eq!(at(100, 100), [0, 0, 0, 255], "the tip of the arrow is its black outline");
    assert_eq!(at(103, 106), [255, 255, 255, 255], "inside the arrow is white");
    assert_eq!(at(400, 400), [255, 0, 0, 255], "the scene (blue in RGBA, so R=0..: B G R order) elsewhere");
}

#[test]
fn mirror_shows_the_first_display_on_every_output() {
    let cfg = DisplayConfig { mirror: true, ..Default::default() };
    let mut k = Kms::open(card(), cfg, 4).unwrap();
    assert_eq!(k.specs().len(), 1, "the shell lays out one display");
    let (w, h) = (k.specs()[0].w as usize, k.specs()[0].h as usize);
    assert_eq!((w, h), (2560, 1600));
    k.present(0, &solid(w, h, [7, 8, 9, 255]), w, h, None).unwrap();
    assert_eq!(k.drm().flips, 2, "both outputs got the frame");
    for o in 0..2 {
        let crtc = k.plan().outputs[o].crtc;
        let fb = k.drm().shown[&crtc].0;
        let buf = *k.drm().fb_of.get(&fb).unwrap();
        assert_eq!(&k.drm().pixels(&buf).unwrap()[0..3], &[9, 8, 7]);
    }
}

#[test]
fn hot_plug_replans_and_keeps_the_old_arrangement_when_the_new_one_cannot_be_shown() {
    let mut k = Kms::open(card(), DisplayConfig::default(), 4).unwrap();
    assert!(k.rescan().unwrap().is_empty());
    let g = k.generation;
    // the monitor is unplugged
    k.drm().connectors[1].connected = false;
    let ch = k.rescan().unwrap();
    assert_eq!(ch, vec![Change::Removed("HDMI-A-1".into())]);
    assert_eq!(k.specs().len(), 1);
    assert!(k.generation > g);
    assert_eq!(k.drm().shown.len(), 1, "its CRTC is off");
    // and plugged in again: it joins on the right
    k.drm().connectors[1].connected = true;
    assert_eq!(k.rescan().unwrap(), vec![Change::Added("HDMI-A-1".into())]);
    assert_eq!(k.specs()[1].x, 2560);
    // a plug-in whose buffers cannot be made: the one display stays
    k.drm().connectors[1].connected = false;
    k.rescan().unwrap();
    k.drm().connectors[1].connected = true;
    k.drm().fail = Some("create_buffer");
    assert!(k.rescan().is_err());
    assert_eq!(k.specs().len(), 1, "the old arrangement is still shown");
    assert_eq!(k.drm().shown.len(), 1);
}

#[test]
fn a_new_setting_is_tried_and_reverts_after_fifteen_seconds_unless_kept() {
    let mut k = Kms::open(card(), DisplayConfig::default(), 4).unwrap();
    let mut cfg = DisplayConfig::default();
    cfg.set_mode("eDP-1", (1920, 1200));
    cfg.set_placement("HDMI-A-1", Placement::Below);
    k.apply(cfg.clone(), 1000).unwrap();
    assert_eq!(k.specs()[0].w, 1920);
    assert_eq!((k.specs()[1].x, k.specs()[1].y), (0, 1200));
    assert_eq!(k.trial_secs(1000), Some(15));
    assert!(!k.tick_trial(10_000).unwrap());
    assert_eq!(k.trial_secs(10_000), Some(6));
    assert!(k.tick_trial(16_000).unwrap(), "time is up: reverted");
    assert_eq!(k.specs()[0].w, 2560);
    assert_eq!(*k.config(), DisplayConfig::default());
    assert!(k.trial_secs(16_000).is_none());
    // kept: stays, and the file gets it once
    k.apply(cfg.clone(), 20_000).unwrap();
    k.keep();
    assert!(!k.tick_trial(99_999).unwrap());
    assert_eq!(k.specs()[0].w, 1920);
    assert_eq!(k.take_kept(), vec![cfg.clone()]);
    assert!(k.take_kept().is_empty());
    // revert by hand
    k.apply(DisplayConfig { mirror: true, ..Default::default() }, 30_000).unwrap();
    assert!(k.specs().len() == 1);
    assert!(k.revert().unwrap());
    assert_eq!(*k.config(), cfg);
    assert!(!k.revert().unwrap(), "nothing to revert");
}

#[test]
fn a_setting_the_card_refuses_is_undone_at_once() {
    let mut k = Kms::open(card(), DisplayConfig::default(), 4).unwrap();
    let mut cfg = DisplayConfig::default();
    cfg.set_mode("eDP-1", (1920, 1200));
    k.drm().fail = Some("set_crtc");
    assert!(k.apply(cfg, 1000).is_err());
    assert_eq!(k.specs()[0].w, 2560, "the old arrangement is back and shown");
    assert_eq!(k.drm().shown.len(), 2);
    assert!(k.trial_secs(1000).is_none());
}

#[test]
fn the_settings_page_requests_are_carried_out_and_the_status_lists_every_output_and_mode() {
    let mut k = Kms::open(card(), DisplayConfig::default(), 4).unwrap();
    let mut cfg = DisplayConfig::default();
    cfg.set_mode("HDMI-A-1", (1280, 720));
    let changed = k.process_requests(vec![Request::Apply(cfg.clone())], 5000);
    assert!(changed);
    let st = k.status(6000);
    assert!(st.kms && st.trial_secs == Some(14));
    let hdmi = st.outputs.iter().find(|o| o.name == "HDMI-A-1").unwrap();
    assert_eq!(hdmi.current, Some((1280, 720)));
    assert!(hdmi.modes.contains(&(1920, 1080, 60)) && hdmi.modes.contains(&(2560, 1600, 30)));
    assert!(st.outputs.iter().find(|o| o.name == "eDP-1").unwrap().primary);
    assert!(!k.process_requests(vec![Request::Keep], 7000));
    assert_eq!(k.take_kept(), vec![cfg]);
    hub().identify(0);
    k.process_requests(vec![Request::Identify], 8000);
    assert!(hub().identifying(9000) && !hub().identifying(11_001));
    // an arrangement that cannot be planned says so and changes nothing
    let mut off = DisplayConfig::default();
    off.primary = Some("nothing".into());
    k.process_requests(vec![Request::Apply(off)], 9000);
    assert_eq!(k.specs().len(), 2);
}

#[test]
fn the_memory_gate_limits_how_many_outputs_are_driven() {
    let mut k = Kms::open(card(), DisplayConfig::default(), 1).unwrap();
    assert_eq!(k.specs().len(), 1);
    assert!(k.status(0).notes.iter().any(|n| n.contains("left off")), "{:?}", k.status(0).notes);
    let _ = k.drm();
}

#[test]
fn the_shell_drives_displays_only_when_two_are_connected_the_memory_allows_it_and_nothing_is_in_the_way() {
    let modes = [DisplayMode { w: 2560, h: 1600 }, DisplayMode { w: 1920, h: 1080 }];
    let big = features_with(Switch::Auto, MemFacts { total: 8 * GIB, available: 6 * GIB }, &modes, &[]);
    let small = features_with(Switch::Auto, MemFacts { total: GIB, available: GIB / 2 }, &modes, &[]);
    let off = features_with(Switch::Off, MemFacts { total: 8 * GIB, available: 6 * GIB }, &modes, &[]);
    assert!(decide(2, false, &big).is_ok());
    assert!(decide(1, false, &big).unwrap_err().contains("one display"));
    assert!(decide(2, true, &big).unwrap_err().contains("window"));
    assert!(decide(2, false, &small).unwrap_err().contains("memory"), "{:?}", decide(2, false, &small));
    assert!(decide(2, false, &off).unwrap_err().contains("switched off"));
    // always on forces it
    let on = features_with(Switch::On, MemFacts { total: GIB, available: GIB / 2 }, &modes, &[]);
    assert!(decide(2, false, &on).is_ok());
}

#[test]
fn the_card_is_the_one_whose_sysfs_has_two_connected_connectors() {
    let d = std::env::temp_dir().join(format!("iyakki-kms-sys-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let mk = |name: &str, status: &str| {
        let p = d.join(name);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("status"), format!("{status}\n")).unwrap();
        std::fs::write(p.join("modes"), "1920x1080\n").unwrap();
    };
    mk("card0-eDP-1", "connected");
    mk("card0-HDMI-A-1", "disconnected");
    mk("card1-DP-1", "connected");
    assert_eq!(pick_card(&d), None, "one connected per card");
    mk("card1-DP-2", "connected");
    assert_eq!(pick_card(&d), Some("/dev/dri/card1".into()));
    mk("card0-HDMI-A-1", "connected");
    assert_eq!(pick_card(&d), Some("/dev/dri/card0".into()), "the first card");
    // the fallback status lists the connected outputs and why
    publish_fallback(&d, "tests");
    let st = hub().status();
    assert!(!st.kms && st.why_not == "tests" && st.outputs.len() == 4, "{st:?}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_shell_takes_the_real_loop_only_where_the_real_card_exists() {
    // in tests nothing opens /dev/dri: the loop is not entered and the framebuffer path would go on
    let (mut sh, _) = crate::shell::tests::shell_with_users("kms-no");
    let stop = AtomicBool::new(true);
    assert!(matches!(maybe_run(&mut sh, &stop), Outcome::NotUsed));
}
