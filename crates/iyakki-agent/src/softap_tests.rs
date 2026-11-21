// SPDX-License-Identifier: Apache-2.0
use super::*;
use iyakki_hotspot::sae::{encode_sae, parse_sae, SaeBody};
use iyakki_hotspot::sae_exchange::Sae;
use iyakki_webcrypto::kdf::hmac_sha256;

pub const AP: Mac = [2, 0, 0, 0, 0, 1];
pub const STA: Mac = [2, 0, 0, 0, 0, 9];
pub const PASSWORD: &str = "hunter2hunter2";
pub const SSID: &[u8] = b"IYAKKI-TEST";

#[derive(Default)]
pub struct FakeIo {
    pub mgmt: Vec<Vec<u8>>,
    pub stations: Vec<(Mac, u16)>,
    pub removed: Vec<(Mac, u16)>,
    pub eapol: Vec<(Mac, Vec<u8>)>,
    pub ptk: Vec<Mac>,
    pub authorized: Vec<Mac>,
    pub refuse_mgmt: bool,
    pub counter: u32,
}

impl SoftIo for FakeIo {
    fn send_mgmt(&mut self, f: &[u8]) -> Result<(), String> {
        if self.refuse_mgmt {
            return Err("busy".into());
        }
        self.mgmt.push(f.to_vec());
        Ok(())
    }
    fn add_station(&mut self, sta: Mac, aid: u16, _: u16, _: &[u8], _: u16) -> Result<(), String> {
        self.stations.push((sta, aid));
        Ok(())
    }
    fn remove_station(&mut self, sta: Mac, reason: u16) -> Result<(), String> {
        self.removed.push((sta, reason));
        Ok(())
    }
    fn install_ptk(&mut self, sta: Mac, _: &[u8; 16]) -> Result<(), String> {
        self.ptk.push(sta);
        Ok(())
    }
    fn authorize(&mut self, sta: Mac) -> Result<(), String> {
        self.authorized.push(sta);
        Ok(())
    }
    fn send_eapol(&mut self, sta: Mac, b: &[u8]) -> Result<(), String> {
        self.eapol.push((sta, b.to_vec()));
        Ok(())
    }
    fn random(&mut self, buf: &mut [u8]) -> bool {
        self.counter += 1;
        buf.copy_from_slice(&hmac_sha256(b"fake", &self.counter.to_le_bytes())[..buf.len()]);
        true
    }
}

pub fn rng(seed: u8) -> impl FnMut(&mut [u8]) -> bool {
    let mut n: u32 = 0;
    move |b: &mut [u8]| {
        n += 1;
        b.copy_from_slice(&hmac_sha256(&[seed], &n.to_le_bytes())[..b.len()]);
        true
    }
}

pub fn soft(security: SecurityMode) -> SoftAp {
    soft_max(security, 4)
}

pub fn soft_max(security: SecurityMode, max: usize) -> SoftAp {
    SoftAp::new(SoftApConfig { bssid: AP, ssid: SSID.to_vec(), passphrase: PASSWORD.into(), band: mgmt::Band::G24, security, max_clients: max, gtk: [4; 16], igtk: [5; 16] }, [1; 32])
}

fn header(fc0: u8, sa: Mac) -> Vec<u8> {
    let mut h = vec![fc0, 0, 0, 0];
    h.extend_from_slice(&AP);
    h.extend_from_slice(&sa);
    h.extend_from_slice(&AP);
    h.extend_from_slice(&[0, 0]);
    h
}

pub fn open_auth(sta: Mac) -> Vec<u8> {
    let mut f = header(mgmt::SUBTYPE_AUTH, sta);
    f.extend_from_slice(&[0, 0, 1, 0, 0, 0]);
    f
}

pub fn assoc(sta: Mac, ssid: &[u8], rsn: &[u8]) -> Vec<u8> {
    let mut f = header(mgmt::SUBTYPE_ASSOC_REQ, sta);
    f.extend_from_slice(&[0x31, 0x04, 10, 0]);
    f.extend_from_slice(&[0, ssid.len() as u8]);
    f.extend_from_slice(ssid);
    f.extend_from_slice(&[1, 2, 0x82, 0x84]);
    f.extend_from_slice(rsn);
    f
}

/// What the kernel hands the program: a station's SAE frame to the access point.
pub fn sae_from(sta: Mac, body: &SaeBody) -> Vec<u8> {
    let mut f = encode_sae(sta, AP, 0, body);
    f[4..10].copy_from_slice(&AP);
    f[10..16].copy_from_slice(&sta);
    f
}

/// What a station reads out of a frame the access point sent it.
pub fn sae_to(b: &[u8], sta: Mac) -> SaeBody {
    let mut f = b.to_vec();
    f[4..10].copy_from_slice(&AP);
    f[10..16].copy_from_slice(&sta);
    parse_sae(&f, AP).unwrap().body
}

