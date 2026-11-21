//! CHECK-UP: a guided hardware self-test (docs/CHECKUP.md). The steps and their verdicts live in `iyakki_agent::checkup` (pure, tested with
//! made-up machines); `checkup_ui.rs` draws; this file is the state, the keys and the two steps that need a live probe.
//!
//! * One list, one step open at a time. The person starts a step; it ends as Pass, Fail or Skipped with advice and the same fix button
//!   DOCTOR would offer. Nothing runs by itself: **the camera and microphone steps start only on a click or Enter on their own button**,
//!   show the same "Camera on" / "Microphone on" sign the browser shows (`browser_media::publish_capture`, clickable to stop) and keep
//!   nothing: the 2 seconds of sound are measured and dropped, the one camera picture is shown and dropped when the person moves on.
//! * The microphone is the one the Voice tile uses (`VoiceSettings.mic`, then the first listed); when none is listed, or a T2 Mac's capture
//!   switch is off, the step says so in words and is Skipped.
//! * The result goes to Share diagnostics: the newest report is kept in memory (`checkup::set_last_report`), DOCTOR's bundle holds it as
//!   its "Check-up" section, and the Share button here opens DOCTOR's preview (read it, then save it yourself).
//!
//! Keys: Up/Down choose a step, Enter starts it (or does what the open step asks), y / n answer, k skips, f the fix, d opens DOCTOR, r checks
//! everything again, s shares the result, Esc cancels what is running, then leaves.

use std::any::Any;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use iyakki_shared::checkup::{CamHeard, MicHeard, MicStart, Outcome, Said};
use iyakki_shared::checkup::{Inventory, StepId, StepResult};
use iyakki_shared::checkup::{self};
use iyakki_shared::doctor::Action;
use iyakki_shared::voicedesk::Capture;
use tiny_skia::Pixmap;

use super::checkup_port::{CheckupPort, CxPort};
pub use super::checkup_port::Parts;
use super::{App, AppCx, Flow};
use crate::camera::probe::Shot;
use crate::gfx::{rgb, Icon};
use crate::scrolllist::ScrollList;
use crate::ui::{Event, Key};

pub const ACCENT: [u8; 4] = rgb(96, 214, 168);

// click ids
pub(super) const T_AGAIN: usize = 1;
pub(super) const T_SHARE: usize = 2;
pub(super) const T_DOCTOR: usize = 3;
pub(super) const T_DISMISS: usize = 4;
pub(super) const T_RUN: usize = 10;
pub(super) const T_YES: usize = 11;
pub(super) const T_NO: usize = 12;
pub(super) const T_SKIP: usize = 13;
pub(super) const T_FIX: usize = 14;
pub(super) const T_ROW_DOCTOR: usize = 15;
pub(super) const T_STOP: usize = 16;
pub(super) const T_TARGET: usize = 17;
pub(super) const T_AGAIN_TONE: usize = 18;
pub(super) const A_LIST: usize = 61_320;

/// Longest the microphone step records, a little over [`checkup::MIC_SECS`] so the stop is ours.
const MIC_MAX_SECS: usize = 4;

/// What the open step is doing right now.
pub(super) enum Active {
    /// Colour bars on screen; waiting for "did you see them".
    Bars,
    /// A target to touch or click.
    Target,
    /// Listening for keys: the different ones so far.
    Keys(Vec<String>),
    /// The tone was played (or could not be); waiting for "did you hear it".
    Tone { played: Result<(), String>, _hold: Option<Box<dyn Any + Send>> },
    /// The microphone is recording.
    Record { cap: Box<dyn Capture>, name: String },
    /// The camera is taking a picture.
    Grab(Receiver<Shot>),
}

pub struct CheckupApp {
    pub(super) parts: Parts,
    pub(super) inv: Inventory,
    pub(super) results: Vec<StepResult>,
    pub(super) sel: usize,
    pub(super) active: Option<Active>,
    /// The step `active` belongs to.
    pub(super) active_step: usize,
    pub(super) list: ScrollList,
    pub(super) msg: Option<(String, bool)>,
    /// The camera picture, shown until the person moves to another step.
    pub(super) preview: Option<Pixmap>,
    /// The level meter of the microphone step, 0 to 1, and how far the recording is (0 to 1).
    pub(super) level: f32,
    pub(super) progress: f32,
    pub(super) request: Option<(String, String)>,
    pub(super) dirty: bool,
    pub(super) ticks: u64,
    /// Where the signed result file goes (`checkup_result`), and the services that know the cluster state.
    k3s: Arc<dyn iyakki_agent_api::DevCluster>,
    dir: std::path::PathBuf,
}

