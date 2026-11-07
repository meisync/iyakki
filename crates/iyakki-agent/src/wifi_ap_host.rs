//! The runner for the hotspot's "host handshake" mode (`wifi_ap::HandshakeMode::Host`): a thread that keeps the 4-way handshake
//! going for a FullMAC card whose firmware ignores the PMK hand-over (the Mac mini's Broadcom BCM4364). The logic is
//! `iyakki_hotspot::hostap`; this file is only the operating-system side: the nl80211 `mlme` events that tell of stations, the
//! packet socket for EAPOL, and the key/authorize calls. Its life is tied to the access point: `begin` after the access point
//! is started, `end` before it is stopped or moved to another channel (that drops the keys, so the next `begin` installs them again).
//!
//! Not verified on hardware by the code's author: see the module tests for what is covered (the logic, not the socket calls).

use crate::nl80211_os::{poll_readable, EapolChannel, Nl80211Socket, PacketEapol, SharedNl};
use iyakki_hotspot::hostap::{HostAp, HostIo};
use iyakki_hotspot::nl80211::{self, ApControl, Event};
use iyakki_hotspot::{kdf, Mac};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

struct Runner {
    stop: Arc<AtomicBool>,
    join: std::thread::JoinHandle<()>,
}

static RUNNER: Mutex<Option<Runner>> = Mutex::new(None);

struct OsIo {
    ap: ApControl<SharedNl>,
    eap: PacketEapol,
    ifindex: u32,
}

fn nl_text(e: nl80211::NlError, nl: &SharedNl) -> String {
    let ext = nl.0.borrow().last_ext_ack().map(|t| format!(" ({t})")).unwrap_or_default();
    format!("{e}{ext}")
}

impl HostIo for OsIo {
    fn send_eapol(&mut self, sta: Mac, b: &[u8]) -> Result<(), String> {
        self.eap.send(sta, b)
    }
    fn install_ptk(&mut self, sta: Mac, tk: &[u8; 16]) -> Result<(), String> {
        self.ap.install_ptk(self.ifindex, sta, tk).map_err(|e| format!("NEW_KEY: {}", nl_text(e, self.ap.transport())))
    }
    fn authorize(&mut self, sta: Mac) -> Result<(), String> {
        self.ap.authorize_station(self.ifindex, sta).map_err(|e| format!("SET_STATION: {}", nl_text(e, self.ap.transport())))
    }
    fn deauth(&mut self, sta: Mac, reason: u16) -> Result<(), String> {
        self.ap.remove_station(self.ifindex, sta, reason).map_err(|e| format!("DEL_STATION: {}", nl_text(e, self.ap.transport())))
    }
    fn random_nonce(&mut self) -> Result<[u8; 32], String> {
        random32()
    }
}

fn random32() -> Result<[u8; 32], String> {
    use std::io::Read;
    let mut b = [0u8; 32];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).map_err(|e| format!("random bytes: {e}"))?;
    Ok(b)
}

/// Start the runner for the access point on `ifindex` (already up). Returns once the group key is installed and the sockets are
/// open, or with the reason it could not. Replaces a runner that is still there.
pub fn begin(ifindex: u32, bssid: Mac, ssid: &str, passphrase: &str) -> Result<(), String> {
    end();
    let pmk = kdf::pmk_from_passphrase(passphrase, ssid.as_bytes());
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let st = stop.clone();
    let join = std::thread::Builder::new()
        .name("hotspot-handshake".into())
        .spawn(move || match setup(ifindex, bssid, pmk) {
            Ok((events, io, ap)) => {
                let _ = tx.send(Ok(()));
                run(events, io, ap, &st);
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
        log::info!("hotspot: handshake runner stopped");
    }
}

fn setup(ifindex: u32, bssid: Mac, pmk: [u8; 32]) -> Result<(Nl80211Socket, OsIo, HostAp), String> {
    // one socket only for events (it joins the multicast groups), one for our calls, so an event never sits between a request and its answer
    let mut events = Nl80211Socket::open(Duration::from_secs(5)).map_err(|e| e.to_string())?;
    events.join_ap_groups().map_err(|e| format!("joining the Wi-Fi event groups: {e}"))?;
    let nl = SharedNl::new(Nl80211Socket::open(Duration::from_secs(5)).map_err(|e| e.to_string())?);
    let mut ap = ApControl::new(nl.clone());
    // the group key: set once now, before any client comes, and repeated in message 3 of every handshake
    let gtk16 = {
        let r = random32()?;
        let mut g = [0u8; 16];
        g.copy_from_slice(&r[..16]);
        g
    };
    ap.install_gtk(ifindex, 1, &gtk16).map_err(|e| format!("installing the group key: {}", nl_text(e, &nl)))?;
    let eap = PacketEapol::open(ifindex)?;
    let host = HostAp::new(pmk, bssid, gtk16, 1, Box::new(|l| log::info!("{l}")));
    log::info!("hotspot: host handshake running on ifindex {ifindex}");
    Ok((events, OsIo { ap, eap, ifindex }, host))
}

fn run(mut events: Nl80211Socket, mut io: OsIo, mut host: HostAp, stop: &AtomicBool) {
    let t0 = Instant::now();
    let now = || t0.elapsed().as_millis() as u64;
    while !stop.load(Ordering::SeqCst) {
        let ready = poll_readable(&[events.as_raw_fd(), io.eap.raw_fd()], 100);
        if ready[0] {
            loop {
                match events.next_event(0) {
                    Ok(Some(raw)) => match nl80211::parse_event(&raw) {
                        Some(Event::Station { ifindex, sta, added }) if ifindex == io.ifindex => {
                            if added {
                                host.station_added(&mut io, sta, now());
                            } else {
                                host.station_removed(sta);
                            }
                        }
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
                match io.eap.recv(0) {
                    Ok(Some((sta, bytes))) => host.eapol(&mut io, sta, &bytes, now()),
                    Ok(None) => break,
                    Err(e) => {
                        log::warn!("hotspot: {e}");
                        break;
                    }
                }
            }
        }
        host.tick(&mut io, now());
    }
}
