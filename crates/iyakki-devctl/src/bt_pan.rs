//! Use a paired phone's Bluetooth tethering as an internet source (PAN, the PANU role) without BlueZ and without the kernel's `bnep`
//! module: an L2CAP connection to the phone's BNEP port (the socket of [`crate::bthid_sys`], not a second copy), the BNEP set-up of
//! [`crate::bnep`], and a TAP interface ([`crate::BT_PAN_IFACE`]) the Ethernet frames are pumped to and from. The DHCP client of this
//! crate then asks the phone for an address on it, and the interface is an uplink like a cable.
//!
//! Layers, so the tests need no phone and no radio:
//! * [`run`] is the whole session (set-up exchange, then the pump) over the trait [`PanLink`]; the tests give it a fake phone;
//! * [`RealLink`] is the L2CAP socket and the TAP together (`poll` on both);
//! * [`RealTether`] is the manager the pages and the assistant use: one connection at a time, a state that says why it failed.
//!
//! The phone must be paired first (Settings -> Bluetooth) and have Bluetooth tethering switched on.
// SPDX-License-Identifier: Apache-2.0

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::bnep::{self, Frame, Mac, Panu, State};
use crate::bthid::HidError;
use crate::btview::Controller;

/// The TAP interface of the connection.
pub const TAP_NAME: &str = crate::BT_PAN_IFACE;

const TUNSETIFF: crate::oscompat::Ioctl = 0x4004_54ca;
const SIOCGIFHWADDR: crate::oscompat::Ioctl = 0x8927;
const IFF_TAP: i16 = 0x0002;
const IFF_NO_PI: i16 = 0x1000;
const CONNECT_SECS: u64 = 15;
/// How long the phone has to answer the BNEP set-up request.
pub const SETUP_SECS: u64 = 10;
/// Fixed program and file: the DHCP client of this link keeps its process number here.
const DHCP_PID: &str = "/run/udhcpc-iyakbt0.pid";

// ------------------------------------------------------------------------------------------------ the session

