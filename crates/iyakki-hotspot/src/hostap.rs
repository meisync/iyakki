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

use crate::groupkey::{GroupRekey, RekeyAction};
use crate::handshake::{Action, AuthConfig, Authenticator, FailReason, State};
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
    /// Make `gtk` (index `id`) the key the interface sends broadcast and multicast frames with. Called once after every connected
    /// station has confirmed it ([`HostAp::rotate_group_key`]). The default refuses, so a card that cannot change it keeps the old key.
    fn install_gtk(&mut self, _id: u8, _gtk: &[u8; 16]) -> Result<(), String> {
        Err("this card cannot change the group key".into())
    }
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
    /// Group key exchanges under way and the key they carry (a rotation: [`HostAp::rotate_group_key`]).
    rekeys: HashMap<Mac, GroupRekey>,
    pending_gtk: Option<([u8; 16], u8)>,
    log: Box<dyn FnMut(String)>,
    pub stats: Stats,
}

fn text(m: &Mac) -> String {
    m.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
}

impl HostAp {
    /// `gtk` is the group key the caller has already installed on the interface (index `gtk_id`).
    pub fn new(pmk: [u8; 32], bssid: Mac, gtk: [u8; 16], gtk_id: u8, log: Box<dyn FnMut(String)>) -> HostAp {
        HostAp { pmk, bssid, gtk, gtk_id, stations: HashMap::new(), rekeys: HashMap::new(), pending_gtk: None, log, stats: Stats::default() }
    }

    /// How many stations are in a handshake or done.
    pub fn station_count(&self) -> usize {
        self.stations.len()
    }

    /// How many stations finished their handshake (the ones a group key change has to reach).
    pub fn connected_count(&self) -> usize {
        self.stations.values().filter(|a| a.state() == State::Done).count()
    }

    /// The index of the group key in use (1 to 3).
    pub fn group_key_id(&self) -> u8 {
        self.gtk_id
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
        // a station that joins during a rotation gets the new key at once
        let (gtk, gtk_id) = self.pending_gtk.unwrap_or((self.gtk, self.gtk_id));
        self.rekeys.remove(&sta);
        let mut auth = Authenticator::new(AuthConfig { pmk: self.pmk, aa: self.bssid, spa: sta, anonce, gtk, gtk_id, rsn_ie: eapol::rsn_ie_ccmp_psk(), first_replay: 1 });
        let first = auth.start(now_ms);
        self.stations.insert(sta, auth);
        (self.log)(format!("hotspot: handshake started for {}", text(&sta)));
        self.apply(io, sta, first);
    }

    /// The card reported that a station left.
    pub fn station_removed(&mut self, sta: Mac) {
        self.rekeys.remove(&sta);
        if self.stations.remove(&sta).is_some() {
            (self.log)(format!("hotspot: station {} left", text(&sta)));
        }
    }

    /// An EAPOL frame came from `sta` (bytes after the Ethernet header).
    pub fn eapol(&mut self, io: &mut dyn HostIo, sta: Mac, bytes: &[u8], now_ms: u64) {
        if let Some(r) = self.rekeys.get_mut(&sta) {
            let acts = r.on_frame(bytes);
            self.apply_rekey(io, sta, acts, now_ms);
            return;
        }
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
        let macs: Vec<Mac> = self.rekeys.keys().copied().collect();
        for m in macs {
            let acts = self.rekeys.get_mut(&m).map(|r| r.tick(now_ms)).unwrap_or_default();
            self.apply_rekey(io, m, acts, now_ms);
        }
    }

    /// Change the group key (`id` 1 to 3, a different index from the one in use so a station keeps the old key until it has the new
    /// one): every station that finished its handshake gets a group key exchange, a station that does not answer is removed, and
    /// when the last exchange is over the interface switches to the new key. With nobody connected the switch is immediate.
    /// Refused while another rotation is going on, and when the card cannot change the key (the old key stays).
    pub fn rotate_group_key(&mut self, io: &mut dyn HostIo, gtk: [u8; 16], id: u8, now_ms: u64) -> Result<(), String> {
        if self.pending_gtk.is_some() {
            return Err("the group key is already being changed".into());
        }
        if !(1..=3).contains(&id) || id == self.gtk_id {
            return Err("the new group key needs a key index from 1 to 3 that is not the one in use".into());
        }
        let ready: Vec<Mac> = self.stations.iter().filter(|(_, a)| a.state() == State::Done).map(|(m, _)| *m).collect();
        self.pending_gtk = Some((gtk, id));
        for m in ready {
            if let Some(mut r) = self.stations.get(&m).and_then(|a| a.group_rekey(gtk, id)) {
                let acts = r.start(now_ms);
                self.rekeys.insert(m, r);
                self.apply_rekey(io, m, acts, now_ms);
            }
        }
        self.finish_rotation(io);
        Ok(())
    }

    fn apply_rekey(&mut self, io: &mut dyn HostIo, sta: Mac, actions: Vec<RekeyAction>, _now_ms: u64) {
        for a in actions {
            match a {
                RekeyAction::Send(b) => {
                    if let Err(e) = io.send_eapol(sta, &b) {
                        (self.log)(format!("hotspot: group key message to {} failed: {e}", text(&sta)));
                    }
                }
                RekeyAction::Confirmed => {
                    self.rekeys.remove(&sta);
                    (self.log)(format!("hotspot: {} has the new group key", text(&sta)));
                }
                RekeyAction::Failed => {
                    self.rekeys.remove(&sta);
                    self.stations.remove(&sta);
                    self.stats.failed += 1;
                    (self.log)(format!("hotspot: group key exchange with {} timed out, station removed", text(&sta)));
                    let _ = io.deauth(sta, crate::mgmt::REASON_4WAY_TIMEOUT);
                }
            }
        }
        self.finish_rotation(io);
    }

