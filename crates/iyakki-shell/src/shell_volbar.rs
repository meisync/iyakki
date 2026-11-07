//! The top bar's Home, Settings and speaker icons, on every screen that has a bar: the home screen, every app's header and the
//! workspace bar of the windowed and multi-display layouts. One list (`Extra`, `wanted_extras`), one drawing (`draw_extra`) and one
//! action each (`go_home`, `open_settings_from_bar`, the volume code below) serve all of them.
//!
//! The home screen's bar: the Settings icon and the speaker icon (left of the other icons).
//!
//! * Settings opens the SETTINGS app with the permissions of a tile (the tile is no longer on the grid; F7 and the IYAKKI menu
//!   (F10, then `s`) lead there too, and so does the key `s` on the home screen).
//! * The speaker shows IYAKKI's one master volume (the value the volume keys, Settings -> Sound and the Devices popover use; there
//!   is no second volume): crossed when muted, one to three waves for low, medium and high, slashed when there is no sound output.
//!   A click mutes or unmutes (the level is kept, like the mute key), the mouse wheel over it changes the volume by 5 % per notch,
//!   and pressing and holding it (or moving the pointer while it is pressed) shows a slider to drag. The key `v` opens the same
//!   slider for the keyboard (Up/Down or +/- change it by 5 %, `m`, Space or Enter mute, Esc closes). Hovering shows the percent.
//!   Every change shows the same on-screen bar as the volume keys, and a missing output says "No sound output" instead.

use crate::safeclamp::SafeClamp;
use super::*;
use crate::fnkeys::{self, VolOp};
use iyakki_agent::audioout::SoundSettings;

/// Events of the two regions this file registers (`Event::Area(n)`).
const AREA_ICON: usize = 0x4D00;
const AREA_POP: usize = 0x4D01;
/// Heartbeats a press on the icon is held before the slider shows (a third of a second).
const HOLD_TICKS: u64 = 10;
/// Clicks of the Home and Settings icons (`Event::Tab`).
pub(super) const HOME_TAB: usize = 0x4D10;
pub(super) const GEAR_TAB: usize = 0x4D11;
/// The three buttons in the speaker's dropdown (`Event::Tab`): louder, quieter, mute or unmute.
const BTN_UP: usize = 0x4D12;
const BTN_DOWN: usize = 0x4D13;
const BTN_MUTE: usize = 0x4D14;
/// The per-sound rows under them (`Event::Tab`): the quieter button of row `i` is `BTN_SRC + 2 * i`, the louder one the next.
const BTN_SRC: usize = 0x4D20;
/// Most sounds listed, and the height of one row and of the heading above them (pixels of a 1.0 scale screen).
const SRC_MAX: usize = 4;
const SRC_ROW: f32 = 34.0;
const SRC_HEAD: f32 = 26.0;
/// A step of a sound's own level, percent.
const SRC_STEP: u32 = 10;
/// The slider strip at the left of the dropdown (pixels of a 1.0 scale screen); the buttons stand right of it.
const STRIP: f32 = 56.0;

/// The icons every top bar shares, in the order they stand on the bar from left to right. When the bar is too narrow the leftmost go
/// first: Home is kept longest, then the speaker, then Settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extra {
    Settings,
    Speaker,
    Home,
}

/// Where one of them was drawn in the last frame, on which display and how big that display is.
#[derive(Clone, Copy, Debug)]
pub(super) struct Placed {
    pub disp: usize,
    pub extra: Extra,
    pub rect: R,
    pub size: (u32, u32),
}

/// How far (pixels of a 1.0 scale screen) the pointer moves with the icon pressed before it counts as a drag.
const DRAG_PX: f32 = 4.0;

/// What the speaker icon shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speaker {
    NoOutput,
    Muted,
    Low,
    Medium,
    High,
}

