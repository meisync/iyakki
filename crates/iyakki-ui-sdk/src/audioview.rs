//! `AudioView`: a sound player as a component an SDK app places in its layout (docs/development/UI-SDK.md, "Embedding audio and video").
//! The app keeps an `AudioView` in its state, gives it the core's [`MediaPort`] and a base action number, and a rectangle to draw in. The
//! view owns what is on screen: the list of the person's sound files, the transport bar (play or pause, 10 seconds back and forward, a
//! seek bar, volume, mute, speed), the level meter, the track list and the approval card for a remote source. It draws through [`Canvas`],
//! so it works in the shell, in a browser or client (display list) and as a WASM tile, with the colours and size class of the host. The
//! sound itself is the core's: it is decoded there and plays through the core's mixer, so the person's volume, ducking under speech and the
//! sound doctor apply; the view never touches a byte of it.
//!
//! What the app does, in four lines:
//! ```text
//! event(Tick):   audio.tick()                                (lists the files once, then polls the core's events)
//! event(Action): if audio.owns(a) { audio.action(a) }        event(Key): if audio.key(k) { return }
//! view:          audio.draw(c, th, &metrics, rect)           start something yourself: audio.open(Source::Home(name), "Title")
//! ```
//! Keys: Space plays or pauses, Left and Right move 5 seconds, Up and Down choose in the list, Enter plays the chosen file, `+` and `-`
//! change the volume, `m` mutes, `r` changes the speed, `n` and `p` go to the next and the previous file, Esc stops.

use crate::canvas::{fit, Canvas, Hit, Role, TextStyle};
use crate::event::UiKey;
use crate::geom::Rect;
use crate::media::{Event, Info, Item, MediaPort, PlayState, Source, Track, TrackKind};
use crate::size::{Metrics, TypeRole};
use crate::tokens::{Line, Radius, Space};
use crate::theme::Theme;
use crate::widgets;

/// The action (from the base) of the play and pause button: another view may draw its own button for it.
pub const ACTION_PLAY: u32 = 0;
const A_PLAY: u32 = ACTION_PLAY;
const A_BACK: u32 = 1;
const A_FWD: u32 = 2;
const A_MUTE: u32 = 3;
const A_VOL_DOWN: u32 = 4;
const A_VOL_UP: u32 = 5;
const A_RATE: u32 = 6;
const A_ALLOW: u32 = 7;
const A_DENY: u32 = 8;
const A_STOP: u32 = 9;
const A_REFRESH: u32 = 10;
const A_TRACK0: u32 = 16;
/// Tracks a card lists (audio, video and caption together).
pub const MAX_TRACKS: u32 = 8;
const A_SEEK0: u32 = 24;
/// The seek bar is this many parts; part `i` seeks to `(i + 1) / SEEK_PARTS` of the length.
pub const SEEK_PARTS: u32 = 10;
const A_ROW0: u32 = 40;
/// The most files drawn at once (a row has the action `base + 40 + i`).
pub const MAX_ROWS: u32 = 48;
/// How many action numbers an audio view uses, from the base the app gave it.
pub const ACTIONS: u32 = A_ROW0 + MAX_ROWS;
/// What the back and forward buttons and the arrow keys move.
/// A pane this wide (inside its padding) puts the list beside the player.
const WIDE_AT: f32 = 560.0;
const JUMP_MS: u64 = 10_000;
const KEY_JUMP_MS: u64 = 5_000;
const RATES: [u32; 5] = [1000, 1250, 1500, 2000, 750];

pub struct AudioView {
    port: Box<dyn MediaPort>,
    base: u32,
    items: Vec<Item>,
    listed: bool,
    pub selected: usize,
    scroll: usize,
    info: Option<Info>,
    state: PlayState,
    pos_ms: u64,
    level: u32,
    shown_level: u32,
    volume: u32,
    muted: bool,
    rate_i: usize,
    error: Option<String>,
    approval: Option<String>,
    /// The file (or address) the session holds, for "next" and "previous".
    playing: Option<usize>,
    auto_next: bool,
    visible_rows: usize,
    /// The newest picture of a video and its sequence number.
    picture: Option<crate::canvas::Image>,
}