/// What the session sees of the world: the phone's L2CAP channel and the TAP, behind one wait.
pub trait PanLink {
    /// Waits up to `t` for a packet from the phone, a frame from the computer, or the end of the link.
    fn wait(&mut self, t: Duration) -> io::Result<LinkEv>;
    /// One BNEP packet to the phone.
    fn to_phone(&mut self, packet: &[u8]) -> io::Result<()>;
    /// One Ethernet frame (no packet-info header) to the TAP interface.
    fn to_tap(&mut self, frame: &[u8]) -> io::Result<()>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum LinkEv {
    Phone(Vec<u8>),
    Tap(Vec<u8>),
    Idle,
    /// The phone closed the channel (or it failed).
    Closed,
}

/// How a session ended.
#[derive(Debug, PartialEq, Eq)]
pub enum End {
    /// The person (or the program) asked for it.
    Stopped,
    /// The link was lost after it was up.
    Lost,
    /// The phone answered the set-up request with this code.
    Refused(u16),
    /// No answer to the set-up request in time, or the phone hung up first.
    NoAnswer,
}

/// An Ethernet frame as the TAP interface wants it (no packet-info header).
pub fn ethernet_bytes(f: &Frame) -> Vec<u8> {
    let mut v = Vec::with_capacity(14 + f.payload.len());
    v.extend_from_slice(&f.dst);
    v.extend_from_slice(&f.src);
    v.extend_from_slice(&f.proto.to_be_bytes());
    v.extend_from_slice(&f.payload);
    v
}

fn frame_of(f: &[u8]) -> Option<Frame> {
    if f.len() < 14 {
        return None;
    }
    Some(Frame { dst: f[0..6].try_into().ok()?, src: f[6..12].try_into().ok()?, proto: u16::from_be_bytes([f[12], f[13]]), payload: f[14..].to_vec() })
}

/// The whole session: asks the phone for its network service, waits `setup_wait` for the answer, calls `up` once the phone said yes
/// (the caller starts DHCP there), then carries frames both ways until `stop` is set or the link ends.
pub fn run<L: PanLink>(link: &mut L, local: Mac, phone: Mac, stop: &AtomicBool, setup_wait: Duration, up: &mut dyn FnMut()) -> End {
    let (mut panu, first) = Panu::start(local, phone, bnep::UUID_NAP);
    if link.to_phone(&first).is_err() {
        return End::NoAnswer;
    }
    let deadline = Instant::now() + setup_wait;
    while panu.state == State::Connecting {
        if stop.load(Ordering::SeqCst) {
            return End::Stopped;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return End::NoAnswer;
        }
        match link.wait(left.min(Duration::from_millis(250))) {
            Ok(LinkEv::Phone(d)) => {
                if let Ok(o) = panu.receive(&d) {
                    for r in &o.replies {
                        let _ = link.to_phone(r);
                    }
                }
            }
            Ok(LinkEv::Closed) | Err(_) => return End::NoAnswer,
            // frames the computer sends before the phone said yes are dropped (nothing can use them yet)
            Ok(LinkEv::Tap(_)) | Ok(LinkEv::Idle) => {}
        }
    }
    if let State::Refused(code) = panu.state {
        return End::Refused(code);
    }
    up();
    loop {
        if stop.load(Ordering::SeqCst) {
            return End::Stopped;
        }
        match link.wait(Duration::from_millis(500)) {
            Ok(LinkEv::Phone(d)) => {
                // a packet that makes no sense is dropped: a phone (or something near it) must not be able to end the link with one
                if let Ok(o) = panu.receive(&d) {
                    for r in &o.replies {
                        if link.to_phone(r).is_err() {
                            return End::Lost;
                        }
                    }
                    if let Some(f) = o.deliver {
                        let _ = link.to_tap(&ethernet_bytes(&f));
                    }
                }
            }
            Ok(LinkEv::Tap(f)) => {
                if let Some(p) = frame_of(&f).and_then(|fr| panu.send(&fr)) {
                    if link.to_phone(&p).is_err() {
                        return End::Lost;
                    }
                }
            }
            Ok(LinkEv::Idle) => {}
            Ok(LinkEv::Closed) | Err(_) => return End::Lost,
        }
    }
}

// ------------------------------------------------------------------------------------------------ plain reasons

/// Why a connect to the phone's network port failed, for the person (the errno the kernel gave).
pub fn connect_reason(errno: i32) -> String {
    match errno {
        e if e == libc::ECONNREFUSED => "The phone refused the network connection. Switch on Bluetooth tethering in the phone's settings (Hotspot and tethering), then press Connect again.".into(),
        e if e == libc::EACCES => "The phone does not trust this computer any more. Forget the phone here and forget this computer in the phone's Bluetooth settings, then pair again.".into(),
        e if e == libc::EHOSTUNREACH || e == libc::ETIMEDOUT || e == libc::EHOSTDOWN => "The phone did not answer. Unlock it, switch its Bluetooth on and bring it closer.".into(),
        e if e == libc::ECONNRESET || e == libc::ECONNABORTED => "The phone dropped the connection while connecting. Try again.".into(),
        e if e == libc::ENETDOWN || e == libc::ENODEV || e == crate::oscompat::EBADFD || e == libc::ENOPROTOOPT || e == libc::EAFNOSUPPORT => "Bluetooth is off or this computer has no Bluetooth controller. Power the controller on in Settings -> Bluetooth.".into(),
        e if e == libc::EPERM => "This needs administrator rights.".into(),
        e => format!("Could not connect to the phone ({}).", io::Error::from_raw_os_error(e)),
    }
}

/// What a BNEP answer other than "success" means for the person.
pub fn refusal_text(code: u16) -> String {
    match code {
        1 | 3 => "The phone does not offer Bluetooth tethering to computers. Check that tethering is switched on and that the phone supports PAN.".into(),
        4 => "The phone does not allow the connection. Switch Bluetooth tethering on in its settings, then press Connect again.".into(),
        c => format!("The phone refused the network service ({}, code {c}).", bnep::setup_text(c)),
    }
}

pub const NO_ANSWER: &str = "The phone did not answer the network request. Switch Bluetooth tethering on in its settings, then press Connect again.";

/// The reason tethering cannot be tried at all right now, from what the controllers and the radio switches say (`None`: nothing in the
/// way). Pure: the real machine is read by [`blocker_now`].
pub fn blocker(ctls: &Result<Vec<Controller>, String>, any_adapter: bool, switches: &[crate::Rfkill]) -> Option<String> {
    if switches.iter().any(|k| k.hard) {
        return Some("Bluetooth is switched off by a key or switch on the machine. Switch it on there first.".into());
    }
    if switches.iter().any(|k| k.soft) {
        return Some("The Bluetooth radio is switched off by the On/off row in Settings -> Bluetooth. Switch it on first.".into());
    }
    match ctls {
        Err(_) if !any_adapter => Some("This computer has no Bluetooth controller. A USB Bluetooth adapter works.".into()),
        Err(e) => Some(format!("The Bluetooth controller cannot be asked ({e}).")),
        Ok(c) if c.is_empty() && !any_adapter => Some("This computer has no Bluetooth controller. A USB Bluetooth adapter works.".into()),
        Ok(c) if c.is_empty() => Some("The Bluetooth controller is not set up yet. It may be waiting for a firmware file (Settings -> Bluetooth shows what is missing).".into()),
        Ok(c) => {
            let good: Vec<&Controller> = c.iter().filter(|x| x.configured && x.addr_state().valid()).collect();
            if good.is_empty() {
                return Some("The Bluetooth controller has no valid address yet: it is waiting for a firmware file that cannot be shipped (the Mac mini's own chip). A USB Bluetooth adapter works.".into());
            }
            if !good.iter().any(|x| x.powered()) {
                return Some("The Bluetooth controller is off. Press Power in Settings -> Bluetooth.".into());
            }
            if !good.iter().any(|x| x.supported & crate::btmgmt::SET_BREDR != 0) {
                return Some("This controller only does Bluetooth Low Energy. Phone tethering needs classic Bluetooth.".into());
            }
            None
        }
    }
}

/// [`blocker`] for the real machine.
pub fn blocker_now() -> Option<String> {
    let r = crate::Roots::real();
    let bt = crate::bluetooth(&r);
    let ctls = if bt.adapters.is_empty() { Ok(Vec::new()) } else { crate::btview::controllers() };
    blocker(&ctls, !bt.adapters.is_empty(), &bt.switches)
}

/// For the sharing page: `Err` is why a phone cannot be the uplink yet, in one line.
pub fn controller_state(r: &crate::Roots) -> Result<String, String> {
    let bt = crate::bluetooth(r);
    if bt.adapters.is_empty() {
        return Err("This computer has no Bluetooth controller.".into());
    }
    if let Some(k) = bt.switches.iter().find(|k| k.hard || k.soft) {
        return Err(if k.hard { "Bluetooth is switched off by a key or switch on the machine.".into() } else { "The Bluetooth radio is switched off (Settings -> Bluetooth, On/off).".into() });
    }
    Ok(bt.adapters[0].clone())
}

// ------------------------------------------------------------------------------------------------ the real link

/// A TAP interface (`/dev/net/tun`): frames in and out, no packet-info header.
struct Tap {
    fd: OwnedFd,
    mac: Mac,
}

impl Tap {
    fn open() -> Result<Tap, String> {
        // SAFETY: opening a device node by a constant path
        let fd = unsafe { libc::open(c"/dev/net/tun".as_ptr(), libc::O_RDWR | libc::O_CLOEXEC | libc::O_NONBLOCK) };
        if fd < 0 {
            return Err(format!("This system has no network tunnel device ({}).", io::Error::last_os_error()));
        }
        // SAFETY: fd is fresh and ours
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let mut req = [0u8; 40];
        req[..TAP_NAME.len()].copy_from_slice(TAP_NAME.as_bytes());
        req[16..18].copy_from_slice(&(IFF_TAP | IFF_NO_PI).to_ne_bytes());
        // SAFETY: req is the size of struct ifreq
        if unsafe { libc::ioctl(fd.as_raw_fd(), TUNSETIFF, req.as_mut_ptr()) } < 0 {
            return Err(format!("The network interface {TAP_NAME} could not be made ({}). Is a connection open already?", io::Error::last_os_error()));
        }
        let mac = hw_address(TAP_NAME).ok_or("The network interface has no address.")?;
        Ok(Tap { fd, mac })
    }
}

/// The hardware address of an interface, asked of the kernel (`SIOCGIFHWADDR`).
fn hw_address(name: &str) -> Option<Mac> {
    let mut req = [0u8; 40];
    req[..name.len().min(15)].copy_from_slice(&name.as_bytes()[..name.len().min(15)]);
    // SAFETY: a datagram socket used for one request; req is the size of struct ifreq
    unsafe {
        let s = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | crate::oscompat::SOCK_CLOEXEC, 0);
        if s < 0 {
            return None;
        }
        let r = libc::ioctl(s, SIOCGIFHWADDR, req.as_mut_ptr());
        libc::close(s);
        if r < 0 {
            return None;
        }
    }
    req[18..24].try_into().ok()
}

