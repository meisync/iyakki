//! The made-up sound output and cue player (`iyakki_core_api::{SoundControl, CuePlayer}`): canned state in, the notices and played cues recorded.

use iyakki_core_api::soundtypes::{Cue, CueHandle, CueOut, Outcome, Output, SoundSettings};
use iyakki_core_api::{CuePlayer, SoundControl, SoundState};
use std::path::Path;
use std::sync::{Arc, Mutex};

fn lk<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

type Scan = Arc<dyn Fn(&Path) -> Vec<Output> + Send + Sync>;
type ViaScan = Arc<dyn Fn(&dyn iyakki_core_api::AudioRoute) -> Vec<Output> + Send + Sync>;
type ViaStart = Arc<dyn Fn(&dyn iyakki_core_api::AudioRoute) -> Option<u32> + Send + Sync>;
type StartScan = Arc<dyn Fn(&Path, &Path) -> Option<u32> + Send + Sync>;

#[derive(Default)]
pub struct FakeSound {
    scan: Mutex<Option<Scan>>,
    start_scan: Mutex<Option<StartScan>>,
    via_scan: Mutex<Option<ViaScan>>,
    via_start: Mutex<Option<ViaStart>>,
    state: Mutex<SoundState>,
    prefs: Mutex<SoundSettings>,
    outputs: Mutex<Vec<Output>>,
    notices: Mutex<Vec<String>>,
    note: Mutex<Option<String>>,
    starting_card: Mutex<Option<u32>>,
    server: Mutex<bool>,
}

impl FakeSound {
    pub fn set_state(&self, s: SoundState) {
        *lk(&self.state) = s;
    }
    pub fn set_prefs(&self, p: SoundSettings) {
        *lk(&self.prefs) = p;
    }
    pub fn set_outputs(&self, o: Vec<Output>) {
        *lk(&self.outputs) = o;
    }
    pub fn push_notice(&self, n: &str) {
        lk(&self.notices).push(n.to_string());
    }
    pub fn set_volume_note(&self, n: Option<&str>) {
        *lk(&self.note) = n.map(str::to_string);
    }
    pub fn set_card_starting(&self, c: Option<u32>) {
        *lk(&self.starting_card) = c;
    }
    /// Answer `outputs_at` by looking at the given tree with `f` (a test that builds a made-up `/proc/asound` brings its own scanner).
    pub fn set_scanner(&self, f: impl Fn(&Path) -> Vec<Output> + Send + Sync + 'static) {
        *lk(&self.scan) = Some(Arc::new(f));
    }
    /// The same for `card_starting`.
    pub fn set_starting_scanner(&self, f: impl Fn(&Path, &Path) -> Option<u32> + Send + Sync + 'static) {
        *lk(&self.start_scan) = Some(Arc::new(f));
    }
    /// Answer `outputs_via` by deciding over the route's facts with `f` (the agent's own decision code in a page test).
    pub fn set_route_scanner(&self, f: impl Fn(&dyn iyakki_core_api::AudioRoute) -> Vec<Output> + Send + Sync + 'static) {
        *lk(&self.via_scan) = Some(Arc::new(f));
    }
    /// The same for `card_starting_via`.
    pub fn set_route_starting_scanner(&self, f: impl Fn(&dyn iyakki_core_api::AudioRoute) -> Option<u32> + Send + Sync + 'static) {
        *lk(&self.via_start) = Some(Arc::new(f));
    }
    pub fn set_server(&self, on: bool) {
        *lk(&self.server) = on;
    }
}

impl SoundControl for FakeSound {
    fn state(&self) -> SoundState {
        lk(&self.state).clone()
    }
    fn refresh_starting(&self) -> Option<String> {
        let s = lk(&self.state);
        if s.active { s.starting.clone() } else { None }
    }
    fn prefs(&self) -> SoundSettings {
        lk(&self.prefs).clone()
    }
    fn outputs_at(&self, proc_root: &Path) -> Vec<Output> {
        if let Some(f) = lk(&self.scan).clone() {
            return f(proc_root);
        }
        lk(&self.outputs).clone()
    }
    fn card_starting(&self, proc_root: &Path, sys: &Path) -> Option<u32> {
        if let Some(f) = lk(&self.start_scan).clone() {
            return f(proc_root, sys);
        }
        *lk(&self.starting_card)
    }
    fn outputs_via(&self, route: &dyn iyakki_core_api::AudioRoute) -> Vec<Output> {
        if let Some(f) = lk(&self.via_scan).clone() {
            return f(route);
        }
        lk(&self.outputs).clone()
    }
    fn card_starting_via(&self, route: &dyn iyakki_core_api::AudioRoute) -> Option<u32> {
        if let Some(f) = lk(&self.via_start).clone() {
            return f(route);
        }
        *lk(&self.starting_card)
    }
    fn volume_note(&self) -> Option<String> {
        lk(&self.note).clone()
    }
    fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut *lk(&self.notices))
    }
    fn server_running(&self) -> bool {
        *lk(&self.server)
    }
    fn command_with_device(&self, play_cmd: &str, device: &str) -> Result<String, String> {
        Ok(format!("{play_cmd} -D {device}"))
    }
}

