//! Matrix device verification by short authentication string (SAS, `m.sas.v1`, F-MSG-08), written from the public specification
//! ("Key verification framework", section "m.sas.v1"): the key agreement `curve25519-hkdf-sha256`, the commitment the accepting side
//! sends before it shows its key, the emoji and decimal strings both people compare, and the MACs (`hkdf-hmac-sha256.v2`) that then
//! vouch for the device keys. Nothing here sends a message: the caller carries the `m.key.verification.*` events and shows the strings.
//!
//! Proof limit: there are no public test vectors here, so the tests check both sides against each other, the derivation steps against an
//! independent reading in the test, and every kind of tampering; a difference from Element that the specification does not show would not
//! be caught (Manual: a real verification against a homeserver and a client).

use x25519_dalek::{x25519, X25519_BASEPOINT_BYTES};

use crate::b64;
use crate::kdf::{hkdf, hmac_sha256};

/// The 64 emoji of the specification, in order, each with its English name (the names are what the specification lists for translation).
pub const EMOJI: [(&str, &str); 64] = [
    ("\u{1F436}", "Dog"), ("\u{1F431}", "Cat"), ("\u{1F981}", "Lion"), ("\u{1F40E}", "Horse"), ("\u{1F984}", "Unicorn"), ("\u{1F437}", "Pig"), ("\u{1F418}", "Elephant"), ("\u{1F430}", "Rabbit"),
    ("\u{1F43C}", "Panda"), ("\u{1F413}", "Rooster"), ("\u{1F427}", "Penguin"), ("\u{1F422}", "Turtle"), ("\u{1F41F}", "Fish"), ("\u{1F419}", "Octopus"), ("\u{1F98B}", "Butterfly"), ("\u{1F337}", "Flower"),
    ("\u{1F333}", "Tree"), ("\u{1F335}", "Cactus"), ("\u{1F344}", "Mushroom"), ("\u{1F30F}", "Globe"), ("\u{1F319}", "Moon"), ("\u{2601}\u{FE0F}", "Cloud"), ("\u{1F525}", "Fire"), ("\u{1F34C}", "Banana"),
    ("\u{1F34E}", "Apple"), ("\u{1F353}", "Strawberry"), ("\u{1F33D}", "Corn"), ("\u{1F355}", "Pizza"), ("\u{1F382}", "Cake"), ("\u{2764}\u{FE0F}", "Heart"), ("\u{1F600}", "Smiley"), ("\u{1F916}", "Robot"),
    ("\u{1F3A9}", "Hat"), ("\u{1F453}", "Glasses"), ("\u{1F527}", "Spanner"), ("\u{1F385}", "Santa"), ("\u{1F44D}", "Thumbs Up"), ("\u{2602}\u{FE0F}", "Umbrella"), ("\u{231B}", "Hourglass"), ("\u{23F0}", "Clock"),
    ("\u{1F381}", "Gift"), ("\u{1F4A1}", "Light Bulb"), ("\u{1F4D5}", "Book"), ("\u{270F}\u{FE0F}", "Pencil"), ("\u{1F4CE}", "Paperclip"), ("\u{2702}\u{FE0F}", "Scissors"), ("\u{1F512}", "Lock"), ("\u{1F511}", "Key"),
    ("\u{1F528}", "Hammer"), ("\u{260E}\u{FE0F}", "Telephone"), ("\u{1F3C1}", "Flag"), ("\u{1F682}", "Train"), ("\u{1F6B2}", "Bicycle"), ("\u{2708}\u{FE0F}", "Aeroplane"), ("\u{1F680}", "Rocket"), ("\u{1F3C6}", "Trophy"),
    ("\u{26BD}", "Ball"), ("\u{1F3B8}", "Guitar"), ("\u{1F3BA}", "Trumpet"), ("\u{1F514}", "Bell"), ("\u{2693}", "Anchor"), ("\u{1F3A7}", "Headphones"), ("\u{1F4C1}", "Folder"), ("\u{1F4CC}", "Pin"),
];

