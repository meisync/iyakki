//! Release checks for the repository. No dependencies; every check is a plain function with unit tests, and the
//! `iyakki-relcheck` program (see `main.rs`) runs them for `./run.sh alpha-check`.

pub mod features;
pub mod links;
pub mod sbom;
pub mod secrets;
pub mod version;
