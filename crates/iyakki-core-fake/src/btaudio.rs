//! Bluetooth audio on the fake machine: paired phones and headphones in memory, a receiving connection and a sending stream that change when asked,
//! the same refusals as the real handle (no such device, nothing connected), and every change recorded. No sound, no radio, no clock.

use std::sync::Mutex;

use iyakki_core_api::btaudio::{AudioDeviceView, AudioState, AudioView, BtAudio, BtAudioAction, SendView};

/// A paired device: its address, name and kind (`phone`, `computer`, `headphones`, `speaker`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paired {
    pub address: String,
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Debug, Default)]
pub struct State {
    pub paired: Vec<Paired>,
    /// The phone that is connected for audio (address).
    pub connected: Option<String>,
    pub playing: bool,
    pub pairing_open: bool,
    /// The headphones the sound goes to (address).
    pub sending_to: Option<String>,
    pub whole_computer: bool,
    pub mute_speakers: bool,
    pub remembered: Option<(String, bool)>,
    pub remote_cleared: u32,
    pub calls: Vec<String>,
}

#[derive(Default)]
pub struct FakeBtAudio {
    state: Mutex<State>,
}

impl FakeBtAudio {
    pub fn set(&self, f: impl FnOnce(&mut State)) {
        f(&mut self.state.lock().unwrap());
    }
    pub fn state(&self) -> State {
        self.state.lock().unwrap().clone()
    }
    /// Every change so far, as `apply(tool)`.
    pub fn calls(&self) -> Vec<String> {
        self.state.lock().unwrap().calls.clone()
    }
}

fn pick(s: &State, kinds: &[&str], who: &str, what: &str, none: &str, many: &str) -> Result<Paired, String> {
    let list: Vec<&Paired> = s.paired.iter().filter(|p| kinds.contains(&p.kind.as_str())).collect();
    let who = who.trim();
    if !who.is_empty() {
        return list.iter().find(|p| p.address.eq_ignore_ascii_case(who) || p.name.eq_ignore_ascii_case(who)).map(|p| (*p).clone()).ok_or_else(|| format!("no {what} named \"{who}\" is paired"));
    }
    match list.len() {
        0 => Err(none.into()),
        1 => Ok(list[0].clone()),
        _ => s.remembered.as_ref().and_then(|(a, _)| list.iter().find(|p| p.address.eq_ignore_ascii_case(a))).map(|p| (*p).clone()).ok_or_else(|| many.into()),
    }
}

fn title(p: &Paired) -> String {
    if p.name.is_empty() { p.address.clone() } else { p.name.clone() }
}

fn source(s: &State, who: &str) -> Result<Paired, String> {
    pick(s, &["phone", "computer"], who, "phone or laptop", "no phone or laptop is paired: let a phone pair (bluetooth_audio_pairing) or pair one in Settings -> Bluetooth first", "more than one phone or laptop is paired: name the one to use (bluetooth_list shows them)")
}

fn sink(s: &State, who: &str) -> Result<Paired, String> {
    pick(s, &["headphones", "speaker"], who, "pair of headphones or speaker", "no headphones or speaker are paired: put them in pairing mode and pair them in Settings -> Bluetooth first", "more than one pair of headphones or speaker is paired: name the one to use (bluetooth_list shows them)")
}

fn sending_name(s: &State) -> Option<String> {
    let a = s.sending_to.as_ref()?;
    s.paired.iter().find(|p| &p.address == a).map(title)
}

impl BtAudio for FakeBtAudio {
    fn receive_view(&self) -> AudioView {
        let s = self.state.lock().unwrap();
        let device = s.connected.as_ref().and_then(|a| s.paired.iter().find(|p| &p.address == a)).map(|p| AudioDeviceView { address: p.address.clone(), name: p.name.clone(), state: if s.playing { AudioState::Playing } else { AudioState::Connected }, codec: "SBC 44.1 kHz joint stereo".into(), error: None, packets: 0, lost: 0 });
        AudioView { ready: true, why_not: String::new(), pairing_open: s.pairing_open, device, aac_note: String::new() }
    }
    fn send_view(&self) -> SendView {
        let s = self.state.lock().unwrap();
        match sending_name(&s) {
            Some(d) => SendView { active: true, device: d, codec: "SBC".into(), ..Default::default() },
            None => SendView::default(),
        }
    }
    fn sinks(&self) -> Vec<(String, String)> {
        self.state.lock().unwrap().paired.iter().filter(|p| p.kind == "headphones" || p.kind == "speaker").map(|p| (p.address.clone(), title(p))).collect()
    }
    fn remembered(&self) -> Option<(String, bool)> {
        self.state.lock().unwrap().remembered.clone()
    }
    fn plan(&self, action: &BtAudioAction) -> Result<String, String> {
        let s = self.state.lock().unwrap();
        plan(&s, action)
    }
    fn apply(&self, action: &BtAudioAction) -> Result<String, String> {
        let mut s = self.state.lock().unwrap();
        plan(&s, action)?;
        s.calls.push(format!("apply({})", action.tool()));
        Ok(match action {
            BtAudioAction::Connect { device } => {
                let d = source(&s, device)?;
                s.connected = Some(d.address.clone());
                format!("Connected to {}. Start playing on it and the sound comes out here.", title(&d))
            }
            BtAudioAction::ConnectAt { address } => {
                let d = source(&s, address)?;
                s.connected = Some(d.address.clone());
                format!("Connecting to {} for audio ... start playing on it.", title(&d))
            }
            BtAudioAction::Disconnect => {
                s.connected = None;
                s.playing = false;
                "Disconnected the Bluetooth audio; the phone is still paired.".into()
            }
            BtAudioAction::Pairing { on } => {
                s.pairing_open = *on;
                if *on { "This computer is visible to phones for two minutes.".into() } else { "Pairing for phones is closed.".into() }
            }
            BtAudioAction::SendStart { device, whole_computer, mute_speakers } => {
                let d = sink(&s, device)?;
                s.sending_to = Some(d.address.clone());
                s.whole_computer = *whole_computer;
                s.mute_speakers = *mute_speakers;
                if *whole_computer {
                    s.remembered = Some((d.address.clone(), *mute_speakers));
                }
                format!("Sending this computer's sound to {} (SBC).", title(&d))
            }
            BtAudioAction::SendStop => {
                s.sending_to = None;
                s.remembered = None;
                "Stopped sending sound; the headphones stay paired and this computer's speakers play again.".into()
            }
            BtAudioAction::SendTone => "Played a two second test tone through the headphones.".into(),
        })
    }
    fn clear_remote(&self) {
        self.state.lock().unwrap().remote_cleared += 1;
    }
}

