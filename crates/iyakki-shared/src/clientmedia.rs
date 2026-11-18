//! Types moved out of `iyakki_agent::clientmedia` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;
use iyakki_core_api::client::media::self as m;
use iyakki_core_api::client::media::Chunker;
use iyakki_core_api::client::media::JitterQueue;
use iyakki_core_api::client::media::Side;

/// A route that has not been pulled from for this long belongs to a hung connection: it stops silencing the machine.
pub const ROUTE_ALIVE: Duration = Duration::from_secs(4);

pub struct Route {
    pub person: String,
    pub side: Side,
    pub chunker: Chunker,
    pub queue: JitterQueue<Vec<u8>>,
    pub seq: u32,
    pub last_pull: Instant,
    /// The sound stopped or paused since the session last pulled: tell the client to drop what it holds.
    pub flush: bool,
}

/// What a session takes from its route.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pulled {
    /// Tell the client to throw away the sound it holds, before anything else.
    pub flush: bool,
    /// Audio payloads in order (`[seq][pcm]`).
    pub chunks: Vec<Vec<u8>>,
}

/// What `open` did.
#[derive(Debug, PartialEq, Eq)]
pub enum Opened {
    New,
    Same,
    /// The session had the other side; it was switched (what waited was dropped).
    Switched,
}

pub type Audible = Arc<dyn Fn(&str) -> bool + Send + Sync>;

pub struct AudioRoutes {
    pub routes: Mutex<HashMap<u64, Route>>,
    pub audible: Mutex<Option<Audible>>,
    pub alive: Duration,
}

impl Default for AudioRoutes {
    fn default() -> Self {
        AudioRoutes::new()
    }
}

impl AudioRoutes {
    pub fn new() -> AudioRoutes {
        AudioRoutes::with_alive(ROUTE_ALIVE)
    }

    pub fn with_alive(alive: Duration) -> AudioRoutes {
        AudioRoutes { routes: Mutex::new(HashMap::new()), audible: Mutex::new(None), alive }
    }

    /// Say whose sound this machine plays (the person signed in at the screen). Until it is set nobody is sent anything and the machine's
    /// speakers are never silenced for a client.
    pub fn set_audible(&self, f: impl Fn(&str) -> bool + Send + Sync + 'static) {
        *self.audible.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(f));
    }

    pub fn is_audible(&self, person: &str) -> bool {
        self.audible.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|f| f(person))
    }

    /// Start (or change) the route of a session. A session has one route and one side; a second `open` changes the side.
    pub fn open(&self, session: u64, person: &str, side: Side) -> Opened {
        let mut r = self.routes.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(x) = r.get_mut(&session) {
            x.last_pull = Instant::now();
            if x.side == side {
                return Opened::Same;
            }
            x.side = side;
            x.queue.clear();
            x.chunker.clear();
            x.flush = true;
            return Opened::Switched;
        }
        r.insert(session, Route { person: person.to_string(), side, chunker: Chunker::new(2), queue: JitterQueue::bounded(), seq: 0, last_pull: Instant::now(), flush: false });
        Opened::New
    }

    /// The session ends or stops the sound: the route goes and the machine's speakers come back.
    pub fn close(&self, session: u64) -> bool {
        self.routes.lock().unwrap_or_else(|e| e.into_inner()).remove(&session).is_some()
    }

    pub fn side_of(&self, session: u64) -> Option<Side> {
        self.routes.lock().unwrap_or_else(|e| e.into_inner()).get(&session).map(|r| r.side)
    }

    /// The sound stopped or paused (the mixer closed its output): drop everything queued, in the instance and, by the flag, at the client.
    pub fn flush_all(&self) {
        for r in self.routes.lock().unwrap_or_else(|e| e.into_inner()).values_mut() {
            r.queue.clear();
            r.chunker.clear();
            r.flush = true;
        }
    }

    /// One mixed period from the mixer (the tee calls this). Returns true when some route took it.
    pub fn tap(&self, rate: u32, channels: u8, pcm: &[i16]) -> bool {
        if rate != m::AUDIO_RATE || !(1..=2).contains(&channels) {
            return false;
        }
        let stereo: std::borrow::Cow<'_, [i16]> = if channels == 1 { pcm.iter().flat_map(|s| [*s, *s]).collect::<Vec<_>>().into() } else { pcm.into() };
        let now = Instant::now();
        let mut took = false;
        let mut r = self.routes.lock().unwrap_or_else(|e| e.into_inner());
        for route in r.values_mut() {
            if now.duration_since(route.last_pull) > self.alive || !self.is_audible(&route.person) {
                continue;
            }
            for c in route.chunker.push(&stereo) {
                route.queue.push(m::encode_audio(route.seq, &c));
                route.seq = route.seq.wrapping_add(1);
            }
            took = true;
        }
        took
    }

    /// What is waiting for the session (and a mark that it is still there).
    pub fn pull(&self, session: u64) -> Pulled {
        let mut r = self.routes.lock().unwrap_or_else(|e| e.into_inner());
        let Some(route) = r.get_mut(&session) else { return Pulled::default() };
        route.last_pull = Instant::now();
        let flush = std::mem::take(&mut route.flush);
        let mut chunks = Vec::new();
        while let Some(c) = route.queue.pop() {
            chunks.push(c);
        }
        Pulled { flush, chunks }
    }

    pub fn live(&self, route: &Route) -> bool {
        route.last_pull.elapsed() <= self.alive && self.is_audible(&route.person)
    }

    /// Must the machine's own speakers stay silent: some live route plays on the client only.
    pub fn machine_muted(&self) -> bool {
        self.routes.lock().unwrap_or_else(|e| e.into_inner()).values().any(|r| r.side == Side::Client && self.live(r))
    }

    /// Does some live route want the sound (so the tee offers it).
    pub fn listening(&self) -> bool {
        self.routes.lock().unwrap_or_else(|e| e.into_inner()).values().any(|r| self.live(r))
    }

    pub fn sessions(&self) -> usize {
        self.routes.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Chunks thrown away by the 100 ms bound since the session began (for tests and the status line).
    pub fn dropped(&self, session: u64) -> u64 {
        self.routes.lock().unwrap_or_else(|e| e.into_inner()).get(&session).map_or(0, |r| r.queue.dropped())
    }
}

