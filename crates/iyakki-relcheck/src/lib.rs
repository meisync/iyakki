//! Release checks for the repository. One dependency (ruzstd, the pure-Rust zstd decoder); every check is a plain function with unit tests, and the
//! `iyakki-relcheck` program (see `main.rs`) runs them for `./run.sh alpha-check`.

pub mod assets;
pub mod cargolic;
pub mod cpio;
pub mod downloads;
pub mod ext4;
pub mod features;
pub mod fsimage;
pub mod gate;
pub mod imagescan;
pub mod knownissues;
pub mod legalinfo;
pub mod manifest;
pub mod notices;
pub mod profile;
pub mod spdx;
pub mod table;
pub mod links;
pub mod sbom;
pub mod secrets;
pub mod version;

#[cfg(test)]
mod ext4_tests;
#[cfg(test)]
mod fsimage_tests;
#[cfg(test)]
mod fsimage_real_tests;
