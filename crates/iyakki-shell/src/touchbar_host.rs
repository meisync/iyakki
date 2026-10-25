//! The Touch Bar's background side: finds the display, opens it, sends the pictures the shell makes and reads the finger.
//!
//! Everything blocking happens on two threads of their own (`iyakki-touchbar` for the display, `iyakki-touchbar-in` for touches). The shell only
//! swaps a picture into a mailbox, takes touch events out of a queue and reads a status; it never waits for the device. A watchdog
//! (the worker's heartbeat) marks the bar *stuck* when the worker stops answering, and the shell then stops sending. Any failure
//! ends in the plain function-key mode: the shell hands the keys back to the firmware ([`Host::status`] is not `Ready`).
//!
//! The device side is behind [`Backend`], so the tests drive the whole worker with a fake device that times out.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

use iyakki_drivers::touchbar::{self as tb, Dfr, DfrError, DisplayIface, Found, Orient, Port, SwitchOutcome, Usb, UsbError};

use crate::touchbar::{Picture, TouchEv};

/// How often the device is looked for: quickly while the machine is new (the bar shows up a few seconds after boot), then rarely.
pub const SEARCH_FAST_MS: u64 = 5_000;
pub const SEARCH_FAST_FOR_MS: u64 = 5 * 60_000;
pub const SEARCH_SLOW_MS: u64 = 60_000;
/// Waits after a failed open: grows, never ends in a tight loop.
pub const OPEN_BACKOFF_MS: [u64; 5] = [2_000, 5_000, 15_000, 30_000, 60_000];
/// Failed opens in a row after which the device is left alone for a while.
pub const MAX_OPEN_FAILS: u32 = 6;
pub const FAILED_RETRY_MS: u64 = 5 * 60_000;
/// Worker poll while drawing.
pub const POLL_MS: u64 = 20;
/// The whole picture is sent again this often when nothing changed: the firmware may draw over the strip (a key row after a mode
/// write, a wake from sleep) and a damage-only update would never repair it.
pub const REFRESH_MS: u64 = 2_000;
/// The worker has not answered for this long: the bar is stuck.
pub const WATCHDOG_MS: u64 = 8_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// No Touch Bar display on this machine (yet).
    Absent,
    /// Found but not usable yet (not configured, no display interface): waiting.
    Waiting(String),
    /// Drawing: the picture size.
    Ready { w: u32, h: u32 },
    /// Gave up for now (the reason); the firmware keys stay.
    Failed(String),
    /// The worker stopped answering.
    Stuck,
}

impl Status {
    pub fn is_ready(&self) -> bool {
        matches!(self, Status::Ready { .. })
    }

    /// For the logs and Settings.
    pub fn text(&self) -> String {
        match self {
            Status::Absent => "no Touch Bar display found".into(),
            Status::Waiting(s) => format!("waiting: {s}"),
            Status::Ready { w, h } => format!("drawing {w} x {h}"),
            Status::Failed(s) => format!("not used: {s}"),
            Status::Stuck => "not answering (left alone)".into(),
        }
    }
}

pub struct Shared {
    status: Mutex<Status>,
    picture: Mutex<Option<Picture>>,
    touches: Mutex<VecDeque<TouchEv>>,
    started: std::time::Instant,
    beat: AtomicU64,
    stop: AtomicBool,
    flip: AtomicU8,
    /// Pictures sent to the device (for the status line).
    sent: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Shared {
    fn new(flip: u8) -> Arc<Shared> {
        Arc::new(Shared { status: Mutex::new(Status::Absent), picture: Mutex::new(None), touches: Mutex::new(VecDeque::new()), started: std::time::Instant::now(), beat: AtomicU64::new(0), stop: AtomicBool::new(false), flip: AtomicU8::new(flip), sent: AtomicU64::new(0) })
    }
    fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }
    fn set(&self, s: Status) {
        *lock(&self.status) = s;
    }
}

/// The shell's handle.
#[derive(Clone)]
pub struct Host {
    shared: Arc<Shared>,
}

impl Host {
    /// Start the two threads (the real device).
    pub fn start(flip: u8) -> Host {
        let h = Host::detached(flip);
        let (s1, s2) = (h.shared.clone(), h.shared.clone());
        let _ = std::thread::Builder::new().name("iyakki-touchbar".into()).spawn(move || {
            let mut w = Worker::new(s1, Box::new(RealBackend::default()));
            while !w.shared.stop.load(Relaxed) {
                let ms = w.tick();
                sleep_stoppable(&w.shared, ms);
            }
            w.shutdown();
        });
        let _ = std::thread::Builder::new().name("iyakki-touchbar-in".into()).spawn(move || touch_loop(s2));
        h
    }