impl CheckupApp {
    pub fn new(cx: &AppCx) -> CheckupApp {
        CheckupApp::with_parts(cx, Parts::real())
    }

    pub fn with_parts(cx: &AppCx, parts: Parts) -> CheckupApp {
        CheckupApp::with_port(&CxPort(cx), parts)
    }

    /// The tile over any port (a fake in tests).
    pub fn with_port(port: &dyn CheckupPort, parts: Parts) -> CheckupApp {
        let inv = (parts.inventory)(port);
        let a = CheckupApp {
            parts,
            inv,
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
            k3s: port.k3s(),
            dir: port.data_dir(),
        };
        a.keep_report();
        a
    }

    pub(super) fn step(&self, i: usize) -> StepId {
        StepId::ALL[i.min(StepId::ALL.len() - 1)]
    }

    pub(super) fn keep_report(&self) {
        let any = self.results.iter().any(|r| r.outcome != Outcome::Pending);
        checkup::set_last_report(if any { checkup::report_lines(&self.results) } else { Vec::new() });
        // a finished check-up leaves a signed file that maps to the hardware matrix (F-CHECKUP-02); a failed write is not shown as a check-up failure
        let extra = iyakki_shared::checkup_result::Extra { k3s_state: Some(self.k3s.health().state), gpu: self.results.iter().find(|r| r.id == StepId::Gpu).filter(|r| r.outcome != Outcome::Pending).map(|r| (r.note.clone(), r.outcome == Outcome::Pass)) };
        let machine = crate::agent().doctor_hardware_text().lines().next().unwrap_or("").to_string();
        let _ = iyakki_shared::checkup_result::save_if_complete(&self.dir, &self.results, &extra, &machine, env!("CARGO_PKG_VERSION"), iyakki_clock::now_unix());
    }

    pub(super) fn ok(&mut self, m: impl Into<String>) {
        self.msg = Some((m.into(), true));
        self.dirty = true;
    }

    pub(super) fn fail(&mut self, m: impl Into<String>) {
        self.msg = Some((m.into(), false));
        self.dirty = true;
    }

    /// The step ended: keep its result, drop what was running, move to the next step that is not done.
    pub(super) fn finish(&mut self, i: usize, r: StepResult) {
        self.end_active();
        self.results[i] = r;
        self.keep_report();
        // the camera step stays open: its picture is shown there until the person moves on
        if self.step(i) != StepId::Camera {
            if let Some(n) = (i + 1..self.results.len()).find(|n| self.results[*n].outcome == Outcome::Pending) {
                self.select(n);
            }
        }
        self.dirty = true;
    }

    /// Stop whatever runs: the recording is dropped unread, the sign goes out.
    pub(super) fn end_active(&mut self) {
        if let Some(Active::Record { .. } | Active::Grab(_)) = self.active {
            (self.parts.sign)(0);
        }
        self.active = None;
        self.level = 0.0;
        self.progress = 0.0;
    }

    pub(super) fn select(&mut self, i: usize) {
        let i = i.min(self.results.len() - 1);
        if i != self.sel {
            if self.active.is_some() && self.active_step != i {
                self.end_active();
            }
            if self.step(i) != StepId::Camera {
                self.preview = None;
            }
            self.sel = i;
        }
        self.dirty = true;
    }

    /// Check everything again from the start (the machine is read again; pictures and recordings are already gone).
    pub(super) fn again(&mut self, port: &dyn CheckupPort) {
        self.end_active();
        self.preview = None;
        self.inv = (self.parts.inventory)(port);
        self.results = StepId::ALL.iter().map(|i| StepResult::pending(*i)).collect();
        self.sel = 0;
        self.msg = None;
        self.keep_report();
        self.dirty = true;
    }

