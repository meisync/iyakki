//! The system settings as the interface that crosses the core socket (F-SPLIT-03, UI-RPC-DESIGN.md): a snapshot and a replace-if-unchanged. The shapes
//! the screens use (`SettingsRef`, `SettingsGuard`) are in `iyakki_shared::settings_ref`; this file is the served trait and its adapters.

use std::sync::{Arc, Mutex};

use iyakki_shared::settings::{Settings, SettingsError, SettingsStore};
pub use iyakki_shared::settings_ref::{SettingsGuard, SettingsRef, SettingsSnap, SettingsSource};

pub trait SettingsApi: Send + Sync {
    fn snapshot(&self) -> SettingsSnap;
    /// Replace the settings when `generation` is still the current count (`Ok(false)`: overtaken, nothing written); validates and saves like `update`.
    fn replace_if(&self, generation: u64, settings: Settings) -> Result<bool, SettingsError>;
}

impl SettingsApi for Arc<Mutex<SettingsStore>> {
    fn snapshot(&self) -> SettingsSnap {
        SettingsSource::snapshot(self)
    }
    fn replace_if(&self, generation: u64, settings: Settings) -> Result<bool, SettingsError> {
        SettingsSource::replace_if(self, generation, settings)
    }
}

/// A served `SettingsApi` as the source of a [`SettingsRef`].
pub struct SettingsSrc(pub Arc<dyn SettingsApi>);

impl SettingsSource for SettingsSrc {
    fn snapshot(&self) -> SettingsSnap {
        self.0.snapshot()
    }
    fn replace_if(&self, generation: u64, settings: Settings) -> Result<bool, SettingsError> {
        self.0.replace_if(generation, settings)
    }
}

/// `SettingsApi` over the `settings_api()` member of a services handle (the core serves it as a root).
pub struct SettingsVia(pub Arc<dyn crate::ServicesHandle>);

impl SettingsApi for SettingsVia {
    fn snapshot(&self) -> SettingsSnap {
        self.0.settings_api().snapshot()
    }
    fn replace_if(&self, generation: u64, settings: Settings) -> Result<bool, SettingsError> {
        self.0.settings_api().replace_if(generation, settings)
    }
}
