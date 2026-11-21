//! The real machine under [`crate::avdtp`] and [`crate::a2dp`]: the L2CAP listeners (AVDTP on PSM 0x19, the service discovery
//! channel on PSM 1 for the one A2DP sink record), one thread per connected phone, and the hand-over of the decoded sound to an
//! output that the agent registers ([`set_output_factory`]: a mixer source named after the device, so the person's volume, mute
//! and ducking apply). Needs administrator rights like the rest of the Bluetooth code. Nothing here is tested against a radio: the
//! protocol it drives is (`avdtp_tests.rs`, `a2dp_tests.rs`, `sbc_tests.rs`).
//!
//! The first AVDTP connection of a paired device is the signalling channel; a second one from the same device is the media
//! channel. Only paired devices are served (the listener has the same security level as the keyboards' channels: encrypted and
//! authenticated); the service discovery channel is open to anyone, as the specification requires.
// SPDX-License-Identifier: Apache-2.0

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::a2dp::*;
use crate::avdtp::{Action, Reassembler, Sink, PSM_AVDTP};
use crate::bthid::{Channel, Recv, PSM_SDP};
use crate::bthid_sys::{hub, Listener, L2Sock};
use crate::btmgmt;

/// The name of the service in the record phones read.
pub const SERVICE_NAME: &str = "IYAKKI Audio";
/// How long "let a phone pair" stays open.
pub const PAIRING_SECS: u16 = 120;
const MAX_SDP_CONNECTIONS: usize = 8;

type Factory = Arc<dyn Fn() -> Box<dyn OutputFactory> + Send + Sync>;
static FACTORY: Mutex<Option<Factory>> = Mutex::new(None);

/// The agent registers how decoded sound reaches the speakers (once, at start). Without it the audio connection works but the sound
/// cannot be played, and the page says so.
pub fn set_output_factory(f: impl Fn() -> Box<dyn OutputFactory> + Send + Sync + 'static) {
    *FACTORY.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(f));
}

type RemoteHandler = Arc<dyn Fn(crate::avrcp::Act) + Send + Sync>;
static REMOTE: Mutex<Option<RemoteHandler>> = Mutex::new(None);

/// The agent registers what a volume key or a volume setting from the phone does (once, at start). Without it the phone's remote
/// control channel is still answered, but nothing is changed here.
pub fn set_remote_handler(f: impl Fn(crate::avrcp::Act) + Send + Sync + 'static) {
    *REMOTE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(f));
}

/// Test door: what a key from a phone would do (the same call the packet reader makes).
pub fn run_remote_for_test(a: crate::avrcp::Act) {
    do_remote(a);
}

/// Take the handler away (sign-out).
pub fn clear_remote_handler() {
    *REMOTE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn do_remote(a: crate::avrcp::Act) {
    let h = REMOTE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(h) = h {
        h(a);
    }
}

/// Answers the remote-control packets of one phone (AVCTP on PSM 0x17) until the channel closes or goes quiet for a minute.
fn serve_avrcp(mut sock: L2Sock) {
    let mut target = crate::avrcp::Target::default();
    let mut idle = 0;
    loop {
        match sock.recv(Duration::from_secs(2)) {
            Ok(Recv::Data(d)) => {
                idle = 0;
                // a bad packet is ignored, the channel stays
                if let Ok((reply, events)) = target.handle(&d) {
                    if let Some(r) = reply {
                        if sock.send(&r).is_err() {
                            return;
                        }
                    }
                    for e in &events {
                        if let Some(a) = crate::avrcp::act_for(e) {
                            do_remote(a);
                        }
                    }
                }
            }
            Ok(Recv::Timeout) => {
                idle += 1;
                if idle >= 30 {
                    return;
                }
            }
            _ => return,
        }
    }
}

struct NoOutput;
impl OutputFactory for NoOutput {
    fn open(&mut self, _: u32, _: usize, _: &str) -> Result<Box<dyn Output>, String> {
        Err("this program has no speaker set up for Bluetooth audio".into())
    }
}

fn make_factory() -> Box<dyn OutputFactory> {
    match FACTORY.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        Some(f) => f(),
        None => Box::new(NoOutput),
    }
}

