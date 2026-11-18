//! Camera views an app may embed (docs/development/UI-SDK.md, "Embedding camera views"; owner request 2026-11-16).
//!
//! A camera is a privacy-sensitive device. An app (an SDK app, a WASM tile, a client) never opens one: it asks the core for a *view*, and
//! the core decides everything that matters:
//!
//! * **Who sees what.** [`Cameras::list`] names only the cameras the signed-in person may watch (the camera's owner and the people on its
//!   list; this computer's own USB cameras only for an account that may use them). A camera the person may not watch is not in the list,
//!   cannot be opened, and the refusal sentence is the same as for a camera that does not exist.
//! * **Consent.** [`Cameras::open`] starts nothing. It returns a view that waits for the person: [`CamEvent::Approval`] carries the card
//!   ("App X wants to show 2 cameras: Front door, Yard") and nothing is pulled from a camera before [`Cameras::decide`] allows it. A denied
//!   card leaves the view closed. There is no standing grant for an embedded view.
//! * **An indicator.** [`ViewState::indicator`] is true while a view is allowed, open and not hidden: the app must show it (the view
//!   component does) and the machine's own sign reports it.
//! * **Nothing is written by a view.** Frames are handed to the app and kept nowhere. A snapshot is a separate request, asks the person
//!   every time and saves one JPEG into the person's own Pictures folder (the core's rule for files), never anywhere else.
//! * **Pan, tilt and zoom** are a separate request that asks once per view and camera, and only for a source that offers them
//!   ([`CameraInfo::ptz`]).
//! * **Audit, by names only.** `cameras.open app: Front door, Yard`, `cameras.consent app allowed`, `cameras.snapshot app: Front door`,
//!   `cameras.ptz app: Front door`, `cameras.close app`. Never a frame, an address, a password or a path.
//! * **Limits** ([`MAX_CELLS`], [`MAX_VIEWS_PER_PERSON`], [`MAX_VIEWS_PER_APP`]) are the core's; a request over a limit is refused with a
//!   sentence. A hidden view pulls nothing ([`Cameras::set_visible`]).
//!
//! Frames come as RGBA or as a complete JPEG ([`FrameFormat`]), newest only: [`Cameras::frame`] gives the latest picture newer than the
//! sequence number the caller has, or `None`. Nothing queues, so a slow viewer sees fewer frames and never old ones.
//!
//! Motion events and the zones of the person's own motion detector are read-only: [`Cameras::overlay`] hands them to the view for
//! drawing; a view cannot change a zone, a mask or the recording.

use serde::{Deserialize, Serialize};

/// The version of this facet (the types and the trait below). Additive changes only.
pub const CAMERAS_FACET_VERSION: u32 = 1;

/// Most cameras in one view (a grid of up to 4).
pub const MAX_CELLS: usize = 4;
/// Most views one person may have open at once, and most of them for one app.
pub const MAX_VIEWS_PER_PERSON: usize = 4;
pub const MAX_VIEWS_PER_APP: usize = 2;
/// Most events kept for a view that does not read them.
pub const MAX_QUEUED_EVENTS: usize = 64;
/// Most motion marks and zone shapes of one overlay.
pub const MAX_MARKS: usize = 16;
pub const MAX_SHAPES: usize = 16;
/// The longest a person's or a camera's name is shown (device text is cut, not trusted).
pub const MAX_NAME: usize = 60;

/// Where a camera is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// Plugged into this computer; showing it lights the camera's own light.
    Usb,
    /// A network camera (RTSP, HTTP Motion-JPEG).
    Network,
}

/// What a camera is doing, in the words the hub uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CamStatus {
    /// Not asked yet, or connecting.
    Connecting,
    Live,
    /// The stream broke; the sentence in [`CameraInfo::detail`] says why.
    Lost,
    /// The camera sends video this system cannot decode yet (the add-on).
    NeedsAddon,
    /// Another app holds the camera.
    Busy,
    /// The camera's password is not available (the owner's vault is closed).
    NeedsPassword,
    /// Pictures arrive but are black (covered camera, closed shutter).
    Dark,
}

