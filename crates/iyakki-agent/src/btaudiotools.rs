//! Bluetooth audio, the receiving side (a phone or laptop plays through this computer's speakers; F-NET-22): the sound goes to the
//! speakers as a mixer source named after the device, so the person's volume, mute and ducking apply, and the assistant has tools for it.
//!
//! * `bluetooth_audio_status` (free): who is playing, the codec, packets lost, or why nothing can.
//! * `bluetooth_audio_connect` and `bluetooth_audio_disconnect` (each asks): this computer connects to a paired phone for audio, or ends it.
//! * `bluetooth_audio_pairing` (asks): lets a new phone find and pair with this computer for two minutes. The number to compare is
//!   answered by the person on Settings -> Bluetooth, never by the assistant.
//!
//! The work is [`iyakki_core_local::devctl::a2dp::AudioControl`]; this file finds the device, writes the card and waits for the result.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use iyakki_core_api::btaudio::{BtAudio, BtAudioAction};
use iyakki_core_local::btaudio::{LocalBtAudio, SendRoute};
use iyakki_core_local::devctl::a2dp::AudioControl;
use iyakki_core_local::devctl::a2dp_send::SendControl;
use iyakki_core_local::devctl::bthid::Devices;
pub use iyakki_core_local::btaudio::test_tone;
pub use iyakki_shared::btaudiotools::{PROMPT_RULE};


pub const STATUS_TOOL: &str = "bluetooth_audio_status";
pub const WRITE_TOOLS: [&str; 3] = ["bluetooth_audio_connect", "bluetooth_audio_disconnect", "bluetooth_audio_pairing"];

pub const SEND_STATUS_TOOL: &str = "bluetooth_audio_send_status";
pub const SEND_TOOLS: [&str; 3] = ["bluetooth_audio_send_start", "bluetooth_audio_send_stop", "bluetooth_audio_send_tone"];

/// Does `bluetooth_audio_send_start` route the whole computer's sound (default yes; `false` sends nothing until a test tone)?
fn whole_computer(a: &Value) -> bool {
    a.get("whole_computer").and_then(Value::as_bool).unwrap_or(true)
}
/// Are the machine's own speakers silent while it is routed (default yes: headphones only)?
fn mute_speakers(a: &Value) -> bool {
    a.get("mute_speakers").and_then(Value::as_bool).unwrap_or(true)
}

pub fn is_write(n: &str) -> bool {
    WRITE_TOOLS.contains(&n) || SEND_TOOLS.contains(&n)
}

pub fn is_audio_tool(n: &str) -> bool {
    n == STATUS_TOOL || n == SEND_STATUS_TOOL || WRITE_TOOLS.contains(&n) || SEND_TOOLS.contains(&n)
}

/// The mixer tap of this computer as the route the core's handle attaches the sound to.
struct RouteHandle(Arc<crate::btroute::BtRoute>);

impl SendRoute for RouteHandle {
    fn attach(&self, sender: Arc<dyn SendControl>, mute_local: bool) {
        self.0.attach(sender, mute_local)
    }
    fn detach(&self) {
        self.0.detach()
    }
    fn listening(&self) -> bool {
        self.0.listening()
    }
    fn muted(&self) -> bool {
        self.0.muted()
    }
    fn status(&self) -> String {
        self.0.status()
    }
}

/// The chat tools of Bluetooth audio over the core's handle ([`LocalBtAudio`]): this file words the tools, the handle does the work.
pub struct BtAudioTools {
    inner: Arc<dyn Tools>,
    core: LocalBtAudio,
}

impl BtAudioTools {
    pub fn new(inner: Arc<dyn Tools>, devs: Arc<dyn Devices>, audio: Arc<dyn AudioControl>) -> BtAudioTools {
        BtAudioTools { inner, core: LocalBtAudio::new(devs, audio).with_route(Arc::new(RouteHandle(Arc::new(crate::btroute::BtRoute::new())))) }
    }

    /// The tools over this computer's real Bluetooth.
    pub fn real(inner: Arc<dyn Tools>) -> BtAudioTools {
        BtAudioTools { inner, core: LocalBtAudio::real().with_route(Arc::new(RouteHandle(crate::btroute::global()))) }
    }