/// Who is who in one verification: the side that sent `m.key.verification.start` and the side that accepted it, and the transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parties {
    pub start_user: String,
    pub start_device: String,
    pub accept_user: String,
    pub accept_device: String,
    pub transaction: String,
}

/// One side of a SAS verification.
pub struct Sas {
    secret: [u8; 32],
    public: [u8; 32],
    their: Option<[u8; 32]>,
    parties: Parties,
    /// Whether this side sent the `start` (its key goes first in the strings' derivation).
    started: bool,
}

impl Sas {
    /// A fresh ephemeral key for this verification. `started` says whether this side sent `m.key.verification.start`.
    pub fn new(parties: Parties, started: bool) -> Result<Sas, String> {
        let mut secret = [0u8; 32];
        crate::random(&mut secret)?;
        Ok(Sas::with_secret(secret, parties, started))
    }

    /// The same with the secret given (tests).
    pub fn with_secret(secret: [u8; 32], parties: Parties, started: bool) -> Sas {
        Sas { public: x25519(secret, X25519_BASEPOINT_BYTES), secret, their: None, parties, started }
    }

    /// Our ephemeral public key, unpadded base64 (the `key` of `m.key.verification.key`).
    pub fn public_b64(&self) -> String {
        b64::std_nopad(&self.public)
    }

    /// The commitment the ACCEPTING side puts in `m.key.verification.accept` before it shows its key: unpadded base64 of
    /// SHA-256 over its public key (base64) followed by the canonical JSON of the `start` content.
    pub fn commitment(&self, start_content_canonical: &str) -> String {
        commitment_of(&self.public_b64(), start_content_canonical)
    }

    /// Take the other side's key from `m.key.verification.key`. A key of low order is refused. When this side started, the accepting
    /// side's key must match the commitment it sent first (`commitment` is that value): a mismatch ends the verification.
    pub fn set_their_key(&mut self, key_b64: &str, expected_commitment: Option<(&str, &str)>) -> Result<(), String> {
        let k: [u8; 32] = b64::decode(key_b64)?.try_into().map_err(|_| "a Curve25519 key is 32 bytes".to_string())?;
        if let Some((commitment, start_content_canonical)) = expected_commitment {
            if !crate::ct_eq(commitment_of(key_b64.trim_end_matches('='), start_content_canonical).as_bytes(), commitment.as_bytes()) {
                return Err("the key does not match what the other device committed to: someone may be in the middle".into());
            }
        }
        if x25519(self.secret, k) == [0u8; 32] {
            return Err("the key is of low order".into());
        }
        self.their = Some(k);
        Ok(())
    }

    fn shared(&self) -> Result<[u8; 32], String> {
        x25519(self.secret, self.their.ok_or("the other side's key has not come yet")?).pipe_ok()
    }

    fn keys_in_order(&self) -> Result<(String, String), String> {
        let (mine, theirs) = (self.public_b64(), b64::std_nopad(&self.their.ok_or("the other side's key has not come yet")?));
        Ok(if self.started { (mine, theirs) } else { (theirs, mine) })
    }

    /// The six bytes the strings are made from: HKDF of the shared secret with the specification's info string (the starting side's user,
    /// device and key first, then the accepting side's, then the transaction).
    pub fn sas_bytes(&self) -> Result<[u8; 6], String> {
        let (start_key, accept_key) = self.keys_in_order()?;
        let p = &self.parties;
        let info = format!("MATRIX_KEY_VERIFICATION_SAS|{}|{}|{}|{}|{}|{}|{}", p.start_user, p.start_device, start_key, p.accept_user, p.accept_device, accept_key, p.transaction);
        let v = hkdf(&[], &self.shared()?, info.as_bytes(), 6);
        Ok(v.try_into().expect("six bytes"))
    }

