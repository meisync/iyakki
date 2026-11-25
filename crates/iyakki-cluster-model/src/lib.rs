//! The cluster registry and the cloud deploy plan as plain data (alpha.8 E7, D-E7-01): what the K8S tile, the AI IDE's Kubernetes panel and the
//! cluster switcher need to know about clusters, without a process, the vault or the network. `iyakki-shared` re-exports every module under its old
//! path (`iyakki_shared::clusters`, `cloudlaunch`, `cloudroute`, `cloudnames`); what needs the machine (`ClusterEntryExt`: the managed k3s layout and the
//! vault) stays there.

pub mod choice;
pub mod cloudlaunch;
pub mod cloudnames;
pub mod cloudroute;
pub mod clusters;
pub mod ids;
pub mod profiles;
