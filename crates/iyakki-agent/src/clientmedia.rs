//! The media half of the client endpoint, on the instance's side (docs/development/CLIENT-SPLIT-DESIGN.md sections 6 and 7b). EXPERIMENTAL.
//!
//! Devices exist on two sides and never mix. This module holds the rules; the WebSocket that carries them is `iyakki-remote::clientsession`.
//!
//! * [`AudioRoutes`]: the person's sound goes to a client session instead of (or beside) the machine's speakers. One route per session,
//!   one side per route ([`Side`]); the mixer's output reaches it through the tee ([`crate::mixer::tee`]); it holds at most 100 ms, is
//!   flushed when the sound stops or pauses, and a session that ends (or stops pulling) gives the machine's speakers back.
//! * [`ClientConsent`]: a `CardAsk` for the broker. The press of the client's own button is the per-use consent for the clipboard; every
//!   other hard ask goes to the card of the machine (or is refused with `no_card` when there is none).
//! * [`Leases`]: who holds an instance device. A camera may be shared for viewing; an exclusive device has one holder; the person at the
//!   machine wins over a remote client.
//! * [`VirtualSources`]: the client's own camera and microphone as sources of their own ("Maya's laptop camera"), session scoped, per
//!   tenant, with the visible indicator the shell's red band calls ([`indicator`]).
//! * [`CameraFeed`]: instance cameras as JPEG frames for a client (the hub fans one capture out; see [`HubFeed`]).
//!
//! Not proven: any real phone or browser, real speakers, a real USB camera. The mixer is one per machine and has no per-person sources, so
//! the sound goes to a session only while [`AudioRoutes::set_audible`] says that session's person is the one the machine's sound belongs to
//! (nothing is sent until the shell sets it: the default fails closed).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use iyakki_core_api::broker::{CardAnswer, CardAsk, Caller, CallerKind};
use iyakki_core_api::client::media::{self as m, Chunker, JitterQueue, Side, SourceKind};

pub use iyakki_core_api::client::media;

// ------------------------------------------------------------------------------------------------ sound

/// A route that has not been pulled from for this long belongs to a hung connection: it stops silencing the machine.
pub const ROUTE_ALIVE: Duration = Duration::from_secs(4);

struct Route {
    person: String,
    side: Side,
    chunker: Chunker,
    queue: JitterQueue<Vec<u8>>,
    seq: u32,
    last_pull: Instant,
    /// The sound stopped or paused since the session last pulled: tell the client to drop what it holds.
    flush: bool,
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

type Audible = Arc<dyn Fn(&str) -> bool + Send + Sync>;

pub struct AudioRoutes {
    routes: Mutex<HashMap<u64, Route>>,
    audible: Mutex<Option<Audible>>,
    alive: Duration,
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

