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
use iyakki_devctl::bthid::{find_device, DeviceView, Devices};

pub const PROMPT_RULE: &str = "\nBluetooth audio from a phone or laptop to this computer's speakers: bluetooth_audio_status says who is playing and how. bluetooth_audio_connect connects a paired phone for audio, bluetooth_audio_disconnect ends it, bluetooth_audio_pairing lets a new phone pair for two minutes (each asks the person); the person compares the number on the phone and on Settings -> Bluetooth themselves. The volume of the phone's sound is the sound source named after the phone in the speaker settings. This computer cannot send sound to headphones yet: say so plainly.";

pub const STATUS_TOOL: &str = "bluetooth_audio_status";
pub const WRITE_TOOLS: [&str; 3] = ["bluetooth_audio_connect", "bluetooth_audio_disconnect", "bluetooth_audio_pairing"];

pub fn is_audio_tool(n: &str) -> bool {
    n == STATUS_TOOL || WRITE_TOOLS.contains(&n)
}

pub struct BtAudioTools {
    inner: Arc<dyn Tools>,
    devs: Arc<dyn Devices>,
    audio: Arc<dyn AudioControl>,
    wait: Duration,
    poll: Duration,
}

impl BtAudioTools {
    pub fn new(inner: Arc<dyn Tools>, devs: Arc<dyn Devices>, audio: Arc<dyn AudioControl>) -> BtAudioTools {
        BtAudioTools { inner, devs, audio, wait: Duration::from_secs(20), poll: Duration::from_millis(500) }
    }

    /// The tools over this computer's real Bluetooth.
    pub fn real(inner: Arc<dyn Tools>) -> BtAudioTools {
        BtAudioTools::new(inner, Arc::new(iyakki_devctl::bthid_sys::RealDevices), Arc::new(iyakki_devctl::a2dp_sys::RealAudio))
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
            _ => Err("not a Bluetooth audio tool".into()),
        }
    }
}

impl Tools for BtAudioTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_audio_tool(&d.spec.name));
        let dev = json!({"type": "object", "properties": {"device": {"type": "string", "description": "The paired phone or laptop's name or address; leave out when only one is paired."}}});
        let ask = |name: &str, desc: &str, schema: Value| ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema }, risk: Risk::Ask, untrusted: false };
        v.push(ToolDef { spec: ToolSpec { name: STATUS_TOOL.into(), description: "Say whether a phone or laptop is playing through this computer over Bluetooth (A2DP), the codec, lost packets, and if nothing can play, why.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ask("bluetooth_audio_connect", "Connect a paired phone or laptop for Bluetooth audio so its sound plays on this computer's speakers. Waits up to 20 seconds. The person approves a card.", dev.clone()));
        v.push(ask("bluetooth_audio_disconnect", "End the Bluetooth audio connection; the phone stays paired. The person approves a card.", json!({"type": "object", "properties": {}})));
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
        if WRITE_TOOLS.contains(&name) {
            return match self.card(name, args) {
                Ok(c) => c.split(':').next().unwrap_or("").to_string(),
                Err(e) => format!("{name} (invalid: {e})"),
            };
        }
        if name == STATUS_TOOL {
            return "Show the Bluetooth audio state".into();
        }
        self.inner.describe(name, args)
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) {
            return self.card(name, args).ok();
        }
        if name == STATUS_TOOL { None } else { self.inner.preview(name, args) }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) {
            return self.card(name, args).err();
        }
        if name == STATUS_TOOL { None } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_audio_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !WRITE_TOOLS.contains(&name) && self.inner.approved_always(name)
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

#[cfg(test)]
#[path = "btaudiotools_tests.rs"]
mod tests;