/// The L2CAP channel and the TAP together.
pub struct RealLink {
    sock: crate::bthid_sys::L2Sock,
    tap: Tap,
}

fn read_fd(fd: RawFd, buf: &mut [u8]) -> isize {
    // SAFETY: buf is valid for its length
    unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) }
}

impl PanLink for RealLink {
    fn wait(&mut self, t: Duration) -> io::Result<LinkEv> {
        let mut fds = [libc::pollfd { fd: self.sock.raw(), events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: self.tap.fd.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
        // SAFETY: two valid pollfds
        let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, t.as_millis().min(i32::MAX as u128) as i32) };
        if n < 0 {
            let e = io::Error::last_os_error();
            return if e.kind() == io::ErrorKind::Interrupted { Ok(LinkEv::Idle) } else { Err(e) };
        }
        let mut buf = vec![0u8; 4096];
        if fds[0].revents != 0 {
            let r = read_fd(self.sock.raw(), &mut buf);
            if r < 0 && io::Error::last_os_error().kind() == io::ErrorKind::WouldBlock {
                return Ok(LinkEv::Idle);
            }
            if r <= 0 {
                return Ok(LinkEv::Closed);
            }
            buf.truncate(r as usize);
            return Ok(LinkEv::Phone(buf));
        }
        if fds[1].revents & libc::POLLIN != 0 {
            let r = read_fd(self.tap.fd.as_raw_fd(), &mut buf);
            if r > 0 {
                buf.truncate(r as usize);
                return Ok(LinkEv::Tap(buf));
            }
        }
        Ok(LinkEv::Idle)
    }

