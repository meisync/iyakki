//! The tenant list and the switch (`tenants.rs`): who sees which tenants, that a switch is checked on the server, binds only the
//! session that asked, moves the volume the next request works on, stops running work, locks the vault hook, is audited and limited,
//! and that a machine that is not a pod has one tenant and no switch.

use super::*;
use crate::cloudroles::{self, Roles, TenantRole};
use crate::webservice::tenants;

fn write_roles(dir: &std::path::Path, owner: &str, members: &[&str]) {
    let mut r = Roles::new(owner).unwrap();
    for m in members {
        r.add_tenant_member(owner, m, TenantRole::Member).unwrap();
    }
    cloudroles::save(&cloudroles::path(dir), &r).unwrap();
}

/// acme (home): root owner, ann and bob members. beta: ann owner. gamma: root owner (ann and bob are strangers there).
fn three() -> (H, String, String, String) {
    let h = harness("cloud", |_| {});
    let (root, ann, bob) = (h.root(), h.ann(), h.login("bob", "bob-password-1"));
    assert_eq!(h.call("POST", "/api/v1/tenant/bootstrap", Some(&root), None).0, 200);
    for who in ["ann", "bob"] {
        assert_eq!(h.call("POST", "/api/v1/tenant/members", Some(&root), Some(json!({"name": who, "role": "member"}))).0, 200);
    }
    let (beta, gamma) = (h.dir.join("beta-volume"), h.dir.join("gamma-volume"));
    write_roles(&beta, "ann", &[]);
    write_roles(&gamma, "root", &[]);
    let file = tenants::file(&h.dir);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let text = json!({"home": {"id": "acme", "name": "Acme Ltd"}, "tenants": [
        {"id": "beta", "name": "Beta GmbH", "data_dir": beta.to_string_lossy()},
        {"id": "gamma", "name": "Gamma", "host": "gamma.example.org", "data_dir": gamma.to_string_lossy()}]});
    std::fs::write(file, text.to_string()).unwrap();
    (h, root, ann, bob)
}

fn ids(v: &Value) -> Vec<String> {
    v["tenants"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap().to_string()).collect()
}

#[test]
fn the_list_holds_only_the_tenants_the_person_belongs_to_and_marks_the_current_one() {
    let (h, root, ann, bob) = three();
    let (s, v, _) = h.call("GET", "/api/v1/tenants", Some(&ann), None);
    assert_eq!(s, 200, "{v}");
    assert_eq!(ids(&v), ["acme", "beta"], "gamma is not ann's");
    assert_eq!((v["current"].as_str(), v["can_switch"].as_bool()), (Some("acme"), Some(true)));
    let acme = &v["tenants"][0];
    assert_eq!((acme["name"].as_str(), acme["role"].as_str(), acme["current"].as_bool()), (Some("Acme Ltd"), Some("member"), Some(true)));
    assert_eq!(v["tenants"][1]["role"], "owner");
    let (_, v, _) = h.call("GET", "/api/v1/tenants", Some(&bob), None);
    assert_eq!((ids(&v), v["can_switch"].as_bool()), (vec!["acme".to_string()], Some(false)), "one tenant: nothing to switch to");
    assert!(!v.to_string().contains("gamma") && !v.to_string().contains("beta"), "no hint of tenants bob is not in");
    let (_, v, _) = h.call("GET", "/api/v1/tenants", Some(&root), None);
    assert_eq!(ids(&v), ["acme", "gamma"]);
    assert_eq!(h.call("GET", "/api/v1/tenants", None, None).0, 401);
}

#[test]
fn listing_every_tenant_is_for_a_service_administrator_who_asks_for_it_and_is_audited() {
    let (h, root, ann, _) = three();
    assert_eq!(h.call("GET", "/api/v1/tenants?scope=all", Some(&ann), None).0, 403);
    assert_eq!(h.call("GET", "/api/v1/tenants?scope=nonsense", Some(&root), None).0, 400);
    let (s, v, _) = h.call("GET", "/api/v1/tenants?scope=all", Some(&root), None);
    assert_eq!((s, ids(&v)), (200, vec!["acme".to_string(), "beta".into(), "gamma".into()]));
    assert!(v["tenants"][1]["role"].is_null(), "root is not in beta: listed, no role");
    assert_eq!(v["scope"], "all");
    assert_eq!(v["can_switch"], true);
    assert!(h.accounts.lines().iter().any(|l| l.contains("root api.tenants.all count=3")), "{:?}", h.accounts.lines());
    // and switching into a tenant the administrator is not a member of is still refused
    assert_eq!(h.call("POST", "/api/v1/tenant/switch", Some(&root), Some(json!({"tenant": "beta"}))).0, 404);
}

