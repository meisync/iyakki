//! The RADIO tile against a fake port: no sound, no network, no folder, no `AppCx` (the worked example of
//! docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10). The same behaviours are also proved through the real port in `radio.rs`.

use super::*;
use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;

pub struct FakePort {
    pub log: Mutex<Vec<String>>,
    pub state: Mutex<RadioState>,
    pub active: Mutex<bool>,
    pub paused: Mutex<bool>,
    pub volume: Mutex<u8>,
    pub mine: Mutex<Vec<Station>>,
    pub deny_hosts: Mutex<Vec<String>>,
    pub problem: Mutex<Option<CertProblem>>,
    pub add_error: Mutex<Option<String>>,
    pub search_tx: Mutex<Option<Sender<iyakki_tile_api::radio::SearchAnswer>>>,
    pub addon: Mutex<bool>,
    pub gate: Arc<FakeGate>,
}

/// The fake speaker question: whoever is set in `holder` is in the way; a stop is recorded, clears the holder and answers at once.
pub struct FakeGate {
    pub holder: Mutex<Option<Holder>>,
    pub stops: Mutex<Vec<String>>,
    pub stop_answer: Mutex<Result<String, String>>,
}

impl SoundGate for FakeGate {
    fn conflict(&self) -> Option<Holder> {
        self.holder.lock().unwrap().clone()
    }
    fn stop(&self, holder: &Holder, _wait_secs: u64) -> std::sync::mpsc::Receiver<Result<String, String>> {
        self.stops.lock().unwrap().push(holder.says.clone());
        let answer = self.stop_answer.lock().unwrap().clone();
        if answer.is_ok() {
            *self.holder.lock().unwrap() = None;
        }
        let (tx, rx) = channel();
        let _ = tx.send(answer);
        rx
    }
}

