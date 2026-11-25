//! Bluetooth audio on this machine (moved from `iyakki-agent::btaudiotools`, F-ARCH-06): [`LocalBtAudio`] is the core's
//! [`iyakki_core_api::btaudio::BtAudio`] over the in-house Bluetooth audio code (`iyakki_devctl::a2dp` and `a2dp_send`). It finds the paired phone or
//! headphones, words the card text, connects and waits for the result, starts and stops the stream, and keeps the last headphones in a small file.
//! Where the whole computer's sound goes while it is sent is the seam [`SendRoute`] (the agent's mixer tap implements it).

use std::sync::Arc;
use std::time::{Duration, Instant};

use iyakki_core_api::btaudio::{AudioView, BtAudio, BtAudioAction, SendView};
use iyakki_devctl::a2dp::{AudioControl, AudioState};
use iyakki_devctl::a2dp_send::SendControl;
use iyakki_devctl::bthid::{find_device, DeviceView, Devices};

/// Where the whole computer's sound goes while it is sent to headphones.
pub trait SendRoute: Send + Sync {
    /// Route the sound into `sender`; `mute_local` silences the computer's own speakers.
    fn attach(&self, sender: Arc<dyn SendControl>, mute_local: bool);
    /// The speakers are back.
    fn detach(&self);
    fn listening(&self) -> bool;
    fn muted(&self) -> bool;
    /// One sentence about the route.
    fn status(&self) -> String;
}

static GLOBAL_ROUTE: std::sync::OnceLock<Arc<dyn SendRoute>> = std::sync::OnceLock::new();

/// The route of this computer's own sound (the program installs it once at start; a second call is ignored). [`LocalBtAudio::real`] uses it.
pub fn set_global_route(route: Arc<dyn SendRoute>) {
    let _ = GLOBAL_ROUTE.set(route);
}

/// A route that does nothing (a machine whose sound is not routed, tests of the card text).
pub struct NoRoute;

impl SendRoute for NoRoute {
    fn attach(&self, _: Arc<dyn SendControl>, _: bool) {}
    fn detach(&self) {}
    fn listening(&self) -> bool {
        false
    }
    fn muted(&self) -> bool {
        false
    }
    fn status(&self) -> String {
        "the whole computer's sound is not routed".into()
    }
}

/// A sending side that is not there (systems without Bluetooth).
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
    (0..96_000)
        .flat_map(|i| {
            let t = i as f64 / 48_000.0;
            let fade = (t.min(2.0 - t) * 20.0).clamp(0.0, 1.0);
            let v = ((t * 440.0 * std::f64::consts::TAU).sin() * 6000.0 * fade) as i16;
            [v, v]
        })
        .collect()
}

pub struct LocalBtAudio {
    devs: Arc<dyn Devices>,
    audio: Arc<dyn AudioControl>,
    send: Arc<dyn SendControl>,
    route: Arc<dyn SendRoute>,
    /// Where the last headphones are remembered (their address and the speakers' mute), so that after a restart the same ones are meant when
    /// several are paired. Nothing connects by itself: every start still asks.
    memory: Option<std::path::PathBuf>,
    wait: Duration,
    poll: Duration,
}

impl LocalBtAudio {
    pub fn new(devs: Arc<dyn Devices>, audio: Arc<dyn AudioControl>) -> LocalBtAudio {
        LocalBtAudio { devs, audio, send: Arc::new(NoSend), route: Arc::new(NoRoute), memory: None, wait: Duration::from_secs(20), poll: Duration::from_millis(500) }
    }

    /// This computer's real Bluetooth (the route is the caller's to give: [`with_route`](Self::with_route)).
    pub fn real() -> LocalBtAudio {
        let b = LocalBtAudio::new(Arc::new(iyakki_devctl::bthid_sys::RealDevices), Arc::new(iyakki_devctl::a2dp_sys::RealAudio)).with_send(iyakki_devctl::a2dp_send_sys::real_sender());
        match GLOBAL_ROUTE.get() {
            Some(r) => b.with_route(r.clone()),
            None => b,
        }
    }