    /// A handle with no threads: the tests push touches and run a [`Worker`] by hand.
    pub fn detached(flip: u8) -> Host {
        Host { shared: Shared::new(flip) }
    }

    /// The status, with the watchdog applied.
    pub fn status(&self) -> Status {
        self.status_at(self.shared.now_ms())
    }

    fn status_at(&self, now_ms: u64) -> Status {
        let s = lock(&self.shared.status).clone();
        if s.is_ready() && now_ms.saturating_sub(self.shared.beat.load(Relaxed)) > WATCHDOG_MS {
            return Status::Stuck;
        }
        s
    }

    pub fn size(&self) -> Option<(u32, u32)> {
        match self.status() {
            Status::Ready { w, h } => Some((w, h)),
            _ => None,
        }
    }

    /// Hand over the newest picture (an older one still waiting is dropped). Does nothing unless the bar is ready.
    pub fn submit(&self, p: Picture) {
        if self.status().is_ready() {
            *lock(&self.shared.picture) = Some(p);
        }
    }

    pub fn take_touches(&self) -> Vec<TouchEv> {
        lock(&self.shared.touches).drain(..).collect()
    }

    pub fn push_touch(&self, ev: TouchEv) {
        let mut q = lock(&self.shared.touches);
        // a finger that moves faster than the shell reads is thinned: only the newest move is kept
        if matches!(ev, TouchEv::Move(_)) && matches!(q.back(), Some(TouchEv::Move(_))) {
            q.pop_back();
        }
        if q.len() < 64 {
            q.push_back(ev);
        }
    }

    pub fn set_flip(&self, bits: u8) {
        self.shared.flip.store(bits & 3, Relaxed);
    }

    pub fn flip(&self) -> u8 {
        self.shared.flip.load(Relaxed)
    }

    pub fn pictures_sent(&self) -> u64 {
        self.shared.sent.load(Relaxed)
    }

    pub fn stop(&self) {
        self.shared.stop.store(true, Relaxed);
    }

    /// Tests: the device is there and drawing.
    #[cfg(test)]
    pub(crate) fn ready_for_test(&self, w: u32, h: u32) {
        self.shared.beat.store(self.shared.now_ms(), Relaxed);
        self.shared.set(Status::Ready { w, h });
    }

    /// Tests: the display went away.
    #[cfg(test)]
    pub(crate) fn stop_for_test(&self) {
        self.shared.set(Status::Failed("test".into()));
    }

    /// Tests: the newest picture waiting for the device, if any.
    #[cfg(test)]
    pub(crate) fn take_picture_for_test(&self) -> Option<Picture> {
        lock(&self.shared.picture).take()
    }

    #[cfg(test)]
    pub(crate) fn shared(&self) -> Arc<Shared> {
        self.shared.clone()
    }
}

fn sleep_stoppable(s: &Shared, ms: u64) {
    let mut left = ms;
    while left > 0 && !s.stop.load(Relaxed) {
        let step = left.min(250);
        std::thread::sleep(std::time::Duration::from_millis(step));
        left -= step;
    }
}

// ------------------------------------------------------------------------------------------------ the device seam

pub trait Backend: Send {
    fn find(&mut self) -> Vec<Found>;
    fn display(&mut self, f: &Found) -> Option<DisplayIface>;
    fn open(&mut self, f: &Found) -> Result<Box<dyn Usb + Send>, UsbError>;
    /// The device as a whole, for the one configuration switch.
    fn port(&mut self, f: &Found) -> Box<dyn Port + Send>;
    /// The switch is allowed once per boot: true until [`Backend::note_switch`] was called.
    fn may_switch(&mut self) -> bool;
    fn note_switch(&mut self);
}

/// Configuration switches made by this process (the budget is one per boot, whatever the number of workers).
static SWITCHES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub struct RealBackend {
    pub usb_devices: std::path::PathBuf,
    pub dev_root: std::path::PathBuf,
}

impl Default for RealBackend {
    fn default() -> RealBackend {
        RealBackend { usb_devices: "/sys/bus/usb/devices".into(), dev_root: "/dev".into() }
    }
}

