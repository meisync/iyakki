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

pub use iyakki_shared::clientmedia::{CamState};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use iyakki_core_api::broker::{CardAnswer, CardAsk, Caller, CallerKind};
use iyakki_core_api::client::media::{self as m, SourceKind};
pub use iyakki_shared::clientmedia::{Audible, AudioRoutes, DEVICE_PERMISSIONS, Opened, Pulled, ROUTE_ALIVE, Route};

pub use iyakki_core_api::client::media;

// ------------------------------------------------------------------------------------------------ sound









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

pub use iyakki_shared::clientmedia::Indicator;

/// How many client devices may offer at once.
pub const MAX_SOURCES: usize = 16;
/// A source that has sent nothing for this long is not shown as streaming.
pub const STREAM_FRESH: Duration = Duration::from_secs(5);

struct Entry {
    src: Source,
    frames: u64,
    dropped: u64,
    last: Option<Instant>,
    /// Programs on this machine that look at or listen to the source now (names, for the status line). While none is attached, everything is dropped.
    consumers: Vec<(u64, String)>,
    /// The newest camera picture (`seq`, JPEG), kept only while a consumer is attached.
    picture: Option<(u64, Vec<u8>)>,
    /// Microphone sound (16 kHz mono, 16 bit), at most [`MIC_KEEP`] bytes, oldest dropped; kept only while a consumer is attached.
    sound: std::collections::VecDeque<u8>,
}

/// One second of 16 kHz mono 16-bit sound: what a listener may fall behind by.
pub const MIC_KEEP: usize = 32_000;

/// What became of a frame or chunk from a client.
#[derive(Debug, PartialEq, Eq)]
pub enum Accepted {
    /// Taken and dropped, because no program on the machine is looking at or listening to this source.
    Dropped,
    /// Taken and kept for the program that is looking or listening ([`SourceTap`]).
    Delivered,
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
        v.push(Entry { src: src.clone(), frames: 0, dropped: 0, last: None, consumers: Vec::new(), picture: None, sound: Default::default() });
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
        e.last = Some(Instant::now());
        if e.consumers.is_empty() {
            e.dropped += 1;
            return Accepted::Dropped;
        }
        match kind {
            SourceKind::Camera => e.picture = Some((e.frames, data.to_vec())),
            SourceKind::Microphone => {
                e.sound.extend(data.iter().copied());
                let over = e.sound.len().saturating_sub(MIC_KEEP);
                e.sound.drain(..over);
            }
        }
        Accepted::Delivered
    }

    /// A program on this machine starts to look at (camera) or listen to (microphone) a client's source. The tenant must be the source's:
    /// another tenant's sources are never reachable. While at least one tap is open the source's frames are kept, newest only; with none
    /// they are dropped as before. The tap lets go when dropped (or when the client's session ends).
    pub fn attach(self: &Arc<Self>, source_id: &str, tenant: &str, consumer: &str) -> Result<SourceTap, String> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let mut v = self.m.lock().unwrap_or_else(|e| e.into_inner());
        let e = v.iter_mut().find(|e| e.src.id == source_id && e.src.tenant == tenant).ok_or_else(|| "that camera or microphone of a client is not on offer".to_string())?;
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        e.consumers.push((id, consumer.to_string()));
        Ok(SourceTap { table: self.clone(), source: e.src.clone(), id, seen: 0 })
    }

    /// The sources with the programs that use them now, for the chat tool and the status line.
    pub fn users(&self, tenant: &str) -> Vec<(Source, Vec<String>)> {
        self.m.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|e| e.src.tenant == tenant).map(|e| (e.src.clone(), e.consumers.iter().map(|c| c.1.clone()).collect())).collect()
    }

    fn detach(&self, source_id: &str, id: u64) {
        let mut v = self.m.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = v.iter_mut().find(|e| e.src.id == source_id) {
            e.consumers.retain(|c| c.0 != id);
            if e.consumers.is_empty() {
                e.picture = None;
                e.sound.clear();
            }
        }
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

/// A program on this machine looking at a client's camera or listening to its microphone (the first consumer of the virtual sources).
/// Dropping it lets go; the client's red band and the machine's sign keep telling the truth because the source stays on offer.
pub struct SourceTap {
    table: Arc<VirtualSources>,
    source: Source,
    id: u64,
    seen: u64,
}

impl SourceTap {
    pub fn source(&self) -> &Source {
        &self.source
    }

    /// The newest picture not yet returned (a camera source), as JPEG. `None` when nothing new arrived, or the source is gone.
    pub fn next_jpeg(&mut self) -> Option<Vec<u8>> {
        let v = self.table.m.lock().unwrap_or_else(|e| e.into_inner());
        let e = v.iter().find(|e| e.src.id == self.source.id)?;
        match &e.picture {
            Some((seq, jpeg)) if *seq > self.seen => {
                self.seen = *seq;
                Some(jpeg.clone())
            }
            _ => None,
        }
    }

    /// The sound that arrived since the last call (a microphone source), at most [`MIC_KEEP`] bytes, 16 kHz mono 16 bit.
    pub fn take_sound(&mut self) -> Vec<u8> {
        let mut v = self.table.m.lock().unwrap_or_else(|e| e.into_inner());
        match v.iter_mut().find(|e| e.src.id == self.source.id) {
            Some(e) => e.sound.drain(..).collect(),
            None => Vec::new(),
        }
    }

    /// Whether the client still offers this source.
    pub fn live(&self) -> bool {
        self.table.m.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|e| e.src.id == self.source.id)
    }
}

