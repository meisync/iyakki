//! The seam between a hands-free voice session and the speech-to-text that hears it: a [`VoiceBackend`] turns recorded sound (a WAV
//! file, 16 kHz mono) into text, says where that sound goes ([`Kind`]), whether the person must agree to that first
//! ([`VoiceBackend::needs_consent`]) and whether it is set up. [`Registry`] holds the backends this computer offers; the
//! `voice_backends` chat tool lists them (never a key) and says which one `voice.stt` uses now.
//!
//! What exists behind the seam today (see `docs/development/VOICE-ASSISTANT-DESIGN.md`): the engine chosen in Settings -> Voice
//! (`voice-settings`: a local command, or an OpenAI-style speech server on this computer, the network or the internet) and the
//! Lemonade Whisper model (`lemonade`). Both are [`EngineBackend`]s over the existing `voice::transcribe`; the voice session
//! itself (`voiceassist_ports::ServicesStt`) asks [`Registry::selected`] and calls the backend, so the consent check and the call sit in one place. A cloud provider
//! (the Gemini lane) plugs in by implementing [`VoiceBackend`] and calling [`Registry::register`]; it must report
//! [`Kind::Cloud`] so [`VoiceBackend::needs_consent`] is true and its own consent card comes first.
//!
//! No sound is stored here and nothing in this file touches a file, a device or the network by itself: the work is done by the
//! closure an [`EngineBackend`] is given.

use std::sync::Arc;

use crate::voice::SpeechEngine;

/// Where the sound goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Stays on this computer (a local program or a server on this computer).
    OnDevice,
    /// Another machine on the person's own network.
    Network,
    /// A service on the internet (the sound leaves the house).
    Cloud,
}

impl Kind {
    pub fn words(self) -> &'static str {
        match self {
            Kind::OnDevice => "sound stays on this computer",
            Kind::Network => "sound goes to another machine on your network",
            Kind::Cloud => "sound goes to a service on the internet",
        }
    }
}

/// One way to turn speech into text.
pub trait VoiceBackend: Send + Sync {
    /// A short stable name (`voice-settings`, `lemonade`).
    fn id(&self) -> &str;
    /// What the person sees: the engine, never a key.
    fn label(&self) -> String;
    fn kind(&self) -> Kind;
    /// The person must agree before sound goes there. Anything that is not on this computer needs it.
    fn needs_consent(&self) -> bool {
        self.kind() != Kind::OnDevice
    }
    /// Set up and usable (an engine is chosen / an account exists).
    fn configured(&self) -> bool;
    /// The engine this backend stands for, so `voice.stt` can be matched to it.
    fn engine(&self) -> Option<&SpeechEngine> {
        None
    }
    /// `wav` is a WAV file (16 kHz mono); `lang` a language hint such as `en`, or empty.
    fn transcribe(&self, wav: &[u8], lang: &str) -> Result<String, String>;
}

/// The call that does the work for an [`EngineBackend`].
pub type RunFn = Arc<dyn Fn(&SpeechEngine, &[u8]) -> Result<String, String> + Send + Sync>;

/// A backend over a [`SpeechEngine`] of the voice settings.
pub struct EngineBackend {
    pub id: String,
    pub engine: Option<SpeechEngine>,
    pub kind: Kind,
    pub run: RunFn,
}

impl VoiceBackend for EngineBackend {
    fn id(&self) -> &str {
        &self.id
    }
    fn label(&self) -> String {
        self.engine.as_ref().map_or_else(|| "not set up".to_string(), SpeechEngine::label)
    }
    fn kind(&self) -> Kind {
        self.kind
    }
    fn configured(&self) -> bool {
        self.engine.is_some()
    }
    fn engine(&self) -> Option<&SpeechEngine> {
        self.engine.as_ref()
    }
    fn transcribe(&self, wav: &[u8], lang: &str) -> Result<String, String> {
        let Some(e) = &self.engine else { return Err("this speech-to-text is not set up (Settings -> Voice)".into()) };
        let mut e = e.clone();
        if let SpeechEngine::Http { language, .. } = &mut e {
            if language.is_none() && !lang.trim().is_empty() {
                *language = Some(lang.trim().to_string());
            }
        }
        (self.run)(&e, wav)
    }
}

