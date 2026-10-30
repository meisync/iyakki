//! CHECK-UP: a guided hardware self-test, in plain words. Every step ends as Pass, Fail or Skipped, with advice and (where DOCTOR
//! or Settings has one) the same fix button as DOCTOR.
//!
//! This file holds everything the CHECK-UP tile decides and reads, so the tile (`apps/checkup*.rs` in the shell) only draws and keeps
//! state:
//!
//! * [`Inventory`] is what the machine lists (input devices, sound cards, microphones, network adapters, Bluetooth, screens,
//!   battery). [`Inventory::read`] reads `/proc` and `/sys`; tests give a made-up one, and the readers take a root folder so they
//!   are tested on files in a temporary folder.
//! * One small function per step turns the inventory plus what the person said or what a probe returned into a [`StepResult`].
//! * Nothing recorded is kept: the microphone step looks at the samples for loudness and drops them; the camera step is handled by
//!   the tile and only a verdict reaches here. The report ([`report_lines`]) holds verdicts and device names, never sound or pictures.
//!   The tile hands the report to Share diagnostics ([`set_last_report`], read by `diagshare` through the shell), which redacts it like
//!   every other section.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use crate::doctor::{Action, Fix};
use crate::voicedesk::{self, Capture, Mic, Recorders};

/// The steps, in the order the tile walks through them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StepId {
    Display,
    Touch,
    Keyboard,
    Pointer,
    Speaker,
    Microphone,
    Camera,
    Monitor,
    Network,
    Wifi,
    Bluetooth,
    Power,
}

impl StepId {
    pub const ALL: [StepId; 12] = [StepId::Display, StepId::Touch, StepId::Keyboard, StepId::Pointer, StepId::Speaker, StepId::Microphone, StepId::Camera, StepId::Monitor, StepId::Network, StepId::Wifi, StepId::Bluetooth, StepId::Power];

    pub fn id(self) -> &'static str {
        match self {
            StepId::Display => "display",
            StepId::Touch => "touch",
            StepId::Keyboard => "keyboard",
            StepId::Pointer => "pointer",
            StepId::Speaker => "speaker",
            StepId::Microphone => "microphone",
            StepId::Camera => "camera",
            StepId::Monitor => "monitor",
            StepId::Network => "network",
            StepId::Wifi => "wifi",
            StepId::Bluetooth => "bluetooth",
            StepId::Power => "power",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            StepId::Display => "Screen",
            StepId::Touch => "Touch screen",
            StepId::Keyboard => "Keyboard",
            StepId::Pointer => "Mouse or touch pad",
            StepId::Speaker => "Speaker",
            StepId::Microphone => "Microphone",
            StepId::Camera => "Camera",
            StepId::Monitor => "External monitor",
            StepId::Network => "Network",
            StepId::Wifi => "Wi-Fi",
            StepId::Bluetooth => "Bluetooth",
            StepId::Power => "Battery and power",
        }
    }

    /// The step needs the person: they look, press, click or listen. The others are answered by the machine alone.
    pub fn asks_the_person(self) -> bool {
        matches!(self, StepId::Display | StepId::Touch | StepId::Keyboard | StepId::Pointer | StepId::Speaker)
    }

    /// The step uses the camera or the microphone: it starts only on the person's click.
    pub fn is_capture(self) -> bool {
        matches!(self, StepId::Microphone | StepId::Camera)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Not done yet.
    Pending,
    Pass,
    Fail,
    /// Nothing to test on this computer, or the person chose to skip it.
    Skipped,
}

impl Outcome {
    pub fn word(self) -> &'static str {
        match self {
            Outcome::Pending => "Not done yet",
            Outcome::Pass => "Pass",
            Outcome::Fail => "Fail",
            Outcome::Skipped => "Skipped",
        }
    }
}

/// How one step ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepResult {
    pub id: StepId,
    pub outcome: Outcome,
    /// What was found, one plain sentence.
    pub note: String,
    /// What to do about it (empty on a pass).
    pub advice: String,
    /// The DOCTOR-style fix: opens the Settings page where the person does it.
    pub fix: Option<Fix>,
    /// The id of the DOCTOR rule this step corresponds to, if there is one (the report names it).
    pub doctor: Option<&'static str>,
}

impl StepResult {
    pub fn pending(id: StepId) -> StepResult {
        StepResult { id, outcome: Outcome::Pending, note: String::new(), advice: String::new(), fix: None, doctor: None }
    }
}

