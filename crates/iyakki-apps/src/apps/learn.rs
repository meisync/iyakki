//! LEARN in the shell: the tile is the crate `iyakki-tile-learn` (docs/STUDY.md; alpha.8 E3). This file gives it the shell's context: on every call it
//! builds the real port (`learn_port::CxPort`: the study files, the calendar, the PDF export, the assistant, the voice) and forwards the `App`
//! methods. The tests that need real services (the chat's `study_*` tools on the same store, the lesson writer with a scripted model, the TALES
//! reader) are in `learn_tests.rs`; the tile's own tests run in the crate against a fake port. `Deref` shows the tile to the tests.

use std::ops::{Deref, DerefMut};

use super::learn_port::{CxPort, ShellPictures, VoiceSlot};
use super::{App, AppCx};
use super::Flow as ShellFlow;
use crate::gfx::{Gfx, Icon};
use crate::ui::{Event, R};

pub use iyakki_tile_learn::app::*;
pub use iyakki_tile_learn::lesson::*;

pub struct LearnApp {
    tile: iyakki_tile_learn::LearnApp,
    pub voice: VoiceSlot,
    /// The picture cache the tile draws through (tests count how many pictures were drawn).
    pub pics: std::sync::Arc<std::sync::Mutex<crate::apps::chatpic::PicCache>>,
}

impl Deref for LearnApp {
    type Target = iyakki_tile_learn::LearnApp;
    fn deref(&self) -> &Self::Target {
        &self.tile
    }
}

impl DerefMut for LearnApp {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.tile
    }
}

impl LearnApp {
    pub fn new(cx: &AppCx) -> LearnApp {
        let voice = VoiceSlot::default();
        let pictures = ShellPictures::default();
        let pics = pictures.0.clone();
        let tile = iyakki_tile_learn::LearnApp::new(&cx.session.user, &CxPort { cx, voice: &voice }, Box::new(pictures));
        LearnApp { tile, voice, pics }
    }

    /// Is the voice reading the lesson now?
    pub fn reading(&self, cx: &AppCx) -> bool {
        iyakki_tile_learn::LearnPort::reading(&CxPort { cx, voice: &self.voice })
    }
}

impl App for LearnApp {
    fn title(&self) -> &'static str {
        self.tile.title()
    }
    fn page(&self) -> String {
        self.tile.page()
    }
    fn icon(&self) -> Icon {
        self.tile.icon()
    }
    fn accent(&self) -> [u8; 4] {
        self.tile.accent()
    }
    fn hint(&self) -> String {
        self.tile.hint()
    }
    fn help(&self) -> Vec<String> {
        self.tile.help()
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> ShellFlow {
        match self.tile.handle(ev, &CxPort { cx, voice: &self.voice }) {
            iyakki_tile_learn::Flow::Stay => ShellFlow::Stay,
            iyakki_tile_learn::Flow::Back => ShellFlow::Back,
        }
    }
    fn open_item(&mut self, item: &str, cx: &AppCx) {
        self.tile.open_item(item, &CxPort { cx, voice: &self.voice })
    }
    fn scroll_px(&mut self, px: i32, cx: &AppCx) -> bool {
        self.tile.scroll_px(px, &CxPort { cx, voice: &self.voice })
    }
    fn take_request(&mut self) -> Option<(String, String)> {
        self.tile.take_request()
    }
    fn wants_redraw(&mut self) -> bool {
        // a lesson being written, or read aloud, moves by itself: the tile knows it from the last frame
        self.tile.wants_redraw()
    }
    fn idle_redraw_ms(&self) -> u64 {
        self.tile.idle_redraw_ms()
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.tile.render(g, area, s, &CxPort { cx, voice: &self.voice }, tick)
    }
}