impl AudioView {
    pub fn new(port: Box<dyn MediaPort>, action_base: u32) -> AudioView {
        AudioView { port, base: action_base, items: Vec::new(), listed: false, selected: 0, scroll: 0, info: None, state: PlayState::Idle, pos_ms: 0, level: 0, shown_level: 0, volume: 100, muted: false, rate_i: 0, error: None, approval: None, playing: None, auto_next: true, visible_rows: 6, picture: None }
    }

    pub fn owns(&self, action: u32) -> bool {
        (self.base..self.base + ACTIONS).contains(&action)
    }

    /// Go on to the next file when one ends (on by default).
    pub fn set_auto_next(&mut self, on: bool) {
        self.auto_next = on;
    }

    // ---- what the view shows, for the app and for tests

    pub fn state(&self) -> PlayState {
        self.state
    }
    pub fn position_ms(&self) -> u64 {
        self.pos_ms
    }
    pub fn duration_ms(&self) -> Option<u64> {
        self.info.as_ref().and_then(|i| i.duration_ms)
    }
    pub fn title(&self) -> Option<&str> {
        self.info.as_ref().map(|i| i.title.as_str())
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub fn approval(&self) -> Option<&str> {
        self.approval.as_deref()
    }
    pub fn names(&self) -> Vec<String> {
        self.items.iter().map(|i| i.name.clone()).collect()
    }
    pub fn volume(&self) -> (u32, bool) {
        (self.volume, self.muted)
    }
    pub fn rate_permille(&self) -> u32 {
        RATES[self.rate_i]
    }
    /// The newest picture of a video (the core's, already RGBA); `None` for a sound.
    pub fn picture(&self) -> Option<&crate::canvas::Image> {
        self.picture.as_ref()
    }
    /// Whether the source that is open has a picture.
    pub fn has_video(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.has_video)
    }
    pub fn tracks(&self) -> Vec<Track> {
        self.info.as_ref().map(|i| i.tracks.clone()).unwrap_or_default()
    }

    // ---- life

    /// Lists the files the first time, then takes what the core says.
    pub fn tick(&mut self) {
        if !self.listed {
            self.refresh();
        }
        for e in self.port.poll() {
            self.apply(e);
        }
        // the picture that is due, when the source has one (the core picks it against the sound's clock)
        if self.has_video() {
            let after = self.picture.as_ref().map_or(0, |p| p.seq);
            match self.port.frame(after) {
                Ok(Some(img)) if img.check().is_ok() => self.picture = Some(img),
                Ok(_) => {}
                Err(e) => self.error = Some(e.sentence()),
            }
        }
        // the meter falls back a little each tick, and at once when nothing plays
        let fall = if self.state == PlayState::Playing { self.shown_level / 8 + 1 } else { self.shown_level / 3 + 1 };
        self.shown_level = self.shown_level.saturating_sub(fall);
    }

    fn refresh(&mut self) {
        match self.port.list_home() {
            Ok(l) => {
                self.items = l;
                self.listed = true;
                self.selected = self.selected.min(self.items.len().saturating_sub(1));
            }
            Err(e) => {
                self.listed = true;
                self.error = Some(e.sentence());
            }
        }
    }

    fn apply(&mut self, e: Event) {
        match e {
            Event::Approval(s) => self.approval = Some(s),
            Event::Opened(i) => {
                self.info = Some(i);
                self.error = None;
            }
            Event::State(s) => {
                self.state = s;
                if s != PlayState::AwaitingApproval {
                    self.approval = None;
                }
                if s == PlayState::Ended && self.auto_next {
                    if let Some(p) = self.playing {
                        if p + 1 < self.items.len() {
                            self.play_index(p + 1);
                        }
                    }
                }
            }
            Event::Position(ms) => self.pos_ms = ms,
            Event::Level(l) => {
                self.level = l.min(1000);
                self.shown_level = self.shown_level.max(self.level);
            }
            Event::Error(s) => self.error = Some(s),
        }
    }