#[test]
fn a_switch_is_checked_on_the_server_binds_only_that_session_and_moves_the_volume() {
    let (h, _, ann, bob) = three();
    let ann2 = h.ann();
    // a stranger, an unknown tenant and a bad id all get the same answer
    for t in ["beta", "gamma", "nope", "../x", "BETA"] {
        let (s, v, _) = h.call("POST", "/api/v1/tenant/switch", Some(&bob), Some(json!({"tenant": t})));
        assert_eq!((s, v["error"]["code"].as_str()), (404, Some("not_found")), "{t}");
    }
    assert_eq!(h.call("POST", "/api/v1/tenant/switch", Some(&ann), Some(json!({"tenant": "gamma"}))).0, 404, "ann is not in gamma");
    assert_eq!(h.call("POST", "/api/v1/tenant/switch", Some(&ann), Some(json!({"nope": 1}))).0, 400);
    assert!(h.switched.lock().unwrap().is_empty(), "no refused switch locked anything");
    // at home ann is a member; in beta she owns, and the next request already works on beta's lists
    assert_eq!(h.call("GET", "/api/v1/tenant", Some(&ann), None).1["tenant_role"], "member");
    let (s, v, _) = h.call("POST", "/api/v1/tenant/switch", Some(&ann), Some(json!({"tenant": "beta"})));
    assert_eq!((s, v["switched"].as_bool(), v["tenant"]["name"].as_str(), v["tenant"]["role"].as_str(), v["previous"].as_str()), (200, Some(true), Some("Beta GmbH"), Some("owner"), Some("acme")), "{v}");
    assert!(v["url"].is_null(), "beta has no address of its own: it is served here");
    assert_eq!(h.call("GET", "/api/v1/tenant", Some(&ann), None).1["tenant_role"], "owner");
    assert_eq!(h.call("GET", "/api/v1/tenants", Some(&ann), None).1["current"], "beta");
    // her other session, and other people, are untouched
    assert_eq!(h.call("GET", "/api/v1/tenants", Some(&ann2), None).1["current"], "acme");
    assert_eq!(h.call("GET", "/api/v1/tenants", Some(&bob), None).1["current"], "acme");
    // files written now land on beta's volume, not acme's
    let (s, v, _) = h.call("POST", "/api/v1/files/write", Some(&ann), Some(json!({"path": "note.txt", "content": "hello"})));
    assert_eq!(s, 200, "{v}");
    let found = |root: &std::path::Path| -> bool {
        fn walk(p: &std::path::Path) -> bool {
            std::fs::read_dir(p).map(|d| d.flatten().any(|e| e.file_name() == "note.txt" || (e.path().is_dir() && walk(&e.path())))).unwrap_or(false)
        }
        walk(root)
    };
    assert!(found(&h.dir.join("beta-volume")) && !found(&h.dir.join("cloud")), "the write went to the tenant the session is in");
    // the same tenant again is a no-op; back home works
    let (_, v, _) = h.call("POST", "/api/v1/tenant/switch", Some(&ann), Some(json!({"tenant": "beta"})));
    assert_eq!(v["switched"], false);
    let (s, v, _) = h.call("POST", "/api/v1/tenant/switch", Some(&ann), Some(json!({"tenant": "acme"})));
    assert_eq!((s, v["tenant"]["role"].as_str()), (200, Some("member")));
    assert_eq!(h.call("GET", "/api/v1/tenant", Some(&ann), None).1["tenant_role"], "member");
}

#[test]
fn a_switch_stops_running_work_locks_the_vault_hook_and_is_audited_without_secrets() {
    let (h, _, ann, _) = three();
    let run = h.start_run(&ann, vec![call_turn("web_fetch")]);
    h.wait_event(&ann, &run, |e| e["type"] == "approval");
    let (s, v, _) = h.call("POST", "/api/v1/tenant/switch", Some(&ann), Some(json!({"tenant": "beta"})));
    assert_eq!((s, v["runs_stopped"].as_u64(), v["vault_locked"].as_bool()), (200, Some(1), Some(true)), "{v}");
    assert_eq!(*h.switched.lock().unwrap(), vec![("ann".to_string(), "acme".to_string(), "beta".to_string())]);
    let lines = h.accounts.lines().join("\n");
    assert!(lines.contains("ann api.tenant_switch from=acme to=beta runs_stopped=1 status=200"), "{lines}");
    assert!(!lines.contains("iyk_") && !lines.contains("password"), "{lines}");
    // a refused switch is not an audited success
    let before = h.accounts.lines().len();
    assert_eq!(h.call("POST", "/api/v1/tenant/switch", Some(&ann), Some(json!({"tenant": "gamma"}))).0, 404);
    assert!(h.accounts.lines()[before..].iter().all(|l| l.contains("refused:not_found")), "{:?}", &h.accounts.lines()[before..]);
    assert_eq!(h.switched.lock().unwrap().len(), 1);
}