/// The backends this computer offers, in the order they are listed.
#[derive(Default)]
pub struct Registry {
    list: Vec<Arc<dyn VoiceBackend>>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry::default()
    }

    /// Add a backend. An id that is already taken is refused (a provider cannot take over another's name).
    pub fn register(&mut self, b: Arc<dyn VoiceBackend>) -> Result<(), String> {
        if b.id().trim().is_empty() || b.id().len() > 40 || !b.id().chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err("a backend id is 1 to 40 characters: lower-case letters, digits and dashes".into());
        }
        if self.list.iter().any(|x| x.id() == b.id()) {
            return Err(format!("a voice backend named {} is already registered", b.id()));
        }
        self.list.push(b);
        Ok(())
    }

    pub fn list(&self) -> &[Arc<dyn VoiceBackend>] {
        &self.list
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn VoiceBackend>> {
        self.list.iter().find(|b| b.id() == id).cloned()
    }

    /// The backend `voice.stt` uses now. A chosen engine is matched to the backend that stands for it; with none chosen the
    /// first configured backend that is on this computer is the automatic one (what `voicedesk::stt_engine` does with Lemonade).
    pub fn selected(&self, stt: Option<&SpeechEngine>) -> Option<Arc<dyn VoiceBackend>> {
        match stt {
            Some(e) => self.list.iter().find(|b| b.engine() == Some(e)).cloned(),
            None => self.list.iter().find(|b| b.configured() && b.kind() == Kind::OnDevice).cloned(),
        }
    }

    /// The `voice_backends` answer: one line per backend, then the one in use. Contains no key and no sound.
    pub fn status(&self, stt: Option<&SpeechEngine>) -> String {
        if self.list.is_empty() {
            return "No voice backends are available.".into();
        }
        let chosen = self.selected(stt).map(|b| b.id().to_string());
        let mut out = String::from("Speech-to-text backends:\n");
        for b in &self.list {
            let set = if b.configured() { "set up" } else { "not set up" };
            let consent = if b.needs_consent() { "asks you before sound is sent" } else { "no consent needed" };
            let now = if chosen.as_deref() == Some(b.id()) { " [in use]" } else { "" };
            out.push_str(&format!("- {}: {} ({}; {}; {}){}\n", b.id(), b.label(), b.kind().words(), set, consent, now));
        }
        match chosen {
            Some(id) => out.push_str(&format!("Voice sessions use: {id}.")),
            None => out.push_str("Voice sessions have no speech-to-text yet (Settings -> Voice, or download a Whisper model in Settings -> Models)."),
        }
        out
    }

    /// The backends of this computer's settings: the chosen engine and the Lemonade Whisper model.
    pub fn from_services(svc: &Arc<crate::services::Services>) -> Registry {
        use crate::voicedesk as vd;
        let kind_of = |e: &SpeechEngine| match vd::destination(svc, e).map(|d| d.place) {
            None | Some(crate::speechtime::Locality::ThisComputer) => Kind::OnDevice,
            Some(crate::speechtime::Locality::Network) => Kind::Network,
            Some(crate::speechtime::Locality::Internet) => Kind::Cloud,
        };
        let run = |svc: &Arc<crate::services::Services>| -> RunFn {
            let svc = svc.clone();
            Arc::new(move |e: &SpeechEngine, wav: &[u8]| {
                let up = svc.slow_upstream();
                crate::voice::transcribe(e, wav, &svc.secrets, up.as_ref())
            })
        };
        let mut r = Registry::new();
        let chosen = svc.voice_settings().stt;
        let kind = chosen.as_ref().map_or(Kind::OnDevice, kind_of);
        let _ = r.register(Arc::new(EngineBackend { id: "voice-settings".into(), engine: chosen, kind, run: run(svc) }));
        let lem = vd::lemonade_stt_engine(svc);
        let kind = lem.as_ref().map_or(Kind::OnDevice, kind_of);
        let _ = r.register(Arc::new(EngineBackend { id: "lemonade".into(), engine: lem, kind, run: run(svc) }));
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn http(url: &str) -> SpeechEngine {
        SpeechEngine::Http { url: url.into(), model: "whisper-1".into(), key_ref: Some("vault:speech".into()), voice: None, language: None }
    }
    fn cmd() -> SpeechEngine {
        SpeechEngine::Command { command: "whisper-cli -f {wav}".into() }
    }
    fn backend(id: &str, e: Option<SpeechEngine>, kind: Kind, heard: &Arc<Mutex<Vec<String>>>) -> Arc<EngineBackend> {
        let h = heard.clone();
        Arc::new(EngineBackend {
            id: id.into(),
            engine: e,
            kind,
            run: Arc::new(move |e, wav| {
                let lang = if let SpeechEngine::Http { language, .. } = e { language.clone().unwrap_or_default() } else { String::new() };
                h.lock().unwrap_or_else(|x| x.into_inner()).push(format!("{} bytes, lang={lang}", wav.len()));
                Ok("hello".into())
            }),
        })
    }

    #[test]
    fn consent_follows_where_the_sound_goes() {
        let h = Arc::new(Mutex::new(Vec::new()));
        assert!(!backend("a", Some(cmd()), Kind::OnDevice, &h).needs_consent());
        assert!(backend("b", Some(http("http://192.168.1.5:8080/v1")), Kind::Network, &h).needs_consent());
        assert!(backend("c", Some(http("https://speech.example/v1")), Kind::Cloud, &h).needs_consent());
    }

    #[test]
    fn transcribe_passes_the_language_hint_and_refuses_when_not_set_up() {
        let h = Arc::new(Mutex::new(Vec::new()));
        let b = backend("a", Some(http("http://localhost:1/v1")), Kind::OnDevice, &h);
        assert_eq!(b.transcribe(&[0; 10], "ta").unwrap(), "hello");
        assert_eq!(b.transcribe(&[0; 4], "").unwrap(), "hello");
        assert_eq!(*h.lock().unwrap(), vec!["10 bytes, lang=ta", "4 bytes, lang="]);
        let none = backend("n", None, Kind::OnDevice, &h);
        assert!(!none.configured());
        assert!(none.transcribe(&[0; 4], "en").unwrap_err().contains("not set up"));
        assert_eq!(h.lock().unwrap().len(), 2, "nothing ran");
    }

    #[test]
    fn the_registry_refuses_duplicate_and_odd_ids_and_finds_the_one_in_use() {
        let h = Arc::new(Mutex::new(Vec::new()));
        let mut r = Registry::new();
        r.register(backend("voice-settings", Some(cmd()), Kind::OnDevice, &h)).unwrap();
        assert!(r.register(backend("voice-settings", None, Kind::OnDevice, &h)).unwrap_err().contains("already"));
        assert!(r.register(backend("Bad Id", None, Kind::OnDevice, &h)).is_err());
        assert!(r.register(backend("", None, Kind::OnDevice, &h)).is_err());
        r.register(backend("cloud-x", Some(http("https://speech.example/v1")), Kind::Cloud, &h)).unwrap();
        assert_eq!(r.list().len(), 2);
        assert_eq!(r.selected(Some(&http("https://speech.example/v1"))).unwrap().id(), "cloud-x");
        assert_eq!(r.selected(Some(&cmd())).unwrap().id(), "voice-settings");
        assert!(r.selected(Some(&http("http://other/v1"))).is_none(), "an engine no backend stands for");
        assert_eq!(r.selected(None).unwrap().id(), "voice-settings", "automatic: the first configured one on this computer");
    }

    #[test]
    fn automatic_never_picks_a_cloud_backend() {
        let h = Arc::new(Mutex::new(Vec::new()));
        let mut r = Registry::new();
        r.register(backend("cloud-x", Some(http("https://speech.example/v1")), Kind::Cloud, &h)).unwrap();
        assert!(r.selected(None).is_none());
        assert!(r.status(None).contains("no speech-to-text yet"));
    }

    #[test]
    fn the_status_names_consent_and_the_backend_in_use_and_never_a_key() {
        let h = Arc::new(Mutex::new(Vec::new()));
        let mut r = Registry::new();
        r.register(backend("voice-settings", Some(http("https://speech.example/v1")), Kind::Cloud, &h)).unwrap();
        r.register(backend("lemonade", None, Kind::OnDevice, &h)).unwrap();
        let s = r.status(Some(&http("https://speech.example/v1")));
        assert!(s.contains("voice-settings") && s.contains("[in use]") && s.contains("asks you before sound is sent") && s.contains("sound goes to a service on the internet"), "{s}");
        assert!(s.contains("lemonade: not set up") && s.contains("not set up;"), "{s}");
        assert!(s.contains("Voice sessions use: voice-settings."));
        assert!(!s.contains("vault:speech") && !s.contains("key"), "no key reference in the answer: {s}");
        assert_eq!(Registry::new().status(None), "No voice backends are available.");
    }
}
