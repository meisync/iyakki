//! The pure, hardware-free core of the IYAKKI Wi-Fi hotspot (plan: docs/development/HOTSPOT-PLAN.md).
//!
//! Everything here works on byte buffers and plain values: no sockets, no files, no clock of its own (time is passed in).
//! The only code that touches the machine is behind the [`nl80211::NetlinkTransport`] trait, and the real transport is a stub
//! on this build (see `nl80211::UnsupportedTransport`). Own code, no new dependencies: hashing comes from `iyakki-sec`, AES is
//! written here (FIPS-197) because the repository had none.
#![forbid(unsafe_code)]

pub mod aes;
pub mod caps;
pub mod dhcp;
pub mod dns;
pub mod eapol;
pub mod handshake;
pub mod hostap;
pub mod kdf;
pub mod keywrap;
pub mod mgmt;
pub mod nl80211;
pub mod settings;
pub mod subnet;

#[cfg(test)]
mod fake_supplicant;

/// A MAC address.
pub type Mac = [u8; 6];

/// Lower-case hex, used by the tests and by log lines (never for secrets in the product).
pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Decode hex (tests and golden vectors).
pub fn unhex(s: &str) -> Vec<u8> {
    let s: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    s.chunks(2).map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap_or("00"), 16).unwrap_or(0)).collect()
}

/// Constant-time equality for secrets and MACs.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
