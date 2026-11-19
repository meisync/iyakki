//! Golden file: the wake-phrase weights file (`IYWK`, version 1) as this release writes it (F-CORE-23, charter section 10 step 3).
//! The fixture `tests/compat/alpha3/wake-weights-v1.hex` was recorded once by the ignored test below (its output saved to the file) and is never edited; every later
//! release must read it. Made-up network (seed 1), no recording of anybody.

use iyakki_wake::model::Net;
use iyakki_wake::weights::{fnv1a, load_bytes, save_bytes, Header, DATA_SYNTHETIC};
use std::path::PathBuf;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/compat/alpha3/wake-weights-v1.hex")
}

fn fixture() -> Vec<u8> {
    let t = std::fs::read_to_string(fixture_path()).expect("the golden weights file is in the repository");
    let h: String = t.split_whitespace().collect();
    (0..h.len() / 2).map(|i| u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).unwrap()).collect()
}

fn reseal(mut b: Vec<u8>) -> Vec<u8> {
    let n = b.len();
    let sum = fnv1a(&b[..n - 8]).to_le_bytes();
    b[n - 8..].copy_from_slice(&sum);
    b
}

fn header() -> Header {
    Header { data_kind: DATA_SYNTHETIC, threshold: 0.8, seed: 1, epochs: 3, notes: "alpha.3 golden weights, made-up network".into() }
}

/// Writes the fixture (once, when the release is cut; never edited after).
#[test]
#[ignore = "records the golden file: cargo test -p iyakki-wake --test compat_alpha3 record -- --ignored --nocapture"]
fn record_alpha3_wake_weights() {
    let b = save_bytes(&Net::random(1), &header());
    let hex: String = b.chunks(48).map(|c| c.iter().map(|x| format!("{x:02x}")).collect::<String>() + "\n").collect();
    // printed, then saved by the person who cuts the release (the build host's copy of the repository is not kept)
    println!("BEGIN-FIXTURE\n{hex}END-FIXTURE");
}

#[test]
fn the_weights_file_alpha3_wrote_is_read_with_its_header_and_gives_the_same_network() {
    let b = fixture();
    assert_eq!(&b[..4], b"IYWK");
    let (net, h) = load_bytes(&b).expect("read by the current code");
    assert_eq!(h, header());
    assert_eq!(net.params.len(), (b.len() - 8 - 44 - h.notes.len()) / 4);
    // the network the file holds is the one that was saved: writing it again gives the same bytes
    assert_eq!(save_bytes(&net, &h), b, "the writer and the reader agree on the layout of version 1");
}

#[test]
fn a_flipped_byte_a_cut_file_a_newer_version_and_other_features_are_refused_in_words() {
    let b = fixture();
    let mut flipped = b.clone();
    flipped[200] ^= 1;
    assert!(load_bytes(&flipped).unwrap_err().contains("damaged"));
    assert!(load_bytes(&b[..b.len() - 30]).is_err());
    assert!(load_bytes(&b[..30]).is_err());
    // a newer version, sealed so only the version is wrong
    let mut newer = b.clone();
    newer[4..8].copy_from_slice(&2u32.to_le_bytes());
    assert!(load_bytes(&reseal(newer)).unwrap_err().contains("version 2"));
    // weights made for other features (more mel bands)
    let mut other = b.clone();
    other[8..12].copy_from_slice(&99u32.to_le_bytes());
    assert!(load_bytes(&reseal(other)).unwrap_err().contains("different features"));
    // a number that is not finite
    let mut nan = b.clone();
    let at = 44 + header().notes.len();
    nan[at..at + 4].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(load_bytes(&reseal(nan)).is_err());
}
