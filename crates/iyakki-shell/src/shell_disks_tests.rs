//! The DISKS tile on the home screen: System group, which roles have it, hidden in Cloud mode like the other hardware apps, light on
//! memory, and opening the app from the launcher. The shell's tests give the tile a made-up machine; no real drive is ever seen.

use super::tests::*;
use super::*;
use iyakki_agent::cloudmode::{self, Class, Facts, Need, Setting};
use iyakki_sec::policy::Role;

#[test]
fn the_disks_tile_is_registered_once_in_the_system_group_and_follows_the_roles() {
    assert_eq!(TILES.iter().filter(|t| t.app == PApp::Disks).count(), 1);
    let t = TILES.iter().find(|t| t.app == PApp::Disks).unwrap();
    assert_eq!((t.label, t.sub), ("DISKS", "Drives, partitions"));
    assert_eq!(Group::of(PApp::Disks), Some(Group::System));
    assert_eq!(PApp::Disks.id(), "disks");
    assert_eq!(PApp::Disks.label(), "DISKS");
    for r in [Role::Admin, Role::Operator, Role::User] {
        assert!(r.template().apps.contains(&PApp::Disks), "{r:?}");
    }
    assert!(!Role::Guest.template().apps.contains(&PApp::Disks), "guests get no drives");
    assert_eq!(crate::memgate::class_of(PApp::Disks), crate::memgate::AppClass::Light);
}

#[test]
fn the_disks_tile_is_hidden_in_cloud_mode_because_it_needs_local_drives() {
    assert_eq!(cloudmode::app_class(PApp::Disks), Class::Hardware(Need::Disks));
    let forced = cloudmode::cloud_mode(Setting::On, &Facts::full());
    assert!(forced.hides_app(PApp::Disks), "a remote desktop has no drives to manage");
    let desktop = cloudmode::cloud_mode(Setting::Auto, &Facts::full());
    assert!(!desktop.hides_app(PApp::Disks));
    // a machine that is in cloud mode only because it has no sound still has its disks
    let nosound = cloudmode::cloud_mode(Setting::Auto, &Facts { audio: false, ..Facts::full() });
    assert!(!nosound.hides_app(PApp::Disks));
    let nodisks = cloudmode::cloud_mode(Setting::Auto, &Facts { audio: false, disks: false, ..Facts::full() });
    assert!(nodisks.hides_app(PApp::Disks));
}

#[test]
fn the_tile_shows_for_each_role_and_opens_the_app() {
    for (user, pass, shown) in [("root", "rootpass12", true), ("oscar", "operpass12", true), ("alice", "alicepass1", true), ("gus", "guestpass1", false)] {
        let (mut sh, _) = shell_with_users(&format!("disks-tile-{user}"));
        login(&mut sh, user, pass);
        if shown {
            assert!(sh.visible_tiles().iter().any(|t| t.label == "DISKS"), "{user}");
            sh.set_group(Group::System);
            let i = sh.group_items().iter().position(|it| matches!(it, LItem::Sys(t) if t.app == PApp::Disks)).unwrap();
            sh.sel = i;
            sh.handle(&Event::Key(Key::Enter));
            assert_eq!(sh.screen_name(), "app:disks", "{user}");
            for (w, h) in [(800u32, 480u32), (1280, 720), (1920, 1080), (360, 740), (2560, 1600), (2880, 1800)] {
                let mut g = Gfx::new(w, h);
                sh.render(&mut g);
                assert!(g.rgba().iter().any(|b| *b != 0), "{user} {w}x{h}");
            }
            sh.handle(&Event::Key(Key::Esc));
            assert_eq!(sh.screen_name(), "launcher", "Esc leaves the tile");
        } else {
            assert!(!sh.session.as_ref().unwrap().policy.allows_app(PApp::Disks), "{user}");
        }
    }
}