    fn to_phone(&mut self, packet: &[u8]) -> io::Result<()> {
        use crate::bthid::Channel;
        match self.sock.send(packet) {
            // a full queue drops the packet; the network above resends
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(()),
            other => other,
        }
    }

    fn to_tap(&mut self, frame: &[u8]) -> io::Result<()> {
        // SAFETY: frame is valid for its length; a dropped frame is fine (the network resends)
        unsafe { libc::write(self.tap.fd.as_raw_fd(), frame.as_ptr() as *const libc::c_void, frame.len()) };
        Ok(())
    }
}

fn stop_dhcp() {
    let Ok(t) = std::fs::read_to_string(DHCP_PID) else { return };
    if let Ok(pid) = t.trim().parse::<i32>() {
        if pid > 1 {
            // SAFETY: a plain signal to the process number the DHCP client of this link wrote
            unsafe { libc::kill(pid, libc::SIGTERM) };
        }
    }
}

// ------------------------------------------------------------------------------------------------ the manager

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TetherState {
    Idle,
    Connecting,
    Connected,
    /// The last try ended with this plain reason.
    Failed(String),
}

/// What the page and the assistant show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TetherView {
    /// The phone of the current or last connection (empty: none yet).
    pub address: String,
    pub name: String,
    pub state: TetherState,
}

impl TetherView {
    pub fn idle() -> TetherView {
        TetherView { address: String::new(), name: String::new(), state: TetherState::Idle }
    }
}

/// The phone-as-uplink connection. The real one is [`tether`]; tests use a fake.
pub trait Tether: Send + Sync {
    fn view(&self) -> TetherView;
    /// Starts connecting to a paired phone in the background (takes seconds; `view` shows the progress and, on failure, the reason).
    /// `Err` is a reason found at once (not paired, not a phone, no controller, radio off, already connected).
    fn connect(&self, address: &str) -> Result<(), String>;
    fn disconnect(&self) -> Result<(), String>;
}

struct Inner {
    view: TetherView,
    stop: Option<Arc<AtomicBool>>,
}

pub struct RealTether {
    inner: Mutex<Inner>,
}

static TETHER: std::sync::OnceLock<Arc<RealTether>> = std::sync::OnceLock::new();

