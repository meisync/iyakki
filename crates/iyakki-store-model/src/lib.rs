//! The pure STORE model a tile may name (alpha.10 AP2, D-S2-01 option a): packages and skills, the catalog, trust, installed list, lifecycle,
//! organize, ratings, licence and update-check rules. `iyakki_shared` re-exports every module under its old path.

pub mod skills;
pub mod pkg;
pub mod wtool;
pub mod store_manifest;
pub mod store_trust;
pub mod store_revocation;
pub mod store_catalog;
pub mod store_core;
pub mod store_bundle;
pub mod store_install;
pub mod store_installed;
pub mod store_lifecycle;
pub mod store_organize;
pub mod store_ratings;
pub mod store_updatecheck;
pub mod store_license;
pub mod store_licencelink;
pub mod store_private;
