//! What the CHECKUP tile asks of the rest of the system (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12).
//!
//! The tile (`checkup.rs`) names no `Services`, no session and no audit log: it asks a [`CheckupPort`] for the machine's inventory, the test
//! tone, a camera picture, the voice settings, the person's data folder, the dev cluster and the audit line. The real port ([`CxPort`]) is the
//! only place that reads the session and calls the core. [`Parts`] stays the way tests replace each reader.

use std::any::Any;
use std::sync::Arc;

use iyakki_shared::checkup::{self, Inventory, ScreenOut};
use iyakki_shared::voicedesk::Recorders;

use super::browser_media;
use super::AppCx;
use crate::camera::probe::{self, Shot};

/// How long a camera picture may take.
const CAMERA_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

pub trait CheckupPort {
    /// What this machine has (screens, sound, network, GPUs ...), read again every time.
    fn inventory(&self) -> Inventory;
    /// Play the test tone through the audio entry point; the returned value keeps the sound alive (dropping it stops it).
    fn play_tone(&self) -> Result<Box<dyn Any + Send>, String>;
    /// A job that takes one camera picture (it runs on a thread of its own).
    fn camera_job(&self) -> Box<dyn FnOnce() -> Shot + Send>;
    fn voice_settings(&self) -> iyakki_shared::voice::VoiceSettings;
    /// Where the signed result file goes.
    fn data_dir(&self) -> std::path::PathBuf;
    /// The services that know the dev cluster's state.
    fn k3s(&self) -> Arc<dyn iyakki_agent_api::DevCluster>;
    /// A line in the audit log for the signed-in person.
    fn audit(&self, action: &str, subject: &str);
}

/// What the tile reads and plays; tests replace every part. The closures get the port (the real ones ask it, the test ones ignore it).
#[derive(Clone)]
pub struct Parts {
    pub inventory: Arc<dyn Fn(&dyn CheckupPort) -> Inventory + Send + Sync>,
    pub recorders: Arc<dyn Recorders>,
    /// Play the test tone through the audio entry point; the returned value keeps the sound alive (dropping it stops it).
    pub play: Arc<dyn Fn(&dyn CheckupPort) -> Result<Box<dyn Any + Send>, String> + Send + Sync>,
    /// Take one camera picture (runs on a thread of its own).
    pub camera: Arc<dyn Fn(&dyn CheckupPort) -> Box<dyn FnOnce() -> Shot + Send> + Send + Sync>,
    /// Tell the shell's sign what is capturing ( bits).
    pub sign: Arc<dyn Fn(u8) + Send + Sync>,
    /// The person clicked the sign: stop everything that captures.
    pub stop_asked: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl Parts {
    pub fn real() -> Parts {
        Parts {
            inventory: Arc::new(|p| p.inventory()),
            recorders: crate::recorders(),
            play: Arc::new(|p| p.play_tone()),
            camera: Arc::new(|p| p.camera_job()),
            sign: Arc::new(browser_media::publish_capture),
            stop_asked: Arc::new(browser_media::take_stop),
        }
    }
}

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl CheckupPort for CxPort<'_> {
    fn inventory(&self) -> Inventory {
        real_inventory(self.0)
    }
    fn play_tone(&self) -> Result<Box<dyn Any + Send>, String> {
        use iyakki_core_api::SoundKind;
        self.0.core.mixer().play_wav(SoundKind::Notification, "Check-up tone", &crate::agent().b0().checkup().tone_wav())
    }
    fn camera_job(&self) -> Box<dyn FnOnce() -> Shot + Send> {
        let home = std::path::PathBuf::from(format!("/home/{}", self.0.session.user));
        Box::new(move || probe::system_grab(&home, CAMERA_WAIT))
    }
    fn voice_settings(&self) -> iyakki_shared::voice::VoiceSettings {
        self.0.voice_settings()
    }
    fn data_dir(&self) -> std::path::PathBuf {
        self.0.data_dir().to_path_buf()
    }
    fn k3s(&self) -> Arc<dyn iyakki_agent_api::DevCluster> {
        self.0.k3sdev()
    }
    fn audit(&self, action: &str, subject: &str) {
        self.0.audit.record(&self.0.session.user, action, subject);
    }
}

