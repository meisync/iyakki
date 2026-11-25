//! CHECK-UP: a guided hardware self-test (docs/CHECKUP.md). The tile itself lives in this crate (it does not depend on the agent or the
//! shared crate). The steps and their verdicts stay in `iyakki_shared::checkup` (pure, tested with made-up machines); the shell's port
//! ([`CheckupPort`]) answers a [`Check`] with a [`StepResult`], plays the tone, takes the camera picture, starts the microphone and keeps
//! the report. This file is the state, the keys and the two steps that need a live probe; `ui.rs` draws.
//!
//! * One list, one step open at a time. The person starts a step; it ends as Pass, Fail or Skipped with advice and the same fix button
//!   DOCTOR would offer. Nothing runs by itself: **the camera and microphone steps start only on a click or Enter on their own button**,
//!   show the same "Camera on" / "Microphone on" sign the browser shows (`port.sign`, clickable to stop) and keep nothing: the 2 seconds
//!   of sound are measured and dropped, the one camera picture is shown and dropped when the person moves on.
//! * The result goes to Share diagnostics: the newest report is kept by the port, DOCTOR's bundle holds it as its "Check-up" section, and
//!   the Share button here opens DOCTOR's preview (read it, then save it yourself).
//!
//! Keys: Up/Down choose a step, Enter starts it (or does what the open step asks), y / n answer, k skips, f the fix, d opens DOCTOR, r checks
//! everything again, s shares the result, Esc cancels what is running, then leaves.

use std::any::Any;
use std::sync::mpsc::{channel, Receiver};

use iyakki_tile_api::checkup::{bits, Check, FixAction, Listening, MicHeard, MicStart, Outcome, Said, Shot, StepId, StepResult, MIC_SECS};
pub use iyakki_tile_api::checkup::CheckupPort;
use iyakki_uikit::gfx::{rgb, Gfx, Icon};
use iyakki_uikit::scrolllist::ScrollList;
use iyakki_uikit::ui::{Event, Key, R};

mod ui;
#[cfg(test)]
mod tests;

/// What the tile answers to an event: stay on the screen, or go back to the launcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Stay,
    Back,
}

pub const ACCENT: [u8; 4] = rgb(96, 214, 168);

// click ids
pub const T_AGAIN: usize = 1;
pub const T_SHARE: usize = 2;
pub const T_DOCTOR: usize = 3;
pub const T_DISMISS: usize = 4;
pub const T_RUN: usize = 10;
pub const T_YES: usize = 11;
pub const T_NO: usize = 12;
pub const T_SKIP: usize = 13;
pub const T_FIX: usize = 14;
pub const T_ROW_DOCTOR: usize = 15;
pub const T_STOP: usize = 16;
pub const T_TARGET: usize = 17;
pub const T_AGAIN_TONE: usize = 18;
pub const A_LIST: usize = 61_320;


/// What the open step is doing right now.
pub enum Active {
    /// Colour bars on screen; waiting for "did you see them".
    Bars,
    /// A target to touch or click.
    Target,
    /// Listening for keys: the different ones so far.
    Keys(Vec<String>),
    /// The tone was played (or could not be); waiting for "did you hear it".
    Tone { played: Result<(), String>, _hold: Option<Box<dyn Any + Send>> },
    /// The microphone is recording.
    Record { cap: Box<dyn iyakki_tile_api::checkup::MicCapture>, name: String },
    /// The camera is taking a picture.
    Grab(Receiver<Shot>),
}

pub struct CheckupTile {
    pub results: Vec<StepResult>,
    pub sel: usize,
    pub active: Option<Active>,
    /// The step `active` belongs to.
    pub active_step: usize,
    pub list: ScrollList,
    pub msg: Option<(String, bool)>,
    /// The camera picture (width, height, straight RGBA), shown until the person moves to another step.
    pub preview: Option<(u32, u32, Vec<u8>)>,
    /// The level meter of the microphone step, 0 to 1, and how far the recording is (0 to 1).
    pub level: f32,
    pub progress: f32,
    pub request: Option<(String, String)>,
    pub dirty: bool,
    pub ticks: u64,
    touchscreens: usize,
}