impl Speaker {
    /// The state for a volume: muted (or 0 %) wins; no sound output beats everything.
    pub fn of(has_output: bool, s: &SoundSettings) -> Speaker {
        if !has_output {
            Speaker::NoOutput
        } else if s.muted || s.volume_pct == 0 {
            Speaker::Muted
        } else if s.volume_pct <= 33 {
            Speaker::Low
        } else if s.volume_pct <= 66 {
            Speaker::Medium
        } else {
            Speaker::High
        }
    }

    pub fn icon(self) -> Icon {
        Icon::Volume(match self {
            Speaker::Muted => 0,
            Speaker::Low => 1,
            Speaker::Medium => 2,
            Speaker::High => 3,
            Speaker::NoOutput => 4,
        })
    }

    /// The lines of the hover text.
    pub fn tip(self, s: &SoundSettings) -> Vec<(String, bool)> {
        match self {
            Speaker::NoOutput => vec![(fnkeys_impl::NO_SOUND.to_string(), true), ("Open Settings, Sound, to choose an output".to_string(), false)],
            Speaker::Muted => vec![(format!("Muted (volume {}%)", s.volume_pct.min(100)), false), ("Click to unmute".to_string(), false)],
            _ => vec![(format!("Volume {}%", s.volume_pct.min(100)), false), ("Click to mute, wheel or drag to change".to_string(), false)],
        }
    }
}

/// Where the icons sit in the top bar: indexes for `header_button`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BarSlots {
    pub speaker: Option<usize>,
    pub settings: Option<usize>,
    /// How many icons the bar has in all.
    pub count: usize,
}

/// The popover's panel and the slider track inside it, under the icon `icon` of a screen `w` wide. The slider stands in the left
/// strip of the panel ([`STRIP`] wide), the buttons ([`pop_buttons`]) right of it.
pub(super) fn pop_rects(icon: R, w: u32, s: f32, rows: usize) -> (R, R) {
    let (pw, ph) = (176.0 * s, 196.0 * s);
    // the slider keeps its size; the panel grows downwards for the list of sounds
    let extra = if rows == 0 { 0.0 } else { (SRC_HEAD + rows.min(SRC_MAX) as f32 * SRC_ROW) * s };
    let x = (icon.x + icon.w / 2.0 - pw / 2.0).clamp_safe(8.0 * s, (w as f32 - pw - 8.0 * s).max(8.0 * s));
    let y = icon.y + icon.h + 8.0 * s;
    let panel = R::new(x, y, pw, ph + extra);
    let track = R::new(x + STRIP * s / 2.0 - 6.0 * s, y + 20.0 * s, 12.0 * s, ph - 20.0 * s - 44.0 * s);
    (panel, track)
}

/// The dropdown's three buttons, top to bottom: Louder, Quieter, Mute or Unmute.
pub(super) fn pop_buttons(panel: R, s: f32) -> [R; 3] {
    let (x, w, h, gap) = (panel.x + STRIP * s, panel.w - STRIP * s - 10.0 * s, 40.0 * s, 12.0 * s);
    let top = panel.y + (196.0 * s - (3.0 * h + 2.0 * gap)) / 2.0;
    [R::new(x, top, w, h), R::new(x, top + h + gap, w, h), R::new(x, top + 2.0 * (h + gap), w, h)]
}

/// The percent for a pointer at height `y` on the track (the bottom is 0, the top 100).
pub(super) fn pct_at(track: R, y: f32) -> u32 {
    (((track.y + track.h - y) / track.h) * 100.0).round().clamp(0.0, 100.0) as u32
}

#[derive(Default)]
pub(super) struct VolUi {
    /// The slider popover is showing.
    pub open: bool,
    /// It was opened with the keyboard: it stays until Esc or a click elsewhere (else it closes when the button is let go).
    pub kb: bool,
    /// The icon is pressed: (heartbeat of the press, where, whether the pointer has moved).
    pub press: Option<(u64, (f32, f32), bool)>,
    /// The press on the icon closed a dropdown that was open: its release must not open it again.
    pub closing: bool,
    /// The icons drawn in the last frame (every display's bar).
    pub rects: Vec<Placed>,
    /// The sounds playing, with the person's level for each (percent), listed under the buttons; read when the dropdown opens and
    /// again about twice a second while it is open.
    pub sources: Vec<(String, u32)>,
}