fn open(label: &'static str, item: &'static str) -> Option<Fix> {
    Some(Fix { label, action: Action::Settings(item) })
}

fn res(id: StepId, outcome: Outcome, note: impl Into<String>, advice: impl Into<String>, fix: Option<Fix>, doctor: Option<&'static str>) -> StepResult {
    StepResult { id, outcome, note: note.into(), advice: advice.into(), fix, doctor }
}

// ------------------------------------------------------------------------------------------------ the inventory

/// A network adapter the kernel lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    /// The link is up (cable plugged in, or joined to a network).
    pub up: bool,
}

/// One screen output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenOut {
    pub name: String,
    pub connected: bool,
    /// The size it shows now.
    pub current: Option<(u32, u32)>,
}

impl ScreenOut {
    /// The built-in panel of a laptop (`eDP-1`, `LVDS-1`, `DSI-1`).
    pub fn internal(&self) -> bool {
        let n = self.name.to_ascii_uppercase();
        n.starts_with("EDP") || n.starts_with("LVDS") || n.starts_with("DSI")
    }
}

/// What the computer power says.
#[derive(Clone, Debug, PartialEq)]
pub struct PowerFacts {
    /// A laptop battery was found and read.
    pub battery: bool,
    /// ACPI says a battery exists but the kernel has no driver for it.
    pub no_driver: bool,
    pub laptop: bool,
    pub percent: Option<u32>,
    pub status: String,
    pub ac_online: Option<bool>,
    pub health_pct: Option<u32>,
}

impl Default for PowerFacts {
    fn default() -> PowerFacts {
        PowerFacts { battery: false, no_driver: false, laptop: false, percent: None, status: String::new(), ac_online: None, health_pct: None }
    }
}

/// Everything the steps read about the machine.
#[derive(Clone, Debug, Default)]
pub struct Inventory {
    pub keyboards: usize,
    pub pointers: usize,
    pub touchscreens: usize,
    pub sound_cards: usize,
    /// The speaker the person chose in Settings (empty: automatic) and whether it is connected.
    pub sound_chosen: String,
    pub sound_chosen_present: bool,
    pub mics: Vec<Mic>,
    pub product: String,
    /// A T2 Mac's microphones are hidden because the capture switch is off.
    pub t2_mic_hidden: bool,
    pub screens: Vec<ScreenOut>,
    pub ethernet: Vec<Adapter>,
    pub wifi: Vec<Adapter>,
    pub route: bool,
    pub bluetooth: usize,
    /// Bluetooth or Wi-Fi switched off by a switch (rfkill).
    pub bluetooth_blocked: bool,
    pub wifi_blocked: bool,
    pub power: PowerFacts,
}

/// Count the keyboards, pointers and touch screens of `/proc/bus/input/devices`.
pub fn count_inputs(devices_text: &str) -> (usize, usize, usize) {
    use iyakki_hwinfo::InputKind;
    let devs = iyakki_hwinfo::parse_input_devices(devices_text);
    let n = |f: &dyn Fn(InputKind) -> bool| devs.iter().filter(|d| f(d.kind)).count();
    (n(&|k| k == InputKind::Keyboard), n(&|k| matches!(k, InputKind::Mouse | InputKind::Touchpad | InputKind::Tablet)), n(&|k| k == InputKind::Touchscreen))
}

/// Network adapters from `<sys>/class/net`: real ones only (they have a `device`), wired and wireless told apart. `(ethernet, wifi)`.
pub fn read_adapters(sys: &Path) -> (Vec<Adapter>, Vec<Adapter>) {
    let dir = sys.join("class/net");
    let mut names: Vec<String> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    let (mut eth, mut wifi) = (Vec::new(), Vec::new());
    for n in names {
        let d = dir.join(&n);
        if n == "lo" || !d.join("device").exists() {
            continue; // virtual: loopback, bridges, containers, VPNs
        }
        let up = std::fs::read_to_string(d.join("operstate")).map(|s| s.trim() == "up").unwrap_or(false);
        let a = Adapter { name: n, up };
        if d.join("wireless").exists() || d.join("phy80211").exists() {
            wifi.push(a);
        } else {
            eth.push(a);
        }
    }
    (eth, wifi)
}