impl FakePort {
    pub fn new() -> FakePort {
        FakePort { log: Mutex::default(), state: Mutex::new(RadioState::Idle), active: Mutex::new(false), paused: Mutex::new(false), volume: Mutex::new(50), mine: Mutex::default(), deny_hosts: Mutex::default(), problem: Mutex::new(None), add_error: Mutex::new(None), search_tx: Mutex::new(None), addon: Mutex::new(true), gate: Arc::new(FakeGate { holder: Mutex::new(None), stops: Mutex::default(), stop_answer: Mutex::new(Ok("Stopped.".to_string())) }) }
    }
    fn say(&self, s: impl Into<String>) {
        self.log.lock().unwrap().push(s.into());
    }
    pub fn calls(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

/// Two small languages for the fake port (the tile must not need the agent's list).
static FAKE_LANGS: [Language; 2] = [Language { name: "Alpha", stations: &[("Alpha One", "https://alpha.example.org/one.mp3"), ("Alpha Two", "https://alpha.example.org/two.aac")] }, Language { name: "Beta", stations: &[("Beta One", "https://beta.example.org/one.mp3")] }];

impl RadioPort for FakePort {
    fn state(&self) -> RadioState {
        self.state.lock().unwrap().clone()
    }
    fn is_active(&self) -> bool {
        *self.active.lock().unwrap()
    }
    fn is_paused(&self) -> bool {
        *self.paused.lock().unwrap()
    }
    fn cert_problem(&self) -> Option<CertProblem> {
        self.problem.lock().unwrap().clone()
    }
    fn volume(&self) -> u8 {
        *self.volume.lock().unwrap()
    }
    fn set_volume(&self, v: u8) {
        *self.volume.lock().unwrap() = v.min(100);
        self.say(format!("volume {}", v.min(100)));
    }
    fn levels(&self) -> [f32; BANDS] {
        [0.3; BANDS]
    }
    fn resume(&self) -> bool {
        let was = std::mem::replace(&mut *self.paused.lock().unwrap(), false);
        if was {
            self.say("resume");
        }
        was
    }
    fn pause(&self) -> bool {
        let on = *self.active.lock().unwrap();
        if on {
            *self.paused.lock().unwrap() = true;
            self.say("pause");
        }
        on
    }
    fn stop(&self) {
        *self.active.lock().unwrap() = false;
        *self.state.lock().unwrap() = RadioState::Idle;
        self.say("stop");
    }
    fn play(&self, station: &Station) {
        *self.active.lock().unwrap() = true;
        *self.state.lock().unwrap() = RadioState::Playing { station: station.name.clone(), title: String::new(), kbps: 128 };
        self.say(format!("play {} {}", station.name, station.url));
    }
    fn allows_host(&self, host: &str) -> bool {
        !self.deny_hosts.lock().unwrap().iter().any(|h| h == host)
    }
    fn mine(&self) -> Vec<Station> {
        self.mine.lock().unwrap().clone()
    }
    fn add(&self, name: &str, url: &str) -> Result<Station, String> {
        if let Some(e) = self.add_error.lock().unwrap().clone() {
            return Err(e);
        }
        let st = Station::new(name, url);
        self.mine.lock().unwrap().push(st.clone());
        self.say(format!("add {name}"));
        Ok(st)
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        self.mine.lock().unwrap().retain(|s| s.name != name);
        self.say(format!("remove {name}"));
        Ok(())
    }
    fn trust(&self, base: &Station, problem: &CertProblem) -> Result<Station, String> {
        self.say(format!("trust {} {}", base.name, problem.host));
        let mut st = base.clone();
        st.cert_pin = Some(problem.pin());
        Ok(st)
    }
    fn forget_trust(&self, name: &str) -> Result<Station, String> {
        self.say(format!("forget {name}"));
        Ok(Station::new(name, "https://x"))
    }
    fn search(&self, words: &str) -> std::sync::mpsc::Receiver<iyakki_tile_api::radio::SearchAnswer> {
        self.say(format!("search {words}"));
        let (tx, rx) = channel();
        *self.search_tx.lock().unwrap() = Some(tx);
        rx
    }
    fn addon_installed(&self) -> bool {
        *self.addon.lock().unwrap()
    }
    fn audit(&self, action: &str, subject: &str) {
        self.say(format!("audit {action} {subject}"));
    }
    fn sound(&self) -> Arc<dyn SoundGate> {
        self.gate.clone()
    }
    fn languages(&self) -> &'static [Language] {
        &FAKE_LANGS
    }
    fn url_needs_addon(&self, url: &str) -> bool {
        url.ends_with(".aac")
    }
    fn failure_needs_addon(&self, why: &str) -> bool {
        why.contains("needs the audio add-on")
    }
}

fn key(a: &mut RadioApp, p: &FakePort, c: char) {
    a.handle_with(&Event::Key(Key::Char(c)), p);
}

fn typ(a: &mut RadioApp, p: &FakePort, text: &str) {
    for c in text.chars() {
        a.handle_with(&Event::Key(Key::Char(c)), p);
    }
}

fn new(p: &FakePort) -> RadioApp {
    RadioApp::new(p)
}

#[test]
fn the_keys_play_stop_pause_and_set_the_volume_through_the_port_and_nothing_else() {
    let p = FakePort::new();
    let mut a = new(&p);
    // language tab 1 has presets; choose the first and play it with Enter
    a.handle_with(&Event::Key(Key::Right), &p);
    let first = a.station_names()[0].clone();
    a.handle_with(&Event::Key(Key::Enter), &p);
    let calls = p.calls();
    assert!(calls.iter().any(|c| c.starts_with(&format!("play {first} "))), "{calls:?}");
    assert!(calls.iter().any(|c| c.starts_with("audit radio.play ")), "{calls:?}");
    // the audit line names the host only, never the path or a query
    let line = calls.iter().find(|c| c.starts_with("audit radio.play ")).unwrap();
    assert!(!line.contains('/') && !line.contains('?'), "{line}");
    // Space stops a playing station, and starts the chosen one when nothing plays
    key(&mut a, &p, ' ');
    assert_eq!(p.calls().last().unwrap(), "stop");
    key(&mut a, &p, ' ');
    assert!(p.calls().last().unwrap().starts_with("play "));
    // pause and carry on
    key(&mut a, &p, 'p');
    assert_eq!(p.calls().last().unwrap(), "pause");
    key(&mut a, &p, 'p');
    assert_eq!(p.calls().last().unwrap(), "resume");
    // volume in steps of five, never past either end
    for _ in 0..30 {
        key(&mut a, &p, '+');
    }
    assert_eq!(p.volume(), 100);
    for _ in 0..30 {
        key(&mut a, &p, '-');
    }
    assert_eq!(p.volume(), 0);
    // a click on the ninth volume bar is 40
    a.handle_with(&Event::Tab(208), &p);
    assert_eq!(p.volume(), 40);
    // s stops; only these calls reached the port, none of them a path or a program
    key(&mut a, &p, 's');
    assert!(p.calls().iter().all(|c| ["play ", "audit ", "stop", "pause", "resume", "volume "].iter().any(|k| c.starts_with(k))), "{:?}", p.calls());
}

#[test]
fn a_host_the_account_may_not_reach_is_refused_before_anything_plays_and_the_person_is_told() {
    let p = FakePort::new();
    let mut a = new(&p);
    a.handle_with(&Event::Key(Key::Right), &p);
    let host = radio::host_of(&a.list()[0].url);
    p.deny_hosts.lock().unwrap().push(host.clone());
    a.handle_with(&Event::Key(Key::Enter), &p);
    assert!(a.status_text().unwrap().contains(&host) && a.status_text().unwrap().contains("may not reach"), "{:?}", a.status_text());
    assert!(!p.calls().iter().any(|c| c.starts_with("play ") || c.starts_with("audit radio.play")), "{:?}", p.calls());
}

#[test]
fn adding_a_station_goes_through_the_port_and_its_refusal_stays_in_the_form() {
    let p = FakePort::new();
    let mut a = new(&p);
    key(&mut a, &p, 'n');
    typ(&mut a, &p, "Jazz FM");
    a.handle_with(&Event::Key(Key::Tab), &p);
    typ(&mut a, &p, "https://jazz.example.org/live.mp3");
    *p.add_error.lock().unwrap() = Some("That address is not allowed.".into());
    a.handle_with(&Event::Key(Key::Enter), &p);
    assert!(matches!(a.mode, Mode::Form(_)), "the form stays open with the reason");
    assert!(!p.calls().iter().any(|c| c.starts_with("add ")));
    *p.add_error.lock().unwrap() = None;
    a.handle_with(&Event::Key(Key::Enter), &p);
    assert!(matches!(a.mode, Mode::Browse));
    assert_eq!(p.calls().iter().filter(|c| c.starts_with("add ")).count(), 1);
    assert!(p.calls().contains(&"audit radio.add jazz.example.org".to_string()), "{:?}", p.calls());
    assert_eq!(a.tab_name(), "My stations");
    assert_eq!(a.station_names(), ["Jazz FM"]);
}

#[test]
fn a_removal_asks_first_and_only_y_removes() {
    let p = FakePort::new();
    p.mine.lock().unwrap().extend([Station::new("One", "https://a.example.org/1"), Station::new("Two", "https://a.example.org/2")]);
    let mut a = new(&p);
    key(&mut a, &p, 'd');
    assert!(matches!(a.mode, Mode::ConfirmDelete(_)));
    key(&mut a, &p, 'x');
    assert!(!p.calls().iter().any(|c| c.starts_with("remove ")), "any other key is a no");
    key(&mut a, &p, 'd');
    key(&mut a, &p, 'y');
    assert_eq!(p.calls().iter().filter(|c| c.starts_with("remove ")).count(), 1);
    assert_eq!(a.station_names(), ["Two"]);
    assert!(p.calls().contains(&"audit radio.remove a.example.org".to_string()), "{:?}", p.calls());
}

#[test]
fn a_search_runs_through_the_port_and_its_answer_becomes_the_last_entry() {
    let p = FakePort::new();
    let mut a = new(&p);
    key(&mut a, &p, '/');
    typ(&mut a, &p, "tamil jazz");
    a.handle_with(&Event::Key(Key::Enter), &p);
    assert!(matches!(a.mode, Mode::Searching { .. }));
    assert!(p.calls().contains(&"search tamil jazz".to_string()) && p.calls().contains(&"audit radio.search stations".to_string()));
    // nothing yet: still searching; Esc gives it up
    a.handle_with(&Event::Tick, &p);
    assert!(matches!(a.mode, Mode::Searching { .. }));
    // the answer arrives
    p.search_tx.lock().unwrap().as_ref().unwrap().send((vec![("A".into(), "https://a.example.org/a".into()), ("B".into(), "https://b.example.org/b".into())], Some("No internet: the built-in lists only.".into()))).unwrap();
    a.handle_with(&Event::Tick, &p);
    assert!(matches!(a.mode, Mode::Browse));
    assert_eq!(a.tab_name(), "Search results");
    assert_eq!(a.station_names(), ["A", "B"]);
    let t = a.status_text().unwrap();
    assert!(t.contains("2 stations") && t.contains("No internet"), "{t}");
    // a search that is given up
    key(&mut a, &p, '/');
    typ(&mut a, &p, "x");
    a.handle_with(&Event::Key(Key::Enter), &p);
    a.handle_with(&Event::Key(Key::Esc), &p);
    assert!(a.status_text().unwrap().contains("Search stopped"));
}

fn problem() -> CertProblem {
    CertProblem { station: "Hello FM".into(), url: "https://radio.example.org:8002/3".into(), host: "radio.example.org".into(), port: 8002, changed: false, sha256: "ab".repeat(32), subject: "radio.example.org".into(), issuer: "Some CA".into(), not_after: 4_000_000_000, trustable: true, message: "The certificate is not signed by an authority this computer knows.".into() }
}

#[test]
fn a_certificate_is_trusted_only_after_the_person_chooses_trust_and_cancel_is_where_the_dialog_starts() {
    let p = FakePort::new();
    p.mine.lock().unwrap().push(Station::new("Hello FM", "https://radio.example.org:8002/3"));
    let mut a = new(&p);
    *p.state.lock().unwrap() = RadioState::Failed { station: "Hello FM".into(), why: "certificate".into() };
    *p.problem.lock().unwrap() = Some(problem());
    key(&mut a, &p, 't');
    assert!(matches!(a.mode, Mode::Trust { focus: 0, .. }));
    // Enter on the starting button is a no
    a.handle_with(&Event::Key(Key::Enter), &p);
    assert!(matches!(a.mode, Mode::Browse));
    assert!(!p.calls().iter().any(|c| c.starts_with("trust ")), "{:?}", p.calls());
    assert_eq!(a.status_text(), Some("Nothing was trusted."));
    // the other way: move to Trust, confirm
    key(&mut a, &p, 't');
    a.handle_with(&Event::Key(Key::Right), &p);
    a.handle_with(&Event::Key(Key::Enter), &p);
    assert!(p.calls().contains(&"trust Hello FM radio.example.org".to_string()), "{:?}", p.calls());
    assert!(p.calls().iter().any(|c| c.starts_with("audit radio.trust radio.example.org sha256 ")), "{:?}", p.calls());
    assert!(p.calls().iter().any(|c| c.starts_with("play Hello FM ")), "the station starts again after the trust");
    // a station whose certificate cannot work is shown and never trusted
    let mut bad = problem();
    bad.trustable = false;
    *p.state.lock().unwrap() = RadioState::Failed { station: "Hello FM".into(), why: "certificate".into() };
    *p.problem.lock().unwrap() = Some(bad);
    key(&mut a, &p, 't');
    assert!(matches!(a.mode, Mode::Browse), "no dialog for a certificate that trusting cannot fix");
}

#[test]
fn the_tile_draws_in_every_state_at_the_usual_sizes_from_what_the_port_says() {
    use iyakki_uikit::gfx::Gfx;
    let p = FakePort::new();
    p.mine.lock().unwrap().push(Station::new("One", "https://a.example.org/1"));
    let mut a = new(&p);
    let states = [RadioState::Idle, RadioState::Connecting("One".into()), RadioState::Playing { station: "One".into(), title: "A song".into(), kbps: 128 }, RadioState::Failed { station: "One".into(), why: "needs the audio add-on".into() }];
    for st in states {
        *p.state.lock().unwrap() = st.clone();
        *p.active.lock().unwrap() = !matches!(st, RadioState::Idle);
        for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080)] {
            let mut g = Gfx::new(w, h);
            a.render_with(&mut g, R::new(0.0, 0.0, w as f32, h as f32), 1.0, &p, 3);
        }
    }
    // with no add-on in place a failed station that needs it offers the installer
    *p.addon.lock().unwrap() = false;
    let mut g = Gfx::new(1280, 720);
    a.render_with(&mut g, R::new(0.0, 0.0, 1280.0, 720.0), 1.0, &p, 4);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(310)), "the Install audio add-on button is there");
}

