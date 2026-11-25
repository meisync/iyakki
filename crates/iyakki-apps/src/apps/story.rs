//! TALES in the shell: the tile is the crate `iyakki-tile-tales` (alpha.11 AP1). This file gives it the shell's context: on every call it hands
//! the tile the real port (`story_port`: the library the core serves, the models, the speech engines, the agents, the audit log) and
//! forwards the `App` methods. The tile's own tests run in the crate against a fake port; the tests here (`story_tests.rs`) run it over the
//! real services. `Deref` shows the tile to the tests.

use std::ops::{Deref, DerefMut};

use super::browser_full::Redraw as ShellRedraw;
#[allow(hidden_glob_reexports)]
use super::{App, AppCx, Flow};
use crate::gfx::{Gfx, Icon};
use crate::ui::{Event, R};

pub use iyakki_tile_tales::*;

// the names the tile's old tests use through `super::*` (the tile's own imports)
#[cfg(test)]
#[allow(unused_imports)]
use {
    iyakki_chat_model::{ChatBackend, Message, ToolCall},
    iyakki_chatview::chat_view::{self, ChatView, NoPics, TOGGLE_BASE},
    iyakki_chatview::chatpic::PicCache,
    iyakki_chatview::model_picker::{self, Current, ModelList, Pick, TopRow},
    iyakki_piclog::{self as piclog, kind as plk, PicEvent},
    iyakki_scene::{self as scene, CharacterSheet},
    iyakki_story_kit::storyillus::{self as illus, Coverage, Readiness},
    iyakki_story_kit::{self as st, Hints, StoryStore},
    iyakki_story_kit::{Audience, Genre, Length, PictureMode, PictureStyle, Tone},
    iyakki_story_kit::{Block, Chunk, ModelPlan, Story, StoryEntry, StoredPic, StoredVariant, TtsStatus, LANGUAGES},
    iyakki_story_kit::{Status, StoryOptions},
    iyakki_story_model::ModelChoice,
    iyakki_voice_model::hostengines,
    iyakki_voice_model::hostvoice::{self, HostVoice},
    iyakki_voice_model::voice::{Highlight, SpeakOpts, SpeechEngine},
    crate::gfx::{rgb, Face},
    crate::ui::{Key, TextField, CARD_HOVER, DIM, ERR, MUTED, OK, TEXT},
    std::collections::HashMap,
    std::sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    std::sync::mpsc::{channel, Receiver, Sender},
    std::sync::Arc,
    std::time::{Duration, Instant},
};

pub struct StoryApp(iyakki_tile_tales::StoryApp);

impl Deref for StoryApp {
    type Target = iyakki_tile_tales::StoryApp;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for StoryApp {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl StoryApp {
    pub fn new(cx: &AppCx) -> StoryApp {
        StoryApp(iyakki_tile_tales::StoryApp::with_port(cx))
    }
}

impl App for StoryApp {
    fn keeps_screen_on(&self) -> bool {
        self.0.keeps_screen_on()
    }
    fn take_request(&mut self) -> Option<(String, String)> {
        self.0.take_request()
    }
    fn title(&self) -> &'static str {
        "Tales"
    }
    fn icon(&self) -> Icon {
        Icon::Tales
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        self.0.hint()
    }
    fn help(&self) -> Vec<String> {
        self.0.help()
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        match self.0.handle_with(ev, cx) {
            iyakki_tile_tales::Flow::Stay => Flow::Stay,
            iyakki_tile_tales::Flow::Back => Flow::Back,
        }
    }
    fn wants_redraw(&mut self) -> bool {
        self.0.wants_redraw()
    }
    fn idle_redraw_ms(&self) -> u64 {
        self.0.idle_redraw_ms()
    }
    fn wants_fast_poll(&self) -> bool {
        self.0.wants_fast_poll()
    }
    fn poll_redraw_due(&mut self) -> bool {
        self.0.poll_redraw_due()
    }
    fn redraw_kind(&mut self) -> ShellRedraw {
        match self.0.redraw_kind() {
            iyakki_tile_tales::Redraw::None => ShellRedraw::None,
            iyakki_tile_tales::Redraw::Live => ShellRedraw::Live,
            iyakki_tile_tales::Redraw::Full => ShellRedraw::Full,
        }
    }
    fn live_draw(&mut self, g: &mut Gfx) -> Option<(i32, i32, i32, i32)> {
        self.0.live_draw(g)
    }
    fn live_rects(&self) -> Vec<(i32, i32, i32, i32)> {
        self.0.live_rects()
    }
    fn live_full_every_ms(&self) -> u64 {
        self.0.live_full_every_ms()
    }
    fn take_host_save(&mut self) -> Option<(String, Vec<u8>)> {
        self.0.take_host_save()
    }
    fn context_items(&mut self, x: i32, y: i32) -> Vec<crate::ctxmenu::Item> {
        self.0.context_items(x, y)
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.0.render_with(g, area, s, cx, tick);
    }
}

#[cfg(test)]
#[path = "story_tests.rs"]
mod tests;