    pub fn with_send(mut self, send: Arc<dyn SendControl>) -> LocalBtAudio {
        self.send = send;
        self
    }

    pub fn with_route(mut self, route: Arc<dyn SendRoute>) -> LocalBtAudio {
        self.route = route;
        self
    }

    /// Remember the last headphones in this file.
    pub fn with_memory(mut self, file: std::path::PathBuf) -> LocalBtAudio {
        self.memory = Some(file);
        self
    }

    pub fn with_wait(mut self, wait: Duration, poll: Duration) -> LocalBtAudio {
        self.wait = wait;
        self.poll = poll;
        self
    }

    /// The sending side this handle holds (the agent's mixer tap writes into it).
    pub fn sender(&self) -> Arc<dyn SendControl> {
        self.send.clone()
    }

    /// Is the whole computer's sound being sent to a pair right now?
    pub fn sending(&self) -> bool {
        self.send.view().active
    }

    /// One sentence for the sending status tool: the sender and the route.
    pub fn send_status(&self) -> String {
        format!("Sending sound to headphones: {}. Whole computer: {}.", self.send.view().line(), self.route.status())
    }

    /// The sentence of the receiving status tool.
    pub fn receive_status(&self) -> String {
        let v = self.audio.view();
        let mut s = format!("Bluetooth audio: {}.", v.line());
        if let Some(d) = &v.device {
            s.push_str(&format!(" Packets received {}, lost {}.", d.packets, d.lost));
        }
        s
    }

    fn remember(&self, address: &str, mute: bool) {
        if let Some(f) = &self.memory {
            let _ = std::fs::write(f, serde_json::json!({"address": address, "mute_speakers": mute}).to_string());
        }
    }

    fn forget(&self) {
        if let Some(f) = &self.memory {
            let _ = std::fs::remove_file(f);
        }
    }

    /// The paired headphones or speaker a sending tool means: the one named, else the only one paired.
    fn sink(&self, who: &str) -> Result<DeviceView, String> {
        let list: Vec<DeviceView> = self.devs.list().into_iter().filter(|d| d.kind == "headphones" || d.kind == "speaker").collect();
        let who = who.trim();
        if !who.is_empty() {
            return find_device(&list, who).map_err(|e| e.replace("device", "pair of headphones or speaker"));
        }
        match list.len() {
            0 => Err("no headphones or speaker are paired: put them in pairing mode and pair them in Settings -> Bluetooth first".into()),
            1 => Ok(list[0].clone()),
            _ => match self.remembered().and_then(|(a, _)| list.iter().find(|d| d.address.eq_ignore_ascii_case(&a)).cloned()) {
                Some(d) => Ok(d),
                None => Err("more than one pair of headphones or speaker is paired: name the one to use (bluetooth_list shows them)".into()),
            },
        }
    }

    /// The paired device an audio tool means: the one named, else the only paired phone or laptop.
    fn source(&self, who: &str) -> Result<DeviceView, String> {
        let list: Vec<DeviceView> = self.devs.list().into_iter().filter(|d| d.kind == "phone" || d.kind == "computer").collect();
        let who = who.trim();
        if !who.is_empty() {
            return find_device(&list, who).map_err(|e| e.replace("device", "phone or laptop"));
        }
        match list.len() {
            0 => Err("no phone or laptop is paired: let a phone pair (bluetooth_audio_pairing) or pair one in Settings -> Bluetooth first".into()),
            1 => Ok(list[0].clone()),
            _ => Err("more than one phone or laptop is paired: name the one to use (bluetooth_list shows them)".into()),
        }
    }
}