impl Backend for RealBackend {
    fn find(&mut self) -> Vec<Found> {
        tb::find_devices(&self.usb_devices)
    }
    fn display(&mut self, f: &Found) -> Option<DisplayIface> {
        tb::display_of(f)
    }
    fn open(&mut self, f: &Found) -> Result<Box<dyn Usb + Send>, UsbError> {
        #[cfg(target_os = "linux")]
        {
            Ok(Box::new(iyakki_drivers::usbfs::UsbFs::open(&f.usbfs_path(&self.dev_root))?))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = f;
            Err(UsbError::NoDevice)
        }
    }
    fn port(&mut self, f: &Found) -> Box<dyn Port + Send> {
        #[cfg(target_os = "linux")]
        {
            Box::new(iyakki_drivers::usbfs::SysPort::new(f.clone(), self.dev_root.clone()))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = f;
            Box::new(NoPort)
        }
    }
    fn may_switch(&mut self) -> bool {
        SWITCHES.load(Relaxed) == 0
    }
    fn note_switch(&mut self) {
        SWITCHES.fetch_add(1, Relaxed);
    }
}

#[cfg(not(target_os = "linux"))]
struct NoPort;

#[cfg(not(target_os = "linux"))]
impl Port for NoPort {
    fn present(&mut self) -> bool {
        false
    }
    fn configured(&mut self) -> Option<u8> {
        None
    }
    fn set_configuration(&mut self, _: u8) -> Result<(), UsbError> {
        Err(UsbError::NoDevice)
    }
    fn reset(&mut self) -> Result<(), UsbError> {
        Err(UsbError::NoDevice)
    }
    fn sleep_ms(&mut self, _: u64) {}
}

// ------------------------------------------------------------------------------------------------ the worker

pub struct Worker {
    pub shared: Arc<Shared>,
    backend: Box<dyn Backend>,
    dfr: Option<Dfr<Box<dyn Usb + Send>>>,
    last: Option<Picture>,
    /// When the last picture went out (ms on the shared clock).
    last_sent_ms: u64,
    fails: u32,
    /// Time of the first look (for the fast search window).
    first_ms: Option<u64>,
    /// The device that failed for good: left alone until it is a different one (replugged) or the retry time comes.
    gave_up_on: Option<(u32, u32)>,
    /// The configuration the device had before the one switch to the display's (for the one way back).
    switched_from: Option<u8>,
    fell_back: bool,
}

impl Worker {
    pub fn new(shared: Arc<Shared>, backend: Box<dyn Backend>) -> Worker {
        Worker { shared, backend, dfr: None, last: None, last_sent_ms: 0, fails: 0, first_ms: None, gave_up_on: None, switched_from: None, fell_back: false }
    }

    /// One step; returns how long to wait before the next.
    pub fn tick(&mut self) -> u64 {
        let now = self.shared.now_ms();
        self.shared.beat.store(now, Relaxed);
        if self.dfr.is_some() {
            return self.draw();
        }
        self.search(now)
    }

    fn search_wait(&self, now: u64) -> u64 {
        if now.saturating_sub(self.first_ms.unwrap_or(now)) < SEARCH_FAST_FOR_MS {
            SEARCH_FAST_MS
        } else {
            SEARCH_SLOW_MS
        }
    }

    fn search(&mut self, now: u64) -> u64 {
        self.first_ms.get_or_insert(now);
        let found = self.backend.find();
        let Some(f) = found.into_iter().next() else {
            self.shared.set(Status::Absent);
            self.fails = 0;
            self.gave_up_on = None;
            return self.search_wait(now);
        };
        if f.configured.is_none() {
            // the boot-time -110: the bring-up (iyakki-core) deals with it; nothing is written from here
            self.shared.set(Status::Waiting("the device has no USB configuration yet".into()));
            return self.search_wait(now);
        }
        let Some(iface) = self.backend.display(&f) else {
            self.shared.set(Status::Failed("no USB configuration of the device has a display interface".into()));
            return FAILED_RETRY_MS;
        };
        let active = f.configured.unwrap_or(iface.config);
        if iface.config != active {
            return self.switch(&f, iface, active);
        }
        if self.gave_up_on == Some((f.bus, f.dev)) && self.fails >= MAX_OPEN_FAILS {
            // left alone until the retry; the next look is a fresh round of attempts
            self.fails = 0;
        }
        let opened = self.backend.open(&f).map_err(DfrError::from).and_then(|u| Dfr::open(u, iface));
        match opened {
            Ok(d) => {
                let (w, h) = d.info.landscape();
                self.fails = 0;
                self.gave_up_on = None;
                self.dfr = Some(d);
                self.last = None;
                self.shared.set(Status::Ready { w, h });
                log::info!("Touch Bar: drawing {w} x {h}");
                POLL_MS
            }
            Err(e) => {
                self.fails += 1;
                let why = e.to_string();
                if self.fails >= MAX_OPEN_FAILS {
                    self.gave_up_on = Some((f.bus, f.dev));
                    self.give_back(&f);
                    self.shared.set(Status::Failed(why.clone()));
                    log::info!("Touch Bar: display not usable ({why}); the keys on the bar stay as the firmware has them");
                    return FAILED_RETRY_MS;
                }
                self.shared.set(Status::Waiting(why));
                OPEN_BACKOFF_MS.get(self.fails as usize - 1).copied().unwrap_or(60_000)
            }
        }
    }