impl CamStatus {
    pub fn word(self) -> &'static str {
        match self {
            CamStatus::Connecting => "connecting",
            CamStatus::Live => "live",
            CamStatus::Lost => "lost",
            CamStatus::NeedsAddon => "needs the video add-on",
            CamStatus::Busy => "in use by another app",
            CamStatus::NeedsPassword => "needs its password",
            CamStatus::Dark => "dark",
        }
    }
}

/// One camera as the person may see it. `name` and `detail` are device text: show them, do not trust them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraInfo {
    pub id: String,
    pub name: String,
    pub source: SourceKind,
    pub status: CamStatus,
    /// The sentence behind a status that is not live (why it was lost, what the add-on is), else empty.
    pub detail: String,
    /// The source takes pan, tilt and zoom commands.
    pub ptz: bool,
    /// The camera is being recorded now (its owner switched that on).
    pub recording: bool,
    /// The camera's pictures are being analysed for motion now.
    pub motion_watch: bool,
    /// The size of the last picture, `(0, 0)` before the first.
    pub size: (u32, u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameSize {
    /// A grid cell (at most 480 wide, 5 pictures a second).
    Small,
    /// One camera filling the view (at most 1280 wide, 10 pictures a second).
    Large,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameFormat {
    /// 8-bit red, green, blue, alpha, row after row (for a canvas that draws pixels).
    Rgba,
    /// One complete JPEG file (for a display list that travels to a browser or client).
    Jpeg,
}

/// What an app asks for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewSpec {
    /// One to [`MAX_CELLS`] camera ids from [`Cameras::list`].
    pub cameras: Vec<String>,
    pub size: FrameSize,
    /// Sound with the picture. This core has no camera audio: `true` is refused with a sentence.
    pub audio: bool,
    /// What the person sees on the card ("Doorbell panel"): the app's own words, cut to [`MAX_NAME`].
    pub label: String,
}

impl Default for ViewSpec {
    fn default() -> ViewSpec {
        ViewSpec { cameras: Vec::new(), size: FrameSize::Small, audio: false, label: String::new() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewInfo {
    pub id: String,
    /// The cameras of the view, in the order asked.
    pub cameras: Vec<CameraInfo>,
}

/// What a view is waiting for, or doing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum ViewPhase {
    /// The card is shown; nothing is pulled from any camera.
    AwaitingConsent { call: String },
    Open,
    /// The person said no (or the core could not ask): the view shows nothing.
    Denied { reason: String },
    Closed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewState {
    pub phase: ViewPhase,
    /// The view is allowed, open and not hidden: pictures are being pulled. The app shows a sign while this is true.
    pub indicator: bool,
    pub visible: bool,
    pub cameras: Vec<CameraInfo>,
}

/// One newest picture of one camera.
#[derive(Clone, Debug, PartialEq)]
pub struct CamFrame {
    /// Grows with every new picture of the camera; ask for `after` this to get a newer one.
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub format: FrameFormat,
    pub data: std::sync::Arc<Vec<u8>>,
}

/// A shape of the person's own motion detector: a zone where motion counts (`include`) or does not, or a privacy mask. Points are
/// 0.0 to 1.0 across the picture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OverlayShape {
    pub name: String,
    pub include: bool,
    pub mask: bool,
    pub points: Vec<(f32, f32)>,
}

/// A recent motion event: when, in which zone, how strong, and where (left, top, right, bottom, 0.0 to 1.0).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MotionMark {
    pub id: u64,
    pub at_ms: u64,
    pub zone: String,
    pub score: u8,
    pub bbox: [f32; 4],
    /// The event is still going on.
    pub active: bool,
}

/// Read-only drawing hints for one camera. Empty for a person who does not own the camera (zones, masks and events are the owner's).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Overlay {
    pub zones: Vec<OverlayShape>,
    pub marks: Vec<MotionMark>,
}

/// Pan, tilt and zoom, one step or a stop. Speeds are -1.0 to 1.0 (negative: left, down, out).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum PtzCommand {
    Move { pan: f32, tilt: f32, zoom: f32 },
    Stop,
}

