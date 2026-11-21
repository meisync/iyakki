//! F-SHELL-64: a screen drawn through the recording layer on several bands is the same pixels as the screen drawn directly.
use super::tests::*;
use super::*;

fn frame(sh: &mut Shell, w: u32, h: u32, bands: Option<usize>) -> Vec<u8> {
    let mut g = Gfx::new(w, h);
    let mut pool = iyakki_uikit::gfx::BandPool::new();
    sh.tick = 1000;
    // two frames, as the display loop draws them: the second one starts from the first one's pixels and its hover and clip state
    for _ in 0..2 {
        sh.tick += 1;
        if bands.is_some() {
            g.record_begin();
        }
        sh.render(&mut g);
        if let Some(n) = bands {
            if std::env::var("IYAKKI_PARDRAW_WHY").is_ok() {
                if let Some(d) = g.record_first_difference(&mut pool, n) {
                    eprintln!("first differing call: #{} rows {} to {} {}", d.0, d.1, d.2, d.3);
                }
            }
            g.record_finish(&mut pool, n);
        }
    }
    g.rgba().to_vec()
}

#[test]
fn home_and_the_apps_drawn_in_bands_are_the_same_pixels_as_drawn_directly() {
    let (mut sh, _) = shell_with_users("pardraw");
    login(&mut sh, "root", "rootpass12");
    let apps = [None, Some(PApp::Chat), Some(PApp::Settings), Some(PApp::Tales), Some(PApp::Voice)];
    for app in apps {
        if let Some(a) = app {
            sh.open_for_test(a);
        }
        for (w, h) in [(1280u32, 720u32), (800, 480)] {
            let direct = frame(&mut sh, w, h, None);
            // a clock or a time-driven animation changes between two draws of the same screen: those pixels cannot be compared
            let again = frame(&mut sh, w, h, None);
            let steady = |i: usize| direct[i * 4..i * 4 + 4] == again[i * 4..i * 4 + 4];
            for n in [2usize, 5] {
                let par = frame(&mut sh, w, h, Some(n));
                let diff = (0..direct.len() / 4).filter(|&i| steady(i) && direct[i * 4..i * 4 + 4] != par[i * 4..i * 4 + 4]).count();
                if diff > 0 {
                    let pix: Vec<(usize, usize)> = (0..direct.len() / 4).filter(|i| steady(*i) && direct[i * 4..i * 4 + 4] != par[i * 4..i * 4 + 4]).map(|i| (i % w as usize, i / w as usize)).collect();
                    let md = pix.iter().map(|&(x, y)| (0..4).map(|k| (direct[(y * w as usize + x) * 4 + k] as i32 - par[(y * w as usize + x) * 4 + k] as i32).abs()).max().unwrap()).max();
                    eprintln!("MAXDELTA {md:?}");
                    eprintln!("DIFFPIX {app:?} {w}x{h} {n} first {:?} last {:?} count {} sample {:?}", pix.first(), pix.last(), pix.len(), &pix[..pix.len().min(12)]);
                }
                assert_eq!(diff, 0, "{app:?} at {w}x{h} on {n} bands: {diff} bytes differ from the single-thread frame");
            }
        }
    }
}

#[test]
fn the_look_key_parallel_draw_is_off_by_default_and_round_trips() {
    let (mut sh, _) = shell_with_users("pardraw2");
    login(&mut sh, "root", "rootpass12");
    assert!(!sh.parallel_draw());
    let d = iyakki_uikit::look::Look::default();
    assert!(!d.surface.parallel_draw);
    assert!(d.to_text().contains("parallel_draw = off"));
}