/// Bluetooth adapters (`<sys>/class/bluetooth/hciN`), and whether Bluetooth / Wi-Fi are switched off (`<sys>/class/rfkill`).
/// `(adapters, bluetooth_blocked, wifi_blocked)`.
pub fn read_radios(sys: &Path) -> (usize, bool, bool) {
    let adapters = std::fs::read_dir(sys.join("class/bluetooth")).into_iter().flatten().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("hci") && !e.file_name().to_string_lossy().contains(':')).count();
    let (mut bt, mut wl) = (false, false);
    for e in std::fs::read_dir(sys.join("class/rfkill")).into_iter().flatten().flatten() {
        let d = e.path();
        let rd = |f: &str| std::fs::read_to_string(d.join(f)).unwrap_or_default().trim().to_string();
        let blocked = rd("soft") == "1" || rd("hard") == "1";
        match rd("type").as_str() {
            "bluetooth" => bt |= blocked,
            "wlan" => wl |= blocked,
            _ => {}
        }
    }
    (adapters, bt, wl)
}

impl Inventory {
    /// The real machine. `screens` come from the shell (it drives the displays); everything else is read here.
    pub fn read(screens: Vec<ScreenOut>) -> Inventory {
        let proc_root = Path::new("/proc");
        let sys = Path::new("/sys");
        let (keyboards, pointers, touchscreens) = count_inputs(&std::fs::read_to_string("/proc/bus/input/devices").unwrap_or_default());
        let sound_chosen = String::new();
        let (eth, wifi) = read_adapters(sys);
        let (bluetooth, bluetooth_blocked, wifi_blocked) = read_radios(sys);
        let env = crate::battery::BatteryEnv::real();
        let r = crate::battery::read(&env);
        let power = PowerFacts {
            battery: r.readable(),
            no_driver: r.presence == crate::battery::Presence::NoDriver,
            laptop: crate::battery::laptop_chassis(&env),
            percent: r.percent_whole(),
            status: r.status_words().to_string(),
            ac_online: r.ac_online,
            health_pct: r.health_pct.map(|h| h.round() as u32),
        };
        Inventory {
            keyboards,
            pointers,
            touchscreens,
            sound_cards: crate::doctor::sound_card_count(),
            sound_chosen,
            sound_chosen_present: true,
            mics: voicedesk::list_mics(proc_root),
            product: voicedesk::product_name(sys),
            t2_mic_hidden: voicedesk::t2_mic_hidden(proc_root),
            screens,
            ethernet: eth,
            wifi,
            route: crate::doctor::has_default_route(),
            bluetooth,
            bluetooth_blocked,
            wifi_blocked,
            power,
        }
    }
}

// ------------------------------------------------------------------------------------------------ the person's side

/// What the person did in a step that needs them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Said {
    /// "Yes, I saw / heard / it worked."
    Yes,
    /// "No."
    No,
    /// The person chose to skip.
    Skip,
}

/// The person chose to skip a step.
pub fn skipped(id: StepId) -> StepResult {
    res(id, Outcome::Skipped, "You skipped this step.", "", None, None)
}

/// The screen step: the person looks at the colour bars.
pub fn display(said: Said) -> StepResult {
    match said {
        Said::Yes => res(StepId::Display, Outcome::Pass, "You could see the test picture, all four colour bars and the frame.", "", None, None),
        Said::No => res(StepId::Display, Outcome::Fail, "The test picture was not drawn properly.", "Try another cable or port if you use an external screen, and check the brightness keys. Settings -> Display lists what the computer found and lets you choose another size.", open("Open Display", "tab:display"), None),
        Said::Skip => skipped(StepId::Display),
    }
}

/// The touch screen step. `listed`: how many touch screens the kernel lists.
pub fn touch(inv: &Inventory, said: Said) -> StepResult {
    if inv.touchscreens == 0 {
        return res(StepId::Touch, Outcome::Skipped, "No touch screen is listed on this computer.", "Nothing to do unless it should have one.", None, None);
    }
    match said {
        Said::Yes => res(StepId::Touch, Outcome::Pass, "The circle answered your touch.", "", None, None),
        Said::No => res(StepId::Touch, Outcome::Fail, "A touch screen is listed, but touching the circle did nothing.", "Clean the screen and try again. If it still fails, the touch driver may be missing: the Hardware page of Settings -> Overview lists the input devices.", open("Open Overview", "tab:overview"), None),
        Said::Skip => skipped(StepId::Touch),
    }
}

