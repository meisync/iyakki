//! The host side of the 4-way handshake for a FullMAC card (for example brcmfmac) whose firmware does authentication, association
//! and beacons but does NOT act on a PMK handed over by `NL80211_ATTR_PMK` (verified on the Broadcom BCM4364 of the Mac mini: the
//! association succeeds and EAPOL message 1 is never sent). The access point is then started without a PMK, so the driver leaves
//! its firmware-auth mode at NONE and expects the host to run the handshake and install the keys, as hostapd does.
//!
//! This module is the pure logic: it learns of stations from "station added / removed" events (the card's association shows up as
//! `NL80211_CMD_NEW_STATION` / `DEL_STATION` on the `mlme` multicast group; there is no management frame to answer and no
//! `NEW_STATION` to send, brcmfmac has no `add_station`), starts one [`Authenticator`] per station, and turns its [`Action`]s into
//! calls on [`HostIo`] (EAPOL as plain Ethernet frames over a packet socket, key and authorize calls over nl80211).
//!
//! Driver facts (linux 6.12 brcmfmac, `cfg80211.c`), which fix the order of the calls:
//! * `brcmf_cfg80211_add_key` keeps the key in `profile.key[idx]`: the pairwise key (index 0, with an address) and the group key
//!   (index 1, no address) use different slots. The group key is set once after the access point is up (the interface must be up,
//!   `check_vif_up`), before any client associates, and the very same GTK goes into message 3.
//! * The pairwise key is installed after message 4 (the [`Authenticator`] orders `InstallPtk` before `Authorized`).
//! * `brcmf_cfg80211_change_station` only does something for the AUTHORIZED flag (`SET_SCB_AUTHORIZE`): that opens the port.
//! * `tx_control_port` is not implemented by brcmfmac, so EAPOL goes through a packet socket on the interface.

use crate::handshake::{Action, AuthConfig, Authenticator, FailReason};
use crate::{eapol, Mac};
use std::collections::HashMap;

/// What the handshake needs from the machine; the real one is in the agent (packet socket and nl80211), the tests fake it.
pub trait HostIo {
    fn send_eapol(&mut self, sta: Mac, eapol: &[u8]) -> Result<(), String>;
    fn install_ptk(&mut self, sta: Mac, tk: &[u8; 16]) -> Result<(), String>;
    fn authorize(&mut self, sta: Mac) -> Result<(), String>;
    /// Remove the station and send it a de-authentication with `reason`.
    fn deauth(&mut self, sta: Mac, reason: u16) -> Result<(), String>;
    /// Fresh random bytes for the authenticator nonce.
    fn random_nonce(&mut self) -> Result<[u8; 32], String>;
}

/// What happened, for tests and the log.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Stats {
    pub associated: u32,
    pub done: u32,
    pub failed: u32,
}

pub struct HostAp {
    pmk: [u8; 32],
    bssid: Mac,
    gtk: [u8; 16],
    gtk_id: u8,
    stations: HashMap<Mac, Authenticator>,
    log: Box<dyn FnMut(String)>,
    pub stats: Stats,
}

fn text(m: &Mac) -> String {
    m.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
}

impl HostAp {
    /// `gtk` is the group key the caller has already installed on the interface (index `gtk_id`).
    pub fn new(pmk: [u8; 32], bssid: Mac, gtk: [u8; 16], gtk_id: u8, log: Box<dyn FnMut(String)>) -> HostAp {
        HostAp { pmk, bssid, gtk, gtk_id, stations: HashMap::new(), log, stats: Stats::default() }
    }

    /// How many stations are in a handshake or done.
    pub fn station_count(&self) -> usize {
        self.stations.len()
    }

    /// The card reported an association (also a re-association of a known station: its handshake starts afresh).
    pub fn station_added(&mut self, io: &mut dyn HostIo, sta: Mac, now_ms: u64) {
        (self.log)(format!("hotspot: station {} associated", text(&sta)));
        self.stats.associated += 1;
        let anonce = match io.random_nonce() {
            Ok(n) => n,
            Err(e) => {
                (self.log)(format!("hotspot: handshake for {} not started: {e}", text(&sta)));
                return;
            }
        };
        let mut auth = Authenticator::new(AuthConfig { pmk: self.pmk, aa: self.bssid, spa: sta, anonce, gtk: self.gtk, gtk_id: self.gtk_id, rsn_ie: eapol::rsn_ie_ccmp_psk(), first_replay: 1 });
        let first = auth.start(now_ms);
        self.stations.insert(sta, auth);
        (self.log)(format!("hotspot: handshake started for {}", text(&sta)));
        self.apply(io, sta, first);
    }

    /// The card reported that a station left.
    pub fn station_removed(&mut self, sta: Mac) {
        if self.stations.remove(&sta).is_some() {
            (self.log)(format!("hotspot: station {} left", text(&sta)));
        }
    }

