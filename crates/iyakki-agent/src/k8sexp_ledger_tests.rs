//! A rollout of the cluster explorer is waiting work in the ledger (the ledger is the agent's, the rest is `iyakki_shared::k8sexp`).

#[test]
fn a_restart_is_waiting_work_in_the_ledger_until_it_ends_and_other_actions_are_not() {
    use crate::pending::{Ledger, Source, State};
    use iyakki_shared::k8s::{FakeKube, Target};
    use iyakki_shared::k8sexp::{run_action_reported, Action, Kind};
    fn target() -> Target {
        Target { kubeconfig: "/k/config".into(), context: "dev".into(), ..Default::default() }
    }
    let d = crate::testdir::short("k8sl", "rollout");
    let l = Ledger::open(&d.join("pending"), "alice");
    let k = FakeKube::new();
    k.answer("rollout restart", "restarted\n");
    k.answer("rollout status", "successfully rolled out\n");
    let a = Action::Restart { kind: Kind::Deployments, ns: "shop".into(), name: "web".into() };
    let r = run_action_reported(&k, &target(), &a, &mut |_| {}, Some(&l)).unwrap();
    assert!(r.contains("done"), "{r}");
    let t = l.list("").into_iter().find(|t| t.source == Source::Rollout).unwrap();
    assert_eq!((t.state, t.label.as_str()), (State::FinishedOk, "rollout of deployment web"), "{t:?}");
    // a restart that fails at its first step ends failed, with the reason
    let k = FakeKube::new();
    k.fail("rollout restart", "Error from server (Forbidden): cannot patch");
    assert!(run_action_reported(&k, &target(), &a, &mut |_| {}, Some(&l)).is_err());
    let failed: Vec<_> = l.list("").into_iter().filter(|t| t.source == Source::Rollout && t.state == State::FinishedFailed).collect();
    assert_eq!(failed.len(), 1);
    assert!(failed[0].detail.contains("Forbidden"), "{failed:?}");
    // a cordon is no rollout
    let k = FakeKube::new();
    k.answer("cordon", "cordoned");
    run_action_reported(&k, &target(), &Action::Cordon { node: "n".into() }, &mut |_| {}, Some(&l)).unwrap();
    assert_eq!(l.list("").into_iter().filter(|t| t.source == Source::Rollout).count(), 2);
}
