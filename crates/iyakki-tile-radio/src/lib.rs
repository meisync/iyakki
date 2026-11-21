//! RADIO: internet radio in a player of its own. Stations come from the person's own list or from ten language
//! tabs (Tamil, Telugu, Malayalam, English, Hindi and more); one key starts a station and the music goes on in the background
//! while other apps are used (the launcher shows what plays). MP3 streams over HTTP or HTTPS and HLS with MP3 play by
//! themselves; AAC and HLS with AAC need the audio add-on, which the Install button below a failed station fetches from
//! its vendor (the Store's installer) and then tries the station again. The sound goes to the
//! system's play command (Settings → Voice), and every address is checked against the account's host policy.

use iyakki_uikit::safeclamp::SafeClamp;
use std::sync::Arc;

use iyakki_tile_api::radio::{self, CertProblem, Holder, Language, RadioState, SoundGate, Station, BANDS};

use iyakki_tile_api::radio::RadioPort;
use iyakki_tile_api::{App, Flow, TileCx};
use iyakki_uikit::blink::blink;
use iyakki_uikit::gfx::{rgb, Face, Gfx, Icon};
use iyakki_uikit::ui::{Event, Key, CARD, CARD_HOVER, CARD_SEL, DIM, ERR, MUTED, OK, R, TEXT};
use iyakki_uikit::widgets::{Form, FormEvent};

use iyakki_uikit::neon::{glow_card, mix, with_alpha, CYAN, MAGENTA, PANEL};

/// Tab 0 is the person's own stations; tabs 1 to 10 are the languages.
/// The speaker question on the screen: who holds it, and the stop while it runs on the port's thread.
pub struct HoldAsk {
    pub holder: Holder,
    pub working: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
}

impl HoldAsk {
    pub fn question(&self) -> String {
        self.holder.question("the radio")
    }
    pub fn stopping(&self) -> bool {
        self.working.is_some()
    }
    /// `Some` once the stop has ended.
    pub fn poll(&mut self) -> Option<Result<String, String>> {
        let r = self.working.as_ref()?.try_recv();
        match r {
            Ok(v) => {
                self.working = None;
                Some(v)
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(_) => {
                self.working = None;
                Some(Err("the stop did not finish".into()))
            }
        }
    }
}

pub enum Mode {
    Browse,
    Form(Form),
    ConfirmDelete(String),
    /// A station waits for the answer to "Browser is playing. Stop it so the radio can play?" (soundask.rs).
    Hold(Station, HoldAsk),
    /// The person is asked whether to trust a station's server certificate. `focus` 0 is Cancel (where the dialog starts), 1 is Trust.
    /// Nothing is stored unless the person chooses Trust.
    Trust { problem: CertProblem, focus: usize },
    /// The search box is open: the form for the words.
    Find(Form),
    /// A search is running on its own thread (the internet directory can be slow); Esc gives it up.
    Searching { words: String, rx: std::sync::mpsc::Receiver<(Vec<(String, String)>, Option<String>)> },
}

/// What the last search found; shown as one more entry after the languages.
pub struct Found {
    pub stations: Vec<Station>,
}

pub struct RadioApp {
    pub mine: Vec<Station>,
    pub tab: usize,
    pub sel: usize,
    pub mode: Mode,
    pub status: Option<(String, bool)>,
    /// What the level display shows: the real levels with a slower fall and a held peak.
    pub peaks: [f32; BANDS],
    /// The first station shown on the right (the wheel moves it; the chosen station follows it only when keys move the choice).
    pub top: usize,
    /// How many station rows fit (set when drawn).
    pub fit: usize,
    /// The choice the list last made room for: a click selects without moving the list, a key that moves the choice scrolls to it.
    pub seen_sel: usize,
    /// The first row shown in the language list on the left.
    pub nav_top: usize,
    /// The pointer is over the language list (the wheel scrolls it instead of the stations).
    pub nav_hover: bool,
    /// The station that could not start for lack of the audio add-on; it is tried again once the add-on is installed.
    pub retry: Option<Station>,
    /// The person pressed "Install audio add-on": the shell opens the installer (`take_request`).
    pub install: bool,
    /// The preset stations by language (from the port).
    pub langs: &'static [Language],
    /// Who holds the speaker, and stopping them (from the port; tests put their own here).
    pub sound: Arc<dyn SoundGate>,
    /// The last search (the entry after the languages while there is one).
    pub found: Option<Found>,
}



impl RadioApp {
    pub fn new(p: &dyn RadioPort) -> RadioApp {
        let mine = p.mine();
        let langs = p.languages();
        RadioApp { langs, tab: if mine.is_empty() { 1.min(langs.len()) } else { 0 }, mine, sel: 0, top: 0, fit: 6, seen_sel: 0, mode: Mode::Browse, status: None, peaks: [0.0; BANDS], nav_top: 0, nav_hover: false, retry: None, install: false, sound: p.sound(), found: None }
    }

    /// The dialog about the speaker is open: its answers, and the end of the stop.
    pub fn hold_event(&mut self, ev: &Event, p: &dyn RadioPort) {
        use iyakki_uikit::askdialog::{answer_for_key, Answer};
        let Mode::Hold(st, ask) = &mut self.mode else { return };
        if let Event::Key(k) = ev {
            if let (Some(a), false) = (answer_for_key(k), ask.stopping()) {
                let found = ask.holder.clone();
                match a {
                    Answer::Stop => {
                        hold_audit(p, "radio.sound.stop", &found, "asked");
                        ask.working = Some(self.sound.stop(&ask.holder, 3));
                    }
                    Answer::Keep => {
                        hold_audit(p, "radio.sound.keep", &found, "kept");
                        self.status = Some((format!("{} The radio was not started.", found.says), false));
                        self.mode = Mode::Browse;
                    }
                    Answer::Cancel => {
                        hold_audit(p, "radio.sound.cancel", &found, "cancelled");
                        self.mode = Mode::Browse;
                    }
                }
                return;
            }
        }
        if let Some(r) = ask.poll() {
            let (found, station) = (ask.holder.clone(), st.clone());
            self.mode = Mode::Browse;
            match r {
                Ok(m) => {
                    hold_audit(p, "radio.sound.stopped", &found, &m);
                    self.start_station(&station, p);
                }
                Err(e) => {
                    hold_audit(p, "radio.sound.stopfailed", &found, &e);
                    self.status = Some((format!("{} {e}", found.says), false));
                }
            }
        }
    }