impl PtzCommand {
    /// The command with every speed brought into -1.0 to 1.0; a speed that is not a number is refused.
    pub fn checked(self) -> Result<PtzCommand, String> {
        match self {
            PtzCommand::Stop => Ok(self),
            PtzCommand::Move { pan, tilt, zoom } => {
                if ![pan, tilt, zoom].iter().all(|v| v.is_finite()) {
                    return Err("a pan, tilt or zoom speed is not a number".into());
                }
                Ok(PtzCommand::Move { pan: pan.clamp(-1.0, 1.0), tilt: tilt.clamp(-1.0, 1.0), zoom: zoom.clamp(-1.0, 1.0) })
            }
        }
    }
}

/// What a card asks about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CamAction {
    /// Show the cameras of the view.
    View,
    /// Save one picture of one camera in the person's Pictures.
    Snapshot,
    /// Move one camera.
    Ptz,
}

impl CamAction {
    pub fn word(self) -> &'static str {
        match self {
            CamAction::View => "view",
            CamAction::Snapshot => "snapshot",
            CamAction::Ptz => "ptz",
        }
    }
}

/// What a view tells the app, in order. [`Cameras::events`] hands each event out once.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CamEvent {
    /// A card: the person decides ([`Cameras::decide`]) before anything happens. `camera` is empty for a card about the whole view.
    Approval { call: String, action: CamAction, camera: String, summary: String, detail: String },
    /// How a card was answered: `by` is `user` or `system` (no screen to ask on, the view was closed).
    Decision { call: String, action: CamAction, approved: bool, by: String },
    /// A motion event of a camera in the view (the owner's detector).
    Motion { camera: String, mark: MotionMark },
    /// A snapshot was saved: the file's name and the folder's name, never the whole path.
    Snapshot { camera: String, file: String, folder: String },
    /// A request that was allowed did not work, in a sentence.
    Failed { camera: String, action: CamAction, error: String },
    /// A camera's status changed.
    Status { camera: String, status: CamStatus, detail: String },
}

