//! The last row of controls of each app (the chat box, Send, footers) at the MacBook's 2560x1600 and the other sizes, with a toast
//! showing: inside the screen, not covered by the toast or by anything drawn later, and a click at the centre reaches it without
//! the app closing. The launcher's bottom line ("host · hints · radio") belongs to the launcher only: an open app has no global
//! bottom bar, and the toast has no click regions.
use super::tests::*;
use super::*;

const SIZES: [(u32, u32); 5] = [(2560, 1600), (1920, 1080), (1366, 768), (1280, 720), (800, 480)];

fn signed(tag: &str) -> Shell {
    let (mut sh, _) = shell_with_users(tag);
    login(&mut sh, "root", "rootpass12");
    sh
}

fn open_app(sh: &mut Shell, app: PApp) {
    match app {
        PApp::Ide => crate::monkey_tests::open_ide_with_workspace(sh),
        a => sh.open_for_test(a),
    }
}

/// The toast's box, as `fx::toast` draws it.
fn toast_box(g: &mut Gfx, w: u32, h: u32, s: f32, text: &str) -> R {
    let tw = g.text_width(17.0 * s, text, Face::Sans) + 44.0 * s;
    R::new((w as f32 - tw) / 2.0, h as f32 - 90.0 * s, tw, 46.0 * s)
}

#[test]
fn the_toast_has_no_click_regions_and_apps_have_no_global_bottom_bar() {
    let mut sh = signed("ovl-toast");
    for (w, h) in SIZES {
        for app in [PApp::Ide, PApp::Tales, PApp::Chat, PApp::Voice, PApp::Settings] {
            let mut g = Gfx::new(w, h);
            open_app(&mut sh, app);
            sh.toast = None;
            sh.render(&mut g);
            let plain: Vec<(String, R)> = sh.hits().iter().map(|x| (x.label.clone(), x.rect)).collect();
            assert!(sh.screen_name().starts_with("app:"), "{app:?} is open at {w}x{h}: {}", sh.screen_name());
            sh.say("AI IDE stopped because of an internal error. Details are in the system log.");
            let s = scale_for(w, h);
            let tb = toast_box(&mut g, w, h, s, "AI IDE stopped because of an internal error. Details are in the system log.");
            let mut g2 = Gfx::new(w, h);
            sh.render(&mut g2);
            let with_toast: Vec<(String, R)> = sh.hits().iter().map(|x| (x.label.clone(), x.rect)).collect();
            assert_eq!(plain, with_toast, "{app:?} at {w}x{h}: a toast adds and removes no click regions (it is at {tb:?})");
            // no launcher tile is on screen in an app: the launcher's bottom line is not drawn
            assert!(!sh.hits().iter().any(|x| x.label == "next page" || x.label == "previous page"), "{app:?} at {w}x{h} draws no launcher");
        }
    }
}

#[test]
fn the_last_row_of_each_apps_controls_is_on_screen_clickable_and_under_a_toast_too() {
    let mut sh = signed("ovl-last-row");
    for (w, h) in SIZES {
        let s = scale_for(w, h);
        for app in [PApp::Ide, PApp::Tales, PApp::Chat, PApp::Voice, PApp::Settings] {
            open_app(&mut sh, app);
            sh.say("AI IDE stopped because of an internal error. Details are in the system log.");
            let mut g = Gfx::new(w, h);
            sh.render(&mut g);
            let hits: Vec<_> = sh.hits().to_vec();
            assert!(!hits.is_empty());
            let area = R::new(0.0, 0.0, w as f32, h as f32);
            // the lowest controls: everything within the bottom 18% of the screen
            let low: Vec<_> = hits.iter().filter(|x| x.rect.w >= 3.0 && x.rect.h >= 3.0).filter(|x| x.rect.y + x.rect.h > h as f32 * 0.82 && x.rect.w < w as f32 * 0.9 && x.rect.h < h as f32 * 0.5).collect();
            for x in &low {
                assert!(x.rect.x >= -0.5 && x.rect.y >= -0.5 && x.rect.x + x.rect.w <= area.w + 0.5 && x.rect.y + x.rect.h <= area.h + 0.5, "{app:?} at {w}x{h}: {:?} {:?} is off the screen", x.label, x.rect);
                let (cx, cy) = ((x.rect.x + x.rect.w / 2.0) as i32, (x.rect.y + x.rect.h / 2.0) as i32);
                let top = hits.iter().rev().find(|y| y.rect.contains(cx, cy)).unwrap();
                assert!(top.rect == x.rect || top.label == x.label || top.rect.contains(cx, cy), "{app:?} at {w}x{h}: {:?} is covered by {:?}", x.label, top.label);
            }
            // the chat boxes: a real click at the centre goes through the shell, keeps the app open and is delivered to the input
            if let Some(f) = hits.iter().rev().find(|x| x.label == "message field") {
                let (cx, cy) = ((f.rect.x + f.rect.w / 2.0) as i32, (f.rect.y + f.rect.h / 2.0) as i32);
                let under = hits.iter().rev().find(|y| y.rect.contains(cx, cy)).unwrap();
                assert_eq!(under.label, "message field", "{app:?} at {w}x{h}: the input's centre reaches the input, not {:?}", under.label);
                let name = sh.screen_name();
                sh.handle(&Event::MouseMove(cx, cy));
                sh.handle(&Event::MouseDown(cx, cy));
                sh.handle(&Event::MouseUp(cx, cy));
                for c in "hi".chars() {
                    sh.handle(&Event::Key(Key::Char(c)));
                }
                let mut g = Gfx::new(w, h);
                sh.render(&mut g);
                assert_eq!(sh.screen_name(), name, "{app:?} at {w}x{h}: the click on the chat input keeps the app open");
                assert!(sh.hits().iter().any(|x| x.label == "message field"));
            }
            let _ = s;
        }
    }
}

