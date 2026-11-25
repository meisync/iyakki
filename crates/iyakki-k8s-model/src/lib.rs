//! The cluster explorer's model (F-K8S-*): `kubectl` argument arrays, the `Kube` trait and its fake, bounded lists, details, logs, custom
//! resources, the retry tracker and the plain-English failure texts. No process is started here (the real runner stays in `iyakki-shared`).
//! Split out of `iyakki-shared` so the K8S tile crate can name it without the shared crate (alpha.8 E2, D-S2-01 option a); `iyakki-shared`
//! re-exports every module under the old path.

pub mod b64;
pub mod k8s;
pub mod k8scr;
pub mod k8sexp;
pub mod k8sexptools;
pub mod k8sretry;
pub mod wsexec;