    // ------------------------------------------------------------------------------------------------ starting a step

    /// The person started step `i` (a click or Enter on its button).
    pub(super) fn begin(&mut self, i: usize, port: &dyn CheckupPort) {
        self.select(i);
        self.end_active();
        self.msg = None;
        self.active_step = i;
        let id = self.step(i);
        // the machine is read again, so a plugged-in microphone or a joined network counts
        if !id.asks_the_person() || id == StepId::Touch {
            self.inv = (self.parts.inventory)(port);
        }
        match id {
            StepId::Display => self.active = Some(Active::Bars),
            StepId::Touch => {
                if self.inv.touchscreens == 0 {
                    let r = checkup::touch(&self.inv, Said::No);
                    self.finish(i, r);
                } else {
                    self.active = Some(Active::Target);
                }
            }
            StepId::Pointer => self.active = Some(Active::Target),
            StepId::Keyboard => self.active = Some(Active::Keys(Vec::new())),
            StepId::Speaker => self.play_tone(port),
            StepId::Microphone => self.start_microphone(port),
            StepId::Camera => self.start_camera(port),
            StepId::Monitor => self.finish(i, checkup::monitor(&self.inv)),
            StepId::Network => self.finish(i, checkup::network(&self.inv)),
            StepId::Wifi => self.finish(i, checkup::wifi(&self.inv)),
            StepId::Bluetooth => self.finish(i, checkup::bluetooth(&self.inv)),
            StepId::Power => self.finish(i, checkup::power(&self.inv)),
            StepId::Gpu => self.finish(i, checkup::gpu(&self.inv)),
            StepId::Usb => self.finish(i, checkup::usb(&self.inv)),
        }
        self.dirty = true;
    }

    fn play_tone(&mut self, port: &dyn CheckupPort) {
        let (played, hold) = match (self.parts.play)(port) {
            Ok(h) => (Ok(()), Some(h)),
            Err(e) => (Err(e), None),
        };
        self.active = Some(Active::Tone { played, _hold: hold });
    }

    fn start_microphone(&mut self, port: &dyn CheckupPort) {
        let vs = port.voice_settings();
        let i = self.active_step;
        match iyakki_shared::checkup::start_mic(self.parts.recorders.as_ref(), &vs.mic, &vs.record_cmd, MIC_MAX_SECS, self.inv.t2_mic_hidden) {
            Ok(cap) => {
                let mics = self.parts.recorders.mics();
                let name = iyakki_shared::voicedesk::pick_mic(&mics, &vs.mic).map(|m| m.label.clone()).unwrap_or_default();
                port.audit("checkup.microphone", "");
                (self.parts.sign)(browser_media_bits::MICROPHONE);
                self.level = 0.0;
                self.progress = 0.0;
                self.active = Some(Active::Record { cap, name });
            }
            Err(MicStart::NoMic(why)) => {
                let r = checkup::microphone(&self.inv, &MicHeard::NoMic(why), "");
                self.finish(i, r);
            }
            Err(MicStart::Failed(e)) => {
                let r = checkup::microphone(&self.inv, &MicHeard::Failed(e), "");
                self.finish(i, r);
            }
        }
    }

    fn start_camera(&mut self, port: &dyn CheckupPort) {
        let job = (self.parts.camera)(port);
        let (tx, rx) = channel();
        match std::thread::Builder::new().name("iyakki-checkup-camera".into()).spawn(move || {
            let _ = tx.send(job());
        }) {
            Ok(_) => {
                port.audit("checkup.camera", "");
                (self.parts.sign)(browser_media_bits::CAMERA);
                self.active = Some(Active::Grab(rx));
            }
            Err(e) => {
                let r = checkup::camera(&CamHeard::Failed { name: "the camera".into(), why: e.to_string() });
                let i = self.active_step;
                self.finish(i, r);
            }
        }
    }

    // ------------------------------------------------------------------------------------------------ live work