    /// The route the whole computer's sound takes while it is sent (the machine's own, [`crate::btroute::global`]).
    pub fn with_route(mut self, route: Arc<crate::btroute::BtRoute>) -> BtAudioTools {
        self.core = self.core.with_route(Arc::new(RouteHandle(route)));
        self
    }

    /// Remember the last headphones in this file.
    pub fn with_memory(mut self, file: std::path::PathBuf) -> BtAudioTools {
        self.core = self.core.with_memory(file);
        self
    }

    /// The sending side (sound to headphones).
    pub fn with_send(mut self, send: Arc<dyn SendControl>) -> BtAudioTools {
        self.core = self.core.with_send(send);
        self
    }

    pub fn with_wait(mut self, wait: Duration, poll: Duration) -> BtAudioTools {
        self.core = self.core.with_wait(wait, poll);
        self
    }

    /// Is the whole computer's sound being sent to a pair right now?
    pub fn sending(&self) -> bool {
        self.core.sending()
    }

    /// The paired headphones and speakers as (address, name), for the Sound page's choice.
    pub fn sinks(&self) -> Vec<(String, String)> {
        self.core.sinks()
    }

    /// One line for the Sound page: where the whole computer's sound goes (or why not), from the sender and the route.
    pub fn sound_line(&self) -> String {
        self.core.sound_line()
    }

    /// The remembered headphones as (address, mute speakers).
    pub fn remembered(&self) -> Option<(String, bool)> {
        self.core.remembered()
    }

    /// The change a tool name and its arguments mean.
    fn action(name: &str, args: &Value) -> Option<BtAudioAction> {
        let device = || args.get("device").and_then(Value::as_str).unwrap_or("").to_string();
        Some(match name {
            "bluetooth_audio_connect" => BtAudioAction::Connect { device: device() },
            "bluetooth_audio_disconnect" => BtAudioAction::Disconnect,
            "bluetooth_audio_pairing" => BtAudioAction::Pairing { on: args.get("on").and_then(Value::as_bool).unwrap_or(true) },
            "bluetooth_audio_send_start" => BtAudioAction::SendStart { device: device(), whole_computer: whole_computer(args), mute_speakers: mute_speakers(args) },
            "bluetooth_audio_send_stop" => BtAudioAction::SendStop,
            "bluetooth_audio_send_tone" => BtAudioAction::SendTone,
            _ => return None,
        })
    }

    fn card(&self, name: &str, args: &Value) -> Result<String, String> {
        match Self::action(name, args) {
            Some(a) => self.core.plan(&a),
            None => Err("not a Bluetooth audio tool".into()),
        }
    }
}

impl Tools for BtAudioTools {