pub(super) fn real_inventory(cx: &AppCx) -> Inventory {
    let screens = crate::display::outputs::hub().status().outputs.iter().map(|o| ScreenOut { name: o.name.clone(), connected: o.connected, current: o.current }).collect();
    let mut inv = cx.agent.b0().checkup().inventory(screens);
    inv.sound_chosen = cx.sound_settings().output;
    inv.k3s_gpu = k3s_sees(&*cx.k3sdev());
    inv.sound_chosen_present = inv.sound_chosen.is_empty() || iyakki_shared::audioout::choose(&crate::agent().helpers_b4().sound_outputs(), &inv.sound_chosen).chosen;
    inv
}

/// What the managed dev cluster says about GPUs on its nodes (read-only `kubectl get nodes -o json`). A cluster that is not running is not asked.
fn k3s_sees(k: &dyn iyakki_agent_api::DevCluster) -> checkup::K3sSees {
    let state = k.health().state;
    if state != "running" {
        return checkup::K3sSees::NoCluster(state);
    }
    let o = k.kubectl_cap(&["get", "nodes", "-o", "json"], 512 * 1024);
    if !o.ok() {
        return checkup::K3sSees::Unreadable(o.combined().lines().last().unwrap_or("no answer").trim().chars().take(120).collect());
    }
    let n: u32 = [iyakki_shared::gpu_workloads::GPU_RESOURCE, iyakki_shared::gpu_workloads::NVIDIA_RESOURCE, iyakki_shared::gpu_rocm::RESOURCE].iter().map(|r| crate::agent().gpu_workloads_node_resource_count(&o.stdout, r)).sum();
    checkup::K3sSees::Count(n)
}


// ------------------------------------------------------------------------------------------------ the tile crate's port

use std::cell::RefCell;

use iyakki_shared::checkup as sc;
use iyakki_shared::doctor::{Action, Fix};
use iyakki_tile_api::checkup as t;

pub(super) fn step_to_tile(i: sc::StepId) -> t::StepId {
    match i {
        sc::StepId::Display => t::StepId::Display,
        sc::StepId::Touch => t::StepId::Touch,
        sc::StepId::Keyboard => t::StepId::Keyboard,
        sc::StepId::Pointer => t::StepId::Pointer,
        sc::StepId::Speaker => t::StepId::Speaker,
        sc::StepId::Microphone => t::StepId::Microphone,
        sc::StepId::Camera => t::StepId::Camera,
        sc::StepId::Monitor => t::StepId::Monitor,
        sc::StepId::Network => t::StepId::Network,
        sc::StepId::Wifi => t::StepId::Wifi,
        sc::StepId::Bluetooth => t::StepId::Bluetooth,
        sc::StepId::Power => t::StepId::Power,
        sc::StepId::Gpu => t::StepId::Gpu,
        sc::StepId::Usb => t::StepId::Usb,
    }
}

pub(super) fn step_from_tile(i: t::StepId) -> sc::StepId {
    match i {
        t::StepId::Display => sc::StepId::Display,
        t::StepId::Touch => sc::StepId::Touch,
        t::StepId::Keyboard => sc::StepId::Keyboard,
        t::StepId::Pointer => sc::StepId::Pointer,
        t::StepId::Speaker => sc::StepId::Speaker,
        t::StepId::Microphone => sc::StepId::Microphone,
        t::StepId::Camera => sc::StepId::Camera,
        t::StepId::Monitor => sc::StepId::Monitor,
        t::StepId::Network => sc::StepId::Network,
        t::StepId::Wifi => sc::StepId::Wifi,
        t::StepId::Bluetooth => sc::StepId::Bluetooth,
        t::StepId::Power => sc::StepId::Power,
        t::StepId::Gpu => sc::StepId::Gpu,
        t::StepId::Usb => sc::StepId::Usb,
    }
}