impl Shell {
    pub(super) fn bar_slots(&self) -> BarSlots {
        let Some(sess) = self.session.as_ref().filter(|_| !self.panes.compact) else { return BarSlots { speaker: None, settings: None, count: 0 } };
        let base = 4 + usize::from(sess.policy.power) + usize::from(sess.policy.allows_app(PApp::Inbox)) + usize::from(self.panes_button());
        let mut n = base;
        let mut next = |on: bool| on.then(|| {
            n += 1;
            n - 1
        });
        let speaker = next(!self.cloud.fn_keys_off);
        let settings = next(!self.cloud.hides_app(PApp::Settings));
        BarSlots { speaker, settings, count: n }
    }

    fn bar_icon_rect(&self, slot: Option<usize>) -> Option<R> {
        let (w, _) = self.size;
        slot.map(|i| header_button(w, scale_for(w, self.size.1), i))
    }

    /// The icon `e` on the bar of the working display, as last drawn (the home screen's bar before the first frame).
    fn extra_entry(&self, e: Extra) -> Option<(R, (u32, u32))> {
        let me = self.panes_active_display();
        if let Some(p) = self.vol.rects.iter().find(|p| p.extra == e && p.disp == me) {
            return Some((p.rect, p.size));
        }
        if !matches!(self.screen, Screen::Launcher) {
            return None;
        }
        let slots = self.bar_slots();
        let slot = match e {
            Extra::Speaker => slots.speaker,
            Extra::Settings => slots.settings,
            Extra::Home => None,
        };
        self.bar_icon_rect(slot).map(|r| (r, self.size))
    }

    #[cfg(test)]
    pub(super) fn speaker_rect(&self) -> Option<R> {
        self.extra_entry(Extra::Speaker).map(|e| e.0)
    }

    pub(super) fn settings_rect(&self) -> Option<R> {
        self.extra_entry(Extra::Settings).map(|e| e.0)
    }

    #[cfg(test)]
    pub(super) fn home_rect(&self) -> Option<R> {
        self.extra_entry(Extra::Home).map(|e| e.0)
    }

    /// The icons a bar shows now, left to right: Settings unless cloud mode hides it, the speaker unless cloud mode switches the
    /// sound keys off, and Home when the screen is not the tiles already (`home_applies`).
    pub(super) fn wanted_extras(&self, home_applies: bool) -> Vec<Extra> {
        if self.session.is_none() {
            return Vec::new();
        }
        let mut v = Vec::new();
        if !self.cloud.hides_app(PApp::Settings) {
            v.push(Extra::Settings);
        }
        if !self.cloud.fn_keys_off {
            v.push(Extra::Speaker);
        }
        if home_applies {
            v.push(Extra::Home);
        }
        v
    }

    /// Whether Home does something on the working display's focused screen.
    pub(super) fn home_applies(&self) -> bool {
        !matches!(self.screen, Screen::Launcher)
    }

    /// Put the icons that fit between `limit` and the right cluster of an app header or workspace bar, right to left from `right`
    /// (the left edge of the cluster), each `d` wide and none left of `limit` (Home may go on down to `home_floor`) with `gap` between. Home is placed first, so it is the last to give way.
    pub(super) fn place_extras(&self, right: f32, limit: f32, home_floor: f32, d: f32, gap: f32, home_applies: bool) -> Vec<(Extra, f32)> {
        let wanted = self.wanted_extras(home_applies);
        let mut out = Vec::new();
        let mut x = right;
        for e in wanted.iter().rev() {
            if x - d < if *e == Extra::Home { home_floor } else { limit } {
                break;
            }
            x -= d;
            out.push((*e, x));
            x -= gap;
        }
        out.reverse();
        out
    }

