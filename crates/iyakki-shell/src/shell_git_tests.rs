//! The GIT tile on the home screen: in the Work group, for administrators, operators and users (not guests), remote-safe in Cloud mode
//! (it only asks hosts and reads folders), light on memory, with its own icon, and opening the app. The shell's tests never reach a
//! host or run git: the tile gets the offline rig there.

use super::tests::*;
use super::*;
use iyakki_agent::cloudmode::{self, Class, Facts, Setting};
use iyakki_sec::policy::Role;

#[test]
fn the_git_tile_is_registered_once_in_the_work_group_for_everyone_but_guests() {
    assert_eq!(TILES.iter().filter(|t| t.app == PApp::Git).count(), 1);
    let t = TILES.iter().find(|t| t.app == PApp::Git).unwrap();
    assert_eq!((t.label, t.sub, t.icon), ("GIT", "Browse repositories", Icon::Git));
    assert_eq!(Group::of(PApp::Git), Some(Group::Work));
    assert!(Group::Work.holds(PApp::Git));
    assert_eq!(PApp::Git.id(), "git");
    assert_eq!(PApp::Git.label(), "GIT");
    for r in [Role::Admin, Role::Operator, Role::User] {
        assert!(r.template().apps.contains(&PApp::Git), "{r:?}");
    }
    assert!(!Role::Guest.template().apps.contains(&PApp::Git), "a guest has no Git connections to browse");
    // like K8S and LAB: remote-only tiles stay in Cloud mode, and the tile is light
    assert_eq!(cloudmode::app_class(PApp::Git), Class::Remote);
    assert_eq!(crate::memgate::class_of(PApp::Git), crate::memgate::AppClass::Light);
    assert!(!cloudmode::cloud_mode(Setting::On, &Facts::full()).hides_app(PApp::Git));
    assert!(!cloudmode::cloud_mode(Setting::Auto, &Facts::bare()).hides_app(PApp::Git), "no sound card, no camera: the GIT tile stays");
    assert!(PApp::ALL.contains(&PApp::Git));
}

#[test]
fn the_tile_shows_for_each_role_and_opens_the_app() {
    for (user, pass, shown) in [("root", "rootpass12", true), ("oscar", "operpass12", true), ("alice", "alicepass1", true), ("gus", "guestpass1", false)] {
        let (mut sh, _) = shell_with_users(&format!("git-tile-{user}"));
        login(&mut sh, user, pass);
        if shown {
            assert!(sh.visible_tiles().iter().any(|t| t.label == "GIT"), "{user}");
            sh.set_group(Group::Work);
            assert!(sh.group_tiles().iter().any(|t| t.app == PApp::Git), "{user}: in the Work group");
            let i = sh.group_items().iter().position(|it| matches!(it, LItem::Sys(t) if t.app == PApp::Git)).unwrap();
            sh.sel = i;
            sh.handle(&Event::Key(Key::Enter));
            assert_eq!(sh.screen_name(), "app:git", "{user}");
            let mut g = Gfx::new(1280, 720);
            sh.render(&mut g);
            sh.handle(&Event::Key(Key::Esc));
            assert_eq!(sh.screen_name(), "launcher", "Esc leaves the tile");
        } else {
            assert!(!sh.session.as_ref().unwrap().policy.allows_app(PApp::Git), "{user}");
        }
    }
}

#[test]
fn the_tile_draws_inside_the_shell_at_the_supported_sizes_and_has_an_icon() {
    let (mut sh, _) = shell_with_users("git-tile-sizes");
    login(&mut sh, "root", "rootpass12");
    sh.set_group(Group::Work);
    let i = sh.group_items().iter().position(|it| matches!(it, LItem::Sys(t) if t.app == PApp::Git)).unwrap();
    sh.sel = i;
    sh.handle(&Event::Key(Key::Enter));
    for (w, h) in [(800u32, 480u32), (1280, 720), (1920, 1080), (360, 740)] {
        let mut g = Gfx::new(w, h);
        sh.render(&mut g);
        assert!(g.rgba().iter().any(|b| *b != 0), "{w}x{h}");
    }
    // the icon draws something of its own
    let mut a = Gfx::new(64, 64);
    a.icon(Icon::Git, 32.0, 32.0, 40.0, [255, 255, 255, 255]);
    let mut b = Gfx::new(64, 64);
    b.icon(Icon::Lab, 32.0, 32.0, 40.0, [255, 255, 255, 255]);
    assert!(a.rgba().iter().any(|v| *v != 0) && a.rgba() != b.rgba());
}
