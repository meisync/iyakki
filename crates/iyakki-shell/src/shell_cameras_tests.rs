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