    /// Draw one shared icon in `r` and register its click; remembers where it stands for the events and the tooltips.
    pub(super) fn draw_extra(&mut self, g: &mut Gfx, e: Extra, r: R, s: f32, accent: [u8; 4], icon_px: f32) {
        let size = (g.width(), g.height());
        self.vol.rects.push(Placed { disp: self.panes_drawing_display(), extra: e, rect: r, size });
        let hot = g.hovered(r);
        match e {
            Extra::Home => {
                crate::fx::chrome_button(g, r, s, hot, accent);
                g.icon(Icon::Home, r.x + r.w / 2.0, r.y + r.h / 2.0, icon_px, MUTED);
                g.hit(r, Event::Tab(HOME_TAB), "home");
            }
            Extra::Settings => {
                let allowed = self.session.as_ref().is_some_and(|x| x.policy.allows_app(PApp::Settings));
                let here = matches!(self.screen, Screen::App(_, PApp::Settings));
                crate::fx::chrome_button(g, r, s, hot || here, accent);
                g.icon(Icon::Settings, r.x + r.w / 2.0, r.y + r.h / 2.0, icon_px, if here { crate::fx::CYAN } else if allowed { MUTED } else { DIM });
                g.hit(r, Event::Tab(GEAR_TAB), "settings");
            }
            Extra::Speaker => {
                let st = self.speaker_state();
                crate::fx::chrome_button(g, r, s, hot || self.vol.open, accent);
                let ink = match st {
                    Speaker::NoOutput => DIM,
                    Speaker::Muted => rgb(255, 196, 94),
                    _ if self.vol.open => crate::fx::CYAN,
                    _ => MUTED,
                };
                g.icon(st.icon(), r.x + r.w / 2.0, r.y + r.h / 2.0, icon_px, ink);
                g.hit(r, Event::Area(AREA_ICON), "volume");
            }
        }
    }

    /// Back to the tiles from any app, the way Back leaves an app (the app is closed like Back closes it; what runs in the
    /// background, the radio, MEDIA, flows and jobs, goes on). An app kept aside (the inbox opened from it) is left as well.
    pub(super) fn go_home(&mut self) {
        if self.session.is_none() {
            return;
        }
        self.vol_close();
        self.devpop.close();
        self.brand.close();
        self.installs.close();
        self.help = false;
        let mut n = 0;
        while matches!(self.screen, Screen::App(..)) && n < 4 {
            self.leave_app();
            n += 1;
        }
        if let Some(f) = self.panes_focus_index() {
            self.ws_mark_dirty(f);
        }
        self.stale = true;
    }

    fn has_sound_output(&self) -> bool {
        self.fnk.sound_probe.unwrap_or_else(fnkeys::sound_present)
    }

    /// What the speaker shows now.
    pub fn speaker_state(&self) -> Speaker {
        Speaker::of(self.has_sound_output(), &self.env.services.sound_settings())
    }

    /// The slider popover is on screen (for tests and the drawing).
    pub fn volume_slider_open(&self) -> bool {
        self.vol.open
    }

    fn vol_close(&mut self) {
        if self.vol.open || self.vol.press.is_some() {
            self.stale = true;
        }
        self.vol = VolUi::default();
    }

    fn vol_toggle_mute(&mut self) {
        self.fn_volume_with(|s| fnkeys::volume_apply(s, VolOp::Mute));
    }

    /// Read which sounds are playing and the person's level for each (the dropdown lists them).
    pub(super) fn vol_refresh_sources(&mut self) {
        let mut now = iyakki_agent::mixer::active_sources();
        now.truncate(SRC_MAX);
        if now != self.vol.sources {
            self.vol.sources = now;
            self.stale = true;
        }
    }