    fn finish_rotation(&mut self, io: &mut dyn HostIo) {
        let Some((gtk, id)) = self.pending_gtk else { return };
        if !self.rekeys.is_empty() {
            return;
        }
        self.pending_gtk = None;
        match io.install_gtk(id, &gtk) {
            Ok(()) => {
                self.gtk = gtk;
                self.gtk_id = id;
                (self.log)(format!("hotspot: group key changed (index {id})"));
            }
            Err(e) => (self.log)(format!("hotspot: group key not switched: {e}")),
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
        gtks: Vec<(u8, [u8; 16])>,
        no_gtk: bool,
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
        fn install_gtk(&mut self, id: u8, gtk: &[u8; 16]) -> Result<(), String> {
            if self.no_gtk {
                return Err("no".into());
            }
            self.gtks.push((id, *gtk));
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

    #[test]
    fn rotating_the_group_key_reaches_every_finished_station_and_then_switches_the_interface() {
        let (mut h, lines) = ap("correct horse");
        let mut io = Fake::default();
        let mut a = client("correct horse", S1);
        let mut b = client("correct horse", S2);
        h.station_added(&mut io, S1, 0);
        h.station_added(&mut io, S2, 0);
        pump(&mut h, &mut io, &mut a, S1, 10);
        pump(&mut h, &mut io, &mut b, S2, 10);
        assert!(h.rotate_group_key(&mut io, [0x66; 16], 1, 20).is_err(), "the index in use is refused");
        assert!(h.rotate_group_key(&mut io, [0x66; 16], 0, 20).is_err());
        h.rotate_group_key(&mut io, [0x66; 16], 2, 20).unwrap();
        assert!(h.rotate_group_key(&mut io, [0x67; 16], 3, 21).is_err(), "one rotation at a time");
        assert!(io.gtks.is_empty(), "nobody has confirmed yet");
        pump(&mut h, &mut io, &mut a, S1, 30);
        assert!(io.gtks.is_empty(), "one station is still missing");
        pump(&mut h, &mut io, &mut b, S2, 30);
        assert_eq!(io.gtks, vec![(2, [0x66; 16])]);
        assert_eq!(a.installed().map(|x| (x.1, x.2)), Some(([0x66; 16], 2)));
        assert_eq!(b.installed().map(|x| (x.1, x.2)), Some(([0x66; 16], 2)));
        assert!(lines.borrow().iter().any(|l| l.contains("group key changed")));
        // a station joining later gets the key now in use
        let mut c = client("correct horse", [0x02, 0, 0, 0, 0, 0x97]);
        h.station_added(&mut io, [0x02, 0, 0, 0, 0, 0x97], 40);
        pump(&mut h, &mut io, &mut c, [0x02, 0, 0, 0, 0, 0x97], 41);
        assert_eq!(c.installed().map(|x| (x.1, x.2)), Some(([0x66; 16], 2)));
    }

    #[test]
    fn a_silent_station_is_removed_during_a_rotation_and_the_others_still_get_the_key() {
        let (mut h, _) = ap("correct horse");
        let mut io = Fake::default();
        let mut a = client("correct horse", S1);
        let mut b = client("correct horse", S2);
        h.station_added(&mut io, S1, 0);
        h.station_added(&mut io, S2, 0);
        pump(&mut h, &mut io, &mut a, S1, 10);
        pump(&mut h, &mut io, &mut b, S2, 10);
        h.rotate_group_key(&mut io, [0x66; 16], 3, 100).unwrap();
        pump(&mut h, &mut io, &mut a, S1, 110);
        let mut t = 100;
        while h.station_count() > 1 && t < 20_000 {
            t += 100;
            h.tick(&mut io, t);
            io.sent.retain(|(m, _)| *m != S2);
        }
        assert_eq!(h.station_count(), 1);
        assert_eq!(io.deauthed.len(), 1);
        assert_eq!(io.deauthed[0].0, S2);
        assert_eq!(io.gtks, vec![(3, [0x66; 16])], "the switch happens once the last exchange is over");
    }

    #[test]
    fn nobody_connected_switches_at_once_and_a_card_that_cannot_change_keeps_the_old_key() {
        let (mut h, lines) = ap("correct horse");
        let mut io = Fake::default();
        h.rotate_group_key(&mut io, [0x66; 16], 2, 0).unwrap();
        assert_eq!(io.gtks, vec![(2, [0x66; 16])]);
        let mut stuck = Fake { no_gtk: true, ..Fake::default() };
        h.rotate_group_key(&mut stuck, [0x68; 16], 3, 5).unwrap();
        assert!(stuck.gtks.is_empty());
        assert!(lines.borrow().iter().any(|l| l.contains("group key not switched")));
        // the failed switch does not leave the host stuck in a rotation, and the old key (index 2) is still the one in use
        assert!(h.rotate_group_key(&mut io, [0x69; 16], 2, 6).is_err());
        h.rotate_group_key(&mut io, [0x69; 16], 1, 7).unwrap();
        assert_eq!(io.gtks.last(), Some(&(1, [0x69; 16])));
    }
}
