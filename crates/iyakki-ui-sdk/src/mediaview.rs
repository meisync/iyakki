//! `MediaView`: a sound and video player as a component an SDK app places in its layout (docs/development/UI-SDK.md, "Embedding audio and
//! video"). It is an [`AudioView`] with a picture: the video surface on top (the core's pictures, drawn letterboxed with [`Canvas::image`]),
//! the transport, the seek bar, the level meter, the track list and the files below it, or the picture alone with a control bar in full
//! screen. The core decodes the video and picks the picture that is due on the sound's clock; the view only draws the newest one it was
//! given, so a slow screen shows fewer pictures and never falls behind the sound.
//!
//! What the app does is what [`AudioView`] asks, plus `f` for full screen:
//! ```text
//! event(Tick):   media.tick()                                 event(Action): if media.owns(a) { media.action(a) }
//! event(Key):    if media.key(k) { return }                   view: media.draw(c, th, &metrics, rect)
//! ```
//! Keys: those of the audio view, `f` full screen, Esc leaves full screen first and then stops the player. Tap the picture to play or pause.
//! Captions: the list of caption tracks is shown and chosen; the caption text itself is not drawn yet (the facet carries no text).

use crate::audioview::{self, AudioView, ACTION_PLAY};
use crate::canvas::{contain, fit, Canvas, Hit, Role, TextStyle};
use crate::event::UiKey;
use crate::geom::Rect;
use crate::media::{MediaPort, PlayState, Source};
use crate::size::{Metrics, TypeRole};
use crate::theme::Theme;
use crate::tokens::{Radius, Space};
use crate::widgets;

const A_FULL: u32 = audioview::ACTIONS;
const A_SURFACE: u32 = audioview::ACTIONS + 1;
/// How many action numbers a media view uses, from the base the app gave it.
pub const ACTIONS: u32 = audioview::ACTIONS + 8;
/// The picture's height is this share of the width when no picture has come yet.
const DEFAULT_ASPECT: f32 = 0.5625;
/// The picture is at most this share of the view's height beside the controls.
const MAX_SURFACE_SHARE: f32 = 0.55;

pub struct MediaView {
    audio: AudioView,
    base: u32,
    pub fullscreen: bool,
}

impl MediaView {
    pub fn new(port: Box<dyn MediaPort>, action_base: u32) -> MediaView {
        MediaView { audio: AudioView::new(port, action_base), base: action_base, fullscreen: false }
    }

    pub fn owns(&self, action: u32) -> bool {
        (self.base..self.base + ACTIONS).contains(&action)
    }

    /// The player under the picture (state, position, files, title).
    pub fn audio(&self) -> &AudioView {
        &self.audio
    }

    pub fn audio_mut(&mut self) -> &mut AudioView {
        &mut self.audio
    }

    pub fn tick(&mut self) {
        self.audio.tick();
        if !self.audio.has_video() {
            self.fullscreen = false;
        }
    }

    pub fn open(&mut self, source: Source, title: &str) {
        self.audio.open(source, title);
    }

    pub fn wheel(&mut self, n: i32) {
        self.audio.wheel(n);
    }

    pub fn action(&mut self, a: u32) -> bool {
        if !self.owns(a) {
            return false;
        }
        match a - self.base {
            x if x == A_FULL => self.toggle_full(),
            x if x == A_SURFACE => self.audio.toggle(),
            _ => return self.audio.action(a),
        }
        true
    }

    fn toggle_full(&mut self) {
        self.fullscreen = self.audio.has_video() && !self.fullscreen;
    }

    pub fn key(&mut self, k: UiKey) -> bool {
        match k {
            UiKey::Char('f') if self.audio.has_video() => self.toggle_full(),
            UiKey::Esc if self.fullscreen => self.fullscreen = false,
            _ => return self.audio.key(k),
        }
        true
    }

    pub fn draw(&mut self, c: &mut dyn Canvas, th: &Theme, m: &Metrics, area: Rect) {
        if !self.audio.has_video() {
            self.audio.draw(c, th, m, area);
            return;
        }
        let md = m.gap(Space::Md);
        let t = m.min_target().max(m.size(TypeRole::Label) * 1.9);
        if self.fullscreen {
            c.rrect(area, m.radius(Radius::Lg), th.well);
            let bar_h = t + 2.0 * md;
            let (surface, bar) = area.split_bottom(bar_h.min(area.h));
            self.draw_surface(c, th, m, surface);
            let play = if matches!(self.audio.state(), PlayState::Playing | PlayState::Buffering) { "Pause" } else { "Play" };
            let bw = m.em(6.0).min(bar.w / 2.0);
            widgets::button(c, th, m, Rect::new(bar.x + md, bar.y + md, bw, t), play, self.base + ACTION_PLAY, true);
            let ew = (bw * 1.6).min((bar.w - bw - md * 2.0 - md).max(0.0));
            widgets::button(c, th, m, Rect::new(bar.right() - md - ew, bar.y + md, ew, t), "Exit full screen", self.base + A_FULL, false);
            return;
        }
        let share = ((area.w - 2.0 * md) * self.aspect()).min(area.h * MAX_SURFACE_SHARE).max(0.0);
        let (top, rest) = area.split_top((share + 2.0 * md).min(area.h));
        c.rrect(top, m.radius(Radius::Lg), th.panel);
        let surface = top.inset(md);
        self.draw_surface(c, th, m, surface);
        let fw = m.em(7.0).min(surface.w);
        if surface.h >= t && surface.w > fw {
            widgets::button(c, th, m, Rect::new(surface.right() - fw, surface.bottom() - t, fw, t), "Full screen", self.base + A_FULL, false);
        }
        self.audio.draw(c, th, m, rest);
    }

    fn aspect(&self) -> f32 {
        self.audio.picture().map_or(DEFAULT_ASPECT, |p| p.height as f32 / p.width.max(1) as f32)
    }

    fn draw_surface(&mut self, c: &mut dyn Canvas, th: &Theme, m: &Metrics, r: Rect) {
        c.rrect(r, m.radius(Radius::Md), th.well);
        match self.audio.picture() {
            Some(img) => {
                let pic = contain(r, img.width, img.height);
                c.image(pic, img);
            }
            None => {
                let fs = m.size(TypeRole::Body);
                let line = if self.audio.state() == PlayState::Failed { "The video could not be shown" } else { "Waiting for the first picture" };
                let tx = fit(c, fs, line, TextStyle::Regular, (r.w - 2.0 * m.gap(Space::Md)).max(0.0));
                if r.h >= fs * 1.5 {
                    c.text(r.x + m.gap(Space::Md), r.y + (r.h - fs) / 2.0, fs, th.muted, &tx, TextStyle::Regular);
                }
            }
        }
        // tapping the picture plays or pauses
        if r.h >= m.min_target() {
            c.hit(r, Hit { action: self.base + A_SURFACE, name: "Picture".into(), role: Role::Image, ..Default::default() });
        }
    }
}
