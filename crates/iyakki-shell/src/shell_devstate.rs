//! The shell's side of `iyakki dev state` and `iyakki dev input click`: while a paired computer is asking (the device sets "wanted" for a
//! few seconds at a time, and only while developer mode is on), the frame is drawn with text capture on and handed over with the click
//! regions and the text fields. The safety rules (secret fields, private apps, credentials, limits) are applied by
//! `iyakki_agent::devmode::state::build`, not here.

use super::*;
use iyakki_agent::devmode::state::{self, Raw, RawField, Rect};

impl Shell {
    /// A frame is about to be drawn: switch text capture on when a computer asked, and say the shell is alive.
    pub(super) fn dev_state_begin(&self, g: &mut Gfx) -> bool {
        let dir = self.env.services.dir();
        state::heartbeat(dir);
        let want = state::wanted(dir);
        g.text_cap = want.then(Vec::new);
        want
    }

    /// The frame is drawn (`self.hits` and `self.fields` are this frame's): hand it over.
    pub(super) fn dev_state_end(&self, g: &mut Gfx) {
        let texts = g.text_cap.take().unwrap_or_default();
        let (app, page) = match &self.screen {
            Screen::Launcher => ("Home".to_string(), String::new()),
            Screen::Login(_) => ("Sign in".to_string(), String::new()),
            Screen::Setup(_) => ("Setup".to_string(), String::new()),
            Screen::HwCheck(_) => ("Hardware check".to_string(), String::new()),
            Screen::PowerMenu => ("Power menu".to_string(), String::new()),
            Screen::App(a, _) => (a.title().to_string(), a.page()),
        };
        let r = |x: &R| Rect { x: x.x, y: x.y, w: x.w, h: x.h };
        let raw = Raw {
            app,
            page,
            size: (g.pm.width(), g.pm.height()),
            texts,
            hits: self.hits.iter().map(|h| (h.label.clone(), r(&h.rect))).collect(),
            fields: self.fields.iter().map(|f| RawField { rect: r(&f.rect), focused: f.focused, secret: f.secret }).collect(),
        };
        state::publish(self.env.services.dir(), &raw);
    }

    /// Every beat: while a computer is asking, the picture is redrawn so a fresh frame is there to hand over.
    pub(super) fn dev_state_tick(&mut self) {
        state::heartbeat(self.env.services.dir());
        if state::wanted(self.env.services.dir()) {
            self.stale = true;
        }
    }
}