/// The camera views of the core, as an app embeds them. Every call names the signed-in person; a view belongs to the person who opened
/// it and to the app that named itself at `open`, and another person's view id is refused like an unknown one.
pub trait Cameras: Send + Sync {
    /// The cameras the person may watch. Free: no stream starts, no picture is read. Empty: none, or this core has no cameras.
    fn list(&self, _user: &str, _app: &str) -> Vec<CameraInfo> {
        Vec::new()
    }
    /// Make a view. Nothing is pulled from a camera until the person allows the card that [`CamEvent::Approval`] carries.
    fn open(&self, _user: &str, _app: &str, _spec: &ViewSpec) -> Result<ViewInfo, String> {
        Err("this core has no camera views".into())
    }
    /// The person's answer to a card (`call` from [`CamEvent::Approval`]).
    fn decide(&self, _user: &str, _view: &str, _call: &str, _approve: bool) -> Result<(), String> {
        Err("this core has no camera views".into())
    }
    fn state(&self, _user: &str, _view: &str) -> Result<ViewState, String> {
        Err("this core has no camera views".into())
    }
    /// The view is on screen (`true`) or hidden (a tab behind, a window minimised, a pane scrolled away). A hidden view pulls nothing.
    fn set_visible(&self, _user: &str, _view: &str, _visible: bool) -> Result<(), String> {
        Err("this core has no camera views".into())
    }
    /// The newest picture of a camera of the view with a sequence number above `after`; `None` when there is none (nothing new, the view
    /// waits for consent, is hidden or closed). The core paces itself: asking faster than the camera's rate returns `None`, not an error.
    fn frame(&self, _user: &str, _view: &str, _camera: &str, _after: u64, _format: FrameFormat) -> Result<Option<CamFrame>, String> {
        Err("this core has no camera views".into())
    }
    /// Zones, masks and recent motion marks of a camera of the view, read-only.
    fn overlay(&self, _user: &str, _view: &str, _camera: &str) -> Result<Overlay, String> {
        Err("this core has no camera views".into())
    }
    /// Ask to save one picture of a camera in the person's Pictures. The answer is the call id of the card; after the person allows it
    /// a [`CamEvent::Snapshot`] (or [`CamEvent::Failed`]) comes. Asks every time.
    fn snapshot(&self, _user: &str, _view: &str, _camera: &str) -> Result<String, String> {
        Err("this core has no camera views".into())
    }
    /// A pan, tilt or zoom command. The first command of a view for a camera asks (the answer is the call id of the card, the command
    /// runs once the person allows it); later commands for the same camera and view run at once (`Ok(None)`). A source without pan, tilt
    /// and zoom refuses with a sentence.
    fn ptz(&self, _user: &str, _view: &str, _camera: &str, _cmd: PtzCommand) -> Result<Option<String>, String> {
        Err("this core has no camera views".into())
    }
    /// The events since the last call, in order.
    fn events(&self, _user: &str, _view: &str) -> Vec<CamEvent> {
        Vec::new()
    }
    /// Close the view: nothing more is pulled, a waiting card is denied.
    fn close(&self, _user: &str, _view: &str) {}
    /// Whether apps may show this person's cameras in their own windows at all (the person's switch; on until they switch it off). A
    /// core that has no camera views says `false`.
    fn enabled(&self, _user: &str) -> bool {
        false
    }
    /// Switch it. Off closes every view of the person at once (nothing more is pulled, a waiting card is denied) and `open` is refused with
    /// a sentence until it is switched on again. The switch is the person's: Settings, or a chat tool that asks a card.
    fn set_enabled(&self, _user: &str, _on: bool) -> Result<(), String> {
        Err("this core has no camera views".into())
    }
    /// The names of the cameras being shown to anybody on this machine right now (views that are allowed, open and visible): what the
    /// machine's own red sign says besides recordings. Names only.
    fn indicator_names(&self) -> Vec<String> {
        Vec::new()
    }
    /// How many views of the person are open now (allowed or waiting for the card).
    fn open_views(&self, _user: &str) -> usize {
        0
    }
}

/// The sentence for a person who switched camera views off.
pub const SWITCHED_OFF: &str = "Camera views in apps are switched off for you (Settings, or ask the assistant to switch them on).";

/// The cameras of a core that has none: every call is refused with a sentence, the list is empty.
pub struct NoCameras;

impl Cameras for NoCameras {}

/// The words every implementation uses for a camera the person may not watch (the same for one that does not exist).
pub const NOT_YOURS: &str = "no such camera, or you are not on its list of people who may watch";

/// The refusal for a request that breaks a limit or is malformed, as a sentence; used by every implementation so the words are the same.
pub fn check_spec(app: &str, spec: &ViewSpec) -> Result<(), String> {
    if app.is_empty() || app.len() > 40 || !app.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') || !app.starts_with(|c: char| c.is_ascii_lowercase()) {
        return Err("the app name must be lower-case letters, digits and `-`, starting with a letter".into());
    }
    if spec.cameras.is_empty() {
        return Err("a camera view needs at least one camera".into());
    }
    if spec.cameras.len() > MAX_CELLS {
        return Err(format!("a camera view shows at most {MAX_CELLS} cameras"));
    }
    let mut seen = std::collections::BTreeSet::new();
    for c in &spec.cameras {
        if c.is_empty() || c.len() > 64 || c.chars().any(char::is_control) {
            return Err("a camera id is 1 to 64 characters without control characters".into());
        }
        if !seen.insert(c.as_str()) {
            return Err("a camera may be in a view once".into());
        }
    }
    if spec.audio {
        return Err("camera views carry no sound in this system".into());
    }
    if spec.label.chars().any(char::is_control) {
        return Err("the label of a camera view has no control characters".into());
    }
    Ok(())
}

/// Text made safe to show or write to an audit line: no control characters, cut at [`MAX_NAME`].
pub fn clean_name(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(MAX_NAME).collect::<String>().trim().to_string()
}

