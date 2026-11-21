//! The UI's `Core` handle as a client of the core's UI socket (F-SPLIT-03, first slice): ONE domain is real end to end, the machine's
//! settings read as text (`settings_view`), carried by the typed contract over a unix socket with the scopes the listener grants. Every
//! The embedded assistant (with pushed events) and the camera views are carried too (`facets`, F-SDK-85, F-SDK-166). Every other facet answers in plain words that it is not served over the socket yet (sound, app statuses) or takes the trait's own default.
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
    assistant: crate::facets::SocketAssistant,
    cameras: crate::facets::SocketCameras,
    /// The file scope of the core, when the program was given one (it asks the core; without one every path is denied).
    file_scope: Option<std::sync::Arc<dyn iyakki_core_api::FileScope>>,
    /// The core's sound as remote objects, when the program was given them (without them the mixer is silent and the sound facets are the quiet defaults).
    sound: Option<SoundFacets>,
    /// What the apps say about themselves, from the core (without it every icon has only its own static line).
    app_statuses: Option<std::sync::Arc<dyn AppStatuses>>,
}

/// The four facets of the core's sound, served by one object of the core.
#[derive(Clone)]
pub struct SoundFacets {
    pub mixer: std::sync::Arc<dyn Mixer>,
    pub sound: std::sync::Arc<dyn iyakki_core_api::SoundControl>,
    pub cues: std::sync::Arc<dyn iyakki_core_api::CuePlayer>,
    pub route: std::sync::Arc<dyn iyakki_core_api::AudioRoute>,
}

impl SocketCore {
    pub fn new(conn: Connection) -> SocketCore {
        let r = Remote::new(conn);
        let shared = r.connection();
        SocketCore { settings: SocketSettings { remote: r.settings() }, vault: SocketVault { remote: r.vault() }, flows: SocketFlows { remote: r.flows() }, mixer: NoMixer, statuses: NoStatuses, assistant: crate::facets::SocketAssistant::new(shared.clone()), cameras: crate::facets::SocketCameras::new(shared), file_scope: None, sound: None, app_statuses: None }
    }

    /// Hand the handle the core's file scope (every question is asked of the core; the default denies every path).
    pub fn with_file_scope(mut self, f: std::sync::Arc<dyn iyakki_core_api::FileScope>) -> SocketCore {
        self.file_scope = Some(f);
        self
    }

    /// Hand the handle the core's sound (mixer, output selector, cue player, sound route): every question and every sound goes to the core.
    pub fn with_sound(mut self, f: SoundFacets) -> SocketCore {
        self.sound = Some(f);
        self
    }

    /// Hand the handle the core's app statuses (the icon lines, chips and Attention list; the waiting approvals).
    pub fn with_statuses(mut self, s: std::sync::Arc<dyn AppStatuses>) -> SocketCore {
        self.app_statuses = Some(s);
        self
    }

    /// The assistant of this connection with its push calls (`subscribe`, `wait_events`), which the `Core` trait's pull-only facet does not have.
    pub fn socket_assistant(&self) -> &crate::facets::SocketAssistant {
        &self.assistant
    }
}

impl Core for SocketCore {
    fn mixer(&self) -> &dyn Mixer {
        match &self.sound {
            Some(s) => s.mixer.as_ref(),
            None => &self.mixer,
        }
    }
    fn sound(&self) -> &dyn iyakki_core_api::SoundControl {
        match &self.sound {
            Some(s) => s.sound.as_ref(),
            None => &iyakki_core_api::soundctl::NoSound,
        }
    }
    fn cues(&self) -> &dyn iyakki_core_api::CuePlayer {
        match &self.sound {
            Some(s) => s.cues.as_ref(),
            None => &iyakki_core_api::soundctl::NoCues,
        }
    }
    fn audio_route(&self) -> std::sync::Arc<dyn iyakki_core_api::AudioRoute> {
        match &self.sound {
            Some(s) => s.route.clone(),
            None => std::sync::Arc::new(iyakki_core_api::audioroute::NoAudioRoute),
        }
    }
    fn app_status(&self) -> &dyn AppStatuses {
        match &self.app_statuses {
            Some(s) => s.as_ref(),
            None => &self.statuses,
        }
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
    fn assistant(&self) -> &dyn iyakki_core_api::Assistant {
        &self.assistant
    }
    fn cameras(&self) -> &dyn iyakki_core_api::Cameras {
        &self.cameras
    }
    fn file_scope(&self) -> &dyn iyakki_core_api::FileScope {
        match &self.file_scope {
            Some(f) => f.as_ref(),
            None => &iyakki_core_api::NoFileScope,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_api::soundctl::SoundState;
    use iyakki_core_fake::FakeCore;
    use std::sync::Arc;

    fn dead() -> Connection {
        Connection::new(Box::new(std::io::Cursor::new(Vec::new())), Box::new(std::io::sink()))
    }

    #[test]
    fn without_sound_objects_the_core_handle_is_silent_and_with_them_every_sound_facet_goes_to_them() {
        let silent = SocketCore::new(dead());
        assert_eq!(silent.mixer().stats(), None);
        assert!(silent.mixer().play_wav(SoundKind::Media, "x", &[1]).is_err());
        assert_eq!(silent.sound().state(), SoundState::default());
        assert_eq!(silent.audio_route().card_count(), 0);

        // the same handle given the sound of a core: each of the four facets is the one it was given
        let fake = Arc::new(FakeCore::new());
        fake.mixer_fake().set_sources(vec![("Radio".into(), 50)]);
        fake.sound_fake().set_state(SoundState { active: true, reason: "chosen".into(), ..Default::default() });
        fake.audio_route_fake().put("cards", " 0 [PCH ]: HDA-Intel\n");
        struct Facet(Arc<FakeCore>);
        impl Mixer for Facet {
            fn stats(&self) -> Option<MixerStats> {
                self.0.mixer().stats()
            }
            fn active_sources(&self) -> Vec<(String, u32)> {
                self.0.mixer().active_sources()
            }
            fn play_wav(&self, k: SoundKind, l: &str, w: &[u8]) -> Result<Box<dyn Any + Send>, String> {
                self.0.mixer().play_wav(k, l, w)
            }
        }
        struct Sound(Arc<FakeCore>);
        impl iyakki_core_api::SoundControl for Sound {
            fn state(&self) -> SoundState {
                self.0.sound().state()
            }
        }
        struct Cues(Arc<FakeCore>);
        impl iyakki_core_api::CuePlayer for Cues {
            fn out(&self) -> Arc<dyn iyakki_core_api::soundtypes::CueOut> {
                self.0.cues().out()
            }
            fn play(&self, c: iyakki_core_api::soundtypes::Cue, o: Arc<dyn iyakki_core_api::soundtypes::CueOut>) -> iyakki_core_api::soundtypes::CueHandle {
                self.0.cues().play(c, o)
            }
        }
        let with = SocketCore::new(dead()).with_sound(SoundFacets { mixer: Arc::new(Facet(fake.clone())), sound: Arc::new(Sound(fake.clone())), cues: Arc::new(Cues(fake.clone())), route: fake.audio_route() });
        assert_eq!(with.mixer().active_sources(), [("Radio".to_string(), 50)]);
        assert_eq!(with.sound().state().reason, "chosen");
        assert_eq!(with.audio_route().card_count(), 1);
        let h = with.cues().play(iyakki_core_api::soundtypes::Cue::Login, with.cues().out());
        assert_eq!(h.wait(std::time::Duration::from_secs(5)), Some(iyakki_core_api::soundtypes::Outcome::Played));
        assert_eq!(fake.cues_fake().played().len(), 1);
    }
}