    fn is_audible(&self, person: &str) -> bool {
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

    fn live(&self, route: &Route) -> bool {
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

// ------------------------------------------------------------------------------------------------ consent

/// Capabilities for which the press of the client's own button counts as the per-use answer. Nothing else is ever answered on the client's
/// say-so: a camera or microphone offer is always asked on the machine.
pub const CLIENT_CONSENT_CAPS: &[&str] = &["clipboard.client"];

/// The card the broker of the client endpoint uses.
pub struct ClientConsent {
    inner: Option<Arc<dyn CardAsk>>,
    noted: Mutex<Vec<(String, String)>>,
}

impl ClientConsent {
    /// `inner` is the card of the machine (the shell's), or none when there is no screen to ask on.
    pub fn new(inner: Option<Arc<dyn CardAsk>>) -> ClientConsent {
        ClientConsent { inner, noted: Mutex::new(Vec::new()) }
    }

    /// The person pressed the button for this one use. Only the capabilities in [`CLIENT_CONSENT_CAPS`] count; others are ignored.
    pub fn note(&self, device_id: &str, capability: &str) {
        if CLIENT_CONSENT_CAPS.contains(&capability) {
            let mut n = self.noted.lock().unwrap_or_else(|e| e.into_inner());
            n.push((device_id.to_string(), capability.to_string()));
            if n.len() > 32 {
                n.remove(0);
            }
        }
    }

    /// The press was not used (the call was refused before the card): forget it so it cannot cover a later call.
    pub fn forget(&self, device_id: &str, capability: &str) {
        self.noted.lock().unwrap_or_else(|e| e.into_inner()).retain(|(d, c)| !(d == device_id && c == capability));
    }
}

impl CardAsk for ClientConsent {
    fn ask(&self, caller: &Caller, capability: &str, label: &str) -> CardAnswer {
        if let CallerKind::Client { device_id } = &caller.kind {
            let mut n = self.noted.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(i) = n.iter().position(|(d, c)| d == device_id && c == capability) {
                n.remove(i);
                return CardAnswer::Allow;
            }
        }
        match &self.inner {
            Some(c) => c.ask(caller, capability, label),
            None => CardAnswer::Unavailable,
        }
    }
}

// ------------------------------------------------------------------------------------------------ leases

/// One holder of an instance device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    pub session: u64,
    /// For messages: "the person at the machine", "Maya's laptop".
    pub who: String,
    pub exclusive: bool,
    /// The person at the machine (or an app on it), not a remote client.
    pub local: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Taken {
    /// Granted; these holders were pushed out by a local holder and must be told.
    Granted { preempted: Vec<Lease> },
    /// Refused: the name of the holder.
    InUse(String),
}

/// One lease table for the devices of the instance (a camera id, a microphone id).
#[derive(Default)]
pub struct Leases {
    m: Mutex<HashMap<String, Vec<Lease>>>,
}

static LOCAL_SESSIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1 << 62);

/// A session number for a local holder (the person at the machine), distinct from every client session.
pub fn local_session() -> u64 {
    LOCAL_SESSIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl Leases {
    pub fn new() -> Leases {
        Leases::default()
    }

    /// Ask for `device`. A camera is taken shared for viewing (`exclusive: false`); a microphone or anything exclusive has one holder. The
    /// local holder wins: a local request pushes remote holders out; a remote request never pushes a local one out.
    pub fn acquire(&self, device: &str, want: Lease) -> Taken {
        let mut t = self.m.lock().unwrap_or_else(|e| e.into_inner());
        let held = t.entry(device.to_string()).or_default();
        held.retain(|h| h.session != want.session);
        let conflicts: Vec<usize> = held.iter().enumerate().filter(|(_, h)| h.exclusive || want.exclusive).map(|(i, _)| i).collect();
        if !conflicts.is_empty() {
            if !want.local {
                return Taken::InUse(held[conflicts[0]].who.clone());
            }
            if let Some(i) = conflicts.iter().find(|i| held[**i].local) {
                return Taken::InUse(held[*i].who.clone());
            }
        }
        let mut preempted = Vec::new();
        let mut keep = Vec::new();
        for (i, h) in held.drain(..).enumerate() {
            if conflicts.contains(&i) {
                preempted.push(h);
            } else {
                keep.push(h);
            }
        }
        *held = keep;
        held.push(want);
        Taken::Granted { preempted }
    }

    /// Who blocks a request for this device: `None` when a shared request would be granted now.
    pub fn exclusive_holder(&self, device: &str) -> Option<String> {
        self.m.lock().unwrap_or_else(|e| e.into_inner()).get(device).and_then(|v| v.iter().find(|h| h.exclusive).map(|h| h.who.clone()))
    }

    pub fn holders(&self, device: &str) -> Vec<Lease> {
        self.m.lock().unwrap_or_else(|e| e.into_inner()).get(device).cloned().unwrap_or_default()
    }

    pub fn holds(&self, device: &str, session: u64) -> bool {
        self.m.lock().unwrap_or_else(|e| e.into_inner()).get(device).is_some_and(|v| v.iter().any(|h| h.session == session))
    }

    pub fn release(&self, device: &str, session: u64) {
        let mut t = self.m.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = t.get_mut(device) {
            v.retain(|h| h.session != session);
            if v.is_empty() {
                t.remove(device);
            }
        }
    }

    /// Every lease of a session goes (it disconnected).
    pub fn release_session(&self, session: u64) {
        let mut t = self.m.lock().unwrap_or_else(|e| e.into_inner());
        t.retain(|_, v| {
            v.retain(|h| h.session != session);
            !v.is_empty()
        });
    }
}

// ------------------------------------------------------------------------------------------------ the client's devices as sources

/// A camera or microphone of a connected client, offered to the instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub kind: SourceKind,
    pub device_id: String,
    pub device_name: String,
    pub person: String,
    pub tenant: String,
    pub session: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum OfferErr {
    /// This device already offers one of that kind (from another session).
    Already(String),
    TooMany,
}

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

/// How many client devices may offer at once.
pub const MAX_SOURCES: usize = 16;
/// A source that has sent nothing for this long is not shown as streaming.
pub const STREAM_FRESH: Duration = Duration::from_secs(5);

struct Entry {
    src: Source,
    frames: u64,
    dropped: u64,
    last: Option<Instant>,
}

/// What became of a frame or chunk from a client.
#[derive(Debug, PartialEq, Eq)]
pub enum Accepted {
    /// Taken and, because nothing on the instance consumes client sources yet, dropped.
    Dropped,
    /// Refused: this kind is not on offer from this session, or the data is malformed or too big.
    Refused(&'static str),
}

#[derive(Default)]
pub struct VirtualSources {
    m: Mutex<Vec<Entry>>,
}

impl VirtualSources {
    pub fn new() -> VirtualSources {
        VirtualSources::default()
    }

    /// Register the client's camera or microphone under a name made from the device name. The name is unique inside the tenant.
    pub fn offer(&self, kind: SourceKind, session: u64, device_id: &str, device_name: &str, person: &str, tenant: &str) -> Result<Source, OfferErr> {
        let mut v = self.m.lock().unwrap_or_else(|e| e.into_inner());
        if v.iter().any(|e| e.src.device_id == device_id && e.src.kind == kind && e.src.session != session) {
            return Err(OfferErr::Already(m::source_name(device_name, kind)));
        }
        v.retain(|e| !(e.src.session == session && e.src.kind == kind));
        if v.len() >= MAX_SOURCES {
            return Err(OfferErr::TooMany);
        }
        let base = m::source_name(device_name, kind);
        let mut name = base.clone();
        let mut n = 2;
        while v.iter().any(|e| e.src.tenant == tenant && e.src.name == name) {
            name = format!("{base} ({n})");
            n += 1;
        }
        let src = Source { id: format!("client/{device_id}/{}", kind.word()), name, kind, device_id: device_id.to_string(), device_name: device_name.to_string(), person: person.to_string(), tenant: tenant.to_string(), session };
        v.push(Entry { src: src.clone(), frames: 0, dropped: 0, last: None });
        Ok(src)
    }

    pub fn close(&self, kind: SourceKind, session: u64) -> Option<Source> {
        let mut v = self.m.lock().unwrap_or_else(|e| e.into_inner());
        let i = v.iter().position(|e| e.src.session == session && e.src.kind == kind)?;
        Some(v.remove(i).src)
    }

    /// The session ended: everything it offered vanishes.
    pub fn vanish(&self, session: u64) -> Vec<Source> {
        let mut v = self.m.lock().unwrap_or_else(|e| e.into_inner());
        let (gone, keep): (Vec<Entry>, Vec<Entry>) = v.drain(..).partition(|e| e.src.session == session);
        *v = keep;
        gone.into_iter().map(|e| e.src).collect()
    }

    /// The sources that tenant may see; another tenant's are never listed.
    pub fn list(&self, tenant: &str) -> Vec<Source> {
        self.m.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|e| e.src.tenant == tenant).map(|e| e.src.clone()).collect()
    }

    pub fn offered(&self, kind: SourceKind, session: u64) -> bool {
        self.m.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|e| e.src.session == session && e.src.kind == kind)
    }

    /// A chunk of microphone sound or one camera JPEG arrives from the client. There is no consumer on the instance yet: it is checked and dropped.
    pub fn accept(&self, session: u64, kind: SourceKind, data: &[u8]) -> Accepted {
        let max = match kind {
            SourceKind::Camera => m::MAX_CAMERA_FRAME,
            SourceKind::Microphone => m::MAX_MIC_CHUNK,
        };
        let mut v = self.m.lock().unwrap_or_else(|e| e.into_inner());
        let Some(e) = v.iter_mut().find(|e| e.src.session == session && e.src.kind == kind) else { return Accepted::Refused("not_open") };
        if data.len() > max {
            return Accepted::Refused("too_big");
        }
        if (kind == SourceKind::Camera && !m::looks_like_jpeg(data)) || (kind == SourceKind::Microphone && (data.is_empty() || data.len() % 2 != 0)) {
            return Accepted::Refused("bad_data");
        }
        e.frames += 1;
        e.dropped += 1;
        e.last = Some(Instant::now());
        Accepted::Dropped
    }

    /// Frames taken and dropped from a source, for tests and the status line.
    pub fn counts(&self, session: u64, kind: SourceKind) -> Option<(u64, u64)> {
        self.m.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|e| e.src.session == session && e.src.kind == kind).map(|e| (e.frames, e.dropped))
    }

    /// Offered and sending lately (the red band shows these while they send, and from the moment of the offer until the first frame).
    pub fn indicator(&self) -> Indicator {
        let v = self.m.lock().unwrap_or_else(|e| e.into_inner());
        let mut i = Indicator::default();
        for e in v.iter() {
            let fresh = e.last.is_none_or(|t| t.elapsed() <= STREAM_FRESH);
            if !fresh {
                continue;
            }
            match e.src.kind {
                SourceKind::Camera => i.cameras.push(e.src.name.clone()),
                SourceKind::Microphone => i.microphones.push(e.src.name.clone()),
            }
        }
        i
    }
}

// ------------------------------------------------------------------------------------------------ instance cameras as frames

/// What an instance camera is doing, for a viewer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CamState {
    Ready,
    /// Another program holds it; the name of the holder.
    Busy(String),
    Missing,
    Refused(String),
}

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

/// The hub as a feed. `who` makes the hub's `Who` for a person (their home, their right to use this computer's cameras).
pub struct HubFeed {
    pub hub: Arc<crate::cameras::Hub>,
    pub who: Arc<dyn Fn(&str) -> crate::cameras::Who + Send + Sync>,
}

impl CameraFeed for HubFeed {
    fn list(&self, person: &str) -> Vec<(String, String)> {
        use crate::cameras::Kind;
        let w = (self.who)(person);
        self.hub.cameras(&w).into_iter().filter(|v| v.kind == Kind::Usb || v.owner == person || v.allowed.iter().any(|a| a == person)).map(|v| (v.id, v.name)).collect()
    }

