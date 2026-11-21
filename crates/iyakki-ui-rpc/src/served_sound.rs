//! The sound of the machine (and the statuses the home screen shows on the app icons) over the UI socket (F-SPLIT-03, D-SPLIT-03-APPS): the mixer, the output selector's picture, the cue player and the
//! kernel facts of the sound route, all asked of the core's own `Core` handle. The UI program has no sound card of its own: what a tile plays is
//! sent to the core (`play_wav`: the bytes go up, the core's mixer plays them, the object that comes back keeps the sound alive and dropping it
//! stops it), and what a settings page shows about the outputs is the core's picture.
//!
//! Wire types of the sound: the core-api types that are not serde (`Output`, `SoundState`, `Outcome`) travel as the small twins of this file.
//! Two reads that name a folder on the machine (`outputs_at`, `card_starting`) are test seams: the UI is never given a way to read a folder of
//! its choice, so they answer nothing here (`outputs_via` and `card_starting_via` use the core's own route).

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use iyakki_core_api::audioroute::{rel_is_safe, AudioRoute, PcmHolder};
use iyakki_core_api::soundctl::{CuePlayer, SoundControl, SoundState};
use iyakki_core_api::soundtypes::{Cue, CueHandle, CueOut, Kind, Outcome, Output, SoundSettings};
use iyakki_core_api::{AppStatus, AppStatuses, Core, Mixer, MixerStats, SoundKind, StatusKind, WaitingCall};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::served::{Served, NOT_SERVED};
use crate::served_apps::{drive, Jobs};
use crate::served_hosts::{bytes_in, send};
use crate::{de, ser, Obj, Registry, RemoteServicesHandle, Rpc, ServicesRoot};

