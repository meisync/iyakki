//! RADIO: internet radio in a player of its own. Stations come from the person's own list or from ten language
//! tabs (Tamil, Telugu, Malayalam, English, Hindi and more); one key starts a station and the music goes on in the background
//! while other apps are used (the launcher shows what plays). MP3 streams over HTTP or HTTPS and HLS with MP3 play by
//! themselves; AAC and HLS with AAC need the audio add-on, which the Install button below a failed station fetches from
//! its vendor (the Store's installer) and then tries the station again. The sound goes to the
//! system's play command (Settings → Voice), and every address is checked against the account's host policy.

use crate::safeclamp::SafeClamp;
use std::sync::Arc;

use iyakki_agent::iyakki_proxy::certinfo;
use iyakki_agent::radio::{self, CertProblem, RadioState, Station, StationStore, BANDS, LANGUAGES};
use iyakki_agent::radio_stations;

use super::widgets::{Form, FormEvent};
use super::{blink, App, AppCx, Flow};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, CARD, CARD_HOVER, CARD_SEL, DIM, ERR, MUTED, OK, R, TEXT};

use super::neon::{glow_card, mix, with_alpha, CYAN, MAGENTA, PANEL};

/// Tab 0 is the person's own stations; tabs 1 to 10 are the languages.
const TABS: usize = 1 + LANGUAGES.len();

enum Mode {
    Browse,
    Form(Form),
    ConfirmDelete(String),
    /// A station waits for the answer to "Browser is playing. Stop it so the radio can play?" (soundask.rs).
    Hold(Station, super::soundask::Ask),
    /// The person is asked whether to trust a station's server certificate. `focus` 0 is Cancel (where the dialog starts), 1 is Trust.
    /// Nothing is stored unless the person chooses Trust.
    Trust { problem: CertProblem, focus: usize },
}

pub struct RadioApp {
    mine: Vec<Station>,
    tab: usize,
    sel: usize,
    mode: Mode,
    status: Option<(String, bool)>,
    /// What the level display shows: the real levels with a slower fall and a held peak.
    peaks: [f32; BANDS],
    /// The first station shown on the right (the wheel moves it; the chosen station follows it only when keys move the choice).
    top: usize,
    /// How many station rows fit (set when drawn).
    fit: usize,
    /// The choice the list last made room for: a click selects without moving the list, a key that moves the choice scrolls to it.
    seen_sel: usize,
    /// The first row shown in the language list on the left.
    nav_top: usize,
    /// The pointer is over the language list (the wheel scrolls it instead of the stations).
    nav_hover: bool,
    /// The station that could not start for lack of the audio add-on; it is tried again once the add-on is installed.
    retry: Option<Station>,
    /// The person pressed "Install audio add-on": the shell opens the installer (`take_request`).
    install: bool,
    /// Whether the mixer shares the speaker between players (tests set it; it decides whether another player is in the way).
    mixing: fn() -> bool,
    /// Where the players announce themselves (the process-wide registry; tests have their own).
    reg: Arc<iyakki_agent::soundhold::Registry>,
}

fn store(cx: &AppCx) -> StationStore {
    StationStore::new(cx.services.dir(), &cx.session.user)
}

impl RadioApp {
    pub fn new(cx: &AppCx) -> RadioApp {
        let mine = store(cx).list();
        RadioApp { tab: if mine.is_empty() { 1.min(TABS - 1) } else { 0 }, mine, sel: 0, top: 0, fit: 6, seen_sel: 0, mode: Mode::Browse, status: None, peaks: [0.0; BANDS], nav_top: 0, nav_hover: false, retry: None, install: false, mixing: iyakki_agent::audiosink::mixing, reg: iyakki_agent::soundhold::global() }
    }

    /// The dialog about the speaker is open: its answers, and the end of the stop.
    fn hold_event(&mut self, ev: &Event, cx: &AppCx) {
        use super::soundask::{audit, answer_for_key, start_stop_real, Answer};
        let Mode::Hold(st, ask) = &mut self.mode else { return };
        if let Event::Key(k) = ev {
            if let (Some(a), false) = (answer_for_key(k), ask.stopping()) {
                let found = ask.found.clone();
                match a {
                    Answer::Stop => {
                        audit(cx, "radio.sound.stop", &found, "asked");
                        start_stop_real(ask, std::time::Duration::from_secs(3));
                    }
                    Answer::Keep => {
                        audit(cx, "radio.sound.keep", &found, "kept");
                        self.status = Some((format!("{} The radio was not started.", found.says), false));
                        self.mode = Mode::Browse;
                    }
                    Answer::Cancel => {
                        audit(cx, "radio.sound.cancel", &found, "cancelled");
                        self.mode = Mode::Browse;
                    }
                }
                return;
            }
        }
        if let Some(r) = ask.poll() {
            let (found, station) = (ask.found.clone(), st.clone());
            self.mode = Mode::Browse;
            match r {
                Ok(m) => {
                    audit(cx, "radio.sound.stopped", &found, &m);
                    self.start_station(&station, cx);
                }
                Err(e) => {
                    audit(cx, "radio.sound.stopfailed", &found, &e);
                    self.status = Some((format!("{} {e}", found.says), false));
                }
            }
        }
    }

    /// The stations on the current tab.
    fn list(&self) -> Vec<Station> {
        if self.tab == 0 {
            self.mine.clone()
        } else {
            LANGUAGES[self.tab - 1].stations.iter().map(|(n, u)| Station::new(n, u)).collect()
        }
    }

    pub fn tab_name(&self) -> String {
        if self.tab == 0 { "My stations".into() } else { LANGUAGES[self.tab - 1].name.into() }
    }

    pub fn station_names(&self) -> Vec<String> {
        self.list().into_iter().map(|s| s.name).collect()
    }

    pub fn status_text(&self) -> Option<&str> {
        self.status.as_ref().map(|s| s.0.as_str())
    }

    fn play_station(&mut self, st: &Station, cx: &AppCx) {
        let policy = cx.session.policy.clone();
        let host = radio::host_of(&st.url);
        if !policy.allows_host(&host) {
            self.status = Some((format!("Your account may not reach {host}."), false));
            return;
        }
        // another player of IYAKKI holds the speaker and would lock this one out (or the browser's page plays): ask first
        if let Some(found) = self.reg.conflict_for_new_player("radio", (self.mixing)()) {
            super::soundask::audit(cx, "radio.sound.ask", &found, "asked");
            self.mode = Mode::Hold(st.clone(), super::soundask::Ask::with_registry(self.reg.clone(), found, "the radio"));
            return;
        }
        self.start_station(st, cx);
    }

