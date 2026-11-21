//! The runner for a card that leaves authentication, association, SAE and the four-way handshake to the computer
//! (`ApMode::Software`, mac80211): the product's hotspot manager starts it through `wifi_ap::start` after the access point is up, and ends it
//! with `wifi_ap::stop` (F-HOTSPOT-22, F-HOTSPOT-18). The frame handling is [`crate::softap`]; this file is the thread and the operating-system side.
//!
//! Decision (docs/development/ALPHA-7-DECISIONS.md, D-HOTSPOT-SW-RUNNER): the runner owns only the radio side. The interface address,
//! DHCP, DNS and the firewall stay the sharing manager's, on the interface `wifi_ap::start` returns, exactly as for the other two card kinds.
//! The radio test loop (`hotspot_radio`, used by the virtual-radio test) keeps its own address and DHCP because it runs without a manager,
//! but its frame handling is the same `softap` code.
//!
//! Its life is tied to the access point, like `wifi_ap_host`: `begin` after the access point started (a restart on another channel drops
//! the keys, so `end` first and `begin` again). Not verified on a card.
// SPDX-License-Identifier: Apache-2.0

use crate::nl80211_os::{poll_readable, EapolChannel, Nl80211Socket, PacketEapol, SharedNl};
use crate::softap::{KernelIo, SoftAp, SoftApConfig, SoftEvent};
use iyakki_hotspot::nl80211::{self, ApControl, Event, NetlinkTransport};
use iyakki_hotspot::rsn::SecurityMode;
use iyakki_hotspot::{mgmt, Mac};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

/// What reached the radio since the last look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incoming {
    Mgmt(Vec<u8>),
    StationGone(Mac),
    Eapol(Mac, Vec<u8>),
}

/// Where the incoming things come from: the kernel's event socket and the EAPOL channel on the real system, a script in the tests.
pub trait Source<E: EapolChannel> {
    fn poll(&mut self, wait_ms: i32, eap: &mut E) -> Vec<Incoming>;
}

/// One round of the access point's work, over any netlink transport, EAPOL channel and source.
pub struct RadioLoop<T: NetlinkTransport, E: EapolChannel, S: Source<E>> {
    pub soft: SoftAp,
    pub ap: ApControl<T>,
    pub eap: E,
    pub source: S,
    pub ifindex: u32,
    pub freq_mhz: u32,
}

impl<T: NetlinkTransport, E: EapolChannel, S: Source<E>> RadioLoop<T, E, S> {
    /// Waits up to `wait_ms` for something, handles it and the timers, returns what happened.
    pub fn step(&mut self, wait_ms: i32, now_ms: u64) -> Vec<SoftEvent> {
        let incoming = self.source.poll(wait_ms, &mut self.eap);
        let mut io = KernelIo { ap: &mut self.ap, eap: &mut self.eap, ifindex: self.ifindex, freq_mhz: self.freq_mhz };
        let mut ev = Vec::new();
        for i in incoming {
            match i {
                Incoming::Mgmt(f) => ev.extend(self.soft.on_mgmt(&mut io, &f, now_ms)),
                Incoming::StationGone(m) => ev.extend(self.soft.on_station_removed(m)),
                Incoming::Eapol(m, b) => ev.extend(self.soft.on_eapol(&mut io, m, &b, now_ms)),
            }
        }
        ev.extend(self.soft.tick(&mut io, now_ms));
        ev
    }
}

/// The real source: station and frame events of the kernel (one socket that registered for the frames) and EAPOL packets.
struct OsSource {
    events: Nl80211Socket,
    ifindex: u32,
}

impl Source<PacketEapol> for OsSource {
    fn poll(&mut self, wait_ms: i32, eap: &mut PacketEapol) -> Vec<Incoming> {
        let ready = poll_readable(&[self.events.as_raw_fd(), eap.raw_fd()], wait_ms);
        let mut out = Vec::new();
        if ready[0] {
            loop {
                match self.events.next_event(0) {
                    Ok(Some(raw)) => match nl80211::parse_event(&raw) {
                        Some(Event::Frame { ifindex, frame, .. }) if ifindex == self.ifindex => out.push(Incoming::Mgmt(frame)),
                        Some(Event::Station { ifindex, sta, added: false }) if ifindex == self.ifindex => out.push(Incoming::StationGone(sta)),
                        _ => {}
                    },
                    Ok(None) => break,
                    Err(e) => {
                        log::warn!("hotspot: reading Wi-Fi events: {e}");
                        std::thread::sleep(Duration::from_millis(200));
                        break;
                    }
                }
            }
        }
        if ready[1] {
            loop {
                match eap.recv(0) {
                    Ok(Some((m, b))) => out.push(Incoming::Eapol(m, b)),
                    Ok(None) => break,
                    Err(e) => {
                        log::warn!("hotspot: {e}");
                        break;
                    }
                }
            }
        }
        out
    }
}

/// What the thread runs; the real one is a [`RadioLoop`].
pub trait Round {
    fn round(&mut self, now_ms: u64) -> Vec<SoftEvent>;
}

struct Runner {
    stop: Arc<AtomicBool>,
    join: std::thread::JoinHandle<()>,
}

static RUNNER: Mutex<Option<Runner>> = Mutex::new(None);

