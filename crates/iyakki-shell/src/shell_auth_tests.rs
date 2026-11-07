//! The AUTH tile on the home screen: registered once, in Devices & System beside USERS, for everybody but guests, with its own accent,
//! a caption that fits the tile, and opened as the one-time codes page.

use super::*;
use iyakki_agent::cloudmode::{self, Class, Facts, Setting};
use iyakki_sec::policy::Role;

#[test]
fn the_auth_tile_is_registered_once_in_devices_and_system_for_everyone_but_guests() {
    assert_eq!(TILES.iter().filter(|t| t.app == PApp::Auth).count(), 1);
    let t = TILES.iter().find(|t| t.app == PApp::Auth).unwrap();
    assert_eq!((t.label, t.sub, t.icon), ("AUTH", "One-time codes", Icon::Lock));
    assert!(t.sub.chars().count() <= 19, "the caption fits the tile");
    assert_eq!(Group::of(PApp::Auth), Some(Group::System));
    assert!(Group::System.holds(PApp::Auth));
    assert_eq!((PApp::Auth.id(), PApp::Auth.label().as_str()), ("auth", "AUTH"));
    assert!(PApp::ALL.contains(&PApp::Auth));
    for r in [Role::Admin, Role::Operator, Role::User] {
        assert!(r.template().apps.contains(&PApp::Auth), "{r:?}");
    }
    assert!(!Role::Guest.template().apps.contains(&PApp::Auth), "a guest has the chat and the inbox only");
    assert_eq!(cloudmode::app_class(PApp::Auth), Class::Remote);
    assert_eq!(crate::memgate::class_of(PApp::Auth), crate::memgate::AppClass::Light);
    assert!(!cloudmode::cloud_mode(Setting::On, &Facts::full()).hides_app(PApp::Auth), "a cloud tenant keeps it like the other personal tiles");
    assert!(layout_tiles(true, &cloudmode::CloudMode::desktop()).iter().any(|t| t.1 == "AUTH" && t.2 == "system"));
}

#[test]
fn the_auth_accent_differs_from_every_other_tile_and_the_captions_are_unique() {
    let me = TILES.iter().find(|t| t.app == PApp::Auth).unwrap();
    for t in TILES.iter().filter(|t| t.app != PApp::Auth) {
        let d: i32 = (0..3).map(|i| (me.accent[i] as i32 - t.accent[i] as i32).abs()).sum();
        assert!(d >= 24, "AUTH and {} look alike ({d})", t.label);
        assert_ne!(t.sub, me.sub);
        assert_ne!(t.label, me.label);
    }
}