    /// An EAPOL frame came from `sta` (bytes after the Ethernet header).
    pub fn eapol(&mut self, io: &mut dyn HostIo, sta: Mac, bytes: &[u8], now_ms: u64) {
        let Some(a) = self.stations.get_mut(&sta) else { return };
        let acts = a.on_frame(bytes, now_ms);
        self.apply(io, sta, acts);
    }

    /// Call every 100 ms or so: resends messages and gives up on silent stations.
    pub fn tick(&mut self, io: &mut dyn HostIo, now_ms: u64) {
        let macs: Vec<Mac> = self.stations.keys().copied().collect();
        for m in macs {
            let acts = self.stations.get_mut(&m).map(|a| a.tick(now_ms)).unwrap_or_default();
            self.apply(io, m, acts);
        }
    }

    fn apply(&mut self, io: &mut dyn HostIo, sta: Mac, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Send(b) => {
                    if let Err(e) = io.send_eapol(sta, &b) {
                        (self.log)(format!("hotspot: EAPOL to {} failed: {e}", text(&sta)));
                    }
                }
                Action::InstallPtk { peer, tk } => {
                    if let Err(e) = io.install_ptk(peer, &tk) {
                        (self.log)(format!("hotspot: pairwise key for {} failed: {e}", text(&peer)));
                    }
                }
                Action::Authorized => match io.authorize(sta) {
                    Ok(()) => {
                        self.stats.done += 1;
                        (self.log)(format!("hotspot: handshake done for {}, keys installed", text(&sta)));
                    }
                    Err(e) => (self.log)(format!("hotspot: authorizing {} failed: {e}", text(&sta))),
                },
                Action::Failed(why) => {
                    let t = match why {
                        FailReason::WrongPassphrase => "bad MIC, wrong password",
                        FailReason::Timeout => "timeout",
                        FailReason::BadRsnIe => "bad RSN element",
                    };
                    (self.log)(format!("hotspot: handshake failed for {} ({t})", text(&sta)));
                    self.stats.failed += 1;
                    self.stations.remove(&sta);
                    let _ = io.deauth(sta, crate::mgmt::REASON_4WAY_TIMEOUT);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_supplicant::Supplicant;
    use crate::kdf::pmk_from_passphrase;
    use std::cell::RefCell;
    use std::rc::Rc;

    const AA: Mac = [0x02, 0, 0, 0, 0, 0x01];
    const S1: Mac = [0x02, 0, 0, 0, 0, 0x99];
    const S2: Mac = [0x02, 0, 0, 0, 0, 0x98];
    const GTK: [u8; 16] = [0x55; 16];

    #[derive(Default)]
    struct Fake {
        sent: Vec<(Mac, Vec<u8>)>,
        ptk: Vec<(Mac, [u8; 16])>,
        authorized: Vec<Mac>,
        deauthed: Vec<(Mac, u16)>,
        n: u8,
    }
    impl HostIo for Fake {
        fn send_eapol(&mut self, sta: Mac, b: &[u8]) -> Result<(), String> {
            self.sent.push((sta, b.to_vec()));
            Ok(())
        }
        fn install_ptk(&mut self, sta: Mac, tk: &[u8; 16]) -> Result<(), String> {
            self.ptk.push((sta, *tk));
            Ok(())
        }
        fn authorize(&mut self, sta: Mac) -> Result<(), String> {
            self.authorized.push(sta);
            Ok(())
        }
        fn deauth(&mut self, sta: Mac, r: u16) -> Result<(), String> {
            self.deauthed.push((sta, r));
            Ok(())
        }
        fn random_nonce(&mut self) -> Result<[u8; 32], String> {
            self.n += 1;
            Ok([self.n; 32])
        }
    }

    fn ap(pass: &str) -> (HostAp, Rc<RefCell<Vec<String>>>) {
        let lines = Rc::new(RefCell::new(Vec::new()));
        let l = lines.clone();
        (HostAp::new(pmk_from_passphrase(pass, b"IYAKKI-1A2B"), AA, GTK, 1, Box::new(move |s| l.borrow_mut().push(s))), lines)
    }

    fn client(pass: &str, sta: Mac) -> Supplicant {
        Supplicant::new(pmk_from_passphrase(pass, b"IYAKKI-1A2B"), AA, sta, [0x22; 32], eapol::rsn_ie_ccmp_psk())
    }

    /// Carry frames back and forth until nothing more is sent.
    fn pump(h: &mut HostAp, io: &mut Fake, cl: &mut Supplicant, sta: Mac, now: u64) {
        for _ in 0..6 {
            let (mine, others): (Vec<_>, Vec<_>) = io.sent.drain(..).partition(|(m, _)| *m == sta);
            io.sent = others;
            let frames: Vec<Vec<u8>> = mine.into_iter().map(|(_, b)| b).collect();
            if frames.is_empty() {
                break;
            }
            for f in frames {
                if let Some(r) = cl.on_frame(&f) {
                    h.eapol(io, sta, &r, now);
                }
            }
        }
    }

    #[test]
    fn an_association_sends_message_1() {
        let (mut h, lines) = ap("correct horse");
        let mut io = Fake::default();
        h.station_added(&mut io, S1, 0);
        assert_eq!(io.sent.len(), 1);
        assert_eq!(io.sent[0].0, S1);
        assert_eq!(h.station_count(), 1);
        let l = lines.borrow().join("\n");
        assert!(l.contains("02:00:00:00:00:99 associated") && l.contains("handshake started"), "{l}");
    }

    #[test]
    fn the_right_passphrase_installs_both_keys_and_opens_the_port() {
        let (mut h, lines) = ap("correct horse");
        let mut io = Fake::default();
        let mut cl = client("correct horse", S1);
        h.station_added(&mut io, S1, 0);
        pump(&mut h, &mut io, &mut cl, S1, 10);
        let (tk, gtk, id) = cl.installed().expect("client finished");
        assert_eq!((gtk, id), (GTK, 1), "message 3 carries the group key the interface already has");
        assert_eq!(io.ptk, vec![(S1, tk)]);
        assert_eq!(io.authorized, vec![S1]);
        assert!(io.deauthed.is_empty());
        assert_eq!(h.stats, Stats { associated: 1, done: 1, failed: 0 });
        assert!(lines.borrow().iter().any(|l| l.contains("handshake done") && l.contains("keys installed")));
    }

    #[test]
    fn a_wrong_passphrase_never_authorizes_and_is_dropped() {
        let (mut h, lines) = ap("correct horse");
        let mut io = Fake::default();
        let mut cl = client("wrong horse!!", S1);
        h.station_added(&mut io, S1, 0);
        pump(&mut h, &mut io, &mut cl, S1, 10);
        assert!(io.ptk.is_empty() && io.authorized.is_empty());
        // the client keeps being asked; after the last resend the host gives up
        let mut t = 0;
        while h.station_count() > 0 && t < 20_000 {
            t += 100;
            h.tick(&mut io, t);
            pump(&mut h, &mut io, &mut cl, S1, t);
        }
        assert_eq!(h.station_count(), 0);
        assert!(io.ptk.is_empty() && io.authorized.is_empty(), "nothing was installed or opened");
        assert_eq!(io.deauthed.len(), 1);
        assert_eq!(h.stats.failed, 1);
        assert!(lines.borrow().iter().any(|l| l.contains("handshake failed") && l.contains("bad MIC")), "{:?}", lines.borrow());
    }

    #[test]
    fn a_silent_station_gets_message_1_again_and_then_times_out() {
        let (mut h, lines) = ap("correct horse");
        let mut io = Fake::default();
        h.station_added(&mut io, S1, 0);
        let mut t = 0;
        while h.station_count() > 0 && t < 20_000 {
            t += 100;
            h.tick(&mut io, t);
        }
        assert_eq!(io.sent.len() as u32, crate::handshake::MAX_TRANSMITS, "first send plus resends");
        assert_eq!(h.stats.failed, 1);
        assert!(lines.borrow().iter().any(|l| l.contains("timeout")));
    }

    #[test]
    fn removal_clears_the_state_and_a_reassociation_starts_over() {
        let (mut h, _) = ap("correct horse");
        let mut io = Fake::default();
        h.station_added(&mut io, S1, 0);
        h.station_removed(S1);
        assert_eq!(h.station_count(), 0);
        // frames from a station we no longer know are ignored
        h.eapol(&mut io, S1, &[1, 3, 0, 0], 5);
        io.sent.clear();
        let mut cl = client("correct horse", S1);
        h.station_added(&mut io, S1, 1000);
        h.station_added(&mut io, S1, 1010); // associates again before the first handshake ended
        assert_eq!(h.station_count(), 1);
        io.sent.drain(..1); // the first M1 belongs to the abandoned handshake
        pump(&mut h, &mut io, &mut cl, S1, 1020);
        assert_eq!(io.authorized, vec![S1]);
    }

    #[test]
    fn two_stations_run_side_by_side_and_one_bad_one_does_not_hurt_the_other() {
        let (mut h, _) = ap("correct horse");
        let mut io = Fake::default();
        let mut good = client("correct horse", S1);
        let mut bad = client("nope nope nope", S2);
        h.station_added(&mut io, S1, 0);
        h.station_added(&mut io, S2, 0);
        for _ in 0..3 {
            pump(&mut h, &mut io, &mut good, S1, 10);
            pump(&mut h, &mut io, &mut bad, S2, 10);
        }
        assert_eq!(io.authorized, vec![S1]);
        assert!(good.installed().is_some() && bad.installed().is_none());
        assert_eq!(h.station_count(), 2, "the bad one is still being retried");
        assert!(io.ptk.iter().all(|(m, _)| *m == S1));
    }
}