    /// Row `i` of the list of sounds: its own level goes one step louder or quieter (it multiplies the master volume; 100 is the full
    /// level and is kept as "no setting").
    fn vol_source_step(&mut self, i: usize, louder: bool) {
        let Some((label, pct)) = self.vol.sources.get(i).cloned() else { return };
        let now = if louder { (pct + SRC_STEP).min(100) } else { pct.saturating_sub(SRC_STEP) };
        let name = label.clone();
        match self.env.services.update_sound(move |st| {
            if now >= 100 {
                st.source_levels.remove(&name);
            } else {
                st.source_levels.insert(name, now);
            }
        }) {
            Ok(_) => {
                if let Some(row) = self.vol.sources.get_mut(i) {
                    row.1 = now;
                }
            }
            Err(e) => self.say(e),
        }
    }

    /// The slider moved to height `y`: the master volume becomes that percent (and the sound comes on).
    fn vol_to_y(&mut self, y: f32) {
        let Some((icon, (w, h))) = self.extra_entry(Extra::Speaker) else { return };
        let (_, track) = pop_rects(icon, w, scale_for(w, h), self.vol.sources.len());
        let pct = pct_at(track, y);
        let now = self.env.services.sound_settings();
        if now.volume_pct == pct && !now.muted {
            return;
        }
        self.fn_volume_with(|s| {
            s.volume_pct = pct;
            s.muted = false;
        });
    }

    /// Open Settings from the top bar (the permissions of the tile). From an app, that app is kept aside, still running, and comes
    /// back with Back (like the inbox); in Settings the icon stays where it is.
    pub(super) fn open_settings_from_bar(&mut self) {
        self.vol_close();
        match &self.screen {
            Screen::App(_, PApp::Settings) => return,
            Screen::Launcher => {
                self.open(PApp::Settings);
                return;
            }
            Screen::App(..) => {}
            _ => return,
        }
        let prev = std::mem::replace(&mut self.screen, Screen::Launcher);
        if let Screen::App(app, pa) = prev {
            if self.stash.is_none() {
                self.stash = Some((app, pa));
            }
        }
        self.open(PApp::Settings);
        if !matches!(self.screen, Screen::App(_, PApp::Settings)) {
            self.leave_app(); // refused: the app comes back
        }
        self.stale = true;
    }

