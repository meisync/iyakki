use crate::xts::{Xts, Xts128};

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

#[test]
fn ieee_1619_vector_1_all_zero_aes128() {
    let x = Xts128::new_128(&[0u8; 32]);
    let mut d = [0u8; 32];
    assert!(x.encrypt_with_tweak([0; 16], &mut d));
    assert_eq!(d.to_vec(), unhex("917cf69ebd68b2ec9b9fe9a3eadda692cd43d2f59598ed858c02c2652fbf922e"));
    assert!(x.decrypt_with_tweak([0; 16], &mut d));
    assert_eq!(d, [0u8; 32]);
}

#[test]
fn ieee_1619_vector_2_aes128() {
    let mut key = [0x11u8; 32];
    key[16..].fill(0x22);
    let x = Xts128::new_128(&key);
    let mut tweak = [0u8; 16];
    tweak[..5].fill(0x33);
    let mut d = [0x44u8; 32];
    assert!(x.encrypt_with_tweak(tweak, &mut d));
    assert_eq!(d.to_vec(), unhex("c454185e6a16936e39334038acef838bfb186fff7480adc4289382ecd6d394f0"));
}

fn key64() -> [u8; 64] {
    std::array::from_fn(|i| i as u8)
}

// The next vectors were made with OpenSSL (python cryptography 46, AES-256-XTS) from the same inputs.
#[test]
fn aes256_known_answers_from_openssl() {
    let x = Xts::new(&key64());
    let mut d: Vec<u8> = (0..32).collect();
    assert!(x.encrypt_sector(7, &mut d));
    assert_eq!(d, unhex("b2d9289b998ebd6bcc8a6d434711b8af6f974e732a6253c9ed6c74de21b56629"));
    let mut d: Vec<u8> = (0..48).collect();
    assert!(x.encrypt_sector(8, &mut d));
    assert_eq!(d, unhex("d669f5e2d9e798975a7ec16179cd66c78ef2c4b05463fe13a7fcf9894375e2aae34c8fac42e6aa1a8757e4020e69b5c5"));
}

#[test]
fn a_4096_byte_sector_uses_the_512_byte_sector_number_like_dm_crypt() {
    use sha2::{Digest, Sha256};
    let x = Xts::new(&key64());
    let plain: Vec<u8> = (0..4096).map(|i| ((i * 7 + 3) & 255) as u8).collect();
    let mut d = plain.clone();
    // 4096-byte sector number 3 is 512-byte sector 24
    assert!(x.encrypt_sector(24, &mut d));
    assert_eq!(Sha256::digest(&d).iter().map(|b| format!("{b:02x}")).collect::<String>(), "14f159dbbd5e743b18f0ff4b6527a0b95692952b050f0b988aa4a729085e2485");
    assert!(x.decrypt_sector(24, &mut d));
    assert_eq!(d, plain);
}

#[test]
fn other_sectors_and_other_keys_give_other_bytes_and_bad_lengths_are_refused() {
    let x = Xts::new(&key64());
    let (mut a, mut b) = (vec![5u8; 512], vec![5u8; 512]);
    x.encrypt_sector(1, &mut a);
    x.encrypt_sector(2, &mut b);
    assert_ne!(a, b);
    let mut other = key64();
    other[0] ^= 1;
    let mut c = vec![5u8; 512];
    Xts::new(&other).encrypt_sector(1, &mut c);
    assert_ne!(a, c);
    assert!(!x.encrypt_sector(0, &mut [0u8; 17]));
    assert!(!x.encrypt_sector(0, &mut []));
    assert!(!x.decrypt_sector(0, &mut [0u8; 15]));
}

/// Speed on this computer: `cargo test --release -p iyakki-volcrypt -- --ignored --nocapture`. Not part of the normal run.
#[test]
#[ignore]
fn measure_throughput_and_password_time() {
    use std::time::Instant;
    let x = Xts::new(&key64());
    let mut buf = vec![0x5au8; 4096 * 1024];
    let t = Instant::now();
    let mut n = 0u64;
    for round in 0..64u64 {
        for (i, s) in buf.chunks_exact_mut(4096).enumerate() {
            x.encrypt_sector((round * 1024 + i as u64) * 8, s);
        }
        n += buf.len() as u64;
    }
    let s = t.elapsed().as_secs_f64();
    println!("aes-256-xts in this process: {:.0} MiB/s", n as f64 / 1048576.0 / s);
    let t = Instant::now();
    crate::header::derive_kek(b"password", &[1; 32], &crate::header::FLOOR).unwrap();
    println!("argon2id at the floor (64 MiB, 3 passes, 2 lanes): {:.2} s", t.elapsed().as_secs_f64());
}
