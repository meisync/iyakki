//! Use a paired phone's Bluetooth tethering as an internet source (PAN, the PANU role), without BlueZ and without the kernel's
//! `bnep` module: an L2CAP connection to the phone's BNEP port, the BNEP set-up of `bnep.rs`, and a TAP interface (`iyakbt0`)
//! the frames are pumped to and from. `udhcpc` then gets an address on it like on any cable, and the interface shows up in
//! Settings -> Network as "Bluetooth (use a phone's tethering)".
//!
//! The phone must be paired first (Settings -> Bluetooth) and have Bluetooth tethering switched on.

use crate::bnep::{self, Frame, Mac, Panu, State};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The TAP interface of the connection.
pub const TAP_NAME: &str = "iyakbt0";

const AF_BLUETOOTH: i32 = 31;
const BTPROTO_L2CAP: i32 = 0;
const SOL_BLUETOOTH: i32 = 274;
const BT_SECURITY: i32 = 4;
const BT_SECURITY_MEDIUM: u8 = 2;
const BDADDR_BREDR: u8 = 0;
const TUNSETIFF: libc::Ioctl = 0x4004_54ca;
const IFF_TAP: i16 = 0x0002;
const IFF_NO_PI: i16 = 0x1000;
const CONNECT_SECS: u64 = 15;
const SETUP_SECS: u64 = 10;

/// `sockaddr_l2` of the kernel (14 bytes): family, PSM, the address in reverse byte order, channel id, address type.
fn sockaddr_l2(addr: Mac) -> [u8; 14] {
    let mut s = [0u8; 14];
    s[0..2].copy_from_slice(&(AF_BLUETOOTH as u16).to_le_bytes());
    s[2..4].copy_from_slice(&bnep::PSM.to_le_bytes());
    for (i, b) in addr.iter().rev().enumerate() {
        s[4 + i] = *b;
    }
    s[12] = 0;
    s[13] = BDADDR_BREDR;
    s
}

fn errno_text(what: &str) -> String {
    format!("{what}: {}", std::io::Error::last_os_error())
}

fn open_l2cap(phone: Mac) -> Result<OwnedFd, String> {
    // SAFETY: plain socket call; the descriptor is owned right away
    let fd = unsafe { libc::socket(AF_BLUETOOTH, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK, BTPROTO_L2CAP) };
    if fd < 0 {
        return Err(errno_text("no Bluetooth network socket (is the Bluetooth driver loaded?)"));
    }
    // SAFETY: fd is a fresh descriptor we own
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // pairing and encryption are asked for by the kernel when the link needs them
    let level = BT_SECURITY_MEDIUM as libc::c_int;
    // SAFETY: valid pointer and length for the option value
    unsafe { libc::setsockopt(fd.as_raw_fd(), SOL_BLUETOOTH, BT_SECURITY, &level as *const _ as *const libc::c_void, std::mem::size_of::<libc::c_int>() as u32) };
    let sa = sockaddr_l2(phone);
    // SAFETY: sa is 14 readable bytes, the length says so
    let r = unsafe { libc::connect(fd.as_raw_fd(), sa.as_ptr() as *const libc::sockaddr, sa.len() as u32) };
    if r < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINPROGRESS) {
        return Err(errno_text("could not reach the phone"));
    }
    let mut p = libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLOUT, revents: 0 };
    // SAFETY: one valid pollfd
    let n = unsafe { libc::poll(&mut p, 1, (CONNECT_SECS * 1000) as i32) };
    if n <= 0 {
        return Err("the phone did not answer: is Bluetooth switched on, and is it in range?".into());
    }
    let mut err: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>() as u32;
    // SAFETY: valid pointers for SO_ERROR
    unsafe { libc::getsockopt(fd.as_raw_fd(), libc::SOL_SOCKET, libc::SO_ERROR, &mut err as *mut _ as *mut libc::c_void, &mut len) };
    if err != 0 {
        return Err(match err {
            libc::EACCES | libc::ECONNREFUSED => "the phone refused: pair it first (Settings -> Bluetooth) and switch on its Bluetooth tethering".to_string(),
            libc::EHOSTUNREACH | libc::ETIMEDOUT => "the phone is out of range or has Bluetooth off".to_string(),
            e => format!("could not connect to the phone ({})", std::io::Error::from_raw_os_error(e)),
        });
    }
    Ok(fd)
}

