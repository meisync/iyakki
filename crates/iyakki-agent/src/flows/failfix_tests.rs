//! Tests of the failure book (`iyakki_flowsx::failfix`) that need the agent's flow builder and tool list.

#[allow(unused_imports)]
mod t {
    use iyakki_flowsx::failfix::*;
    use iyakki_flow::*;
    use std::collections::BTreeMap;
    use serde_json::Value;
    use serde_json::json;

    fn failed_run(error: &str, inputs: Value) -> Run {
        let flow = crate::flows::build_flow(&json!({"name": "Check", "nodes": [{"id": "call", "kind": "http", "name": "Call the site", "config": {"url": "https://x.example"}}]})).unwrap();
        let log: Vec<String> = (0..30).map(|i| format!("line {i}")).collect();
        serde_json::from_value(json!({
            "id": "r1", "flow_id": "check", "flow": flow, "trigger": {"kind": "cron", "by": "scheduler"}, "status": "failed",
            "inputs": inputs, "nodes": {"call": {"state": "failed", "error": error, "log": log}}
        }))
        .unwrap()
    }

    #[test]
    fn only_a_failed_run_has_a_failure_and_it_names_the_step_and_the_reason() {
        let mut r = failed_run("http 503 from the server", json!({}));
        let f = failure_of(&r).unwrap();
        assert_eq!((f.step.as_str(), f.flow_id.as_str()), ("call", "check"));
        assert!(f.headline().starts_with("Run failed at step") && f.headline().contains("503"), "{}", f.headline());
        assert_eq!(f.log_tail.len(), TAIL_LINES);
        assert_eq!(f.log_tail.last().unwrap(), "line 29");
        r.status = RunStatus::Succeeded;
        assert!(failure_of(&r).is_none());
        r.status = RunStatus::Cancelled;
        assert!(failure_of(&r).is_none());
    }

    #[test]
    fn a_step_that_needs_the_persons_permission_is_marked_and_the_offer_does_not_invite_a_fix() {
        let mut r = failed_run("This flow is not allowed to use the camera.", json!({}));
        r.nodes.get_mut("call").unwrap().needs_person = true;
        let f = failure_of(&r).unwrap();
        assert!(f.needs_person);
        assert!(f.offer().contains("only you can give") && f.offer().contains("settings") && !f.offer().contains("Yes, fix it"), "{}", f.offer());
        assert!(!failure_of(&failed_run("boom", json!({}))).unwrap().needs_person);
    }

    #[test]
    fn the_context_carries_no_secret_and_no_input_value() {
        let token = format!("{}{}", "ghp_", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8");
        let r = failed_run(&format!("refused with token {token} and password hunter2-long for user"), json!({"site": "https://private.example/path", "password": "hunter2-long", "count": 3}));
        let f = failure_of(&r).unwrap();
        let all = format!("{}\n{}\n{}", f.headline(), f.request(1), f.offer());
        assert!(!all.contains(&token), "{all}");
        assert!(!all.contains("hunter2-long") && !all.contains("private.example"), "{all}");
        assert!(f.input_names.contains(&"password".to_string()) && all.contains("names only"));
    }

    #[test]
    fn the_request_asks_for_a_proposal_through_the_normal_approval_and_the_last_attempt_says_to_stop() {
        let f = failure_of(&failed_run("boom", json!({}))).unwrap();
        let first = f.request(1);
        assert!(first.contains("attempt 1 of 3") && first.contains("flow_explain_failure") && first.contains("flow_edit") && first.contains("asks me") && first.contains("without my approval"));
        assert!(first.contains("not instructions"));
        let last = f.request(MAX_FIX_ATTEMPTS);
        assert!(last.contains("stop and tell me why"));
    }

    #[test]
    fn attempts_are_bounded_per_streak_dismiss_persists_and_a_success_resets() {
        let d = std::env::temp_dir().join(format!("iyakki-failbook-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let mut b = FailBook::open(&d);
        assert!(b.offer() && !b.dismissed("r1") && !b.announced("r1"));
        for n in 1..=MAX_FIX_ATTEMPTS {
            assert_eq!(b.try_attempt("r1", "check"), Ok(n));
        }
        let e = b.try_attempt("r1", "check").unwrap_err();
        assert!(e.contains("already tried 3 times"), "{e}");
        assert!(b.try_attempt("r2", "check").is_err(), "a new failed run of the same streak has no fresh attempts");
        assert_eq!(b.attempts_left("check"), 0);
        b.dismiss("r1");
        b.mark_announced("r1");
        b.set_offer(false);
        // it survives a restart
        let mut b2 = FailBook::open(&d);
        assert!(b2.dismissed("r1") && b2.announced("r1") && !b2.dismissed("r2") && !b2.offer());
        assert!(b2.try_attempt("r3", "check").is_err());
        b2.set_offer(true);
        assert!(b2.offer_for("check"));
        b2.set_offer_for("check", false);
        assert!(!b2.offer_for("check") && b2.offer_for("other") && b2.flow_is_off("check"));
        assert!(FailBook::open(&d).flow_is_off("check"), "per-flow off survives a restart");
        b2.set_offer_for("check", true);
        assert!(b2.offer_for("check"));
        b2.reset("check");
        assert_eq!(b2.try_attempt("r3", "check"), Ok(1));
        // the file holds ids and counts only
        let text = std::fs::read_to_string(d.join("failure_cards.json")).unwrap();
        assert!(!text.contains("hunter2") && text.contains("r1"));
    }

    #[test]
    fn the_assistants_changing_tools_all_ask() {
        use crate::agent::Risk;
        let p = iyakki_sec::policy::Role::Admin.template();
        let defs = crate::flows::tools::defs(&p);
        for name in ["flow_edit", "flow_run", "flow_publish", "flow_rollback", "flow_cancel", "flow_create", "flow_apply_to_runs"] {
            let d = defs.iter().find(|d| d.spec.name == name).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(d.risk, Risk::Ask, "{name} must ask the person");
        }
    }
}