/// The keyboard step. `keys`: how many different keys the person pressed.
pub fn keyboard(inv: &Inventory, keys: usize, said: Said) -> StepResult {
    if said == Said::Skip {
        return skipped(StepId::Keyboard);
    }
    if keys >= 3 {
        let n = if inv.keyboards == 0 { String::new() } else { format!(" ({} listed)", inv.keyboards) };
        return res(StepId::Keyboard, Outcome::Pass, format!("{keys} different keys arrived{n}."), "", None, None);
    }
    let note = if keys == 0 { "No key arrived.".to_string() } else { format!("Only {keys} key{} arrived.", if keys == 1 { "" } else { "s" }) };
    let listed = if inv.keyboards == 0 { " No keyboard is listed by the computer." } else { "" };
    res(StepId::Keyboard, Outcome::Fail, format!("{note}{listed}"), "Press at least three different keys. For a USB keyboard try another port; for a wireless one check its battery. Settings -> Keyboard has the layout.", open("Open Keyboard", "tab:keyboard"), None)
}

/// The mouse or touch pad step.
pub fn pointer(inv: &Inventory, clicked: bool, said: Said) -> StepResult {
    if said == Said::Skip {
        return skipped(StepId::Pointer);
    }
    if clicked {
        return res(StepId::Pointer, Outcome::Pass, "The pointer moved and the click landed on the target.", "", None, None);
    }
    let listed = if inv.pointers == 0 { " No mouse or touch pad is listed by the computer." } else { "" };
    res(StepId::Pointer, Outcome::Fail, format!("The target was not clicked.{listed}"), "Move the pointer over the target and click. For a USB mouse try another port; for a wireless one check its battery.", open("Open Keyboard", "tab:keyboard"), None)
}

// ------------------------------------------------------------------------------------------------ the speaker

/// A short, soft test tone (a rising pair of notes) as a WAV: 16 kHz mono, about 0.7 s.
pub fn tone_wav() -> Vec<u8> {
    let mut pcm = crate::voice::tone(523.0, 300);
    pcm.extend(crate::voice::tone(784.0, 400));
    crate::voice::encode_wav(&pcm, crate::voice::SAMPLE_RATE)
}

/// The speaker step. `played`: the tone was handed to the mixer (an error says why not). `heard`: the person's answer.
pub fn speaker(inv: &Inventory, played: &Result<(), String>, heard: Said) -> StepResult {
    let id = StepId::Speaker;
    if heard == Said::Skip {
        return skipped(id);
    }
    if inv.sound_cards == 0 {
        return res(id, Outcome::Fail, "No sound card was found, so there is nothing to play on.", "Nothing is listed under Settings -> Sound. If a sound chip should be there, its driver may be missing.", open("Open Sound", "tab:sound"), Some("sound-none"));
    }
    if !inv.sound_chosen_present {
        return res(id, Outcome::Fail, "The sound output you chose is not connected.", "Choose another output, or plug the speaker or headphones back in.", open("Choose a sound output", "tab:sound"), Some("sound-missing"));
    }
    if let Err(e) = played {
        return res(id, Outcome::Fail, format!("The tone could not be played: {}", e.trim().trim_end_matches('.')), "Another program may be holding the speaker. Close it and try again, or choose another output.", open("Open Sound", "tab:sound"), None);
    }
    match heard {
        Said::Yes => res(id, Outcome::Pass, "You heard the test tone through the IYAKKI mixer.", "", None, None),
        _ => res(id, Outcome::Fail, "The tone was played but you did not hear it.", "Check that the volume is up and not muted, that the right output is chosen, and that the speaker or headphones are plugged in.", open("Open Sound", "tab:sound"), None),
    }
}

// ------------------------------------------------------------------------------------------------ the microphone

/// Why a recording could not be started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MicStart {
    /// No real microphone is listed (the note says why, T2 switch included).
    NoMic(String),
    Failed(String),
}

/// Start recording from the microphone the Voice tile uses: the one chosen in `VoiceSettings.mic` when it is still there, else the first
/// listed (`voicedesk::pick_mic`), with the record command of Settings -> Voice. Nothing is written to disk.
pub fn start_mic(rec: &dyn Recorders, chosen: &str, record_cmd: &str, max_secs: usize, t2_hidden: bool) -> Result<Box<dyn Capture>, MicStart> {
    let mics = rec.mics();
    if mics.is_empty() {
        return Err(MicStart::NoMic(voicedesk::no_mic_text_t2(&rec.product(), t2_hidden)));
    }
    let mic = voicedesk::pick_mic(&mics, chosen);
    let cmd = voicedesk::record_cmd_for(record_cmd, mic);
    rec.start(&cmd, max_secs).map_err(MicStart::Failed)
}