impl Drop for SourceTap {
    fn drop(&mut self) {
        self.table.detach(&self.source.id, self.id);
    }
}

/// A hold a program on this machine has on an instance device, taken through [`Leases::hold_local`]; the lease goes when it is dropped.
pub struct LocalHold {
    table: Arc<Leases>,
    device: String,
    session: u64,
    /// The remote holders this hold pushed out (they find out at their next check).
    pub preempted: Vec<Lease>,
}

impl Drop for LocalHold {
    fn drop(&mut self) {
        self.table.release(&self.device, self.session);
    }
}

impl Leases {
    /// A program on this machine (`who`: "the person at the machine", an app's name) takes `device`. It wins over remote clients: an
    /// exclusive hold pushes a client's shared look out as well, and the client is told at its next check. Two local holders still conflict.
    pub fn hold_local(self: &Arc<Self>, device: &str, who: &str, exclusive: bool) -> Result<LocalHold, String> {
        let session = local_session();
        match self.acquire(device, Lease { session, who: who.to_string(), exclusive, local: true }) {
            Taken::Granted { preempted } => Ok(LocalHold { table: self.clone(), device: device.to_string(), session, preempted }),
            Taken::InUse(by) => Err(format!("in use by {by}")),
        }
    }
}

// ------------------------------------------------------------------------------------------------ instance cameras as frames


pub use iyakki_shared::clientmedia::CameraFeed;

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


pub use iyakki_shared::clientmedia::permission_capability;

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

/// The role table of a client device (F-CORE-30): named presets over [`GRANTABLE`], so a person picks "listener" instead of five switches.
/// A role only says what the device MAY ASK for; a hard ask stays a card at every use. `viewer` (the default of a new device) may ask
/// for nothing: it sees the picture and sends keys, mouse and size, as before. The instance enforces it at the broker, never the client.
pub const DEVICE_ROLES: &[(&str, &str, &[&str])] = &[
    ("viewer", "sees the picture and sends keys and mouse; asks for nothing else", &[]),
    ("listener", "also hears this computer's sound", &["speaker.play"]),
    ("presenter", "also hears sound and shares its clipboard", &["speaker.play", "clipboard.client"]),
    ("full", "may also ask to view cameras here and to offer its own camera and microphone", &["speaker.play", "clipboard.client", "camera.view", "client.camera.offer", "client.microphone.offer"]),
];

/// The capabilities of a role, or `None` for a name that is not in the table.
pub fn role_caps(role: &str) -> Option<&'static [&'static str]> {
    DEVICE_ROLES.iter().find(|r| r.0 == role).map(|r| r.2)
}

/// Put a device on a role: grant what the role has, take away every other grantable capability. The caller writes the audit line.
pub fn apply_role(data: &std::path::Path, device_id: &str, role: &str, by: &str, now: i64) -> Result<(), String> {
    let caps = role_caps(role).ok_or_else(|| format!("{role} is not a role; the choices are {}", DEVICE_ROLES.iter().map(|r| r.0).collect::<Vec<_>>().join(", ")))?;
    if !caps.is_empty() {
        grant_device(data, device_id, by, caps, now)?;
    }
    let rest: Vec<&str> = GRANTABLE.iter().copied().filter(|c| !caps.contains(c)).collect();
    revoke_device(data, device_id, &rest)?;
    Ok(())
}

/// The role a device is on now: the one whose capabilities it holds exactly, else `custom` (switches set one by one).
pub fn role_of(data: &std::path::Path, device_id: &str) -> &'static str {
    let held = device_holds(data, device_id);
    DEVICE_ROLES.iter().find(|r| r.2.len() == held.len() && r.2.iter().all(|c| held.iter().any(|h| h == c))).map(|r| r.0).unwrap_or("custom")
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
    let mut i = sources().indicator();
    // the Gemini live voice session (EXPERIMENTAL) holds this machine's own microphone open: the same red sign, not a second one
    if crate::geminilive::mic_open() {
        i.microphones.push(crate::geminilive::INDICATOR_NAME.to_string());
    }
    i
}

pub mod devicetools;
mod card;
pub use card::{card_text, DeviceCard, CARD_WAIT};

#[cfg(test)]
mod perm_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
use iyakki_core_api::client::media::Side;