fn outcome_to_tile(o: sc::Outcome) -> t::Outcome {
    match o {
        sc::Outcome::Pending => t::Outcome::Pending,
        sc::Outcome::Pass => t::Outcome::Pass,
        sc::Outcome::Fail => t::Outcome::Fail,
        sc::Outcome::Skipped => t::Outcome::Skipped,
    }
}

fn outcome_from_tile(o: t::Outcome) -> sc::Outcome {
    match o {
        t::Outcome::Pending => sc::Outcome::Pending,
        t::Outcome::Pass => sc::Outcome::Pass,
        t::Outcome::Fail => sc::Outcome::Fail,
        t::Outcome::Skipped => sc::Outcome::Skipped,
    }
}

fn said_from_tile(s: t::Said) -> sc::Said {
    match s {
        t::Said::Yes => sc::Said::Yes,
        t::Said::No => sc::Said::No,
        t::Said::Skip => sc::Said::Skip,
    }
}

pub(super) fn result_to_tile(r: sc::StepResult) -> t::StepResult {
    let fix = r.fix.map(|f| t::Fix {
        label: f.label,
        action: match f.action {
            Action::Settings(i) => t::FixAction::Settings(i),
            Action::ExtendDeveloperMode => t::FixAction::ExtendDeveloperMode,
        },
    });
    t::StepResult { id: step_to_tile(r.id), outcome: outcome_to_tile(r.outcome), note: r.note, advice: r.advice, fix, doctor: r.doctor }
}

pub(super) fn result_from_tile(r: &t::StepResult) -> sc::StepResult {
    let fix = r.fix.as_ref().map(|f| Fix {
        label: f.label,
        action: match f.action {
            t::FixAction::Settings(i) => Action::Settings(i),
            t::FixAction::ExtendDeveloperMode => Action::ExtendDeveloperMode,
        },
    });
    sc::StepResult { id: step_from_tile(r.id), outcome: outcome_from_tile(r.outcome), note: r.note.clone(), advice: r.advice.clone(), fix, doctor: r.doctor }
}

fn cam_to_tile(h: sc::CamHeard) -> t::CamHeard {
    match h {
        sc::CamHeard::NoCamera(w) => t::CamHeard::NoCamera(w),
        sc::CamHeard::Picture { name, width, height, spread } => t::CamHeard::Picture { name, width, height, spread },
        sc::CamHeard::Failed { name, why } => t::CamHeard::Failed { name, why },
    }
}

fn cam_from_tile(h: &t::CamHeard) -> sc::CamHeard {
    match h {
        t::CamHeard::NoCamera(w) => sc::CamHeard::NoCamera(w.clone()),
        t::CamHeard::Picture { name, width, height, spread } => sc::CamHeard::Picture { name: name.clone(), width: *width, height: *height, spread: *spread },
        t::CamHeard::Failed { name, why } => sc::CamHeard::Failed { name: name.clone(), why: why.clone() },
    }
}

/// The shell's recording as the tile crate's.
struct TileCap(Box<dyn Capture>);

impl t::MicCapture for TileCap {
    fn level(&self) -> f32 {
        self.0.level()
    }
    fn elapsed(&self) -> std::time::Duration {
        self.0.elapsed()
    }
    fn is_full(&self) -> bool {
        self.0.is_full()
    }
    fn is_alive(&mut self) -> bool {
        self.0.is_alive()
    }
    fn stop(self: Box<Self>) -> Result<Vec<i16>, String> {
        self.0.stop()
    }
}

use iyakki_shared::voicedesk::Capture;