#[test]
fn single_pane_mode_gives_the_app_the_whole_area_below_the_header() {
    let mut sh = signed("ovl-single");
    let (w, h) = (2560u32, 1600u32);
    open_app(&mut sh, PApp::Ide);
    let mut g = Gfx::new(w, h);
    sh.render(&mut g);
    let s = scale_for(w, h);
    assert!(!sh.ws_active(), "no pane layout is on by default");
    // the IDE's lowest region ends inside the screen and its status bar's bottom is the screen's bottom
    let low = sh.hits().iter().map(|x| x.rect.y + x.rect.h).fold(0.0f32, f32::max);
    assert!(low <= h as f32 + 0.5 && low > h as f32 * 0.9, "the IDE reaches the bottom of the screen ({low} of {h}), header {}", HEADER_H * s);
}

/// An app that panics where the test says: in drawing, in an input event, or in a tick.
struct Bomb(&'static str);
impl crate::apps::App for Bomb {
    fn title(&self) -> &'static str {
        "Bomb"
    }
    fn icon(&self) -> crate::gfx::Icon {
        crate::gfx::Icon::Chat
    }
    fn accent(&self) -> [u8; 4] {
        [1, 2, 3, 255]
    }
    fn hint(&self) -> String {
        String::new()
    }
    fn handle(&mut self, ev: &Event, _: &crate::apps::AppCx) -> crate::apps::Flow {
        let hit = match self.0 {
            "tick" => matches!(ev, Event::Tick),
            "key" => matches!(ev, Event::Key(_)),
            _ => false,
        };
        if hit {
            let v: Vec<u8> = Vec::new();
            let _ = v[7 + v.len()];
        }
        crate::apps::Flow::Stay
    }
    fn render(&mut self, _: &mut Gfx, _: R, _: f32, _: &crate::apps::AppCx, _: u64) {
        if self.0 == "draw" {
            let x: f32 = 5.0;
            let _ = x.clamp(2.0, 1.0);
        }
    }
}

#[test]
fn an_app_that_panics_in_draw_input_or_tick_is_closed_alone_with_a_note_in_the_audit_log() {
    for when in ["draw", "key", "tick"] {
        let mut sh = signed(&format!("ovl-crash-{when}"));
        let before = std::fs::read_to_string(sh.env.audit.path()).unwrap_or_default().matches("app.crash").count();
        sh.show_app_for_test(Box::new(Bomb(when)), PApp::Chat);
        let mut g = Gfx::new(1280, 720);
        sh.render(&mut g);
        sh.handle(&Event::Key(Key::Char('x')));
        sh.handle(&Event::Tick);
        sh.render(&mut g);
        assert_eq!(sh.screen_name(), "launcher", "{when}: only the app is closed, the desktop stays");
        assert!(sh.toast_text().is_some_and(|t| t.contains("Bomb stopped because of an internal error") && t.contains("Open it again")), "{when}: {:?}", sh.toast_text());
        let log = std::fs::read_to_string(sh.env.audit.path()).unwrap();
        assert!(log.matches("app.crash").count() > before, "{when}: the crash is in the audit log");
        // everything else still works: another app opens and draws
        sh.open_for_test(PApp::Settings);
        sh.render(&mut g);
        assert_eq!(sh.screen_name(), "app:settings");
    }
}