#[derive(Default)]
struct Shared {
    playing: bool,
    codec: String,
    error: Option<String>,
    packets: u64,
    lost: u64,
}

struct Live {
    address: String,
    name: String,
    stop: Arc<AtomicBool>,
    shared: Arc<Mutex<Shared>>,
    media_tx: Sender<L2Sock>,
}

struct Inner {
    started: Option<Instant>,
    running: bool,
    why_not: String,
    live: Option<Live>,
    sdp_open: Arc<AtomicUsize>,
}

pub struct AudioHub {
    inner: Mutex<Inner>,
}

static HUB: OnceLock<Arc<AudioHub>> = OnceLock::new();

/// The one audio hub of this program.
pub fn audio_hub() -> Arc<AudioHub> {
    HUB.get_or_init(|| Arc::new(AudioHub { inner: Mutex::new(Inner { started: None, running: false, why_not: "not started".into(), live: None, sdp_open: Arc::new(AtomicUsize::new(0)) }) })).clone()
}

impl AudioHub {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Opens the listeners (and the Bluetooth hub with the paired devices) once; tries again every ten seconds while that fails.
    pub fn ensure_started(self: &Arc<Self>) {
        {
            let mut g = self.lock();
            if g.running || g.started.is_some_and(|t| t.elapsed() < Duration::from_secs(10)) {
                return;
            }
            g.started = Some(Instant::now());
        }
        hub().ensure_started();
        // the signalling channel needs a paired device; the service discovery channel is open to everyone
        let avdtp = Listener::open_with(PSM_AVDTP, 2, Some(1024));
        let sdp = Listener::open_with(PSM_SDP, 0, None);
        // the remote-control channel is optional: without it the sound still plays
        let avctp = Listener::open_with(crate::avrcp::PSM_AVCTP, 2, Some(512)).ok();
        match (avdtp, sdp) {
            (Ok(a), Ok(s)) => {
                let mut g = self.lock();
                g.running = true;
                g.why_not.clear();
                drop(g);
                let h = self.clone();
                std::thread::spawn(move || h.accept_loop(a, s, avctp));
            }
            (a, s) => {
                let e = a.err().or(s.err()).map(|e| e.to_string()).unwrap_or_default();
                self.lock().why_not = format!("the Bluetooth audio channels could not be opened ({e}); this needs administrator rights and a powered controller");
            }
        }
    }

