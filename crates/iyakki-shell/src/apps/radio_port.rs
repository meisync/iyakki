//! What the RADIO tile asks of the rest of the system, as one trait (F-RADIO-01, the worked example of docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10).
//!
//! The tile (`radio.rs`) holds no `Services`, no `AppCx`, no file path and no thread of its own for the network: it asks a `RadioPort`.
//! The real port ([`CxPort`]) is the only place that knows `cx.services`; it also checks the person's host policy and writes the audit line.
//! Tests drive the tile through [`tests::FakePort`], so a test needs no sound, no network and no folder.

use std::sync::mpsc::Receiver;
use std::sync::Arc;

use iyakki_agent::radio::{CertProblem, Holder, Language, RadioState, SoundGate, Station, StationStore, BANDS};
use iyakki_agent::soundhold::{Found, Registry, Who};

use super::AppCx;

pub use iyakki_tile_api::radio::{RadioPort, SearchAnswer};

fn holder_of(f: &Found) -> Holder {
    match &f.who {
        Who::Own(k) => Holder { says: f.says.clone(), own_key: Some(k.clone()), outside: None },
        Who::Outside { pid, name } => Holder { says: f.says.clone(), own_key: None, outside: Some((*pid, name.clone())) },
    }
}

fn found_of(h: &Holder) -> Found {
    let who = match (&h.own_key, &h.outside) {
        (Some(k), _) => Who::Own(k.clone()),
        (None, Some((pid, name))) => Who::Outside { pid: *pid, name: name.clone() },
        _ => Who::Own(String::new()),
    };
    Found { who, says: h.says.clone() }
}

/// The real speaker question: the players announce themselves in `reg`; `mixing` says whether the mixer shares the speaker (then only the browser is in the way).
pub struct RegistrySound {
    reg: Arc<Registry>,
    mixing: fn() -> bool,
}

impl RegistrySound {
    pub fn new(reg: Arc<Registry>, mixing: fn() -> bool) -> RegistrySound {
        RegistrySound { reg, mixing }
    }
}

impl SoundGate for RegistrySound {
    fn conflict(&self) -> Option<Holder> {
        self.reg.conflict_for_new_player("radio", (self.mixing)()).map(|f| holder_of(&f))
    }
    fn stop(&self, holder: &Holder, wait_secs: u64) -> Receiver<Result<String, String>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let (found, reg) = (found_of(holder), self.reg.clone());
        let spawned = std::thread::Builder::new().name("iyakki-soundstop".into()).spawn({
            let tx = tx.clone();
            move || {
                let _ = tx.send(reg.stop(&found, &iyakki_agent::quiet::System, std::path::Path::new(iyakki_agent::quiet::ASOUND), std::time::Duration::from_secs(wait_secs)));
            }
        });
        if spawned.is_err() {
            let _ = tx.send(Err("the stop could not start".into()));
        }
        rx
    }
}

/// The real port: the signed-in person's services, policy and audit log, all taken from the shell's `AppCx`.
pub struct CxPort<'a>(pub &'a AppCx);

impl CxPort<'_> {
    fn store(&self) -> StationStore {
        StationStore::new(self.0.services.dir(), &self.0.session.user)
    }
}

impl RadioPort for CxPort<'_> {
    fn state(&self) -> RadioState {
        self.0.services.radio.state()
    }
    fn is_active(&self) -> bool {
        self.0.services.radio.is_active()
    }
    fn is_paused(&self) -> bool {
        self.0.services.radio.is_paused()
    }
    fn cert_problem(&self) -> Option<CertProblem> {
        self.0.services.radio.cert_problem()
    }
    fn volume(&self) -> u8 {
        self.0.services.radio.volume()
    }
    fn set_volume(&self, v: u8) {
        self.0.services.radio.set_volume(v)
    }
    fn levels(&self) -> [f32; BANDS] {
        self.0.services.radio.levels()
    }
    fn resume(&self) -> bool {
        self.0.services.radio.resume()
    }
    fn pause(&self) -> bool {
        self.0.services.radio.pause()
    }
    fn stop(&self) {
        self.0.services.radio.stop()
    }
    fn play(&self, station: &Station) {
        let policy = self.0.session.policy.clone();
        let allow: Arc<dyn Fn(&str) -> bool + Send + Sync> = Arc::new(move |h| policy.allows_host(h));
        self.0.services.radio.play(station, allow, &self.0.services.voice_settings().play_cmd);
    }
    fn allows_host(&self, host: &str) -> bool {
        self.0.session.policy.allows_host(host)
    }
    fn mine(&self) -> Vec<Station> {
        self.store().list()
    }
    fn add(&self, name: &str, url: &str) -> Result<Station, String> {
        self.store().add(name, url)
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        self.store().remove(name)
    }
    fn trust(&self, base: &Station, problem: &CertProblem) -> Result<Station, String> {
        self.store().trust(base, problem)
    }
    fn forget_trust(&self, name: &str) -> Result<Station, String> {
        self.store().forget_trust(name)
    }
    fn search(&self, words: &str) -> Receiver<SearchAnswer> {
        let (tx, rx) = std::sync::mpsc::channel();
        let (svc, policy, user, q) = (self.0.services.clone(), self.0.session.policy.clone(), self.0.session.user.clone(), words.to_string());
        std::thread::spawn(move || {
            let _ = tx.send(iyakki_agent::radiotools::search_stations(&svc, &policy, &user, &q, 60));
        });
        rx
    }
    fn addon_installed(&self) -> bool {
        iyakki_agent::aacpipe::installed()
    }
    fn audit(&self, action: &str, subject: &str) {
        self.0.audit.record(&self.0.session.user, action, subject);
    }
    fn sound(&self) -> Arc<dyn SoundGate> {
        Arc::new(RegistrySound::new(iyakki_agent::soundhold::global(), iyakki_agent::audiosink::mixing))
    }
    fn languages(&self) -> &'static [Language] {
        &iyakki_agent::radio::LANGUAGES[..]
    }
    fn url_needs_addon(&self, url: &str) -> bool {
        iyakki_agent::radio_stations::needs_addon(url)
    }
    fn failure_needs_addon(&self, why: &str) -> bool {
        iyakki_agent::audiofmt::needs_addon(why)
    }
}
