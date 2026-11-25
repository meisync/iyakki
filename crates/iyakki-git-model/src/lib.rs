//! The data and the read-only engine of the GIT tile (alpha.8 N1, D-S2-01 option (a)): git host addresses (`gitsite`), the connections and the host's
//! API client (`gitconn`), the repository browser (`gitbrowse`), the plain data of the git jobs (`gitjobs`: operations, phases, specs, the status
//! parse), the vault and audit traits the keys use (`keys`) and the RFC 3339 time parse (`isotime`). `iyakki-shared` re-exports every module under its
//! old path, so nothing that named them before changes; the keys, the host-key store and the job engine stay there.

pub mod gitbrowse;
pub mod gitconn;
pub mod gitjobs;
pub mod gitsite;
pub mod hostkey;
pub mod isotime;
pub mod keys;