#[test]
fn switching_is_limited_per_person_and_follows_the_origin_rules_of_other_posts() {
    let (h, _, ann, bob) = three();
    let mut last = 0;
    for i in 0..12 {
        let t = if i % 2 == 0 { "beta" } else { "acme" };
        last = h.call("POST", "/api/v1/tenant/switch", Some(&ann), Some(json!({"tenant": t}))).0;
        if last == 429 {
            assert!(i >= 10, "ten switches a minute are allowed");
            break;
        }
    }
    assert_eq!(last, 429, "the eleventh switch in a minute is refused");
    // someone else is not slowed by ann's switches
    assert_eq!(h.call("POST", "/api/v1/tenant/switch", Some(&bob), Some(json!({"tenant": "acme"}))).0, 200);
    // a foreign origin is refused before anything happens
    let mut r = req("POST", "/api/v1/tenant/switch", Some(&bob), Some(json!({"tenant": "acme"})));
    r.headers.insert("origin".into(), "https://evil.example".into());
    assert_eq!(h.call_req(r).0, 403);
    assert_eq!(h.call("POST", "/api/v1/tenant/switch", None, Some(json!({"tenant": "acme"}))).0, 401);
    assert_eq!(h.call("GET", "/api/v1/tenant/switch", Some(&ann), None).0, 405);
}

#[test]
fn a_tenant_with_its_own_address_is_named_in_the_answer_and_a_removed_member_falls_back_home() {
    let (h, root, _, _) = three();
    // root owns gamma, which has a host of its own: the page is told where to go
    let (s, v, _) = h.call("POST", "/api/v1/tenant/switch", Some(&root), Some(json!({"tenant": "gamma"})));
    assert_eq!((s, v["url"].as_str()), (200, Some("https://gamma.example.org/ui/")), "{v}");
    // root is then removed from gamma: the session is back in the home tenant on the next request, never on a volume it lost
    write_roles(&h.dir.join("gamma-volume"), "zed", &[]);
    let (_, v, _) = h.call("GET", "/api/v1/tenants", Some(&root), None);
    assert_eq!((v["current"].as_str(), ids(&v)), (Some("acme"), vec!["acme".to_string()]));
    assert_eq!(h.call("GET", "/api/v1/tenant", Some(&root), None).1["tenant_role"], "owner", "home lists, not gamma's");
}

#[test]
fn a_bad_directory_file_leaves_only_the_home_tenant_and_a_pod_without_one_has_a_single_tenant() {
    let h = harness("cloud", |_| {});
    let ann = h.ann();
    let (s, v, _) = h.call("GET", "/api/v1/tenants", Some(&ann), None);
    assert_eq!((s, ids(&v), v["can_switch"].as_bool()), (200, vec!["default".to_string()], Some(false)));
    let file = tenants::file(&h.dir);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "{ not json").unwrap();
    assert_eq!(ids(&h.call("GET", "/api/v1/tenants", Some(&ann), None).1), ["default"]);
    // bad entries (relative folder, '..', repeated or home id, bad host) are skipped, good ones kept
    let ok = h.dir.join("ok");
    write_roles(&ok, "ann", &[]);
    let t = json!({"home": {"id": "acme", "name": "A"}, "tenants": [
        {"id": "a1", "name": "x", "data_dir": "relative"}, {"id": "a2", "name": "x", "data_dir": "/x/../y"},
        {"id": "acme", "name": "dup", "data_dir": ok.to_string_lossy()}, {"id": "a3", "name": "x", "host": "bad host", "data_dir": ok.to_string_lossy()},
        {"id": "good", "name": "Good", "data_dir": ok.to_string_lossy()}, {"id": "good", "name": "again", "data_dir": ok.to_string_lossy()}]});
    std::fs::write(&file, t.to_string()).unwrap();
    let (_, v, _) = h.call("GET", "/api/v1/tenants", Some(&ann), None);
    assert_eq!(ids(&v), ["acme", "good"]);
}

#[test]
fn a_machine_that_is_not_a_pod_has_one_tenant_this_machine_and_no_switch() {
    let h = harness("agent-api", |_| {});
    let t = h.ann();
    let (s, v, _) = h.call("GET", "/api/v1/tenants", Some(&t), None);
    assert_eq!(s, 200, "{v}");
    assert_eq!((ids(&v), v["current"].as_str(), v["can_switch"].as_bool()), (vec!["this-machine".to_string()], Some("this-machine"), Some(false)));
    assert_eq!((v["tenants"][0]["name"].as_str(), v["tenants"][0]["role"].as_str()), (Some("This machine"), Some("user")));
    assert_eq!(h.call("GET", "/api/v1/tenants?scope=all", Some(&t), None).0, 200, "a one-tenant machine has no directory to list");
    assert_eq!(h.call("POST", "/api/v1/tenant/switch", Some(&t), Some(json!({"tenant": "this-machine"}))).0, 404, "the route does not exist here");
    assert!(openapi::document(Profile::AgentApi)["paths"].get("/api/v1/tenant/switch").is_none());
    assert!(openapi::document(Profile::AgentApi)["paths"].get("/api/v1/tenants").is_some());
    assert!(openapi::document(Profile::Cloud)["paths"].get("/api/v1/tenant/switch").is_some());
    assert!(!h.dir.join("cloud").exists(), "nothing was written");
}