    /// The station as it is kept in My stations: with the certificate its owner approved, when there is one.
    fn with_pin(&self, st: &Station) -> Station {
        match self.mine.iter().find(|m| m.name == st.name && m.url == st.url) {
            Some(m) => Station { cert_pin: m.cert_pin.clone(), ..st.clone() },
            None => st.clone(),
        }
    }

    /// The station a certificate question is about (kept ones first, then the presets).
    fn station_named(&self, name: &str) -> Option<Station> {
        self.mine.iter().cloned().chain(LANGUAGES.iter().flat_map(|l| l.stations.iter().map(|(n, u)| Station::new(n, u)))).find(|s| s.name == name)
    }

    /// The certificate question of the station that failed, if that is why it did.
    fn open_trust(&mut self, cx: &AppCx) {
        if let (RadioState::Failed { station, .. }, Some(problem)) = (cx.services.radio.state(), cx.services.radio.cert_problem()) {
            if problem.station == station && problem.trustable {
                self.mode = Mode::Trust { problem, focus: 0 };
            }
        }
    }

    /// The person chose Trust: the exact certificate is kept with the station, and the station starts again.
    fn confirm_trust(&mut self, problem: &CertProblem, cx: &AppCx) {
        self.mode = Mode::Browse;
        let Some(base) = self.station_named(&problem.station) else {
            self.status = Some(("That station is not in your lists any more.".into(), false));
            return;
        };
        match store(cx).trust(&base, problem) {
            Ok(st) => {
                cx.audit.record(&cx.session.user, "radio.trust", &format!("{} sha256 {}", problem.host, certinfo::short_fingerprint(&problem.sha256)));
                self.reload(cx);
                self.start_station(&st, cx);
                self.status = Some((format!("Trusting the certificate of {} for {} only. It is asked about again when the certificate is renewed.", problem.host, st.name), true));
            }
            Err(e) => self.status = Some((e, false)),
        }
    }

    /// Forget the approved certificate of the chosen station.
    fn forget_trust(&mut self, cx: &AppCx) {
        let Some(st) = self.list().get(self.sel).cloned() else { return };
        let pinned = self.with_pin(&st).cert_pin;
        let Some(pin) = pinned else { return };
        match store(cx).forget_trust(&st.name) {
            Ok(_) => {
                cx.audit.record(&cx.session.user, "radio.trust.revoked", &format!("{} sha256 {}", pin.host, certinfo::short_fingerprint(&pin.sha256)));
                self.reload(cx);
                self.status = Some((format!("{} no longer trusts the certificate of {}.", st.name, pin.host), true));
            }
            Err(e) => self.status = Some((e, false)),
        }
    }