#[derive(Serialize, Deserialize, Clone, Copy)]
enum WKind {
    Analog,
    Hdmi,
    Usb,
    Bluetooth,
    Digital,
}
impl From<Kind> for WKind {
    fn from(k: Kind) -> WKind {
        match k {
            Kind::Analog => WKind::Analog,
            Kind::Hdmi => WKind::Hdmi,
            Kind::Usb => WKind::Usb,
            Kind::Bluetooth => WKind::Bluetooth,
            Kind::Digital => WKind::Digital,
        }
    }
}
impl From<WKind> for Kind {
    fn from(k: WKind) -> Kind {
        match k {
            WKind::Analog => Kind::Analog,
            WKind::Hdmi => Kind::Hdmi,
            WKind::Usb => Kind::Usb,
            WKind::Bluetooth => Kind::Bluetooth,
            WKind::Digital => Kind::Digital,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct WOutput {
    card: u32,
    device: u32,
    card_id: String,
    card_name: String,
    pcm_name: String,
    kind: WKind,
    connected: Option<bool>,
    monitor: Option<String>,
    speakers: Option<bool>,
    hdmi_no: usize,
    digital_off: bool,
    builtin_speaker: bool,
    hw_volume: Option<String>,
    dummy_volume: bool,
}
impl From<&Output> for WOutput {
    fn from(o: &Output) -> WOutput {
        WOutput { card: o.card, device: o.device, card_id: o.card_id.clone(), card_name: o.card_name.clone(), pcm_name: o.pcm_name.clone(), kind: o.kind.into(), connected: o.connected, monitor: o.monitor.clone(), speakers: o.speakers, hdmi_no: o.hdmi_no, digital_off: o.digital_off, builtin_speaker: o.builtin_speaker, hw_volume: o.hw_volume.clone(), dummy_volume: o.dummy_volume }
    }
}
impl From<WOutput> for Output {
    fn from(o: WOutput) -> Output {
        Output { card: o.card, device: o.device, card_id: o.card_id, card_name: o.card_name, pcm_name: o.pcm_name, kind: o.kind.into(), connected: o.connected, monitor: o.monitor, speakers: o.speakers, hdmi_no: o.hdmi_no, digital_off: o.digital_off, builtin_speaker: o.builtin_speaker, hw_volume: o.hw_volume, dummy_volume: o.dummy_volume }
    }
}

#[derive(Serialize, Deserialize, Default)]
struct WState {
    active: bool,
    chosen: Option<WOutput>,
    reason: String,
    by_person: bool,
    sound_server: bool,
    mix_channels: Option<u32>,
    starting: Option<String>,
    hw_ignored: Option<u32>,
    busy: Option<String>,
    softvol_card: Option<u32>,
}
impl From<SoundState> for WState {
    fn from(s: SoundState) -> WState {
        WState { active: s.active, chosen: s.chosen.as_ref().map(WOutput::from), reason: s.reason, by_person: s.by_person, sound_server: s.sound_server, mix_channels: s.mix_channels, starting: s.starting, hw_ignored: s.hw_ignored, busy: s.busy, softvol_card: s.softvol_card }
    }
}
impl From<WState> for SoundState {
    fn from(s: WState) -> SoundState {
        SoundState { active: s.active, chosen: s.chosen.map(Output::from), reason: s.reason, by_person: s.by_person, sound_server: s.sound_server, mix_channels: s.mix_channels, starting: s.starting, hw_ignored: s.hw_ignored, busy: s.busy, softvol_card: s.softvol_card }
    }
}

#[derive(Serialize, Deserialize)]
enum WOutcome {
    Played,
    Off(String),
    NoOutput(String),
    Failed(String),
}
impl From<Outcome> for WOutcome {
    fn from(o: Outcome) -> WOutcome {
        match o {
            Outcome::Played => WOutcome::Played,
            Outcome::Off(s) => WOutcome::Off(s),
            Outcome::NoOutput(s) => WOutcome::NoOutput(s),
            Outcome::Failed(s) => WOutcome::Failed(s),
        }
    }
}
impl From<WOutcome> for Outcome {
    fn from(o: WOutcome) -> Outcome {
        match o {
            WOutcome::Played => Outcome::Played,
            WOutcome::Off(s) => Outcome::Off(s),
            WOutcome::NoOutput(s) => Outcome::NoOutput(s),
            WOutcome::Failed(s) => Outcome::Failed(s),
        }
    }
}

fn kind_name(k: SoundKind) -> &'static str {
    match k {
        SoundKind::Radio => "radio",
        SoundKind::Media => "media",
        SoundKind::Notification => "notification",
        SoundKind::Speech => "speech",
        SoundKind::Alarm => "alarm",
    }
}
fn kind_of(name: &str) -> Result<SoundKind, String> {
    Ok(match name {
        "radio" => SoundKind::Radio,
        "media" => SoundKind::Media,
        "notification" => SoundKind::Notification,
        "speech" => SoundKind::Speech,
        "alarm" => SoundKind::Alarm,
        other => return Err(format!("{other} is not a kind of sound")),
    })
}

fn status_word(k: StatusKind) -> &'static str {
    match k {
        StatusKind::Running => "running",
        StatusKind::Attention => "attention",
        StatusKind::Setup => "setup",
    }
}
fn status_kind(w: &str) -> StatusKind {
    match w {
        "running" => StatusKind::Running,
        "setup" => StatusKind::Setup,
        _ => StatusKind::Attention,
    }
}

/// How long the core waits for a cue to end before it says it played (a cue is a second or two; the sound finishes by itself).
const CUE_WAIT: Duration = Duration::from_secs(20);

// ---------------------------------------------------------------------------------------------------------------- the core's end

/// The core's sound facets as the root `CoreSound`: every call is one question to the core's own handle. `audit` gets a line for each sound the
/// UI starts on the machine's speaker.
pub fn serve_core_sound(core: Arc<dyn Core>, audit: iyakki_shared::gitkeys::AuditFn) -> Obj {
    Obj::Served(Arc::new(SoundObj { core, audit, cues: Jobs::new() }))
}

struct SoundObj {
    core: Arc<dyn Core>,
    audit: iyakki_shared::gitkeys::AuditFn,
    cues: Jobs<WOutcome>,
}

/// A sound the core is playing for the UI: it plays until the object is released (or `stop` is called).
struct PlayingObj(Mutex<Option<Box<dyn std::any::Any + Send>>>);
impl Served for PlayingObj {
    fn iface(&self) -> &'static str {
        "Playing"
    }
    fn serve(&self, _reg: &Registry<'_>, method: &str, _a: Value) -> Result<Value, String> {
        match method {
            "stop" => ser(&self.0.lock().unwrap_or_else(PoisonError::into_inner).take().is_some()),
            other => Err(format!("Playing has no call {other}")),
        }
    }
}

