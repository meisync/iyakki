//! The DEV TOOLS tile on the home screen: registered once, in Work, for everybody but guests, opened as the tools page.

use super::*;
use iyakki_agent::cloudmode::{self, Class, Facts, Setting};
use iyakki_sec::policy::Role;

#[test]
fn the_dev_tools_tile_is_registered_once_in_work_for_everyone_but_guests() {
    assert_eq!(TILES.iter().filter(|t| t.app == PApp::DevTools).count(), 1);
    let t = TILES.iter().find(|t| t.app == PApp::DevTools).unwrap();
    assert_eq!(t.label, "DEV TOOLS");
    assert_eq!(Group::of(PApp::DevTools), Some(Group::Work));
    assert!(t.sub.contains("QR") && t.sub.chars().count() <= 19, "the caption names the QR tool and stays short: {}", t.sub);
    assert_eq!((PApp::DevTools.id(), PApp::DevTools.label().as_str()), ("devtools", "DEV TOOLS"));
    assert!(PApp::ALL.contains(&PApp::DevTools));
    for r in [Role::Admin, Role::Operator, Role::User] {
        assert!(r.template().apps.contains(&PApp::DevTools), "{r:?}");
    }
    assert!(!Role::Guest.template().apps.contains(&PApp::DevTools));
    assert_eq!(cloudmode::app_class(PApp::DevTools), Class::Remote);
    assert_eq!(crate::memgate::class_of(PApp::DevTools), crate::memgate::AppClass::Light);
    assert!(!cloudmode::cloud_mode(Setting::On, &Facts::full()).hides_app(PApp::DevTools));
}