/// The card text for opening a view: what is shown and what is not kept.
pub fn view_card(app: &str, label: &str, names: &[String]) -> (String, String) {
    let shown: Vec<String> = names.iter().map(|n| clean_name(n)).collect();
    let what = if label.trim().is_empty() { app.to_string() } else { format!("{app} ({})", clean_name(label)) };
    let summary = format!("{what} wants to show {} camera{}: {}", shown.len(), if shown.len() == 1 { "" } else { "s" }, shown.join(", "));
    (summary, "Live pictures only. Nothing is recorded or saved by the view. A sign shows while it is open and visible.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(cams: &[&str]) -> ViewSpec {
        ViewSpec { cameras: cams.iter().map(|s| s.to_string()).collect(), ..ViewSpec::default() }
    }

    #[test]
    fn the_limits_refuse_with_a_sentence() {
        assert!(check_spec("sample", &spec(&["a"])).is_ok());
        assert!(check_spec("Sample", &spec(&["a"])).unwrap_err().contains("lower-case"));
        assert!(check_spec("sample", &spec(&[])).unwrap_err().contains("at least one"));
        assert!(check_spec("sample", &spec(&["a", "b", "c", "d", "e"])).unwrap_err().contains("at most 4"));
        assert!(check_spec("sample", &spec(&["a", "a"])).unwrap_err().contains("once"));
        assert!(check_spec("sample", &spec(&["a\n"])).unwrap_err().contains("control"));
        let mut s = spec(&["a"]);
        s.audio = true;
        assert!(check_spec("sample", &s).unwrap_err().contains("no sound"));
    }

    #[test]
    fn a_core_with_no_cameras_refuses_and_lists_nothing() {
        let n = NoCameras;
        assert!(n.list("u", "app").is_empty() && n.events("u", "v").is_empty());
        assert!(n.open("u", "app", &spec(&["a"])).unwrap_err().contains("no camera views"));
        assert!(n.frame("u", "v", "a", 0, FrameFormat::Rgba).is_err() && n.snapshot("u", "v", "a").is_err());
        assert!(n.ptz("u", "v", "a", PtzCommand::Stop).is_err() && n.state("u", "v").is_err());
        assert!(!n.enabled("u") && n.open_views("u") == 0 && n.set_enabled("u", true).is_err());
    }

    #[test]
    fn ptz_speeds_are_clamped_and_must_be_numbers() {
        assert_eq!(PtzCommand::Move { pan: 4.0, tilt: -3.0, zoom: 0.5 }.checked().unwrap(), PtzCommand::Move { pan: 1.0, tilt: -1.0, zoom: 0.5 });
        assert!(PtzCommand::Move { pan: f32::NAN, tilt: 0.0, zoom: 0.0 }.checked().is_err());
        assert_eq!(PtzCommand::Stop.checked().unwrap(), PtzCommand::Stop);
    }

    #[test]
    fn the_card_names_the_cameras_and_says_nothing_is_kept_and_device_text_is_cleaned() {
        let (s, d) = view_card("sample", "Panel", &["Front\u{7}door".into(), "Yard".into()]);
        assert_eq!(s, "sample (Panel) wants to show 2 cameras: Frontdoor, Yard");
        assert!(d.contains("Nothing is recorded"));
        assert!(view_card("sample", "", &["A".into()]).0.contains("1 camera: A"));
    }

    #[test]
    fn events_have_a_stable_wire_shape() {
        let e = CamEvent::Snapshot { camera: "front".into(), file: "camera-front.jpg".into(), folder: "Pictures".into() };
        let j = serde_json::to_string(&e).unwrap();
        assert_eq!(j, "{\"type\":\"snapshot\",\"camera\":\"front\",\"file\":\"camera-front.jpg\",\"folder\":\"Pictures\"}");
        assert_eq!(serde_json::from_str::<CamEvent>(&j).unwrap(), e);
        let c = serde_json::to_string(&PtzCommand::Move { pan: 0.5, tilt: 0.0, zoom: 0.0 }).unwrap();
        assert_eq!(c, "{\"cmd\":\"move\",\"pan\":0.5,\"tilt\":0.0,\"zoom\":0.0}");
    }
}