impl Served for SoundObj {
    fn iface(&self) -> &'static str {
        "CoreSound"
    }
    fn serve(&self, reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        let c = &self.core;
        let (sound, route) = (c.sound(), c.audio_route());
        match method {
            "poll" => {
                let (id,): (u64,) = de(a)?;
                ser(&self.cues.poll(id)?)
            }
            "cancel" => de::<(u64,)>(a).and_then(|(id,)| ser(&self.cues.cancel(id))),
            // the mixer
            "mixer_stats" => ser(&c.mixer().stats().map(|s| (s.sink_open, s.detail, s.last_error))),
            "mixer_sources" => ser(&c.mixer().active_sources()),
            "mixer_play" => {
                let (kind, label, inline, upload): (String, String, String, u64) = de(a)?;
                let wav = bytes_in(reg, &inline, upload)?;
                let kind = kind_of(&kind)?;
                let playing = c.mixer().play_wav(kind, &label, &wav)?;
                (self.audit)("iyakki-ui", &format!("sound.play {} {label} ({} bytes)", kind_name(kind), wav.len()));
                ser(&reg.add(Obj::Served(Arc::new(PlayingObj(Mutex::new(Some(playing)))))))
            }
            // the output selector's picture
            "state" => ser(&WState::from(sound.state())),
            "refresh_starting" => ser(&sound.refresh_starting()),
            "prefs" => ser(&sound.prefs()),
            "outputs_via" => ser(&sound.outputs_via(route.as_ref()).iter().map(WOutput::from).collect::<Vec<_>>()),
            "card_starting_via" => ser(&sound.card_starting_via(route.as_ref())),
            "volume_note" => ser(&sound.volume_note()),
            "take_notices" => ser(&sound.take_notices()),
            "server_running" => ser(&sound.server_running()),
            "command_with_device" => {
                let (cmd, device): (String, String) = de(a)?;
                ser(&sound.command_with_device(&cmd, &device))
            }
            // the cues: a cue is a job, its end comes back as how it went
            "cue_begin" => {
                let (cue,): (Cue,) = de(a)?;
                let core = c.clone();
                ser(&self.cues.start(move |_, _| {
                    let cues = core.cues();
                    cues.play(cue, cues.out()).wait(CUE_WAIT).unwrap_or(Outcome::Played).into()
                })?)
            }
            // the kernel facts of the sound route: paths stay under /proc/asound (the route checks too)
            "asound_text" | "asound_dir" | "asound_has" => {
                let (rel,): (String,) = de(a)?;
                if !rel_is_safe(&rel) {
                    return Err(format!("{NOT_SERVED}: that path leaves /proc/asound"));
                }
                match method {
                    "asound_text" => ser(&route.asound_text(&rel)),
                    "asound_dir" => ser(&route.asound_dir(&rel)),
                    _ => ser(&route.asound_has(&rel)),
                }
            }
            "product_name" => ser(&route.product_name()),
            "pcm_holders" => {
                let (card, device): (u32, u32) = de(a)?;
                ser(&route.pcm_holders(card, device).into_iter().map(|h| (h.pid, h.name)).collect::<Vec<_>>())
            }
            "process_parent" => de::<(u32,)>(a).and_then(|(p,)| ser(&route.process_parent(p))),
            "process_name" => de::<(u32,)>(a).and_then(|(p,)| ser(&route.process_name(p))),
            "mixer_contents" => de::<(u32,)>(a).and_then(|(card,)| ser(&route.mixer_contents(card))),
            // what the apps say about themselves (the home screen's icon lines, chips and Attention list) and the person's waiting approvals
            "statuses_all" => de::<(String,)>(a).and_then(|(user,)| ser(&c.app_status().all(&user).into_iter().map(|(id, s)| (id, status_word(s.kind), s.line, s.count, s.needs)).collect::<Vec<_>>())),
            "tokens_24h" => de::<(String,)>(a).and_then(|(user,)| ser(&c.app_status().tokens_24h(&user))),
            "waiting_calls" => de::<(String,)>(a).and_then(|(user,)| ser(&c.app_status().waiting_calls(&user).into_iter().map(|w| (w.tool, w.summary)).collect::<Vec<_>>())),
            "cards_text" => ser(&route.cards_text()),
            "is_machine" => ser(&route.is_machine()),
            "card_count" => ser(&route.card_count()),
            other => Err(format!("CoreSound has no call {other}")),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------- the UI's end

/// The core's mixer, output selector, cue player and sound route as one remote object: a program holds one `Arc` of it and gives it to the
/// `SocketCore` handle four times (it is all four traits).
pub struct RemoteCoreSound {
    rpc: Rpc,
    obj: u64,
    out: Arc<dyn CueOut>,
    machine: std::sync::OnceLock<bool>,
}

/// The UI's end of a served `CoreSound` (`obj` 0 is a root); `out` is the core's cue output (`RemoteServicesHandle::m_cue_out`).
pub fn remote_core_sound(rpc: &Rpc, obj: u64, out: Arc<dyn CueOut>) -> Arc<RemoteCoreSound> {
    Arc::new(RemoteCoreSound { rpc: rpc.clone(), obj, out, machine: std::sync::OnceLock::new() })
}

impl Drop for RemoteCoreSound {
    fn drop(&mut self) {
        self.rpc.release(self.obj);
    }
}

impl RemoteCoreSound {
    fn ask<T: serde::de::DeserializeOwned + Default>(&self, method: &str, args: Value) -> T {
        self.rpc.call::<T>("CoreSound", self.obj, method, args).unwrap_or_default()
    }
}

/// A sound the core plays for this program: dropping it tells the core to let go, which stops the sound.
struct Playing {
    rpc: Rpc,
    obj: u64,
}
impl Drop for Playing {
    fn drop(&mut self) {
        self.rpc.release(self.obj);
    }
}

impl Mixer for RemoteCoreSound {
    fn stats(&self) -> Option<MixerStats> {
        self.ask::<Option<(bool, String, Option<String>)>>("mixer_stats", json!([])).map(|(sink_open, detail, last_error)| MixerStats { sink_open, detail, last_error })
    }
    fn active_sources(&self) -> Vec<(String, u32)> {
        self.ask("mixer_sources", json!([]))
    }
    fn play_wav(&self, kind: SoundKind, label: &str, wav: &[u8]) -> Result<Box<dyn std::any::Any + Send>, String> {
        let sent = send(&self.rpc, wav)?;
        let id = self.rpc.call::<u64>("CoreSound", self.obj, "mixer_play", json!([kind_name(kind), label, sent.inline, sent.id]))?;
        Ok(Box::new(Playing { rpc: self.rpc.clone(), obj: id }))
    }
}

impl SoundControl for RemoteCoreSound {
    fn state(&self) -> SoundState {
        self.ask::<WState>("state", json!([])).into()
    }
    fn refresh_starting(&self) -> Option<String> {
        self.ask("refresh_starting", json!([]))
    }
    fn prefs(&self) -> SoundSettings {
        self.rpc.call::<SoundSettings>("CoreSound", self.obj, "prefs", json!([])).unwrap_or_default()
    }
    // `outputs_at` and `card_starting` name a folder of the machine: the UI is never given a read of a folder of its choice
    fn outputs_via(&self, _route: &dyn AudioRoute) -> Vec<Output> {
        self.ask::<Vec<WOutput>>("outputs_via", json!([])).into_iter().map(Output::from).collect()
    }
    fn card_starting_via(&self, _route: &dyn AudioRoute) -> Option<u32> {
        self.ask("card_starting_via", json!([]))
    }
    fn volume_note(&self) -> Option<String> {
        self.ask("volume_note", json!([]))
    }
    fn take_notices(&self) -> Vec<String> {
        self.ask("take_notices", json!([]))
    }
    fn server_running(&self) -> bool {
        self.ask("server_running", json!([]))
    }
    fn command_with_device(&self, play_cmd: &str, device: &str) -> Result<String, String> {
        self.rpc.call::<Result<String, String>>("CoreSound", self.obj, "command_with_device", json!([play_cmd, device]))?
    }
}

impl CuePlayer for RemoteCoreSound {
    fn out(&self) -> Arc<dyn CueOut> {
        self.out.clone()
    }
    /// The core plays the cue on its own output (`out` is only the name of where; the core uses its own): a thread follows the job and the
    /// handle answers with how it ended.
    fn play(&self, cue: Cue, _out: Arc<dyn CueOut>) -> CueHandle {
        let (tx, rx) = std::sync::mpsc::channel();
        let (rpc, obj) = (self.rpc.clone(), self.obj);
        std::thread::spawn(move || {
            let outcome = match rpc.call::<u64>("CoreSound", obj, "cue_begin", json!([cue])).and_then(|job| drive::<WOutcome>(&rpc, "CoreSound", obj, job, &mut |_| true)) {
                Ok(o) => o.into(),
                Err(e) => Outcome::NoOutput(e),
            };
            let _ = tx.send(outcome);
        });
        CueHandle::pending(rx)
    }
}

impl AudioRoute for RemoteCoreSound {
    fn asound_text(&self, rel: &str) -> Option<String> {
        self.ask("asound_text", json!([rel]))
    }
    fn asound_dir(&self, rel: &str) -> Vec<String> {
        self.ask("asound_dir", json!([rel]))
    }
    fn asound_has(&self, rel: &str) -> bool {
        self.ask("asound_has", json!([rel]))
    }
    fn product_name(&self) -> Option<String> {
        self.ask("product_name", json!([]))
    }
    fn pcm_holders(&self, card: u32, device: u32) -> Vec<PcmHolder> {
        self.ask::<Vec<(u32, String)>>("pcm_holders", json!([card, device])).into_iter().map(|(pid, name)| PcmHolder { pid, name }).collect()
    }
    fn process_parent(&self, pid: u32) -> Option<u32> {
        self.ask("process_parent", json!([pid]))
    }
    fn process_name(&self, pid: u32) -> Option<String> {
        self.ask("process_name", json!([pid]))
    }
    fn mixer_contents(&self, card: u32) -> Option<String> {
        self.ask("mixer_contents", json!([card]))
    }
    fn cards_text(&self) -> String {
        self.ask("cards_text", json!([]))
    }
    fn is_machine(&self) -> bool {
        *self.machine.get_or_init(|| self.ask("is_machine", json!([])))
    }
    fn card_count(&self) -> usize {
        self.ask("card_count", json!([]))
    }
}

impl AppStatuses for RemoteCoreSound {
    fn all(&self, user: &str) -> Vec<(String, AppStatus)> {
        self.ask::<Vec<(String, String, String, usize, Option<(String, String)>)>>("statuses_all", json!([user])).into_iter().map(|(id, kind, line, count, needs)| (id, AppStatus { kind: status_kind(&kind), line, count, needs })).collect()
    }
    fn tokens_24h(&self, user: &str) -> Option<(u64, u64, u64)> {
        self.ask("tokens_24h", json!([user]))
    }
    fn waiting_calls(&self, user: &str) -> Vec<WaitingCall> {
        self.ask::<Vec<(String, String)>>("waiting_calls", json!([user])).into_iter().map(|(tool, summary)| WaitingCall { tool, summary }).collect()
    }
}

// ---------------------------------------------------------------------------------------------------------------- the accessor of the handle

/// The call of this file's accessor (the core's end; `served_apps::serve_apps` falls through to here).
pub(crate) fn serve_sound(root: &ServicesRoot, reg: &Registry<'_>, method: &str, _a: Value) -> Option<Result<Value, String>> {
    Some(match method {
        "core_sound" => crate::served::ServedParts::need(&root.parts.core, "core handle").and_then(|c| ser(&reg.add(serve_core_sound(c, root.audit.clone())))),
        _ => return None,
    })
}

impl RemoteServicesHandle {
    /// The core's sound (mixer, output selector, cues, sound route) as one remote object. A core that has none answers like a core without sound:
    /// nothing plays, no outputs, no facts.
    pub fn core_sound(&self) -> Arc<RemoteCoreSound> {
        remote_core_sound(&self.rpc, self.rpc.call::<u64>("ServicesHandle", 0, "core_sound", json!([])).unwrap_or(u64::MAX), self.m_cue_out())
    }
}