    /// The stations on the current tab.
    pub fn list(&self) -> Vec<Station> {
        if self.tab == 0 {
            self.mine.clone()
        } else if self.tab >= self.base_tabs() {
            self.found.as_ref().map(|f| f.stations.clone()).unwrap_or_default()
        } else {
            self.langs[self.tab - 1].stations.iter().map(|(n, u)| Station::new(n, u)).collect()
        }
    }

    pub fn tab_name(&self) -> String {
        if self.tab == 0 {
            "My stations".into()
        } else if self.tab >= self.base_tabs() {
            "Search results".into()
        } else {
            self.langs[self.tab - 1].name.into()
        }
    }

    pub fn station_names(&self) -> Vec<String> {
        self.list().into_iter().map(|s| s.name).collect()
    }

    pub fn status_text(&self) -> Option<&str> {
        self.status.as_ref().map(|s| s.0.as_str())
    }

    pub fn play_station(&mut self, st: &Station, p: &dyn RadioPort) {
        let host = radio::host_of(&st.url);
        if !p.allows_host(&host) {
            self.status = Some((format!("Your account may not reach {host}."), false));
            return;
        }
        // another player of IYAKKI holds the speaker and would lock this one out (or the browser's page plays): ask first
        if let Some(found) = self.sound.conflict() {
            hold_audit(p, "radio.sound.ask", &found, "asked");
            self.mode = Mode::Hold(st.clone(), HoldAsk { holder: found, working: None });
            return;
        }
        self.start_station(st, p);
    }

    /// The station as it is kept in My stations: with the certificate its owner approved, when there is one.
    pub fn with_pin(&self, st: &Station) -> Station {
        match self.mine.iter().find(|m| m.name == st.name && m.url == st.url) {
            Some(m) => Station { cert_pin: m.cert_pin.clone(), ..st.clone() },
            None => st.clone(),
        }
    }

    /// The station a certificate question is about (kept ones first, then the presets).
    pub fn station_named(&self, name: &str) -> Option<Station> {
        self.mine.iter().cloned().chain(self.langs.iter().flat_map(|l| l.stations.iter().map(|(n, u)| Station::new(n, u)))).find(|s| s.name == name)
    }

    /// The certificate question of the station that failed, if that is why it did.
    pub fn open_trust(&mut self, p: &dyn RadioPort) {
        if let (RadioState::Failed { station, .. }, Some(problem)) = (p.state(), p.cert_problem()) {
            if problem.station == station && problem.trustable {
                self.mode = Mode::Trust { problem, focus: 0 };
            }
        }
    }

    /// The person chose Trust: the exact certificate is kept with the station, and the station starts again.
    pub fn confirm_trust(&mut self, problem: &CertProblem, p: &dyn RadioPort) {
        self.mode = Mode::Browse;
        let Some(base) = self.station_named(&problem.station) else {
            self.status = Some(("That station is not in your lists any more.".into(), false));
            return;
        };
        match p.trust(&base, problem) {
            Ok(st) => {
                p.audit("radio.trust", &format!("{} sha256 {}", problem.host, radio::short_fingerprint(&problem.sha256)));
                self.reload(p);
                self.start_station(&st, p);
                self.status = Some((format!("Trusting the certificate of {} for {} only. It is asked about again when the certificate is renewed.", problem.host, st.name), true));
            }
            Err(e) => self.status = Some((e, false)),
        }
    }

    /// Forget the approved certificate of the chosen station.
    pub fn forget_trust(&mut self, p: &dyn RadioPort) {
        let Some(st) = self.list().get(self.sel).cloned() else { return };
        let pinned = self.with_pin(&st).cert_pin;
        let Some(pin) = pinned else { return };
        match p.forget_trust(&st.name) {
            Ok(_) => {
                p.audit("radio.trust.revoked", &format!("{} sha256 {}", pin.host, radio::short_fingerprint(&pin.sha256)));
                self.reload(p);
                self.status = Some((format!("{} no longer trusts the certificate of {}.", st.name, pin.host), true));
            }
            Err(e) => self.status = Some((e, false)),
        }
    }

    pub fn trust_event(&mut self, ev: &Event, p: &dyn RadioPort) {
        let Mode::Trust { problem, focus } = &mut self.mode else { return };
        let answer = match ev {
            Event::Tab(321) | Event::Key(Key::Esc) => Some(false),
            Event::Tab(322) => Some(true),
            Event::Key(Key::Enter) => Some(*focus == 1),
            Event::Key(Key::Left) | Event::Key(Key::Right) | Event::Key(Key::Tab) | Event::Key(Key::Up) | Event::Key(Key::Down) => {
                *focus = 1 - *focus;
                None
            }
            _ => None,
        };
        match answer {
            Some(true) => {
                let problem = problem.clone();
                self.confirm_trust(&problem, p);
            }
            Some(false) => {
                self.mode = Mode::Browse;
                self.status = Some(("Nothing was trusted.".into(), true));
            }
            None => {}
        }
    }

    pub fn start_station(&mut self, st: &Station, p: &dyn RadioPort) {
        let st = &self.with_pin(st);
        let host = radio::host_of(&st.url);
        self.retry = None;
        p.audit("radio.play", &host);
        p.play(st);
        self.status = None;
    }

    pub fn play_selected(&mut self, p: &dyn RadioPort) {
        if let Some(st) = self.list().get(self.sel).cloned() {
            self.play_station(&st, p);
        }
    }

    /// Move to the previous (-1) or next (1) station on this tab and play it.
    pub fn step(&mut self, d: i32, p: &dyn RadioPort) {
        let n = self.list().len() as i32;
        if n == 0 {
            return;
        }
        self.sel = (self.sel as i32 + d).rem_euclid(n) as usize;
        self.play_selected(p);
    }

    pub fn find_form() -> Form {
        Form::new("Find a station").text("Search", "", "a name, a language or a kind of music: tamil, jazz, BBC")
    }

    /// Start looking for what the form holds, on a thread of its own: the internet directory can take seconds.
    pub fn start_search(&mut self, words: &str, p: &dyn RadioPort) {
        let words = words.trim().to_string();
        if words.is_empty() {
            self.status = Some(("Type a name, a language or a kind of music to search for.".into(), false));
            self.mode = Mode::Browse;
            return;
        }
        p.audit("radio.search", "stations");
        let rx = p.search(&words);
        self.mode = Mode::Searching { words, rx };
    }