/// The one tethering connection of this program.
pub fn tether() -> Arc<RealTether> {
    TETHER.get_or_init(|| Arc::new(RealTether { inner: Mutex::new(Inner { view: TetherView::idle(), stop: None }) })).clone()
}

impl RealTether {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set(&self, address: &str, s: TetherState) {
        let mut g = self.lock();
        if g.view.address.eq_ignore_ascii_case(address) {
            g.view.state = s;
        }
    }

    fn session(&self, address: &str, stop: &AtomicBool) -> Result<End, String> {
        let phone = crate::btmgmt::parse_addr(address).ok_or("That is not a Bluetooth address.")?;
        let sock = crate::bthid_sys::L2Sock::connect_mtu(phone, bnep::PSM, CONNECT_SECS, Some(bnep::MIN_MTU)).map_err(|e| match e {
            HidError::Connect(n) => connect_reason(n),
            other => other.to_string(),
        })?;
        let tap = Tap::open()?;
        let local = tap.mac;
        // the kernel's byte order is the reverse of the written address; BNEP carries the written order
        let mut phone_written = phone;
        phone_written.reverse();
        crate::set_link(TAP_NAME, true).map_err(|e| format!("The network interface could not be switched on ({e})."))?;
        let mut link = RealLink { sock, tap };
        let mut up = || {
            self.set(address, TetherState::Connected);
            // the existing DHCP client, as for a cable
            let _ = crate::Control::dhcp(&crate::System, TAP_NAME);
        };
        let end = run(&mut link, local, phone_written, stop, Duration::from_secs(SETUP_SECS), &mut up);
        stop_dhcp();
        drop(link); // closes the TAP: the interface and its routes go
        Ok(end)
    }
}

impl Tether for RealTether {
    fn view(&self) -> TetherView {
        self.lock().view.clone()
    }

    fn connect(&self, address: &str) -> Result<(), String> {
        use crate::bthid::Devices;
        let list = crate::bthid_sys::RealDevices.list();
        let d = list.iter().find(|d| d.address.eq_ignore_ascii_case(address)).ok_or("That phone is not paired. Pair it in Settings -> Bluetooth first.")?;
        if d.kind != "phone" {
            return Err(format!("{} is not a phone: only a phone can share its internet over Bluetooth.", d.title()));
        }
        if let Some(why) = blocker_now() {
            return Err(why);
        }
        let stop = Arc::new(AtomicBool::new(false));
        {
            let mut g = self.lock();
            if matches!(g.view.state, TetherState::Connecting | TetherState::Connected) {
                return Err(if g.view.state == TetherState::Connected { "The phone is connected already. Disconnect it first.".into() } else { "A connection is being made already.".into() });
            }
            g.view = TetherView { address: d.address.clone(), name: d.name.clone(), state: TetherState::Connecting };
            g.stop = Some(stop.clone());
        }
        // the keys of the paired phones are given to the kernel first (the same hub that reconnects keyboards)
        crate::bthid_sys::hub().ensure_started();
        let (me, addr) = (tether(), d.address.clone());
        std::thread::spawn(move || {
            let r = me.session(&addr, &stop);
            let s = match r {
                Ok(End::Stopped) => TetherState::Idle,
                Ok(End::Lost) if stop.load(Ordering::SeqCst) => TetherState::Idle,
                Ok(End::Lost) => TetherState::Failed("The phone went away or switched tethering off. Press Connect to try again.".into()),
                Ok(End::Refused(c)) => TetherState::Failed(refusal_text(c)),
                Ok(End::NoAnswer) => TetherState::Failed(NO_ANSWER.into()),
                Err(why) => TetherState::Failed(why),
            };
            me.set(&addr, s);
            let mut g = me.lock();
            if g.stop.as_ref().is_some_and(|x| Arc::ptr_eq(x, &stop)) {
                g.stop = None;
            }
        });
        Ok(())
    }

    fn disconnect(&self) -> Result<(), String> {
        let g = self.lock();
        match (&g.stop, &g.view.state) {
            (Some(s), TetherState::Connecting | TetherState::Connected) => {
                s.store(true, Ordering::SeqCst);
                Ok(())
            }
            _ => Err("No phone is connected.".into()),
        }
    }
}

#[cfg(test)]
#[path = "bt_pan_tests.rs"]
mod tests;