    /// Move the running step along: the recording's level and end, the camera's picture, the sign's stop click.
    pub(super) fn poll(&mut self) {
        let i = self.active_step;
        if matches!(self.active, Some(Active::Record { .. } | Active::Grab(_))) {
            if (self.parts.stop_asked)() {
                self.end_active();
                self.ok("Stopped. Nothing was kept.");
                return;
            }
        }
        let rec_done = match &mut self.active {
            Some(Active::Record { cap, .. }) => {
                self.level = cap.level();
                self.progress = (cap.elapsed().as_secs_f32() / checkup::MIC_SECS.as_secs_f32()).min(1.0);
                // keep the sign lit while recording (it holds two seconds after the last beat)
                (self.parts.sign)(browser_media_bits::MICROPHONE);
                cap.elapsed() >= checkup::MIC_SECS || cap.is_full() || !cap.is_alive()
            }
            _ => false,
        };
        if rec_done {
            if let Some(Active::Record { cap, name }) = self.active.take() {
                (self.parts.sign)(0);
                let heard = match cap.stop() {
                    Ok(pcm) => MicHeard::Samples(pcm),
                    Err(e) => MicHeard::Failed(e),
                };
                let r = checkup::microphone(&self.inv, &heard, &name);
                self.finish(i, r);
            }
            return;
        }
        let shot = match &self.active {
            Some(Active::Grab(rx)) => {
                (self.parts.sign)(browser_media_bits::CAMERA);
                rx.try_recv().ok()
            }
            _ => None,
        };
        if let Some(shot) = shot {
            (self.parts.sign)(0);
            self.preview = shot.picture.and_then(|(w, h, rgba)| crate::camera::pixels::pixmap(w, h, rgba));
            let r = checkup::camera(&shot.heard);
            self.finish(i, r);
        }
    }

    pub(super) fn live(&self) -> bool {
        matches!(self.active, Some(Active::Record { .. } | Active::Grab(_)))
    }

    // ------------------------------------------------------------------------------------------------ answers

    pub(super) fn answer(&mut self, said: Said) {
        let i = self.active_step;
        let r = match (&self.active, self.step(i)) {
            (Some(Active::Bars), _) => checkup::display(said),
            (Some(Active::Tone { played, .. }), _) => checkup::speaker(&self.inv, &played.clone(), said),
            (Some(Active::Target), StepId::Touch) => checkup::touch(&self.inv, said),
            (Some(Active::Target), _) => checkup::pointer(&self.inv, said == Said::Yes, said),
            (Some(Active::Keys(keys)), _) => checkup::keyboard(&self.inv, keys.len(), if said == Said::Skip { Said::Skip } else { Said::Yes }),
            _ => return,
        };
        self.finish(i, r);
    }

    fn skip(&mut self) {
        let i = self.sel;
        self.end_active();
        self.finish(i, checkup::skipped(self.step(i)));
    }

    pub(super) fn do_fix(&mut self, port: &dyn CheckupPort) {
        let Some(fix) = self.results.get(self.sel).and_then(|r| r.fix.clone()) else { return };
        port.audit("checkup.fix", self.step(self.sel).id());
        match fix.action {
            Action::Settings(item) => self.request = Some(("settings".into(), item.into())),
            Action::ExtendDeveloperMode => self.request = Some(("settings".into(), "tab:developer".into())),
        }
    }

    pub(super) fn open_doctor(&mut self) {
        self.request = Some(("doctor".into(), String::new()));
    }

    /// Hand the report to DOCTOR's share preview. Nothing is sent anywhere; the person saves the file.
    fn share(&mut self) {
        if self.results.iter().all(|r| r.outcome == Outcome::Pending) {
            self.fail("Nothing has been checked yet. Start a step first.");
            return;
        }
        self.keep_report();
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
            1 => self.answer(Said::Yes),
            0 => self.begin(self.sel, port),
            _ => {}
        }
    }

    fn handle_keys_mode(&mut self, ev: &Event) -> bool {
        let Some(Active::Keys(keys)) = &mut self.active else { return false };
        match ev {
            Event::Key(Key::Esc) => {
                self.end_active();
                self.dirty = true;
            }
            Event::Key(Key::Enter) => self.answer(Said::Yes),
            Event::Key(k) => {
                let name = format!("{k:?}");
                if !keys.contains(&name) {
                    keys.push(name);
                }
                let n = keys.len();
                if n >= 3 {
                    self.answer(Said::Yes);
                }
                self.dirty = true;
            }
            Event::Tab(T_YES) | Event::Tab(T_RUN) => self.answer(Said::Yes),
            Event::Tab(T_NO) => self.answer(Said::No),
            Event::Tab(T_SKIP) => self.skip(),
            _ => return false,
        }
        true
    }
}