    fn accept_loop(self: Arc<Self>, avdtp: Listener, sdp: Listener, avctp: Option<Listener>) {
        loop {
            let mut fds = vec![libc::pollfd { fd: avdtp.raw(), events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: sdp.raw(), events: libc::POLLIN, revents: 0 }];
            if let Some(l) = avctp.as_ref() {
                fds.push(libc::pollfd { fd: l.raw(), events: libc::POLLIN, revents: 0 });
            }
            // SAFETY: valid pollfds
            unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 1000) };
            while let Some((addr, sock)) = sdp.accept() {
                let open = self.lock().sdp_open.clone();
                if open.load(Ordering::SeqCst) >= MAX_SDP_CONNECTIONS {
                    continue;
                }
                open.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(move || {
                    serve_sdp(sock);
                    open.fetch_sub(1, Ordering::SeqCst);
                    let _ = addr;
                });
            }
            while let Some((addr, sock)) = avdtp.accept() {
                self.on_avdtp(&addr, sock);
            }
            if let Some(l) = avctp.as_ref() {
                while let Some((addr, sock)) = l.accept() {
                    // only devices the person paired
                    if hub().known(&addr).is_some() {
                        std::thread::spawn(move || serve_avrcp(sock));
                    }
                }
            }
        }
    }

    /// A connection on the AVDTP channel: the signalling channel of a new session, or the media channel of the running one.
    fn on_avdtp(self: &Arc<Self>, addr: &str, sock: L2Sock) {
        // only devices the person paired
        let Some(k) = hub().known(addr) else { return };
        let mut g = self.lock();
        if let Some(l) = g.live.as_ref() {
            if l.address.eq_ignore_ascii_case(addr) && !l.stop.load(Ordering::SeqCst) {
                let _ = l.media_tx.send(sock);
            }
            // one phone at a time: another device's connection is dropped
            return;
        }
        let name = if k.name.is_empty() { hub().seen_name(addr) } else { k.name.clone() };
        let name = if name.is_empty() { addr.to_string() } else { name };
        self.spawn_session(&mut g, addr, &name, sock);
    }

    fn spawn_session(self: &Arc<Self>, g: &mut Inner, addr: &str, name: &str, sig: L2Sock) {
        let (tx, rx) = channel::<L2Sock>();
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Mutex::new(Shared::default()));
        g.live = Some(Live { address: addr.to_ascii_uppercase(), name: name.to_string(), stop: stop.clone(), shared: shared.clone(), media_tx: tx });
        let (h, a, n) = (self.clone(), addr.to_ascii_uppercase(), name.to_string());
        std::thread::spawn(move || {
            run_session(sig, rx, &n, &stop, &shared);
            let mut g = h.lock();
            if g.live.as_ref().is_some_and(|l| l.address == a && Arc::ptr_eq(&l.stop, &stop)) {
                g.live = None;
            }
        });
    }

    pub fn view_inner(&self) -> AudioView {
        let g = self.lock();
        let device = g.live.as_ref().map(|l| {
            let s = l.shared.lock().unwrap_or_else(|e| e.into_inner());
            AudioDeviceView {
                address: l.address.clone(),
                name: l.name.clone(),
                state: if s.playing { AudioState::Playing } else { AudioState::Connected },
                codec: s.codec.clone(),
                error: s.error.clone(),
                packets: s.packets,
                lost: s.lost,
            }
        });
        AudioView { ready: g.running, why_not: g.why_not.clone(), pairing_open: hub().pairing_open(), device, aac_note: crate::a2dp_aac::note_line() }
    }
}

/// Answers service discovery requests on one connection until it closes or goes quiet.
fn serve_sdp(mut sock: L2Sock) {
    let mut idle = 0;
    loop {
        match sock.recv(Duration::from_secs(2)) {
            Ok(Recv::Data(d)) => {
                idle = 0;
                if sock.send(&sdp_respond(&d, SERVICE_NAME)).is_err() {
                    return;
                }
            }
            Ok(Recv::Timeout) => {
                idle += 1;
                if idle >= 10 {
                    return;
                }
            }
            _ => return,
        }
    }
}