/// What a finished recording heard.
#[derive(Clone, Debug, PartialEq)]
pub enum MicHeard {
    Samples(Vec<i16>),
    NoMic(String),
    Failed(String),
}

/// Loudness of a recording as 0 to 1 (the meter): the peak against full scale, stretched so speech is visible (square root).
pub fn level_of(pcm: &[i16]) -> f32 {
    (voice_peak(pcm) as f32 / i16::MAX as f32).sqrt().min(1.0)
}

fn voice_peak(pcm: &[i16]) -> i16 {
    crate::voice::peak(pcm)
}

/// The microphone step's verdict. `name`: the microphone that was used (empty if unknown).
pub fn microphone(inv: &Inventory, heard: &MicHeard, name: &str) -> StepResult {
    let id = StepId::Microphone;
    match heard {
        MicHeard::NoMic(why) => {
            let (advice, doctor) = if inv.t2_mic_hidden {
                ("The capture switch of the T2 sound driver is off on purpose: recording can hang that computer. A USB microphone works.", Some("engine-microphone"))
            } else {
                ("Plug in a USB microphone or a headset with a USB adapter; it appears here at once.", None)
            };
            res(id, Outcome::Skipped, why.clone(), advice, None, doctor)
        }
        MicHeard::Failed(e) => res(id, Outcome::Fail, format!("The microphone could not be opened: {}", e.trim().trim_end_matches('.')), "Another program may be using it. Choose another microphone in the Voice tile, or check Settings -> Sound.", open("Open Sound", "tab:sound"), Some("engine-microphone")),
        MicHeard::Samples(pcm) => {
            let who = if name.is_empty() { String::new() } else { format!(" ({name})") };
            if pcm.is_empty() {
                return res(id, Outcome::Fail, format!("Nothing was recorded{who}."), "The microphone opened but delivered no sound. Try another microphone in the Voice tile.", open("Open Sound", "tab:sound"), Some("engine-microphone"));
            }
            if voice_peak(pcm) < 150 {
                return res(id, Outcome::Fail, format!("Only silence was heard{who}."), "Speak close to the microphone while it records. If it stays silent, check that it is not muted (Settings -> Sound) and try another microphone.", open("Open Sound", "tab:sound"), Some("engine-microphone"));
            }
            res(id, Outcome::Pass, format!("The microphone{who} heard sound (level {} %). The recording was dropped, nothing was kept.", (level_of(pcm) * 100.0).round() as u32), "", None, None)
        }
    }
}

// ------------------------------------------------------------------------------------------------ the camera

/// What the camera probe found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CamHeard {
    /// No camera is listed (the tile's own words say why).
    NoCamera(String),
    /// A picture arrived from this camera: size, and the spread between its darkest and brightest pixel.
    Picture { name: String, width: u32, height: u32, spread: u8 },
    Failed { name: String, why: String },
}

pub fn camera(heard: &CamHeard) -> StepResult {
    let id = StepId::Camera;
    match heard {
        CamHeard::NoCamera(why) => res(id, Outcome::Skipped, why.clone(), "A USB camera shows up here when you plug it in.", None, None),
        CamHeard::Failed { name, why } => res(id, Outcome::Fail, format!("The camera {name} could not be opened: {}", why.trim().trim_end_matches('.')), "Another program (the Browser or the CAMERA tile) may be holding it. Close it and try again.", open("Open Browser settings", "tab:browser"), Some("engine-camera")),
        CamHeard::Picture { name, width, height, spread } => {
            if *spread < 6 {
                return res(id, Outcome::Fail, format!("The camera {name} sent a picture ({width} x {height}) but it is one flat colour."), "A lens cover, a privacy shutter or a closed lid can cause this. Uncover the lens and try again.", None, Some("engine-camera"));
            }
            res(id, Outcome::Pass, format!("The camera {name} sent a {width} x {height} picture. It was shown here and dropped, nothing was kept."), "", None, None)
        }
    }
}