fn tales() -> Holder {
    Holder { says: "TALES is reading".into(), own_key: Some("tales".into()), outside: None }
}

#[test]
fn a_player_in_the_way_is_asked_about_first_keep_it_starts_nothing_and_the_audit_names_only_the_kind() {
    let p = FakePort::new();
    *p.gate.holder.lock().unwrap() = Some(tales());
    let mut a = new(&p);
    a.handle_with(&Event::Key(Key::Right), &p);
    a.handle_with(&Event::Key(Key::Enter), &p);
    assert!(matches!(&a.mode, Mode::Hold(_, ask) if ask.question() == "TALES is reading. Stop it so the radio can play?"));
    assert!(!p.calls().iter().any(|c| c.starts_with("play ")), "nothing starts before the answer");
    key(&mut a, &p, 'n');
    assert!(matches!(a.mode, Mode::Browse));
    assert!(a.status_text().unwrap().contains("TALES is reading") && a.status_text().unwrap().contains("not started"));
    assert!(!p.calls().iter().any(|c| c.starts_with("play ")));
    assert!(p.calls().contains(&"audit radio.sound.ask own:tales asked".to_string()), "{:?}", p.calls());
    assert!(p.calls().contains(&"audit radio.sound.keep own:tales kept".to_string()), "{:?}", p.calls());
    assert!(p.gate.stops.lock().unwrap().is_empty());
}