    fn trust_event(&mut self, ev: &Event, cx: &AppCx) {
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
                self.confirm_trust(&problem, cx);
            }
            Some(false) => {
                self.mode = Mode::Browse;
                self.status = Some(("Nothing was trusted.".into(), true));
            }
            None => {}
        }
    }

    fn start_station(&mut self, st: &Station, cx: &AppCx) {
        let st = &self.with_pin(st);
        let policy = cx.session.policy.clone();
        let host = radio::host_of(&st.url);
        self.retry = None;
        cx.audit.record(&cx.session.user, "radio.play", &host);
        let allow: Arc<dyn Fn(&str) -> bool + Send + Sync> = Arc::new(move |h| policy.allows_host(h));
        cx.services.radio.play(st, allow, &cx.services.voice_settings().play_cmd);
        self.status = None;
    }

    fn play_selected(&mut self, cx: &AppCx) {
        if let Some(st) = self.list().get(self.sel).cloned() {
            self.play_station(&st, cx);
        }
    }

    /// Move to the previous (-1) or next (1) station on this tab and play it.
    fn step(&mut self, d: i32, cx: &AppCx) {
        let n = self.list().len() as i32;
        if n == 0 {
            return;
        }
        self.sel = (self.sel as i32 + d).rem_euclid(n) as usize;
        self.play_selected(cx);
    }

    fn new_form() -> Form {
        Form::new("Add your own station").text("Name", "", "e.g. Jazz FM").text("Stream address", "", "https://stream.example.org/live.mp3")
    }

    fn reload(&mut self, cx: &AppCx) {
        self.mine = store(cx).list();
        self.sel = self.sel.min(self.list().len().saturating_sub(1));
    }

    fn set_tab(&mut self, t: usize) {
        self.tab = t.min(TABS - 1);
        self.sel = 0;
        self.top = 0;
        self.seen_sel = 0;
    }

    /// Keep the chosen preset in My stations.
    fn keep_selected(&mut self, cx: &AppCx) {
        if self.tab == 0 {
            self.mode = Mode::Form(Self::new_form());
            return;
        }
        let Some(st) = self.list().get(self.sel).cloned() else { return };
        match store(cx).add(&st.name, &st.url) {
            Ok(st) => {
                cx.audit.record(&cx.session.user, "radio.add", &radio::host_of(&st.url));
                self.reload(cx);
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
            "t · when a station fails on its server's certificate: review it and decide whether to trust it (asks first)",
            "f · forget the certificate you trusted for the chosen station",
            "d · delete the chosen station from My stations (asks first)",
            "Esc · back to the apps; the music keeps playing in the background",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn media_key(&mut self, k: crate::fnkeys::FnKey, cx: &AppCx) -> bool {
        use crate::fnkeys::FnKey;
        if !matches!(self.mode, Mode::Browse) {
            return false;
        }
        match k {
            FnKey::PlayPause => {
                if !cx.services.radio.resume() && !cx.services.radio.pause() {
                    self.play_selected(cx);
                }
            }
            FnKey::Next => self.step(1, cx),
            FnKey::Prev => self.step(-1, cx),
            FnKey::Stop => cx.services.radio.stop(),
            _ => return false,
        }
        true
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        if let Mode::Hold(..) = &self.mode {
            self.hold_event(ev, cx);
            return Flow::Stay;
        }
        if let Mode::Trust { .. } = &self.mode {
            self.trust_event(ev, cx);
            return Flow::Stay;
        }
        if let Mode::Form(form) = &mut self.mode {
            match form.handle_event(ev) {
                FormEvent::Cancel => self.mode = Mode::Browse,
                FormEvent::Submit => {
                    let (name, url) = (form.value(0), form.value(1));
                    match store(cx).add(&name, &url) {
                        Ok(st) => {
                            cx.audit.record(&cx.session.user, "radio.add", &radio::host_of(&st.url));
                            self.mode = Mode::Browse;
                            self.reload(cx);
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
            Event::Tab(n) if *n < TABS => {
                self.set_tab(*n);
                return Flow::Stay;
            }
            Event::Tab(100) => {
                self.step(-1, cx);
                return Flow::Stay;
            }
            Event::Tab(101) => {
                if cx.services.radio.is_active() {
                    cx.services.radio.stop();
                } else {
                    self.play_selected(cx);
                }
                return Flow::Stay;
            }
            Event::Tab(102) => {
                self.step(1, cx);
                return Flow::Stay;
            }
            Event::Tab(n) if (200..=220).contains(n) => {
                cx.services.radio.set_volume(((n - 200) * 5) as u8);
                return Flow::Stay;
            }
            // "Install audio add-on" under a station that needs it
            Event::Tab(310) => {
                if let RadioState::Failed { station, why } = cx.services.radio.state() {
                    if iyakki_agent::audiofmt::needs_addon(&why) {
                        self.retry = self.list().into_iter().chain(self.mine.clone()).find(|s| s.name == station);
                        self.install = true;
                    }
                }
                return Flow::Stay;
            }
            // "Review the certificate" under a station that failed on its server's certificate, and "Forget trust"
            Event::Tab(320) => {
                self.open_trust(cx);
                return Flow::Stay;
            }
            Event::Tab(323) => {
                self.forget_trust(cx);
                return Flow::Stay;
            }
            Event::Tab(300) => {
                self.keep_selected(cx);
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
                match store(cx).remove(&name) {
                    Ok(()) => self.status = Some((format!("Removed {name}."), true)),
                    Err(e) => self.status = Some((e, false)),
                }
                self.reload(cx);
            }
            self.mode = Mode::Browse;
            return Flow::Stay;
        }
        let n = self.list().len();
        match k {
            Key::Esc => return Flow::Back,
            Key::Left => self.set_tab((self.tab + TABS - 1) % TABS),
            Key::Right => self.set_tab((self.tab + 1) % TABS),
            Key::Up => self.sel = self.sel.saturating_sub(1),
            Key::Down => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            Key::Enter => self.play_selected(cx),
            Key::Char(' ') => {
                if cx.services.radio.is_active() {
                    cx.services.radio.stop();
                } else {
                    self.play_selected(cx);
                }
            }
            Key::Char('s') => cx.services.radio.stop(),
            // pause and carry on (a stopped station starts again from the beginning)
            Key::Char('p') => {
                if !cx.services.radio.resume() {
                    cx.services.radio.pause();
                }
            }
            Key::Char('[') => self.step(-1, cx),
            Key::Char(']') => self.step(1, cx),
            Key::Char('+') | Key::Char('=') => cx.services.radio.set_volume(cx.services.radio.volume().saturating_add(5)),
            Key::Char('-') | Key::Char('_') => cx.services.radio.set_volume(cx.services.radio.volume().saturating_sub(5)),
            Key::Char('a') => self.keep_selected(cx),
            Key::Char('t') => self.open_trust(cx),
            Key::Char('f') => self.forget_trust(cx),
            Key::Char('n') => self.mode = Mode::Form(Self::new_form()),
            Key::Char('d') if self.tab == 0 => {
                if let Some(s) = self.mine.get(self.sel) {
                    self.mode = Mode::ConfirmDelete(s.name.clone());
                }
            }
            _ => {}
        }
        Flow::Stay
    }

    fn take_request(&mut self) -> Option<(String, String)> {
        // the Store's Setup page, on the media tools (the same installer as the MEDIA player's)
        std::mem::take(&mut self.install).then(|| ("quickinstall".to_string(), "ffmpeg".to_string()))
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        // back from the installer with the add-on in place: the station that waited is tried again
        if self.retry.is_some() && iyakki_agent::aacpipe::installed() {
            if let Some(st) = self.retry.take() {
                self.play_station(&st, cx);
            }
        }
        let addon_missing = !iyakki_agent::aacpipe::installed();
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let mut y = area.y + 10.0 * s;
        let state = cx.services.radio.state();
        let playing = matches!(state, RadioState::Playing { .. });
        let active = cx.services.radio.is_active();
        // the display follows the real levels: up at once, down slowly
        let levels = cx.services.radio.levels();
        for (p, l) in self.peaks.iter_mut().zip(levels) {
            *p = if l > *p { l } else { (*p - 0.035).max(l) };
        }
        let energy = levels.iter().sum::<f32>() / BANDS as f32;

        // ---- the languages: a list down the left side, or a row of tabs when the window is narrow
        let names: Vec<String> = std::iter::once("My stations".to_string()).chain(LANGUAGES.iter().map(|l| l.name.to_string())).collect();
        let counts: Vec<usize> = std::iter::once(self.mine.len()).chain(LANGUAGES.iter().map(|l| l.stations.len())).collect();
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
            self.nav_top = self.nav_top.min(TABS.saturating_sub(slots));
            for (k, i) in (self.nav_top..TABS.min(self.nav_top + slots)).enumerate() {
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
            if TABS > slots {
                g.text(nav.x + 16.0 * s, nav.y + nav.h - 74.0 * s, 11.0 * s, DIM, &format!("{}–{} of {}", self.nav_top + 1, (self.nav_top + slots).min(TABS), TABS), Face::Sans);
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
            for i in first..TABS {
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
            RadioState::Connecting(n) => (n.clone(), "tuning in…".to_string(), CYAN),
            RadioState::Playing { station, title, kbps } => (station.clone(), if title.is_empty() { format!("on air{}", if *kbps > 0 { format!(" · {kbps} kbit/s") } else { String::new() }) } else { title.clone() }, OK),
            RadioState::Failed { station, why } => {
                let waiting = cx.services.radio.cert_problem().is_some_and(|c| &c.station == station);
                (format!("{station} did not play"), if waiting { "The server did not send its full certificate chain. Review its certificate below.".to_string() } else { why.clone() }, ERR)
            }
        };
        g.text(tx, card.y + 22.0 * s, 12.0 * s, if playing { MAGENTA } else { DIM }, if playing && cx.services.radio.is_paused() { "❚❚ PAUSED" } else if playing { "● ON AIR" } else if active { "◌ TUNING" } else { "○ STANDBY" }, Face::Bold);
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
        let vol = cx.services.radio.volume() as usize;
        let vx = bx + 18.0 * s;
        let vw = (card.x + card.w - vx - 26.0 * s).clamp_safe(80.0 * s, 260.0 * s);
        g.text(vx, by - 2.0 * s, 12.0 * s, DIM, &format!("VOLUME {vol}%"), Face::Bold);
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
        let maxh = (card.h - 196.0 * s).max(40.0 * s);
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

        if let Some((m, ok)) = &self.status {
            let m = g.fit(14.0 * s, m, Face::Sans, w);
            g.text(x, y, 14.0 * s, if *ok { OK } else { ERR }, &m, Face::Sans);
            y += 24.0 * s;
        }
        // a station whose server's certificate could not be checked: what it showed, and the person's decision
        if let (RadioState::Failed { station, .. }, Some(c)) = (&state, cx.services.radio.cert_problem()) {
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
            if iyakki_agent::audiofmt::needs_addon(why) {
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
                if addon_missing && radio_stations::needs_addon(&st.url) {
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
            Mode::Form(f) => f.render(g, area, s, CYAN, blink(tick)),
            Mode::ConfirmDelete(n) => super::widgets::render_confirm(g, area, s, &format!("Remove {n}?"), "Remove"),
            Mode::Trust { problem, focus } => render_trust(g, area, s, problem, *focus),
            Mode::Hold(_, ask) => super::soundask::render(g, area, s, ask, Event::Key(Key::Char('y')), Event::Key(Key::Char('n')), Event::Key(Key::Esc)),
            Mode::Browse => {}
        }
    }
}

/// The question before a server certificate is trusted: everything the person can compare, a plain warning, Cancel first.
fn render_trust(g: &mut Gfx, area: R, s: f32, c: &CertProblem, focus: usize) {
    let p = crate::fx::modal(g, area, s, 680.0 * s, 470.0 * s, ERR, true);
    let title = if c.changed { "The certificate changed. Trust the new one?" } else { "Trust this station's certificate?" };
    let mut y = crate::fx::modal_title(g, p, s, title, ERR) + 6.0 * s;
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
    let groups: Vec<String> = certinfo::fingerprint_groups(&c.sha256).split(' ').map(str::to_string).collect();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
    use crate::apps::tile_harness::TileHarness;
    use iyakki_agent::iyakki_proxy::{StreamHead, Streamer};
    use iyakki_sec::policy::Role;
    use std::sync::Mutex;

    /// Silence in MP3 frames that never ends (a little slower than real time, so the test stays quiet).
    struct Endless(Vec<u8>, usize);
    impl std::io::Read for Endless {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            std::thread::sleep(std::time::Duration::from_millis(2));
            let n = out.len().min(417);
            for b in out[..n].iter_mut() {
                *b = self.0[self.1 % self.0.len()];
                self.1 += 1;
            }
            Ok(n)
        }
    }

    struct Fake(Mutex<Vec<String>>);
    impl Streamer for Fake {
        fn open(&self, url: &str, _h: &[(String, String)]) -> Result<StreamHead, String> {
            self.0.lock().unwrap().push(url.to_string());
            let mut frame = vec![0u8; 417];
            frame[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x64]);
            Ok(StreamHead { status: 200, headers: vec![("content-type".into(), "audio/mpeg".into())], body: Box::new(Endless(frame, 0)) })
        }
    }

    fn cx(tag: &str) -> (AppCx, Arc<Fake>) {
        let mut cx = cx_with_admin_store(tag);
        let d = temp_dir(&format!("{tag}-svc"));
        cx.services = services_in(&d, Arc::new(crate::apps::tests::Offline));
        let f = Arc::new(Fake(Mutex::new(vec![])));
        cx.services.radio.set_streamer(f.clone());
        cx.services.radio.set_retry_wait(std::time::Duration::from_millis(20));
        cx.services.settings.lock().unwrap().update(|s| s.voice.play_cmd = "tee /dev/null".into()).unwrap();
        (cx, f)
    }

    fn typ(a: &mut RadioApp, cx: &AppCx, s: &str) {
        for c in s.chars() {
            a.handle(&Event::Key(Key::Char(c)), cx);
        }
    }

    fn wait(cx: &AppCx, what: impl Fn(&RadioState) -> bool) {
        for _ in 0..500 {
            if what(&cx.services.radio.state()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("{:?}", cx.services.radio.state());
    }

    #[test]
    fn a_station_waits_for_the_answer_when_the_browser_plays_and_starts_after_stop_it() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let (cx, fake) = cx("radio-hold");
        let mut a = RadioApp::new(&cx);
        a.mixing = || false;
        a.reg = Arc::new(iyakki_agent::soundhold::Registry::default());
        a.handle(&Event::Tab(1), &cx);
        // another player holds the speaker
        let on = Arc::new(AtomicBool::new(true));
        let (h, st) = (on.clone(), on.clone());
        a.reg.announce("tales", Arc::new(move || h.load(Ordering::SeqCst).then(|| "TALES is reading".to_string())), Arc::new(move || st.store(false, Ordering::SeqCst)));
        a.handle(&Event::Key(Key::Enter), &cx);
        assert!(matches!(&a.mode, Mode::Hold(_, ask) if ask.question() == "TALES is reading. Stop it so the radio can play?"));
        assert_eq!(cx.services.radio.state(), RadioState::Idle, "nothing starts before the answer");
        let g = draw(&mut a, &cx, 1280, 720);
        for l in ["Stop it", "Keep it", "Cancel"] {
            assert!(g.hits.iter().any(|h| h.label == l), "{l}");
        }
        // Keep it: nothing starts, the other player goes on
        a.handle(&Event::Key(Key::Char('n')), &cx);
        assert!(matches!(a.mode, Mode::Browse) && on.load(Ordering::SeqCst));
        assert!(a.status_text().unwrap().contains("TALES is reading") && cx.services.radio.state() == RadioState::Idle);
        // Cancel
        a.handle(&Event::Key(Key::Enter), &cx);
        a.handle(&Event::Key(Key::Esc), &cx);
        assert!(matches!(a.mode, Mode::Browse) && on.load(Ordering::SeqCst));
        // Stop it: the other player is stopped cleanly, then the station starts
        a.handle(&Event::Key(Key::Enter), &cx);
        a.handle(&Event::Key(Key::Char('y')), &cx);
        let t0 = std::time::Instant::now();
        while matches!(a.mode, Mode::Hold(..)) {
            assert!(t0.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(10));
            a.handle(&Event::Tick, &cx);
        }
        assert!(!on.load(Ordering::SeqCst), "TALES was stopped");
        wait(&cx, |s| matches!(s, RadioState::Playing { .. }));
        assert!(!fake.0.lock().unwrap().is_empty());
        cx.services.radio.stop();
        // with the mixer on, a player that goes through it is no conflict
        on.store(true, Ordering::SeqCst);
        a.mixing = || true;
        a.handle(&Event::Key(Key::Enter), &cx);
        assert!(matches!(a.mode, Mode::Browse));
        wait(&cx, |s| matches!(s, RadioState::Playing { .. }));
        cx.services.radio.stop();
        a.reg.forget("tales");
    }

    #[test]
    fn a_station_is_added_played_in_the_background_and_stopped() {
        let (cx, fake) = cx("radio1");
        let mut a = RadioApp::new(&cx);
        assert_eq!(a.tab_name(), "Tamil", "with no stations of your own it opens on a language");
        a.handle(&Event::Tab(0), &cx);
        assert!(a.station_names().is_empty());
        let _ = draw(&mut a, &cx, 1280, 720); // the empty state
        a.handle(&Event::Key(Key::Char('n')), &cx);
        typ(&mut a, &cx, "Jazz FM");
        a.handle(&Event::Key(Key::Enter), &cx);
        typ(&mut a, &cx, "not a url");
        a.handle(&Event::Key(Key::Ctrl('s')), &cx);
        assert!(matches!(&a.mode, Mode::Form(f) if f.error.is_some()), "a bad address keeps the form open");
        if let Mode::Form(f) = &mut a.mode {
            f.set_value(1, "https://jazz.example.org/live.mp3");
        }
        a.handle(&Event::Key(Key::Ctrl('s')), &cx);
        assert_eq!(a.station_names(), vec!["Jazz FM"]);
        a.handle(&Event::Key(Key::Enter), &cx);
        wait(&cx, |s| matches!(s, RadioState::Playing { .. }));
        assert_eq!(fake.0.lock().unwrap()[0], "https://jazz.example.org/live.mp3");
        let _ = draw(&mut a, &cx, 1280, 720);
        // leave the app: the music goes on
        assert!(matches!(a.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
        assert!(cx.services.radio.is_active());
        // volume (keys and the bars) and stop from a fresh visit
        let mut b = RadioApp::new(&cx);
        assert_eq!(b.station_names(), vec!["Jazz FM"], "stations are kept");
        let v = cx.services.radio.volume();
        b.handle(&Event::Key(Key::Char('-')), &cx);
        assert_eq!(cx.services.radio.volume(), v - 5);
        b.handle(&Event::Tab(210), &cx);
        assert_eq!(cx.services.radio.volume(), 50, "the tenth bar of twenty");
        let g = draw(&mut b, &cx, 1280, 720);
        assert!(g.hits.iter().filter(|h| h.label == "volume").count() == 20 && g.hits.iter().any(|h| h.label == "Play or stop"));
        b.handle(&Event::Tab(101), &cx); // the big button stops what plays
        assert_eq!(cx.services.radio.state(), RadioState::Idle);
        b.handle(&Event::Key(Key::Char(' ')), &cx); // and starts the chosen station again
        wait(&cx, |s| matches!(s, RadioState::Playing { .. }));
        b.handle(&Event::Key(Key::Char('s')), &cx);
        assert_eq!(cx.services.radio.state(), RadioState::Idle);
        let log = std::fs::read_to_string(cx.audit.path()).unwrap();
        assert!(log.contains("radio.add") && log.contains("radio.play"), "{log}");
    }

    #[test]
    fn the_chat_radio_tools_use_the_same_player_and_station_list_as_the_app() {
        use iyakki_agent::agent::{Tools, UserTools};
        let (cx, fake) = cx("radio-chat");
        let tools = UserTools::new(&cx.session.user, cx.session.policy.clone(), cx.services.clone(), cx.host.clone(), vec![]);
        let said = tools.call("radio_play", &serde_json::json!({"query": "play a tamil online fm"})).unwrap();
        assert!(said.starts_with("Playing "), "{said}");
        wait(&cx, |s| matches!(s, RadioState::Playing { .. }));
        assert!(fake.0.lock().unwrap()[0].starts_with("https://"), "the stream the chat chose was opened by the shared player");
        // the app, opened afterwards, finds the station playing; its stop key stops what the chat started
        let mut a = RadioApp::new(&cx);
        assert!(cx.services.radio.is_active());
        let _ = draw(&mut a, &cx, 1280, 720);
        // a station the chat keeps appears in the app's My stations
        tools.call("radio_save", &serde_json::json!({})).unwrap();
        let b = RadioApp::new(&cx);
        assert_eq!(b.tab_name(), "My stations");
        assert_eq!(b.station_names().len(), 1);
        a.handle(&Event::Key(Key::Char('s')), &cx);
        assert_eq!(cx.services.radio.state(), RadioState::Idle);
        assert_eq!(tools.call("radio_stop", &serde_json::json!({})).unwrap(), "The radio was not playing.");
    }

    #[test]
    fn the_language_tabs_hold_presets_that_can_be_played_kept_and_stepped_through() {
        let (cx, _fake) = cx("radio3");
        let mut a = RadioApp::new(&cx);
        // Tamil is the first language and is on the tab list
        a.handle(&Event::Tab(1), &cx);
        assert_eq!(a.tab_name(), "Tamil");
        assert!(a.station_names().contains(&"Sooriyan FM".to_string()));
        // Left and Right walk through My stations and every language, wrapping round
        let mut seen = vec![a.tab_name()];
        for _ in 0..TABS {
            a.handle(&Event::Key(Key::Right), &cx);
            seen.push(a.tab_name());
        }
        assert_eq!(seen[..4].to_vec(), vec!["Tamil", "Telugu", "Malayalam", "English"]);
        assert_eq!(seen.last().unwrap(), "Tamil", "one step per tab brings it back round");
        assert!(seen.contains(&"My stations".to_string()));
        a.handle(&Event::Key(Key::Left), &cx);
        assert_eq!(a.tab_name(), "My stations");
        // play a preset straight from its tab: no need to keep it first
        a.handle(&Event::Tab(4), &cx);
        a.handle(&Event::Key(Key::Enter), &cx);
        wait(&cx, |s| matches!(s, RadioState::Playing { station, .. } if station == "Radio Paradise"));
        // ] plays the next station, [ the previous, both wrapping
        a.handle(&Event::Key(Key::Char(']')), &cx);
        wait(&cx, |s| matches!(s, RadioState::Playing { station, .. } | RadioState::Connecting(station) if station == "KEXP Seattle"));
        a.handle(&Event::Key(Key::Char('[')), &cx);
        a.handle(&Event::Key(Key::Char('[')), &cx);
        let last = LANGUAGES[3].stations.last().unwrap().0;
        wait(&cx, |s| matches!(s, RadioState::Playing { station, .. } | RadioState::Connecting(station) if station == last));
        // keep one in My stations
        a.handle(&Event::Key(Key::Down), &cx);
        a.handle(&Event::Key(Key::Char('a')), &cx);
        assert!(a.status_text().unwrap().contains("is in My stations"), "{:?}", a.status_text());
        a.handle(&Event::Tab(0), &cx);
        assert_eq!(a.station_names().len(), 1);
        // every tab draws at several sizes, playing or not
        for t in 0..TABS {
            a.handle(&Event::Tab(t), &cx);
            let _ = draw(&mut a, &cx, 1280, 720);
        }
        for (w, h) in [(800, 600), (640, 480), (1920, 1080)] {
            let _ = draw(&mut a, &cx, w, h);
        }
        cx.services.radio.stop();
        let _ = draw(&mut a, &cx, 1280, 720);
    }

    #[test]
    fn the_wheel_scrolls_the_stations_and_a_click_chooses_a_row_without_moving_the_list() {
        let (cx, _d) = crate::apps::tests::cx_with_backend("radiowheel", iyakki_agent::chat::Unconfigured);
        let mut a = RadioApp::new(&cx);
        a.handle(&Event::Tab(1), &cx);
        // a window small enough that the tab's stations do not all fit
        let g = draw(&mut a, &cx, 1280, 720);
        let total = a.list().len();
        assert!(a.fit < total, "{} stations, {} fit", total, a.fit);
        assert_eq!((a.top, a.sel), (0, 0));
        let first_rows: Vec<usize> = g.hits.iter().filter_map(|h| if let Event::Row(i) = h.ev { Some(i) } else { None }).collect();
        assert_eq!(first_rows.first(), Some(&0));
        // the wheel moves the list and leaves the choice alone
        a.handle(&Event::Scroll(-1), &cx);
        let g = draw(&mut a, &cx, 1280, 720);
        assert_eq!((a.top, a.sel), (3, 0), "three rows down, the choice stays");
        let rows: Vec<usize> = g.hits.iter().filter_map(|h| if let Event::Row(i) = h.ev { Some(i) } else { None }).collect();
        assert_eq!(rows.first(), Some(&3));
        // a click chooses that row and the list does not shift under the pointer
        let clicked = rows[1];
        a.handle(&Event::Row(clicked), &cx);
        let g = draw(&mut a, &cx, 1280, 720);
        assert_eq!((a.top, a.sel), (3, clicked), "the same rows are shown after the click");
        let after: Vec<usize> = g.hits.iter().filter_map(|h| if let Event::Row(i) = h.ev { Some(i) } else { None }).collect();
        assert_eq!(after, rows);
        // the wheel stops at both ends
        for _ in 0..40 {
            a.handle(&Event::Scroll(-1), &cx);
        }
        draw(&mut a, &cx, 1280, 720);
        assert_eq!(a.top, total - a.fit);
        for _ in 0..40 {
            a.handle(&Event::Scroll(1), &cx);
        }
        assert_eq!(a.top, 0);
        // keys still bring the choice into view
        a.handle(&Event::Tab(1), &cx);
        for _ in 0..a.fit + 1 {
            a.handle(&Event::Key(Key::Down), &cx);
        }
        draw(&mut a, &cx, 1280, 720);
        assert!(a.top > 0 && a.sel >= a.top && a.sel < a.top + a.fit, "top {} sel {} fit {}", a.top, a.sel, a.fit);
    }

    #[test]
    fn the_language_list_is_on_the_left_a_station_is_added_by_name_and_address_and_removed_from_my_stations_with_a_button() {
        let (cx, _d) = {
            let (cx, d) = crate::apps::tests::cx_with_backend("radiolist", iyakki_agent::chat::Unconfigured);
            (cx, d)
        };
        let mut a = RadioApp::new(&cx);
        let g = draw(&mut a, &cx, 1280, 760);
        // the nav lists My stations and the languages, with counts, and has the add button
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(0) && h.label == "My stations"));
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(1) && h.label == "Tamil"));
        assert!(g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('n'))), "an Add station button");
        // clicking it opens the form: a name and an address
        a.handle(&Event::Key(Key::Char('n')), &cx);
        let g = draw(&mut a, &cx, 1280, 760);
        let _ = g;
        for c in "Night Jazz".chars() {
            a.handle(&Event::Key(Key::Char(c)), &cx);
        }
        a.handle(&Event::Key(Key::Tab), &cx);
        for c in "https://jazz.example.org/live.mp3".chars() {
            a.handle(&Event::Key(Key::Char(c)), &cx);
        }
        a.handle(&Event::Key(Key::Enter), &cx);
        a.handle(&Event::Tab(0), &cx);
        assert_eq!(a.station_names(), vec!["Night Jazz".to_string()], "{:?}", a.status_text());
        // a bad address is explained and nothing is added
        a.handle(&Event::Key(Key::Char('n')), &cx);
        for c in "Bad".chars() {
            a.handle(&Event::Key(Key::Char(c)), &cx);
        }
        a.handle(&Event::Key(Key::Tab), &cx);
        for c in "not an address".chars() {
            a.handle(&Event::Key(Key::Char(c)), &cx);
        }
        a.handle(&Event::Key(Key::Enter), &cx);
        assert!(matches!(a.mode, Mode::Form(_)), "the form stays open with the reason");
        a.handle(&Event::Key(Key::Esc), &cx);
        // the station in My stations has a Remove button, which asks first
        let g = draw(&mut a, &cx, 1280, 760);
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(301)), "a Remove button on the chosen station");
        a.handle(&Event::Tab(301), &cx);
        assert!(matches!(a.mode, Mode::ConfirmDelete(_)));
        draw(&mut a, &cx, 1280, 760);
        a.handle(&Event::Key(Key::Char('n')), &cx); // anything but y keeps it
        assert_eq!(a.station_names().len(), 1);
        a.handle(&Event::Tab(301), &cx);
        a.handle(&Event::Key(Key::Char('y')), &cx);
        assert!(a.station_names().is_empty());
        // the list scrolls to show the chosen language, and the wheel over it moves it
        a.handle(&Event::Tab(TABS - 1), &cx);
        draw(&mut a, &cx, 1280, 560);
        assert!(a.nav_top > 0, "the list moved down to the last language");
        // the narrow window falls back to tabs
        let g = draw(&mut a, &cx, 640, 480);
        assert!(g.hits.iter().any(|h| matches!(h.ev, Event::Tab(_))));
    }

    #[test]
    fn an_aac_station_without_the_add_on_offers_to_install_it() {
        struct AacOnly;
        impl Streamer for AacOnly {
            fn open(&self, _u: &str, _h: &[(String, String)]) -> Result<StreamHead, String> {
                Ok(StreamHead { status: 200, headers: vec![("content-type".into(), "audio/aacp".into())], body: Box::new(std::io::Cursor::new(vec![0u8; 64])) })
            }
        }
        let (cx, _) = cx("addon");
        cx.services.radio.set_streamer(Arc::new(AacOnly));
        cx.services.radio.set_aac_factory(Arc::new(|| Err(iyakki_agent::audiofmt::NEEDS_AUDIO_ADDON.to_string())));
        let mut t = TileHarness::new(RadioApp::new(&cx), cx.clone());
        t.app.set_tab(1);
        t.app.sel = t.app.station_names().iter().position(|n| n == "80’s Tamil Hits").expect("80’s Tamil Hits is in the Tamil list");
        t.key(Key::Enter);
        wait(&cx, |s| matches!(s, RadioState::Failed { why, .. } if iyakki_agent::audiofmt::needs_addon(why)));
        assert!(t.app.take_request().is_none(), "nothing is installed without a click");
        t.draw_at(1100, 720);
        t.click_label("Install audio add-on");
        assert_eq!(t.app.take_request(), Some(("quickinstall".to_string(), "ffmpeg".to_string())));
        assert_eq!(t.app.retry.as_ref().map(|s| s.name.as_str()), Some("80’s Tamil Hits"), "the station waits to be tried again");
        assert!(t.app.take_request().is_none(), "one click, one request");
        // playing another station forgets the wait
        t.app.sel = 0;
        t.key(Key::Enter);
        assert!(t.app.retry.is_none());
        cx.services.radio.stop();
    }

    #[test]
    fn the_host_policy_and_deleting() {
        let (mut cx, fake) = cx("radio2");
        let mut a = RadioApp::new(&cx);
        a.handle(&Event::Tab(4), &cx);
        a.handle(&Event::Key(Key::Char('a')), &cx);
        a.handle(&Event::Tab(3), &cx);
        a.handle(&Event::Key(Key::Char('a')), &cx);
        a.handle(&Event::Tab(0), &cx);
        assert_eq!(a.station_names().len(), 2);
        a.handle(&Event::Key(Key::Char('d')), &cx);
        a.handle(&Event::Key(Key::Char('n')), &cx);
        assert_eq!(a.station_names().len(), 2, "anything but y cancels");
        a.handle(&Event::Key(Key::Char('d')), &cx);
        a.handle(&Event::Key(Key::Char('y')), &cx);
        assert_eq!(a.station_names().len(), 1);
        // an account that may not reach the host cannot play, and nothing is contacted
        cx.session.role = Role::User;
        cx.session.policy = Role::User.template();
        cx.session.policy.net_hosts = vec!["other.example".into()];
        let mut b = RadioApp::new(&cx);
        b.handle(&Event::Key(Key::Enter), &cx);
        assert!(b.status_text().unwrap().contains("may not reach"), "{:?}", b.status_text());
        assert!(fake.0.lock().unwrap().is_empty());
        assert!(!b.help().is_empty());
    }

    /// A server that sends its leaf only: the usual check fails, the exact certificate approved by a person plays.
    struct NoChain(Vec<u8>, Mutex<Vec<String>>);
    impl Streamer for NoChain {
        fn open(&self, url: &str, _h: &[(String, String)]) -> Result<StreamHead, String> {
            self.1.lock().unwrap().push(format!("open {url}"));
            Err("io: invalid peer certificate: UnknownIssuer".into())
        }
        fn open_tls(&self, url: &str, h: &[(String, String)], mode: &iyakki_agent::iyakki_proxy::TlsMode) -> Result<StreamHead, String> {
            self.1.lock().unwrap().push(format!("tls {url}"));
            match mode {
                iyakki_agent::iyakki_proxy::TlsMode::Pin(fp) if certinfo::norm_fingerprint(fp) == certinfo::norm_fingerprint(&certinfo::fingerprint(&self.0)) => Fake(Mutex::new(vec![])).open(url, h),
                _ => Err("io: invalid peer certificate: UnknownIssuer".into()),
            }
        }
        fn peer_chain(&self, _: &str, _: u16) -> Result<Vec<Vec<u8>>, String> {
            Ok(vec![self.0.clone()])
        }
    }

    fn chain_world(tag: &str, years: Option<(i32, i32)>) -> (AppCx, Arc<NoChain>, String) {
        let (cx, _) = cx(tag);
        let leaf = iyakki_agent::iyakki_proxy::testcerts::leaf_der(&["radios.example.org"], years, None);
        let fp = certinfo::fingerprint(&leaf);
        let net = Arc::new(NoChain(leaf, Mutex::new(vec![])));
        cx.services.radio.set_streamer(net.clone());
        store(&cx).add("Hello FM", "https://radios.example.org:8002/3").unwrap();
        (cx, net, fp)
    }

    fn pin_of(cx: &AppCx) -> Option<radio::CertPin> {
        store(cx).list().iter().find(|s| s.name == "Hello FM").and_then(|s| s.cert_pin.clone())
    }

    fn audit_has(cx: &AppCx, what: &str) -> bool {
        cx.audit.recent(50).iter().any(|l| l.contains(what))
    }

    #[test]
    fn a_certificate_is_trusted_only_after_the_person_confirms_and_can_be_forgotten() {
        let (cx, net, fp) = chain_world("radio-trust", None);
        let mut t = TileHarness::new(RadioApp::new(&cx), cx.clone());
        t.key(Key::Enter);
        wait(&cx, |s| matches!(s, RadioState::Failed { .. }));
        t.draw_at(1280, 800);
        assert!(t.shows("did not send its full certificate chain"), "{:?}", t.texts());
        assert!(t.has_button("Review certificate… (t)"));
        assert!(pin_of(&cx).is_none() && !t.shows("trusted certificate"));
        // the question: what the server showed, a plain warning, Cancel is where it starts
        t.click_label("Review certificate… (t)");
        assert!(matches!(t.app.mode, Mode::Trust { focus: 0, .. }));
        for needle in ["radios.example.org:8002", "Hello FM", "someone on your network could pretend to be it", &certinfo::fingerprint_groups(&fp)[..8]] {
            assert!(t.shows(needle), "{needle}: {:?}", t.texts());
        }
        assert!(t.has_button("Cancel") && t.has_button("Trust this certificate"));
        // letters do not answer it; Enter on the first button, Esc and Cancel store nothing
        t.key(Key::Char('y'));
        t.key(Key::Char('t'));
        assert!(matches!(t.app.mode, Mode::Trust { .. }) && pin_of(&cx).is_none());
        t.key(Key::Enter);
        assert!(matches!(t.app.mode, Mode::Browse) && pin_of(&cx).is_none(), "Enter on the default Cancel");
        t.key(Key::Char('t'));
        t.key(Key::Esc);
        assert!(matches!(t.app.mode, Mode::Browse) && pin_of(&cx).is_none());
        t.key(Key::Char('t'));
        t.click_label("Cancel");
        assert!(pin_of(&cx).is_none() && !audit_has(&cx, "radio.trust"), "cancelling stores and audits nothing");
        assert!(!matches!(cx.services.radio.state(), RadioState::Playing { .. }));
        assert!(!net.1.lock().unwrap().iter().any(|l| l.starts_with("tls ")), "no unchecked connection was made");
        // confirming: the exact certificate is kept with the station, audited in short form, and the station plays
        t.key(Key::Char('t'));
        t.key(Key::Right);
        assert!(matches!(t.app.mode, Mode::Trust { focus: 1, .. }));
        t.key(Key::Enter);
        let pin = pin_of(&cx).expect("kept after the confirmation");
        assert_eq!((pin.host.as_str(), pin.port, pin.sha256.as_str()), ("radios.example.org", 8002, fp.as_str()));
        assert!(audit_has(&cx, &format!("radio.trust")) && audit_has(&cx, &certinfo::short_fingerprint(&fp)) && !cx.audit.recent(50).iter().any(|l| l.contains(&fp)));
        wait(&cx, |s| matches!(s, RadioState::Playing { .. }));
        t.draw_at(1280, 800);
        assert!(t.shows("trusted certificate") && t.has_button("Forget trust (f)"));
        // forgetting
        t.click_label("Forget trust (f)");
        assert!(pin_of(&cx).is_none() && audit_has(&cx, "radio.trust.revoked"));
        cx.services.radio.stop();
        // and the other button of the question also works
        t.key(Key::Enter);
        wait(&cx, |s| matches!(s, RadioState::Failed { .. }));
        t.draw_at(1280, 800);
        t.click_label("Review certificate… (t)");
        t.click_label("Trust this certificate");
        assert!(pin_of(&cx).is_some());
        cx.services.radio.stop();
    }

    #[test]
    fn a_certificate_that_cannot_work_is_shown_but_cannot_be_trusted() {
        let (cx, _net, _) = chain_world("radio-trust-old", Some((2019, 2020)));
        let mut t = TileHarness::new(RadioApp::new(&cx), cx.clone());
        t.key(Key::Enter);
        wait(&cx, |s| matches!(s, RadioState::Failed { .. }));
        t.draw_at(1280, 800);
        assert!(t.shows("expired") && !t.has_button("Review certificate… (t)") && t.shows("cannot be trusted"), "{:?}", t.texts());
        t.key(Key::Char('t'));
        assert!(matches!(t.app.mode, Mode::Browse) && pin_of(&cx).is_none());
    }

    /// `IYAKKI_SHOT=/path.png cargo test -p iyakki-shell --lib radio_picture -- --ignored`: the player drawn while a live station plays.
    #[test]
    #[ignore]
    fn radio_picture() {
        let Ok(out) = std::env::var("IYAKKI_SHOT") else { return };
        let (cx, _) = cx("radio-pic");
        cx.services.radio.set_streamer(Arc::new(iyakki_agent::iyakki_proxy::https::UreqStreamer::default()));
        let mut a = RadioApp::new(&cx);
        a.handle(&Event::Tab(4), &cx);
        a.handle(&Event::Key(Key::Enter), &cx);
        wait(&cx, |s| matches!(s, RadioState::Playing { .. }));
        let mut g = Gfx::new(1280, 720);
        for t in 0..60u64 {
            std::thread::sleep(std::time::Duration::from_millis(40));
            g = Gfx::new(1280, 720);
            g.clear(rgb(9, 11, 20));
            a.render(&mut g, R::new(0.0, 46.0, 1280.0, 674.0), 1.0, &cx, 200 + t);
        }
        g.save_png(std::path::Path::new(&out)).unwrap();
        cx.services.radio.stop();
    }

    fn draw(a: &mut RadioApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
        let mut g = Gfx::new(w, h);
        a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), (w as f32 / 1280.0).clamp(0.6, 3.0), cx, 0);
        g
    }
}