/// Runs `setup` on a new thread (sockets are not handed between threads) and, when it succeeds, `round` until [`end`]. Returns once
/// the set-up is done, or with its error. Replaces a runner that is still there.
pub fn spawn_with<F>(setup: F) -> Result<(), String>
where
    F: FnOnce() -> Result<Box<dyn Round>, String> + Send + 'static,
{
    end();
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let st = stop.clone();
    let join = std::thread::Builder::new()
        .name("hotspot-sw-handshake".into())
        .spawn(move || match setup() {
            Ok(mut r) => {
                let _ = tx.send(Ok(()));
                let t0 = Instant::now();
                while !st.load(Ordering::SeqCst) {
                    for e in r.round(t0.elapsed().as_millis() as u64) {
                        if let SoftEvent::Log(l) = e {
                            log::info!("hotspot: {l}");
                        }
                    }
                }
            }
            Err(e) => {
                let _ = tx.send(Err(e));
            }
        })
        .map_err(|e| format!("starting the handshake thread: {e}"))?;
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => {
            *RUNNER.lock().unwrap_or_else(|p| p.into_inner()) = Some(Runner { stop, join });
            Ok(())
        }
        Ok(Err(e)) => {
            let _ = join.join();
            Err(e)
        }
        Err(_) => {
            stop.store(true, Ordering::SeqCst);
            Err("the handshake thread did not start".into())
        }
    }
}

/// Stop the runner (if any) and wait for it.
pub fn end() {
    let r = RUNNER.lock().unwrap_or_else(|p| p.into_inner()).take();
    if let Some(r) = r {
        r.stop.store(true, Ordering::SeqCst);
        let _ = r.join.join();
        log::info!("hotspot: software handshake runner stopped");
    }
}

/// Is a runner going?
pub fn running() -> bool {
    RUNNER.lock().unwrap_or_else(|p| p.into_inner()).is_some()
}

struct OsRound {
    inner: RadioLoop<SharedNl, PacketEapol, OsSource>,
}

impl Round for OsRound {
    fn round(&mut self, now_ms: u64) -> Vec<SoftEvent> {
        self.inner.step(100, now_ms)
    }
}

fn random(buf: &mut [u8]) -> Result<(), String> {
    iyakki_sec::random_fill(buf).map_err(|e| format!("random bytes: {e}"))
}

/// The key index of the integrity group key, as the handshake's key data announces it (and as the kernel gets it).
pub const IGTK_IDX: u8 = 4;

/// Start the runner for the access point on `ifindex` (already up and started on `freq_mhz`). WPA3 and WPA2/WPA3 run the SAE exchange
/// here (plain SAE, D-HOTSPOT-22). Replaces a runner that is still there.
pub fn begin(ifindex: u32, bssid: Mac, ssid: &str, passphrase: &str, freq_mhz: u32, security: SecurityMode) -> Result<(), String> {
    let (ssid, passphrase) = (ssid.to_string(), passphrase.to_string());
    spawn_with(move || {
        let setup = || -> Result<Box<dyn Round>, String> {
            // one socket only for events and the registered frames, one for our calls, so an event never sits between a request and its answer
            let mut events = Nl80211Socket::open(Duration::from_secs(5)).map_err(|e| e.to_string())?;
            events.join_ap_groups().map_err(|e| format!("joining the Wi-Fi event groups: {e}"))?;
            for t in [nl80211::FRAME_AUTH, nl80211::FRAME_ASSOC_REQ] {
                events.run(|f, q| nl80211::register_frame(f, q, ifindex, t)).map_err(|e| format!("registering for frames: {e}"))?;
            }
            let mut ap = ApControl::new(SharedNl::new(Nl80211Socket::open(Duration::from_secs(5)).map_err(|e| e.to_string())?));
            let (mut gtk, mut igtk, mut secret) = ([0u8; 16], [0u8; 16], [0u8; 32]);
            random(&mut gtk)?;
            random(&mut igtk)?;
            random(&mut secret)?;
            ap.install_gtk(ifindex, 1, &gtk).map_err(|e| format!("installing the group key: {e}"))?;
            if security != SecurityMode::Wpa2 {
                // the integrity group key of the key data (index 4, as the handshake announces it) goes to the kernel as well
                ap.install_igtk(ifindex, IGTK_IDX, &igtk).map_err(|e| format!("installing the integrity group key: {e}"))?;
            }
            let eap = PacketEapol::open(ifindex)?;
            let band = if freq_mhz < 4000 { mgmt::Band::G24 } else { mgmt::Band::A5 };
            let soft = SoftAp::new(SoftApConfig { bssid, ssid: ssid.as_bytes().to_vec(), passphrase, band, security, max_clients: 16, gtk, igtk }, secret);
            log::info!("hotspot: software handshake running on ifindex {ifindex} ({})", security.label());
            Ok(Box::new(OsRound { inner: RadioLoop { soft, ap, eap, source: OsSource { events, ifindex }, ifindex, freq_mhz } }))
        };
        // built and used on the runner thread only (the netlink handle holds an `Rc`)
        setup()
    })
}

#[cfg(test)]
#[path = "wifi_ap_sw_tests.rs"]
mod tests;
