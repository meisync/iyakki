//! The cluster explorer's model lives in `iyakki-k8s-model::k8s` (alpha.8 E2, D-S2-01 option a) and is re-exported here under the old path; only
//! the real `kubectl` runner, which starts a program through [`crate::wsexec`], stays in this crate.

use std::time::Duration;

pub use iyakki_k8s_model::k8s::*;
use crate::wsexec::Out;

/// The real thing.
pub struct Kubectl;

impl Kube for Kubectl {
    fn run(&self, args: &[String]) -> Out {
        let mut argv = vec!["kubectl".to_string()];
        argv.extend(args.iter().cloned());
        let cwd = std::env::temp_dir();
        crate::wsexec::run_argv(&cwd, &argv, &[], Duration::from_secs(REQUEST_TIMEOUT_SECS + 6), MAX_OUTPUT)
    }
}
