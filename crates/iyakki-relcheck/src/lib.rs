//! Release checks for the repository. No dependencies; every check is a plain function with unit tests, and the
//! `iyakki-relcheck` program (see `main.rs`) runs them for `./run.sh alpha-check`.

pub mod assets;
pub mod cargolic;
pub mod cpio;
pub mod downloads;
pub mod features;
pub mod gate;
pub mod imagescan;
pub mod legalinfo;
pub mod notices;
pub mod profile;
pub mod spdx;
pub mod table;
pub mod links;
pub mod sbom;
pub mod secrets;
pub mod version;