    /// The display interface is in another configuration than the active one (the Touch Bar powers up with the keys only). One switch
    /// per boot, with one way back; the next look then finds the device in the new configuration and opens it.
    fn switch(&mut self, f: &Found, iface: DisplayIface, active: u8) -> u64 {
        if !self.backend.may_switch() {
            self.shared.set(Status::Failed(format!("the display is in USB configuration {}, the device is in {active}, and the one switch allowed this boot is used", iface.config)));
            return FAILED_RETRY_MS;
        }
        self.backend.note_switch();
        self.shared.set(Status::Waiting(format!("switching the USB configuration {active} -> {} (once)", iface.config)));
        let mut port = self.backend.port(f);
        let r = tb::switch_to_display(&mut *port, iface.config, active);
        log::info!("Touch Bar: configuration {active} -> {}: {r:?}", iface.config);
        match r {
            SwitchOutcome::Switched | SwitchOutcome::Already => {
                self.switched_from = Some(active);
                POLL_MS
            }
            SwitchOutcome::FellBack => {
                self.fell_back = true;
                self.shared.set(Status::Failed(format!("switching to USB configuration {} failed; the device is back in {active}", iface.config)));
                FAILED_RETRY_MS
            }
            SwitchOutcome::Failed => {
                self.shared.set(Status::Failed(format!("switching to USB configuration {} failed and the device did not go back; restart to try again", iface.config)));
                FAILED_RETRY_MS
            }
            SwitchOutcome::Absent => {
                self.shared.set(Status::Absent);
                self.search_wait(self.shared.now_ms())
            }
        }
    }

    /// The display would not open after the switch: put the device back in the configuration it had (once).
    fn give_back(&mut self, f: &Found) {
        let Some(from) = self.switched_from.take().filter(|_| !self.fell_back) else { return };
        self.fell_back = true;
        let mut port = self.backend.port(f);
        let r = port.switch_configuration(from);
        port.sleep_ms(1_500);
        log::info!("Touch Bar: back to USB configuration {from}: {r:?}");
    }

    fn draw(&mut self) -> u64 {
        let fresh = lock(&self.shared.picture).take();
        let now = self.shared.now_ms();
        let repeat = fresh.is_none() && now.saturating_sub(self.last_sent_ms) >= REFRESH_MS;
        let Some(pic) = fresh.or_else(|| if repeat { self.last.clone() } else { None }) else { return POLL_MS };
        let orient = Orient::from_bits(self.shared.flip.load(Relaxed));
        let Some(dfr) = self.dfr.as_mut() else { return POLL_MS };
        // a repeat is the whole picture; a new picture only what changed
        let whole = tb::Rect { x: 0, y: 0, w: pic.w, h: pic.h };
        let Some(r) = (if repeat { Some(whole) } else { pic.damage(self.last.as_ref()) }) else { return POLL_MS };
        let data = tb::pack_rect(&pic.rgba, pic.w, pic.h, tb::Rect { x: r.x, y: r.y, w: r.w, h: r.h }, orient);
        let now_ns = self.shared.started.elapsed().as_nanos() as u64;
        match dfr.update(&[(r, data)], now_ns) {
            Ok(()) => {
                self.last = Some(pic);
                self.last_sent_ms = now;
                self.shared.sent.fetch_add(1, Relaxed);
            }
            Err(DfrError::Dead) => {
                log::info!("Touch Bar: the display stopped answering; back to the firmware keys");
                self.dfr = None;
                self.last = None;
                self.fails = 1;
                self.shared.set(Status::Failed("the display stopped answering".into()));
                return OPEN_BACKOFF_MS[1];
            }
            Err(_) => {
                // a lost frame: draw everything next time
                self.last = None;
            }
        }
        POLL_MS
    }

    pub fn shutdown(&mut self) {
        if let Some(d) = self.dfr.take() {
            d.close();
        }
        self.shared.set(Status::Absent);
    }
}

// ------------------------------------------------------------------------------------------------ touches

