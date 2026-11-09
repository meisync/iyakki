//! The real machine under [`crate::bthid`]: L2CAP sockets, `/dev/uhid` and the background hub that keeps paired keyboards, mice and game
//! pads connected (F-NET-14). Needs administrator rights (the management channel, PSMs below 0x1001). Nothing here is unit-tested with a
//! real device: the protocol it drives is (`bthid_tests.rs`), and the glue below is small on purpose.
//!
//! What the hub does:
//! * remembers pairings in one file (`Store`, mode 0600: it holds the link keys) and gives the kernel its keys again after a restart;
//! * listens on the HID control and interrupt channels, so a bonded keyboard that is switched on connects back by itself;
//! * connects out (Connect button, the assistant's tool, once at start for devices that are on) when the device does not;
//! * keeps one thread per connected device; when the link is lost it tries again a few times, then waits for the device.
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::bthid::*;
use crate::btmgmt::{self, Answer, ControlSocket, Mgmt, Prompt, ADDR_BREDR};

/// Mgmt event: User Confirmation Request (a device is pairing with us and the number must be compared).
const EV_USER_CONFIRM_REQUEST_EV: u16 = btmgmt::EV_USER_CONFIRM_REQUEST;

const AF_BLUETOOTH: i32 = 31;
const BTPROTO_L2CAP: i32 = 0;
const SOL_BLUETOOTH: i32 = 274;
const BT_SECURITY: i32 = 4;
const BT_SECURITY_MEDIUM: u8 = 2;
const SOL_L2CAP: i32 = 6;
const L2CAP_OPTIONS: i32 = 1;
const CONNECT_SECS: u64 = 12;
const SDP_SECS: u64 = 8;
/// Where the pairings and keys are kept (one file, mode 0600).
pub const STORE_PATH: &str = "/var/lib/iyakki/bluetooth.json";

// ------------------------------------------------------------------------------------------------ sockets

/// `struct sockaddr_l2`: family, PSM, address (kernel byte order), channel id, address type.
fn sockaddr_l2(addr: [u8; 6], psm: u16) -> [u8; 14] {
    sockaddr_l2_full(addr, psm, 0, ADDR_BREDR)
}

/// The same with a fixed channel id and an address type (low energy: channel 4 is the attribute protocol, address type 1 or 2).
fn sockaddr_l2_full(addr: [u8; 6], psm: u16, cid: u16, addr_type: u8) -> [u8; 14] {
    let mut s = [0u8; 14];
    s[0..2].copy_from_slice(&(AF_BLUETOOTH as u16).to_le_bytes());
    s[2..4].copy_from_slice(&psm.to_le_bytes());
    s[4..10].copy_from_slice(&addr);
    s[10..12].copy_from_slice(&cid.to_le_bytes());
    s[12] = addr_type;
    s
}

/// The L2CAP fixed channel of the low-energy attribute protocol.
pub const CID_LE_ATT: u16 = 4;

fn new_l2cap() -> io::Result<OwnedFd> {
    new_l2cap_level(BT_SECURITY_MEDIUM)
}