impl BtAudio for LocalBtAudio {
    fn receive_view(&self) -> AudioView {
        self.audio.view()
    }
    fn send_view(&self) -> SendView {
        self.send.view()
    }
    fn sinks(&self) -> Vec<(String, String)> {
        self.devs.list().into_iter().filter(|d| d.kind == "headphones" || d.kind == "speaker").map(|d| (d.address.clone(), d.title())).collect()
    }
    fn sound_line(&self) -> String {
        let v = self.send.view();
        if v.active {
            format!("{} {}", v.line(), self.route.status())
        } else {
            v.line()
        }
    }
    fn remembered(&self) -> Option<(String, bool)> {
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(self.memory.as_ref()?).ok()?).ok()?;
        Some((v.get("address")?.as_str()?.to_string(), v.get("mute_speakers").and_then(serde_json::Value::as_bool).unwrap_or(true)))
    }

    fn plan(&self, action: &BtAudioAction) -> Result<String, String> {
        match action {
            BtAudioAction::Connect { device } => {
                let d = self.source(device)?;
                Ok(format!("Connect “{}” ({}) for Bluetooth audio: when it plays music or video, the sound comes out of this computer's speakers until it is disconnected.", d.title(), d.address))
            }
            BtAudioAction::ConnectAt { address } => {
                let d = self.source(address)?;
                Ok(format!("Connect “{}” ({}) for Bluetooth audio: when it plays music or video, the sound comes out of this computer's speakers until it is disconnected.", d.title(), d.address))
            }
            BtAudioAction::Disconnect => match self.audio.view().device {
                Some(d) => Ok(format!("Disconnect “{}” from Bluetooth audio: its sound stops. It stays paired.", d.title())),
                None => Err("no phone or laptop is connected for audio".into()),
            },
            BtAudioAction::Pairing { on } => {
                if *on {
                    Ok("Make this computer visible to phones for two minutes so a phone can pair with it for audio. The person must be at the phone and answer the number comparison on this screen. Nobody else can pair afterwards.".into())
                } else {
                    Ok("Close pairing for phones now.".into())
                }
            }
            BtAudioAction::SendStart { device, whole_computer, mute_speakers } => {
                let d = self.sink(device)?;
                let tail = if *whole_computer {
                    if *mute_speakers { "All of this computer's sound goes to them and its own speakers are silent until you stop it." } else { "All of this computer's sound goes to them and its own speakers play too." }
                } else {
                    "Only the test tone goes to them; the sound of this computer stays on its speakers."
                };
                Ok(format!("Send sound to “{}” ({}): this computer connects to it and plays through it until you stop it. Only SBC is sent. {tail}", d.title(), d.address))
            }
            BtAudioAction::SendStop => {
                let v = self.send.view();
                if v.active { Ok(format!("Stop sending sound to “{}”. It stays paired.", v.device)) } else { Err("no sound is being sent to a device".into()) }
            }
            BtAudioAction::SendTone => {
                let v = self.send.view();
                if v.active { Ok(format!("Play a two second test tone through “{}” at a quiet level.", v.device)) } else { Err("start sending to the headphones first (bluetooth_audio_send_start)".into()) }
            }
        }
    }

    fn apply(&self, action: &BtAudioAction) -> Result<String, String> {
        match action {
            BtAudioAction::Connect { device } => {
                let d = self.source(device)?;
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
            BtAudioAction::ConnectAt { address } => {
                let d = self.source(address)?;
                self.audio.connect(&d.address)?;
                Ok(format!("Connecting to {} for audio ... start playing on it.", d.title()))
            }
            BtAudioAction::Disconnect => {
                self.audio.disconnect()?;
                Ok("Disconnected the Bluetooth audio; the phone is still paired.".into())
            }
            BtAudioAction::Pairing { on } => {
                self.audio.pairing_open(*on)?;
                Ok(if *on { "This computer is visible to phones for two minutes. On the phone, open Bluetooth settings and choose this computer; the number to compare appears on this screen and the person confirms it there.".into() } else { "Pairing for phones is closed.".into() })
            }
            BtAudioAction::SendStart { device, whole_computer, mute_speakers } => {
                let d = self.sink(device)?;
                self.send.start(&d.address, &d.title())?;
                if *whole_computer {
                    self.route.attach(self.send.clone(), *mute_speakers);
                    self.remember(&d.address, *mute_speakers);
                    Ok(format!("Sending this computer's sound to {} ({}){}. If the headphones leave or stop taking sound, playback pauses (it does not move to the speakers); press play to carry on.", d.title(), self.send.view().codec, if *mute_speakers { "; its own speakers are silent" } else { "" }))
                } else {
                    Ok(format!("Connected to {} ({}). Only a test tone goes to it; use bluetooth_audio_send_tone to hear it.", d.title(), self.send.view().codec))
                }
            }
            BtAudioAction::SendStop => {
                self.route.detach(); // the speakers are back before the stream closes
                self.send.stop()?;
                self.forget();
                Ok("Stopped sending sound; the headphones stay paired and this computer's speakers play again.".into())
            }
            BtAudioAction::SendTone => {
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
        }
    }

    fn clear_remote(&self) {
        iyakki_devctl::a2dp_sys::clear_remote_handler();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_devctl::a2dp::AudioDeviceView;
    use iyakki_devctl::bthid::LinkState;
    use std::sync::Mutex;

    struct Devs(Vec<DeviceView>);
    impl Devices for Devs {
        fn list(&self) -> Vec<DeviceView> {
            self.0.clone()
        }
        fn connect(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn disconnect(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn forget(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
    }

    struct Audio {
        view: Mutex<AudioView>,
        after: Option<AudioDeviceView>,
        calls: Mutex<Vec<String>>,
    }
    impl AudioControl for Audio {
        fn view(&self) -> AudioView {
            self.view.lock().unwrap().clone()
        }
        fn connect(&self, a: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("connect {a}"));
            if let Some(d) = self.after.clone() {
                self.view.lock().unwrap().device = Some(d);
            }
            Ok(())
        }
        fn disconnect(&self) -> Result<(), String> {
            self.calls.lock().unwrap().push("disconnect".into());
            self.view.lock().unwrap().device = None;
            Ok(())
        }
        fn pairing_open(&self, on: bool) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("pairing {on}"));
            self.view.lock().unwrap().pairing_open = on;
            Ok(())
        }
    }

    #[derive(Default)]
    struct Send {
        live: Mutex<Option<String>>,
        writes: Mutex<usize>,
    }
    impl SendControl for Send {
        fn view(&self) -> SendView {
            match self.live.lock().unwrap().clone() {
                Some(d) => SendView { active: true, device: d, codec: "SBC".into(), ..Default::default() },
                None => SendView::default(),
            }
        }
        fn start(&self, _: &str, name: &str) -> Result<(), String> {
            *self.live.lock().unwrap() = Some(name.into());
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            self.live.lock().unwrap().take().map(|_| ()).ok_or_else(|| "no sound is being sent to a device".to_string())
        }
        fn write(&self, pcm: &[i16], rate: u32, ch: usize) -> Result<(), String> {
            assert_eq!((rate, ch), (48_000, 2));
            *self.writes.lock().unwrap() += pcm.len();
            Ok(())
        }
    }

    #[derive(Default)]
    struct Route {
        log: Mutex<Vec<String>>,
        on: Mutex<bool>,
    }
    impl SendRoute for Route {
        fn attach(&self, _: Arc<dyn SendControl>, mute: bool) {
            self.log.lock().unwrap().push(format!("attach mute={mute}"));
            *self.on.lock().unwrap() = true;
        }
        fn detach(&self) {
            self.log.lock().unwrap().push("detach".into());
            *self.on.lock().unwrap() = false;
        }
        fn listening(&self) -> bool {
            *self.on.lock().unwrap()
        }
        fn muted(&self) -> bool {
            true
        }
        fn status(&self) -> String {
            "routed".into()
        }
    }

    fn dev(a: &str, n: &str, kind: &str) -> DeviceView {
        DeviceView { address: a.into(), addr_type: 0, name: n.into(), kind: kind.into(), state: LinkState::Disconnected }
    }

    fn rig(devs: Vec<DeviceView>, after: Option<AudioDeviceView>, ready: bool) -> (LocalBtAudio, Arc<Audio>) {
        let a = Arc::new(Audio { view: Mutex::new(AudioView { ready, why_not: if ready { String::new() } else { "the channels could not be opened".into() }, ..Default::default() }), after, calls: Mutex::new(vec![]) });
        (LocalBtAudio::new(Arc::new(Devs(devs)), a.clone()).with_wait(Duration::ZERO, Duration::ZERO), a)
    }

    #[test]
    fn the_phone_is_the_named_one_or_the_only_one_and_the_card_says_so_before_anything_is_connected() {
        let (b, a) = rig(vec![dev("AA:01", "Pixel 8", "phone"), dev("BB:01", "Buds", "headphones")], None, true);
        assert!(b.plan(&BtAudioAction::Connect { device: String::new() }).unwrap().contains("“Pixel 8” (AA:01)"));
        assert!(a.calls.lock().unwrap().is_empty(), "a plan connects nothing");
        let (b, _) = rig(vec![dev("AA:01", "Pixel", "phone"), dev("AA:02", "Laptop", "computer")], None, true);
        assert!(b.plan(&BtAudioAction::Connect { device: String::new() }).unwrap_err().contains("more than one phone or laptop"));
        assert!(b.plan(&BtAudioAction::Connect { device: "laptop".into() }).unwrap().contains("Laptop"));
        assert!(b.plan(&BtAudioAction::Connect { device: "buds".into() }).unwrap_err().contains("phone or laptop"));
        let (none, _) = rig(vec![], None, true);
        assert!(none.plan(&BtAudioAction::Connect { device: String::new() }).unwrap_err().contains("no phone or laptop is paired"));
    }

    #[test]
    fn a_connect_waits_for_the_phone_and_reports_playing_or_connected_or_the_reason_or_still_waiting() {
        let d = |state| AudioDeviceView { address: "AA:01".into(), name: "Pixel 8".into(), state, codec: "SBC".into(), error: None, packets: 1, lost: 0 };
        let (b, a) = rig(vec![dev("AA:01", "Pixel 8", "phone")], Some(d(AudioState::Playing)), true);
        assert!(b.apply(&BtAudioAction::Connect { device: String::new() }).unwrap().contains("is playing through this computer"));
        assert_eq!(*a.calls.lock().unwrap(), vec!["connect AA:01"]);
        let (b, _) = rig(vec![dev("AA:01", "Pixel 8", "phone")], Some(d(AudioState::Connected)), true);
        assert!(b.apply(&BtAudioAction::Connect { device: String::new() }).unwrap().contains("Start playing on it"));
        let (b, _) = rig(vec![dev("AA:01", "Pixel 8", "phone")], None, false);
        assert!(b.apply(&BtAudioAction::Connect { device: String::new() }).unwrap_err().contains("did not connect: the channels could not be opened"));
        let (b, _) = rig(vec![dev("AA:01", "Pixel 8", "phone")], None, true);
        assert!(b.apply(&BtAudioAction::Connect { device: String::new() }).unwrap().contains("Still connecting"));
        // a page that cannot wait asks and returns at once
        let (b, a) = rig(vec![dev("AA:01", "Pixel 8", "phone")], None, true);
        assert!(b.apply(&BtAudioAction::ConnectAt { address: "AA:01".into() }).unwrap().contains("Connecting to Pixel 8"));
        assert_eq!(*a.calls.lock().unwrap(), vec!["connect AA:01"]);
    }

    #[test]
    fn disconnect_and_pairing_act_and_the_status_names_the_device_the_packets_and_the_losses() {
        let d = AudioDeviceView { address: "AA:01".into(), name: "Pixel 8".into(), state: AudioState::Playing, codec: "SBC".into(), error: None, packets: 120, lost: 2 };
        let (b, a) = rig(vec![dev("AA:01", "Pixel 8", "phone")], Some(d), true);
        assert!(b.plan(&BtAudioAction::Disconnect).unwrap_err().contains("no phone or laptop is connected"));
        b.apply(&BtAudioAction::Connect { device: String::new() }).unwrap();
        let s = b.receive_status();
        assert!(s.contains("Pixel 8") && s.contains("Packets received 120, lost 2"), "{s}");
        assert!(b.plan(&BtAudioAction::Disconnect).unwrap().contains("Disconnect “Pixel 8”"));
        b.apply(&BtAudioAction::Disconnect).unwrap();
        assert!(b.apply(&BtAudioAction::Pairing { on: true }).unwrap().contains("visible to phones for two minutes"));
        assert!(b.receive_view().pairing_open);
        assert!(b.plan(&BtAudioAction::Pairing { on: true }).unwrap().contains("answer the number comparison on this screen"), "the person answers the number, never the assistant");
        assert!(b.apply(&BtAudioAction::Pairing { on: false }).unwrap().contains("closed"));
        assert_eq!(*a.calls.lock().unwrap(), vec!["connect AA:01", "disconnect", "pairing true", "pairing false"]);
    }

    #[test]
    fn sending_routes_the_whole_computer_remembers_the_pair_plays_a_tone_through_the_route_and_stopping_forgets() {
        let dir = std::env::temp_dir().join(format!("iyakki-btaudio-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mem = dir.join("sink.json");
        let (b, _) = rig(vec![dev("B1", "Buds", "headphones"), dev("B2", "Bar", "speaker")], None, true);
        let (send, route) = (Arc::new(Send::default()), Arc::new(Route::default()));
        let b = b.with_send(send.clone()).with_route(route.clone()).with_memory(mem.clone());
        let start = BtAudioAction::SendStart { device: String::new(), whole_computer: true, mute_speakers: true };
        assert!(b.plan(&start).unwrap_err().contains("more than one pair of headphones or speaker"));
        let start_bar = BtAudioAction::SendStart { device: "bar".into(), whole_computer: true, mute_speakers: true };
        assert!(b.plan(&start_bar).unwrap().contains("its own speakers are silent until you stop it"));
        assert!(b.apply(&start_bar).unwrap().contains("its own speakers are silent"));
        assert_eq!(*route.log.lock().unwrap(), vec!["attach mute=true"]);
        assert_eq!(b.remembered(), Some(("B2".into(), true)));
        assert!(b.plan(&start).unwrap().contains("“Bar”"), "the remembered pair breaks the tie");
        assert!(b.sound_line().contains("routed") && b.sending());
        assert!(b.send_status().contains("Whole computer: routed"));
        // the tone takes the stream by itself and the route goes on
        b.apply(&BtAudioAction::SendTone).unwrap();
        assert_eq!(*send.writes.lock().unwrap(), 192_000, "two seconds of stereo at 48 kHz");
        assert_eq!(*route.log.lock().unwrap(), vec!["attach mute=true", "detach", "attach mute=true"]);
        b.apply(&BtAudioAction::SendStop).unwrap();
        assert_eq!(route.log.lock().unwrap().last().map(String::as_str), Some("detach"));
        assert!(b.remembered().is_none() && !mem.exists() && !b.sending());
        assert!(b.plan(&BtAudioAction::SendStop).unwrap_err().contains("no sound is being sent"));
        assert!(b.plan(&BtAudioAction::SendTone).unwrap_err().contains("start sending to the headphones first"));
        // only a tone: nothing is routed or remembered
        let tone_only = BtAudioAction::SendStart { device: "buds".into(), whole_computer: false, mute_speakers: true };
        assert!(b.apply(&tone_only).unwrap().contains("Only a test tone goes to it"));
        assert_eq!(route.log.lock().unwrap().len(), 4);
        assert!(b.remembered().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_machine_without_a_sender_says_so_and_the_tone_is_two_seconds_of_stereo() {
        let (b, _) = rig(vec![dev("B1", "Buds", "headphones")], None, true);
        let start = BtAudioAction::SendStart { device: String::new(), whole_computer: true, mute_speakers: true };
        assert!(b.apply(&start).unwrap_err().contains("not available here"));
        assert!(b.send_view().why.contains("not available here"));
        assert_eq!(b.sinks(), vec![("B1".to_string(), "Buds".to_string())]);
        assert_eq!(test_tone().len(), 192_000);
        assert!(!NoRoute.listening() && !NoRoute.muted() && NoRoute.status().contains("not routed"));
        NoRoute.attach(Arc::new(Send::default()), true);
        NoRoute.detach();
    }
}