    fn label(&self, person: &str, unit: &str) -> String {
        let w = (self.who)(person);
        self.hub.cameras(&w).into_iter().find(|v| v.id == unit).map(|v| v.name).unwrap_or_else(|| unit.to_string())
    }

    fn state(&self, person: &str, unit: &str) -> CamState {
        use crate::cameras::Status;
        let w = (self.who)(person);
        match self.hub.cameras(&w).into_iter().find(|v| v.id == unit) {
            None => CamState::Missing,
            Some(v) => match v.status {
                Status::Busy(h) => CamState::Busy(h),
                Status::NeedsAddon(s) | Status::NeedsPassword(s) => CamState::Refused(s),
                _ => CamState::Ready,
            },
        }
    }

    fn jpeg(&self, person: &str, unit: &str, after: u64) -> Result<Option<(u64, Vec<u8>)>, String> {
        self.hub.view_jpeg(&(self.who)(person), unit, after)
    }
}

// ------------------------------------------------------------------------------------------------ the bundle a session uses

/// Everything the media half needs, shared by all sessions of an endpoint.
pub struct MediaHost {
    pub audio: Arc<AudioRoutes>,
    pub leases: Arc<Leases>,
    pub sources: Arc<VirtualSources>,
    pub cameras: Option<Arc<dyn CameraFeed>>,
    pub consent: Arc<ClientConsent>,
    tenant_of: Arc<dyn Fn(&str) -> String + Send + Sync>,
}

impl MediaHost {
    /// Parts of its own (tests, or a second endpoint).
    pub fn new(consent: Arc<ClientConsent>) -> MediaHost {
        MediaHost { audio: Arc::new(AudioRoutes::new()), leases: Arc::new(Leases::new()), sources: Arc::new(VirtualSources::new()), cameras: None, consent, tenant_of: Arc::new(|_| String::new()) }
    }

