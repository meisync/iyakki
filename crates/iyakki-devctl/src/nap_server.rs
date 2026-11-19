//! Bluetooth sharing, the running part (NAP, F-HOTSPOT-15): phones connect to this computer's network port (L2CAP PSM 15) and their
//! Ethernet frames meet the computer's own network on a TAP interface ([`crate::BT_NAP_IFACE`]).
//!
//! Layers, so the tests need no phone and no radio:
//! * [`NapHub`] is the whole decision-making of the server: it owns the BNEP session of every phone and the learning bridge of
//!   [`crate::bnep_nap`]; it takes events (a phone came, a packet from a phone, a frame from the TAP, a phone left) and answers with a
//!   list of [`Out`] actions. It has no sockets and no clock (the caller passes seconds);
//! * [`serve`] is the real loop: the listener on PSM 15 (encrypted and authenticated, so only paired phones get in), the TAP and one
//!   `poll` over all of them, until `stop` is set;
//! * [`NapService`] is the switch the pages and the assistant use (on, off, the phones now connected, why it cannot run).
//!
//! The TAP interface is only the door: the address, the forwarding and the sharing rules belong to the hotspot manager that treats it as
//! a downstream, exactly as for the access point of a Wi-Fi card. Never tried with a phone: Manual needed.
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;
use std::io;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use crate::bnep::{self, Mac};
use crate::bnep_nap::{Bridge, NapSession, NapState, Port};

/// What the server asks its caller to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    /// A BNEP packet to the phone with this port number.
    ToPhone(u8, Vec<u8>),
    /// An Ethernet frame (no header of ours) to the TAP interface.
    ToTap(Vec<u8>),
    /// Close the channel of this phone (a phone that does not belong here, or one the server could not take).
    Close(u8),
}

/// One connected phone.
struct Phone {
    session: NapSession,
    /// The address, written the usual way, for the person's list.
    address: String,
}

/// The server's brain.
pub struct NapHub {
    local: Mac,
    bridge: Bridge,
    phones: HashMap<u8, Phone>,
    allowed: bool,
}

impl NapHub {
    /// `local` is the TAP interface's own hardware address. Sharing is on: `allowed` is false while the person has it switched off
    /// (every phone is then answered "not allowed" and closed).
    pub fn new(local: Mac, allowed: bool) -> NapHub {
        NapHub { local, bridge: Bridge::new(), phones: HashMap::new(), allowed }
    }

    pub fn set_allowed(&mut self, on: bool) {
        self.allowed = on;
    }

    pub fn phones_see_each_other(&mut self, on: bool) {
        self.bridge.phones_see_each_other = on;
    }

    /// A phone connected. Its port number, or `None` when seven are connected already (the caller closes the channel).
    pub fn phone_came(&mut self, address: &str, phone_mac: Mac) -> Option<u8> {
        let id = self.bridge.add_phone()?;
        self.phones.insert(id, Phone { session: NapSession::new(self.local, phone_mac, self.allowed), address: address.to_string() });
        Some(id)
    }

    /// A phone left (or its channel failed).
    pub fn phone_left(&mut self, id: u8) {
        self.bridge.remove_phone(id);
        self.phones.remove(&id);
    }

    /// Addresses of the phones connected now, in port order (set up or still waiting for the set-up).
    pub fn connected(&self) -> Vec<(u8, String, bool)> {
        let mut v: Vec<_> = self.phones.iter().map(|(id, p)| (*id, p.address.clone(), p.session.state == NapState::Up)).collect();
        v.sort();
        v
    }

    /// A packet came from phone `id`. A packet that makes no sense is dropped: nothing a phone sends may end the server.
    pub fn from_phone(&mut self, id: u8, data: &[u8], now: u64) -> Vec<Out> {
        let mut out = Vec::new();
        let Some(p) = self.phones.get_mut(&id) else { return out };
        let Ok(o) = p.session.receive(data) else { return out };
        for r in o.replies {
            out.push(Out::ToPhone(id, r));
        }
        let refused = !self.allowed && p.session.state == NapState::Waiting && !out.is_empty();
        if let Some(f) = o.deliver {
            for port in self.bridge.forward(Port::Phone(id), &f, now) {
                self.emit(port, &f, &mut out);
            }
        }
        if refused {
            out.push(Out::Close(id));
        }
        out
    }

