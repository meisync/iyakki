//! The LEARN tile on the home screen: registered once, in Ask & Create, for everybody but guests.

use super::*;
use iyakki_agent::cloudmode::{self, Class, Facts, Setting};
use iyakki_sec::policy::Role;

#[test]
fn the_learn_tile_is_registered_once_in_the_home_group_for_everyone_but_guests() {
    assert_eq!(TILES.iter().filter(|t| t.app == PApp::Learn).count(), 1);
    let t = TILES.iter().find(|t| t.app == PApp::Learn).unwrap();
    assert_eq!((t.label, t.icon), ("LEARN", Icon::Learn));
    assert_eq!(Group::of(PApp::Learn), Some(Group::Home));
    assert_eq!((PApp::Learn.id(), PApp::Learn.label().as_str()), ("learn", "LEARN"));
    assert!(PApp::ALL.contains(&PApp::Learn));
    for r in [Role::Admin, Role::Operator, Role::User] {
        assert!(r.template().apps.contains(&PApp::Learn), "{r:?}");
    }
    assert!(!Role::Guest.template().apps.contains(&PApp::Learn));
    assert_eq!(cloudmode::app_class(PApp::Learn), Class::Remote);
    assert_eq!(crate::memgate::class_of(PApp::Learn), crate::memgate::AppClass::Light);
    assert!(!cloudmode::cloud_mode(Setting::On, &Facts::full()).hides_app(PApp::Learn));
}

#[test]
fn the_icon_draws_something() {
    let mut a = Gfx::new(64, 64);
    a.clear(crate::gfx::rgb(0, 0, 0));
    a.icon(Icon::Learn, 32.0, 32.0, 40.0, [255, 255, 255, 255]);
    assert!(a.rgba().chunks(4).any(|p| p[0] > 100), "the open book is drawn");
}
