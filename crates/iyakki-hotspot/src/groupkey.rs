//! The group key handshake, authenticator side (IEEE 802.11i clause 8.5.4): changing the group key (GTK) of a client that is already
//! connected, for example when another client left and the broadcast key must change, or on a timer. A pure state machine like
//! [`crate::handshake`]: it is given the client's reply and the time in milliseconds and answers with actions.
//!
//! Two messages: group message 1 (authenticator to client: the new GTK wrapped with the KEK, MIC, ack, secure) and group message 2
//! (client to authenticator: MIC, secure, same replay counter). It is made from a finished 4-way handshake
//! ([`crate::handshake::Authenticator::group_rekey`]) and uses the same keys; the replay counter continues where that handshake
//! ended. A reply whose MIC is wrong is ignored (anyone can forge a frame); a client that does not answer after
//! [`MAX_TRANSMITS`] transmissions is de-authenticated by the caller. The new key is switched on for transmission only
//! after every client has confirmed, which the caller decides (a client that is slow keeps getting the old key until it fails).

use crate::eapol::{self, KeyFrame, KEYVER_AES, KI_ACK, KI_ENCRYPTED, KI_ERROR, KI_INSTALL, KI_MIC, KI_PAIRWISE, KI_REQUEST, KI_SECURE};
use crate::handshake::{MAX_TRANSMITS, RETRY_MS};
use crate::keywrap;

