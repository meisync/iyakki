//! The plain data and the port of the CHECK-UP tile (D-S1-E4-01). The steps, their results and what a person said live here as plain
//! mirrors of the pure types in `iyakki_shared::checkup`; the shell converts one to the other (one `match` each way, a test keeps them
//! in step). The tile never sees the machine's inventory: it asks the [`CheckupPort`] for a verdict on a [`Check`], and the port
//! (the shell's real one, or a fake in a test) answers with a [`StepResult`].

use std::any::Any;
use std::time::Duration;

/// How long the microphone step records.
pub const MIC_SECS: Duration = Duration::from_secs(2);
/// Longest the microphone step records, a little over [`MIC_SECS`] so the stop is ours.
pub const MIC_MAX_SECS: usize = 4;

/// How one step ended.
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

/// What the person did in a step that needs them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Said {
    Yes,
    No,
    Skip,
}

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
    Gpu,
    Usb,
}

impl StepId {
    pub const ALL: [StepId; 14] = [StepId::Display, StepId::Touch, StepId::Keyboard, StepId::Pointer, StepId::Speaker, StepId::Microphone, StepId::Camera, StepId::Monitor, StepId::Network, StepId::Wifi, StepId::Bluetooth, StepId::Power, StepId::Gpu, StepId::Usb];

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
            StepId::Gpu => "gpu",
            StepId::Usb => "usb",
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
            StepId::Gpu => "Graphics card for models",
            StepId::Usb => "USB devices",
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

/// What a fix button does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FixAction {
    /// Open this Settings page (`tab:firewall`).
    Settings(&'static str),
    /// Developer mode: push its switch-off time four hours on.
    ExtendDeveloperMode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fix {
    /// The button's words.
    pub label: &'static str,
    pub action: FixAction,
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
    pub fix: Option<Fix>,
    /// The id of the DOCTOR rule this step corresponds to, if there is one.
    pub doctor: Option<&'static str>,
}

impl StepResult {
    pub fn pending(id: StepId) -> StepResult {
        StepResult { id, outcome: Outcome::Pending, note: String::new(), advice: String::new(), fix: None, doctor: None }
    }
}

/// The line over the list: how many steps ended each way.
pub fn summary(results: &[StepResult]) -> String {
    let n = |o: Outcome| results.iter().filter(|r| r.outcome == o).count();
    format!("{} passed, {} failed, {} skipped, {} not done yet", n(Outcome::Pass), n(Outcome::Fail), n(Outcome::Skipped), n(Outcome::Pending))
}

/// Why a recording could not be started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MicStart {
    NoMic(String),
    Failed(String),
}

/// What a finished recording heard.
#[derive(Clone, Debug, PartialEq)]
pub enum MicHeard {
    Samples(Vec<i16>),
    NoMic(String),
    Failed(String),
}

/// What the camera probe found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CamHeard {
    NoCamera(String),
    /// A picture arrived: size, and the spread between its darkest and brightest pixel.
    Picture { name: String, width: u32, height: u32, spread: u8 },
    Failed { name: String, why: String },
}

/// One camera picture: what was found and the pixels (straight RGBA), if any.
pub struct Shot {
    pub heard: CamHeard,
    pub picture: Option<(u32, u32, Vec<u8>)>,
}

/// A recording in progress (16 kHz mono; nothing is written to disk).
pub trait MicCapture: Send {
    /// Loudness of the last moment, 0 to 1.
    fn level(&self) -> f32;
    fn elapsed(&self) -> Duration;
    /// The longest recording was reached.
    fn is_full(&self) -> bool;
    /// The recording program is still running.
    fn is_alive(&mut self) -> bool;
    /// Stop and give the samples.
    fn stop(self: Box<Self>) -> Result<Vec<i16>, String>;
}

/// What the tile asks the port to judge: one step with what the person or the probe produced.
#[derive(Clone, Debug, PartialEq)]
pub enum Check {
    Display(Said),
    Touch(Said),
    Keyboard { keys: usize, said: Said },
    Pointer { clicked: bool, said: Said },
    Speaker { played: Result<(), String>, said: Said },
    Microphone { heard: MicHeard, name: String },
    Camera(CamHeard),
    /// A step the machine answers alone (monitor, network, Wi-Fi, Bluetooth, power, GPU, USB).
    Machine(StepId),
    /// The person (or the tile) skipped the step.
    Skipped(StepId),
}

/// A running microphone: the recording and the name of the microphone it listens to.
pub struct Listening {
    pub capture: Box<dyn MicCapture>,
    pub name: String,
}

/// What the tile asks of the rest of the system. The real port reads the machine, the session and the core; a fake answers in tests.
pub trait CheckupPort {
    /// Read the machine again (a plugged-in microphone or a joined network counts) and say how many touch screens it lists.
    fn refresh(&self) -> usize;
    /// The verdict on one step, from the machine's last reading.
    fn verdict(&self, check: &Check) -> StepResult;
    /// Play the test tone; the returned value keeps the sound alive (dropping it stops it).
    fn play_tone(&self) -> Result<Box<dyn Any + Send>, String>;
    /// A job that takes one camera picture (it runs on a thread of its own).
    fn camera_job(&self) -> Box<dyn FnOnce() -> Shot + Send>;
    /// Start the microphone the Voice tile uses.
    fn start_mic(&self) -> Result<Listening, MicStart>;
    /// The newest results: kept for Share diagnostics, and a finished check-up leaves a signed file.
    fn keep_report(&self, results: &[StepResult]);
    /// Tell the shell's sign what is capturing (`media_kind` bits, 0 for nothing).
    fn sign(&self, bits: u8);
    /// The words of the sign for those bits ("Camera on").
    fn sign_text(&self, bits: u8) -> String;
    /// The person clicked the sign: stop everything that captures.
    fn stop_asked(&self) -> bool;
    /// A line in the audit log for the signed-in person.
    fn audit(&self, action: &str, subject: &str);
}

/// The `media_kind` bits of the sign (the same numbers as `iyakki_webview::proto::media_kind`; a test in the shell keeps them equal).
pub mod bits {
    pub const MICROPHONE: u8 = 1;
    pub const CAMERA: u8 = 2;
}