/// The darkest-to-brightest spread of an RGBA picture (0 to 255), sampled; a flat picture gives 0.
pub fn picture_spread(rgba: &[u8]) -> u8 {
    let (mut lo, mut hi) = (255u8, 0u8);
    for px in rgba.chunks_exact(4).step_by(7) {
        let l = ((px[0] as u16 * 3 + px[1] as u16 * 6 + px[2] as u16) / 10) as u8;
        lo = lo.min(l);
        hi = hi.max(l);
    }
    hi.saturating_sub(lo)
}

// ------------------------------------------------------------------------------------------------ the machine's own steps

/// The external monitor step: lists the outputs.
pub fn monitor(inv: &Inventory) -> StepResult {
    let id = StepId::Monitor;
    let connected: Vec<&ScreenOut> = inv.screens.iter().filter(|s| s.connected).collect();
    let external: Vec<&ScreenOut> = connected.iter().copied().filter(|s| !s.internal()).collect();
    let list = |v: &[&ScreenOut]| {
        v.iter().map(|s| match s.current {
            Some((w, h)) => format!("{} {w} x {h}", s.name),
            None => format!("{} (no picture)", s.name),
        }).collect::<Vec<_>>().join(", ")
    };
    if inv.screens.is_empty() {
        return res(id, Outcome::Skipped, "The screen list is not available here.", "Settings -> Display shows the outputs when the computer drives its screens itself.", None, None);
    }
    if external.is_empty() {
        let all = if connected.is_empty() { "no screen".to_string() } else { list(&connected) };
        return res(id, Outcome::Skipped, format!("No external monitor is connected (connected: {all})."), "Plug in a monitor and run this step again to test it.", None, None);
    }
    if let Some(blank) = external.iter().find(|s| s.current.is_none()) {
        return res(id, Outcome::Fail, format!("{} is connected but shows no picture.", blank.name), "Try another cable or port. Settings -> Display lets you choose its size and where it sits.", open("Open Display", "tab:display"), None);
    }
    res(id, Outcome::Pass, format!("Connected and showing a picture: {}.", list(&external)), "", None, None)
}

/// The network step: wired and wireless adapters and the route.
pub fn network(inv: &Inventory) -> StepResult {
    let id = StepId::Network;
    let all = inv.ethernet.len() + inv.wifi.len();
    if all == 0 {
        return res(id, Outcome::Fail, "No network adapter is listed.", "A wired adapter is missing its driver, or nothing is plugged in. A USB network adapter shows up as soon as it is plugged in.", open("Open Network", "tab:network"), None);
    }
    let up: Vec<&str> = inv.ethernet.iter().chain(inv.wifi.iter()).filter(|a| a.up).map(|a| a.name.as_str()).collect();
    if up.is_empty() {
        return res(id, Outcome::Fail, format!("{all} adapter{} listed, none is connected.", if all == 1 { "" } else { "s" }), "Plug in the network cable, or join a Wi-Fi network in Settings -> Network.", open("Open Network", "tab:network"), None);
    }
    if !inv.route {
        return res(id, Outcome::Fail, format!("{} is connected, but the computer has no way out to other networks.", up.join(", ")), "The router may not hand out an address. Check the cable or the Wi-Fi password in Settings -> Network.", open("Open Network", "tab:network"), None);
    }
    res(id, Outcome::Pass, format!("Connected through {} and a route out exists.", up.join(", ")), "", None, None)
}

/// The Wi-Fi step: the adapter is there and not switched off.
pub fn wifi(inv: &Inventory) -> StepResult {
    let id = StepId::Wifi;
    if inv.wifi.is_empty() {
        let blocked = if inv.wifi_blocked { " A switch has turned wireless off (the airplane-mode key)." } else { "" };
        return res(id, Outcome::Skipped, format!("No Wi-Fi adapter is listed.{blocked}"), "On a Mac the Wi-Fi chip needs firmware that you install from the firmware page; on a desktop a USB Wi-Fi stick adds it.", open("Open Network", "tab:network"), None);
    }
    if inv.wifi_blocked {
        return res(id, Outcome::Fail, format!("Wi-Fi adapter {} is listed but switched off.", inv.wifi[0].name), "Press the airplane-mode key or the Wi-Fi switch of the laptop, then run this step again.", open("Open Network", "tab:network"), None);
    }
    let state = if inv.wifi.iter().any(|a| a.up) { "joined to a network" } else { "not joined to a network yet" };
    res(id, Outcome::Pass, format!("Wi-Fi adapter {} is listed and switched on ({state}).", inv.wifi[0].name), "", None, None)
}

