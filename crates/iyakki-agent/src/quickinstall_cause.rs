//! The cause class of a failed Quick install step. Every failure line (the job's log and the card) starts with one of a few plain
//! classes, so a person sees at once whether to check the network, wait, free disk space, accept a licence, look at another item,
//! look at the cluster, or read the log. Pure text work over the messages the steps already produce.

/// Why a step failed, in the words a person reads first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    Network,
    TimedOut,
    DiskFull,
    /// A licence or consent that the person has to give on a card (never given on their behalf).
    Consent,
    /// A step this one needs failed first.
    Dependency,
    ClusterNotReady,
    VerifyFailed,
    Other,
}

impl Cause {
    /// The class word(s) that start the failure line.
    pub fn label(self) -> &'static str {
        match self {
            Cause::Network => "Network problem",
            Cause::TimedOut => "Timed out",
            Cause::DiskFull => "Disk full",
            Cause::Consent => "Licence or consent needed",
            Cause::Dependency => "Dependency failed",
            Cause::ClusterNotReady => "Cluster not ready",
            Cause::VerifyFailed => "Verify failed",
            Cause::Other => "Other",
        }
    }

    const ALL: [Cause; 8] = [Cause::Network, Cause::TimedOut, Cause::DiskFull, Cause::Consent, Cause::Dependency, Cause::ClusterNotReady, Cause::VerifyFailed, Cause::Other];
}

/// The class of an error text. A text that already starts with a class label keeps it; otherwise the first rule that matches wins, most
/// specific first (a failed prerequisite and a failed check name their own class even when the text mentions the network).
pub fn classify(err: &str) -> Cause {
    let l = err.trim().trim_start_matches("error:").trim().to_lowercase();
    if let Some(c) = Cause::ALL.iter().find(|c| l.starts_with(&c.label().to_lowercase())) {
        return *c;
    }
    let has = |ks: &[&str]| ks.iter().any(|k| l.contains(k));
    if has(&["verify failed"]) {
        Cause::VerifyFailed
    } else if has(&["failed first", "was not installed after"]) {
        Cause::Dependency
    } else if has(&["network problem", "no data from", "no answer from", "could not connect", "connection refused", "dns error", "failed to lookup", "name resolution", "connection reset"]) {
        Cause::Network
    } else if has(&["timed out", "timeout", "did not answer after", "did not become ready in"]) && !has(&["cluster did not become ready"]) {
        Cause::TimedOut
    } else if has(&["no space left", "disk full", "not enough room", "mb free; this needs"]) {
        Cause::DiskFull
    } else if has(&["licence was not accepted", "licence not accepted", "has not confirmed", "not confirmed the licences", "consent"]) {
        Cause::Consent
    } else if has(&["not installed and ready", "did not become ready within", "cluster is not ready", "no cluster", "cluster did not become ready"]) {
        Cause::ClusterNotReady
    } else {
        Cause::Other
    }
}

/// The failure line: the class first (`Network problem: no data from X for 60 s`). The error keeps its own words; a leading class
/// label it already has is not repeated, (the download guard's "network problem:" is the same class, so it only gains a capital letter).
pub fn failure_line(err: &str) -> String {
    let e = err.trim();
    let e = e.strip_prefix("error:").map(str::trim).unwrap_or(e);
    let cause = classify(e);
    let lower = e.to_lowercase();
    let label = cause.label();
    if lower.starts_with(&label.to_lowercase()) {
        // already begins with the class (a verify failure says "Verify failed: ..."): keep the capital letter of the label
        return format!("{label}{}", &e[label.len()..]);
    }
    if cause == Cause::Other {
        return e.to_string();
    }
    format!("{label}: {e}")
}
