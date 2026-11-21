//! The generated facade (see dist/split/gen_facade.py). Hand-written traits for stateful handles go next to `generated`.

pub mod facets;
pub mod facets_b0;
pub use facets_b0::*;
pub mod facets_b3;
pub mod facets_s6b;
pub use facets_s6b::*;
pub mod facets_dev;
pub mod facets_platform;
pub mod facets_tasks;
pub use facets_tasks::*;
pub use facets_platform::*;
pub use facets_dev::*;
pub mod member_stores;
pub use member_stores::*;
pub mod accounts_api;
pub use accounts_api::{AccountsGuarded, UsersAsApi, UsersGuard, UsersRef, UsersSrc};
pub mod settings_api;
pub use settings_api::{SettingsApi, SettingsGuard, SettingsRef, SettingsSnap, SettingsSource, SettingsSrc, SettingsVia};
mod kb_handle;
pub use kb_handle::KbHandle;

/// The red "camera in use" mark while it lives (the guard form of `CameraHubApi::busy_set`).
pub struct CameraBusy(std::sync::Arc<dyn CameraHubApi>);

impl CameraBusy {
    pub fn begin(hub: std::sync::Arc<dyn CameraHubApi>, user: &str, what: &str) -> CameraBusy {
        hub.busy_set(user, what);
        CameraBusy(hub)
    }
}

impl Drop for CameraBusy {
    fn drop(&mut self) {
        self.0.busy_clear();
    }
}
pub mod study_calendar;
pub use study_calendar::{StudyCalendarApi, StudyOverCalendar};
pub mod chatruns;
pub use chatruns::*;
pub mod facets_b4;
pub mod facets_b6;
pub use facets_b6::*;
pub mod facets_b7;
pub mod facets_b8;
pub mod facets_g;
pub mod facets_h;
pub use facets_h::*;
pub use facets_b7::*;
pub mod flows_jobs;
pub use flows_jobs::*;
pub use facets_b8::*;
pub use facets_g::*;
pub mod facets_b5;
pub use facets_b5::*;
pub mod store_api;
pub use store_api::*;
pub use facets_b4::*;
pub mod facets_b1;
pub mod facets_b2;
pub use facets_b2::*;
pub mod pending;
pub mod generated;
pub use facets::*;
pub use facets_b3::*;
pub use generated::*;
pub use facets_b1::*;

pub mod store;
pub use store::{ComponentReview, FakeStoreFacets, NoDrivers, NoStoreFacets, StoreCatalog, StoreComponents, StoreDriverSource, StoreDrivers, StoreFacets, StoreHub, StoreUpdates, WizardLine, MAX_DRIVER_FILE};
pub mod facets_stores;
pub use facets_stores::*;

#[cfg(test)]
mod camera_busy_tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn the_camera_in_use_mark_is_set_through_the_trait_object_and_cleared_when_the_guard_ends() {
        let hub: Arc<dyn CameraHubApi> = Arc::new(iyakki_shared::camtools::CameraHub::new());
        assert_eq!(hub.in_use(), None);
        {
            let _busy = CameraBusy::begin(hub.clone(), "amy", "taking a picture");
            assert_eq!(hub.in_use().as_deref(), Some("taking a picture"));
        }
        assert_eq!(hub.in_use(), None, "the mark ends with the guard, as the borrowing guard's did");
    }

    #[test]
    fn a_frame_already_drawn_serves_nobody_when_nobody_waits() {
        let screen: Arc<dyn ScreenCaptureApi> = Arc::new(iyakki_shared::screencap::ScreenCapture::new());
        assert!(!screen.wanted("amy"));
        assert_eq!(screen.serve_frame("amy", Some(iyakki_shared::screencap::Frame { w: 1, h: 1, rgba: vec![0, 0, 0, 255] })), 0);
    }
}