    fn inner_tools(&self) -> Option<&dyn Tools> {

        Some(&*self.inner)

    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_audio_tool(&d.spec.name));
        let dev = json!({"type": "object", "properties": {"device": {"type": "string", "description": "The paired phone or laptop's name or address; leave out when only one is paired."}}});
        let ask = |name: &str, desc: &str, schema: Value| ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema }, risk: Risk::Ask, untrusted: false };
        v.push(ToolDef { spec: ToolSpec { name: STATUS_TOOL.into(), description: "Say whether a phone or laptop is playing through this computer over Bluetooth (A2DP), the codec, lost packets, and if nothing can play, why.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ask("bluetooth_audio_connect", "Connect a paired phone or laptop for Bluetooth audio so its sound plays on this computer's speakers. Waits up to 20 seconds. The person approves a card.", dev.clone()));
        v.push(ask("bluetooth_audio_disconnect", "End the Bluetooth audio connection; the phone stays paired. The person approves a card.", json!({"type": "object", "properties": {}})));
        v.push(ToolDef { spec: ToolSpec { name: SEND_STATUS_TOOL.into(), description: "Say whether this computer is sending sound to Bluetooth headphones or a speaker, to which, with which codec, and if not, why.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ask("bluetooth_audio_send_start", "Connect paired Bluetooth headphones or a speaker and send ALL of this computer's sound to them (SBC); by default the computer's own speakers go silent while it runs (mute_speakers=false keeps them on; whole_computer=false only connects, for the test tone). Waits for the set-up. The person approves a card.", json!({"type": "object", "properties": {"device": {"type": "string", "description": "The paired headphones or speaker's name or address; leave out when only one is paired."}, "whole_computer": {"type": "boolean", "description": "true (default): all of this computer's sound goes to them."}, "mute_speakers": {"type": "boolean", "description": "true (default): the computer's own speakers are silent while it is routed."}}})));
        v.push(ask("bluetooth_audio_send_stop", "Stop sending sound to the headphones or speaker; they stay paired. The person approves a card.", json!({"type": "object", "properties": {}})));
        v.push(ask("bluetooth_audio_send_tone", "Play a two second quiet test tone through the headphones or speaker that sound is being sent to. The person approves a card.", json!({"type": "object", "properties": {}})));
        v.push(ask("bluetooth_audio_pairing", "Let a new phone pair with this computer for two minutes (or close pairing with on=false). The person approves a card and answers the number comparison on screen.", json!({"type": "object", "properties": {"on": {"type": "boolean", "description": "true (default) opens pairing, false closes it."}}})));
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            STATUS_TOOL => Ok(self.core.receive_status()),
            SEND_STATUS_TOOL => Ok(self.core.send_status()),
            _ => match Self::action(name, args) {
                Some(a) => self.core.apply(&a),
                None => self.inner.call(name, args),
            },
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        if is_write(name) {
            return match self.card(name, args) {
                Ok(c) => c.split(':').next().unwrap_or("").to_string(),
                Err(e) => format!("{name} (invalid: {e})"),
            };
        }
        if name == STATUS_TOOL || name == SEND_STATUS_TOOL {
            return "Show the Bluetooth audio state".into();
        }
        self.inner.describe(name, args)
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if is_write(name) {
            return self.card(name, args).ok();
        }
        if name == STATUS_TOOL || name == SEND_STATUS_TOOL { None } else { self.inner.preview(name, args) }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if is_write(name) {
            return self.card(name, args).err();
        }
        if name == STATUS_TOOL || name == SEND_STATUS_TOOL { None } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_audio_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !is_write(name) && self.inner.approved_always(name)
    }
}

// ------------------------------------------------------------------------------------------------ the speaker

/// Sound for a phone's stream: a media source of the mixer (or the play command when the mixer is off) named after the device.
struct SpeakerFactory {
    play_cmd: String,
}

struct SpeakerOutput(crate::audiosink::Output);

impl iyakki_core_local::devctl::a2dp::Output for SpeakerOutput {
    fn write(&mut self, pcm: &[i16]) -> Result<(), String> {
        self.0.write(pcm)
    }
}

impl iyakki_core_local::devctl::a2dp::OutputFactory for SpeakerFactory {
    fn open(&mut self, rate: u32, channels: usize, label: &str) -> Result<Box<dyn iyakki_core_local::devctl::a2dp::Output>, String> {
        let out = crate::audiosink::open_output(crate::mixer::SourceKind::Media, rate, channels as u16, label, &self.play_cmd)?;
        Ok(Box::new(SpeakerOutput(out)))
    }
}

/// Registers the speaker for Bluetooth audio (the program calls this once, with the play command of Settings -> Voice).
pub fn install(play_cmd: impl Fn() -> String + Send + Sync + 'static) {
    // the core's own handle (`Os::bt_audio`) sends the whole computer's sound through the same tap
    iyakki_core_local::btaudio::set_global_route(Arc::new(RouteHandle(crate::btroute::global())));
    iyakki_core_local::devctl::a2dp_sys::set_output_factory(move || Box::new(SpeakerFactory { play_cmd: play_cmd() }));
    // AAC for Bluetooth goes through the person's ffmpeg (the audio and video add-on); nothing starts until a stream does
    crate::btaac::install();
}

// ------------------------------------------------------------------------------------------------ the phone's remote control

/// What a volume or play key of a phone or headset does here: the master volume, or IYAKKI's own player (the radio and the music it plays).
/// Returns a short note for the audit line, empty when nothing was taken.
pub fn do_remote(svc: &crate::services::Services, act: iyakki_core_local::devctl::avrcp::Act) -> String {
    do_remote_with(svc, &crate::media::shared(), act)
}

/// [`do_remote`] with the music player named (the shell's shared one, or a fake in a test).
pub fn do_remote_with(svc: &crate::services::Services, player: &crate::media::MediaPlayer, act: iyakki_core_local::devctl::avrcp::Act) -> String {
    use iyakki_core_local::devctl::avrcp::{Act, MediaKey};
    match act {
        Act::SetPercent(p) => svc.master_set(u32::from(p)).map(|m| format!("volume {}", m.percent)).unwrap_or_default(),
        Act::Step(d) => svc.master_nudge(i32::from(d)).map(|m| format!("volume {}", m.percent)).unwrap_or_default(),
        Act::ToggleMute => svc.master_toggle_mute().map(|m| if m.muted { "muted".to_string() } else { format!("volume {}", m.percent) }).unwrap_or_default(),
        Act::Media(MediaKey::Play) => if svc.radio.resume() { "play".into() } else { String::new() },
        Act::Media(MediaKey::Pause) => if svc.radio.pause() { "pause".into() } else { String::new() },
        Act::Media(MediaKey::Next) => player.skip(1).map(|l| format!("next {l}")).unwrap_or_default(),
        Act::Media(MediaKey::Previous) => player.skip(-1).map(|l| format!("previous {l}")).unwrap_or_default(),
        Act::Media(MediaKey::Stop) => {
            if svc.radio.is_active() {
                svc.radio.stop();
                "stop".into()
            } else {
                String::new()
            }
        }
    }
}

/// While somebody is signed in, the keys of a connected phone or headset act on the player and the volume (the phone's remote channel is
/// answered at all times; without a session nothing is changed). `audit` gets one line per key that did something.
pub fn install_remote(svc: &std::sync::Arc<crate::services::Services>, user: &str, audit: impl Fn(&str, &str) + Send + Sync + 'static) {
    let audit = std::sync::Arc::new(audit);
    crate::btroute::global().on_lost("radio", route_lost_pauser(svc, user, audit.clone()));
    crate::btroute::global().on_lost("video-tile", crate::routepause::pauser(crate::routepause::global(), user, audit.clone()));
    crate::btroute::global().on_lost("media-player", media_route_lost_pauser(&crate::media::shared(), user, audit.clone()));
    let (weak, user) = (std::sync::Arc::downgrade(svc), user.to_string());
    iyakki_core_local::devctl::a2dp_sys::set_remote_handler(move |act| {
        if let Some(s) = weak.upgrade() {
            let did = do_remote(&s, act);
            if !did.is_empty() {
                audit(&user, &format!("bluetooth.remote {did}"));
            }
        }
    });
}

/// What pauses the radio (and the music the radio plays) when the Bluetooth headphones leave (F-LIFE-14): playback does not move to the
/// room speakers. One audit line when something was paused.
pub fn route_lost_pauser(svc: &std::sync::Arc<crate::services::Services>, user: &str, audit: std::sync::Arc<dyn Fn(&str, &str) + Send + Sync>) -> crate::btroute::Pauser {
    let (weak, user) = (std::sync::Arc::downgrade(svc), user.to_string());
    std::sync::Arc::new(move || {
        let did = weak.upgrade().is_some_and(|s| s.radio.pause());
        if did {
            audit(&user, "bluetooth.route_lost paused the radio");
        }
        did
    })
}

/// What stops the external music player when the Bluetooth headphones leave (F-LIFE-14): the program cannot be paused, so it is stopped and
/// its queue and place are kept (press play to carry on on the speakers). One audit line when something was stopped.
pub fn media_route_lost_pauser(player: &std::sync::Arc<crate::media::MediaPlayer>, user: &str, audit: std::sync::Arc<dyn Fn(&str, &str) + Send + Sync>) -> crate::btroute::Pauser {
    let (player, user) = (player.clone(), user.to_string());
    std::sync::Arc::new(move || {
        let did = player.pause_for_route_lost();
        if did {
            audit(&user, "bluetooth.route_lost stopped the music player (the queue is kept)");
        }
        did
    })
}

/// Sign-out: the keys change nothing until somebody signs in again.
pub fn clear_remote() {
    crate::btroute::global().forget_pauser("radio");
    crate::btroute::global().forget_pauser("video-tile");
    crate::btroute::global().forget_pauser("media-player");
    iyakki_core_local::devctl::a2dp_sys::clear_remote_handler();
}

#[cfg(test)]
#[path = "btaudiotools_tests.rs"]
mod tests;