    /// Start a source (a file the app knows, an address, a station). The core asks the person first for a remote one.
    pub fn open(&mut self, source: Source, title: &str) {
        self.pos_ms = 0;
        self.info = None;
        self.error = None;
        self.approval = None;
        self.level = 0;
        self.shown_level = 0;
        self.picture = None;
        match self.port.open(&source, title, true) {
            Ok(_) => {
                self.state = PlayState::Opening;
                // the port's own volume and mute are kept across files
                let _ = self.port.set_volume(self.volume);
                let _ = self.port.set_mute(self.muted);
                let _ = self.port.set_rate(RATES[self.rate_i]);
            }
            Err(e) => {
                self.state = PlayState::Failed;
                self.error = Some(e.sentence());
            }
        }
        // what the port said while opening
        for e in self.port.poll() {
            self.apply(e);
        }
    }

    pub fn play_index(&mut self, i: usize) {
        let Some(it) = self.items.get(i).cloned() else { return };
        self.selected = i;
        self.playing = Some(i);
        let title = it.name.rsplit('/').next().unwrap_or(&it.name).to_string();
        self.open(Source::Home(it.name), &title);
    }

    pub fn toggle(&mut self) {
        let r = match self.state {
            PlayState::Playing | PlayState::Buffering => self.port.pause(),
            PlayState::Ready | PlayState::Paused | PlayState::Ended => self.port.play(),
            PlayState::Idle | PlayState::Failed => {
                if !self.items.is_empty() {
                    let i = self.selected.min(self.items.len() - 1);
                    self.play_index(i);
                }
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(e) = r {
            self.error = Some(e.sentence());
        }
        self.drain();
    }

    fn drain(&mut self) {
        for e in self.port.poll() {
            self.apply(e);
        }
    }

    pub fn seek_by(&mut self, delta_ms: i64) {
        let Some(d) = self.duration_ms() else {
            self.error = Some("A live stream cannot be moved.".into());
            return;
        };
        let to = (self.pos_ms as i64 + delta_ms).clamp(0, d as i64) as u64;
        self.seek_to(to);
    }

    pub fn seek_to(&mut self, ms: u64) {
        match self.port.seek(ms) {
            Ok(()) => self.pos_ms = ms,
            Err(e) => self.error = Some(e.sentence()),
        }
        self.drain();
    }

    pub fn change_volume(&mut self, delta: i32) {
        self.volume = (self.volume as i32 + delta).clamp(0, 100) as u32;
        self.muted = false;
        let r = self.port.set_volume(self.volume).and_then(|_| self.port.set_mute(false));
        if let Err(e) = r {
            self.error = Some(e.sentence());
        }
    }

    pub fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        if let Err(e) = self.port.set_mute(self.muted) {
            self.error = Some(e.sentence());
        }
    }

    pub fn cycle_rate(&mut self) {
        self.rate_i = (self.rate_i + 1) % RATES.len();
        if let Err(e) = self.port.set_rate(RATES[self.rate_i]) {
            self.error = Some(e.sentence());
        }
    }

    pub fn stop(&mut self) {
        self.port.close();
        self.state = PlayState::Idle;
        self.info = None;
        self.pos_ms = 0;
        self.approval = None;
        self.playing = None;
        self.picture = None;
        self.level = 0;
        self.shown_level = 0;
    }

    fn step_file(&mut self, d: i32) {
        let n = self.items.len() as i32;
        if n == 0 {
            return;
        }
        let from = self.playing.unwrap_or(self.selected) as i32;
        self.play_index((from + d).rem_euclid(n) as usize);
    }

    fn select(&mut self, i: usize) {
        self.selected = i.min(self.items.len().saturating_sub(1));
        let rows = self.visible_rows.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + rows {
            self.scroll = self.selected + 1 - rows;
        }
    }

    pub fn action(&mut self, a: u32) -> bool {
        if !self.owns(a) {
            return false;
        }
        let r = a - self.base;
        match r {
            A_PLAY => self.toggle(),
            A_BACK => self.seek_by(-(JUMP_MS as i64)),
            A_FWD => self.seek_by(JUMP_MS as i64),
            A_MUTE => self.toggle_mute(),
            A_VOL_DOWN => self.change_volume(-10),
            A_VOL_UP => self.change_volume(10),
            A_RATE => self.cycle_rate(),
            A_ALLOW | A_DENY => {
                if let Err(e) = self.port.decide(r == A_ALLOW) {
                    self.error = Some(e.sentence());
                }
                self.drain();
            }
            A_STOP => self.stop(),
            A_REFRESH => self.refresh(),
            x if (A_TRACK0..A_TRACK0 + MAX_TRACKS).contains(&x) => {
                let id = self.tracks().get((x - A_TRACK0) as usize).map(|t| t.id);
                if let Some(id) = id {
                    if let Err(e) = self.port.select_track(id) {
                        self.error = Some(e.sentence());
                    } else if let Some(i) = self.info.as_mut() {
                        let kind = i.tracks.iter().find(|t| t.id == id).map(|t| t.kind);
                        for t in i.tracks.iter_mut().filter(|t| Some(t.kind) == kind) {
                            t.selected = t.id == id;
                        }
                    }
                }
            }
            x if (A_SEEK0..A_SEEK0 + SEEK_PARTS).contains(&x) => {
                if let Some(d) = self.duration_ms() {
                    self.seek_to(d * u64::from(x - A_SEEK0 + 1) / u64::from(SEEK_PARTS));
                }
            }
            x if x >= A_ROW0 => {
                let i = self.scroll + (x - A_ROW0) as usize;
                self.select(i);
                self.play_index(self.selected);
            }
            _ => {}
        }
        true
    }

    /// Keys; returns whether the key was used.
    pub fn key(&mut self, k: UiKey) -> bool {
        match k {
            UiKey::Char(' ') => self.toggle(),
            UiKey::Left => self.seek_by(-(KEY_JUMP_MS as i64)),
            UiKey::Right => self.seek_by(KEY_JUMP_MS as i64),
            UiKey::Up => self.select(self.selected.saturating_sub(1)),
            UiKey::Down => self.select(self.selected + 1),
            UiKey::Enter => self.play_index(self.selected),
            UiKey::Char('+') | UiKey::Char('=') => self.change_volume(10),
            UiKey::Char('-') => self.change_volume(-10),
            UiKey::Char('m') => self.toggle_mute(),
            UiKey::Char('r') => self.cycle_rate(),
            UiKey::Char('n') => self.step_file(1),
            UiKey::Char('p') => self.step_file(-1),
            UiKey::Esc if self.state != PlayState::Idle => self.stop(),
            _ => return false,
        }
        true
    }

    /// The wheel scrolls the list (positive: towards the top).
    pub fn wheel(&mut self, n: i32) {
        let max = self.items.len().saturating_sub(self.visible_rows.max(1));
        self.scroll = (self.scroll as i64 - n as i64).clamp(0, max as i64) as usize;
    }

    // ---- drawing

    pub fn draw(&mut self, c: &mut dyn Canvas, th: &Theme, m: &Metrics, area: Rect) {
        c.rrect(area, m.radius(Radius::Lg), th.panel);
        let md = m.gap(Space::Md);
        let inner = area.inset(md);
        let wide = inner.w >= WIDE_AT;
        let (player, list) = if wide {
            let (l, r) = inner.split_left((inner.w * 0.4).floor());
            (Rect::new(r.x + md, r.y, (r.w - md).max(0.0), r.h), Some(l))
        } else {
            (inner, None)
        };
        let used = self.draw_player(c, th, m, player);
        match list {
            Some(l) => self.draw_list(c, th, m, l),
            None => {
                let sm = m.gap(Space::Sm);
                let rest = Rect::new(inner.x, used + sm, inner.w, (inner.bottom() - used - sm).max(0.0));
                self.draw_list(c, th, m, rest);
            }
        }
    }

    /// Draws the title, seek bar, meter, transport, tracks and the approval card; returns the y where it ended.
    fn draw_player(&mut self, c: &mut dyn Canvas, th: &Theme, m: &Metrics, r: Rect) -> f32 {
        let t = m.min_target().max(m.size(TypeRole::Label) * 1.9);
        let (xs, sm, md, lg) = (m.gap(Space::Xs), m.gap(Space::Sm), m.gap(Space::Md), m.gap(Space::Lg));
        let mut y = r.y;
        // title and state
        let fs = m.size(TypeRole::Large);
        let title = self.info.as_ref().map(|i| i.title.clone()).unwrap_or_else(|| "No sound chosen".into());
        let word = self.state.word();
        let small = m.size(TypeRole::Small);
        let ww = c.measure(small, word, TextStyle::Regular);
        if y + fs * 1.4 <= r.bottom() {
            let tx = fit(c, fs, &title, TextStyle::Bold, (r.w - ww - lg).max(0.0));
            c.text(r.x, y, fs, th.text, &tx, TextStyle::Bold);
            let sw = ww;
            if r.w > sw + md {
                c.text(r.right() - sw, y + (fs - small).max(0.0) / 2.0, small, th.muted, word, TextStyle::Regular);
            }
            y += fs * 1.4 + xs;
        }
        // a refusal or a failure
        if let Some(e) = self.error.clone() {
            let s = m.size(TypeRole::Caption);
            if y + s * 1.4 <= r.bottom() {
                let tx = fit(c, s, &e, TextStyle::Regular, r.w);
                c.text(r.x, y, s, th.err, &tx, TextStyle::Regular);
                y += s * 1.4 + xs;
            }
        }
        // the approval card
        if let Some(s) = self.approval.clone() {
            let fs = m.size(TypeRole::Body);
            if y + fs * 1.4 + t + md <= r.bottom() {
                c.rrect(Rect::new(r.x, y, r.w, fs * 1.4 + t + md), m.radius(Radius::Md), th.card);
                let tx = fit(c, fs, &s, TextStyle::Regular, r.w - 2.0 * md);
                c.text(r.x + md, y + xs, fs, th.text, &tx, TextStyle::Regular);
                let bw = (r.w / 2.0 - lg).max(0.0);
                widgets::button(c, th, m, Rect::new(r.x + md, y + fs * 1.4 + xs, bw, t), "Allow", self.base + A_ALLOW, true);
                widgets::button(c, th, m, Rect::new(r.x + 2.0 * md + bw, y + fs * 1.4 + xs, bw, t), "Deny", self.base + A_DENY, false);
                y += fs * 1.4 + t + lg + xs;
            }
        }
        // the seek bar with the two times
        let tf = m.size(TypeRole::Caption);
        let bar_h = md;
        if y + t <= r.bottom() {
            let dur = self.duration_ms();
            let left_t = clock(self.pos_ms);
            let right_t = dur.map(clock).unwrap_or_else(|| "LIVE".into());
            let lw = c.measure(tf, &left_t, TextStyle::Mono).max(c.measure(tf, &right_t, TextStyle::Mono));
            // the times go on a line of their own when the bar would be squeezed
            let stacked = r.w < lw * 2.0 + m.em(8.0);
            let bar_x = if stacked { r.x } else { r.x + lw + md };
            let bar_w = if stacked { r.w } else { (r.w - 2.0 * lw - 2.0 * md).max(0.0) };
            let by = y + (t - bar_h) / 2.0;
            c.rrect(Rect::new(bar_x, by, bar_w, bar_h), bar_h / 2.0, th.well);
            if let Some(d) = dur.filter(|d| *d > 0) {
                let frac = (self.pos_ms as f32 / d as f32).clamp(0.0, 1.0);
                if frac > 0.0 {
                    c.rrect(Rect::new(bar_x, by, (bar_w * frac).max(bar_h).min(bar_w), bar_h), bar_h / 2.0, th.accent);
                }
                let seg = bar_w / SEEK_PARTS as f32;
                for i in 0..SEEK_PARTS {
                    let hr = Rect::new(bar_x + i as f32 * seg, y, seg, t);
                    c.hit(hr, Hit { action: self.base + A_SEEK0 + i, name: format!("Seek to {}%", (i + 1) * 100 / SEEK_PARTS), role: Role::Button, ..Default::default() });
                }
            } else {
                c.outline(Rect::new(bar_x, by, bar_w, bar_h), bar_h / 2.0, m.line(Line::Hair), th.border);
            }
            if stacked {
                y += t;
                c.text(r.x, y, tf, th.muted, &left_t, TextStyle::Mono);
                let rw = c.measure(tf, &right_t, TextStyle::Mono);
                c.text(r.right() - rw, y, tf, th.muted, &right_t, TextStyle::Mono);
                y += tf * 1.4 + xs;
            } else {
                c.text(r.x, y + (t - tf) / 2.0, tf, th.muted, &left_t, TextStyle::Mono);
                let rw = c.measure(tf, &right_t, TextStyle::Mono);
                c.text(r.right() - rw, y + (t - tf) / 2.0, tf, th.muted, &right_t, TextStyle::Mono);
                y += t + m.gap(Space::Xxs);
            }
        }
        // the level meter
        let mh = xs;
        if y + mh <= r.bottom() {
            c.rrect(Rect::new(r.x, y, r.w, mh), mh / 2.0, th.well);
            let w = r.w * self.shown_level.min(1000) as f32 * 0.001;
            if w > 0.5 {
                c.rrect(Rect::new(r.x, y, w.max(mh).min(r.w), mh), mh / 2.0, if self.shown_level > 900 { th.err } else { th.ok });
            }
            y += mh + sm;
        }
        // the transport buttons, wrapped
        let play_label = if matches!(self.state, PlayState::Playing | PlayState::Buffering) { "Pause" } else { "Play" };
        let rate = RATES[self.rate_i];
        let rate_label = if rate % 1000 == 0 { format!("{}x", rate / 1000) } else { format!("{:.2}x", rate as f32 * 0.001).trim_end_matches('0').to_string() };
        let mute_label = if self.muted { "Unmute" } else { "Mute" };
        let buttons: [(&str, u32, bool); 7] = [(play_label, A_PLAY, true), ("-10s", A_BACK, false), ("+10s", A_FWD, false), (mute_label, A_MUTE, false), ("Vol-", A_VOL_DOWN, false), ("Vol+", A_VOL_UP, false), (&rate_label, A_RATE, false)];
        let bf = m.size(TypeRole::Label);
        let mut x = r.x;
        let mut line_y = y;
        let mut any = false;
        for (label, a, primary) in buttons.iter() {
            let w = (c.measure(bf, label, TextStyle::Bold) + 2.0 * lg).max(m.min_target()).min(r.w);
            if x + w > r.right() + 0.5 && x > r.x {
                x = r.x;
                line_y += t + xs;
            }
            if line_y + t > r.bottom() {
                break;
            }
            widgets::button(c, th, m, Rect::new(x, line_y, w, t), label, self.base + a, *primary);
            x += w + xs;
            any = true;
        }
        if any {
            y = line_y + t + sm;
        }
        // the volume line
        let vs = m.size(TypeRole::Caption);
        if y + vs * 1.4 <= r.bottom() {
            let v = if self.muted { "Muted".to_string() } else { format!("Volume {}%", self.volume) };
            c.text(r.x, y, vs, th.muted, &v, TextStyle::Regular);
            if self.state != PlayState::Idle {
                let sw = c.measure(vs, "Stop", TextStyle::Bold) + 2.0 * lg;
                let sh = t.min(r.bottom() - y);
                if sh >= m.min_target() && r.w > sw + m.em(10.0) {
                    widgets::button(c, th, m, Rect::new(r.right() - sw, y - m.gap(Space::Xxs), sw, sh), "Stop", self.base + A_STOP, false);
                    y += sh + xs;
                } else {
                    y += vs * 1.4 + m.gap(Space::Xxs);
                }
            } else {
                y += vs * 1.4 + 2.0;
            }
        }
        // tracks: only when there is a choice
        let tracks = self.tracks();
        let choices = tracks.iter().filter(|t| t.kind == TrackKind::Audio).count() > 1 || tracks.iter().any(|t| t.kind == TrackKind::Caption);
        if choices {
            for (i, tr) in tracks.iter().enumerate().take(MAX_TRACKS as usize) {
                if y + t > r.bottom() {
                    break;
                }
                let kind = match tr.kind {
                    TrackKind::Audio => "Audio",
                    TrackKind::Video => "Video",
                    TrackKind::Caption => "Captions",
                };
                let lang = if tr.lang.is_empty() { String::new() } else { format!(" ({})", tr.lang) };
                widgets::row(c, th, m, Rect::new(r.x, y, r.w, t), &format!("{kind}: {}{lang}{}", tr.label, if tr.selected { ", on" } else { "" }), tr.selected, self.base + A_TRACK0 + i as u32);
                y += t + xs;
            }
        }
        y
    }

    fn draw_list(&mut self, c: &mut dyn Canvas, th: &Theme, m: &Metrics, la: Rect) {
        if la.h < m.gap(Space::Md) {
            return;
        }
        let t = m.min_target();
        let (xs, md, lg) = (m.gap(Space::Xs), m.gap(Space::Md), m.gap(Space::Lg));
        let rh = t.max(m.size(TypeRole::Label) * 1.8);
        let gap = m.gap(Space::Xs);
        let head = m.size(TypeRole::Small) * 1.5;
        let mut top = la.y;
        if la.h > head + rh {
            c.text(la.x, top, m.size(TypeRole::Small), th.muted, "Your sound files", TextStyle::Bold);
            top += head;
        }
        let rows = (((la.bottom() - top + gap) / (rh + gap)).floor().max(0.0) as usize).min(MAX_ROWS as usize);
        self.visible_rows = rows.max(1);
        if self.items.is_empty() {
            let s = m.size(TypeRole::Body);
            if top + s * 1.4 <= la.bottom() {
                let text = if self.error.is_some() { "" } else { "No sound files in your home folder." };
                c.text(la.x + m.gap(Space::Xs), top, s, th.dim, text, TextStyle::Regular);
            }
            return;
        }
        let max_scroll = self.items.len().saturating_sub(rows.max(1));
        self.scroll = self.scroll.min(max_scroll);
        for (i, it) in self.items.iter().enumerate().skip(self.scroll).take(rows) {
            let y = top + (i - self.scroll) as f32 * (rh + gap);
            let r = Rect::new(la.x, y, la.w, rh);
            let sel = i == self.selected;
            let now = self.playing == Some(i) && self.state != PlayState::Idle;
            c.rrect(r, m.radius(Radius::Md), if sel { th.card_sel } else { th.card });
            if now {
                c.rect(Rect::new(r.x, r.y + xs, xs, (r.h - 2.0 * xs).max(0.0)), th.accent);
            }
            let fs = m.size(TypeRole::Label);
            let tag = if it.video { "video" } else { "" };
            let mw = if tag.is_empty() { 0.0 } else { c.measure(fs * 0.8, tag, TextStyle::Regular) + lg };
            let tx = fit(c, fs, &it.name, if now { TextStyle::Bold } else { TextStyle::Regular }, (r.w - 2.0 * md - lg - mw).max(0.0));
            c.text(r.x + md + xs / 2.0, r.y + ((r.h - fs) / 2.0).max(0.0), fs, th.text, &tx, if now { TextStyle::Bold } else { TextStyle::Regular });
            if mw > 0.0 {
                c.text(r.right() - mw, r.y + ((r.h - fs * 0.8) / 2.0).max(0.0), fs * 0.8, th.dim, tag, TextStyle::Regular);
            }
            c.hit(r, Hit { action: self.base + A_ROW0 + (i - self.scroll) as u32, name: it.name.clone(), role: Role::ListItem, ..Default::default() });
        }
    }
}

/// "3:07" or "1:02:03".
pub fn clock(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}
