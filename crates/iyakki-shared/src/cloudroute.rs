//! The cloud route table: the data and its rules live in `iyakki-cluster-model` (alpha.8 E7) and are re-exported here; the certificate reader
//! (`iyakki_link::certinfo`, which needs the TLS stack) is added here.

pub use iyakki_cluster_model::cloudroute::*;
pub use iyakki_link::certinfo::{name_covers, read_cert, CertInfo};
