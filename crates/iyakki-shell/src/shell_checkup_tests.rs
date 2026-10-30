//! The CHECK-UP tile on the home screen: registered once, in Devices & System, for everybody but guests.

use super::*;
use iyakki_agent::cloudmode::{self, Class, Facts, Setting};
use iyakki_sec::policy::Role;

#[test]
fn the_checkup_tile_is_registered_once_in_the_system_group_for_everyone_but_guests() {
    assert_eq!(TILES.iter().filter(|t| t.app == PApp::Checkup).count(), 1);
    let t = TILES.iter().find(|t| t.app == PApp::Checkup).unwrap();
    assert_eq!((t.label, t.icon), ("CHECK-UP", Icon::Checkup));
    assert_eq!(Group::of(PApp::Checkup), Some(Group::System));
    assert!(Group::System.holds(PApp::Checkup));
    assert_eq!((PApp::Checkup.id(), PApp::Checkup.label().as_str()), ("checkup", "CHECK-UP"));
    assert!(PApp::ALL.contains(&PApp::Checkup));
    for r in [Role::Admin, Role::Operator, Role::User] {
        assert!(r.template().apps.contains(&PApp::Checkup), "{r:?}");
    }
    assert!(!Role::Guest.template().apps.contains(&PApp::Checkup));
    assert_eq!(cloudmode::app_class(PApp::Checkup), Class::Remote);
    assert_eq!(crate::memgate::class_of(PApp::Checkup), crate::memgate::AppClass::Medium);
    assert!(!cloudmode::cloud_mode(Setting::On, &Facts::full()).hides_app(PApp::Checkup));
    assert!(!cloudmode::cloud_mode(Setting::Auto, &Facts::bare()).hides_app(PApp::Checkup), "no sound card, no camera: CHECK-UP stays (it explains that)");
}

#[test]
fn the_icon_draws_something() {
    let mut a = Gfx::new(64, 64);
    a.clear(crate::gfx::rgb(0, 0, 0));
    a.icon(Icon::Checkup, 32.0, 32.0, 40.0, [255, 255, 255, 255]);
    assert!(a.rgba().chunks(4).any(|p| p[0] > 100), "the clipboard and the tick are drawn");
}