    /// A frame came from the TAP interface (the computer's side).
    pub fn from_tap(&mut self, frame: &[u8], now: u64) -> Vec<Out> {
        let mut out = Vec::new();
        let Some(f) = crate::bt_pan::frame_of(frame) else { return out };
        for port in self.bridge.forward(Port::Tap, &f, now) {
            self.emit(port, &f, &mut out);
        }
        out
    }

    fn emit(&self, port: Port, f: &bnep::Frame, out: &mut Vec<Out>) {
        match port {
            Port::Tap => out.push(Out::ToTap(crate::bt_pan::ethernet_bytes(f))),
            Port::Phone(n) => {
                if let Some(pk) = self.phones.get(&n).and_then(|p| p.session.send(f)) {
                    out.push(Out::ToPhone(n, pk));
                }
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------ the real loop

const MAX_PACKET: usize = 4096;

struct Live {
    sock: crate::bthid_sys::L2Sock,
    id: u8,
}

/// Why the server could not start or ended, in words for the person.
pub type Why = String;

/// Runs the server until `stop` is set. `view` is updated with the phones connected whenever they change.
pub fn serve(stop: &AtomicBool, allowed: &AtomicBool, view: &Mutex<Vec<(String, bool)>>) -> Result<(), Why> {
    use crate::bthid::Channel;
    let listener = crate::bthid_sys::Listener::open_with(bnep::PSM, 2, Some(bnep::MIN_MTU)).map_err(|e| format!("The Bluetooth network port could not be opened ({e}); this needs administrator rights and a powered controller."))?;
    let tap = crate::bt_pan::Tap::open_named(crate::BT_NAP_IFACE)?;
    crate::set_link(crate::BT_NAP_IFACE, true).map_err(|e| format!("The network interface could not be switched on ({e})."))?;
    let started = Instant::now();
    let mut hub = NapHub::new(tap.mac, allowed.load(Ordering::SeqCst));
    let mut live: Vec<Live> = Vec::new();
    let mut buf = vec![0u8; MAX_PACKET];
    let mut shown: Vec<(String, bool)> = Vec::new();
    while !stop.load(Ordering::SeqCst) {
        hub.set_allowed(allowed.load(Ordering::SeqCst));
        let mut fds = vec![libc::pollfd { fd: listener.raw(), events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: tap.fd.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
        fds.extend(live.iter().map(|l| libc::pollfd { fd: l.sock.raw(), events: libc::POLLIN, revents: 0 }));
        // SAFETY: valid pollfds
        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 500) };
        if n < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return Err("waiting for the network failed".into());
        }
        let now = started.elapsed().as_secs();
        let mut out: Vec<Out> = Vec::new();
        let mut gone: Vec<u8> = Vec::new();
        if fds[0].revents != 0 {
            while let Some((addr, sock)) = listener.accept() {
                let mut mac = crate::btmgmt::parse_addr(&addr).unwrap_or([0; 6]);
                mac.reverse();
                match hub.phone_came(&addr, mac) {
                    Some(id) => live.push(Live { sock, id }),
                    None => drop(sock),
                }
            }
        }
        if fds[1].revents & libc::POLLIN != 0 {
            // SAFETY: buf is valid for its length
            let r = unsafe { libc::read(tap.fd.as_raw_fd(), buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if r > 0 {
                out.extend(hub.from_tap(&buf[..r as usize], now));
            }
        }
        for (i, l) in live.iter_mut().enumerate() {
            if fds[2 + i].revents == 0 {
                continue;
            }
            // SAFETY: buf is valid for its length
            let r = unsafe { libc::read(l.sock.raw(), buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if r > 0 {
                out.extend(hub.from_phone(l.id, &buf[..r as usize], now));
            } else if r == 0 || io::Error::last_os_error().kind() != io::ErrorKind::WouldBlock {
                gone.push(l.id);
            }
        }
        for o in out {
            match o {
                Out::ToPhone(id, p) => {
                    if let Some(l) = live.iter_mut().find(|l| l.id == id) {
                        match l.sock.send(&p) {
                            // a full queue drops the packet; the network above resends
                            Err(e) if e.kind() != io::ErrorKind::WouldBlock => gone.push(id),
                            _ => {}
                        }
                    }
                }
                Out::ToTap(f) => {
                    // SAFETY: f is valid for its length; a dropped frame is fine
                    unsafe { libc::write(tap.fd.as_raw_fd(), f.as_ptr() as *const libc::c_void, f.len()) };
                }
                Out::Close(id) => gone.push(id),
            }
        }
        gone.sort_unstable();
        gone.dedup();
        for id in gone {
            hub.phone_left(id);
            live.retain(|l| l.id != id);
        }
        let now_view: Vec<(String, bool)> = hub.connected().into_iter().map(|(_, a, up)| (a, up)).collect();
        if now_view != shown {
            *view.lock().unwrap_or_else(|e| e.into_inner()) = now_view.clone();
            shown = now_view;
        }
    }
    view.lock().unwrap_or_else(|e| e.into_inner()).clear();
    Ok(())
}

// ------------------------------------------------------------------------------------------------ the switch

/// What the pages show.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NapView {
    pub running: bool,
    /// Set when it could not run (or stopped), in plain words.
    pub why_not: String,
    /// The phones connected now: address and whether the set-up finished.
    pub phones: Vec<(String, bool)>,
}

#[derive(Default)]
struct Inner {
    stop: Option<Arc<AtomicBool>>,
    why_not: String,
}

/// The one Bluetooth sharing service of this program.
pub struct NapService {
    inner: Mutex<Inner>,
    allowed: Arc<AtomicBool>,
    view: Arc<Mutex<Vec<(String, bool)>>>,
}

static SERVICE: OnceLock<Arc<NapService>> = OnceLock::new();

pub fn service() -> Arc<NapService> {
    SERVICE.get_or_init(|| Arc::new(NapService { inner: Mutex::new(Inner::default()), allowed: Arc::new(AtomicBool::new(false)), view: Arc::new(Mutex::new(Vec::new())) })).clone()
}

impl NapService {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Switch sharing on: starts the loop once. Off: the loop ends, the TAP goes, the phones are dropped.
    pub fn set_on(self: &Arc<Self>, on: bool) {
        let mut g = self.lock();
        if on {
            self.allowed.store(true, Ordering::SeqCst);
            if g.stop.is_some() {
                return;
            }
            let stop = Arc::new(AtomicBool::new(false));
            g.stop = Some(stop.clone());
            g.why_not.clear();
            let me = self.clone();
            std::thread::spawn(move || {
                let r = serve(&stop, &me.allowed, &me.view);
                let mut g = me.lock();
                // a stop that someone else already replaced is not ours to clear
                if g.stop.as_ref().is_some_and(|s| Arc::ptr_eq(s, &stop)) {
                    g.stop = None;
                    if let Err(w) = r {
                        g.why_not = w;
                    }
                }
            });
        } else {
            self.allowed.store(false, Ordering::SeqCst);
            if let Some(s) = g.stop.take() {
                s.store(true, Ordering::SeqCst);
            }
        }
    }

    pub fn view(&self) -> NapView {
        let g = self.lock();
        NapView { running: g.stop.is_some() && g.why_not.is_empty(), why_not: g.why_not.clone(), phones: self.view.lock().unwrap_or_else(|e| e.into_inner()).clone() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bnep::{Frame, UUID_NAP, UUID_PANU};

    const ME: Mac = [2, 0, 0, 0, 0, 1];
    const P1: Mac = [0xaa, 0, 0, 0, 0, 1];
    const P2: Mac = [0xaa, 0, 0, 0, 0, 2];
    const HOST: Mac = [2, 0, 0, 0, 0, 9];

    fn fr(dst: Mac, src: Mac) -> Frame {
        Frame { dst, src, proto: 0x0800, payload: vec![1, 2, 3] }
    }

    fn up(h: &mut NapHub, addr: &str, mac: Mac) -> u8 {
        let id = h.phone_came(addr, mac).unwrap();
        let o = h.from_phone(id, &bnep::setup_request(UUID_NAP, UUID_PANU), 0);
        assert_eq!(o, vec![Out::ToPhone(id, bnep::setup_response(0))]);
        id
    }

    #[test]
    fn a_phone_is_set_up_its_frames_reach_the_tap_and_the_answers_come_back() {
        let mut h = NapHub::new(ME, true);
        let a = up(&mut h, "AA:00:00:00:00:01", P1);
        assert_eq!(h.connected(), vec![(a, "AA:00:00:00:00:01".to_string(), true)]);
        let o = h.from_phone(a, &bnep::encode_frame(&fr(HOST, P1)), 1);
        assert_eq!(o, vec![Out::ToTap(crate::bt_pan::ethernet_bytes(&fr(HOST, P1)))]);
        let o = h.from_tap(&crate::bt_pan::ethernet_bytes(&fr(P1, HOST)), 2);
        assert_eq!(o, vec![Out::ToPhone(a, bnep::encode_frame(&fr(P1, HOST)))]);
    }

    #[test]
    fn phones_are_kept_apart_a_seventh_fits_an_eighth_does_not_and_a_gone_phone_is_forgotten() {
        let mut h = NapHub::new(ME, true);
        let a = up(&mut h, "A", P1);
        let b = up(&mut h, "B", P2);
        assert!(h.from_phone(a, &bnep::encode_frame(&fr(P2, P1)), 1).iter().all(|o| !matches!(o, Out::ToPhone(n, _) if *n == b)), "phone to phone is not carried");
        for i in 0..5 {
            h.phone_came(&format!("X{i}"), [0xbb, 0, 0, 0, 0, i]).unwrap();
        }
        assert!(h.phone_came("full", [0xcc; 6]).is_none());
        h.phone_left(b);
        assert!(h.connected().iter().all(|(id, _, _)| *id != b));
        assert!(h.from_phone(b, &bnep::encode_frame(&fr(HOST, P2)), 2).is_empty(), "a packet from a gone port is ignored");
    }

    #[test]
    fn nothing_flows_before_the_set_up_and_a_switched_off_server_refuses_and_closes() {
        let mut h = NapHub::new(ME, true);
        let a = h.phone_came("A", P1).unwrap();
        assert!(h.from_phone(a, &bnep::encode_frame(&fr(HOST, P1)), 0).is_empty());
        assert!(h.from_tap(&crate::bt_pan::ethernet_bytes(&fr(P1, HOST)), 0).is_empty(), "no packet to a phone that has not been accepted");
        let mut off = NapHub::new(ME, false);
        let b = off.phone_came("B", P1).unwrap();
        let o = off.from_phone(b, &bnep::setup_request(UUID_NAP, UUID_PANU), 0);
        assert_eq!(o, vec![Out::ToPhone(b, bnep::setup_response(crate::bnep_nap::RESP_NOT_ALLOWED)), Out::Close(b)]);
    }

    #[test]
    fn garbage_from_a_phone_or_the_tap_changes_nothing() {
        let mut h = NapHub::new(ME, true);
        let a = up(&mut h, "A", P1);
        assert!(h.from_phone(a, &[], 1).is_empty());
        assert!(h.from_phone(a, &[0x7f, 1], 1).is_empty());
        assert!(h.from_tap(&[1, 2, 3], 1).is_empty());
        assert_eq!(h.connected().len(), 1);
    }

    #[test]
    fn the_service_view_starts_empty_and_off() {
        let v = service().view();
        assert!(!v.running && v.phones.is_empty());
    }
}
