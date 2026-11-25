//! VOICE in the shell: the tile is the crate `iyakki-tile-voice` (alpha.11 AP1). This file gives it the shell's context: on every call it
//! hands the tile the real port (`voice_port::CxPort`) and forwards the `App` methods. The tile's own tests run in the crate against a fake
//! port; the tests here (`voice_tests.rs` and the files beside it) run it over the real services. `Deref` shows the tile to the tests.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use super::voice_port::CxPort;
#[allow(hidden_glob_reexports)]
use super::{App, AppCx, Flow};
use crate::gfx::Icon;
use crate::ui::Event;

pub use iyakki_tile_voice::*;

// the names the tile's old tests use through `super::*` (the tile's own imports)
#[cfg(test)]
#[allow(unused_imports)]
use {
    iyakki_tile_voice::port::VoicePort,
    crate::ui::{Key, TextField},
    iyakki_shared::dictfix,
    iyakki_shared::story::{Sink, Status, Synth},
    iyakki_shared::voice::SpeakOpts,
    iyakki_shared::voicedesk::{self as vd, Capture, Dest, Mic, ReadVoice, Recorders, MAX_RECORD_SECS, MAX_TEXT_CHARS},
    iyakki_tile_voice::voice_mine::{Mine, T_TAB_MINE},
    iyakki_tile_voice::voice_rec::{Rec, T_TAB_REC},
    iyakki_uikit::multiline::MultiLine,
};

pub struct VoiceApp(iyakki_tile_voice::VoiceApp);

impl Deref for VoiceApp {
    type Target = iyakki_tile_voice::VoiceApp;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for VoiceApp {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// The rig of this person's session: the microphone is the browser's in a browser session, the machine's otherwise
/// (`iyakki_shared::browsersource::recorders_for`, the one place that decides).
pub fn rig_for_user(core: Arc<dyn iyakki_core_api::Core>, user: &str) -> Rig {
    Rig {
        recorders: if user.is_empty() {
            crate::recorders()
        } else {
            match iyakki_shared::browsersource::browser_session(user) {
                Some(m) => Arc::new(iyakki_shared::browsersource::BrowserRecorders { media: m }) as Arc<dyn iyakki_shared::voicedesk::Recorders>,
                None => crate::recorders(),
            }
        },
        sink: None,
        // the same check the cues make: is there an output to play on (the core's own output chain)
        output: Arc::new(move || core.cues().out().ready()),
    }
}

/// The rig of the system itself (no person: the machine's microphone).
pub fn rig_system(core: Arc<dyn iyakki_core_api::Core>) -> Rig {
    rig_for_user(core, "")
}

impl VoiceApp {
    pub fn new(cx: &AppCx) -> VoiceApp {
        let port = CxPort(cx);
        VoiceApp(iyakki_tile_voice::VoiceApp::with_port(&port, port.rig()))
    }

    /// The tile with a rig of the caller's choosing (tests give fake recorders).
    pub fn with_rig(cx: &AppCx, rig: Rig) -> VoiceApp {
        VoiceApp(iyakki_tile_voice::VoiceApp::with_port(&CxPort(cx), rig))
    }

    /// Keep recordings in `dir` instead of the person's `Recordings` folder (tests).
    pub fn with_recordings_dir(self, dir: std::path::PathBuf) -> VoiceApp {
        VoiceApp(self.0.with_recordings_dir(dir))
    }

    /// The tile over any port.
    pub fn with_port(port: &dyn iyakki_tile_voice::port::VoicePort, rig: Rig) -> VoiceApp {
        VoiceApp(iyakki_tile_voice::VoiceApp::with_port(port, rig))
    }
}

impl App for VoiceApp {
    fn title(&self) -> &'static str {
        "Voice"
    }
    fn icon(&self) -> Icon {
        Icon::Speaker
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
    fn take_request(&mut self) -> Option<(String, String)> {
        self.0.take_request()
    }
    fn wants_redraw(&mut self) -> bool {
        self.0.wants_redraw()
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        match self.0.handle_with(ev, &CxPort(cx)) {
            iyakki_tile_voice::Flow::Stay => Flow::Stay,
            iyakki_tile_voice::Flow::Back => Flow::Back,
        }
    }
    fn render(&mut self, g: &mut crate::gfx::Gfx, area: crate::ui::R, s: f32, cx: &AppCx, tick: u64) {
        self.0.render_with(g, area, s, &CxPort(cx), tick);
    }
}

#[cfg(test)]
#[path = "voice_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "voice_browser_tests.rs"]
mod browser_tests;

#[cfg(test)]
#[path = "voice_stream_tests.rs"]
mod stream_tests;