    /// Pointer and key events for the speaker; true when taken.
    pub(super) fn vol_event(&mut self, ev: &Event) -> bool {
        if self.session.is_none() || self.panes.compact {
            return false;
        }
        let Some((icon, (w, h))) = self.extra_entry(Extra::Speaker) else { return false };
        let s = scale_for(w, h);
        let (panel, _) = pop_rects(icon, w, s, self.vol.sources.len());
        match ev {
            // the wheel over the icon (or the slider) changes the volume, 5 % a notch, louder upwards
            Event::Scroll(d) if *d != 0 && (icon.contains(self.mouse.0, self.mouse.1) || (self.vol.open && panel.contains(self.mouse.0, self.mouse.1))) => {
                let op = if *d > 0 { VolOp::Up } else { VolOp::Down };
                let n = d.unsigned_abs().min(20);
                self.fn_volume_with(|st| (0..n).for_each(|_| fnkeys::volume_apply(st, op)));
                true
            }
            // a press on the icon: a click mutes, holding or moving shows the slider and follows the pointer
            Event::AreaAt(AREA_ICON, dx, dy) => {
                let at = (icon.x + dx, icon.y + dy);
                match self.vol.press {
                    None => {
                        self.vol.press = Some((self.tick, at, false));
                        if self.vol.open && self.vol.kb {
                            self.vol_close(); // the dropdown a click or the keyboard opened goes when the icon is pressed again
                            self.vol.press = Some((self.tick, at, false));
                            self.vol.closing = true;
                        }
                    }
                    Some((t, from, moved)) => {
                        let far = (at.0 - from.0).abs().max((at.1 - from.1).abs()) > DRAG_PX * s;
                        if far || moved {
                            if self.has_sound_output() {
                                self.vol.open = true;
                                self.vol_to_y(at.1);
                            } else {
                                self.fn_notice(fnkeys_impl::NO_SOUND);
                            }
                            self.vol.press = Some((t, from, true));
                        }
                    }
                }
                self.stale = true;
                true
            }
            // a press or drag on the slider itself
            Event::AreaAt(AREA_POP, dx, dy) => {
                if *dx <= STRIP * s {
                    self.vol_to_y(panel.y + dy);
                }
                self.stale = true;
                true
            }
            // the dropdown's buttons
            Event::Tab(t) if self.vol.open && (BTN_SRC..BTN_SRC + 2 * SRC_MAX).contains(t) => {
                self.vol_source_step((*t - BTN_SRC) / 2, (*t - BTN_SRC) % 2 == 1);
                self.stale = true;
                true
            }
            Event::Tab(BTN_UP) if self.vol.open => {
                self.fn_volume(VolOp::Up);
                self.stale = true;
                true
            }
            Event::Tab(BTN_DOWN) if self.vol.open => {
                self.fn_volume(VolOp::Down);
                self.stale = true;
                true
            }
            Event::Tab(BTN_MUTE) if self.vol.open => {
                self.vol_toggle_mute();
                self.stale = true;
                true
            }
            Event::MouseUp(..) => {
                let Some((_, _, moved)) = self.vol.press.take() else { return false };
                let shown = self.vol.open;
                let closing = std::mem::take(&mut self.vol.closing);
                if !moved && !shown && !closing {
                    // a plain click opens the dropdown: the slider, Louder, Quieter and Mute (it stays until Esc or a click elsewhere)
                    if self.has_sound_output() {
                        self.vol.open = true;
                        self.vol.kb = true;
                    } else {
                        self.fn_notice(fnkeys_impl::NO_SOUND);
                    }
                }
                if !self.vol.kb {
                    self.vol.open = false;
                }
                self.stale = true;
                true
            }
            Event::Tick => {
                if self.vol.open && self.tick % 15 == 0 {
                    self.vol_refresh_sources();
                }
                if let Some((t, _, false)) = self.vol.press {
                    if !self.vol.open && self.tick.saturating_sub(t) >= HOLD_TICKS && self.has_sound_output() {
                        self.vol.open = true; // held: show the slider
                        self.stale = true;
                    }
                }
                false
            }
            // a click anywhere else closes the slider the keyboard opened, and goes on to do what it does
            Event::MouseDown(x, y) if self.vol.open && !panel.contains(*x, *y) && !icon.contains(*x, *y) => {
                self.vol_close();
                false
            }
            Event::Key(k) if self.vol.open => {
                match k {
                    Key::Up | Key::Right | Key::Char('+') | Key::Char('=') => self.fn_volume(VolOp::Up),
                    Key::Down | Key::Left | Key::Char('-') => self.fn_volume(VolOp::Down),
                    Key::PageUp => self.fn_volume_with(|st| (0..2).for_each(|_| fnkeys::volume_apply(st, VolOp::Up))),
                    Key::PageDown => self.fn_volume_with(|st| (0..2).for_each(|_| fnkeys::volume_apply(st, VolOp::Down))),
                    Key::Home => self.fn_volume_with(|st| {
                        st.volume_pct = 0;
                        st.muted = false;
                    }),
                    Key::End => self.fn_volume_with(|st| {
                        st.volume_pct = 100;
                        st.muted = false;
                    }),
                    Key::Char('m') | Key::Char(' ') | Key::Enter => self.vol_toggle_mute(),
                    Key::Esc | Key::Char('v') | Key::Tab | Key::BackTab => self.vol_close(),
                    _ => {}
                }
                self.stale = true;
                true
            }
            // the keyboard's way to the speaker
            Event::Key(Key::Char('v')) if matches!(self.screen, Screen::Launcher) && !self.lay.modal() => {
                if self.has_sound_output() {
                    self.vol.open = true;
                    self.vol.kb = true;
                    self.stale = true;
                } else {
                    self.fn_notice(fnkeys_impl::NO_SOUND);
                }
                true
            }
            _ => false,
        }
    }