/// What the tile crate's port is made of: the shell's port ([`CheckupPort`], the only code that reads the session and calls the core), the
/// replaceable parts, and the machine's last reading (the tile never sees it; the verdicts are judged here with the pure rules of
/// `iyakki_shared::checkup`).
pub struct Bridge<'a> {
    pub port: &'a dyn CheckupPort,
    pub parts: &'a Parts,
    pub inv: &'a RefCell<Inventory>,
}

impl t::CheckupPort for Bridge<'_> {
    fn refresh(&self) -> usize {
        let i = (self.parts.inventory)(self.port);
        let n = i.touchscreens;
        *self.inv.borrow_mut() = i;
        n
    }
    fn verdict(&self, check: &t::Check) -> t::StepResult {
        let inv = self.inv.borrow();
        let r = match check {
            t::Check::Display(s) => sc::display(said_from_tile(*s)),
            t::Check::Touch(s) => sc::touch(&inv, said_from_tile(*s)),
            t::Check::Keyboard { keys, said } => sc::keyboard(&inv, *keys, said_from_tile(*said)),
            t::Check::Pointer { clicked, said } => sc::pointer(&inv, *clicked, said_from_tile(*said)),
            t::Check::Speaker { played, said } => sc::speaker(&inv, played, said_from_tile(*said)),
            t::Check::Microphone { heard, name } => {
                let h = match heard {
                    t::MicHeard::Samples(p) => sc::MicHeard::Samples(p.clone()),
                    t::MicHeard::NoMic(w) => sc::MicHeard::NoMic(w.clone()),
                    t::MicHeard::Failed(w) => sc::MicHeard::Failed(w.clone()),
                };
                sc::microphone(&inv, &h, name)
            }
            t::Check::Camera(h) => sc::camera(&cam_from_tile(h)),
            t::Check::Machine(id) => match id {
                t::StepId::Monitor => sc::monitor(&inv),
                t::StepId::Network => sc::network(&inv),
                t::StepId::Wifi => sc::wifi(&inv),
                t::StepId::Bluetooth => sc::bluetooth(&inv),
                t::StepId::Power => sc::power(&inv),
                t::StepId::Gpu => sc::gpu(&inv),
                t::StepId::Usb => sc::usb(&inv),
                other => sc::skipped(step_from_tile(*other)),
            },
            t::Check::Skipped(id) => sc::skipped(step_from_tile(*id)),
        };
        result_to_tile(r)
    }
    fn play_tone(&self) -> Result<Box<dyn Any + Send>, String> {
        (self.parts.play)(self.port)
    }
    fn camera_job(&self) -> Box<dyn FnOnce() -> t::Shot + Send> {
        let job = (self.parts.camera)(self.port);
        Box::new(move || {
            let s = job();
            t::Shot { heard: cam_to_tile(s.heard), picture: s.picture }
        })
    }
    fn start_mic(&self) -> Result<t::Listening, t::MicStart> {
        let vs = self.port.voice_settings();
        let t2 = self.inv.borrow().t2_mic_hidden;
        match sc::start_mic(self.parts.recorders.as_ref(), &vs.mic, &vs.record_cmd, t::MIC_MAX_SECS, t2) {
            Ok(cap) => {
                let mics = self.parts.recorders.mics();
                let name = iyakki_shared::voicedesk::pick_mic(&mics, &vs.mic).map(|m| m.label.clone()).unwrap_or_default();
                Ok(t::Listening { capture: Box::new(TileCap(cap)), name })
            }
            Err(sc::MicStart::NoMic(w)) => Err(t::MicStart::NoMic(w)),
            Err(sc::MicStart::Failed(w)) => Err(t::MicStart::Failed(w)),
        }
    }
    fn keep_report(&self, results: &[t::StepResult]) {
        let results: Vec<sc::StepResult> = results.iter().map(result_from_tile).collect();
        let any = results.iter().any(|r| r.outcome != sc::Outcome::Pending);
        checkup::set_last_report(if any { checkup::report_lines(&results) } else { Vec::new() });
        // a finished check-up leaves a signed file that maps to the hardware matrix (F-CHECKUP-02); a failed write is not shown as a check-up failure
        let extra = iyakki_shared::checkup_result::Extra { k3s_state: Some(self.port.k3s().health().state), gpu: results.iter().find(|r| r.id == sc::StepId::Gpu).filter(|r| r.outcome != sc::Outcome::Pending).map(|r| (r.note.clone(), r.outcome == sc::Outcome::Pass)) };
        let machine = crate::agent().doctor_hardware_text().lines().next().unwrap_or("").to_string();
        let _ = iyakki_shared::checkup_result::save_if_complete(&self.port.data_dir(), &results, &extra, &machine, env!("CARGO_PKG_VERSION"), iyakki_clock::now_unix());
    }
    fn sign(&self, bits: u8) {
        (self.parts.sign)(bits)
    }
    fn sign_text(&self, bits: u8) -> String {
        browser_media::indicator(bits).unwrap_or_default()
    }
    fn stop_asked(&self) -> bool {
        (self.parts.stop_asked)()
    }
    fn audit(&self, action: &str, subject: &str) {
        self.port.audit(action, subject)
    }
}

