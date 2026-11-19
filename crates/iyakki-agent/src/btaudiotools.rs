//! Bluetooth audio, the receiving side (a phone or laptop plays through this computer's speakers; F-NET-22): the sound goes to the
//! speakers as a mixer source named after the device, so the person's volume, mute and ducking apply, and the assistant has tools for it.
//!
//! * `bluetooth_audio_status` (free): who is playing, the codec, packets lost, or why nothing can.
//! * `bluetooth_audio_connect` and `bluetooth_audio_disconnect` (each asks): this computer connects to a paired phone for audio, or ends it.
//! * `bluetooth_audio_pairing` (asks): lets a new phone find and pair with this computer for two minutes. The number to compare is
//!   answered by the person on Settings -> Bluetooth, never by the assistant.
//!
//! The work is [`iyakki_devctl::a2dp::AudioControl`]; this file finds the device, writes the card and waits for the result.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use iyakki_devctl::a2dp::{AudioControl, AudioState};
use iyakki_devctl::a2dp_send::{SendControl, SendView};
use iyakki_devctl::bthid::{find_device, DeviceView, Devices};
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

/// A sending side that is not there (tests of the receiving tools, systems without Bluetooth).
struct NoSend;
impl SendControl for NoSend {
    fn view(&self) -> SendView {
        SendView { why: "sending sound to headphones is not available here".into(), ..Default::default() }
    }
    fn start(&self, _: &str, _: &str) -> Result<(), String> {
        Err("sending sound to headphones is not available here".into())
    }
    fn stop(&self) -> Result<(), String> {
        Err("no sound is being sent to a device".into())
    }
    fn write(&self, _: &[i16], _: u32, _: usize) -> Result<(), String> {
        Err("no sound is being sent to a device".into())
    }
}

/// Two seconds of 440 Hz at a quiet level, as stereo PCM of 48 kHz.
pub fn test_tone() -> Vec<i16> {
    (0..96_000).flat_map(|i| {
        let t = i as f64 / 48_000.0;
        let fade = (t.min(2.0 - t) * 20.0).clamp(0.0, 1.0);
        let v = ((t * 440.0 * std::f64::consts::TAU).sin() * 6000.0 * fade) as i16;
        [v, v]
    }).collect()
}

pub struct BtAudioTools {
    inner: Arc<dyn Tools>,
    devs: Arc<dyn Devices>,
    audio: Arc<dyn AudioControl>,
    send: Arc<dyn SendControl>,
    /// Where the whole computer's sound goes while sending (the machine's route; a private one in tests).
    route: Arc<crate::btroute::BtRoute>,
    wait: Duration,
    poll: Duration,
}

impl BtAudioTools {
    pub fn new(inner: Arc<dyn Tools>, devs: Arc<dyn Devices>, audio: Arc<dyn AudioControl>) -> BtAudioTools {
        BtAudioTools { inner, devs, audio, send: Arc::new(NoSend), route: Arc::new(crate::btroute::BtRoute::new()), wait: Duration::from_secs(20), poll: Duration::from_millis(500) }
    }

    /// The tools over this computer's real Bluetooth.
    pub fn real(inner: Arc<dyn Tools>) -> BtAudioTools {
        BtAudioTools::new(inner, Arc::new(iyakki_devctl::bthid_sys::RealDevices), Arc::new(iyakki_devctl::a2dp_sys::RealAudio)).with_send(iyakki_devctl::a2dp_send_sys::real_sender()).with_route(crate::btroute::global())
    }

    /// The route the whole computer's sound takes while it is sent (the machine's own, [`crate::btroute::global`]).
    pub fn with_route(mut self, route: Arc<crate::btroute::BtRoute>) -> BtAudioTools {
        self.route = route;
        self
    }

    /// The sending side (sound to headphones).
    pub fn with_send(mut self, send: Arc<dyn SendControl>) -> BtAudioTools {
        self.send = send;
        self
    }

    /// The paired headphones or speaker a sending tool means: the one named, else the only one paired.
    fn sink(&self, args: &Value) -> Result<DeviceView, String> {
        let list: Vec<DeviceView> = self.devs.list().into_iter().filter(|d| d.kind == "headphones" || d.kind == "speaker").collect();
        let who = args.get("device").and_then(Value::as_str).unwrap_or("").trim();
        if !who.is_empty() {
            return find_device(&list, who).map_err(|e| e.replace("device", "pair of headphones or speaker"));
        }
        match list.len() {
            0 => Err("no headphones or speaker are paired: put them in pairing mode and pair them in Settings -> Bluetooth first".into()),
            1 => Ok(list[0].clone()),
            _ => Err("more than one pair of headphones or speaker is paired: name the one to use (bluetooth_list shows them)".into()),
        }
    }