    /// The search finished: its stations become the last entry of the list and are chosen.
    pub fn poll_search(&mut self) {
        let Mode::Searching { words, rx } = &self.mode else { return };
        let (found, note) = match rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => (Vec::new(), Some("The search stopped without an answer.".to_string())),
        };
        let words = words.clone();
        self.mode = Mode::Browse;
        let n = found.len();
        let mut msg = if n == 0 { format!("No station matches \u{201c}{words}\u{201d}.") } else { format!("{n} station{} for \u{201c}{words}\u{201d}: Enter plays one, a keeps it in My stations.", if n == 1 { "" } else { "s" }) };
        if let Some(note) = note {
            msg.push(' ');
            msg.push_str(&note);
        }
        let ok = n > 0;
        self.found = Some(Found { stations: found.into_iter().map(|(n, u)| Station::new(&n, &u)).collect() });
        self.set_tab(self.base_tabs());
        self.status = Some((msg, ok));
    }

    pub fn new_form() -> Form {
        Form::new("Add your own station").text("Name", "", "e.g. Jazz FM").text("Stream address", "", "https://stream.example.org/live.mp3")
    }

    pub fn reload(&mut self, p: &dyn RadioPort) {
        self.mine = p.mine();
        self.sel = self.sel.min(self.list().len().saturating_sub(1));
    }

    /// The entries after My stations: the languages, then the search results while there are some.
    pub fn tabs(&self) -> usize {
        self.base_tabs() + usize::from(self.found.is_some())
    }

    /// My stations and the languages (the search results come after them).
    pub fn base_tabs(&self) -> usize {
        1 + self.langs.len()
    }

    pub fn set_tab(&mut self, t: usize) {
        self.tab = t.min(self.tabs() - 1);
        self.sel = 0;
        self.top = 0;
        self.seen_sel = 0;
    }

    /// Keep the chosen preset in My stations.
    pub fn keep_selected(&mut self, p: &dyn RadioPort) {
        if self.tab == 0 {
            self.mode = Mode::Form(Self::new_form());
            return;
        }
        let Some(st) = self.list().get(self.sel).cloned() else { return };
        match p.add(&st.name, &st.url) {
            Ok(st) => {
                p.audit("radio.add", &radio::host_of(&st.url));
                self.reload(p);
                self.status = Some((format!("{} is in My stations.", st.name), true));
            }
            Err(e) => self.status = Some((e, false)),
        }
    }
}