    /// The process-wide parts: the audio routes the mixer's tee feeds and the sources the shell's indicator reads. The consent card has
    /// the machine's card behind it, if there is one.
    pub fn shared(card: Option<Arc<dyn CardAsk>>) -> MediaHost {
        MediaHost { audio: audio_routes(), leases: leases(), sources: sources(), cameras: None, consent: Arc::new(ClientConsent::new(card)), tenant_of: Arc::new(|_| String::new()) }
    }

    pub fn with_cameras(mut self, feed: Arc<dyn CameraFeed>) -> MediaHost {
        self.cameras = Some(feed);
        self
    }

    /// Which tenant a person belongs to (cloud: the namespace). Default: all one tenant.
    pub fn with_tenant(mut self, f: impl Fn(&str) -> String + Send + Sync + 'static) -> MediaHost {
        self.tenant_of = Arc::new(f);
        self
    }

    pub fn tenant(&self, person: &str) -> String {
        (self.tenant_of)(person)
    }

    /// The card to give the broker of the endpoint.
    pub fn card(&self) -> Arc<dyn CardAsk> {
        self.consent.clone()
    }
}

// ------------------------------------------------------------------------------------------------ granting a device

/// What a person can give a client device (Settings, Devices; the chat tool). Each stays a hard ask where the catalogue says so: the grant
/// only lets the question be asked.
pub const GRANTABLE: &[&str] = &["speaker.play", "clipboard.client", "camera.view", "client.camera.offer", "client.microphone.offer"];

/// Give a client device these capabilities (kept in the approval book under `client/<device id>`; earlier grants stay). The caller writes
/// the audit line. Names outside [`GRANTABLE`] are refused.
pub fn grant_device(data: &std::path::Path, device_id: &str, by: &str, caps: &[&str], now: i64) -> Result<(), String> {
    use iyakki_core_api::appcaps::{record_approval, ApprovalBook, CapabilityRequest};
    if let Some(bad) = caps.iter().find(|c| !GRANTABLE.contains(*c)) {
        return Err(format!("{bad} is not something a client device can be given"));
    }
    let key = iyakki_core_api::broker::client_key(device_id);
    let store = crate::store::Store::open(data);
    let mut book = crate::store::approvals::FileBook(&store);
    let reqs: Vec<CapabilityRequest> = caps.iter().map(|c| CapabilityRequest { name: (*c).into(), why: "granted to this client device".into(), param: None, extra: Default::default() }).collect();
    let mut a = book.get(&key).unwrap_or_else(|| record_approval(by, "client", "1", "", now, &[]));
    for r in record_approval(by, "client", "1", "", now, &reqs).grants {
        a.revoked.retain(|g| g.name != r.name);
        if !a.grants.contains(&r) {
            a.grants.push(r);
        }
    }
    book.put(&key, a);
    Ok(())
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

/// The broker capability behind a permission name (`audio.out` is `speaker.play`; the others are the same).
pub fn permission_capability(name: &str) -> Option<&'static str> {
    DEVICE_PERMISSIONS.iter().find(|(n, c, _)| *n == name || *c == name).map(|(_, c, _)| *c)
}

/// Which of the [`DEVICE_PERMISSIONS`] this device holds now (granted and not revoked), by capability.
pub fn device_holds(data: &std::path::Path, device_id: &str) -> Vec<String> {
    use iyakki_core_api::appcaps::ApprovalBook;
    let store = crate::store::Store::open(data);
    let book = crate::store::approvals::FileBook(&store);
    let Some(a) = book.get(&iyakki_core_api::broker::client_key(device_id)) else { return Vec::new() };
    DEVICE_PERMISSIONS.iter().filter(|(_, c, _)| a.holds(c, None)).map(|(_, c, _)| c.to_string()).collect()
}

/// Switch one permission of a device on or off (`name` is one of the [`DEVICE_PERMISSIONS`] names). Returns the capability changed. The caller
/// writes the audit line. Effective at the next broker call.
pub fn set_permission(data: &std::path::Path, device_id: &str, name: &str, on: bool, by: &str, now: i64) -> Result<&'static str, String> {
    let cap = permission_capability(name).ok_or_else(|| format!("{name} is not something a client device can be given; the choices are {}", DEVICE_PERMISSIONS.iter().map(|p| p.0).collect::<Vec<_>>().join(", ")))?;
    if on {
        grant_device(data, device_id, by, &[cap], now)?;
    } else {
        revoke_device(data, device_id, &[cap])?;
    }
    Ok(cap)
}