    /// Draw the Settings icon and the speaker in their slots on the home screen's bar, and register the speaker's region.
    pub(super) fn draw_bar_extras(&mut self, g: &mut Gfx, s: f32) {
        let slots = self.bar_slots();
        let (w, _) = self.size;
        let allowed = self.session.as_ref().is_some_and(|x| x.policy.allows_app(PApp::Settings));
        let size = (g.width(), g.height());
        if let Some(i) = slots.settings {
            let r = header_button(w, s, i);
            self.vol.rects.push(Placed { disp: self.panes_drawing_display(), extra: Extra::Settings, rect: r, size });
            crate::fx::chrome_button(g, r, s * 2.5, g.hovered(r), crate::fx::CYAN);
            g.icon(Icon::Settings, r.x + r.w / 2.0, r.y + r.h / 2.0, 22.0 * s, if allowed { MUTED } else { DIM });
        }
        if let Some(i) = slots.speaker {
            let r = header_button(w, s, i);
            self.vol.rects.push(Placed { disp: self.panes_drawing_display(), extra: Extra::Speaker, rect: r, size });
            let st = self.speaker_state();
            crate::fx::chrome_button(g, r, s * 2.5, g.hovered(r) || self.vol.open, crate::fx::CYAN);
            let ink = match st {
                Speaker::NoOutput => DIM,
                Speaker::Muted => rgb(255, 196, 94),
                _ if self.vol.open => crate::fx::CYAN,
                _ => MUTED,
            };
            g.icon(st.icon(), r.x + r.w / 2.0, r.y + r.h / 2.0, 22.0 * s, ink);
            g.hit(r, Event::Area(AREA_ICON), "volume");
        }
    }

