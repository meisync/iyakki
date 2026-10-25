use super::*;
use crate::display::kms::FakeDrm;

fn mac() -> (Resources, Vec<Connector>) {
    // a MacBookPro14,1: the panel and an external monitor on HDMI (here a 4K one and a 1080p one on DP)
    let edp = FakeDrm::conn(10, "eDP-1", true, &[(2560, 1600, 60), (1920, 1200, 60), (1280, 800, 60)], 0b0011);
    let hdmi = FakeDrm::conn(11, "HDMI-A-1", true, &[(1920, 1080, 60), (1920, 1080, 50), (1280, 720, 60), (2560, 1600, 30)], 0b0110);
    let mut d = FakeDrm::new(3, vec![edp, hdmi]);
    use crate::display::kms::DrmBackend;
    let r = d.resources().unwrap();
    let cs = r.connectors.iter().map(|c| d.connector(*c).unwrap()).collect();
    (r, cs)
}

#[test]
fn the_built_in_panel_is_primary_and_the_monitor_is_extended_to_its_right_at_its_preferred_mode() {
    let (r, cs) = mac();
    let p = plan(&r, &cs, &DisplayConfig::default(), 4).unwrap();
    assert_eq!(p.outputs.len(), 2);
    assert_eq!(p.outputs[0].name, "eDP-1");
    assert_eq!((p.outputs[0].x, p.outputs[0].y, p.outputs[0].w(), p.outputs[0].h()), (0, 0, 2560, 1600));
    assert_eq!(p.outputs[1].name, "HDMI-A-1");
    assert_eq!((p.outputs[1].x, p.outputs[1].y, p.outputs[1].w(), p.outputs[1].h()), (2560, 0, 1920, 1080));
    assert!(!p.mirror && p.notes.is_empty());
    assert_ne!(p.outputs[0].crtc, p.outputs[1].crtc);
    assert_eq!(p.bounds(), (4480, 1600));
}

#[test]
fn the_person_picks_the_primary_a_resolution_and_a_side() {
    let (r, cs) = mac();
    let mut cfg = DisplayConfig::default();
    cfg.primary = Some("HDMI-A-1".into());
    cfg.set_mode("eDP-1", (1920, 1200));
    cfg.set_placement("eDP-1", Placement::LeftOf);
    let p = plan(&r, &cs, &cfg, 4).unwrap();
    assert_eq!(p.outputs[0].name, "HDMI-A-1");
    assert_eq!((p.outputs[0].x, p.outputs[0].y), (1920, 0), "the arrangement is shifted so it starts at 0,0");
    assert_eq!((p.outputs[1].x, p.outputs[1].w()), (0, 1920));
    for pl in [Placement::Above, Placement::Below] {
        cfg.set_placement("eDP-1", pl);
        let p = plan(&r, &cs, &cfg, 4).unwrap();
        let (a, b) = (&p.outputs[0], &p.outputs[1]);
        assert_eq!(a.x, b.x, "{pl:?}");
        assert!(if pl == Placement::Above { b.y + b.h() <= a.y } else { a.y + a.h() <= b.y }, "{pl:?}");
        assert_eq!(p.outputs.iter().map(|o| o.y).min(), Some(0));
    }
    // the highest refresh rate of the chosen size wins
    let mut cfg = DisplayConfig::default();
    cfg.set_mode("HDMI-A-1", (1920, 1080));
    assert_eq!(plan(&r, &cs, &cfg, 4).unwrap().outputs[1].mode.vrefresh, 60);
    // a size the monitor does not have falls back to its preferred mode
    cfg.set_mode("HDMI-A-1", (1234, 567));
    assert_eq!(plan(&r, &cs, &cfg, 4).unwrap().outputs[1].mode.size(), (1920, 1080));
}

#[test]
fn mirror_uses_the_biggest_size_both_have_and_falls_back_to_extend_when_there_is_none() {
    let (r, cs) = mac();
    let cfg = DisplayConfig { mirror: true, ..Default::default() };
    let p = plan(&r, &cs, &cfg, 4).unwrap();
    assert!(p.mirror);
    assert_eq!(p.outputs[0].mode.size(), (2560, 1600), "both have 2560x1600");
    assert_eq!(p.outputs[1].mode.size(), (2560, 1600));
    assert!(p.outputs.iter().all(|o| (o.x, o.y) == (0, 0)));
    // no common size
    let a = FakeDrm::conn(10, "eDP-1", true, &[(2560, 1600, 60)], 0b01);
    let b = FakeDrm::conn(11, "HDMI-A-1", true, &[(1920, 1080, 60)], 0b10);
    let res = Resources { crtcs: vec![41, 42], connectors: vec![10, 11], max_size: (8192, 8192) };
    let p = plan(&res, &[a, b], &cfg, 4).unwrap();
    assert!(!p.mirror);
    assert!(p.notes.iter().any(|n| n.contains("Mirror needs")), "{:?}", p.notes);
    assert_eq!(p.outputs.len(), 2);
}

#[test]
fn the_memory_gate_limits_the_outputs_and_the_rest_are_left_off_with_a_note() {
    let (r, cs) = mac();
    let p = plan(&r, &cs, &DisplayConfig::default(), 1).unwrap();
    assert_eq!(p.outputs.len(), 1);
    assert_eq!(p.outputs[0].name, "eDP-1");
    assert!(p.notes[0].contains("HDMI-A-1 is left off"), "{:?}", p.notes);
    assert!(plan(&r, &[], &DisplayConfig::default(), 4).is_err());
    let mut off = cs.clone();
    off.iter_mut().for_each(|c| c.connected = false);
    assert!(plan(&r, &off, &DisplayConfig::default(), 4).unwrap_err().contains("no display"));
}