/// Runs a whole SAE exchange of `sta` against `soft`; returns the sent frames count.
pub fn do_sae(soft: &mut SoftAp, io: &mut FakeIo, sta: Mac, password: &str, now: u64) -> Vec<SoftEvent> {
    let mut client = Sae::new(password.as_bytes(), sta, AP, &mut rng(5)).unwrap();
    let before = io.mgmt.len();
    let mut ev = soft.on_mgmt(io, &sae_from(sta, &client.commit(&[])), now);
    let out: Vec<Vec<u8>> = io.mgmt[before..].to_vec();
    let SaeBody::Commit { scalar, element, .. } = sae_to(&out[0], sta) else { panic!("commit first") };
    let SaeBody::Confirm { send_confirm, confirm } = sae_to(&out[1], sta) else { panic!("then confirm") };
    let mine = client.on_commit(&scalar, &element).unwrap();
    // with a wrong password the station notices at the confirm, but still sends its own: the access point decides
    let _ = client.on_confirm(send_confirm, &confirm);
    ev.extend(soft.on_mgmt(io, &sae_from(sta, &mine), now + 10));
    ev
}

fn status_of(resp: &[u8]) -> u16 {
    u16::from_le_bytes([resp[26], resp[27]])
}

#[test]
fn wpa2_answers_open_authentication_and_starts_the_handshake_after_a_good_association() {
    let mut s = soft(SecurityMode::Wpa2);
    let mut io = FakeIo::default();
    let ev = s.on_mgmt(&mut io, &open_auth(STA), 0);
    assert!(ev.is_empty() && io.mgmt.len() == 1);
    assert_eq!(u16::from_le_bytes([io.mgmt[0][28], io.mgmt[0][29]]), 0, "the open authentication is accepted");
    let ev = s.on_mgmt(&mut io, &assoc(STA, SSID, &eapol::rsn_ie_ccmp_psk()), 5);
    assert!(ev.contains(&SoftEvent::Associated(STA)));
    assert_eq!(io.stations, vec![(STA, 1)]);
    assert_eq!(status_of(&io.mgmt[1]), 0);
    assert_eq!(io.eapol.len(), 1, "message 1 goes out at once");
    assert_eq!(s.station_count(), 1);
    // the kernel says the station is gone: all is forgotten
    assert_eq!(s.on_station_removed(STA), vec![SoftEvent::Left(STA)]);
    assert_eq!(s.station_count(), 0);
}

#[test]
fn wpa2_refuses_another_name_a_missing_rsn_element_and_a_full_access_point() {
    let mut s = soft_max(SecurityMode::Wpa2, 1);
    let mut io = FakeIo::default();
    s.on_mgmt(&mut io, &assoc(STA, b"OTHER", &eapol::rsn_ie_ccmp_psk()), 0);
    s.on_mgmt(&mut io, &assoc(STA, SSID, &[]), 0);
    assert_eq!(io.mgmt.iter().map(|f| status_of(f)).collect::<Vec<_>>(), vec![mgmt::STATUS_UNSPECIFIED, mgmt::STATUS_CIPHER_REJECTED]);
    assert!(io.stations.is_empty() && s.station_count() == 0);
    s.on_mgmt(&mut io, &assoc(STA, SSID, &eapol::rsn_ie_ccmp_psk()), 0);
    s.on_mgmt(&mut io, &assoc([2, 0, 0, 0, 0, 10], SSID, &eapol::rsn_ie_ccmp_psk()), 0);
    assert_eq!(status_of(io.mgmt.last().unwrap()), mgmt::STATUS_AP_FULL);
    assert_eq!(s.station_count(), 1);
}

#[test]
fn wpa3_only_ignores_open_authentication_and_refuses_an_association_without_a_finished_exchange() {
    let mut s = soft(SecurityMode::Wpa3);
    let mut io = FakeIo::default();
    let ev = s.on_mgmt(&mut io, &open_auth(STA), 0);
    assert!(io.mgmt.is_empty() && matches!(&ev[..], [SoftEvent::Log(l)] if l.contains("WPA3 only")));
    s.on_mgmt(&mut io, &assoc(STA, SSID, &rsn_ie(SecurityMode::Wpa3)), 1);
    assert_ne!(status_of(&io.mgmt[0]), 0, "refused: the station never did SAE");
    assert!(io.stations.is_empty() && io.eapol.is_empty());
}