fn open_tap() -> Result<OwnedFd, String> {
    // SAFETY: opening a device node
    let fd = unsafe { libc::open(c"/dev/net/tun".as_ptr(), libc::O_RDWR | libc::O_CLOEXEC | libc::O_NONBLOCK) };
    if fd < 0 {
        return Err(errno_text("no network tunnel device"));
    }
    // SAFETY: fd is fresh
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut req = [0u8; 40];
    req[..TAP_NAME.len()].copy_from_slice(TAP_NAME.as_bytes());
    req[16..18].copy_from_slice(&(IFF_TAP | IFF_NO_PI).to_ne_bytes());
    // SAFETY: req is the size of struct ifreq
    if unsafe { libc::ioctl(fd.as_raw_fd(), TUNSETIFF, req.as_mut_ptr()) } < 0 {
        return Err(errno_text("could not make the network interface"));
    }
    Ok(fd)
}

fn read_fd(fd: RawFd, buf: &mut [u8]) -> isize {
    // SAFETY: buf is valid for its length
    unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) }
}

fn write_fd(fd: RawFd, data: &[u8]) {
    // SAFETY: data is valid for its length; a dropped packet is fine (the network resends)
    unsafe { libc::write(fd, data.as_ptr() as *const libc::c_void, data.len()) };
}

fn mac_of(name: &str) -> Option<Mac> {
    let t = std::fs::read_to_string(format!("/sys/class/net/{name}/address")).ok()?;
    t.trim().split(':').map(|p| u8::from_str_radix(p, 16).ok()).collect::<Option<Vec<_>>>()?.try_into().ok()
}

