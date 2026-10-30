//! The DOCTOR tile on the home screen: registered once, in Devices & System, for everybody but guests.

use super::*;
use iyakki_agent::cloudmode::{self, Class, Facts, Setting};
use iyakki_sec::policy::Role;

#[test]
fn the_doctor_tile_is_registered_once_in_the_system_group_for_everyone_but_guests() {
    assert_eq!(TILES.iter().filter(|t| t.app == PApp::Doctor).count(), 1);
    let t = TILES.iter().find(|t| t.app == PApp::Doctor).unwrap();
    assert_eq!((t.label, t.icon), ("DOCTOR", Icon::Doctor));
    assert_eq!(Group::of(PApp::Doctor), Some(Group::System));
    assert!(Group::System.holds(PApp::Doctor));
    assert_eq!((PApp::Doctor.id(), PApp::Doctor.label().as_str()), ("doctor", "DOCTOR"));
    assert!(PApp::ALL.contains(&PApp::Doctor));
    for r in [Role::Admin, Role::Operator, Role::User] {
        assert!(r.template().apps.contains(&PApp::Doctor), "{r:?}");
    }
    assert!(!Role::Guest.template().apps.contains(&PApp::Doctor));
    assert_eq!(cloudmode::app_class(PApp::Doctor), Class::Remote);
    assert_eq!(crate::memgate::class_of(PApp::Doctor), crate::memgate::AppClass::Light);
    assert!(!cloudmode::cloud_mode(Setting::On, &Facts::full()).hides_app(PApp::Doctor));
    assert!(!cloudmode::cloud_mode(Setting::Auto, &Facts::bare()).hides_app(PApp::Doctor), "no sound card, no camera: DOCTOR stays (it explains that)");
}

#[test]
fn the_icon_draws_something() {
    let mut a = Gfx::new(64, 64);
    a.clear(crate::gfx::rgb(0, 0, 0));
    a.icon(Icon::Doctor, 32.0, 32.0, 40.0, [255, 255, 255, 255]);
    assert!(a.rgba().chunks(4).any(|p| p[0] > 100), "the case and the plus are drawn");
}