fn touch_loop(s: Arc<Shared>) {
    use crate::touchbar::RawTouch;
    let mut nodes: Vec<(crate::input::BarNode, RawTouch)> = Vec::new();
    let mut next_scan = 0u64;
    while !s.stop.load(Relaxed) {
        let now = s.now_ms();
        if now >= next_scan {
            // looked at rarely and only while a display is there: a machine without a Touch Bar costs one status read every few seconds
            next_scan = now + if nodes.is_empty() { 3_000 } else { 30_000 };
            if lock(&s.status).is_ready() {
                nodes.retain(|(n, _)| std::path::Path::new(&n.path).exists());
                let (w, _) = match *lock(&s.status) {
                    Status::Ready { w, h } => (w, h),
                    _ => (2170, 60),
                };
                for p in crate::input::event_nodes() {
                    if nodes.iter().any(|(n, _)| n.path == p) {
                        continue;
                    }
                    if let Some(n) = crate::input::open_touchbar_node(&p) {
                        let mirror = s.flip.load(Relaxed) & 1 != 0;
                        let rt = RawTouch::new(n.min, n.max, w, mirror, n.axis);
                        log::info!("Touch Bar: touch panel {}", n.path);
                        nodes.push((n, rt));
                    }
                }
            } else {
                nodes.clear();
            }
        }
        if nodes.is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(500));
            continue;
        }
        let mut fds: Vec<libc::pollfd> = nodes.iter().map(|(n, _)| libc::pollfd { fd: n.fd(), events: libc::POLLIN, revents: 0 }).collect();
        // SAFETY: `fds` is a valid array of pollfd for the duration of the call.
        unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 250) };
        let mut dead = Vec::new();
        for (k, pf) in fds.iter().enumerate() {
            if pf.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                dead.push(k);
                continue;
            }
            if pf.revents & libc::POLLIN == 0 {
                continue;
            }
            let Some((n, rt)) = nodes.get_mut(k) else { continue };
            match crate::input::read_touchbar_node(n) {
                Ok(raws) => {
                    for r in raws {
                        for ev in rt.feed(r.typ, r.code, r.value) {
                            let mut q = lock(&s.touches);
                            if matches!(ev, TouchEv::Move(_)) && matches!(q.back(), Some(TouchEv::Move(_))) {
                                q.pop_back();
                            }
                            if q.len() < 64 {
                                q.push_back(ev);
                            }
                        }
                    }
                }
                Err(()) => dead.push(k),
            }
        }
        for k in dead.into_iter().rev() {
            nodes.remove(k);
        }
    }
}

#[cfg(test)]
pub(crate) mod fakes {
    use super::*;
    use iyakki_drivers::touchbar::fake::FakeUsb;

    /// A fake whose log the test can read after the worker owns it.
    #[derive(Clone, Default)]
    pub struct Dev(pub Arc<Mutex<FakeUsb>>);

    impl Usb for Dev {
        fn claim(&mut self, i: u8) -> Result<(), UsbError> {
            lock(&self.0).claim(i)
        }
        fn release(&mut self, i: u8) {
            lock(&self.0).release(i)
        }
        fn bulk_out(&mut self, ep: u8, d: &[u8], t: u32) -> Result<usize, UsbError> {
            lock(&self.0).bulk_out(ep, d, t)
        }
        fn bulk_in(&mut self, ep: u8, b: &mut [u8], t: u32) -> Result<usize, UsbError> {
            lock(&self.0).bulk_in(ep, b, t)
        }
        fn clear_halt(&mut self, ep: u8) -> Result<(), UsbError> {
            lock(&self.0).clear_halt(ep)
        }
    }

    pub struct FakeBackend {
        pub found: Vec<Found>,
        pub iface: Option<DisplayIface>,
        pub dev: Dev,
        pub opens: u32,
        pub open_error: Option<UsbError>,
        /// The device for the switch: starts in `found[0].configured`; the fake backend moves `found` along with it.
        pub port: Option<std::sync::Arc<Mutex<iyakki_drivers::touchbar::fake::FakeSwitchPort>>>,
        pub switches: u32,
    }

    /// A port that is the shared fake device.
    pub struct SharedPort(pub std::sync::Arc<Mutex<iyakki_drivers::touchbar::fake::FakeSwitchPort>>);

    impl Port for SharedPort {
        fn present(&mut self) -> bool {
            lock(&self.0).present()
        }
        fn configured(&mut self) -> Option<u8> {
            lock(&self.0).configured()
        }
        fn set_configuration(&mut self, v: u8) -> Result<(), UsbError> {
            lock(&self.0).set_configuration(v)
        }
        fn reset(&mut self) -> Result<(), UsbError> {
            lock(&self.0).reset()
        }
        fn sleep_ms(&mut self, ms: u64) {
            lock(&self.0).sleep_ms(ms)
        }
        fn switch_configuration(&mut self, v: u8) -> Result<(), UsbError> {
            lock(&self.0).switch_configuration(v)
        }
    }