fn plan(s: &State, action: &BtAudioAction) -> Result<String, String> {
    match action {
        BtAudioAction::Connect { device } | BtAudioAction::ConnectAt { address: device } => source(s, device).map(|d| format!("Connect “{}” ({}) for Bluetooth audio.", title(&d), d.address)),
        BtAudioAction::Disconnect => match s.connected.as_ref().and_then(|a| s.paired.iter().find(|p| &p.address == a)) {
            Some(d) => Ok(format!("Disconnect “{}” from Bluetooth audio: its sound stops. It stays paired.", title(d))),
            None => Err("no phone or laptop is connected for audio".into()),
        },
        BtAudioAction::Pairing { on } => Ok(if *on { "Make this computer visible to phones for two minutes.".into() } else { "Close pairing for phones now.".into() }),
        BtAudioAction::SendStart { device, .. } => sink(s, device).map(|d| format!("Send sound to “{}” ({}).", title(&d), d.address)),
        BtAudioAction::SendStop => sending_name(s).map(|d| format!("Stop sending sound to “{d}”. It stays paired.")).ok_or_else(|| "no sound is being sent to a device".to_string()),
        BtAudioAction::SendTone => sending_name(s).map(|d| format!("Play a two second test tone through “{d}” at a quiet level.")).ok_or_else(|| "start sending to the headphones first (bluetooth_audio_send_start)".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &str, n: &str, k: &str) -> Paired {
        Paired { address: a.into(), name: n.into(), kind: k.into() }
    }

    #[test]
    fn a_phone_connects_pairing_opens_and_every_change_is_recorded_after_the_same_checks_as_the_card() {
        let f = FakeBtAudio::default();
        assert!(f.apply(&BtAudioAction::Connect { device: String::new() }).unwrap_err().contains("no phone or laptop is paired"));
        f.set(|s| s.paired = vec![p("AA:01", "Pixel 8", "phone"), p("AA:02", "Buds", "headphones")]);
        assert!(f.plan(&BtAudioAction::Connect { device: String::new() }).unwrap().contains("Pixel 8"));
        assert!(f.calls().is_empty(), "a plan changes nothing");
        assert!(f.apply(&BtAudioAction::Connect { device: "pixel 8".into() }).unwrap().contains("Connected to Pixel 8"));
        assert_eq!(f.receive_view().device.unwrap().state, AudioState::Connected);
        f.set(|s| s.playing = true);
        assert_eq!(f.receive_view().device.unwrap().state, AudioState::Playing);
        assert!(f.apply(&BtAudioAction::Pairing { on: true }).is_ok() && f.receive_view().pairing_open);
        assert!(f.apply(&BtAudioAction::Disconnect).unwrap().contains("Disconnected"));
        assert!(f.plan(&BtAudioAction::Disconnect).unwrap_err().contains("no phone or laptop is connected"));
        assert_eq!(f.calls(), vec!["apply(bluetooth_audio_connect)", "apply(bluetooth_audio_pairing)", "apply(bluetooth_audio_disconnect)"]);
    }

    #[test]
    fn sending_needs_a_paired_sink_the_tone_needs_a_stream_and_stopping_forgets_the_remembered_pair() {
        let f = FakeBtAudio::default();
        assert!(f.plan(&BtAudioAction::SendTone).unwrap_err().contains("start sending"));
        assert!(f.plan(&BtAudioAction::SendStop).is_err());
        f.set(|s| s.paired = vec![p("B1", "Buds", "headphones"), p("B2", "Bar", "speaker")]);
        let start = BtAudioAction::SendStart { device: String::new(), whole_computer: true, mute_speakers: true };
        assert!(f.plan(&start).unwrap_err().contains("more than one"));
        f.set(|s| s.remembered = Some(("B2".into(), true)));
        assert!(f.apply(&start).unwrap().contains("Bar"));
        assert!(f.send_view().active && f.send_view().device == "Bar");
        assert!(f.apply(&BtAudioAction::SendTone).is_ok());
        assert_eq!(f.sinks().len(), 2);
        assert!(f.apply(&BtAudioAction::SendStop).is_ok());
        assert!(!f.send_view().active && f.remembered().is_none());
        f.clear_remote();
        assert_eq!(f.state().remote_cleared, 1);
    }
}
