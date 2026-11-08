use super::quickinstall_cause::*;

#[test]
fn each_known_message_gets_its_class() {
    let cases: [(&str, Cause); 12] = [
        ("network problem: no data from github.com for 60 s (the connection went quiet)", Cause::Network),
        ("network problem reaching pypi.org: dns error", Cause::Network),
        ("Install k3s timed out: it ran for 120 minutes without finishing (it is not marked as installed)", Cause::TimedOut),
        ("elasticsearch did not become ready in 15 minutes: error: timed out", Cause::TimedOut),
        ("the data volume has 120 MB free; this needs about 600 MB", Cause::DiskFull),
        ("write failed: No space left on device (os error 28)", Cause::DiskFull),
        ("the licence was not accepted: Nothing was downloaded", Cause::Consent),
        ("k3s failed first, so this step stopped without waiting. See its log in the installer runs: network problem: x", Cause::Dependency),
        ("k3s was not installed and Ready after 20 minutes; see the k3s job's log", Cause::ClusterNotReady),
        ("k3s was installed, but the cluster did not become Ready within 10 minutes; it is not marked as installed", Cause::ClusterNotReady),
        ("Verify failed: duckdb does not start", Cause::VerifyFailed),
        ("kubectl apply failed: forbidden", Cause::Other),
    ];
    for (text, want) in cases {
        assert_eq!(classify(text), want, "{text}");
    }
}

#[test]
fn the_failure_line_starts_with_the_class_and_never_repeats_it() {
    assert_eq!(failure_line("network problem: no data from github.com for 60 s"), "Network problem: no data from github.com for 60 s");
    assert_eq!(failure_line("Network problem: x"), "Network problem: x");
    assert_eq!(failure_line("Verify failed: duckdb does not start. DuckDB is not marked as installed"), "Verify failed: duckdb does not start. DuckDB is not marked as installed");
    assert_eq!(failure_line("the data volume has 1 MB free; this needs about 600 MB"), "Disk full: the data volume has 1 MB free; this needs about 600 MB");
    // an unknown cause keeps its own words
    assert_eq!(failure_line("kubectl apply failed: forbidden"), "kubectl apply failed: forbidden");
    // formatting twice changes nothing
    let once = failure_line("Install k3s timed out: it ran for 5 minutes");
    assert_eq!(once, "Timed out: Install k3s timed out: it ran for 5 minutes");
    assert_eq!(failure_line(&once), once);
    for c in [Cause::Network, Cause::TimedOut, Cause::DiskFull, Cause::Consent, Cause::Dependency, Cause::ClusterNotReady, Cause::VerifyFailed] {
        assert_eq!(classify(&format!("{}: anything", c.label())), c);
    }
}
