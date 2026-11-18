//! The CAMERAS tile on the home screen: one tile, in the Web & Media group, for everybody but guests.

use super::*;
use iyakki_sec::policy::Role;

#[test]
fn the_cameras_tile_is_one_tile_in_web_and_media_and_not_for_guests() {
    assert_eq!(TILES.iter().filter(|t| t.app == PApp::Cameras).count(), 1);
    assert_eq!(Group::of(PApp::Cameras), Some(Group::WebMedia));
    assert_eq!(PApp::Cameras.id(), "cameras");
    for r in [Role::Admin, Role::Operator, Role::User] {
        assert!(r.template().apps.contains(&PApp::Cameras), "{r:?}");
    }
    assert!(!Role::Guest.template().apps.contains(&PApp::Cameras));
    // the camera tile of this computer's own device stays a separate tile
    assert_eq!(TILES.iter().filter(|t| t.app == PApp::Camera).count(), 1);
}

#[test]
fn the_pdf_viewer_is_one_launcher_tile_in_web_and_media_allowed_for_every_role_but_guests_and_works_in_cloud_mode() {
    let t: Vec<_> = TILES.iter().filter(|t| t.app == PApp::Pdf).collect();
    assert_eq!(t.len(), 1);
    assert_eq!((t[0].label, t[0].group), ("PDF", Some(Group::WebMedia)));
    assert_eq!((PApp::Pdf.id(), PApp::Pdf.label().as_str()), ("pdf", "PDF"));
    for r in [Role::Admin, Role::Operator, Role::User] {
        assert!(r.template().apps.contains(&PApp::Pdf), "{r:?}");
    }
    assert!(!Role::Guest.template().apps.contains(&PApp::Pdf));
    assert_eq!(iyakki_cloudmode::app_class(PApp::Pdf), iyakki_cloudmode::Class::Remote, "no local hardware: it works over a remote desktop");
}

// ------------------------------------------------------------------------------------------------ KI-093: every way off the tile

mod release {
    use super::super::tests::*;
    use super::super::*;
    use crate::apps::cameras::{hub_with, ShellUsb};
    use crate::camera::detect::tests::FakeMachine;
    use crate::camera::fake::FakeOpener;
    use crate::panes::LayoutKey;
    use std::time::{Duration, Instant};

    struct Rig {
        sh: Shell,
        g: Gfx,
        hub: Arc<iyakki_agent::cameras::Hub>,
        _m: FakeMachine,
    }

    /// A signed-in shell whose installation hub has one USB camera (a fake machine); the CAMERAS tile shows it.
    fn rig(tag: &str) -> Rig {
        let (mut sh, _) = shell_with_users(tag);
        login(&mut sh, "root", "rootpass12");
        sh.set_mem_for_test(Some(crate::memgate::MemFacts { total: 16 * crate::memgate::GIB, available: 12 * crate::memgate::GIB }));
        let m = FakeMachine::new(tag);
        m.plug_webcam(0, "HD Webcam");
        let usb = Arc::new(ShellUsb::with(m.presence(), Arc::new(FakeOpener::default())));
        let cx = sh.cx().expect("signed in");
        let hub = hub_with(&cx, usb).downcast::<iyakki_agent::cameras::Hub>().expect("the agent's hub");
        Rig { sh, g: Gfx::new(1280, 720), hub, _m: m }
    }