    pub fn with_wait(mut self, wait: Duration, poll: Duration) -> BtAudioTools {
        self.wait = wait;
        self.poll = poll;
        self
    }

    /// The paired device an audio tool means: the one named, else the only paired phone or laptop.
    fn source(&self, args: &Value) -> Result<DeviceView, String> {
        let list: Vec<DeviceView> = self.devs.list().into_iter().filter(|d| d.kind == "phone" || d.kind == "computer").collect();
        let who = args.get("device").and_then(Value::as_str).unwrap_or("").trim();
        if !who.is_empty() {
            return find_device(&list, who).map_err(|e| e.replace("device", "phone or laptop"));
        }
        match list.len() {
            0 => Err("no phone or laptop is paired: let a phone pair (bluetooth_audio_pairing) or pair one in Settings -> Bluetooth first".into()),
            1 => Ok(list[0].clone()),
            _ => Err("more than one phone or laptop is paired: name the one to use (bluetooth_list shows them)".into()),
        }
    }

    fn card(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "bluetooth_audio_connect" => {
                let d = self.source(args)?;
                Ok(format!("Connect “{}” ({}) for Bluetooth audio: when it plays music or video, the sound comes out of this computer's speakers until it is disconnected.", d.title(), d.address))
            }
            "bluetooth_audio_disconnect" => {
                let v = self.audio.view();
                match v.device {
                    Some(d) => Ok(format!("Disconnect “{}” from Bluetooth audio: its sound stops. It stays paired.", d.title())),
                    None => Err("no phone or laptop is connected for audio".into()),
                }
            }
            "bluetooth_audio_pairing" => {
                if args.get("on").and_then(Value::as_bool).unwrap_or(true) {
                    Ok("Make this computer visible to phones for two minutes so a phone can pair with it for audio. The person must be at the phone and answer the number comparison on this screen. Nobody else can pair afterwards.".into())
                } else {
                    Ok("Close pairing for phones now.".into())
                }
            }
            "bluetooth_audio_send_start" => {
                let d = self.sink(args)?;
                Ok(format!("Send sound to “{}” ({}): this computer connects to it and plays through it until you stop it. Only SBC is sent. {}", d.title(), d.address, if whole_computer(args) { if mute_speakers(args) { "All of this computer's sound goes to them and its own speakers are silent until you stop it." } else { "All of this computer's sound goes to them and its own speakers play too." } } else { "Only the test tone goes to them; the sound of this computer stays on its speakers." }))
            }
            "bluetooth_audio_send_stop" => {
                let v = self.send.view();
                if v.active { Ok(format!("Stop sending sound to “{}”. It stays paired.", v.device)) } else { Err("no sound is being sent to a device".into()) }
            }
            "bluetooth_audio_send_tone" => {
                let v = self.send.view();
                if v.active { Ok(format!("Play a two second test tone through “{}” at a quiet level.", v.device)) } else { Err("start sending to the headphones first (bluetooth_audio_send_start)".into()) }
            }
            _ => Err("not a Bluetooth audio tool".into()),
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
            STATUS_TOOL => {
                let v = self.audio.view();
                let mut s = format!("Bluetooth audio: {}.", v.line());
                if let Some(d) = &v.device {
                    s.push_str(&format!(" Packets received {}, lost {}.", d.packets, d.lost));
                }
                Ok(s)
            }
            "bluetooth_audio_connect" => {
                let d = self.source(args)?;
                self.audio.connect(&d.address)?;
                let until = Instant::now() + self.wait;
                loop {
                    let v = self.audio.view();
                    if let Some(x) = v.device.filter(|x| x.address.eq_ignore_ascii_case(&d.address)) {
                        return Ok(match x.state {
                            AudioState::Playing => format!("Connected: {} is playing through this computer.", d.title()),
                            AudioState::Connected => format!("Connected to {}. Start playing on it and the sound comes out here.", d.title()),
                        });
                    }
                    if !v.ready {
                        return Err(format!("{} did not connect: {}", d.title(), v.why_not));
                    }
                    if Instant::now() >= until {
                        return Ok(format!("Still connecting to {}; look again with bluetooth_audio_status in a few seconds. A phone usually needs its Bluetooth audio switched on and the music started.", d.title()));
                    }
                    std::thread::sleep(self.poll);
                }
            }
            SEND_STATUS_TOOL => Ok(format!("Sending sound to headphones: {}. Whole computer: {}.", self.send.view().line(), self.route.status())),
            "bluetooth_audio_send_start" => {
                let d = self.sink(args)?;
                self.send.start(&d.address, &d.title())?;
                if whole_computer(args) {
                    self.route.attach(self.send.clone(), mute_speakers(args));
                    Ok(format!("Sending this computer's sound to {} ({}){}. If the headphones stop taking sound the speakers play again by themselves.", d.title(), self.send.view().codec, if mute_speakers(args) { "; its own speakers are silent" } else { "" }))
                } else {
                    Ok(format!("Connected to {} ({}). Only a test tone goes to it; use bluetooth_audio_send_tone to hear it.", d.title(), self.send.view().codec))
                }
            }
            "bluetooth_audio_send_stop" => {
                self.route.detach(); // the speakers are back before the stream closes
                self.send.stop()?;
                Ok("Stopped sending sound; the headphones stay paired and this computer's speakers play again.".into())
            }
            "bluetooth_audio_send_tone" => {
                let pcm = test_tone();
                // while the whole computer is routed the tone takes the stream by itself for two seconds, then the route goes on
                let (routed, muted) = (self.route.listening(), self.route.muted());
                if routed {
                    self.route.detach();
                }
                let r = pcm.chunks(2 * 4800).try_for_each(|c| self.send.write(c, 48_000, 2));
                if routed {
                    self.route.attach(self.send.clone(), muted);
                }
                r?;
                Ok("Played a two second test tone through the headphones.".into())
            }
            "bluetooth_audio_disconnect" => {
                self.audio.disconnect()?;
                Ok("Disconnected the Bluetooth audio; the phone is still paired.".into())
            }
            "bluetooth_audio_pairing" => {
                let on = args.get("on").and_then(Value::as_bool).unwrap_or(true);
                self.audio.pairing_open(on)?;
                Ok(if on { "This computer is visible to phones for two minutes. On the phone, open Bluetooth settings and choose this computer; the number to compare appears on this screen and the person confirms it there.".into() } else { "Pairing for phones is closed.".into() })
            }
            _ => self.inner.call(name, args),
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

impl iyakki_devctl::a2dp::Output for SpeakerOutput {
    fn write(&mut self, pcm: &[i16]) -> Result<(), String> {
        self.0.write(pcm)
    }
}

impl iyakki_devctl::a2dp::OutputFactory for SpeakerFactory {
    fn open(&mut self, rate: u32, channels: usize, label: &str) -> Result<Box<dyn iyakki_devctl::a2dp::Output>, String> {
        let out = crate::audiosink::open_output(crate::mixer::SourceKind::Media, rate, channels as u16, label, &self.play_cmd)?;
        Ok(Box::new(SpeakerOutput(out)))
    }
}

/// Registers the speaker for Bluetooth audio (the program calls this once, with the play command of Settings -> Voice).
pub fn install(play_cmd: impl Fn() -> String + Send + Sync + 'static) {
    iyakki_devctl::a2dp_sys::set_output_factory(move || Box::new(SpeakerFactory { play_cmd: play_cmd() }));
}

// ------------------------------------------------------------------------------------------------ the phone's remote control

/// What a volume or play key of a phone or headset does here: the master volume, or IYAKKI's own player (the radio and the music it plays).
/// Returns a short note for the audit line, empty when nothing was taken.
pub fn do_remote(svc: &crate::services::Services, act: iyakki_devctl::avrcp::Act) -> String {
    do_remote_with(svc, &crate::media::shared(), act)
}

/// [`do_remote`] with the music player named (the shell's shared one, or a fake in a test).
pub fn do_remote_with(svc: &crate::services::Services, player: &crate::media::MediaPlayer, act: iyakki_devctl::avrcp::Act) -> String {
    use iyakki_devctl::avrcp::{Act, MediaKey};
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
    let (weak, user) = (std::sync::Arc::downgrade(svc), user.to_string());
    iyakki_devctl::a2dp_sys::set_remote_handler(move |act| {
        if let Some(s) = weak.upgrade() {
            let did = do_remote(&s, act);
            if !did.is_empty() {
                audit(&user, &format!("bluetooth.remote {did}"));
            }
        }
    });
}

/// Sign-out: the keys change nothing until somebody signs in again.
pub fn clear_remote() {
    iyakki_devctl::a2dp_sys::clear_remote_handler();
}

#[cfg(test)]
#[path = "btaudiotools_tests.rs"]
mod tests;