/// One connected phone: signalling, the media channel when it opens, and the audio path, until the link ends or we are told to stop.
fn run_session(mut sig: L2Sock, media_rx: Receiver<L2Sock>, name: &str, stop: &AtomicBool, shared: &Mutex<Shared>) {
    // an AAC endpoint is offered only while the add-on that decodes it is installed (checked at each connection)
    let aac = crate::a2dp_aac::ready_backend();
    let mut sink = if aac.is_some() { Sink::new().with_aac() } else { Sink::new() };
    let mut re = Reassembler::default();
    let mut path = AudioPath::new(make_factory(), name);
    let mut media: Option<L2Sock> = None;
    let publish = |path: &AudioPath, shared: &Mutex<Shared>| {
        let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
        s.playing = path.playing();
        s.error = path.error.clone();
        s.packets = path.stats().packets;
        s.lost = path.stats().lost_packets;
        if let Some(p) = path.params() {
            s.codec = codec_text(&p);
        }
    };
    'outer: while !stop.load(Ordering::SeqCst) {
        if let Ok(m) = media_rx.try_recv() {
            media = Some(m);
        }
        let mut fds = vec![libc::pollfd { fd: sig.raw(), events: libc::POLLIN, revents: 0 }];
        if let Some(m) = media.as_ref() {
            fds.push(libc::pollfd { fd: m.raw(), events: libc::POLLIN, revents: 0 });
        }
        // SAFETY: valid pollfds for their count
        unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 250) };
        path.poll();
        publish(&path, shared);
        if fds[0].revents != 0 {
            // everything that is waiting
            loop {
                match sig.recv(Duration::ZERO) {
                    Ok(Recv::Data(d)) => {
                        let Ok(Some(msg)) = re.push(&d) else { continue };
                        let (reply, acts) = sink.handle(&msg);
                        if let Some(r) = reply {
                            if sig.send(&r.encode()).is_err() {
                                break 'outer;
                            }
                        }
                        for a in acts {
                            match a {
                                Action::ConfiguredAac(c) => {
                                    if let Some(b) = aac.clone() {
                                        path.set_aac(c, b);
                                    }
                                    shared.lock().unwrap_or_else(|e| e.into_inner()).codec = crate::a2dp_aac::codec_text(&c);
                                }
                                Action::Configured(p) => {
                                    path.set_sbc();
                                    shared.lock().unwrap_or_else(|e| e.into_inner()).codec = codec_text(&p);
                                }
                                Action::Started => path.start(),
                                Action::Suspended => path.stop(),
                                Action::Ended => {
                                    path.stop();
                                    media = None;
                                }
                                Action::Opened => {}
                            }
                        }
                        publish(&path, shared);
                    }
                    Ok(Recv::Timeout) => break,
                    _ => break 'outer,
                }
            }
        }
        if fds.len() > 1 && fds[1].revents != 0 {
            let mut closed = false;
            if let Some(m) = media.as_mut() {
                loop {
                    match m.recv(Duration::ZERO) {
                        Ok(Recv::Data(d)) => {
                            path.on_media(&d);
                            path.poll();
                        }
                        Ok(Recv::Timeout) => break,
                        _ => {
                            closed = true;
                            break;
                        }
                    }
                }
            }
            if closed {
                media = None;
                path.stop();
            }
            publish(&path, shared);
        }
    }
    for _ in sink.link_lost() {}
    path.stop();
    publish(&path, shared);
}

/// The real [`AudioControl`].
pub struct RealAudio;

impl AudioControl for RealAudio {
    fn view(&self) -> AudioView {
        let h = audio_hub();
        h.ensure_started();
        h.view_inner()
    }

    fn connect(&self, address: &str) -> Result<(), String> {
        let h = audio_hub();
        h.ensure_started();
        let Some(k) = hub().known(address) else { return Err("that device is not paired: pair it first (Settings -> Bluetooth)".into()) };
        let addr = btmgmt::parse_addr(address).ok_or("bad address")?;
        {
            let g = h.lock();
            if !g.running {
                return Err(g.why_not.clone());
            }
            if g.live.is_some() {
                return Err("a device is already connected for audio: disconnect it first".into());
            }
        }
        let name = if k.name.is_empty() { address.to_string() } else { k.name.clone() };
        let (h2, a) = (h.clone(), address.to_string());
        std::thread::spawn(move || match L2Sock::connect(addr, PSM_AVDTP, 12) {
            Ok(sock) => {
                let mut g = h2.lock();
                if g.live.is_none() {
                    h2.spawn_session(&mut g, &a, &name, sock);
                }
            }
            Err(e) => {
                h2.lock().why_not = format!("could not connect to the device for audio: {e}");
            }
        });
        Ok(())
    }

    fn disconnect(&self) -> Result<(), String> {
        let h = audio_hub();
        let g = h.lock();
        match g.live.as_ref() {
            Some(l) => {
                l.stop.store(true, Ordering::SeqCst);
                Ok(())
            }
            None => Err("no device is connected for audio".into()),
        }
    }

    fn pairing_open(&self, on: bool) -> Result<(), String> {
        let h = audio_hub();
        h.ensure_started();
        hub().set_pairing_open(on, PAIRING_SECS)
    }
}