    /// The seven emoji both people compare: 42 bits of the six bytes, six at a time.
    pub fn emoji(&self) -> Result<[(&'static str, &'static str); 7], String> {
        let b = self.sas_bytes()?;
        let n = b.iter().fold(0u64, |a, x| a << 8 | u64::from(*x)) >> 6;
        let mut out = [("", ""); 7];
        for (i, o) in out.iter_mut().enumerate() {
            *o = EMOJI[((n >> (6 * (6 - i))) & 63) as usize];
        }
        Ok(out)
    }

    /// The three four-digit numbers both people may compare instead: 13 bits each of the first five bytes, plus 1000.
    pub fn decimals(&self) -> Result<[u16; 3], String> {
        let b = self.sas_bytes()?;
        let (b0, b1, b2, b3, b4) = (u16::from(b[0]), u16::from(b[1]), u16::from(b[2]), u16::from(b[3]), u16::from(b[4]));
        Ok([(b0 << 5 | b1 >> 3) + 1000, ((b1 & 7) << 10 | b2 << 2 | b3 >> 6) + 1000, ((b3 & 0x3f) << 7 | b4 >> 1) + 1000])
    }

    /// The MAC of one value for `key_id` (a device key as `ed25519:DEVICE`, or `KEY_IDS` for the list), as this side SENDS it:
    /// the info names the sending device first, then the receiving one, the transaction and the key id.
    pub fn mac(&self, sending_is_us: bool, key_id: &str, value: &str) -> Result<String, String> {
        Ok(b64::std_nopad(&hmac_sha256(&self.mac_key(sending_is_us, key_id)?, value.as_bytes())))
    }

    fn mac_key(&self, sending_is_us: bool, key_id: &str) -> Result<[u8; 32], String> {
        let p = &self.parties;
        let we_are_start = self.started;
        // who sends: us or them; who that is in the start/accept roles
        let sender_is_start = sending_is_us == we_are_start;
        let ((su, sd), (ru, rd)) = if sender_is_start { ((&p.start_user, &p.start_device), (&p.accept_user, &p.accept_device)) } else { ((&p.accept_user, &p.accept_device), (&p.start_user, &p.start_device)) };
        let info = format!("MATRIX_KEY_VERIFICATION_MAC{su}{sd}{ru}{rd}{}{key_id}", p.transaction);
        Ok(hkdf(&[], &self.shared()?, info.as_bytes(), 32).try_into().expect("32 bytes"))
    }

    /// The `m.key.verification.mac` content this side sends once its person said the strings match: a MAC for each of our device keys
    /// (`(key id, key)`), and a MAC over the sorted, comma-joined list of the key ids.
    pub fn mac_message(&self, keys: &[(String, String)]) -> Result<(Vec<(String, String)>, String), String> {
        let mut macs = Vec::new();
        for (id, key) in keys {
            macs.push((id.clone(), self.mac(true, id, key)?));
        }
        let mut ids: Vec<&str> = keys.iter().map(|(i, _)| i.as_str()).collect();
        ids.sort();
        Ok((macs, self.mac(true, "KEY_IDS", &ids.join(","))?))
    }

    /// Check the other side's `m.key.verification.mac`: every key it names must be one WE know for that device (`known`: key id and key),
    /// each MAC must match, and the list MAC must match the sorted ids. Anything else leaves the device unverified, with a reason.
    pub fn check_mac(&self, macs: &[(String, String)], keys_mac: &str, known: &[(String, String)]) -> Result<(), String> {
        let mut ids: Vec<&str> = macs.iter().map(|(i, _)| i.as_str()).collect();
        ids.sort();
        if !crate::ct_eq(self.mac(false, "KEY_IDS", &ids.join(","))?.as_bytes(), keys_mac.as_bytes()) {
            return Err("the list of keys was not vouched for with the shared secret".into());
        }
        for (id, got) in macs {
            let key = known.iter().find(|(i, _)| i == id).map(|(_, k)| k).ok_or_else(|| format!("the other device vouched for a key ({id}) we do not know"))?;
            if !crate::ct_eq(self.mac(false, id, key)?.as_bytes(), got.as_bytes()) {
                return Err(format!("the MAC of {id} is wrong: the key is not the one the other device holds"));
            }
        }
        if macs.is_empty() {
            return Err("the other device vouched for no key".into());
        }
        Ok(())
    }
}

