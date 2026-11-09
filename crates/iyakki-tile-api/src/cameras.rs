//! The cameras facade: what the CAMERAS tile (a live view of USB and network cameras, F-CAM-02) asks of the shell. Plain data and one
//! source trait; the agent's `cameras` module has the real thing (saved cameras, the vault, access rules, streams) and the shell
//! adapts it. The tile never touches the network or a device: it asks for the newest picture of each camera it shows.

/// Where a camera is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A camera plugged into this computer.
    Usb,
    /// An RTSP or HTTP camera on the network.
    Network,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Not asked for yet, or asked and nothing has arrived.
    Connecting,
    Live,
    /// The connection failed or broke; the stream is tried again with a growing pause. The text says why, with no secret in it.
    Lost(String),
    /// The camera sends H.264 and this computer has no video decoder: the person can add the video add-on.
    NeedsAddon(String),
    /// The camera is used by something else on this computer.
    Busy(String),
    /// The vault that holds the camera's password is locked, or the password is missing.
    NeedsPassword(String),
}

impl Status {
    pub fn label(&self) -> &'static str {
        match self {
            Status::Connecting => "Connecting",
            Status::Live => "Live",
            Status::Lost(_) => "Lost",
            Status::NeedsAddon(_) => "Needs video add-on",
            Status::Busy(_) => "In use",
            Status::NeedsPassword(_) => "Needs password",
        }
    }
    pub fn detail(&self) -> &str {
        match self {
            Status::Lost(m) | Status::NeedsAddon(m) | Status::Busy(m) | Status::NeedsPassword(m) => m,
            _ => "",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CamInfo {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// The address without any user name or password; empty for a USB camera.
    pub address: String,
    pub user: String,
    pub status: Status,
    pub fps: f32,
    pub size: (u32, u32),
    /// The person may change or remove this camera and its access list.
    pub manage: bool,
    pub owner: String,
    /// People who may watch besides the owner (empty for a USB camera: everybody who can use the CAMERA app).
    pub allowed: Vec<String>,
    pub has_password: bool,
}

/// A picture to draw: straight opaque RGBA.
#[derive(Clone, Debug)]
pub struct Frame {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: std::sync::Arc<Vec<u8>>,
}

/// What the person typed in the form.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct NewCam {
    pub name: String,
    pub address: String,
    pub user: String,
    pub allowed: Vec<String>,
}

/// How a camera is recorded (F-CAM-03). Recording is off unless the owner turned it on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecMode {
    Off,
    Continuous,
    Schedule,
}

/// A camera's recording setting and state, as the recordings page shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecInfo {
    pub mode: RecMode,
    /// Weekly windows in words, like `weekdays 08:00-18:00`.
    pub windows: Vec<String>,
    pub max_days: u32,
    pub max_mb: u64,
    /// What the recorder is doing, in words.
    pub state: String,
    /// The recorder is running (the red REC mark).
    pub recording: bool,
    /// Stopped by a problem that needs a person (disk nearly full ...).
    pub problem: bool,
    /// The person is the owner: only they can turn recording on, play it back, export it and read the log.
    pub owner: bool,
    pub segments: usize,
    pub bytes: u64,
    /// The line about the key in the owner's vault.
    pub key_note: String,
}

/// A day with recordings: its recorded stretches in milliseconds since 1970.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecDay {
    pub label: String,
    pub day_start_ms: u64,
    pub spans: Vec<(u64, u64)>,
    pub frames: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug)]
pub struct PlayFrame {
    pub ts_ms: u64,
    /// Date and time in words (local).
    pub label: String,
    pub width: u32,
    pub height: u32,
    pub rgba: std::sync::Arc<Vec<u8>>,
    pub prev_ms: Option<u64>,
    pub next_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecSeek {
    AtOrAfter,
    After,
    Before,
}

/// One line of the camera's log of who looked at what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditRow {
    pub time: String,
    pub who: String,
    pub what: String,
    pub ok: bool,
}

/// What a recording change asks for; `None` keeps the current value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecChange {
    pub mode: Option<RecMode>,
    pub windows: Option<Vec<String>>,
    pub max_days: Option<u32>,
    pub max_mb: Option<u64>,
}

pub trait CameraSource {
    /// The cameras this person may watch (USB cameras of this computer first), in a stable order.
    fn cameras(&self) -> Vec<CamInfo>;
    /// The newest picture of a camera; asking also keeps the stream going (it stops a few seconds after the last ask). `big` asks for
    /// the larger size used when one camera fills the window.
    fn view(&self, id: &str, big: bool) -> Option<Frame>;
    /// Save the newest picture in the person's Pictures folder; the path (shown to the person) on success.
    fn snapshot(&self, id: &str) -> Result<String, String>;
    /// Save a network camera. `password`: `None` keeps the stored one, `Some("")` removes it, `Some(text)` stores it in the vault.
    fn save(&self, replacing: Option<&str>, cam: &NewCam, password: Option<&str>) -> Result<String, String>;
    fn remove(&self, id: &str) -> Result<(), String>;
    /// People who can be given access (everybody with an account except the person).
    fn people(&self) -> Vec<String>;

    // ---- recording (F-CAM-03); a source without recording keeps these defaults

    /// The ids of the cameras whose recorder is running now (each gets a red REC mark).
    fn recording_ids(&self) -> Vec<String> {
        vec![]
    }
    fn rec_info(&self, _id: &str) -> Option<RecInfo> {
        None
    }
    fn rec_set(&self, _id: &str, _change: &RecChange) -> Result<(), String> {
        Err("Recording is not available here.".into())
    }
    /// Days with recordings, newest first (owner only).
    fn rec_days(&self, _id: &str) -> Result<Vec<RecDay>, String> {
        Ok(vec![])
    }
    /// The person opened the recordings of a camera at this time (written to the camera's log).
    fn rec_play_start(&self, _id: &str, _at_ms: u64) -> Result<(), String> {
        Ok(())
    }
    fn rec_frame(&self, _id: &str, _at_ms: u64, _seek: RecSeek) -> Result<Option<PlayFrame>, String> {
        Ok(None)
    }
    /// Copy the pictures between two times, decrypted, into the person's folder. `confirmed` is true only after the person said yes.
    fn rec_export(&self, _id: &str, _from_ms: u64, _to_ms: u64, _confirmed: bool) -> Result<String, String> {
        Err("Recording is not available here.".into())
    }
    fn rec_audit(&self, _id: &str) -> Result<Vec<AuditRow>, String> {
        Ok(vec![])
    }
}

/// A source with no cameras.
pub struct NoCameras;

impl CameraSource for NoCameras {
    fn cameras(&self) -> Vec<CamInfo> {
        vec![]
    }
    fn view(&self, _: &str, _: bool) -> Option<Frame> {
        None
    }
    fn snapshot(&self, _: &str) -> Result<String, String> {
        Err("no cameras".into())
    }
    fn save(&self, _: Option<&str>, _: &NewCam, _: Option<&str>) -> Result<String, String> {
        Err("no cameras".into())
    }
    fn remove(&self, _: &str) -> Result<(), String> {
        Err("no cameras".into())
    }
    fn people(&self) -> Vec<String> {
        vec![]
    }
}
