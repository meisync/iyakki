//! AES-256-XTS (IEEE 1619-2007) for whole 16-byte multiples, which is all a disk sector is. The tweak follows the kernel's
//! `aes-xts-plain64`: the sector number in 512-byte units, little-endian, in the first 8 bytes of the tweak. With 4096-byte sectors
//! the number therefore advances by 8 per sector, exactly as dm-crypt does with `sector_size:4096`, so data written here reads back
//! through dm-crypt with the same key and the other way round. Ciphertext stealing (lengths that are not a multiple of 16) is not
//! needed for sectors and is refused.

use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use aes::{Aes128, Aes256};

/// The two AES-256 keys, 64 bytes together (the volume key).
pub struct XtsImpl<C> {
    data: C,
    tweak: C,
}

/// The cipher the volumes use: two AES-256 keys.
pub type Xts = XtsImpl<Aes256>;
/// Two AES-128 keys: only for checking the code against the IEEE 1619 vectors, which are AES-128.
pub type Xts128 = XtsImpl<Aes128>;

/// Multiply the tweak by x in GF(2^128), little-endian as XTS defines it.
fn mul_alpha(t: &mut [u8; 16]) {
    let carry = t[15] >> 7;
    for i in (1..16).rev() {
        t[i] = (t[i] << 1) | (t[i - 1] >> 7);
    }
    t[0] = (t[0] << 1) ^ (carry * 0x87);
}

impl XtsImpl<Aes256> {
    pub fn new(key: &[u8; 64]) -> Xts {
        XtsImpl { data: Aes256::new(GenericArray::from_slice(&key[..32])), tweak: Aes256::new(GenericArray::from_slice(&key[32..])) }
    }
}

impl XtsImpl<Aes128> {
    pub fn new_128(key: &[u8; 32]) -> Xts128 {
        XtsImpl { data: Aes128::new(GenericArray::from_slice(&key[..16])), tweak: Aes128::new(GenericArray::from_slice(&key[16..])) }
    }
}

impl<C: BlockEncrypt + BlockDecrypt> XtsImpl<C> {
    fn run(&self, tweak: [u8; 16], data: &mut [u8], encrypt: bool) -> bool {
        if data.is_empty() || data.len() % 16 != 0 {
            return false;
        }
        let mut t = GenericArray::clone_from_slice(&tweak);
        self.tweak.encrypt_block(&mut t);
        let mut tw = [0u8; 16];
        tw.copy_from_slice(&t);
        let mut t = tw;
        for chunk in data.chunks_exact_mut(16) {
            for (c, k) in chunk.iter_mut().zip(&t) {
                *c ^= k;
            }
            let blk = GenericArray::from_mut_slice(chunk);
            if encrypt {
                self.data.encrypt_block(blk);
            } else {
                self.data.decrypt_block(blk);
            }
            for (c, k) in chunk.iter_mut().zip(&t) {
                *c ^= k;
            }
            mul_alpha(&mut t);
        }
        true
    }

    /// Encrypt in place with an explicit 16-byte tweak (the IEEE 1619 vectors use this). `false` if the length is not a multiple of 16.
    pub fn encrypt_with_tweak(&self, tweak: [u8; 16], data: &mut [u8]) -> bool {
        self.run(tweak, data, true)
    }

    pub fn decrypt_with_tweak(&self, tweak: [u8; 16], data: &mut [u8]) -> bool {
        self.run(tweak, data, false)
    }

    /// The `plain64` tweak for the 512-byte sector number `sector512`.
    pub fn plain64(sector512: u64) -> [u8; 16] {
        let mut t = [0u8; 16];
        t[..8].copy_from_slice(&sector512.to_le_bytes());
        t
    }

    /// Encrypt one disk sector (any multiple of 16 bytes) that starts at 512-byte sector `sector512`.
    pub fn encrypt_sector(&self, sector512: u64, data: &mut [u8]) -> bool {
        self.run(Self::plain64(sector512), data, true)
    }

    pub fn decrypt_sector(&self, sector512: u64, data: &mut [u8]) -> bool {
        self.run(Self::plain64(sector512), data, false)
    }
}