/// Group message 1: authenticator to client (group key type, wrapped GTK, MIC, ack, secure).
pub const INFO_G1: u16 = KEYVER_AES | KI_ACK | KI_MIC | KI_SECURE | KI_ENCRYPTED;
/// Group message 2: client to authenticator (MIC, secure).
pub const INFO_G2: u16 = KEYVER_AES | KI_MIC | KI_SECURE;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RekeyAction {
    /// An EAPOL frame to send to the client.
    Send(Vec<u8>),
    /// The client holds the new group key.
    Confirmed,
    /// No valid answer in time: the caller de-authenticates the client.
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RekeyState {
    Idle,
    WaitG2,
    Done,
    Failed,
}

pub struct GroupRekey {
    kck: [u8; 16],
    kek: [u8; 16],
    gtk: [u8; 16],
    gtk_id: u8,
    replay: u64,
    state: RekeyState,
    sent: u32,
    deadline: u64,
    bad_mic: u32,
    akm: eapol::Akm,
}

impl GroupRekey {
    /// `replay` is the counter of group message 1; it must be higher than any used with these keys before.
    pub fn new(kck: [u8; 16], kek: [u8; 16], gtk: [u8; 16], gtk_id: u8, replay: u64) -> GroupRekey {
        GroupRekey { kck, kek, gtk, gtk_id: gtk_id & 3, replay, state: RekeyState::Idle, sent: 0, deadline: 0, bad_mic: 0, akm: eapol::Akm::Psk }
    }

    /// The key management the keys came from (SAE frames use version 3 and the CMAC MIC).
    pub fn with_akm(mut self, akm: eapol::Akm) -> GroupRekey {
        self.akm = akm;
        self
    }

    pub fn state(&self) -> RekeyState {
        self.state
    }

    /// The highest replay counter used, for the next exchange with the same keys.
    pub fn replay(&self) -> u64 {
        self.replay
    }

    pub fn bad_mic_count(&self) -> u32 {
        self.bad_mic
    }

    /// Send group message 1.
    pub fn start(&mut self, now_ms: u64) -> Vec<RekeyAction> {
        if self.state != RekeyState::Idle {
            return vec![];
        }
        self.state = RekeyState::WaitG2;
        self.send(now_ms).into_iter().collect()
    }

    fn send(&mut self, now: u64) -> Option<RekeyAction> {
        let wrapped = keywrap::wrap(&self.kek, &eapol::pad_key_data(eapol::gtk_kde(self.gtk_id, &self.gtk)))?;
        self.sent += 1;
        self.deadline = now + RETRY_MS;
        let mut f = KeyFrame::new(self.akm.info(INFO_G1) | ((self.gtk_id as u16 & 3) << 4), self.replay);
        f.data = wrapped;
        let mut b = f.encode();
        eapol::sign_for(self.akm, &self.kck, &mut b);
        Some(RekeyAction::Send(b))
    }

    /// Handle a frame from the client.
    pub fn on_frame(&mut self, raw: &[u8]) -> Vec<RekeyAction> {
        if self.state != RekeyState::WaitG2 {
            return vec![];
        }
        let Ok(f) = KeyFrame::decode(raw) else { return vec![] };
        // group message 2: group type, MIC and secure set, no ack, install, error, request, and the counter of message 1
        if f.key_info & eapol::KI_VERSION_MASK != self.akm.keyver() || f.key_info & KI_PAIRWISE != 0 || f.key_info & (KI_ACK | KI_INSTALL | KI_ERROR | KI_REQUEST) != 0 || f.key_info & (KI_MIC | KI_SECURE) != (KI_MIC | KI_SECURE) || f.replay != self.replay {
            return vec![];
        }
        if !eapol::verify_mic_for(self.akm, &self.kck, raw) {
            self.bad_mic += 1;
            return vec![];
        }
        self.state = RekeyState::Done;
        vec![RekeyAction::Confirmed]
    }

    /// Call now and then: resends message 1 with a higher counter, or gives up.
    pub fn tick(&mut self, now_ms: u64) -> Vec<RekeyAction> {
        if self.state != RekeyState::WaitG2 || now_ms < self.deadline {
            return vec![];
        }
        if self.sent >= MAX_TRANSMITS {
            self.state = RekeyState::Failed;
            return vec![RekeyAction::Failed];
        }
        self.replay += 1;
        self.send(now_ms).into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_supplicant::Supplicant;
    use crate::handshake::{Action, AuthConfig, Authenticator};
    use crate::kdf::pmk_from_passphrase;
    use crate::Mac;

    const AA: Mac = [0x02, 0, 0, 0, 0, 0x01];
    const SPA: Mac = [0x02, 0, 0, 0, 0, 0x99];

    fn connected() -> (Authenticator, Supplicant) {
        let pmk = pmk_from_passphrase("correct horse", b"IYAKKI-1A2B");
        let mut ap = Authenticator::new(AuthConfig { pmk, aa: AA, spa: SPA, anonce: [0x11; 32], gtk: [0x77; 16], gtk_id: 1, rsn_ie: eapol::rsn_ie_ccmp_psk(), first_replay: 1, akm: eapol::Akm::Psk, igtk: None, pmkid: None });
        let mut cl = Supplicant::new(pmk, AA, SPA, [0x22; 32], eapol::rsn_ie_ccmp_psk());
        let send = |a: Vec<Action>| a.into_iter().find_map(|x| if let Action::Send(b) = x { Some(b) } else { None });
        let m1 = send(ap.start(0)).unwrap();
        let m2 = cl.on_frame(&m1).unwrap();
        let m3 = send(ap.on_frame(&m2, 1)).unwrap();
        let m4 = cl.on_frame(&m3).unwrap();
        ap.on_frame(&m4, 2);
        (ap, cl)
    }

    fn first(a: Vec<RekeyAction>) -> Vec<u8> {
        a.into_iter().find_map(|x| if let RekeyAction::Send(b) = x { Some(b) } else { None }).expect("a frame")
    }

    #[test]
    fn a_connected_client_gets_the_new_group_key_and_confirms_it() {
        let (ap, mut cl) = connected();
        let mut g = ap.group_rekey([0x55; 16], 2).expect("keys after the handshake");
        let g1 = first(g.start(100));
        let f = KeyFrame::decode(&g1).unwrap();
        assert_eq!(f.key_info & KI_PAIRWISE, 0, "a group key message is not a pairwise one");
        assert_eq!(f.key_info >> 4 & 3, 2, "the key id is in the key info");
        assert!(f.replay > 2, "the counter continues after the 4-way handshake: {}", f.replay);
        let g2 = cl.on_frame(&g1).expect("the client answers");
        assert_eq!(cl.installed().map(|(_, gtk, id)| (gtk, id)), Some(([0x55; 16], 2)));
        assert_eq!(g.on_frame(&g2), vec![RekeyAction::Confirmed]);
        assert_eq!(g.state(), RekeyState::Done);
        assert!(g.tick(10_000).is_empty());
    }

    #[test]
    fn a_lost_message_is_resent_with_a_higher_counter_and_silence_ends_in_failure() {
        let (ap, mut cl) = connected();
        let mut g = ap.group_rekey([0x55; 16], 0).unwrap();
        let g1 = first(g.start(0));
        assert!(g.tick(999).is_empty());
        let again = first(g.tick(1000));
        assert!(KeyFrame::decode(&again).unwrap().replay > KeyFrame::decode(&g1).unwrap().replay);
        // the answer to the first copy is no longer accepted, the answer to the resend is
        let g2_old = cl.on_frame(&g1).expect("old copy answered");
        assert!(g.on_frame(&g2_old).is_empty());
        assert!(cl.on_frame(&g1).is_none(), "the client refuses a replayed message");
        let g2 = cl.on_frame(&again).expect("resend answered");
        assert_eq!(g.on_frame(&g2), vec![RekeyAction::Confirmed]);

        let mut quiet = ap.group_rekey([0x56; 16], 1).unwrap();
        quiet.start(0);
        let mut t = 0;
        let mut sent = 1;
        loop {
            t += RETRY_MS;
            let a = quiet.tick(t);
            if a == vec![RekeyAction::Failed] {
                break;
            }
            sent += a.len() as u32;
            assert!(t < 10_000);
        }
        assert_eq!((sent, quiet.state()), (MAX_TRANSMITS, RekeyState::Failed));
    }

    #[test]
    fn forged_replies_never_confirm() {
        let (ap, mut cl) = connected();
        let mut g = ap.group_rekey([0x55; 16], 3).unwrap();
        let g1 = first(g.start(0));
        let good = cl.on_frame(&g1).unwrap();
        let mut bad_mic = good.clone();
        bad_mic[90] ^= 1;
        assert!(g.on_frame(&bad_mic).is_empty());
        assert_eq!(g.bad_mic_count(), 1);
        // wrong counter, an ack, a pairwise bit, junk
        let mut wrong_counter = KeyFrame::decode(&good).unwrap();
        wrong_counter.replay += 5;
        assert!(g.on_frame(&wrong_counter.encode()).is_empty());
        for flags in [KI_ACK, KI_PAIRWISE, KI_INSTALL, KI_ERROR, KI_REQUEST] {
            let mut f = KeyFrame::decode(&good).unwrap();
            f.key_info |= flags;
            assert!(g.on_frame(&f.encode()).is_empty(), "{flags:#x}");
        }
        assert!(g.on_frame(&[]).is_empty() && g.on_frame(&[0u8; 200]).is_empty());
        assert_eq!(g.state(), RekeyState::WaitG2);
        assert_eq!(g.on_frame(&good), vec![RekeyAction::Confirmed]);
    }

    #[test]
    fn a_handshake_that_did_not_finish_gives_no_rekey_and_a_stranger_cannot_read_the_key() {
        let pmk = pmk_from_passphrase("correct horse", b"IYAKKI-1A2B");
        let ap = Authenticator::new(AuthConfig { pmk, aa: AA, spa: SPA, anonce: [0x11; 32], gtk: [0x77; 16], gtk_id: 1, rsn_ie: eapol::rsn_ie_ccmp_psk(), first_replay: 1, akm: eapol::Akm::Psk, igtk: None, pmkid: None });
        assert!(ap.group_rekey([1; 16], 1).is_none());
        let (ap, _) = connected();
        let mut g = ap.group_rekey([0x55; 16], 1).unwrap();
        let g1 = first(g.start(0));
        // a client with another PTK (another passphrase) cannot use the frame
        let mut other = Supplicant::new(pmk_from_passphrase("other", b"IYAKKI-1A2B"), AA, SPA, [0x33; 32], eapol::rsn_ie_ccmp_psk());
        assert!(other.on_frame(&g1).is_none());
        assert!(!g1.windows(16).any(|w| w == [0x55u8; 16]), "the key is not in the clear");
    }
}
