//! The 4-way handshake, authenticator side (the hotspot is the authenticator), WPA2-personal with CCMP (IEEE 802.11i clause 8.5.3).
//!
//! A pure state machine: it is given received EAPOL-Key frames and the current time in milliseconds and answers with actions
//! (frames to send, the key to install, the result). It never touches a socket or a clock. One [`Authenticator`] per client.
//!
//! Rules it keeps: the replay counter goes up with every transmission and a reply must carry the latest value; a frame whose
//! MIC is wrong is ignored (anyone on the air can forge a frame; only the timeout ends a handshake), and when the handshake
//! ends by timeout after bad MICs the reason is "wrong passphrase"; the client's RSN element must be exactly CCMP + PSK; the
//! group key travels wrapped with the KEK; the pairwise key is installed only after message 4 proved the client holds it.

use crate::eapol::{self, KeyFrame, INFO_M1, INFO_M3, KI_ACK, KI_ERROR, KI_INSTALL, KI_MIC, KI_PAIRWISE, KI_REQUEST, KI_SECURE};
use crate::kdf::{derive_ptk, Ptk};
use crate::keywrap;
use crate::Mac;

/// Time to wait for the answer to message 1 or 3 before sending it again.
pub const RETRY_MS: u64 = 1000;
/// Transmissions of one message (the first included) before the client is given up on.
pub const MAX_TRANSMITS: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthConfig {
    pub pmk: [u8; 32],
    /// The hotspot's own address on this BSS (authenticator address).
    pub aa: Mac,
    /// The client's address.
    pub spa: Mac,
    /// Fresh random nonce for this handshake.
    pub anonce: [u8; 32],
    /// The current group key and its index.
    pub gtk: [u8; 16],
    pub gtk_id: u8,
    /// The RSN element advertised in beacons; message 3 repeats it so the client can detect a downgrade.
    pub rsn_ie: Vec<u8>,
    /// Replay counter of message 1; must be higher than any used before with this PMK.
    pub first_replay: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailReason {
    /// Replies came but their MIC never matched: the client uses another passphrase.
    WrongPassphrase,
    /// No (valid) reply in time.
    Timeout,
    /// The client chose a cipher or key management this hotspot does not offer.
    BadRsnIe,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// An EAPOL frame to send to the client.
    Send(Vec<u8>),
    /// Install the pairwise key for the client (CCMP, key index 0).
    InstallPtk { peer: Mac, tk: [u8; 16] },
    /// The port may be opened: the client holds the right keys.
    Authorized,
    /// The handshake is over without success; the caller de-authenticates the client.
    Failed(FailReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    WaitM2,
    WaitM4,
    Done,
    Failed,
}

pub struct Authenticator {
    cfg: AuthConfig,
    state: State,
    replay: u64,
    ptk: Option<Ptk>,
    sent: u32,
    deadline: u64,
    bad_mic: u32,
}

impl Authenticator {
    pub fn new(cfg: AuthConfig) -> Authenticator {
        let replay = cfg.first_replay;
        Authenticator { cfg, state: State::Idle, replay, ptk: None, sent: 0, deadline: 0, bad_mic: 0 }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// How many replies were thrown away for a wrong MIC (for the log; never the key).
    pub fn bad_mic_count(&self) -> u32 {
        self.bad_mic
    }

    /// Send message 1.
    pub fn start(&mut self, now_ms: u64) -> Vec<Action> {
        if self.state != State::Idle {
            return vec![];
        }
        self.state = State::WaitM2;
        vec![self.send_m1(now_ms)]
    }

    fn send_m1(&mut self, now: u64) -> Action {
        self.sent += 1;
        self.deadline = now + RETRY_MS;
        let mut f = KeyFrame::new(INFO_M1, self.replay);
        f.nonce = self.cfg.anonce;
        Action::Send(f.encode())
    }

    fn send_m3(&mut self, now: u64) -> Option<Action> {
        let ptk = self.ptk.as_ref()?;
        self.sent += 1;
        self.deadline = now + RETRY_MS;
        let mut data = self.cfg.rsn_ie.clone();
        data.extend(eapol::gtk_kde(self.cfg.gtk_id, &self.cfg.gtk));
        let wrapped = keywrap::wrap(&ptk.kek, &eapol::pad_key_data(data))?;
        let mut f = KeyFrame::new(INFO_M3, self.replay);
        f.nonce = self.cfg.anonce;
        f.data = wrapped;
        let mut b = f.encode();
        eapol::sign(&ptk.kck, &mut b);
        Some(Action::Send(b))
    }

    fn fail(&mut self, why: FailReason) -> Vec<Action> {
        self.state = State::Failed;
        self.ptk = None;
        vec![Action::Failed(why)]
    }

    /// Handle a frame received from the client (the EAPOL bytes, starting with the version byte).
    pub fn on_frame(&mut self, raw: &[u8], now_ms: u64) -> Vec<Action> {
        if !matches!(self.state, State::WaitM2 | State::WaitM4) {
            return vec![];
        }
        let Ok(f) = KeyFrame::decode(raw) else { return vec![] };
        // frames from a client carry no ack, no install and no error/request (those are for the Michael report and are ignored)
        if f.key_info & eapol::KI_VERSION_MASK != eapol::KEYVER_AES || f.key_info & KI_PAIRWISE == 0 || f.key_info & (KI_ACK | KI_INSTALL | KI_ERROR | KI_REQUEST) != 0 || f.key_info & KI_MIC == 0 {
            return vec![];
        }
        if f.replay != self.replay {
            return vec![];
        }
        match self.state {
            State::WaitM2 if f.key_info & KI_SECURE == 0 => self.on_m2(&f, raw, now_ms),
            State::WaitM4 if f.key_info & KI_SECURE != 0 => self.on_m4(raw),
            _ => vec![],
        }
    }

    fn on_m2(&mut self, f: &KeyFrame, raw: &[u8], now: u64) -> Vec<Action> {
        if f.nonce == [0u8; 32] || f.nonce == self.cfg.anonce {
            return vec![];
        }
        let ptk = derive_ptk(&self.cfg.pmk, &self.cfg.aa, &self.cfg.spa, &self.cfg.anonce, &f.nonce);
        if !eapol::verify_mic(&ptk.kck, raw) {
            self.bad_mic += 1;
            return vec![];
        }
        let ok = eapol::parse_key_data(&f.data).and_then(|k| k.rsn_ie).is_some_and(|ie| eapol::rsn_ie_is_ccmp_psk(&ie));
        if !ok {
            return self.fail(FailReason::BadRsnIe);
        }
        self.ptk = Some(ptk);
        self.replay += 1;
        self.sent = 0;
        self.state = State::WaitM4;
        self.send_m3(now).into_iter().collect()
    }

    fn on_m4(&mut self, raw: &[u8]) -> Vec<Action> {
        let Some(ptk) = self.ptk.as_ref() else { return vec![] };
        if !eapol::verify_mic(&ptk.kck, raw) {
            self.bad_mic += 1;
            return vec![];
        }
        self.state = State::Done;
        let tk = ptk.tk;
        vec![Action::InstallPtk { peer: self.cfg.spa, tk }, Action::Authorized]
    }

    /// Call now and then (every 100 ms is plenty): resends a message that got no answer, or gives up.
    pub fn tick(&mut self, now_ms: u64) -> Vec<Action> {
        if !matches!(self.state, State::WaitM2 | State::WaitM4) || now_ms < self.deadline {
            return vec![];
        }
        if self.sent >= MAX_TRANSMITS {
            let why = if self.bad_mic > 0 { FailReason::WrongPassphrase } else { FailReason::Timeout };
            return self.fail(why);
        }
        self.replay += 1;
        match self.state {
            State::WaitM2 => vec![self.send_m1(now_ms)],
            _ => self.send_m3(now_ms).into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_supplicant::Supplicant;
    use crate::kdf::pmk_from_passphrase;

    const AA: Mac = [0x02, 0, 0, 0, 0, 0x01];
    const SPA: Mac = [0x02, 0, 0, 0, 0, 0x99];

    fn cfg(pass: &str) -> AuthConfig {
        AuthConfig { pmk: pmk_from_passphrase(pass, b"IYAKKI-1A2B"), aa: AA, spa: SPA, anonce: [0x11; 32], gtk: [0x77; 16], gtk_id: 1, rsn_ie: eapol::rsn_ie_ccmp_psk(), first_replay: 1 }
    }

    fn sup(pass: &str) -> Supplicant {
        Supplicant::new(pmk_from_passphrase(pass, b"IYAKKI-1A2B"), AA, SPA, [0x22; 32], eapol::rsn_ie_ccmp_psk())
    }

    fn sent(actions: &[Action]) -> Vec<Vec<u8>> {
        actions.iter().filter_map(|a| if let Action::Send(b) = a { Some(b.clone()) } else { None }).collect()
    }

    #[test]
    fn both_sides_finish_with_the_same_pairwise_key_and_the_group_key_arrives() {
        let mut ap = Authenticator::new(cfg("correct horse"));
        let mut cl = sup("correct horse");
        let m1 = sent(&ap.start(0)).remove(0);
        assert_eq!(KeyFrame::decode(&m1).unwrap().replay, 1);
        let m2 = cl.on_frame(&m1).expect("m2");
        let acts = ap.on_frame(&m2, 10);
        let m3 = sent(&acts).remove(0);
        assert_eq!(ap.state(), State::WaitM4);
        assert_eq!(KeyFrame::decode(&m3).unwrap().replay, 2);
        let m4 = cl.on_frame(&m3).expect("m4");
        let acts = ap.on_frame(&m4, 20);
        assert_eq!(ap.state(), State::Done);
        assert_eq!(acts.len(), 2);
        let Action::InstallPtk { peer, tk } = &acts[0] else { panic!("no key") };
        assert_eq!(*peer, SPA);
        assert_eq!(acts[1], Action::Authorized);
        let (ctk, gtk, gid) = cl.installed().expect("client installed");
        assert_eq!(*tk, ctk);
        assert_eq!((gtk, gid), ([0x77; 16], 1));
        assert!(cl.saw_rsn_ie_match());
    }

    #[test]
    fn a_wrong_passphrase_never_authorizes_and_ends_as_wrong_passphrase_after_the_retries() {
        let mut ap = Authenticator::new(cfg("correct horse"));
        let mut cl = sup("wrong horse");
        let mut m1 = sent(&ap.start(0)).remove(0);
        let mut now = 0;
        let mut result = None;
        for _ in 0..10 {
            let m2 = cl.on_frame(&m1).expect("the client answers with its own key");
            assert!(ap.on_frame(&m2, now + 5).is_empty(), "a bad MIC is ignored");
            assert_ne!(ap.state(), State::Done);
            now += RETRY_MS;
            let acts = ap.tick(now);
            if let Some(Action::Failed(r)) = acts.first() {
                result = Some(*r);
                break;
            }
            m1 = sent(&acts).remove(0);
        }
        assert_eq!(result, Some(FailReason::WrongPassphrase));
        assert_eq!(ap.state(), State::Failed);
        assert!(ap.bad_mic_count() >= 3);
    }

    #[test]
    fn silence_ends_as_timeout_after_four_transmissions_one_second_apart() {
        let mut ap = Authenticator::new(cfg("p"));
        assert_eq!(sent(&ap.start(100)).len(), 1);
        assert!(ap.tick(1099).is_empty());
        let r2 = KeyFrame::decode(&sent(&ap.tick(1100))[0]).unwrap().replay;
        let r3 = KeyFrame::decode(&sent(&ap.tick(2100))[0]).unwrap().replay;
        let r4 = KeyFrame::decode(&sent(&ap.tick(3100))[0]).unwrap().replay;
        assert_eq!((r2, r3, r4), (2, 3, 4), "the replay counter goes up with every transmission");
        assert_eq!(ap.tick(4100), vec![Action::Failed(FailReason::Timeout)]);
        assert!(ap.tick(9000).is_empty());
        assert!(ap.on_frame(&[0; 120], 9000).is_empty());
    }

    #[test]
    fn a_lost_first_message_is_resent_and_the_answer_to_the_resend_is_accepted_but_the_old_one_is_not() {
        let mut ap = Authenticator::new(cfg("pw12345678"));
        let mut cl = sup("pw12345678");
        let m1a = sent(&ap.start(0)).remove(0);
        let stale_m2 = cl.on_frame(&m1a).unwrap(); // the client did get it, but the answer is slow
        let m1b = sent(&ap.tick(1000)).remove(0);
        assert!(ap.on_frame(&stale_m2, 1001).is_empty(), "an answer to the old counter is ignored");
        assert_eq!(ap.bad_mic_count(), 0);
        let mut cl2 = sup("pw12345678");
        let m2 = cl2.on_frame(&m1b).unwrap();
        assert_eq!(sent(&ap.on_frame(&m2, 1002)).len(), 1);
        assert_eq!(ap.state(), State::WaitM4);
    }

    #[test]
    fn message_3_is_resent_with_a_new_counter_and_a_reply_to_the_latest_one_finishes() {
        let mut ap = Authenticator::new(cfg("pw12345678"));
        let mut cl = sup("pw12345678");
        let m1 = sent(&ap.start(0)).remove(0);
        let m2 = cl.on_frame(&m1).unwrap();
        let m3a = sent(&ap.on_frame(&m2, 1)).remove(0);
        let m3b = sent(&ap.tick(1001)).remove(0);
        assert_eq!(KeyFrame::decode(&m3b).unwrap().replay, KeyFrame::decode(&m3a).unwrap().replay + 1);
        let m4 = cl.on_frame(&m3b).unwrap();
        assert!(ap.on_frame(&m4, 1002).contains(&Action::Authorized));
    }

    #[test]
    fn forged_and_replayed_message_4_does_not_open_the_port() {
        let mut ap = Authenticator::new(cfg("pw12345678"));
        let mut cl = sup("pw12345678");
        let m1 = sent(&ap.start(0)).remove(0);
        let m2 = cl.on_frame(&m1).unwrap();
        // a replayed message 2 while waiting for message 4 does nothing
        let m3 = sent(&ap.on_frame(&m2, 1)).remove(0);
        assert!(ap.on_frame(&m2, 2).is_empty());
        // a message 4 from someone who does not hold the key
        let mut forged = KeyFrame::new(eapol::INFO_M4, 2).encode();
        eapol::sign(&[1; 16], &mut forged);
        assert!(ap.on_frame(&forged, 3).is_empty());
        assert_eq!(ap.state(), State::WaitM4);
        // a genuine message 4 with a stale counter
        let mut stale = KeyFrame::new(eapol::INFO_M4, 1).encode();
        eapol::sign(&[1; 16], &mut stale);
        assert!(ap.on_frame(&stale, 4).is_empty());
        let m4 = cl.on_frame(&m3).unwrap();
        assert!(ap.on_frame(&m4, 5).contains(&Action::Authorized));
        // after success nothing more is accepted
        assert!(ap.on_frame(&m4, 6).is_empty());
    }

    #[test]
    fn a_client_that_picked_tkip_or_another_key_management_is_refused() {
        let mut bad_ie = eapol::rsn_ie_ccmp_psk();
        bad_ie[13] = 2; // pairwise TKIP
        let mut ap = Authenticator::new(cfg("pw12345678"));
        let mut cl = Supplicant::new(pmk_from_passphrase("pw12345678", b"IYAKKI-1A2B"), AA, SPA, [0x22; 32], bad_ie);
        let m1 = sent(&ap.start(0)).remove(0);
        let m2 = cl.on_frame(&m1).unwrap();
        assert_eq!(ap.on_frame(&m2, 1), vec![Action::Failed(FailReason::BadRsnIe)]);
    }

    #[test]
    fn the_client_refuses_a_downgraded_or_replayed_message_3() {
        let mut ap = Authenticator::new(cfg("pw12345678"));
        let mut cl = sup("pw12345678");
        let m1 = sent(&ap.start(0)).remove(0);
        let m2 = cl.on_frame(&m1).unwrap();
        let m3 = sent(&ap.on_frame(&m2, 1)).remove(0);
        assert!(cl.on_frame(&m3).is_some());
        assert!(cl.on_frame(&m3).is_none(), "the same counter twice is a replay");
        // an AP whose beacon said something else than message 3 repeats
        let mut other = Authenticator::new(AuthConfig { rsn_ie: { let mut i = eapol::rsn_ie_ccmp_psk(); i[20] = 0x01; i }, ..cfg("pw12345678") });
        let mut cl2 = sup("pw12345678");
        let m1 = sent(&other.start(0)).remove(0);
        let m2 = cl2.on_frame(&m1).unwrap();
        let m3 = sent(&other.on_frame(&m2, 1)).remove(0);
        assert!(cl2.on_frame(&m3).is_none());
        assert!(!cl2.saw_rsn_ie_match());
    }

    #[test]
    fn junk_frames_never_panic_or_authorize_in_any_state() {
        let mut s = 99u32;
        let mut ap = Authenticator::new(cfg("pw12345678"));
        ap.start(0);
        for n in 0..4000usize {
            let len = n % 150;
            let mut v: Vec<u8> = (0..len)
                .map(|_| {
                    s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                    (s >> 24) as u8
                })
                .collect();
            if n % 2 == 0 && v.len() > 8 {
                v[1] = 3;
                v[4] = 2;
            }
            let acts = ap.on_frame(&v, n as u64);
            assert!(!acts.contains(&Action::Authorized));
            let _ = ap.tick(n as u64 * 3);
        }
    }
}