/// The Bluetooth step.
pub fn bluetooth(inv: &Inventory) -> StepResult {
    let id = StepId::Bluetooth;
    if inv.bluetooth == 0 {
        return res(id, Outcome::Skipped, "No Bluetooth adapter is listed.", "A USB Bluetooth adapter adds it. On some Macs the Bluetooth chip needs firmware from the firmware page.", open("Open Bluetooth", "tab:bluetooth"), None);
    }
    if inv.bluetooth_blocked {
        return res(id, Outcome::Fail, "A Bluetooth adapter is listed but switched off.", "Press the airplane-mode key, or switch Bluetooth on in Settings -> Bluetooth.", open("Open Bluetooth", "tab:bluetooth"), None);
    }
    res(id, Outcome::Pass, format!("{} Bluetooth adapter{} listed and switched on.", inv.bluetooth, if inv.bluetooth == 1 { " is" } else { "s are" }), "", None, None)
}

/// The battery and power step.
pub fn power(inv: &Inventory) -> StepResult {
    let id = StepId::Power;
    let p = &inv.power;
    if p.battery {
        let pct = p.percent.map_or("?".to_string(), |v| format!("{v} %"));
        let ac = match p.ac_online {
            Some(true) => ", charger connected",
            Some(false) => ", running on the battery",
            None => "",
        };
        let health = p.health_pct.map_or(String::new(), |h| format!(", holds {h} % of its original capacity"));
        if p.health_pct.is_some_and(|h| h < 50) {
            return res(id, Outcome::Fail, format!("Battery {pct} ({}){ac}{health}.", p.status), "The battery is worn: it holds less than half of what it did when new. It still works, but not for long.", open("Open Battery", "tab:battery"), None);
        }
        return res(id, Outcome::Pass, format!("Battery {pct} ({}){ac}{health}.", p.status), "", None, None);
    }
    if p.no_driver {
        return res(id, Outcome::Fail, "The computer says it has a battery, but the kernel has no driver for it.", "The battery cannot be read, so the charge is not shown. Settings -> Battery explains what is known.", open("Open Battery", "tab:battery"), None);
    }
    if p.laptop {
        return res(id, Outcome::Fail, "This looks like a laptop, but no battery is listed.", "The battery may be removed or dead. Check that it is inserted, and try the charger.", open("Open Battery", "tab:battery"), None);
    }
    let ac = if p.ac_online == Some(true) { " It runs on mains power." } else { "" };
    res(id, Outcome::Skipped, format!("No battery: this is not a laptop.{ac}"), "Nothing to do.", None, None)
}

// ------------------------------------------------------------------------------------------------ the report

/// The summary sentence: "3 passed, 1 failed, 2 skipped, 6 not done yet".
pub fn summary(results: &[StepResult]) -> String {
    let n = |o: Outcome| results.iter().filter(|r| r.outcome == o).count();
    format!("{} passed, {} failed, {} skipped, {} not done yet", n(Outcome::Pass), n(Outcome::Fail), n(Outcome::Skipped), n(Outcome::Pending))
}

/// The report as plain lines (the Check-up section of the diagnostics bundle): one line per step, the advice under a failure.
pub fn report_lines(results: &[StepResult]) -> Vec<String> {
    let mut out = vec![summary(results)];
    for r in results {
        let note = if r.note.is_empty() { String::new() } else { format!(" - {}", r.note) };
        out.push(format!("{}: {}{}", r.id.title(), r.outcome.word(), note));
        if r.outcome == Outcome::Fail && !r.advice.is_empty() {
            out.push(format!("  advice: {}", r.advice));
        }
        if let (Outcome::Fail, Some(d)) = (r.outcome, r.doctor) {
            out.push(format!("  DOCTOR rule: {d}"));
        }
    }
    out
}

static LAST: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Keep the newest report where Share diagnostics finds it (memory only; empty when no check-up ran).
pub fn set_last_report(lines: Vec<String>) {
    if let Ok(mut l) = LAST.lock() {
        *l = lines;
    }
}

pub fn last_report() -> Vec<String> {
    LAST.lock().map(|l| l.clone()).unwrap_or_default()
}

/// How long the microphone step records.
pub const MIC_SECS: Duration = Duration::from_secs(2);

#[cfg(test)]
#[path = "checkup_tests.rs"]
mod tests;