/// Take these capabilities away from a device (the broker refuses them from its next call). Returns how many changed.
pub fn revoke_device(data: &std::path::Path, device_id: &str, caps: &[&str]) -> Result<usize, String> {
    use iyakki_core_api::appcaps::ApprovalBook;
    if let Some(bad) = caps.iter().find(|c| !GRANTABLE.contains(*c)) {
        return Err(format!("{bad} is not something a client device can be given"));
    }
    let key = iyakki_core_api::broker::client_key(device_id);
    let store = crate::store::Store::open(data);
    let mut book = crate::store::approvals::FileBook(&store);
    let Some(mut a) = book.get(&key) else { return Ok(0) };
    let n = caps.iter().filter(|c| a.revoke(c)).count();
    if n > 0 {
        book.put(&key, a);
    }
    Ok(n)
}

static AUDIO: OnceLock<Arc<AudioRoutes>> = OnceLock::new();
static LEASES: OnceLock<Arc<Leases>> = OnceLock::new();
static SOURCES: OnceLock<Arc<VirtualSources>> = OnceLock::new();

/// The routes the mixer's tee feeds.
pub fn audio_routes() -> Arc<AudioRoutes> {
    AUDIO.get_or_init(|| Arc::new(AudioRoutes::new())).clone()
}

pub fn leases() -> Arc<Leases> {
    LEASES.get_or_init(|| Arc::new(Leases::new())).clone()
}

pub fn sources() -> Arc<VirtualSources> {
    SOURCES.get_or_init(|| Arc::new(VirtualSources::new())).clone()
}

/// For the shell's red band: which client cameras and microphones this machine is using right now.
pub fn indicator() -> Indicator {
    sources().indicator()
}

pub mod devicetools;
mod card;
pub use card::{card_text, DeviceCard, CARD_WAIT};

#[cfg(test)]
mod perm_tests;
#[cfg(test)]
mod tests;