    /// The slider popover and the hover texts of the icons of the display being drawn, drawn last so nothing covers them.
    pub(super) fn draw_bar_overlay(&mut self, g: &mut Gfx, s: f32) {
        let (w, h) = (g.width(), g.height());
        let me = self.panes_drawing_display();
        let placed: Vec<Placed> = self.vol.rects.iter().filter(|p| p.disp == me).copied().collect();
        for p in placed {
            let icon = p.rect;
            match p.extra {
                Extra::Speaker => {
                    let sound = self.env.services.sound_settings();
                    let st = self.speaker_state();
                    if self.vol.open && me == self.panes_active_display() {
                        let (panel, track) = pop_rects(icon, w, s, self.vol.sources.len());
                        crate::fx::modal_panel(g, panel, s * 0.6, crate::fx::CYAN, false);
                        let frac = if sound.muted { 0.0 } else { sound.volume_pct.min(100) as f32 / 100.0 };
                        g.rrect(track.x, track.y, track.w, track.h, track.w / 2.0, rgb(40, 48, 78));
                        let fh = (track.h * frac).max(if frac > 0.0 { track.w } else { 0.0 });
                        if fh > 0.0 {
                            g.rrect(track.x, track.y + track.h - fh, track.w, fh, track.w / 2.0, crate::fx::CYAN);
                        }
                        let ky = track.y + track.h - track.h * frac;
                        g.circle(track.x + track.w / 2.0, ky, 9.0 * s, TEXT);
                        let label = if matches!(st, Speaker::Muted) { "Muted".to_string() } else { format!("{}%", sound.volume_pct.min(100)) };
                        let lw = g.text_width(13.0 * s, &label, Face::Bold);
                        g.text(panel.x + (STRIP * s - lw) / 2.0, panel.y + 196.0 * s - 30.0 * s, 13.0 * s, TEXT, &label, Face::Bold);
                        g.hit(panel, Event::Area(AREA_POP), "volume slider");
                        let muted = matches!(st, Speaker::Muted);
                        for (r, text, tab, name) in pop_buttons(panel, s).into_iter().zip([("Louder  +", BTN_UP, "volume louder"), ("Quieter  -", BTN_DOWN, "volume quieter"), (if muted { "Unmute" } else { "Mute" }, BTN_MUTE, "volume mute")]).map(|(r, (t, e, n))| (r, t, e, n)) {
                            let hot = g.hovered(r);
                            let ink = crate::fx::pill(g, r, s, crate::fx::CYAN, hot, true);
                            let tw = g.text_width(13.0 * s, text, Face::Bold);
                            g.text(r.x + (r.w - tw) / 2.0, r.y + (r.h - 13.0 * s) / 2.0 - 1.0 * s, 13.0 * s, ink, text, Face::Bold);
                            g.hit(r, Event::Tab(tab), name);
                        }
                        if !self.vol.sources.is_empty() {
                            let y0 = panel.y + 196.0 * s;
                            g.rect(panel.x + 12.0 * s, y0 - 2.0 * s, panel.w - 24.0 * s, 1.0, [255, 255, 255, 30]);
                            g.text(panel.x + 14.0 * s, y0 + 5.0 * s, 12.0 * s, MUTED, "Sounds", Face::Bold);
                            for (i, (name, pct)) in self.vol.sources.iter().take(SRC_MAX).enumerate() {
                                let ry = y0 + (SRC_HEAD + i as f32 * SRC_ROW) * s;
                                let shown: String = name.chars().next().map(|c| c.to_uppercase().collect::<String>() + &name[c.len_utf8()..]).unwrap_or_default();
                                let shown = g.fit(13.0 * s, &shown, Face::Bold, 62.0 * s);
                                g.text(panel.x + 14.0 * s, ry + 7.0 * s, 13.0 * s, TEXT, &shown, Face::Bold);
                                let (bw, bh) = (28.0 * s, 26.0 * s);
                                let down = R::new(panel.x + panel.w - 12.0 * s - 2.0 * bw - 46.0 * s, ry, bw, bh);
                                let up = R::new(down.x + bw + 46.0 * s, ry, bw, bh);
                                for (r, text, tab, hit) in [(down, "-", BTN_SRC + 2 * i, "sound quieter"), (up, "+", BTN_SRC + 2 * i + 1, "sound louder")] {
                                    let hot = g.hovered(r);
                                    let ink = crate::fx::pill(g, r, s, crate::fx::CYAN, hot, true);
                                    let tw = g.text_width(14.0 * s, text, Face::Bold);
                                    g.text(r.x + (r.w - tw) / 2.0, r.y + (r.h - 14.0 * s) / 2.0 - 1.0 * s, 14.0 * s, ink, text, Face::Bold);
                                    g.hit(r, Event::Tab(tab), hit);
                                }
                                let pl = format!("{pct}%");
                                let pw = g.text_width(12.0 * s, &pl, Face::Sans);
                                g.text(down.x + bw + (46.0 * s - pw) / 2.0, ry + 7.0 * s, 12.0 * s, MUTED, &pl, Face::Sans);
                            }
                        }
                    } else if g.hovered(icon) && self.vol.press.is_none() {
                        draw_clock_tip(g, w, h, s, icon, &st.tip(&sound));
                    }
                }
                Extra::Settings => {
                    if g.hovered(icon) {
                        let denied = !self.session.as_ref().is_some_and(|x| x.policy.allows_app(PApp::Settings));
                        let here = matches!(self.screen, Screen::App(_, PApp::Settings));
                        let lines = if denied {
                            vec![("Settings".to_string(), false), ("Your role can't open Settings".to_string(), true)]
                        } else if here {
                            vec![("Settings".to_string(), false), ("You are in Settings".to_string(), false)]
                        } else {
                            vec![("Settings".to_string(), false), ("Models, network, sound, display and more (s on the home screen)".to_string(), false)]
                        };
                        draw_clock_tip(g, w, h, s, icon, &lines);
                    }
                }
                Extra::Home => {
                    if g.hovered(icon) {
                        draw_clock_tip(g, w, h, s, icon, &[("Home".to_string(), false), ("Back to the tiles (F11)".to_string(), false)]);
                    }
                }
            }
        }
    }
}