/// The `media_kind` bits the sign understands (`iyakki_webview::proto::media_kind`).
mod browser_media_bits {
    pub const MICROPHONE: u8 = iyakki_webview::proto::media_kind::MICROPHONE;
    pub const CAMERA: u8 = iyakki_webview::proto::media_kind::CAMERA;
}

impl Drop for CheckupApp {
    fn drop(&mut self) {
        self.end_active();
    }
}

impl App for CheckupApp {
    fn title(&self) -> &'static str {
        "Check-up"
    }
    fn page(&self) -> String {
        self.step(self.sel).id().into()
    }
    fn icon(&self) -> Icon {
        Icon::Checkup
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        match &self.active {
            Some(Active::Keys(_)) => "press keys · Enter done · Esc stop".into(),
            Some(Active::Bars) | Some(Active::Tone { .. }) => "y yes · n no · Esc stop".into(),
            Some(Active::Record { .. }) | Some(Active::Grab(_)) => "Esc stop · nothing is kept".into(),
            Some(Active::Target) => "click the circle · n it did not work · Esc stop".into(),
            None => "↑↓ choose · Enter start · k skip · f fix · d DOCTOR · r again · s share · Esc back".into(),
        }
    }
    fn help(&self) -> Vec<String> {
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

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        let port = CxPort(cx);
        if self.handle_keys_mode(ev) {
            return Flow::Stay;
        }
        match ev {
            Event::Key(Key::Esc) => {
                if self.active.is_some() {
                    self.end_active();
                } else {
                    return Flow::Back;
                }
            }
            Event::Tab(T_DISMISS) => self.msg = None,
            Event::Tab(T_AGAIN) | Event::Key(Key::Char('r')) => self.again(&port),
            Event::Tab(T_SHARE) | Event::Key(Key::Char('s')) => self.share(),
            Event::Tab(T_DOCTOR) | Event::Tab(T_ROW_DOCTOR) | Event::Key(Key::Char('d')) => self.open_doctor(),
            Event::Tab(T_RUN) | Event::Key(Key::Enter) => self.primary(&port),
            Event::Tab(T_AGAIN_TONE) => self.begin(self.sel, &port),
            Event::Tab(T_YES) | Event::Key(Key::Char('y')) => self.answer(Said::Yes),
            Event::Tab(T_NO) | Event::Key(Key::Char('n')) => self.answer(Said::No),
            Event::Tab(T_TARGET) => self.answer(Said::Yes),
            Event::Tab(T_SKIP) | Event::Key(Key::Char('k')) => self.skip(),
            Event::Tab(T_STOP) => {
                self.end_active();
                self.ok("Stopped. Nothing was kept.");
            }
            Event::Tab(T_FIX) | Event::Key(Key::Char('f')) => self.do_fix(&port),
            Event::Row(i) => self.select(*i),
            Event::Key(Key::Up) => self.select(self.sel.saturating_sub(1)),
            Event::Key(Key::Down) => self.select(self.sel + 1),
            Event::Key(k) => {
                self.list.key(k);
            }
            other => {
                self.list.event(other, A_LIST);
            }
        }
        self.dirty = true;
        Flow::Stay
    }

    fn scroll_px(&mut self, px: i32, _cx: &AppCx) -> bool {
        self.list.wheel_px(px)
    }
    fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }
    fn keeps_screen_on(&self) -> bool {
        self.active.is_some()
    }
    fn wants_redraw(&mut self) -> bool {
        self.poll();
        self.ticks += 1;
        std::mem::take(&mut self.dirty) | self.live()
    }
    fn render(&mut self, g: &mut crate::gfx::Gfx, area: crate::ui::R, s: f32, cx: &AppCx, tick: u64) {
        self.poll();
        self.draw(g, area, s, cx, tick);
    }
}