    impl Rig {
        /// Open the tile and draw until its camera runs.
        fn show(&mut self) {
            self.sh.screen = Screen::Launcher;
            self.sh.stash = None;
            assert!(self.sh.open_for_demo("cameras"));
            let t = Instant::now();
            while self.hub.running() == 0 {
                self.sh.handle(&Event::Tick);
                self.sh.render(&mut self.g);
                assert!(t.elapsed() < Duration::from_secs(5), "the tile did not start its camera");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        fn closed(&self, why: &str) {
            assert_eq!(self.hub.running(), 0, "{why}: the computer's own camera is still open");
        }
    }

    #[test]
    fn esc_q_home_f11_and_the_top_bar_buttons_close_the_computers_own_camera_in_the_same_event() {
        let mut r = rig("rel-keys");
        for (why, ev) in [("Esc", Event::Key(Key::Esc)), ("q", Event::Key(Key::Char('q'))), ("F11", Event::Key(Key::F(11)))] {
            r.show();
            r.sh.handle(&ev);
            r.closed(why);
        }
        // the Home icon and the Back button of the top bar are clicks on drawn buttons
        for l in ["home", "back"] {
            r.show();
            r.sh.render(&mut r.g);
            let rect = if l == "back" { back_rect(1280, scale_for(1280, 720)) } else { r.sh.hits().iter().rev().find(|h| h.label == l).map(|h| h.rect).expect("no home button") };
            let (x, y) = ((rect.x + rect.w / 2.0) as i32, (rect.y + rect.h / 2.0) as i32);
            r.sh.handle(&Event::MouseMove(x, y));
            r.sh.handle(&Event::MouseDown(x, y));
            r.sh.handle(&Event::MouseUp(x, y));
            r.closed(l);
        }
    }

    #[test]
    fn another_tile_the_inbox_a_view_switch_and_a_new_layout_without_the_tile_close_it() {
        let mut r = rig("rel-other");
        // an app kept aside is not on the screen
        r.show();
        r.sh.handle(&Event::Key(Key::F(9)));
        assert!(r.sh.stash.is_some());
        r.sh.handle(&Event::Tick);
        r.closed("the inbox opened over the tile");
        // another desktop view
        r.show();
        r.sh.views.model.add("Docs", None).unwrap();
        r.sh.handle(&Event::Layout(LayoutKey::View(2)));
        r.sh.handle(&Event::Tick);
        r.closed("a view switch");
        r.sh.handle(&Event::Layout(LayoutKey::View(1)));
        // a pane layout: the tile in the second pane stays open while shown, closes when its pane is closed
        r.sh.screen = Screen::Launcher;
        assert!(r.sh.open_for_demo("chat"));
        r.sh.handle(&Event::Layout(LayoutKey::Apply(2)));
        assert_eq!(r.sh.panes_layout_for_test(), "cols-50-50");
        let cx = r.sh.cx().unwrap();
        r.sh.panes_put_for_test(1, Box::new(crate::apps::cameras::CamerasApp::new(&cx)), PApp::Cameras);
        let t = Instant::now();
        while r.hub.running() == 0 {
            r.sh.handle(&Event::Tick);
            r.sh.render(&mut r.g);
            assert!(t.elapsed() < Duration::from_secs(5), "the tile in a pane did not start its camera");
            std::thread::sleep(Duration::from_millis(10));
        }
        r.sh.handle(&Event::Tick);
        assert!(r.sh.panes_hold(PApp::Cameras) && r.hub.running() == 1, "a tile in a pane is on the screen");
        // the person works in the first pane and chooses the single layout: the second pane, and the tile with it, is gone
        r.sh.ws_focus(0);
        r.sh.handle(&Event::Layout(LayoutKey::Apply(1)));
        r.sh.handle(&Event::Tick);
        r.closed("a layout without the tile");
    }

    #[test]
    fn the_lock_and_the_sign_out_close_it_and_the_tile_opens_it_again_after_unlock() {
        let mut r = rig("rel-lock");
        r.show();
        assert!(r.sh.lock_screen("test"));
        r.sh.handle(&Event::Tick);
        r.closed("the lock");
        // after the password the tile is still the shown app and looks again
        typ(&mut r.sh, "rootpass12");
        key(&mut r.sh, Key::Enter);
        assert!(!r.sh.is_locked());
        let t = Instant::now();
        while r.hub.running() == 0 {
            r.sh.handle(&Event::Tick);
            r.sh.render(&mut r.g);
            assert!(t.elapsed() < Duration::from_secs(5), "the tile did not look again after the unlock");
            std::thread::sleep(Duration::from_millis(10));
        }
        r.sh.sign_out();
        r.sh.handle(&Event::Tick);
        r.closed("the sign-out");
    }
}