#[test]
fn stop_it_asks_the_port_to_stop_the_holder_then_the_station_starts() {
    let p = FakePort::new();
    *p.gate.holder.lock().unwrap() = Some(tales());
    let mut a = new(&p);
    a.handle_with(&Event::Key(Key::Right), &p);
    a.handle_with(&Event::Key(Key::Enter), &p);
    key(&mut a, &p, 'y');
    assert_eq!(*p.gate.stops.lock().unwrap(), vec!["TALES is reading".to_string()]);
    // the answer is already on the channel: the next tick ends the question and starts the station
    a.handle_with(&Event::Tick, &p);
    assert!(matches!(a.mode, Mode::Browse));
    assert!(p.calls().iter().any(|c| c.starts_with("play ")), "{:?}", p.calls());
    assert!(p.calls().contains(&"audit radio.sound.stopped own:tales Stopped.".to_string()), "{:?}", p.calls());
}

#[test]
fn a_stop_that_fails_says_why_and_starts_nothing() {
    let p = FakePort::new();
    *p.gate.holder.lock().unwrap() = Some(Holder { says: "mpv is using the sound card".into(), own_key: None, outside: Some((4242, "mpv".into())) });
    *p.gate.stop_answer.lock().unwrap() = Err("it did not let go".into());
    let mut a = new(&p);
    a.handle_with(&Event::Key(Key::Right), &p);
    a.handle_with(&Event::Key(Key::Enter), &p);
    key(&mut a, &p, 'y');
    a.handle_with(&Event::Tick, &p);
    assert!(matches!(a.mode, Mode::Browse));
    assert!(!p.calls().iter().any(|c| c.starts_with("play ")));
    assert!(a.status_text().unwrap().contains("it did not let go"));
    assert!(p.calls().contains(&"audit radio.sound.stopfailed outside:mpv it did not let go".to_string()), "{:?}", p.calls());
}
