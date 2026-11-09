//! Data-volume encryption, slice 1 (F-STORE-05; design in docs/development/DATA-VOLUME-ENCRYPTION.md).
//!
//! Pure code, no device: the volume header with key slots (Argon2id from a password, or a printed recovery key, wrapping one random
//! 512-bit volume key with AES key wrap), key-slot rotation, AES-256-XTS sector crypto that matches the kernel's `aes-xts-plain64`
//! (so a later slice can hand the same key to dm-crypt), and a container that works on any byte store (a file, memory) with two
//! header copies so a power cut during a change never loses the volume. Nothing here opens a disk, calls dm-crypt or needs root.

pub mod container;
pub mod header;
pub mod kw;
pub mod mac;
pub mod recovery;
pub mod xts;

#[cfg(test)]
mod container_tests;
#[cfg(test)]
mod header_tests;
#[cfg(test)]
mod kw_tests;
#[cfg(test)]
mod mac_tests;
#[cfg(test)]
mod recovery_tests;
#[cfg(test)]
mod xts_tests;

/// What can go wrong, each with a sentence a person can read.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VolError {
    #[error("this is not an IYAKKI encrypted volume (the start of the drive does not match)")]
    NotOurs,
    #[error("the volume header is too short to be read")]
    Truncated,
    #[error("the volume header is damaged (its checksum does not match); if the drive has a second header copy it will be used")]
    Damaged,
    #[error("this volume was made by a newer IYAKKI (format {0}); update IYAKKI to open it")]
    Newer(u32),
    #[error("the volume header holds values that make no sense: {0}")]
    Invalid(&'static str),
    #[error("the password or recovery key is wrong")]
    WrongSecret,
    #[error("the volume header was changed by someone without the key; do not trust this drive")]
    Tampered,
    #[error("the drive asks for {0} MiB of memory to check a password, more than this computer allows")]
    KdfTooBig(u32),
    #[error("there is no such key slot")]
    NoSuchSlot,
    #[error("this is the last way to open the volume; it cannot be removed")]
    LastSlot,
    #[error("all key slots are in use; remove one first")]
    SlotsFull,
    #[error("that key slot cannot be used for this (a recovery key slot has no password)")]
    WrongSlotKind,
    #[error("the request does not fit the volume: {0}")]
    BadRequest(&'static str),
    #[error("the recovery key is not written correctly: {0}")]
    BadRecoveryKey(&'static str),
    #[error("the system could not supply random numbers")]
    Entropy,
    #[error("reading or writing the drive failed: {0}")]
    Io(String),
}

/// A source of random bytes; the real one asks the operating system, tests use a fixed sequence.
pub trait Random {
    fn fill(&mut self, buf: &mut [u8]) -> Result<(), VolError>;
}

/// Random bytes from the operating system (`getrandom`).
pub struct OsRandom;

impl Random for OsRandom {
    fn fill(&mut self, buf: &mut [u8]) -> Result<(), VolError> {
        iyakki_sec::random_fill(buf).map_err(|_| VolError::Entropy)
    }
}