/// Plays nothing; records the cues that were asked for, in order.
#[derive(Default)]
pub struct FakeCues {
    played: Mutex<Vec<Cue>>,
    ready: Mutex<Option<String>>,
}

impl FakeCues {
    /// The cues started so far.
    pub fn played(&self) -> Vec<Cue> {
        lk(&self.played).clone()
    }
    /// Make the output refuse (`Some(why)`) or work again (`None`).
    pub fn set_unavailable(&self, why: Option<&str>) {
        *lk(&self.ready) = why.map(str::to_string);
    }
}

struct FakeOut(Option<String>);
impl CueOut for FakeOut {
    fn ready(&self) -> Result<(), String> {
        self.0.clone().map_or(Ok(()), Err)
    }
    fn play(&self, _wav: &[u8]) -> Result<(), String> {
        Ok(())
    }
}

impl CuePlayer for FakeCues {
    fn out(&self) -> Arc<dyn CueOut> {
        Arc::new(FakeOut(lk(&self.ready).clone()))
    }
    fn play(&self, cue: Cue, out: Arc<dyn CueOut>) -> CueHandle {
        lk(&self.played).push(cue);
        match out.ready() {
            Ok(()) => CueHandle::finished(Outcome::Played),
            Err(why) => CueHandle::finished(Outcome::NoOutput(why)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fake_sound_answers_what_it_was_given_and_hands_notices_out_once() {
        let s = FakeSound::default();
        s.set_state(SoundState { active: true, starting: Some("waiting".into()), ..Default::default() });
        assert_eq!(s.refresh_starting().as_deref(), Some("waiting"));
        s.push_notice("a");
        assert_eq!(s.take_notices(), vec!["a".to_string()]);
        assert!(s.take_notices().is_empty());
        s.set_volume_note(Some("no control"));
        assert_eq!(s.volume_note().as_deref(), Some("no control"));
    }

    #[test]
    fn a_scanner_answers_for_the_tree_it_is_given() {
        let s = FakeSound::default();
        s.set_scanner(|p| if p.ends_with("here") { vec![Output::system_default()] } else { vec![] });
        assert_eq!(s.outputs_at(Path::new("/x/here")).len(), 1);
        assert!(s.outputs_at(Path::new("/x/there")).is_empty());
        s.set_starting_scanner(|_, _| Some(2));
        assert_eq!(s.card_starting(Path::new("/p"), Path::new("/s")), Some(2));
        // the route forms: canned answers, or the decision code over the route's facts
        s.set_outputs(vec![Output::system_default()]);
        let route = crate::audioroute::FakeAudioRoute::default();
        route.put("cards", " 0 [PCH]: HDA-Intel - HDA Intel PCH\n");
        assert_eq!(s.outputs_via(&route).len(), 1);
        s.set_route_scanner(|r| if r.card_count() == 1 { vec![Output::system_default(), Output::system_default()] } else { vec![] });
        assert_eq!(s.outputs_via(&route).len(), 2);
        s.set_route_starting_scanner(|r| r.asound_has("cards").then_some(0));
        assert_eq!(s.card_starting_via(&route), Some(0));
        assert!(route.calls().iter().any(|c| c == "text:cards"), "the facts were asked of the route");
    }

    #[test]
    fn cues_are_recorded_and_a_missing_output_is_said() {
        let c = FakeCues::default();
        assert_eq!(c.play(Cue::Login, c.out()).wait_short(), Some(Outcome::Played));
        c.set_unavailable(Some("none"));
        assert_eq!(c.play(Cue::Logout, c.out()).wait_short(), Some(Outcome::NoOutput("none".into())));
        assert_eq!(c.played(), vec![Cue::Login, Cue::Logout]);
    }
}