#[test]
fn crtcs_are_shared_out_so_every_output_gets_one_it_can_use_and_keeps_its_own_when_possible() {
    // both connectors can only use CRTC index 0 and 1; the first wants index 1 which it is on now
    let mut a = FakeDrm::conn(10, "eDP-1", true, &[(1920, 1080, 60)], 0b011);
    a.current_crtc = 42;
    let b = FakeDrm::conn(11, "HDMI-A-1", true, &[(1920, 1080, 60)], 0b001);
    let res = Resources { crtcs: vec![41, 42, 43], connectors: vec![10, 11], max_size: (8192, 8192) };
    let p = plan(&res, &[a.clone(), b.clone()], &DisplayConfig::default(), 4).unwrap();
    assert_eq!(p.outputs[0].crtc, 42, "keeps its CRTC");
    assert_eq!(p.outputs[1].crtc, 41);
    // the second can only use the one the first would like: the matching moves the first away
    let b2 = FakeDrm::conn(11, "HDMI-A-1", true, &[(1920, 1080, 60)], 0b010);
    let p = plan(&res, &[a.clone(), b2], &DisplayConfig::default(), 4).unwrap();
    assert_eq!((p.outputs[0].crtc, p.outputs[1].crtc), (41, 42));
    // only one CRTC for two outputs: the second is left off with a note
    let res1 = Resources { crtcs: vec![41], connectors: vec![10, 11], max_size: (8192, 8192) };
    let p = plan(&res1, &[a, b], &DisplayConfig::default(), 4).unwrap();
    assert_eq!(p.outputs.len(), 1);
    assert!(p.notes.iter().any(|n| n.contains("no free CRTC")));
}

#[test]
fn the_pointer_moves_between_outputs_by_position_and_cannot_leave_the_arrangement() {
    let (r, cs) = mac();
    let p = plan(&r, &cs, &DisplayConfig::default(), 4).unwrap();
    assert_eq!(p.locate(100, 100), Some((0, 100, 100)));
    assert_eq!(p.locate(2560, 5), Some((1, 0, 5)));
    assert_eq!(p.locate(3000, 1500), None, "below the monitor, outside both");
    assert_eq!(p.clamp(3000, 1500), (3000, 1079));
    assert_eq!(p.clamp(-50, -50), (0, 0));
    assert_eq!(p.clamp(9999, 0), (4479, 0));
    assert_eq!(p.global(1, 10, 20), (2570, 20));
    // walk right across the edge in small steps: the display index changes once
    let mut seen = vec![];
    let mut x = 2540;
    while x < 2580 {
        let (cx, cy) = p.clamp(x, 800);
        if let Some((i, _, _)) = p.locate(cx, cy) {
            if seen.last() != Some(&i) {
                seen.push(i);
            }
        }
        x += 4;
    }
    assert_eq!(seen, vec![0, 1]);
}

#[test]
fn hot_plug_changes_are_found_from_two_looks() {
    let (_, cs) = mac();
    let one = vec![cs[0].clone()];
    assert_eq!(diff(&one, &cs), vec![Change::Added("HDMI-A-1".into())]);
    assert_eq!(diff(&cs, &one), vec![Change::Removed("HDMI-A-1".into())]);
    assert!(diff(&cs, &cs).is_empty());
    let mut other = cs.clone();
    other[1].modes.truncate(1);
    assert_eq!(diff(&cs, &other), vec![Change::ModesChanged("HDMI-A-1".into())]);
    let mut gone = cs.clone();
    gone[1].connected = false;
    assert_eq!(diff(&cs, &gone), vec![Change::Removed("HDMI-A-1".into())]);
}

#[test]
fn a_new_setting_is_tried_for_fifteen_seconds_and_then_reverts() {
    let prev = DisplayConfig { mirror: true, ..Default::default() };
    let t = Trial::start(prev.clone(), 1000);
    assert_eq!(t.secs_left(1000), 15);
    assert_eq!(t.secs_left(1001), 15);
    assert_eq!(t.secs_left(5000), 11);
    assert!(!t.expired(15_999));
    assert!(t.expired(16_000));
    assert_eq!(t.secs_left(99_999), 0);
    assert_eq!(t.previous, prev);
}

#[test]
fn the_arrangement_is_kept_per_person_and_a_bad_file_or_name_is_harmless() {
    let d = std::env::temp_dir().join(format!("iyakki-disp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    assert_eq!(load(&d, "alice"), DisplayConfig::default());
    let mut c = DisplayConfig { mirror: false, primary: Some("HDMI-A-1".into()), ..Default::default() };
    c.set_mode("eDP-1", (1920, 1200));
    c.set_placement("eDP-1", Placement::Above);
    c.set_template("HDMI-A-1", "cols-50-50");
    save(&d, "alice", &c).unwrap();
    assert_eq!(load(&d, "alice"), c);
    assert_eq!(load(&d, "bob"), DisplayConfig::default());
    assert!(save(&d, "../evil", &c).is_err());
    assert_eq!(load(&d, "../evil"), DisplayConfig::default());
    std::fs::write(d.join("displays-carol.json"), "{ not json").unwrap();
    assert_eq!(load(&d, "carol"), DisplayConfig::default());
    assert_eq!(c.template_of("HDMI-A-1"), Some("cols-50-50"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_hub_passes_requests_one_way_and_status_the_other() {
    let h = hub();
    assert!(h.take_requests().is_empty());
    h.request(Request::Identify);
    h.request(Request::Keep);
    assert_eq!(h.take_requests(), vec![Request::Identify, Request::Keep]);
    assert!(h.take_requests().is_empty());
    h.set_status(Status { kms: true, ..Default::default() });
    assert!(hub().status().kms, "the same hub on the same thread");
    h.identify(5000);
    assert!(h.identifying(4999) && !h.identifying(5000));
}
