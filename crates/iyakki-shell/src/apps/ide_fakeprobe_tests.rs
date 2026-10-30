//! Test-only: what the unit tests' fake runner answers for "is the registry up". The real probe asks the network, which a unit test never does.

use std::sync::Arc;

use iyakki_agent::wsexec::Runner;

fn ask(run: &dyn Runner, secs: &str, addr: &str) -> Option<bool> {
    let prog = "cu".to_string() + "rl";
    run.has(&prog).then(|| run.run(&std::env::temp_dir(), &[prog.clone(), "-fsS".into(), "--max-time".into(), secs.into(), format!("http://{addr}/v2/")], &[], std::time::Duration::from_secs(5), 4096, None).ok())
}

/// What the cluster tools' registry check says in a unit test.
pub(crate) fn registry_probe(run: Arc<dyn Runner>) -> Arc<dyn Fn() -> bool + Send + Sync> {
    Arc::new(move || ask(&*run, "2", iyakki_agent::k8stools::REGISTRY) == Some(true))
}

/// What the registry status check (`reachable`) says in a unit test.
pub(crate) fn reachable(run: &dyn Runner, addr: &str) -> Option<bool> {
    ask(run, "3", addr)
}
