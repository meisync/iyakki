//! What the STORE screen reads from the rest of the system (alpha.10 AP3, D-AP3-02 release info and private store, D-AP3-03): the System
//! update line, the private store's status, the sticks' stores, the update-check mode, the pending licence and the drivers' install side.
//! Plain answers; the real port is `AgentFacts` and `AgentDriversInstall` in `crates/iyakki-apps/src/apps/store_port.rs`.

use std::path::{Path, PathBuf};

use iyakki_store_model::store_private::StatusInfo;

/// The largest FaceTime driver file the store reads (the caller checks the size before it reads).
pub const MAX_DRIVER_FILE: u64 = 16 * 1024 * 1024;

/// The drivers' install side: the packs for a folder or a stick, and the camera firmware from Apple's driver file.
pub trait DriversInstall: Send + Sync {
    /// The pack text and the names of the files taken from `dir` for driver `id`.
    fn folder_package(&self, id: &str, dir: &Path, now: i64) -> Result<(String, Vec<String>), String>;
    /// The pack text and the notice for the FaceTime camera driver file.
    fn facetime_package(&self, driver_file: &[u8], accepted: bool, now: i64) -> Result<(String, String), String>;
    /// The title of a driver pack the catalogue knows.
    fn title_of(&self, id: &str) -> Option<String>;
}

pub trait StoreFacts: Send + Sync {
    /// The System update, when one is out: (the version it is, this one's version).
    fn system_update_out(&self) -> Option<(String, String)>;
    /// The private store of this person (its instance and its catalog source, if added).
    fn private_store(&self, user: &str) -> StatusInfo;
    /// The stores found on mounted sticks.
    fn stick_stores(&self) -> Vec<PathBuf>;
    /// The update check runs once a day (`None` when the settings cannot be read).
    fn update_check_daily(&self) -> Option<bool>;
    /// Set the update check to once a day (true) or to manual (false); true when it was saved.
    fn set_update_check_daily(&self, daily: bool) -> bool;
    /// The licence text a purchase left pending for this person, under the app data keys `app` and `key`.
    fn pending_licence(&self, user: &str, app: &str, key: &str) -> Option<String>;
    fn clear_pending_licence(&self, user: &str, app: &str, key: &str);
    /// What the update check needs from the system: its settings, the running release version and its variant.
    fn update_check_inputs(&self) -> (iyakki_store_model::store_updatecheck::CheckSettings, String, String);
    /// The title of a driver pack in the catalogue (`None` for an id the catalogue does not know).
    fn driver_title(&self, id: &str) -> Option<String>;
    /// The words for one permission of an app's manifest (`None` for a permission the words do not cover).
    fn permission_words(&self, permission: &str, user: &str) -> Option<String>;
}

/// Canned answers (tests): nothing is out, no sticks, the update check is manual, no pending licence.
#[derive(Debug, Default)]
pub struct FakeStoreFacts;

impl StoreFacts for FakeStoreFacts {
    fn system_update_out(&self) -> Option<(String, String)> {
        None
    }
    fn private_store(&self, _user: &str) -> StatusInfo {
        StatusInfo::default()
    }
    fn stick_stores(&self) -> Vec<PathBuf> {
        Vec::new()
    }
    fn update_check_daily(&self) -> Option<bool> {
        Some(false)
    }
    fn set_update_check_daily(&self, _daily: bool) -> bool {
        true
    }
    fn pending_licence(&self, _user: &str, _app: &str, _key: &str) -> Option<String> {
        None
    }
    fn clear_pending_licence(&self, _user: &str, _app: &str, _key: &str) {}
    fn permission_words(&self, _permission: &str, _user: &str) -> Option<String> {
        None
    }
    fn update_check_inputs(&self) -> (iyakki_store_model::store_updatecheck::CheckSettings, String, String) {
        (Default::default(), String::new(), String::new())
    }
    fn driver_title(&self, _id: &str) -> Option<String> {
        None
    }
}