#[cfg(test)]
mod mirror_tests {
    use super::*;

    #[test]
    fn the_steps_outcomes_and_sign_bits_are_the_same_in_both_crates() {
        assert_eq!(sc::StepId::ALL.len(), t::StepId::ALL.len());
        for (a, b) in sc::StepId::ALL.iter().zip(t::StepId::ALL.iter()) {
            assert_eq!(step_to_tile(*a), *b);
            assert_eq!(step_from_tile(*b), *a);
            assert_eq!((a.id(), a.title(), a.asks_the_person(), a.is_capture()), (b.id(), b.title(), b.asks_the_person(), b.is_capture()));
        }
        for (a, b) in [(sc::Outcome::Pending, t::Outcome::Pending), (sc::Outcome::Pass, t::Outcome::Pass), (sc::Outcome::Fail, t::Outcome::Fail), (sc::Outcome::Skipped, t::Outcome::Skipped)] {
            assert_eq!(outcome_to_tile(a), b);
            assert_eq!(outcome_from_tile(b), a);
            assert_eq!(a.word(), b.word());
        }
        assert_eq!((t::bits::MICROPHONE, t::bits::CAMERA), (iyakki_webview::proto::media_kind::MICROPHONE, iyakki_webview::proto::media_kind::CAMERA));
        assert_eq!(t::MIC_SECS, sc::MIC_SECS);
    }

    #[test]
    fn a_result_with_a_fix_survives_the_round_trip_and_the_summary_lines_match() {
        let r = sc::res(sc::StepId::Wifi, sc::Outcome::Fail, "no adapter", "plug one in", sc::open("Open Wi-Fi", "tab:wifi"), Some("wifi-off"));
        let back = result_from_tile(&result_to_tile(r.clone()));
        assert_eq!(back, r);
        let dev = sc::StepResult { fix: Some(Fix { label: "More time", action: Action::ExtendDeveloperMode }), ..r.clone() };
        assert_eq!(result_from_tile(&result_to_tile(dev.clone())), dev);
        let rs = vec![r, sc::StepResult::pending(sc::StepId::Usb)];
        let tr: Vec<t::StepResult> = rs.iter().cloned().map(result_to_tile).collect();
        assert_eq!(t::summary(&tr), sc::summary(&rs));
    }

    #[test]
    fn a_camera_finding_round_trips() {
        for h in [sc::CamHeard::NoCamera("none".into()), sc::CamHeard::Picture { name: "c".into(), width: 4, height: 3, spread: 9 }, sc::CamHeard::Failed { name: "c".into(), why: "busy".into() }] {
            assert_eq!(cam_from_tile(&cam_to_tile(h.clone())), h);
        }
    }
}