/// The five things a client device can be allowed to ask for, as Settings and the chat tool name them: (what the person reads, the capability
/// the broker checks, one line of what it means). "On" means allowed to ask: the hard asks (the clipboard, the device's camera and microphone)
/// still put a card on the machine at every use.
pub const DEVICE_PERMISSIONS: &[(&str, &str, &str)] = &[
    ("audio.out", "speaker.play", "play the machine's sound on the device"),
    ("clipboard.client", "clipboard.client", "send text from the device to the machine's clipboard (asked every use)"),
    ("camera.view", "camera.view", "watch the machine's cameras that the person may view"),
    ("client.camera.offer", "client.camera.offer", "share the device's camera with the machine (asked every use)"),
    ("client.microphone.offer", "client.microphone.offer", "share the device's microphone with the machine (asked every use)"),
];

/// What the shell's red band shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Indicator {
    /// Names of client cameras that are offered and streaming to this machine.
    pub cameras: Vec<String>,
    pub microphones: Vec<String>,
}

impl Indicator {
    pub fn active(&self) -> bool {
        !self.cameras.is_empty() || !self.microphones.is_empty()
    }

    /// For the band: "Maya's laptop camera and Maya's laptop microphone are in use by this machine".
    pub fn text(&self) -> Option<String> {
        if !self.active() {
            return None;
        }
        let all: Vec<&str> = self.cameras.iter().chain(self.microphones.iter()).map(String::as_str).collect();
        Some(format!("{} {} in use by this machine", all.join(" and "), if all.len() == 1 { "is" } else { "are" }))
    }
}

/// The broker capability behind a permission name (`audio.out` is `speaker.play`; the others are the same).
pub fn permission_capability(name: &str) -> Option<&'static str> {
    DEVICE_PERMISSIONS.iter().find(|(n, c, _)| *n == name || *c == name).map(|(_, c, _)| *c)
}

/// The assistant's rule for the client-device tools.
pub const DEVICE_PROMPT_RULE: &str = "\nCLIENT DEVICES (experimental): client_devices lists the browsers, windows and phones enrolled to use this desktop and what each may ask for (audio.out = the machine's sound on the device, clipboard.client, camera.view = the machine's cameras, client.camera.offer and client.microphone.offer = the device's own camera and microphone shown to the machine). client_device_permission switches one of those on or off for one device, and client_device_revoke revokes a device; both ask the person first, and only an administrator may. 'On' only means the device may ask: the clipboard and the device's camera and microphone still ask on the machine at every use. You cannot make or show a one-time link; the person does that in Settings.";

/// Instance cameras as JPEG frames (the hub in the product, a fake in tests).
pub trait CameraFeed: Send + Sync {
    /// The cameras this person may watch: (id, name to show). Names, never device paths. Default: none.
    fn list(&self, _person: &str) -> Vec<(String, String)> {
        Vec::new()
    }
    fn label(&self, person: &str, unit: &str) -> String;
    fn state(&self, person: &str, unit: &str) -> CamState;
    /// The newest frame with a sequence number above `after`, or none yet.
    fn jpeg(&self, person: &str, unit: &str, after: u64) -> Result<Option<(u64, Vec<u8>)>, String>;
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----


/// What an instance camera is doing, for a viewer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CamState {
    Ready,
    /// Another program holds it; the name of the holder.
    Busy(String),
    Missing,
    Refused(String),
}