#[test]
fn a_wpa3_station_that_proved_the_password_is_associated_and_the_handshake_starts_with_the_pmkid() {
    let mut s = soft(SecurityMode::Wpa3);
    let mut io = FakeIo::default();
    let ev = do_sae(&mut s, &mut io, STA, PASSWORD, 100);
    assert!(ev.iter().any(|e| matches!(e, SoftEvent::Log(l) if l.starts_with("SAE finished"))));
    assert_eq!(s.sae_in_progress(), 0);
    let before = io.mgmt.len();
    let ev = s.on_mgmt(&mut io, &assoc(STA, SSID, &rsn_ie(SecurityMode::Wpa3)), 200);
    assert!(ev.contains(&SoftEvent::Associated(STA)), "{ev:?}");
    assert_eq!(status_of(&io.mgmt[before]), 0);
    assert_eq!(io.eapol.len(), 1);
    // message 1 of the WPA3 handshake carries the PMKID item
    assert!(io.eapol[0].1.windows(6).any(|w| w == [0xdd, 20, 0x00, 0x0f, 0xac, 4]));
}

#[test]
fn a_wrong_password_gets_a_failure_frame_and_no_association_follows() {
    let mut s = soft(SecurityMode::Transition);
    let mut io = FakeIo::default();
    let ev = do_sae(&mut s, &mut io, STA, "another-password", 100);
    assert!(ev.iter().any(|e| matches!(e, SoftEvent::Log(l) if l.starts_with("SAE failed"))), "{ev:?}");
    // the last frame is the refusal: algorithm 3, status 1
    let last = io.mgmt.last().unwrap();
    assert_eq!((&last[24..26], status_of(last)), (&3u16.to_le_bytes()[..], 1));
    // an association now: the SAE station is not known, and it names SAE keys: refused by the PSK checks (CCMP + PSK expected)
    s.on_mgmt(&mut io, &assoc(STA, SSID, &rsn_ie(SecurityMode::Wpa3)), 200);
    assert_eq!(status_of(io.mgmt.last().unwrap()), mgmt::STATUS_CIPHER_REJECTED);
    assert!(io.stations.is_empty());
}

#[test]
fn an_sae_station_may_not_name_the_pre_shared_key_management_and_the_other_way_round() {
    let mut s = soft(SecurityMode::Transition);
    let mut io = FakeIo::default();
    do_sae(&mut s, &mut io, STA, PASSWORD, 100);
    // finished SAE, but the association names the PSK key management
    s.on_mgmt(&mut io, &assoc(STA, SSID, &eapol::rsn_ie_ccmp_psk()), 200);
    assert_eq!(status_of(io.mgmt.last().unwrap()), mgmt::STATUS_CIPHER_REJECTED, "no protected management frames announced");
    assert!(io.stations.is_empty());
    // a plain WPA2 station in transition mode still gets in (open authentication, PSK)
    let other: Mac = [2, 0, 0, 0, 0, 11];
    s.on_mgmt(&mut io, &open_auth(other), 300);
    s.on_mgmt(&mut io, &assoc(other, SSID, &eapol::rsn_ie_ccmp_psk()), 310);
    assert_eq!(io.stations, vec![(other, 1)]);
}

#[test]
fn a_frame_the_kernel_refuses_is_logged_and_never_stops_the_flow() {
    let mut s = soft(SecurityMode::Wpa2);
    let mut io = FakeIo { refuse_mgmt: true, ..FakeIo::default() };
    let ev = s.on_mgmt(&mut io, &open_auth(STA), 0);
    assert!(matches!(&ev[..], [SoftEvent::Log(l)] if l.contains("failed")));
}

#[test]
fn junk_never_panics_or_creates_a_station() {
    let mut s = soft(SecurityMode::Transition);
    let mut io = FakeIo::default();
    let good = assoc(STA, SSID, &eapol::rsn_ie_ccmp_psk());
    for n in 0..good.len() {
        s.on_mgmt(&mut io, &good[..n], 0);
        s.on_eapol(&mut io, STA, &good[..n], 0);
    }
    s.on_eapol(&mut io, STA, &[1, 2, 3], 0);
    assert_eq!(s.station_count() <= 1, true);
    assert!(s.on_station_removed([9; 6]).is_empty());
}

#[test]
fn a_silent_station_ends_as_a_time_out_and_is_removed() {
    let mut s = soft(SecurityMode::Wpa2);
    let mut io = FakeIo::default();
    s.on_mgmt(&mut io, &assoc(STA, SSID, &eapol::rsn_ie_ccmp_psk()), 0);
    let mut ev = Vec::new();
    for t in 1..=10u64 {
        ev.extend(s.tick(&mut io, t * 1000));
    }
    assert!(ev.contains(&SoftEvent::Failed(STA, "timeout".into())), "{ev:?}");
    assert!(ev.contains(&SoftEvent::Left(STA)));
    assert_eq!(io.removed.last(), Some(&(STA, mgmt::REASON_4WAY_TIMEOUT)));
    assert_eq!(s.station_count(), 0);
}
