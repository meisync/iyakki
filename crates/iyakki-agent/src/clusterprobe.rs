//! What the chosen cluster runs, asked of the cluster itself (KI-091). Settings -> Dev cluster -> Platform services used to read only
//! the account's install records, so a Gitea put in by Store -> Setup read "not installed" although it ran, and the Registry row spun for
//! ever when no cluster had been chosen. One read-only `kubectl get deploy -A` (short timeout) answers for all three rows.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::devcluster::{Choice, Kc};
use crate::wsexec::Runner;

pub const GITEA_NS: &str = "iyakki-gitea";
pub const STORE_NS: &str = "iyakki-store";
pub const REGISTRY_NS: &str = crate::regsetup::IN_NS;
/// How long an answer is reused.
pub const FRESH: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub gitea: bool,
    pub store: bool,
    pub registry: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    /// The account has no saved cluster choice.
    NoCluster,
    /// The cluster did not answer (why, in words).
    Failed(String),
    Seen(Seen),
}

impl Probe {
    /// The Registry row: (text, installed).
    pub fn registry_text(&self) -> String {
        match self {
            Probe::NoCluster => "no cluster chosen".into(),
            Probe::Failed(w) => format!("cluster not answering ({w})"),
            Probe::Seen(s) if s.registry => "installed".into(),
            Probe::Seen(_) => "not installed".into(),
        }
    }
    /// Is the service with this id (`gitea`, the private store's id) running in the cluster?
    pub fn has(&self, service: &str) -> bool {
        match self {
            Probe::Seen(s) => (service == "gitea" && s.gitea) || (service != "gitea" && service.contains("store") && s.store),
            _ => false,
        }
    }
}

/// Ask the cluster (3 s per request, 5 s in all). Never panics, never waits longer.
pub fn probe(run: Arc<dyn Runner>, choice: Option<&Choice>) -> Probe {
    let Some(c) = choice else { return Probe::NoCluster };
    let kc = Kc::new(run, &c.kubeconfig, &c.context);
    let o = kc.kubectl(&["get", "deploy", "-A", "-o", "jsonpath={range .items[*]}{.metadata.namespace}{\"\\n\"}{end}"], 3, 5);
    if o.timed_out {
        return Probe::Failed("timed out".into());
    }
    if o.missing {
        return Probe::Failed("kubectl is not available".into());
    }
    if o.code != Some(0) {
        let why = o.stderr.lines().next().unwrap_or("refused").trim();
        return Probe::Failed(if why.is_empty() { "refused".into() } else { why.chars().take(80).collect() });
    }
    let has = |ns: &str| o.stdout.lines().any(|l| l.trim() == ns);
    Probe::Seen(Seen { gitea: has(GITEA_NS), store: has(STORE_NS), registry: has(REGISTRY_NS) })
}

/// A probe kept for [`FRESH`].
pub fn fresh(at: Option<Instant>) -> bool {
    at.is_some_and(|t| t.elapsed() < FRESH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wsexec::{Out, Sink};
    use std::path::Path;

    struct Fake(Out);
    impl Runner for Fake {
        fn run(&self, _: &Path, argv: &[String], _: &[(String, String)], _: Duration, _: usize, _: Option<Sink>) -> Out {
            assert!(argv.iter().any(|a| a == "get") && !argv.iter().any(|a| a == "delete" || a == "apply"), "read-only");
            self.0.clone()
        }
        fn has(&self, _: &str) -> bool {
            true
        }
    }
    fn ch() -> Choice {
        Choice { kubeconfig: "/k".into(), context: "c".into() }
    }
    fn go(o: Out) -> Probe {
        probe(Arc::new(Fake(o)), Some(&ch()))
    }
    fn ok(s: &str) -> Out {
        Out { code: Some(0), stdout: s.into(), ..Default::default() }
    }

    #[test]
    fn installed_elsewhere_is_seen() {
        let p = go(ok("kube-system\niyakki-gitea\niyakki-registry\n"));
        assert!(p.has("gitea"));
        assert!(!p.has("privatestore") && !p.has("private-store"));
        assert_eq!(p.registry_text(), "installed");
    }
    #[test]
    fn absent() {
        let p = go(ok("kube-system\n"));
        assert!(!p.has("gitea"));
        assert_eq!(p.registry_text(), "not installed");
    }
    #[test]
    fn store_namespace() {
        assert!(go(ok("iyakki-store\n")).has("private-store"));
    }
    #[test]
    fn timeout_is_named() {
        let p = go(Out { timed_out: true, ..Default::default() });
        assert_eq!(p, Probe::Failed("timed out".into()));
        assert!(p.registry_text().contains("timed out"));
        assert!(!p.has("gitea"));
    }
    #[test]
    fn refused_names_the_reason() {
        let p = go(Out { code: Some(1), stderr: "connection refused\nmore".into(), ..Default::default() });
        assert!(p.registry_text().contains("connection refused"));
    }
    #[test]
    fn no_choice_finishes_at_once() {
        let p = probe(Arc::new(Fake(Out::default())), None);
        assert_eq!(p, Probe::NoCluster);
        assert_eq!(p.registry_text(), "no cluster chosen");
    }
    #[test]
    fn freshness() {
        assert!(!fresh(None));
        assert!(fresh(Some(Instant::now())));
    }
}