    impl Backend for FakeBackend {
        fn find(&mut self) -> Vec<Found> {
            let mut v = self.found.clone();
            if let (Some(p), Some(f)) = (&self.port, v.first_mut()) {
                f.configured = lock(p).config;
            }
            v
        }
        fn display(&mut self, _f: &Found) -> Option<DisplayIface> {
            self.iface
        }
        fn port(&mut self, _f: &Found) -> Box<dyn Port + Send> {
            Box::new(SharedPort(self.port.clone().unwrap_or_default()))
        }
        fn may_switch(&mut self) -> bool {
            self.switches == 0
        }
        fn note_switch(&mut self) {
            self.switches += 1;
        }
        fn open(&mut self, _f: &Found) -> Result<Box<dyn Usb + Send>, UsbError> {
            self.opens += 1;
            match self.open_error {
                Some(e) => Err(e),
                None => Ok(Box::new(self.dev.clone())),
            }
        }
    }

    pub fn found(configured: Option<u8>) -> Found {
        Found { sysfs: "/sys/bus/usb/devices/5-6".into(), bus: 5, dev: 7, configured }
    }

    pub const IFACE: DisplayIface = DisplayIface { config: 1, interface: 2, alt: 0, ep_out: 2, ep_in: 0x82 };
}

#[cfg(test)]
mod tests {
    use super::fakes::*;
    use super::*;
    use iyakki_drivers::touchbar::fake::FakeUsb;

    fn rig(found_list: Vec<Found>, iface: Option<DisplayIface>) -> (Host, Worker, Dev) {
        let dev = Dev::default();
        let host = Host::detached(0);
        let w = Worker::new(host.shared(), Box::new(FakeBackend { found: found_list, iface, dev: dev.clone(), opens: 0, open_error: None, port: None, switches: 0 }));
        (host, w, dev)
    }