/// An L2CAP socket with the given security level (0: none, as the service discovery channel needs; 2: encrypted and authenticated).
fn new_l2cap_level(level: u8) -> io::Result<OwnedFd> {
    // SAFETY: plain socket call; the descriptor is owned right away
    let fd = unsafe { libc::socket(AF_BLUETOOTH, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK, BTPROTO_L2CAP) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is fresh and ours
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // encryption and authentication (a bonded device passes; an unpaired one is refused)
    let level = level as libc::c_int;
    // SAFETY: valid pointer and size for the option value
    unsafe { libc::setsockopt(fd.as_raw_fd(), SOL_BLUETOOTH, BT_SECURITY, &level as *const _ as *const libc::c_void, std::mem::size_of::<libc::c_int>() as u32) };
    Ok(fd)
}

pub(crate) fn poll_one(fd: RawFd, events: i16, ms: i32) -> i16 {
    let mut p = libc::pollfd { fd, events, revents: 0 };
    // SAFETY: one valid pollfd
    let n = unsafe { libc::poll(&mut p, 1, ms) };
    if n > 0 { p.revents } else { 0 }
}

/// A connected L2CAP channel.
pub struct L2Sock {
    fd: OwnedFd,
}

impl L2Sock {
    /// Connects to `addr` (kernel byte order) on `psm`.
    pub fn connect(addr: [u8; 6], psm: u16, secs: u64) -> Result<L2Sock, HidError> {
        L2Sock::connect_mtu(addr, psm, secs, None)
    }

    /// [`connect`](Self::connect) with a larger L2CAP message size asked for before the connection is made (BNEP needs 1691; the kernel's
    /// default of 672 would cut Ethernet frames).
    pub fn connect_mtu(addr: [u8; 6], psm: u16, secs: u64, mtu: Option<u16>) -> Result<L2Sock, HidError> {
        let fd = new_l2cap().map_err(|e| HidError::Connect(e.raw_os_error().unwrap_or(libc::ENODEV)))?;
        if let Some(m) = mtu {
            let mut o = [0u8; 16];
            let mut len = o.len() as u32;
            // SAFETY: o is valid for len bytes; the kernel fills in its current options, of which only the two message sizes change
            let got = unsafe { libc::getsockopt(fd.as_raw_fd(), SOL_L2CAP, L2CAP_OPTIONS, o.as_mut_ptr() as *mut libc::c_void, &mut len) };
            if got == 0 && len >= 4 {
                o[0..2].copy_from_slice(&m.to_le_bytes());
                o[2..4].copy_from_slice(&m.to_le_bytes());
                // SAFETY: same buffer and length the kernel gave
                unsafe { libc::setsockopt(fd.as_raw_fd(), SOL_L2CAP, L2CAP_OPTIONS, o.as_ptr() as *const libc::c_void, len) };
            }
        }
        L2Sock::finish_connect(fd, sockaddr_l2(addr, psm), secs)
    }

    /// Connects to a low-energy device on the attribute protocol's fixed channel (HID over GATT). `addr_type` is 1 (public) or 2 (random).
    pub fn connect_le(addr: [u8; 6], addr_type: u8, secs: u64) -> Result<L2Sock, HidError> {
        let fd = new_l2cap().map_err(|e| HidError::Connect(e.raw_os_error().unwrap_or(libc::ENODEV)))?;
        L2Sock::finish_connect(fd, sockaddr_l2_full(addr, 0, CID_LE_ATT, addr_type), secs)
    }

    fn finish_connect(fd: OwnedFd, sa: [u8; 14], secs: u64) -> Result<L2Sock, HidError> {
        // SAFETY: sa is 14 readable bytes and the length says so
        let r = unsafe { libc::connect(fd.as_raw_fd(), sa.as_ptr() as *const libc::sockaddr, sa.len() as u32) };
        if r < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(HidError::Connect(io::Error::last_os_error().raw_os_error().unwrap_or(libc::EIO)));
        }
        if poll_one(fd.as_raw_fd(), libc::POLLOUT, (secs * 1000) as i32) == 0 {
            return Err(HidError::Connect(libc::ETIMEDOUT));
        }
        let mut err: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as u32;
        // SAFETY: valid pointers for SO_ERROR
        unsafe { libc::getsockopt(fd.as_raw_fd(), libc::SOL_SOCKET, libc::SO_ERROR, &mut err as *mut _ as *mut libc::c_void, &mut len) };
        if err != 0 {
            return Err(HidError::Connect(err));
        }
        Ok(L2Sock { fd })
    }

    pub fn raw(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl Channel for L2Sock {
    fn send(&mut self, data: &[u8]) -> io::Result<()> {
        // SAFETY: data is valid for its length
        let n = unsafe { libc::write(self.raw(), data.as_ptr() as *const libc::c_void, data.len()) };
        if n < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }

    fn recv(&mut self, wait: Duration) -> io::Result<Recv> {
        let rev = poll_one(self.raw(), libc::POLLIN, wait.as_millis().min(i32::MAX as u128) as i32);
        if rev == 0 {
            return Ok(Recv::Timeout);
        }
        let mut buf = vec![0u8; 4096];
        // SAFETY: buf is valid for its length
        let n = unsafe { libc::read(self.raw(), buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n < 0 {
            let e = io::Error::last_os_error();
            return if e.kind() == io::ErrorKind::WouldBlock { Ok(Recv::Timeout) } else { Ok(Recv::Closed) };
        }
        if n == 0 {
            return Ok(Recv::Closed);
        }
        buf.truncate(n as usize);
        Ok(Recv::Data(buf))
    }
}

/// A listening L2CAP socket on one PSM (for devices that connect to us).
pub(crate) struct Listener {
    fd: OwnedFd,
}

impl Listener {
    pub(crate) fn open(psm: u16) -> io::Result<Listener> {
        Listener::open_with(psm, BT_SECURITY_MEDIUM, None)
    }

    /// A listener with its own security level and (optionally) a larger incoming L2CAP message size.
    pub(crate) fn open_with(psm: u16, level: u8, imtu: Option<u16>) -> io::Result<Listener> {
        let fd = new_l2cap_level(level)?;
        if let Some(m) = imtu {
            let mut o = [0u8; 16];
            let mut len = o.len() as u32;
            // SAFETY: o is valid for len bytes; the kernel fills in its current options, of which only the message sizes change
            let got = unsafe { libc::getsockopt(fd.as_raw_fd(), SOL_L2CAP, L2CAP_OPTIONS, o.as_mut_ptr() as *mut libc::c_void, &mut len) };
            if got == 0 && len >= 4 {
                o[0..2].copy_from_slice(&m.to_le_bytes());
                o[2..4].copy_from_slice(&m.to_le_bytes());
                // SAFETY: same buffer and length the kernel gave
                unsafe { libc::setsockopt(fd.as_raw_fd(), SOL_L2CAP, L2CAP_OPTIONS, o.as_ptr() as *const libc::c_void, len) };
            }
        }
        let sa = sockaddr_l2([0; 6], psm);
        // SAFETY: sa is 14 readable bytes
        if unsafe { libc::bind(fd.as_raw_fd(), sa.as_ptr() as *const libc::sockaddr, sa.len() as u32) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: plain call on our descriptor
        if unsafe { libc::listen(fd.as_raw_fd(), 8) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Listener { fd })
    }

    pub(crate) fn raw(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// The next device that connected, with its address (display form).
    pub(crate) fn accept(&self) -> Option<(String, L2Sock)> {
        let mut sa = [0u8; 14];
        let mut len = sa.len() as u32;
        // SAFETY: sa and len are valid for the kernel to fill
        let fd = unsafe { libc::accept4(self.fd.as_raw_fd(), sa.as_mut_ptr() as *mut libc::sockaddr, &mut len, libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK) };
        if fd < 0 {
            return None;
        }
        // SAFETY: fd is a fresh descriptor we own
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        Some((btmgmt::addr_string(&sa[4..10]), L2Sock { fd }))
    }
}

/// `/dev/uhid`.
pub struct UhidFile {
    fd: OwnedFd,
}

impl UhidFile {
    pub fn open() -> Result<UhidFile, HidError> {
        let open = || {
            // SAFETY: opening a device node by a constant path
            unsafe { libc::open(c"/dev/uhid".as_ptr(), libc::O_RDWR | libc::O_CLOEXEC | libc::O_NONBLOCK) }
        };
        let mut fd = open();
        if fd < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
            // the driver is a module: load it once (fixed program and argument, nothing from outside)
            let _ = std::process::Command::new("modprobe").arg("uhid").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
            fd = open();
        }
        if fd < 0 {
            return Err(HidError::Uhid(io::Error::last_os_error()));
        }
        // SAFETY: fd is fresh and ours
        Ok(UhidFile { fd: unsafe { OwnedFd::from_raw_fd(fd) } })
    }
}

impl Uhid for UhidFile {
    fn write(&mut self, event: &[u8]) -> io::Result<()> {
        // SAFETY: event is valid for its length
        let n = unsafe { libc::write(self.fd.as_raw_fd(), event.as_ptr() as *const libc::c_void, event.len()) };
        if n < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }
}

/// Waits on the two channels and the uhid file together.
struct Mux {
    control: RawFd,
    interrupt: RawFd,
    uhid: RawFd,
}

impl Source for Mux {
    fn wait(&mut self, t: Duration) -> io::Result<Ev> {
        let mut fds = [
            libc::pollfd { fd: self.interrupt, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: self.control, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: self.uhid, events: libc::POLLIN, revents: 0 },
        ];
        // SAFETY: three valid pollfds
        let n = unsafe { libc::poll(fds.as_mut_ptr(), 3, t.as_millis().min(i32::MAX as u128) as i32) };
        if n < 0 {
            let e = io::Error::last_os_error();
            return if e.kind() == io::ErrorKind::Interrupted { Ok(Ev::Idle) } else { Err(e) };
        }
        let mut buf = vec![0u8; UHID_EVENT_SIZE + 64];
        for (i, p) in fds.iter().enumerate() {
            if p.revents & libc::POLLIN == 0 && p.revents & (libc::POLLHUP | libc::POLLERR) == 0 {
                continue;
            }
            // SAFETY: buf is valid for its length
            let r = unsafe { libc::read(p.fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if r <= 0 {
                if r < 0 && io::Error::last_os_error().kind() == io::ErrorKind::WouldBlock {
                    continue;
                }
                return Ok(if i == 2 { Ev::Idle } else { Ev::ChannelClosed });
            }
            let d = buf[..r as usize].to_vec();
            return Ok(match i {
                0 => Ev::Interrupt(d),
                1 => Ev::Control(d),
                _ => Ev::Uhid(d),
            });
        }
        Ok(Ev::Idle)
    }
}

// ------------------------------------------------------------------------------------------------ pairing state

fn random_pin() -> String {
    let mut b = [0u8; 4];
    // SAFETY: b is valid for 4 bytes
    unsafe { libc::getrandom(b.as_mut_ptr() as *mut libc::c_void, b.len(), 0) };
    format!("{:06}", u32::from_le_bytes(b) % 1_000_000)
}

// ------------------------------------------------------------------------------------------------ the hub

struct Inner {
    store: Store,
    live: HashMap<String, Arc<AtomicBool>>,
    states: HashMap<String, LinkState>,
    pair: PairState,
    answer: Option<Sender<bool>>,
    started: Option<Instant>,
    running: bool,
    /// Names devices gave when they connected (by address), for pairing prompts and the entry kept when a phone pairs with us.
    names: HashMap<String, String>,
    /// Until when this computer accepts pairing that a phone starts (Settings -> Bluetooth "Let a phone pair", `bluetooth_audio_pairing`).
    pairing_until: Option<Instant>,
}

pub struct Hub {
    path: PathBuf,
    inner: Mutex<Inner>,
}

static HUB: OnceLock<Arc<Hub>> = OnceLock::new();

/// The one hub of this program.
pub fn hub() -> Arc<Hub> {
    HUB.get_or_init(|| Hub::new(PathBuf::from(STORE_PATH))).clone()
}

impl Hub {
    pub fn new(path: PathBuf) -> Arc<Hub> {
        let store = std::fs::read_to_string(&path).map(|t| Store::parse(&t)).unwrap_or_default();
        Arc::new(Hub { path, inner: Mutex::new(Inner { store, live: HashMap::new(), states: HashMap::new(), pair: PairState::Idle, answer: None, started: None, running: false, names: HashMap::new(), pairing_until: None }) })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn save(&self, store: &Store) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = self.path.with_extension("tmp");
        use std::os::unix::fs::OpenOptionsExt;
        let ok = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).and_then(|mut f| io::Write::write_all(&mut f, store.to_text().as_bytes())).is_ok();
        if ok {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }

    fn set_state(&self, address: &str, s: LinkState) {
        self.lock().states.insert(address.to_ascii_uppercase(), s);
    }

    /// Starts the background work once (and again, at most every ten seconds, while the Bluetooth management channel is not available).
    pub fn ensure_started(self: &Arc<Self>) {
        {
            let mut g = self.lock();
            if g.running || g.started.is_some_and(|t| t.elapsed() < Duration::from_secs(10)) {
                return;
            }
            g.started = Some(Instant::now());
        }
        let Ok(sock) = ControlSocket::open() else { return };
        let mut m = Mgmt::new(sock);
        let store = self.lock().store.clone();
        let Ok(indexes) = m.index_list() else { return };
        for i in &indexes {
            // keys first: bonded devices reconnect without pairing again; connectable so they can reach us
            let _ = load_keys(&mut m, *i, &store);
            let _ = m.set_connectable(*i, true);
            for k in store.devices.iter() {
                self.auto_connect_le(&mut m, *i, k);
            }
        }
        self.lock().running = true;
        let (l1, l2) = (Listener::open(PSM_HID_CONTROL), Listener::open(PSM_HID_INTERRUPT));
        if let (Ok(c), Ok(i)) = (l1, l2) {
            let h = self.clone();
            std::thread::spawn(move || h.listen(c, i));
        }
        let h = self.clone();
        std::thread::spawn(move || h.watch(m));
        // devices that are on connect back by themselves; try the ones that do not
        for k in store.devices.iter().filter(|k| k.auto) {
            self.start_session(&k.address, None, true);
        }
    }

    /// Saves keys the kernel makes at pairing, and reacts to connections of known devices.
    fn watch(self: Arc<Self>, mut m: Mgmt<ControlSocket>) {
        loop {
            let ev = match m.next_event(Duration::from_secs(2)) {
                Ok(Some(e)) => e,
                Ok(None) => continue,
                Err(_) => {
                    self.lock().running = false;
                    return;
                }
            };
            match ev.code {
                EV_USER_CONFIRM_REQUEST_EV => {
                    let h = self.clone();
                    let (index, params) = (ev.index, ev.params.clone());
                    std::thread::spawn(move || h.incoming_confirm(index, &params));
                }
                EV_NEW_LINK_KEY | EV_NEW_LTK => {
                    if ev.code == EV_NEW_LINK_KEY && ev.params.len() >= 8 {
                        let a = btmgmt::addr_string(&ev.params[1..7]);
                        self.remember_peer(&a, ev.params[7]);
                    }
                    let mut g = self.lock();
                    let changed = if ev.code == EV_NEW_LINK_KEY { g.store.note_link_key(&ev.params) } else { g.store.note_ltk(&ev.params) };
                    if changed {
                        let s = g.store.clone();
                        drop(g);
                        self.save(&s);
                    }
                }
                btmgmt::EV_DEVICE_CONNECTED if ev.params.len() >= 7 => {
                    let a = btmgmt::addr_string(&ev.params[..6]);
                    if ev.params.len() >= 13 {
                        let n = u16::from_le_bytes([ev.params[11], ev.params[12]]) as usize;
                        if let Some(eir) = ev.params.get(13..13 + n) {
                            let name = btmgmt::parse_eir(eir).0;
                            if !name.is_empty() {
                                self.lock().names.insert(a.to_ascii_uppercase(), name);
                            }
                        }
                    }
                    let known = self.lock().store.device(&a).is_some();
                    if known {
                        let h = self.clone();
                        let le = ev.params.get(6).is_some_and(|t| *t != ADDR_BREDR);
                        std::thread::spawn(move || {
                            // a classic device usually opens the channels itself; if it has not after a moment, we do.
                            // A low-energy device only has the attribute channel, which we always open: no wait.
                            std::thread::sleep(Duration::from_secs(if le { 0 } else { 3 }));
                            h.start_session(&a, None, false);
                        });
                    }
                }
                _ => {}
            }
        }
    }

    /// Accepts the channels bonded devices open to us.
    fn listen(self: Arc<Self>, ctl: Listener, intr: Listener) {
        let mut waiting: HashMap<String, (Option<L2Sock>, Option<L2Sock>, Instant)> = HashMap::new();
        loop {
            let mut fds = [libc::pollfd { fd: ctl.fd.as_raw_fd(), events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: intr.fd.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
            // SAFETY: two valid pollfds
            unsafe { libc::poll(fds.as_mut_ptr(), 2, 1000) };
            for (i, l) in [&ctl, &intr].into_iter().enumerate() {
                while let Some((addr, sock)) = l.accept() {
                    if self.lock().store.device(&addr).is_none() {
                        continue; // not a device the person paired: dropped
                    }
                    let e = waiting.entry(addr.to_ascii_uppercase()).or_insert((None, None, Instant::now()));
                    if i == 0 { e.0 = Some(sock) } else { e.1 = Some(sock) }
                }
            }
            waiting.retain(|_, w| w.2.elapsed() < Duration::from_secs(10));
            let ready: Vec<String> = waiting.iter().filter(|(_, w)| w.0.is_some() && w.1.is_some()).map(|(a, _)| a.clone()).collect();
            for a in ready {
                if let Some((Some(c), Some(i), _)) = waiting.remove(&a) {
                    self.start_session(&a, Some((c, i)), true);
                }
            }
        }
    }

    /// Starts the thread of one device unless it has one already.
    fn start_session(self: &Arc<Self>, address: &str, socks: Option<(L2Sock, L2Sock)>, retry: bool) {
        let key = address.to_ascii_uppercase();
        let stop = Arc::new(AtomicBool::new(false));
        {
            let mut g = self.lock();
            // a phone is used for its tethering ([`crate::bt_pan`]), never as an input device
            if g.store.device(&key).is_none_or(|k| k.kind == "phone") || g.live.contains_key(&key) {
                return;
            }
            g.live.insert(key.clone(), stop.clone());
            g.states.insert(key.clone(), LinkState::Connecting);
        }
        let h = self.clone();
        std::thread::spawn(move || h.supervise(key, socks, retry, stop));
    }

    fn supervise(self: Arc<Self>, address: String, mut socks: Option<(L2Sock, L2Sock)>, mut retry: bool, stop: Arc<AtomicBool>) {
        let mut attempt = 0;
        loop {
            match self.once(&address, socks.take(), &stop) {
                Ok(End::Stopped) => {
                    self.set_state(&address, LinkState::Disconnected);
                    break;
                }
                Ok(End::Unplugged) => {
                    let _ = self.forget_inner(&address);
                    break;
                }
                Ok(End::LinkLost) => {
                    self.set_state(&address, LinkState::Disconnected);
                    attempt = 0;
                    retry = true;
                }
                Ok(End::Failed(m)) => self.set_state(&address, LinkState::Failed(m)),
                Err(e) => self.set_state(&address, LinkState::Failed(e.to_string())),
            }
            let auto = self.lock().store.device(&address).is_some_and(|k| k.auto);
            let Some(wait) = retry_delay(attempt).filter(|_| retry && auto) else { break };
            attempt += 1;
            let until = Instant::now() + wait;
            while Instant::now() < until && !stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(250));
            }
            if stop.load(Ordering::SeqCst) {
                break;
            }
            self.set_state(&address, LinkState::Connecting);
        }
        let mut g = self.lock();
        if g.live.get(&address).is_some_and(|s| Arc::ptr_eq(s, &stop)) {
            g.live.remove(&address);
        }
    }

    /// One connection, from the channels to the end of the session.
    fn once(&self, address: &str, socks: Option<(L2Sock, L2Sock)>, stop: &AtomicBool) -> Result<End, HidError> {
        let known = self.lock().store.device(address).cloned().ok_or(HidError::Closed)?;
        let addr = btmgmt::parse_addr(address).ok_or_else(|| HidError::Sdp("bad address".into()))?;
        if known.addr_type != ADDR_BREDR {
            return self.once_le(address, &known, addr, stop);
        }
        let (mut control, mut interrupt) = match socks {
            Some(p) => p,
            None => {
                let c = L2Sock::connect(addr, PSM_HID_CONTROL, CONNECT_SECS)?;
                (c, L2Sock::connect(addr, PSM_HID_INTERRUPT, CONNECT_SECS)?)
            }
        };
        let descriptor = match unhex(&known.descriptor).filter(|d| !d.is_empty()) {
            Some(d) => d,
            None => {
                let mut sdp = L2Sock::connect(addr, PSM_SDP, CONNECT_SECS)?;
                let rec = fetch_hid_record(&mut sdp, Duration::from_secs(SDP_SECS))?;
                let mut g = self.lock();
                let mut k = known.clone();
                k.descriptor = hex(&rec.descriptor);
                if k.name.is_empty() {
                    k.name = rec.name.unwrap_or_default();
                }
                g.store.upsert(k);
                let s = g.store.clone();
                drop(g);
                self.save(&s);
                rec.descriptor
            }
        };
        let name = if known.name.is_empty() { address.to_string() } else { known.name.clone() };
        let uf = UhidFile::open()?;
        let uhid_fd = uf.fd.as_raw_fd();
        let mut sess = Session::open(uf, &name, address, known.vendor, known.product, &descriptor, &mut control)?;
        self.set_state(address, LinkState::Connected);
        let mut mux = Mux { control: control.raw(), interrupt: interrupt.raw(), uhid: uhid_fd };
        let end = run_session(&mut sess, &mut interrupt, &mut mux, stop);
        sess.close();
        Ok(end)
    }

    /// One low-energy connection (HID over GATT): the attribute channel, discovery, notifications on, then the same kernel input device.
    fn once_le(&self, address: &str, known: &Known, addr: [u8; 6], stop: &AtomicBool) -> Result<End, HidError> {
        let sock = L2Sock::connect_le(addr, known.addr_type, CONNECT_SECS)?;
        let fd = sock.raw();
        let mut g = crate::att::Gatt::new(sock);
        let _ = g.exchange_mtu();
        let dev = crate::att::discover_hid(&mut g)?;
        if known.vendor == 0 && dev.vendor != 0 {
            let mut gd = self.lock();
            let mut k = known.clone();
            k.vendor = dev.vendor;
            k.product = dev.product;
            gd.store.upsert(k);
            let s = gd.store.clone();
            drop(gd);
            self.save(&s);
        }
        let name = if known.name.is_empty() { address.to_string() } else { known.name.clone() };
        let uf = UhidFile::open()?;
        let uhid_fd = uf.fd.as_raw_fd();
        let mut sess = crate::att::HogpSession::open(uf, &mut g, dev, &name, address)?;
        self.set_state(address, LinkState::Connected);
        // no control channel on low energy: -1 is ignored by poll
        let mut mux = Mux { control: -1, interrupt: fd, uhid: uhid_fd };
        let end = crate::att::run_hogp(&mut sess, &mut g, &mut mux, stop);
        sess.close();
        Ok(end)
    }

    /// Asks the controller to connect to a bonded low-energy device whenever it advertises (the kernel keeps doing it; the device
    /// switching on then shows up as a connection event and the hub starts its session).
    fn auto_connect_le(&self, m: &mut Mgmt<ControlSocket>, index: u16, k: &Known) {
        if k.addr_type != ADDR_BREDR && k.auto {
            let _ = m.add_device(index, &k.address, k.addr_type, btmgmt::ADD_DEVICE_AUTO_CONNECT);
        }
    }

    fn first_controller(&self) -> Option<u16> {
        let list = crate::btview::controllers().ok()?;
        list.iter().find(|c| c.configured && c.powered()).or_else(|| list.iter().find(|c| c.configured)).map(|c| c.index)
    }

    fn forget_inner(&self, address: &str) -> Result<(), String> {
        let key = address.to_ascii_uppercase();
        let (stop, k) = {
            let g = self.lock();
            (g.live.get(&key).cloned(), g.store.device(&key).cloned())
        };
        if let Some(s) = stop {
            s.store(true, Ordering::SeqCst);
        }
        // a phone that is forgotten stops sharing its internet at once
        {
            use crate::bt_pan::Tether;
            let t = crate::bt_pan::tether();
            if t.view().address.eq_ignore_ascii_case(&key) {
                let _ = t.disconnect();
            }
        }
        if let (Some(k), Some(i), Ok(sock)) = (k.as_ref(), self.first_controller(), ControlSocket::open()) {
            let _ = Mgmt::new(sock).unpair(i, &k.address, k.addr_type);
        }
        let mut g = self.lock();
        g.store.forget(&key);
        g.states.remove(&key);
        let s = g.store.clone();
        drop(g);
        self.save(&s);
        Ok(())
    }

    // ---- pairing

    pub fn pair_state(&self) -> PairState {
        self.lock().pair.clone()
    }

    /// Answers the confirmation the page shows.
    pub fn pair_answer(&self, yes: bool) -> bool {
        match self.lock().answer.as_ref() {
            Some(a) => a.send(yes).is_ok(),
            None => false,
        }
    }

    /// Pairs with a classic keyboard, mouse or game pad in the background (the page shows [`PairState`]).
    pub fn pair_start(self: &Arc<Self>, f: &btmgmt::Found) -> Result<(), String> {
        if f.addr_type != ADDR_BREDR && !is_hid_kind(f.kind) {
            return Err("only low-energy keyboards, mice and game pads (HID over GATT) can be paired; this low-energy device looks like something else".into());
        }
        if !is_pairable_kind(f.kind) {
            return Err(format!("only keyboards, mice, game pads and phones (for their Bluetooth tethering) can be paired in this version (this looks like: {}); audio (A2DP) is not available yet", kind_label(f.kind)));
        }
        {
            let g = self.lock();
            if matches!(g.pair, PairState::Running(_) | PairState::Confirm(..) | PairState::ShowPasskey(..) | PairState::ShowPin(..)) {
                return Err("pairing is already running".into());
            }
        }
        let index = self.first_controller().ok_or("there is no Bluetooth controller that is set up")?;
        let name = if f.name.is_empty() { f.address.clone() } else { f.name.clone() };
        let (tx, rx) = channel::<bool>();
        {
            let mut g = self.lock();
            g.pair = PairState::Running(name.clone());
            g.answer = Some(tx);
        }
        self.ensure_started();
        let (h, f) = (self.clone(), f.clone());
        std::thread::spawn(move || {
            let r = h.pair_run(index, &f, &name, &rx);
            let mut g = h.lock();
            g.answer = None;
            g.pair = PairState::Done(r.clone());
            drop(g);
            if r.is_ok() && is_hid_kind(f.kind) {
                // a keyboard usually connects to us by itself right after pairing; if not, we connect to it
                std::thread::sleep(Duration::from_secs(3));
                h.start_session(&f.address, None, true);
            }
        });
        Ok(())
    }

    fn pair_run(&self, index: u16, f: &btmgmt::Found, name: &str, rx: &Receiver<bool>) -> Result<String, String> {
        if !self.lock().running {
            return Err("the Bluetooth service of this computer is not running (needs administrator rights and a powered controller)".into());
        }
        let sock = ControlSocket::open().map_err(|e| format!("cannot open the Bluetooth management channel: {e}"))?;
        let mut m = Mgmt::new(sock);
        m.prepare(index).map_err(|e| e.to_string())?;
        if f.addr_type != ADDR_BREDR {
            // low-energy pairing needs the controller's LE side on (best effort: most controllers have it on)
            let _ = m.command(btmgmt::OP_SET_LE, index, &[1]);
        }
        let set = |s: PairState| self.lock().pair = s;
        let nm = name.to_string();
        let phone = f.kind == btmgmt::Kind::Phone;
        let mut ask = |p: &Prompt| -> Answer {
            match p {
                Prompt::Confirm { value, hint } => {
                    if *hint && !phone {
                        return Answer::Yes; // "just works" keyboard: nothing to compare
                    }
                    // a phone always asks the person, with the number to compare when it shows one
                    set(if *hint { PairState::ConfirmNoNumber(nm.clone()) } else { PairState::Confirm(nm.clone(), *value) });
                    match rx.recv_timeout(Duration::from_secs(60)) {
                        Ok(true) => Answer::Yes,
                        Ok(false) | Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => Answer::No,
                    }
                }
                Prompt::PasskeyShow(k) => {
                    set(PairState::ShowPasskey(nm.clone(), *k));
                    Answer::No
                }
                Prompt::PinRequest => {
                    let pin = random_pin();
                    set(PairState::ShowPin(nm.clone(), pin.clone()));
                    Answer::Pin(pin)
                }
                Prompt::PasskeyRequest => Answer::No,
            }
        };
        let r = m.pair(index, &f.address, f.addr_type, btmgmt::IO_DISPLAY_YES_NO, &mut ask);
        // nobody else may pair with this computer afterwards
        let _ = m.set_bondable(index, false);
        r.map_err(|e| format!("pairing failed: {e}"))?;
        if f.addr_type != ADDR_BREDR && is_hid_kind(f.kind) {
            let _ = m.add_device(index, &f.address, f.addr_type, btmgmt::ADD_DEVICE_AUTO_CONNECT);
        }
        let mut g = self.lock();
        g.store.upsert(Known { address: f.address.to_ascii_uppercase(), addr_type: f.addr_type, name: f.name.clone(), kind: kind_label(f.kind).into(), descriptor: String::new(), vendor: 0, product: 0, auto: is_hid_kind(f.kind) });
        let s = g.store.clone();
        drop(g);
        self.save(&s);
        Ok(format!("{name} is paired."))
    }
}

impl Hub {
    /// A device the person paired, by address.
    pub fn known(&self, address: &str) -> Option<Known> {
        self.lock().store.device(address).cloned()
    }

    /// The name a device gave when it connected (empty when not seen).
    pub fn seen_name(&self, address: &str) -> String {
        self.lock().names.get(&address.to_ascii_uppercase()).cloned().unwrap_or_default()
    }

    /// A device that paired with us (a phone choosing this computer in its own Bluetooth list) gets an entry, so it is listed, can be
    /// forgotten and is trusted by the audio sink. Kept as a "phone" (it is not used as an input device).
    fn remember_peer(&self, address: &str, addr_type: u8) {
        let mut g = self.lock();
        if g.store.device(address).is_some() {
            return;
        }
        let name = g.names.get(&address.to_ascii_uppercase()).cloned().unwrap_or_default();
        g.store.upsert(Known { address: address.to_ascii_uppercase(), addr_type, name, kind: "phone".into(), descriptor: String::new(), vendor: 0, product: 0, auto: false });
        let s = g.store.clone();
        drop(g);
        self.save(&s);
    }

    /// Whether pairing that a phone starts is accepted now.
    pub fn pairing_open(&self) -> bool {
        self.lock().pairing_until.is_some_and(|t| Instant::now() < t)
    }

    /// Lets a phone find and pair with this computer for `secs` seconds (discoverable, pairable, listed as a loudspeaker so phones offer
    /// it for audio), or ends that. The person answers the number comparison on the page ([`PairState::Confirm`]).
    pub fn set_pairing_open(self: &Arc<Self>, on: bool, secs: u16) -> Result<(), String> {
        self.ensure_started();
        let index = self.first_controller().ok_or("there is no Bluetooth controller that is set up")?;
        let sock = ControlSocket::open().map_err(|e| format!("cannot open the Bluetooth management channel: {e}"))?;
        let mut m = Mgmt::new(sock);
        if on {
            m.prepare(index).map_err(|e| e.to_string())?;
            m.set_connectable(index, true).map_err(|e| e.to_string())?;
            // audio / video, loudspeaker; services: rendering and audio (what phones look for when they list speakers)
            let _ = m.set_device_class(index, 4, 0x14);
            let _ = m.add_uuid16(index, 0x110b, 0x24);
            m.set_discoverable(index, true, secs).map_err(|e| e.to_string())?;
            self.lock().pairing_until = Some(Instant::now() + Duration::from_secs(secs as u64));
            let h = self.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(secs as u64 + 2));
                if !h.pairing_open() {
                    if let (Some(i), Ok(sock)) = (h.first_controller(), ControlSocket::open()) {
                        // nobody else may pair with this computer afterwards
                        let _ = Mgmt::new(sock).set_bondable(i, false);
                    }
                }
            });
        } else {
            self.lock().pairing_until = None;
            let _ = m.set_discoverable(index, false, 0);
            let _ = m.set_bondable(index, false);
        }
        Ok(())
    }

    /// A phone is pairing with us: ask the person (always, with the number when the phone shows one), then answer the kernel. Refused
    /// when pairing is not open, or when a pairing that the person started is running (that one answers for itself).
    fn incoming_confirm(&self, index: u16, p: &[u8]) {
        if p.len() < 12 {
            return;
        }
        let (addr, atype, hint, value) = (btmgmt::addr_string(&p[..6]), p[6], p[7] != 0, u32::from_le_bytes([p[8], p[9], p[10], p[11]]));
        let (tx, rx) = channel::<bool>();
        let open = {
            let mut g = self.lock();
            if g.answer.is_some() {
                return;
            }
            let open = g.pairing_until.is_some_and(|t| Instant::now() < t);
            if open {
                let name = g.names.get(&addr.to_ascii_uppercase()).cloned().filter(|n| !n.is_empty()).unwrap_or_else(|| addr.clone());
                g.pair = if hint { PairState::ConfirmNoNumber(name) } else { PairState::Confirm(name, value) };
                g.answer = Some(tx);
            }
            open
        };
        let yes = open && matches!(rx.recv_timeout(Duration::from_secs(60)), Ok(true));
        let Ok(sock) = ControlSocket::open() else { return };
        let mut m = Mgmt::new(sock);
        let mut a = p[..6].to_vec();
        a.push(atype);
        let _ = m.command(if yes { btmgmt::OP_USER_CONFIRM_REPLY } else { btmgmt::OP_USER_CONFIRM_NEG_REPLY }, index, &a);
        if open {
            let mut g = self.lock();
            g.answer = None;
            g.pair = PairState::Done(if yes { Ok("The phone is paired. It can play audio through this computer.".to_string()) } else { Err("pairing was not confirmed".to_string()) });
        }
    }

    fn list_inner(&self) -> Vec<DeviceView> {
        let g = self.lock();
        g.store
            .devices
            .iter()
            .map(|k| {
                let state = g.states.get(&k.address.to_ascii_uppercase()).cloned().unwrap_or(LinkState::Disconnected);
                DeviceView { address: k.address.clone(), addr_type: k.addr_type, name: k.name.clone(), kind: k.kind.clone(), state }
            })
            .collect()
    }

    fn disconnect_inner(&self, address: &str) -> Result<(), String> {
        match self.lock().live.get(&address.to_ascii_uppercase()) {
            Some(s) => {
                s.store(true, Ordering::SeqCst);
                Ok(())
            }
            None => Err("the device is not connected".into()),
        }
    }

    fn connect_inner(self: &Arc<Self>, address: &str) -> Result<(), String> {
        let key = address.to_ascii_uppercase();
        {
            let g = self.lock();
            match g.store.device(&key) {
                None => return Err("that device is not paired".into()),
                Some(k) if k.kind == "phone" => return Err("a phone is used for its internet, not as an input device: use Tether (Settings -> Bluetooth) or bluetooth_tether_connect".into()),
                Some(_) => {}
            }
            if g.live.contains_key(&key) {
                return Err(match g.states.get(&key) {
                    Some(LinkState::Connected) => "it is connected already".into(),
                    _ => "it is connecting already".into(),
                });
            }
        }
        self.ensure_started();
        self.start_session(&key, None, false);
        Ok(())
    }
}

/// The real [`Devices`]: the global hub, started on first use.
pub struct RealDevices;

impl Devices for RealDevices {
    fn list(&self) -> Vec<DeviceView> {
        let h = hub();
        h.ensure_started();
        h.list_inner()
    }
    fn connect(&self, address: &str) -> Result<(), String> {
        hub().connect_inner(address)
    }
    fn disconnect(&self, address: &str) -> Result<(), String> {
        hub().disconnect_inner(address)
    }
    fn forget(&self, address: &str) -> Result<(), String> {
        hub().forget_inner(address)
    }
}