fn run(cmd: &[&str]) {
    let _ = std::process::Command::new(cmd[0]).args(&cmd[1..]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
}

/// A running connection. Dropping it (or `stop`) ends it and removes the interface.
pub struct PanLink {
    stop: Arc<AtomicBool>,
    pub phone: Mac,
}

impl PanLink {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    pub fn running(&self) -> bool {
        !self.stop.load(Ordering::SeqCst)
    }
}

impl Drop for PanLink {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Connect to a paired phone and bring the interface up. Returns after the phone accepted (or refused) the connection.
pub fn connect(phone: Mac, log: impl Fn(String) + Send + 'static) -> Result<PanLink, String> {
    let l2 = open_l2cap(phone)?;
    log("bluetooth tethering: connected to the phone, setting up the network service".into());
    let tap = open_tap()?;
    run(&["ip", "link", "set", "dev", TAP_NAME, "up"]);
    let local = mac_of(TAP_NAME).ok_or("the network interface has no address")?;
    let (mut panu, first) = Panu::start(local, phone, bnep::UUID_NAP);
    write_fd(l2.as_raw_fd(), &first);
    let deadline = Instant::now() + Duration::from_secs(SETUP_SECS);
    let mut buf = vec![0u8; 4096];
    while panu.state == State::Connecting {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("the phone did not answer the network request: switch Bluetooth tethering on in its settings".into());
        }
        let mut p = libc::pollfd { fd: l2.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd
        if unsafe { libc::poll(&mut p, 1, left.as_millis().min(1000) as i32) } <= 0 {
            continue;
        }
        let n = read_fd(l2.as_raw_fd(), &mut buf);
        if n <= 0 {
            return Err("the phone closed the connection".into());
        }
        if let Ok(o) = panu.receive(&buf[..n as usize]) {
            o.replies.iter().for_each(|r| write_fd(l2.as_raw_fd(), r));
        }
    }
    if let State::Refused(code) = panu.state {
        return Err(bnep::setup_text(code).to_string());
    }
    log("bluetooth tethering: the phone accepted; asking it for an address".into());
    run(&["udhcpc", "-b", "-R", "-i", TAP_NAME, "-p", &format!("/run/udhcpc-{TAP_NAME}.pid")]);
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    std::thread::spawn(move || {
        pump(l2, tap, panu, &flag);
        run(&["ip", "link", "set", "dev", TAP_NAME, "down"]);
        flag.store(true, Ordering::SeqCst);
        log("bluetooth tethering: the connection ended".into());
    });
    Ok(PanLink { stop, phone })
}

fn pump(l2: OwnedFd, tap: OwnedFd, mut panu: Panu, stop: &AtomicBool) {
    let mut buf = vec![0u8; 4096];
    let mut frame = vec![0u8; 2048];
    while !stop.load(Ordering::SeqCst) {
        let mut fds = [libc::pollfd { fd: l2.as_raw_fd(), events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: tap.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
        // SAFETY: two valid pollfds
        if unsafe { libc::poll(fds.as_mut_ptr(), 2, 500) } <= 0 {
            continue;
        }
        if fds[0].revents & (libc::POLLHUP | libc::POLLERR) != 0 {
            return;
        }
        if fds[0].revents & libc::POLLIN != 0 {
            let n = read_fd(l2.as_raw_fd(), &mut buf);
            if n <= 0 {
                return;
            }
            if let Ok(o) = panu.receive(&buf[..n as usize]) {
                o.replies.iter().for_each(|r| write_fd(l2.as_raw_fd(), r));
                if let Some(f) = o.deliver {
                    write_fd(tap.as_raw_fd(), &ethernet_bytes(&f));
                }
            }
        }
        if fds[1].revents & libc::POLLIN != 0 {
            let n = read_fd(tap.as_raw_fd(), &mut frame);
            if n >= 14 {
                let f = &frame[..n as usize];
                let fr = Frame { dst: f[0..6].try_into().expect("six"), src: f[6..12].try_into().expect("six"), proto: u16::from_be_bytes([f[12], f[13]]), payload: f[14..].to_vec() };
                if let Some(p) = panu.send(&fr) {
                    write_fd(l2.as_raw_fd(), &p);
                }
            }
        }
    }
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

/// Whether a Bluetooth controller with a usable address is there. `Err` is one plain line for the person.
pub fn controller_state(sys_root: &std::path::Path) -> Result<String, String> {
    let dir = sys_root.join("class/bluetooth");
    let mut names: Vec<String> = std::fs::read_dir(&dir).map(|d| d.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| n.starts_with("hci") && !n.contains(':')).collect()).unwrap_or_default();
    names.sort();
    if names.is_empty() {
        return Err("This computer has no Bluetooth controller.".into());
    }
    for n in &names {
        let a = std::fs::read_to_string(dir.join(n).join("address")).unwrap_or_default();
        let a = a.trim();
        if !a.is_empty() && a != "00:00:00:00:00:00" && !a.eq_ignore_ascii_case("AA:AA:AA:AA:AA:AA") {
            return Ok(n.clone());
        }
    }
    Err("The Bluetooth controller has no address yet: it is waiting for a firmware file that cannot be shipped (the Mac mini's own chip). A USB Bluetooth adapter works.".into())
}

/// The one connection of this process.
static LINK: Mutex<Option<PanLink>> = Mutex::new(None);

/// Connect to a phone (replacing an earlier connection).
pub fn connect_global(phone: Mac, log: impl Fn(String) + Send + 'static) -> Result<(), String> {
    let mut g = LINK.lock().map_err(|_| "internal lock")?;
    if let Some(old) = g.take() {
        old.stop();
    }
    *g = Some(connect(phone, log)?);
    Ok(())
}

pub fn disconnect_global() {
    if let Ok(mut g) = LINK.lock() {
        if let Some(l) = g.take() {
            l.stop();
        }
    }
}

/// The phone the tethering is connected to, while it is.
pub fn connected_phone() -> Option<Mac> {
    LINK.lock().ok()?.as_ref().filter(|l| l.running()).map(|l| l.phone)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_l2cap_address_is_in_the_kernels_byte_order_on_the_bnep_port() {
        let s = sockaddr_l2([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
        assert_eq!(&s[0..2], &31u16.to_le_bytes());
        assert_eq!(&s[2..4], &0x000fu16.to_le_bytes(), "PSM 15");
        assert_eq!(&s[4..10], &[0xff, 0xee, 0xdd, 0xcc, 0xbb, 0xaa], "reversed");
        assert_eq!(s[13], 0, "a classic (BR/EDR) address");
    }

    #[test]
    fn a_frame_for_the_interface_has_no_bluetooth_headers() {
        let f = Frame { dst: [1; 6], src: [2; 6], proto: 0x0806, payload: vec![7, 8] };
        assert_eq!(ethernet_bytes(&f), [&[1u8; 6][..], &[2u8; 6], &[8, 6], &[7, 8]].concat());
    }

    #[test]
    fn a_controller_without_an_address_is_told_apart_from_none_and_from_a_good_one() {
        let d = std::env::temp_dir().join(format!("bt-pan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        assert!(controller_state(&d).unwrap_err().contains("no Bluetooth controller"));
        std::fs::create_dir_all(d.join("class/bluetooth/hci0")).unwrap();
        assert!(controller_state(&d).unwrap_err().contains("no address"), "hci0 exists without an address file");
        std::fs::write(d.join("class/bluetooth/hci0/address"), "AA:AA:AA:AA:AA:AA\n").unwrap();
        assert!(controller_state(&d).is_err(), "the placeholder address is no address");
        std::fs::write(d.join("class/bluetooth/hci0/address"), "5C:F7:E6:01:02:03\n").unwrap();
        assert_eq!(controller_state(&d), Ok("hci0".to_string()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn nothing_is_connected_at_the_start() {
        assert_eq!(connected_phone(), None);
    }
}