    fn pic(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Picture {
        let mut p = Picture::black(w, h);
        for y in 0..h {
            for x in 0..w {
                let at = (y as usize * w as usize + x as usize) * 4;
                p.rgba[at..at + 3].copy_from_slice(&f(x, y));
            }
        }
        p
    }

    fn tags(d: &Dev) -> Vec<u32> {
        lock(&d.0).sent.iter().map(|m| u32::from_le_bytes([m[16], m[17], m[18], m[19]])).collect()
    }

    #[test]
    fn a_configured_display_is_opened_and_pictures_reach_it_once_each() {
        let (host, mut w, dev) = rig(vec![found(Some(1))], Some(IFACE));
        assert_eq!(host.status(), Status::Absent);
        host.submit(Picture::black(2170, 60));
        assert!(host.shared.picture.lock().unwrap().is_none(), "not ready: nothing is kept");
        w.tick();
        assert_eq!(host.status(), Status::Ready { w: 2170, h: 60 });
        assert_eq!(host.size(), Some((2170, 60)));
        assert_eq!(tags(&dev), [tb::MSG_INFORMATION, tb::MSG_READY, tb::MSG_CLEAR]);
        host.submit(pic(2170, 60, |x, _| [x as u8, 0, 0]));
        w.tick();
        assert_eq!(host.pictures_sent(), 1);
        assert_eq!(lock(&dev.0).sent.len(), 4);
        // the same picture again: nothing changed, nothing sent
        host.submit(pic(2170, 60, |x, _| [x as u8, 0, 0]));
        w.tick();
        assert_eq!(lock(&dev.0).sent.len(), 4);
        // one changed pixel: a small rectangle, not the whole bar
        host.submit(pic(2170, 60, |x, y| if (x, y) == (100, 5) { [9, 9, 9] } else { [x as u8, 0, 0] }));
        w.tick();
        let last = lock(&dev.0).sent.last().cloned().unwrap();
        assert!(last.len() < 200, "{} bytes for one pixel", last.len());
        w.shutdown();
        assert_eq!(host.status(), Status::Absent);
        assert_eq!(lock(&dev.0).released, [2]);
    }

    #[test]
    fn the_wire_order_follows_the_flip_setting() {
        let p = |flip: u8| {
            let (host, mut w, dev) = rig(vec![found(Some(1))], Some(IFACE));
            host.set_flip(flip);
            w.tick();
            host.submit(pic(2170, 60, |_, y| [y as u8, 0, 0]));
            w.tick();
            let m = lock(&dev.0).sent.last().cloned().unwrap();
            m[48 + 12..48 + 12 + 6].to_vec()
        };
        // column 0, bottom row (y = 59) first: red 59 is the third byte of a BGR pixel
        assert_eq!(p(0), [0, 0, 59, 0, 0, 58]);
        assert_eq!(p(2), [0, 0, 0, 0, 0, 1], "flip across: top row first");
    }

    #[test]
    fn a_device_that_is_missing_unconfigured_or_without_a_display_is_never_opened() {
        let (host, mut w, _) = rig(vec![], Some(IFACE));
        assert_eq!(w.tick(), SEARCH_FAST_MS);
        assert_eq!(host.status(), Status::Absent);
        let dev = Dev::default();
        let host = Host::detached(0);
        let mut be = FakeBackend { found: vec![found(None)], iface: Some(IFACE), dev: dev.clone(), opens: 0, open_error: None, port: None, switches: 0 };
        let mut w = Worker::new(host.shared(), Box::new(FakeBackend { found: vec![found(None)], iface: Some(IFACE), dev: dev.clone(), opens: 0, open_error: None, port: None, switches: 0 }));
        assert_eq!(w.tick(), SEARCH_FAST_MS);
        assert!(matches!(host.status(), Status::Waiting(_)));
        assert!(lock(&dev.0).claimed.is_empty(), "no claim, no write: the boot bring-up owns an unconfigured device");
        be.found = vec![found(Some(1))];
        be.iface = None;
        let mut w2 = Worker::new(Host::detached(0).shared(), Box::new(be));
        assert_eq!(w2.tick(), FAILED_RETRY_MS);
        assert!(matches!(&*lock(&w2.shared.status), Status::Failed(m) if m.contains("has a display interface")));
        let _ = &mut w;
    }

    #[test]
    fn open_failures_back_off_and_end_in_a_long_wait_never_a_tight_loop() {
        let host = Host::detached(0);
        let mut w = Worker::new(host.shared(), Box::new(FakeBackend { found: vec![found(Some(1))], iface: Some(IFACE), dev: Dev::default(), opens: 0, open_error: Some(UsbError::Busy), port: None, switches: 0 }));
        let waits: Vec<u64> = (0..MAX_OPEN_FAILS).map(|_| w.tick()).collect();
        assert_eq!(waits, [2_000, 5_000, 15_000, 30_000, 60_000, FAILED_RETRY_MS]);
        assert!(matches!(host.status(), Status::Failed(_)));
        assert!(!host.status().is_ready());
        assert!(host.status().text().contains("not used"));
        // a start-up that times out half way is the same: released, counted
        let dev = Dev(std::sync::Arc::new(std::sync::Mutex::new(FakeUsb { timeouts: 1, ..Default::default() })));
        let host = Host::detached(0);
        let mut w = Worker::new(host.shared(), Box::new(FakeBackend { found: vec![found(Some(1))], iface: Some(IFACE), dev: dev.clone(), opens: 0, open_error: None, port: None, switches: 0 }));
        assert_eq!(w.tick(), 2_000);
        assert_eq!(lock(&dev.0).released, [2]);
        // and the next try works
        assert_eq!(w.tick(), POLL_MS);
        assert!(host.status().is_ready());
    }

    #[test]
    fn a_display_that_dies_hands_the_keys_back_and_is_found_again() {
        let (host, mut w, dev) = rig(vec![found(Some(1))], Some(IFACE));
        w.tick();
        lock(&dev.0).write_fail = Some((1, UsbError::NoDevice));
        host.submit(pic(2170, 60, |x, _| [x as u8, 1, 1]));
        assert_eq!(w.tick(), OPEN_BACKOFF_MS[1]);
        assert!(matches!(host.status(), Status::Failed(_)));
        assert!(!host.status().is_ready(), "the shell sees this and returns the keys to the firmware");
        host.submit(Picture::black(2170, 60));
        assert!(lock(&host.shared.picture).is_none(), "nothing is queued for a dead display");
        assert_eq!(w.tick(), POLL_MS, "the next look reopens it");
        assert!(host.status().is_ready());
    }

    #[test]
    fn a_worker_that_stops_answering_is_marked_stuck() {
        let (host, mut w, _) = rig(vec![found(Some(1))], Some(IFACE));
        w.tick();
        let beat = host.shared.beat.load(Relaxed);
        assert!(host.status_at(beat + WATCHDOG_MS).is_ready());
        assert_eq!(host.status_at(beat + WATCHDOG_MS + 1), Status::Stuck);
        assert!(Status::Stuck.text().contains("not answering"));
    }

    #[test]
    fn touches_queue_up_with_fast_moves_thinned() {
        let h = Host::detached(0);
        for ev in [TouchEv::Down(1), TouchEv::Move(2), TouchEv::Move(3), TouchEv::Move(4), TouchEv::Up(4)] {
            h.push_touch(ev);
        }
        assert_eq!(h.take_touches(), [TouchEv::Down(1), TouchEv::Move(4), TouchEv::Up(4)]);
        assert!(h.take_touches().is_empty());
        for k in 0..500 {
            h.push_touch(if k % 2 == 0 { TouchEv::Down(k) } else { TouchEv::Up(k) });
        }
        assert_eq!(h.take_touches().len(), 64, "a stalled shell cannot make the queue grow");
        h.set_flip(7);
        assert_eq!(h.flip(), 3);
    }

    use iyakki_drivers::touchbar::fake::FakeSwitchPort;

    fn switch_rig(active: u8, display_in: u8, port: FakeSwitchPort, open_error: Option<UsbError>) -> (Host, Worker, std::sync::Arc<Mutex<FakeSwitchPort>>) {
        let port = std::sync::Arc::new(Mutex::new(port));
        let host = Host::detached(0);
        let iface = DisplayIface { config: display_in, ..IFACE };
        let be = FakeBackend { found: vec![found(Some(active))], iface: Some(iface), dev: Dev::default(), opens: 0, open_error, port: Some(port.clone()), switches: 0 };
        (host.clone(), Worker::new(host.shared(), Box::new(be)), port)
    }

    #[test]
    fn a_display_in_the_other_configuration_is_switched_to_once_then_opened() {
        let (host, mut w, port) = switch_rig(1, 2, FakeSwitchPort::new(1), None);
        assert_eq!(w.tick(), POLL_MS);
        assert!(matches!(host.status(), Status::Waiting(m) if m.contains("1 -> 2")));
        assert_eq!(lock(&port).log, [2]);
        w.tick();
        assert!(host.status().is_ready(), "{:?}", host.status());
        assert_eq!(lock(&port).log, [2], "no second write");
        assert_eq!(lock(&port).config, Some(2));
    }

    #[test]
    fn a_device_already_in_the_display_configuration_is_not_switched() {
        let (host, mut w, port) = switch_rig(2, 2, FakeSwitchPort::new(2), None);
        w.tick();
        assert!(host.status().is_ready());
        assert!(lock(&port).log.is_empty());
    }

    #[test]
    fn when_no_configuration_has_the_display_the_strip_stays_off_and_says_so() {
        let host = Host::detached(0);
        let port = std::sync::Arc::new(Mutex::new(FakeSwitchPort::new(1)));
        let mut w = Worker::new(host.shared(), Box::new(FakeBackend { found: vec![found(Some(1))], iface: None, dev: Dev::default(), opens: 0, open_error: None, port: Some(port.clone()), switches: 0 }));
        assert_eq!(w.tick(), FAILED_RETRY_MS);
        assert!(matches!(host.status(), Status::Failed(m) if m.contains("no USB configuration of the device has a display interface")));
        assert!(lock(&port).log.is_empty(), "nothing written");
    }

    #[test]
    fn a_switch_that_times_out_falls_back_and_the_boot_makes_no_second_attempt() {
        let mut p = FakeSwitchPort::new(1);
        p.fail_to = Some(2);
        let (host, mut w, port) = switch_rig(1, 2, p, None);
        assert_eq!(w.tick(), FAILED_RETRY_MS);
        assert!(matches!(host.status(), Status::Failed(m) if m.contains("back in 1")), "{:?}", host.status());
        assert_eq!(lock(&port).log, [2]);
        for _ in 0..5 {
            w.tick();
        }
        assert_eq!(lock(&port).log, [2], "five more looks, no more writes");
        assert!(matches!(host.status(), Status::Failed(m) if m.contains("one switch allowed this boot is used")));
        // a second worker over the same backend budget is the same: the fake counts per backend, the real one per process
    }

    #[test]
    fn if_the_display_will_not_open_after_the_switch_the_device_goes_back_once() {
        let (host, mut w, port) = switch_rig(1, 2, FakeSwitchPort::new(1), Some(UsbError::Busy));
        let waits: Vec<u64> = (0..8).map(|_| w.tick()).collect();
        assert!(matches!(host.status(), Status::Failed(_)), "{waits:?}");
        assert_eq!(lock(&port).log, [2, 1], "one switch, one way back");
        for _ in 0..4 {
            w.tick();
        }
        assert_eq!(lock(&port).log, [2, 1], "and nothing more");
    }
}