impl App for RadioApp {
    fn title(&self) -> &'static str {
        "Radio"
    }
    fn icon(&self) -> Icon {
        Icon::Radio
    }
    fn accent(&self) -> [u8; 4] {
        CYAN
    }
    fn hint(&self) -> String {
        "← → language · ↑↓ station · Enter play · P pause · Space stop · [ ] prev/next · + − volume".into()
    }

    fn help(&self) -> Vec<String> {
        [
            "Left / Right · My stations, then Tamil, Telugu, Malayalam, English, Hindi, Spanish, French, Arabic, Portuguese, Russian, Chinese, Bengali",
            "Up / Down · choose a station      Enter · play it",
            "Space or s · stop (Space starts the chosen station when nothing plays)",
            "[ and ] · the previous and next station, played at once",
            "+ and − (or = and -) · volume; click the bars under the dial too",
            "a · keep the chosen station in My stations (in My stations: add your own)",
            "n · add your own station: a name and an MP3 stream address",
            "/ · search for a station by name, language or kind of music (the same search the chat's radio_search runs); the results are the last entry of the list",
            "t · when a station fails on its server's certificate: review it and decide whether to trust it (asks first)",
            "f · forget the certificate you trusted for the chosen station",
            "d · delete the chosen station from My stations (asks first)",
            "Esc · back to the apps; the music keeps playing in the background",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn media_key(&mut self, k: iyakki_uikit::keys::FnKey, cx: &TileCx) -> bool {
        self.media_key_with(k, cx.radio)
    }

    fn handle(&mut self, ev: &Event, cx: &TileCx) -> Flow {
        self.handle_with(ev, cx.radio)
    }

    fn take_request(&mut self) -> Option<(String, String)> {
        // the Store's Setup page, on the media tools (the same installer as the MEDIA player's)
        std::mem::take(&mut self.install).then(|| ("quickinstall".to_string(), "ffmpeg".to_string()))
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &TileCx, tick: u64) {
        self.render_with(g, area, s, cx.radio, tick)
    }
}

/// The question before a server certificate is trusted: everything the person can compare, a plain warning, Cancel first.
fn render_trust(g: &mut Gfx, area: R, s: f32, c: &CertProblem, focus: usize) {
    let p = iyakki_uikit::fx::modal(g, area, s, 680.0 * s, 470.0 * s, ERR, true);
    let title = if c.changed { "The certificate changed. Trust the new one?" } else { "Trust this station's certificate?" };
    let mut y = iyakki_uikit::fx::modal_title(g, p, s, title, ERR) + 6.0 * s;
    let x = p.x + 26.0 * s;
    let iw = p.w - 52.0 * s;
    let row = |g: &mut Gfx, label: &str, value: &str, y: &mut f32| {
        g.text(x, *y, 12.0 * s, DIM, label, Face::Bold);
        let v = g.fit(15.0 * s, value, Face::Sans, iw - 120.0 * s);
        g.text(x + 120.0 * s, *y - 2.0 * s, 15.0 * s, TEXT, &v, Face::Sans);
        *y += 25.0 * s;
    };
    row(g, "STATION", &c.station, &mut y);
    row(g, "SERVER", &format!("{}:{}", c.host, c.port), &mut y);
    row(g, "CERTIFICATE FOR", &c.subject, &mut y);
    row(g, "ISSUED BY", &c.issuer, &mut y);
    row(g, "VALID UNTIL", &c.expiry(), &mut y);
    g.text(x, y, 12.0 * s, DIM, "SHA-256", Face::Bold);
    let groups: Vec<String> = radio::fingerprint_groups(&c.sha256).split(' ').map(str::to_string).collect();
    for half in groups.chunks(4) {
        g.text(x + 120.0 * s, y - 2.0 * s, 15.0 * s, TEXT, &half.join("  "), Face::Mono);
        y += 22.0 * s;
    }
    y += 8.0 * s;
    let warn = if c.changed {
        "This station showed a different certificate than the one you trusted. A renewed certificate looks like this, and so does someone pretending to be the station. Trust the new one only if you know the station."
    } else {
        "Only trust this if you know the station; someone on your network could pretend to be it. IYAKKI will then accept exactly this certificate for this station's server, nothing else, and ask again when it is renewed."
    };
    for l in g.wrap(14.0 * s, warn, Face::Sans, iw).iter().take(4) {
        g.text(x, y, 14.0 * s, ERR, l, Face::Sans);
        y += 20.0 * s;
    }
    let by = p.y + p.h - 58.0 * s;
    let w1 = g.button(x, by, 14.0 * s, "Cancel", MUTED, Event::Tab(321));
    let w2 = g.button(x + w1 + 14.0 * s, by, 14.0 * s, "Trust this certificate", ERR, Event::Tab(322));
    let (fx, fw) = if focus == 0 { (x, w1) } else { (x + w1 + 14.0 * s, w2) };
    g.rrect_outline(fx - 3.0 * s, by - 3.0 * s, fw + 6.0 * s, 14.0 * s * 2.1 + 6.0 * s, 16.0 * s, 2.0 * s, TEXT);
    g.text(x + w1 + w2 + 34.0 * s, by + 8.0 * s, 12.0 * s, DIM, "← → choose · Enter · Esc cancels", Face::Sans);
}

impl RadioApp {
    pub fn media_key_with(&mut self, k: iyakki_uikit::keys::FnKey, p: &dyn RadioPort) -> bool {
        use iyakki_uikit::keys::FnKey;
        if !matches!(self.mode, Mode::Browse) {
            return false;
        }
        match k {
            FnKey::PlayPause => {
                if !p.resume() && !p.pause() {
                    self.play_selected(p);
                }
            }
            FnKey::Next => self.step(1, p),
            FnKey::Prev => self.step(-1, p),
            FnKey::Stop => p.stop(),
            _ => return false,
        }
        true
    }

    pub fn handle_with(&mut self, ev: &Event, p: &dyn RadioPort) -> Flow {
        self.poll_search();
        if let Mode::Searching { .. } = &self.mode {
            // Esc gives the search up; nothing else is taken while it runs
            if matches!(ev, Event::Key(Key::Esc)) {
                self.mode = Mode::Browse;
                self.status = Some(("Search stopped.".into(), false));
            }
            return Flow::Stay;
        }
        if let Mode::Find(form) = &mut self.mode {
            match form.handle_event(ev) {
                FormEvent::Cancel => self.mode = Mode::Browse,
                FormEvent::Submit => {
                    let words = form.value(0);
                    self.start_search(&words, p);
                }
                _ => {}
            }
            return Flow::Stay;
        }
        if let Mode::Hold(..) = &self.mode {
            self.hold_event(ev, p);
            return Flow::Stay;
        }
        if let Mode::Trust { .. } = &self.mode {
            self.trust_event(ev, p);
            return Flow::Stay;
        }
        if let Mode::Form(form) = &mut self.mode {
            match form.handle_event(ev) {
                FormEvent::Cancel => self.mode = Mode::Browse,
                FormEvent::Submit => {
                    let (name, url) = (form.value(0), form.value(1));
                    match p.add(&name, &url) {
                        Ok(st) => {
                            p.audit("radio.add", &radio::host_of(&st.url));
                            self.mode = Mode::Browse;
                            self.reload(p);
                            self.tab = 0;
                            self.sel = self.mine.iter().position(|s| s.name == st.name).unwrap_or(0);
                            self.status = Some((format!("Added {}. Press Enter to play it.", st.name), true));
                        }
                        Err(e) => {
                            if let Mode::Form(f) = &mut self.mode {
                                f.error = Some(e);
                            }
                        }
                    }
                }
                _ => {}
            }
            return Flow::Stay;
        }
        match ev {
            Event::Row(i) => {
                // a click chooses the row where it is: the list stays put
                self.sel = (*i).min(self.list().len().saturating_sub(1));
                self.seen_sel = self.sel;
                return Flow::Stay;
            }
            Event::Scroll(d) if self.nav_hover => {
                self.nav_top = (self.nav_top as i32 - d).max(0) as usize;
                return Flow::Stay;
            }
            Event::Scroll(d) => {
                // the wheel moves the list, not the choice
                let most = self.list().len().saturating_sub(self.fit.max(1)) as i32;
                self.top = (self.top as i32 - d * 3).clamp_safe(0, most.max(0)) as usize;
                return Flow::Stay;
            }
            // the tabs, the transport buttons and the volume bars
            Event::Tab(n) if *n < self.tabs() => {
                self.set_tab(*n);
                return Flow::Stay;
            }
            Event::Tab(100) => {
                self.step(-1, p);
                return Flow::Stay;
            }
            Event::Tab(101) => {
                if p.is_active() {
                    p.stop();
                } else {
                    self.play_selected(p);
                }
                return Flow::Stay;
            }
            Event::Tab(102) => {
                self.step(1, p);
                return Flow::Stay;
            }
            Event::Tab(n) if (200..=220).contains(n) => {
                p.set_volume(((n - 200) * 5) as u8);
                return Flow::Stay;
            }
            // "Install audio add-on" under a station that needs it
            Event::Tab(310) => {
                if let RadioState::Failed { station, why } = p.state() {
                    if p.failure_needs_addon(&why) {
                        self.retry = self.list().into_iter().chain(self.mine.clone()).find(|s| s.name == station);
                        self.install = true;
                    }
                }
                return Flow::Stay;
            }
            // "Review the certificate" under a station that failed on its server's certificate, and "Forget trust"
            Event::Tab(320) => {
                self.open_trust(p);
                return Flow::Stay;
            }
            Event::Tab(323) => {
                self.forget_trust(p);
                return Flow::Stay;
            }
            // the Search button
            Event::Tab(330) => {
                self.mode = Mode::Find(Self::find_form());
                return Flow::Stay;
            }
            Event::Tab(300) => {
                self.keep_selected(p);
                return Flow::Stay;
            }
            // the Remove button on a station of My stations
            Event::Tab(301) => {
                if self.tab == 0 {
                    if let Some(s) = self.mine.get(self.sel) {
                        self.mode = Mode::ConfirmDelete(s.name.clone());
                    }
                }
                return Flow::Stay;
            }
            _ => {}
        }
        let Event::Key(k) = ev else { return Flow::Stay };
        if let Mode::ConfirmDelete(name) = &self.mode {
            let name = name.clone();
            if *k == Key::Char('y') {
                let host = self.mine.iter().find(|s| s.name == name).map(|s| radio::host_of(&s.url)).unwrap_or_default();
                match p.remove(&name) {
                    Ok(()) => {
                        p.audit("radio.remove", &host);
                        self.status = Some((format!("Removed {name}."), true));
                    }
                    Err(e) => self.status = Some((e, false)),
                }
                self.reload(p);
            }
            self.mode = Mode::Browse;
            return Flow::Stay;
        }
        let n = self.list().len();
        match k {
            Key::Esc => return Flow::Back,
            Key::Left => self.set_tab((self.tab + self.tabs() - 1) % self.tabs()),
            Key::Right => self.set_tab((self.tab + 1) % self.tabs()),
            Key::Up => self.sel = self.sel.saturating_sub(1),
            Key::Down => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            Key::Enter => self.play_selected(p),
            Key::Char(' ') => {
                if p.is_active() {
                    p.stop();
                } else {
                    self.play_selected(p);
                }
            }
            Key::Char('s') => p.stop(),
            // pause and carry on (a stopped station starts again from the beginning)
            Key::Char('p') => {
                if !p.resume() {
                    p.pause();
                }
            }
            Key::Char('[') => self.step(-1, p),
            Key::Char(']') => self.step(1, p),
            Key::Char('+') | Key::Char('=') => p.set_volume(p.volume().saturating_add(5)),
            Key::Char('-') | Key::Char('_') => p.set_volume(p.volume().saturating_sub(5)),
            Key::Char('a') => self.keep_selected(p),
            Key::Char('t') => self.open_trust(p),
            Key::Char('f') => self.forget_trust(p),
            Key::Char('n') => self.mode = Mode::Form(Self::new_form()),
            Key::Char('/') => self.mode = Mode::Find(Self::find_form()),
            Key::Char('d') if self.tab == 0 => {
                if let Some(s) = self.mine.get(self.sel) {
                    self.mode = Mode::ConfirmDelete(s.name.clone());
                }
            }
            _ => {}
        }
        Flow::Stay
    }

    pub fn render_with(&mut self, g: &mut Gfx, area: R, s: f32, p: &dyn RadioPort, tick: u64) {
        // back from the installer with the add-on in place: the station that waited is tried again
        if self.retry.is_some() && p.addon_installed() {
            if let Some(st) = self.retry.take() {
                self.play_station(&st, p);
            }
        }
        self.poll_search();
        let addon_missing = !p.addon_installed();
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let mut y = area.y + 10.0 * s;
        let state = p.state();
        let playing = matches!(state, RadioState::Playing { .. });
        let active = p.is_active();
        // the display follows the real levels: up at once, down slowly
        let levels = p.levels();
        for (p, l) in self.peaks.iter_mut().zip(levels) {
            *p = if l > *p { l } else { (*p - 0.035).max(l) };
        }
        let energy = levels.iter().sum::<f32>() / BANDS as f32;

        // ---- the languages: a list down the left side, or a row of tabs when the window is narrow
        let mut names: Vec<String> = std::iter::once("My stations".to_string()).chain(self.langs.iter().map(|l| l.name.to_string())).collect();
        let mut counts: Vec<usize> = std::iter::once(self.mine.len()).chain(self.langs.iter().map(|l| l.stations.len())).collect();
        if let Some(f) = &self.found {
            names.push("Search results".to_string());
            counts.push(f.stations.len());
        }
        let ntabs = self.tabs();
        let nav_w = if area.w >= 760.0 * s { (230.0 * s).min(area.w * 0.26) } else { 0.0 };
        self.nav_hover = false;
        let (x, w) = if nav_w > 0.0 {
            let nav = R::new(area.x + 12.0 * s, area.y + 8.0 * s, nav_w, area.h - 16.0 * s);
            self.nav_hover = g.hovered(nav);
            g.rrect(nav.x, nav.y, nav.w, nav.h, 14.0 * s, PANEL);
            g.text(nav.x + 16.0 * s, nav.y + 12.0 * s, 12.0 * s, DIM, "LANGUAGES", Face::Bold);
            // adding a station of your own (a name and a stream address) is always one click away
            g.button(nav.x + 10.0 * s, nav.y + nav.h - 52.0 * s, 14.0 * s, "+ Add station (n)", CYAN, Event::Key(Key::Char('n')));
            let rh = 36.0 * s;
            let slots = (((nav.h - 40.0 * s - 56.0 * s) / rh).floor() as usize).max(1);
            if self.tab < self.nav_top {
                self.nav_top = self.tab;
            } else if self.tab >= self.nav_top + slots && !self.nav_hover {
                self.nav_top = self.tab + 1 - slots;
            }
            self.nav_top = self.nav_top.min(ntabs.saturating_sub(slots));
            for (k, i) in (self.nav_top..ntabs.min(self.nav_top + slots)).enumerate() {
                let r = R::new(nav.x + 8.0 * s, nav.y + 34.0 * s + k as f32 * rh, nav.w - 16.0 * s, rh - 3.0 * s);
                let on = i == self.tab;
                g.rrect(r.x, r.y, r.w, r.h, 10.0 * s, if on { with_alpha(CYAN, 46) } else if g.hovered(r) { CARD_HOVER } else { with_alpha(PANEL, 0) });
                if on {
                    g.rrect_outline(r.x, r.y, r.w, r.h, 10.0 * s, 1.3 * s, CYAN);
                }
                let cnt = counts[i].to_string();
                let cw = g.text_width(12.0 * s, &cnt, Face::Sans);
                let nm = g.fit(14.0 * s, &names[i], Face::Bold, r.w - cw - 36.0 * s);
                g.text(r.x + 12.0 * s, r.y + 9.0 * s, 14.0 * s, if on { CYAN } else { TEXT }, &nm, Face::Bold);
                g.text(r.x + r.w - cw - 12.0 * s, r.y + 11.0 * s, 12.0 * s, if on { CYAN } else { DIM }, &cnt, Face::Sans);
                g.hit(r, Event::Tab(i), &names[i]);
            }
            if ntabs > slots {
                g.text(nav.x + 16.0 * s, nav.y + nav.h - 74.0 * s, 11.0 * s, DIM, &format!("{}–{} of {}", self.nav_top + 1, (self.nav_top + slots).min(ntabs), ntabs), Face::Sans);
            }
            (area.x + 12.0 * s + nav_w + 18.0 * s, area.w - nav_w - 12.0 * s - 18.0 * s - 12.0 * s)
        } else {
            let widths: Vec<f32> = names.iter().map(|n| g.text_width(14.0 * s, n, Face::Bold) + 28.0 * s).collect();
            // start far enough along that the chosen tab is visible
            let mut first = 0;
            while first < self.tab && widths[first..=self.tab].iter().sum::<f32>() + 8.0 * s * (self.tab - first) as f32 > w - 40.0 * s {
                first += 1;
            }
            let mut tx = x;
            if first > 0 {
                g.text(tx, y + 8.0 * s, 16.0 * s, DIM, "‹", Face::Bold);
                tx += 18.0 * s;
            }
            for i in first..ntabs {
                if tx + widths[i] > x + w - 14.0 * s {
                    g.text(x + w - 12.0 * s, y + 8.0 * s, 16.0 * s, DIM, "›", Face::Bold);
                    break;
                }
                let r = R::new(tx, y, widths[i], 32.0 * s);
                let on = i == self.tab;
                g.rrect(r.x, r.y, r.w, r.h, 16.0 * s, if on { with_alpha(CYAN, 46) } else if g.hovered(r) { CARD_HOVER } else { rgb(18, 24, 46) });
                if on {
                    g.rrect_outline(r.x, r.y, r.w, r.h, 16.0 * s, 1.5 * s, CYAN);
                }
                g.text(r.x + 14.0 * s, r.y + 7.0 * s, 14.0 * s, if on { CYAN } else { MUTED }, &names[i], Face::Bold);
                g.hit(r, Event::Tab(i), &names[i]);
                tx += widths[i] + 8.0 * s;
            }
            y += 46.0 * s;
            (x, w)
        };

        // ---- the player
        let ch = (area.y + area.h - y - 250.0 * s).clamp_safe(286.0 * s, 310.0 * s);
        // a narrow card (a phone, or large text): the transport buttons leave no room for the volume bars beside them, so the bars get a row
        // of their own under the buttons and the card grows by that row (the level display below keeps clear of it)
        let narrow = x + w - 26.0 * s - (x + 24.0 * s + 2.0 * (78.0 * s) + 12.0 * s + 34.0 * s + 3.0 * 46.0 * s + 10.0 * s + 36.0 * s + 18.0 * s) < 130.0 * s;
        let extra = if narrow { 60.0 * s } else { 0.0 };
        let ch = ch + extra;
        let card = R::new(x, y, w, ch);
        let glow = if playing { 0.35 + 0.65 * energy.min(1.0) } else { 0.15 };
        glow_card(g, card, s, CYAN, MAGENTA, glow);
        // the tuning dial
        let dr = (card.h * 0.30).min(78.0 * s);
        let (dcx, dcy) = (card.x + 24.0 * s + dr + 12.0 * s, card.y + 24.0 * s + dr + 10.0 * s);
        g.circle(dcx, dcy, dr + 10.0 * s * energy.min(1.0) + 6.0 * s, with_alpha(CYAN, (20.0 + 30.0 * energy.min(1.0)) as u8));
        g.circle(dcx, dcy, dr, rgb(12, 17, 38));
        g.ring(dcx, dcy, dr, 2.0 * s, with_alpha(CYAN, 150));
        let spin = if active { tick as f32 * 0.025 } else { 0.0 };
        for k in 0..36 {
            let a = spin + k as f32 * std::f32::consts::TAU / 36.0;
            let lit = ((a * 0.0 + k as f32) % 9.0) < 1.0 && active;
            let (r0, r1) = (dr - 12.0 * s, dr - 5.0 * s);
            g.line(dcx + a.cos() * r0, dcy + a.sin() * r0, dcx + a.cos() * r1, dcy + a.sin() * r1, 2.0 * s, if lit { MAGENTA } else { with_alpha(MUTED, 90) });
        }
        // the needle sweeps as the station changes
        let list = self.list();
        let frac = if list.len() > 1 { self.sel as f32 / (list.len() - 1) as f32 } else { 0.5 };
        let na = std::f32::consts::PI * (0.75 + 1.5 * frac);
        g.line(dcx, dcy, dcx + na.cos() * (dr - 20.0 * s), dcy + na.sin() * (dr - 20.0 * s), 3.0 * s, if active { MAGENTA } else { MUTED });
        g.circle(dcx, dcy, 8.0 * s, if active { CYAN } else { DIM });
        // what is playing
        let tx = dcx + dr + 34.0 * s;
        let tw = card.x + card.w - tx - 26.0 * s;
        let (head, sub, color) = match &state {
            RadioState::Idle => ("Nothing is playing".to_string(), format!("{} · press Enter on a station", self.tab_name()), MUTED),
            RadioState::Connecting(n) => (n.clone(), p.notice().map_or_else(|| "tuning in…".to_string(), |t| format!("{t}…")), CYAN),
            RadioState::Playing { station, title, kbps } => (station.clone(), if title.is_empty() { format!("on air{}{}", if *kbps > 0 { format!(" · {kbps} kbit/s") } else { String::new() }, p.notice().map_or(String::new(), |t| format!(" · {t}"))) } else { title.clone() }, OK),
            RadioState::Failed { station, why } => {
                let waiting = p.cert_problem().is_some_and(|c| &c.station == station);
                (format!("{station} did not play"), if waiting { "The server did not send its full certificate chain. Review its certificate below.".to_string() } else { why.clone() }, ERR)
            }
        };
        let state_word = if playing && p.is_paused() { "❚❚ PAUSED" } else if playing { "● ON AIR" } else if active { "◌ TUNING" } else { "○ STANDBY" };
        let state_word = g.fit(12.0 * s, state_word, Face::Bold, tw.max(30.0 * s));
        g.text(tx, card.y + 22.0 * s, 12.0 * s, if playing { MAGENTA } else { DIM }, &state_word, Face::Bold);
        let head = g.fit(28.0 * s, &head, Face::Bold, tw);
        g.text(tx, card.y + 42.0 * s, 28.0 * s, TEXT, &head, Face::Bold);
        // a long song title scrolls
        let sw = g.text_width(15.0 * s, &sub, Face::Sans);
        if sw > tw && playing {
            let gap = 60.0 * s;
            let off = (tick as f32 * 0.9 * s) % (sw + gap);
            let line = format!("{sub}      {sub}");
            let (lx, lw) = (tx, tw);
            let mut shown = String::new();
            // draw the visible part of the looping text by trimming characters off the front
            let mut skip = 0usize;
            for (i, _) in line.char_indices() {
                if g.text_width(15.0 * s, &line[..i], Face::Sans) >= off {
                    skip = i;
                    break;
                }
            }
            for (i, c) in line[skip..].char_indices() {
                shown.push(c);
                if g.text_width(15.0 * s, &shown, Face::Sans) > lw {
                    shown.truncate(i);
                    break;
                }
            }
            g.text(lx, card.y + 82.0 * s, 15.0 * s, color, &shown, Face::Sans);
        } else {
            let sub = g.fit(15.0 * s, &sub, Face::Sans, tw);
            g.text(tx, card.y + 82.0 * s, 15.0 * s, color, &sub, Face::Sans);
        }
        // transport
        let by = card.y + 118.0 * s;
        let bd = 46.0 * s;
        let mut bx = tx;
        for (i, label) in ["Previous", "Play or stop", "Next"].iter().enumerate() {
            let r = R::new(bx, by, if i == 1 { bd + 10.0 * s } else { bd }, if i == 1 { bd + 10.0 * s } else { bd });
            let (cx_, cy_) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
            let hot = g.hovered(r);
            g.circle(cx_, cy_, r.w / 2.0, if i == 1 { with_alpha(if active { MAGENTA } else { CYAN }, if hot { 90 } else { 55 }) } else if hot { CARD_HOVER } else { rgb(18, 24, 46) });
            g.ring(cx_, cy_, r.w / 2.0, 1.5 * s, if i == 1 { if active { MAGENTA } else { CYAN } } else { with_alpha(MUTED, 160) });
            let k = r.w * 0.17;
            let c = if i == 1 { TEXT } else { MUTED };
            match i {
                0 => {
                    g.polygon(&[(cx_ + k, cy_ - k), (cx_ + k, cy_ + k), (cx_ - k * 0.6, cy_)], c);
                    g.rect(cx_ - k * 1.1, cy_ - k, 2.5 * s, 2.0 * k, c);
                }
                1 => {
                    if active {
                        g.rect(cx_ - k * 0.9, cy_ - k * 0.9, k * 1.8, k * 1.8, c);
                    } else {
                        g.polygon(&[(cx_ - k * 0.7, cy_ - k * 1.1), (cx_ - k * 0.7, cy_ + k * 1.1), (cx_ + k * 1.2, cy_)], c);
                    }
                }
                _ => {
                    g.polygon(&[(cx_ - k, cy_ - k), (cx_ - k, cy_ + k), (cx_ + k * 0.6, cy_)], c);
                    g.rect(cx_ + k * 0.9, cy_ - k, 2.5 * s, 2.0 * k, c);
                }
            }
            g.hit(r, Event::Tab(100 + i), label);
            bx += r.w + 12.0 * s;
        }
        // volume: 20 bars you can click
        let vol = p.volume() as usize;
        let (vx, by) = if narrow { (tx, by + 62.0 * s) } else { (bx + 18.0 * s, by) };
        let vw = (card.x + card.w - vx - 26.0 * s).clamp_safe(80.0 * s, 260.0 * s);
        let vol_label = g.fit(12.0 * s, &format!("VOLUME {vol}%"), Face::Bold, (card.x + card.w - vx - 8.0 * s).max(30.0 * s));
        g.text(vx, by - 2.0 * s, 12.0 * s, DIM, &vol_label, Face::Bold);
        let seg = vw / 20.0;
        for i in 0..20usize {
            let on = vol >= (i + 1) * 5;
            let hgt = (10.0 + i as f32 * 1.1) * s;
            let r = R::new(vx + i as f32 * seg, by + 38.0 * s - hgt + 6.0 * s, seg - 3.0 * s, hgt);
            g.rrect(r.x, r.y, r.w, r.h, 2.0 * s, if on { mix(CYAN, MAGENTA, i as f32 / 19.0) } else { rgb(28, 36, 62) });
            g.hit(R::new(vx + i as f32 * seg, by, seg, 50.0 * s), Event::Tab(201 + i), "volume");
        }
        // the level display along the bottom
        let (sx, sw2) = (card.x + 26.0 * s, card.w - 52.0 * s);
        let base = card.y + card.h - 18.0 * s;
        let maxh = (card.h - 196.0 * s - extra).max(40.0 * s);
        let bw = sw2 / BANDS as f32;
        for i in 0..BANDS {
            let idle = if active { 0.0 } else { 0.04 + 0.03 * (((tick as f32 * 0.05) + i as f32 * 0.7).sin() + 1.0) };
            let lvl = self.peaks[i].max(idle);
            let c = mix(CYAN, MAGENTA, i as f32 / (BANDS - 1) as f32);
            let hgt = (lvl * maxh).max(3.0 * s);
            g.rrect(sx + i as f32 * bw - 1.0 * s, base - hgt - 1.0 * s, bw - 1.0 * s, hgt + 2.0 * s, 3.0 * s, with_alpha(c, 40));
            g.rrect(sx + i as f32 * bw, base - hgt, bw - 4.0 * s, hgt, 2.0 * s, c);
            g.rect(sx + i as f32 * bw, base - hgt - 5.0 * s, bw - 4.0 * s, 2.0 * s, with_alpha(TEXT, 170));
        }
        y += ch + 16.0 * s;

        // ---- the tuner scale: one tick per station, the needle on the chosen one
        let n = list.len().max(1);
        let sy = y + 8.0 * s;
        g.line(x, sy + 10.0 * s, x + w, sy + 10.0 * s, 1.0 * s, with_alpha(MUTED, 80));
        for i in 0..=(n * 4) {
            let tx2 = x + w * i as f32 / (n * 4) as f32;
            let major = i % 4 == 0;
            g.line(tx2, sy + 10.0 * s, tx2, sy + if major { 0.0 } else { 5.0 * s }, 1.0 * s, with_alpha(MUTED, if major { 160 } else { 70 }));
        }
        let nx = x + w * (if n > 1 { self.sel as f32 / (n - 1) as f32 } else { 0.5 });
        g.line(nx, sy - 4.0 * s, nx, sy + 18.0 * s, 2.5 * s, MAGENTA);
        y += 34.0 * s;

        // the search box is always one click away; while a search runs the line says so
        let bw = g.button(x + w - 140.0 * s, y - 4.0 * s, 13.0 * s, "Search (/)", CYAN, Event::Tab(330));
        let line_w = (w - bw - 20.0 * s).max(60.0 * s);
        if let Mode::Searching { words, .. } = &self.mode {
            let m = g.fit(14.0 * s, &format!("Searching for \u{201c}{words}\u{201d}\u{2026} Esc gives it up."), Face::Sans, line_w);
            g.text(x, y, 14.0 * s, CYAN, &m, Face::Sans);
            y += 24.0 * s;
        } else if let Some((m, ok)) = &self.status {
            let m = g.fit(14.0 * s, m, Face::Sans, line_w);
            g.text(x, y, 14.0 * s, if *ok { OK } else { ERR }, &m, Face::Sans);
            y += 24.0 * s;
        } else {
            y += 24.0 * s;
        }
        // a station whose server's certificate could not be checked: what it showed, and the person's decision
        if let (RadioState::Failed { station, .. }, Some(c)) = (&state, p.cert_problem()) {
            if &c.station == station {
                let lines = g.wrap(14.0 * s, &c.message, Face::Sans, w);
                for l in lines.iter().take(4) {
                    g.text(x, y, 14.0 * s, MUTED, l, Face::Sans);
                    y += 20.0 * s;
                }
                if c.trustable {
                    g.button(x, y + 4.0 * s, 15.0 * s, if c.changed { "Review the new certificate (t)" } else { "Review certificate… (t)" }, CYAN, Event::Tab(320));
                } else {
                    g.text(x, y + 4.0 * s, 14.0 * s, ERR, "This certificate cannot be trusted: it is not for this server's name or not within its dates.", Face::Sans);
                }
                y += 48.0 * s;
            }
        }
        // a station that needs the audio add-on: one calm line and the way to get it (the person's click, from the vendor)
        if let RadioState::Failed { station, why } = &state {
            if p.failure_needs_addon(why) {
                let line = g.fit(14.0 * s, &format!("{station} sends AAC audio. RADIO needs the audio add-on (ffmpeg) to play it; it is downloaded from its builder when you ask."), Face::Sans, w);
                g.text(x, y, 14.0 * s, MUTED, &line, Face::Sans);
                g.button(x, y + 22.0 * s, 15.0 * s, "Install audio add-on", CYAN, Event::Tab(310));
                y += 60.0 * s;
            }
        }

        // ---- the stations on this tab
        let now = match &state {
            RadioState::Playing { station, .. } | RadioState::Connecting(station) => Some(station.clone()),
            _ => None,
        };
        if list.is_empty() {
            g.text(x, y + 6.0 * s, 17.0 * s, TEXT, "No stations of your own yet", Face::Bold);
            g.text(x, y + 34.0 * s, 14.0 * s, MUTED, "Pick a language on the left, choose a station and press A to keep it here, or press N to add an address.", Face::Sans);
            let b1 = g.button(x, y + 66.0 * s, 15.0 * s, "Add my own (n)", CYAN, Event::Key(Key::Char('n')));
            g.button(x + b1 + 10.0 * s, y + 66.0 * s, 15.0 * s, "Browse Tamil (→)", TEXT, Event::Tab(1));
        } else {
            let rh = 52.0 * s;
            let fit = (((area.y + area.h - y - 8.0 * s) / (rh + 6.0 * s)).floor() as usize).max(1);
            self.fit = fit;
            if self.sel != self.seen_sel {
                // the choice moved by a key (or a new tab): show it
                if self.sel < self.top {
                    self.top = self.sel;
                } else if self.sel >= self.top + fit {
                    self.top = self.sel + 1 - fit;
                }
                self.seen_sel = self.sel;
            }
            self.top = self.top.min(list.len().saturating_sub(fit));
            let start = self.top;
            let kept: Vec<&str> = self.mine.iter().map(|m| m.url.as_str()).collect();
            for (i, st) in list.iter().enumerate().skip(start).take(fit) {
                let rect = R::new(x, y, w, rh);
                let sel = i == self.sel;
                g.rrect(rect.x, rect.y, rect.w, rect.h, 12.0 * s, if sel { CARD_SEL } else if g.hovered(rect) { CARD_HOVER } else { CARD });
                if sel {
                    g.rrect_outline(rect.x, rect.y, rect.w, rect.h, 12.0 * s, 1.2 * s, with_alpha(CYAN, 160));
                }
                g.hit(rect, Event::Row(i), &st.name);
                let on = now.as_deref() == Some(st.name.as_str());
                // a little level display for the station that is on
                if on {
                    for k in 0..4 {
                        let hh = (6.0 + 12.0 * (0.5 + 0.5 * ((tick as f32 * 0.25) + k as f32 * 1.7).sin()) * if playing { 1.0 } else { 0.3 }) * s;
                        g.rrect(x + 16.0 * s + k as f32 * 6.0 * s, y + rh / 2.0 + 9.0 * s - hh, 4.0 * s, hh, 1.5 * s, mix(CYAN, MAGENTA, k as f32 / 3.0));
                    }
                } else {
                    g.text(x + 18.0 * s, y + 14.0 * s, 15.0 * s, DIM, &format!("{:02}", i + 1), Face::Mono);
                }
                let (nx, avail) = (x + 52.0 * s, w - 52.0 * s - 150.0 * s);
                let name = g.fit(17.0 * s, &st.name, Face::Bold, avail);
                let url = g.fit(12.0 * s, &st.url, Face::Sans, avail);
                g.text(nx, y + 7.0 * s, 17.0 * s, if on { OK } else { TEXT }, &name, Face::Bold);
                g.text(nx, y + 31.0 * s, 12.0 * s, DIM, &url, Face::Sans);
                if addon_missing && p.url_needs_addon(&st.url) {
                    g.text(x + w - 150.0 * s, y + 32.0 * s, 11.0 * s, DIM, "needs audio add-on", Face::Sans);
                }
                let pinned = self.with_pin(st).cert_pin.is_some();
                if pinned {
                    g.text(x + w - 150.0 * s, y + 32.0 * s, 11.0 * s, OK, "trusted certificate", Face::Sans);
                    if sel {
                        g.button(x + w - 290.0 * s, y + 9.0 * s, 13.0 * s, "Forget trust (f)", MUTED, Event::Tab(323));
                    }
                }
                // remove (My stations) / keep (the presets)
                if self.tab == 0 {
                    if sel {
                        g.button(x + w - 140.0 * s, y + 9.0 * s, 13.0 * s, "Remove (d)", ERR, Event::Tab(301));
                    }
                } else if self.tab > 0 {
                    if kept.contains(&st.url.as_str()) {
                        g.text(x + w - 120.0 * s, y + 16.0 * s, 13.0 * s, OK, "✓ in My stations", Face::Sans);
                    } else if sel {
                        g.button(x + w - 130.0 * s, y + 9.0 * s, 13.0 * s, "Keep (a)", CYAN, Event::Tab(300));
                    }
                }
                y += rh + 6.0 * s;
            }
        }
        match &self.mode {
            Mode::Form(f) | Mode::Find(f) => f.render(g, area, s, CYAN, blink(tick)),
            Mode::Searching { .. } => {}
            Mode::ConfirmDelete(n) => iyakki_uikit::widgets::render_confirm(g, area, s, &format!("Remove {n}?"), "Remove"),
            Mode::Trust { problem, focus } => render_trust(g, area, s, problem, *focus),
            Mode::Hold(_, ask) => iyakki_uikit::askdialog::render_question(g, area, s, &ask.question(), ask.stopping(), Event::Key(Key::Char('y')), Event::Key(Key::Char('n')), Event::Key(Key::Esc)),
            Mode::Browse => {}
        }
    }
}

/// One audit line per answer to the speaker question: what kind of holder and its name, nothing else.
fn hold_audit(p: &dyn RadioPort, action: &str, found: &Holder, outcome: &str) {
    p.audit(action, &format!("{} {outcome}", found.audit_kind()));
}


#[cfg(test)]
mod fake_tests;
