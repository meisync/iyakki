//! The UI's `Core` handle as a client of the core's UI socket (F-SPLIT-03, first slice): ONE domain is real end to end, the machine's
//! settings read as text (`settings_view`), carried by the typed contract over a unix socket with the scopes the listener grants. Every
//! other facet answers in plain words that it is not served over the socket yet (sound, app statuses) or takes the trait's own default.
//!
//! The UI program that uses this does not exist yet (it needs its own `main`, see docs/development/UI-BOUNDARY.md); this is the part of
//! it that can be built and tested now.

use std::any::Any;

use iyakki_core_api::contract::{CallContext, SettingValue, Settings};
use iyakki_core_api::{AppStatus, AppStatuses, Core, Mixer, MixerStats, FlowsView, SettingsView, SoundKind, VaultView};

use crate::client::{Connection, Remote, RemoteFlows, RemoteSharedSettings, RemoteVault};

/// Sound is not served over the socket: nothing plays, and the mixer says so.
struct NoMixer;

impl Mixer for NoMixer {
    fn stats(&self) -> Option<MixerStats> {
        None
    }
    fn active_sources(&self) -> Vec<(String, u32)> {
        Vec::new()
    }
    fn play_wav(&self, _: SoundKind, _: &str, _: &[u8]) -> Result<Box<dyn Any + Send>, String> {
        Err("sound is not served over the UI socket yet".into())
    }
}

struct NoStatuses;

impl AppStatuses for NoStatuses {
    fn all(&self, _: &str) -> Vec<(String, AppStatus)> {
        Vec::new()
    }
}

/// The settings domain over the socket.
pub struct SocketSettings {
    remote: RemoteSharedSettings,
}

/// The flows domain over the socket (list only here).
pub struct SocketFlows {
    remote: RemoteFlows,
}

impl FlowsView for SocketFlows {
    fn list(&self, _user: &str, _may_use: bool) -> Result<Vec<(String, String)>, String> {
        self.remote.list().map(|v| v.into_iter().map(|f| (f.id, f.name)).collect()).map_err(|e| e.to_string())
    }
}

/// The vault domain over the socket (status only: the unlock needs the machine's own seat).
pub struct SocketVault {
    remote: RemoteVault,
}

impl VaultView for SocketVault {
    fn status(&self, _user: &str) -> Result<(bool, u32), String> {
        self.remote.status().map(|s| (s.locked, s.entries)).map_err(|e| e.to_string())
    }
}

impl SettingsView for SocketSettings {
    fn keys(&self) -> Vec<String> {
        let mut k: Vec<String> = self.remote.schema(&CallContext::local("", false)).map(|s| s.into_iter().map(|x| x.key).collect()).unwrap_or_default();
        k.sort();
        k
    }
    fn get(&self, key: &str) -> Result<String, String> {
        self.remote.get(&CallContext::local("", false), key).map(|v| match v {
            SettingValue::Bool(b) => b.to_string(),
            SettingValue::Int(i) => i.to_string(),
            SettingValue::Text(t) => t,
        }).map_err(|e| e.to_string())
    }
}

pub struct SocketCore {
    settings: SocketSettings,
    vault: SocketVault,
    flows: SocketFlows,
    mixer: NoMixer,
    statuses: NoStatuses,
}

impl SocketCore {
    pub fn new(conn: Connection) -> SocketCore {
        let r = Remote::new(conn);
        SocketCore { settings: SocketSettings { remote: r.settings() }, vault: SocketVault { remote: r.vault() }, flows: SocketFlows { remote: r.flows() }, mixer: NoMixer, statuses: NoStatuses }
    }
}

impl Core for SocketCore {
    fn mixer(&self) -> &dyn Mixer {
        &self.mixer
    }
    fn app_status(&self) -> &dyn AppStatuses {
        &self.statuses
    }
    fn settings_view(&self) -> &dyn SettingsView {
        &self.settings
    }
    fn vault_view(&self) -> &dyn VaultView {
        &self.vault
    }
    fn flows_view(&self) -> &dyn FlowsView {
        &self.flows
    }
}