fn commitment_of(key_b64: &str, start_content_canonical: &str) -> String {
    let d = ring::digest::digest(&ring::digest::SHA256, format!("{key_b64}{start_content_canonical}").as_bytes());
    b64::std_nopad(d.as_ref())
}

trait PipeOk {
    fn pipe_ok(self) -> Result<[u8; 32], String>;
}
impl PipeOk for [u8; 32] {
    fn pipe_ok(self) -> Result<[u8; 32], String> {
        if self == [0u8; 32] { Err("the key is of low order".into()) } else { Ok(self) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parties() -> Parties {
        Parties { start_user: "@alice:example.org".into(), start_device: "ALICEDEV".into(), accept_user: "@bob:example.org".into(), accept_device: "BOBDEV".into(), transaction: "txn-1".into() }
    }

    const START: &str = r#"{"from_device":"ALICEDEV","method":"m.sas.v1","transaction_id":"txn-1"}"#;

    fn pair() -> (Sas, Sas) {
        let mut a = Sas::with_secret([1; 32], parties(), true);
        let mut b = Sas::with_secret([2; 32], parties(), false);
        let commit = b.commitment(START);
        // the accepting side commits first; the starter sends its key; then the accepting side shows its own
        b.set_their_key(&a.public_b64(), None).unwrap();
        a.set_their_key(&b.public_b64(), Some((&commit, START))).unwrap();
        (a, b)
    }

    #[test]
    fn both_sides_get_the_same_strings_and_a_third_party_does_not() {
        let (a, b) = pair();
        assert_eq!(a.sas_bytes().unwrap(), b.sas_bytes().unwrap());
        assert_eq!(a.emoji().unwrap(), b.emoji().unwrap());
        assert_eq!(a.decimals().unwrap(), b.decimals().unwrap());
        assert!(a.decimals().unwrap().iter().all(|d| (1000..=9191).contains(d)), "{:?}", a.decimals());
        // someone in the middle: Alice talks to Mallory, Bob to Mallory: neither pair of strings matches
        let mut m1 = Sas::with_secret([9; 32], parties(), false);
        let mut a2 = Sas::with_secret([1; 32], parties(), true);
        a2.set_their_key(&m1.public_b64(), None).unwrap();
        m1.set_their_key(&a2.public_b64(), None).unwrap();
        assert_ne!(a2.emoji().unwrap(), b.emoji().unwrap());
        assert_ne!(a2.sas_bytes().unwrap(), a.sas_bytes().unwrap());
    }

    #[test]
    fn the_strings_follow_the_derivation_of_the_specification_step_by_step() {
        let (a, _) = pair();
        // an independent reading: the info string, HKDF with no salt, six bytes, 6-bit and 13-bit groups
        let (ka, kb) = (a.public_b64(), b64::std_nopad(&a.their.unwrap()));
        let info = format!("MATRIX_KEY_VERIFICATION_SAS|@alice:example.org|ALICEDEV|{ka}|@bob:example.org|BOBDEV|{kb}|txn-1");
        let shared = x25519([1; 32], a.their.unwrap());
        let bytes = hkdf(&[], &shared, info.as_bytes(), 6);
        assert_eq!(bytes, a.sas_bytes().unwrap().to_vec());
        let bits: String = bytes.iter().map(|b| format!("{b:08b}")).collect();
        let idx: Vec<usize> = (0..7).map(|i| usize::from_str_radix(&bits[i * 6..i * 6 + 6], 2).unwrap()).collect();
        assert_eq!(a.emoji().unwrap().iter().map(|e| e.1).collect::<Vec<_>>(), idx.iter().map(|i| EMOJI[*i].1).collect::<Vec<_>>());
        let dec: Vec<u16> = (0..3).map(|i| u16::from_str_radix(&bits[i * 13..i * 13 + 13], 2).unwrap() + 1000).collect();
        assert_eq!(a.decimals().unwrap().to_vec(), dec);
        assert_eq!(EMOJI.len(), 64);
        let mut names: Vec<&str> = EMOJI.iter().map(|e| e.1).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 64, "no emoji twice");
    }

    #[test]
    fn a_key_that_is_not_the_committed_one_a_low_order_key_and_a_missing_key_are_refused() {
        let mut a = Sas::with_secret([1; 32], parties(), true);
        let b = Sas::with_secret([2; 32], parties(), false);
        let commit = b.commitment(START);
        let evil = Sas::with_secret([7; 32], parties(), false);
        assert!(a.set_their_key(&evil.public_b64(), Some((&commit, START))).unwrap_err().contains("committed"), "a swapped key");
        assert!(a.set_their_key(&b.public_b64(), Some((&commit, r#"{"other":"start"}"#))).unwrap_err().contains("committed"), "a changed start message");
        assert!(a.set_their_key(&b64::std_nopad(&[0u8; 32]), None).unwrap_err().contains("low order"));
        assert!(a.set_their_key("short", None).is_err());
        assert!(a.sas_bytes().unwrap_err().contains("not come yet"));
        a.set_their_key(&b.public_b64(), Some((&commit, START))).unwrap();
        assert!(a.sas_bytes().is_ok());
    }

    #[test]
    fn the_macs_vouch_for_the_device_keys_in_both_directions_and_every_tampering_is_caught() {
        let (a, b) = pair();
        let a_keys = vec![("ed25519:ALICEDEV".to_string(), "AAAA-alice-key".to_string())];
        let b_keys = vec![("ed25519:BOBDEV".to_string(), "BBBB-bob-key".to_string()), ("ed25519:MASTER".to_string(), "MMMM-master".to_string())];
        let (a_macs, a_list) = a.mac_message(&a_keys).unwrap();
        let (b_macs, b_list) = b.mac_message(&b_keys).unwrap();
        // each side checks the other's against the keys it knows from the homeserver
        b.check_mac(&a_macs, &a_list, &a_keys).unwrap();
        a.check_mac(&b_macs, &b_list, &b_keys).unwrap();
        // a different key for the device, an unknown key, a damaged MAC, a changed list, no key at all, one side's MAC read as the other's
        assert!(b.check_mac(&a_macs, &a_list, &[("ed25519:ALICEDEV".into(), "EVIL".into())]).unwrap_err().contains("wrong"));
        assert!(b.check_mac(&a_macs, &a_list, &[]).unwrap_err().contains("we do not know"));
        let mut bad = a_macs.clone();
        bad[0].1 = b64::std_nopad(&[0u8; 32]);
        assert!(b.check_mac(&bad, &a_list, &a_keys).unwrap_err().contains("wrong"));
        assert!(b.check_mac(&a_macs, &b_list, &a_keys).unwrap_err().contains("list"));
        assert!(b.check_mac(&[], &a_list, &a_keys).is_err());
        assert!(a.check_mac(&a_macs, &a_list, &a_keys).is_err(), "our own MAC does not pass as the other's");
        // another transaction gives other MACs
        let other = Parties { transaction: "txn-2".into(), ..parties() };
        let mut a2 = Sas::with_secret([1; 32], other, true);
        a2.set_their_key(&b.public_b64(), None).unwrap();
        assert_ne!(a2.mac_message(&a_keys).unwrap().1, a_list);
    }
}