impl CheckupTile {
    /// The tile over a port; the machine is read once now.
    pub fn new(port: &dyn CheckupPort) -> CheckupTile {
        let touchscreens = port.refresh();
        let a = CheckupTile {
            results: StepId::ALL.iter().map(|i| StepResult::pending(*i)).collect(),
            sel: 0,
            active: None,
            active_step: 0,
            list: ScrollList::default(),
            msg: None,
            preview: None,
            level: 0.0,
            progress: 0.0,
            request: None,
            dirty: true,
            ticks: 0,
            touchscreens,
        };
        a.keep_report(port);
        a
    }

    // read-only views for the shell's adapter and the tests
    pub fn results(&self) -> &[StepResult] {
        &self.results
    }
    pub fn selected(&self) -> usize {
        self.sel
    }
    /// A step is running (the colour bars, a target, the keys, the tone, a recording, a camera picture).
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }
    /// A recording or a camera picture is in progress (the screen redraws every frame).
    pub fn is_live(&self) -> bool {
        self.live()
    }

    pub fn step(&self, i: usize) -> StepId {
        StepId::ALL[i.min(StepId::ALL.len() - 1)]
    }

    pub fn keep_report(&self, port: &dyn CheckupPort) {
        port.keep_report(&self.results);
    }

    pub fn ok(&mut self, m: impl Into<String>) {
        self.msg = Some((m.into(), true));
        self.dirty = true;
    }

    pub fn fail(&mut self, m: impl Into<String>) {
        self.msg = Some((m.into(), false));
        self.dirty = true;
    }

    /// The step ended: keep its result, drop what was running, move to the next step that is not done.
    pub fn finish(&mut self, i: usize, r: StepResult, port: &dyn CheckupPort) {
        self.end_active(port);
        self.results[i] = r;
        self.keep_report(port);
        // the camera step stays open: its picture is shown there until the person moves on
        if self.step(i) != StepId::Camera {
            if let Some(n) = (i + 1..self.results.len()).find(|n| self.results[*n].outcome == Outcome::Pending) {
                self.select(n, port);
            }
        }
        self.dirty = true;
    }

    /// Stop whatever runs: the recording is dropped unread, the sign goes out.
    pub fn end_active(&mut self, port: &dyn CheckupPort) {
        if self.release() {
            port.sign(0);
        }
    }

    /// Drop what runs and say whether the sign must go out (a recording or a camera picture was in progress). The shell's adapter calls this
    /// when the tile is closed, where it has no port.
    pub fn release(&mut self) -> bool {
        let lit = matches!(self.active, Some(Active::Record { .. } | Active::Grab(_)));
        self.active = None;
        self.level = 0.0;
        self.progress = 0.0;
        lit
    }

    pub fn select(&mut self, i: usize, port: &dyn CheckupPort) {
        let i = i.min(self.results.len() - 1);
        if i != self.sel {
            if self.active.is_some() && self.active_step != i {
                self.end_active(port);
            }
            if self.step(i) != StepId::Camera {
                self.preview = None;
            }
            self.sel = i;
        }
        self.dirty = true;
    }

    /// Check everything again from the start (the machine is read again; pictures and recordings are already gone).
    pub fn again(&mut self, port: &dyn CheckupPort) {
        self.end_active(port);
        self.preview = None;
        self.touchscreens = port.refresh();
        self.results = StepId::ALL.iter().map(|i| StepResult::pending(*i)).collect();
        self.sel = 0;
        self.msg = None;
        self.keep_report(port);
        self.dirty = true;
    }

    // ------------------------------------------------------------------------------------------------ starting a step

    /// The person started step `i` (a click or Enter on its button).
    pub fn begin(&mut self, i: usize, port: &dyn CheckupPort) {
        self.select(i, port);
        self.end_active(port);
        self.msg = None;
        self.active_step = i;
        let id = self.step(i);
        // the machine is read again, so a plugged-in microphone or a joined network counts
        if !id.asks_the_person() || id == StepId::Touch {
            self.touchscreens = port.refresh();
        }
        match id {
            StepId::Display => self.active = Some(Active::Bars),
            StepId::Touch => {
                if self.touchscreens == 0 {
                    let r = port.verdict(&Check::Touch(Said::No));
                    self.finish(i, r, port);
                } else {
                    self.active = Some(Active::Target);
                }
            }
            StepId::Pointer => self.active = Some(Active::Target),
            StepId::Keyboard => self.active = Some(Active::Keys(Vec::new())),
            StepId::Speaker => self.play_tone(port),
            StepId::Microphone => self.start_microphone(port),
            StepId::Camera => self.start_camera(port),
            StepId::Monitor | StepId::Network | StepId::Wifi | StepId::Bluetooth | StepId::Power | StepId::Gpu | StepId::Usb => {
                let r = port.verdict(&Check::Machine(id));
                self.finish(i, r, port);
            }
        }
        self.dirty = true;
    }

    fn play_tone(&mut self, port: &dyn CheckupPort) {
        let (played, hold) = match port.play_tone() {
            Ok(h) => (Ok(()), Some(h)),
            Err(e) => (Err(e), None),
        };
        self.active = Some(Active::Tone { played, _hold: hold });
    }

    fn start_microphone(&mut self, port: &dyn CheckupPort) {
        let i = self.active_step;
        match port.start_mic() {
            Ok(Listening { capture, name }) => {
                port.audit("checkup.microphone", "");
                port.sign(bits::MICROPHONE);
                self.level = 0.0;
                self.progress = 0.0;
                self.active = Some(Active::Record { cap: capture, name });
            }
            Err(MicStart::NoMic(why)) => {
                let r = port.verdict(&Check::Microphone { heard: MicHeard::NoMic(why), name: String::new() });
                self.finish(i, r, port);
            }
            Err(MicStart::Failed(e)) => {
                let r = port.verdict(&Check::Microphone { heard: MicHeard::Failed(e), name: String::new() });
                self.finish(i, r, port);
            }
        }
    }

    fn start_camera(&mut self, port: &dyn CheckupPort) {
        let job = port.camera_job();
        let (tx, rx) = channel();
        match std::thread::Builder::new().name("iyakki-checkup-camera".into()).spawn(move || {
            let _ = tx.send(job());
        }) {
            Ok(_) => {
                port.audit("checkup.camera", "");
                port.sign(bits::CAMERA);
                self.active = Some(Active::Grab(rx));
            }
            Err(e) => {
                let r = port.verdict(&Check::Camera(iyakki_tile_api::checkup::CamHeard::Failed { name: "the camera".into(), why: e.to_string() }));
                let i = self.active_step;
                self.finish(i, r, port);
            }
        }
    }

    // ------------------------------------------------------------------------------------------------ live work

    /// Move the running step along: the recording's level and end, the camera's picture, the sign's stop click.
    pub fn poll(&mut self, port: &dyn CheckupPort) {
        let i = self.active_step;
        if matches!(self.active, Some(Active::Record { .. } | Active::Grab(_))) && port.stop_asked() {
            self.end_active(port);
            self.ok("Stopped. Nothing was kept.");
            return;
        }
        let rec_done = match &mut self.active {
            Some(Active::Record { cap, .. }) => {
                self.level = cap.level();
                self.progress = (cap.elapsed().as_secs_f32() / MIC_SECS.as_secs_f32()).min(1.0);
                // keep the sign lit while recording (it holds two seconds after the last beat)
                port.sign(bits::MICROPHONE);
                cap.elapsed() >= MIC_SECS || cap.is_full() || !cap.is_alive()
            }
            _ => false,
        };
        if rec_done {
            if let Some(Active::Record { cap, name }) = self.active.take() {
                port.sign(0);
                let heard = match cap.stop() {
                    Ok(pcm) => MicHeard::Samples(pcm),
                    Err(e) => MicHeard::Failed(e),
                };
                let r = port.verdict(&Check::Microphone { heard, name });
                self.finish(i, r, port);
            }
            return;
        }
        let shot: Option<Shot> = match &self.active {
            Some(Active::Grab(rx)) => {
                port.sign(bits::CAMERA);
                rx.try_recv().ok()
            }
            _ => None,
        };
        if let Some(shot) = shot {
            port.sign(0);
            self.preview = shot.picture.and_then(|(w, h, rgba)| opaque(w, h, rgba));
            let r = port.verdict(&Check::Camera(shot.heard));
            self.finish(i, r, port);
        }
    }

    pub fn live(&self) -> bool {
        matches!(self.active, Some(Active::Record { .. } | Active::Grab(_)))
    }

    // ------------------------------------------------------------------------------------------------ answers

    pub fn answer(&mut self, said: Said, port: &dyn CheckupPort) {
        let i = self.active_step;
        let check = match (&self.active, self.step(i)) {
            (Some(Active::Bars), _) => Check::Display(said),
            (Some(Active::Tone { played, .. }), _) => Check::Speaker { played: played.clone(), said },
            (Some(Active::Target), StepId::Touch) => Check::Touch(said),
            (Some(Active::Target), _) => Check::Pointer { clicked: said == Said::Yes, said },
            (Some(Active::Keys(keys)), _) => Check::Keyboard { keys: keys.len(), said: if said == Said::Skip { Said::Skip } else { Said::Yes } },
            _ => return,
        };
        let r = port.verdict(&check);
        self.finish(i, r, port);
    }

    fn skip(&mut self, port: &dyn CheckupPort) {
        let i = self.sel;
        self.end_active(port);
        let r = port.verdict(&Check::Skipped(self.step(i)));
        self.finish(i, r, port);
    }

    pub fn do_fix(&mut self, port: &dyn CheckupPort) {
        let Some(fix) = self.results.get(self.sel).and_then(|r| r.fix.clone()) else { return };
        port.audit("checkup.fix", self.step(self.sel).id());
        match fix.action {
            FixAction::Settings(item) => self.request = Some(("settings".into(), item.into())),
            FixAction::ExtendDeveloperMode => self.request = Some(("settings".into(), "tab:developer".into())),
        }
    }

    pub fn open_doctor(&mut self) {
        self.request = Some(("doctor".into(), String::new()));
    }

    /// Hand the report to DOCTOR's share preview. Nothing is sent anywhere; the person saves the file.
    fn share(&mut self, port: &dyn CheckupPort) {
        if self.results.iter().all(|r| r.outcome == Outcome::Pending) {
            self.fail("Nothing has been checked yet. Start a step first.");
            return;
        }
        self.keep_report(port);
        self.request = Some(("doctor".into(), "share".into()));
    }

    /// The step's main action: start it, or what the open panel asks.
    fn primary(&mut self, port: &dyn CheckupPort) {
        // 0: nothing runs, 1: the open step waits for a yes, 2: something else runs (a recording, a target)
        let state = match &self.active {
            Some(Active::Bars | Active::Tone { .. } | Active::Keys(_)) if self.active_step == self.sel => 1,
            Some(_) if self.active_step == self.sel => 2,
            _ => 0,
        };
        match state {
            1 => self.answer(Said::Yes, port),
            0 => self.begin(self.sel, port),
            _ => {}
        }
    }

    fn handle_keys_mode(&mut self, ev: &Event, port: &dyn CheckupPort) -> bool {
        let Some(Active::Keys(keys)) = &mut self.active else { return false };
        match ev {
            Event::Key(Key::Esc) => {
                self.end_active(port);
                self.dirty = true;
            }
            Event::Key(Key::Enter) => self.answer(Said::Yes, port),
            Event::Key(k) => {
                let name = format!("{k:?}");
                if !keys.contains(&name) {
                    keys.push(name);
                }
                let n = keys.len();
                if n >= 3 {
                    self.answer(Said::Yes, port);
                }
                self.dirty = true;
            }
            Event::Tab(T_YES) | Event::Tab(T_RUN) => self.answer(Said::Yes, port),
            Event::Tab(T_NO) => self.answer(Said::No, port),
            Event::Tab(T_SKIP) => self.skip(port),
            _ => return false,
        }
        true
    }

    pub fn title(&self) -> &'static str {
        "Check-up"
    }
    pub fn page(&self) -> String {
        self.step(self.sel).id().into()
    }
    pub fn icon(&self) -> Icon {
        Icon::Checkup
    }
    pub fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    pub fn hint(&self) -> String {
        match &self.active {
            Some(Active::Keys(_)) => "press keys · Enter done · Esc stop".into(),
            Some(Active::Bars) | Some(Active::Tone { .. }) => "y yes · n no · Esc stop".into(),
            Some(Active::Record { .. }) | Some(Active::Grab(_)) => "Esc stop · nothing is kept".into(),
            Some(Active::Target) => "click the circle · n it did not work · Esc stop".into(),
            None => "↑↓ choose · Enter start · k skip · f fix · d DOCTOR · r again · s share · Esc back".into(),
        }
    }
    pub fn help(&self) -> Vec<String> {
        [
            "CHECK-UP walks through the hardware of this computer one step at a time. Each step ends as Pass, Fail or Skipped, with plain advice.",
            "Up / Down · choose a step      Enter · start it      k · skip it      r · check everything again",
            "Screen, touch, keyboard, mouse, speaker: you look, touch, press or listen and answer y (yes) or n (no).",
            "Microphone and camera start only when you press their button. The sign \"Microphone on\" or \"Camera on\" is lit while they work (click it to stop). The microphone is the one the Voice tile uses. The 2 seconds of sound are measured and dropped, the picture is shown once and dropped when you move on: nothing is kept.",
            "Monitor, network, Wi-Fi, Bluetooth and battery read the computer and answer by themselves.",
            "f · the fix button of a failed step opens the Settings page, like DOCTOR's. d · open DOCTOR.",
            "s · Share result: opens DOCTOR's preview with this report inside \"Check-up\". You see exactly what would be saved, then save it yourself. Nothing is uploaded.",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    pub fn handle(&mut self, ev: &Event, port: &dyn CheckupPort) -> Answer {
        if self.handle_keys_mode(ev, port) {
            return Answer::Stay;
        }
        match ev {
            Event::Key(Key::Esc) => {
                if self.active.is_some() {
                    self.end_active(port);
                } else {
                    return Answer::Back;
                }
            }
            Event::Tab(T_DISMISS) => self.msg = None,
            Event::Tab(T_AGAIN) | Event::Key(Key::Char('r')) => self.again(port),
            Event::Tab(T_SHARE) | Event::Key(Key::Char('s')) => self.share(port),
            Event::Tab(T_DOCTOR) | Event::Tab(T_ROW_DOCTOR) | Event::Key(Key::Char('d')) => self.open_doctor(),
            Event::Tab(T_RUN) | Event::Key(Key::Enter) => self.primary(port),
            Event::Tab(T_AGAIN_TONE) => self.begin(self.sel, port),
            Event::Tab(T_YES) | Event::Key(Key::Char('y')) => self.answer(Said::Yes, port),
            Event::Tab(T_NO) | Event::Key(Key::Char('n')) => self.answer(Said::No, port),
            Event::Tab(T_TARGET) => self.answer(Said::Yes, port),
            Event::Tab(T_SKIP) | Event::Key(Key::Char('k')) => self.skip(port),
            Event::Tab(T_STOP) => {
                self.end_active(port);
                self.ok("Stopped. Nothing was kept.");
            }
            Event::Tab(T_FIX) | Event::Key(Key::Char('f')) => self.do_fix(port),
            Event::Row(i) => self.select(*i, port),
            Event::Key(Key::Up) => self.select(self.sel.saturating_sub(1), port),
            Event::Key(Key::Down) => self.select(self.sel + 1, port),
            Event::Key(k) => {
                self.list.key(k);
            }
            other => {
                self.list.event(other, A_LIST);
            }
        }
        self.dirty = true;
        Answer::Stay
    }

    pub fn scroll_px(&mut self, px: i32) -> bool {
        self.list.wheel_px(px)
    }
    pub fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }
    pub fn keeps_screen_on(&self) -> bool {
        self.active.is_some()
    }
    pub fn wants_redraw(&mut self, port: &dyn CheckupPort) -> bool {
        self.poll(port);
        self.ticks += 1;
        std::mem::take(&mut self.dirty) | self.live()
    }
    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32, port: &dyn CheckupPort, tick: u64) {
        self.poll(port);
        self.draw(g, area, s, port, tick);
    }
}

/// An opaque RGBA picture: padded or cut to `w` x `h` pixels, alpha 255 (what the camera probe hands over can be shorter than it says).
fn opaque(w: u32, h: u32, mut rgba: Vec<u8>) -> Option<(u32, u32, Vec<u8>)> {
    if w == 0 || h == 0 {
        return None;
    }
    rgba.resize(w as usize * h as usize * 4, 255);
    for p in rgba.chunks_exact_mut(4) {
        p[3] = 255;
    }
    Some((w, h, rgba))
}
