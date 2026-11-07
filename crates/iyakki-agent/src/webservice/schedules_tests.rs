//! The schedule routes (`/api/v1/schedules...`) on the service fakes: who may create one, what is refused and why, the record kept for
//! the tenant service (owner, team, role snapshot), the audit lines, the quota, the preview in the tenant's zone, and the status.
//! (The test harness hands query values over undecoded; the real parser decodes them, see `httpmini`.)

use super::*;

fn post(h: &H, tok: &str, path: &str, body: Value) -> (u16, Value) {
    let (s, v, _) = h.call("POST", path, Some(tok), Some(body));
    (s, v)
}
fn get(h: &H, tok: &str, path: &str) -> (u16, Value) {
    let (s, v, _) = h.call("GET", path, Some(tok), None);
    (s, v)
}
fn cron(flow: &str, team: &str) -> Value {
    json!({"flow": flow, "team": team, "trigger": {"id": "t1", "kind": "cron", "expr": "0 9 * * *"}})
}
fn registry(h: &H) -> Vec<Value> {
    std::fs::read_to_string(h.dir.join("cloud/flows/schedules.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

#[test]
fn an_editor_creates_lists_pauses_resumes_and_deletes_a_schedule_and_every_change_is_audited() {
    let (h, _, ann, _, _) = acme_cloud();
    let (s, v) = post(&h, &ann, "/api/v1/schedules", cron("f1", "web"));
    assert_eq!((s, v["active"].clone()), (200, json!(true)), "{v}");
    let rec = registry(&h);
    assert_eq!(rec.len(), 1);
    assert_eq!((rec[0]["owner"].as_str(), rec[0]["team"].as_str(), rec[0]["role_snapshot"].as_str(), rec[0]["run_as"].as_str()), (Some("ann"), Some("web"), Some("editor"), Some("owner")));
    let (s, v) = get(&h, &ann, "/api/v1/schedules");
    assert_eq!(s, 200);
    let one = &v["schedules"][0];
    assert_eq!((one["flow"].as_str(), one["team"].as_str(), one["owner"].as_str(), one["role_snapshot"].as_str()), (Some("f1"), Some("web"), Some("ann"), Some("editor")));
    assert_eq!(v["limit"], 50);
    assert_eq!(post(&h, &ann, "/api/v1/schedules/f1/pause", json!({})).0, 200);
    assert_eq!(post(&h, &ann, "/api/v1/schedules/f1/resume", json!({})).0, 200);
    assert_eq!(h.call("DELETE", "/api/v1/schedules/f1/t1", Some(&ann), None).0, 200);
    assert!(registry(&h).is_empty(), "the last timed trigger is gone, so is the record");
    assert_eq!(h.call("DELETE", "/api/v1/schedules/f1/nope", Some(&ann), None).0, 404);
    for id in ["sched_save", "sched_pause", "sched_resume", "sched_delete"] {
        assert!(h.accounts.lines().iter().any(|l| l.contains(&format!("api.{id}"))), "{id} is in the service audit");
    }
    let audit = std::fs::read_to_string(h.dir.join("cloud/audit.log")).or_else(|_| std::fs::read_to_string(crate::cloudroles::audit_path(&h.dir))).unwrap_or_default();
    for ev in ["flow.schedule.save", "flow.schedule.pause", "flow.schedule.resume", "flow.schedule.delete"] {
        assert!(audit.contains(ev), "{ev} in the tenant audit: {audit}");
    }
    assert!(!audit.contains("password") && !audit.contains("Bearer"));
}

#[test]
fn the_team_and_the_role_decide_who_may_own_a_schedule() {
    let (h, root, ann, bob, gus) = acme_cloud();
    assert_eq!(post(&h, &bob, "/api/v1/schedules", cron("f1", "web")).0, 403, "bob is in the tenant, not in web");
    assert_eq!(post(&h, &gus, "/api/v1/schedules", cron("f1", "web")).0, 403, "a stranger");
    assert_eq!(get(&h, &gus, "/api/v1/schedules").0, 403);
    let mut body = cron("f1", "web");
    body["run_as"] = json!("team-service");
    assert_eq!(post(&h, &ann, "/api/v1/schedules", body.clone()).0, 403, "an editor may not choose the team identity");
    // a tenant admin counts as an admin of every team, so the tenant's owner may choose the team identity
    assert_eq!(post(&h, &root, "/api/v1/schedules", body).0, 200);
    let rec = registry(&h);
    assert_eq!((rec[0]["run_as"].as_str(), rec[0]["chosen_by"].as_str(), rec[0]["role_snapshot"].as_str()), (Some("team-service"), Some("root"), Some("admin")));
    assert_eq!(h.call("DELETE", "/api/v1/schedules/f1/t1", Some(&root), None).0, 200);
    assert_eq!(post(&h, &ann, "/api/v1/schedules", cron("f1", "nowhere")).0, 403);
    assert!(registry(&h).is_empty());
}

#[test]
fn a_flow_that_needs_a_persons_login_or_an_unmarked_secret_cannot_be_scheduled() {
    let (h, _, ann, _, _) = acme_cloud();
    let (s, v) = post(&h, &ann, "/api/v1/schedules", cron("f2", "web"));
    assert_eq!(s, 422, "{v}");
    assert!(v["error"]["problems"][0].as_str().unwrap().contains("Claude"), "{v}");
    let (s, v) = post(&h, &ann, "/api/v1/schedules", cron("f3", "web"));
    assert_eq!(s, 422);
    assert!(v["error"]["problems"][0].as_str().unwrap().contains("not marked"), "{v}");
    assert!(registry(&h).is_empty());
    // a team admin marks the secret: the reason goes away (the vault is still locked in this harness, which is a run-time block, not a save-time one)
    std::fs::create_dir_all(h.dir.join("cloud/config")).unwrap();
    std::fs::write(h.dir.join("cloud/config/tenant-config.json"), json!({"items": [{"kind": "flows.unattended", "namespace": "web", "name": "api-token", "enabled": true, "values": {"secret": "vault:team/web/api-token", "terms_ack": true}}]}).to_string()).unwrap();
    assert_eq!(post(&h, &ann, "/api/v1/schedules", cron("f3", "web")).0, 200);
}

#[test]
fn bad_triggers_are_refused_before_anything_is_saved() {
    let (h, _, ann, _, _) = acme_cloud();
    let t = |tr: Value| post(&h, &ann, "/api/v1/schedules", json!({"flow": "f1", "team": "web", "trigger": tr})).0;
    assert_eq!(t(json!({"kind": "cron", "expr": "0 9 * *"})), 400, "four fields");
    assert_eq!(t(json!({"kind": "cron", "expr": "61 9 * * *"})), 400);
    assert_eq!(t(json!({"kind": "interval", "every_s": 30})), 400, "at least a minute");
    assert_eq!(t(json!({"kind": "once", "at_ms": 1000})), 400, "in the past");
    assert_eq!(t(json!({"kind": "webhook", "path": "/x"})), 400, "not a schedule");
    assert_eq!(t(json!({"kind": "manual"})), 400);
    assert_eq!(post(&h, &ann, "/api/v1/schedules", json!({"flow": "f1", "team": "web", "trigger": {"kind": "cron", "expr": "0 9 * * *"}, "extra": 1})).0, 400, "unknown fields");
    assert_eq!(post(&h, &ann, "/api/v1/schedules", cron("missing", "web")).0, 404);
    assert!(registry(&h).is_empty());
}

#[test]
fn the_tenants_quota_limits_the_number_of_schedules() {
    let (h, _, ann, _, _) = acme_cloud();
    std::fs::create_dir_all(h.dir.join("cloud/config")).unwrap();
    std::fs::write(h.dir.join("cloud/config/tenant-config.json"), json!({"items": [{"kind": "flows.quota", "enabled": true, "values": {"max_schedules": 1}}, {"kind": "flows.unattended", "namespace": "web", "name": "api-token", "enabled": true, "values": {"secret": "vault:team/web/api-token", "terms_ack": true}}]}).to_string()).unwrap();
    assert_eq!(post(&h, &ann, "/api/v1/schedules", cron("f1", "web")).0, 200);
    let (s, v) = post(&h, &ann, "/api/v1/schedules", cron("f3", "web"));
    assert_eq!(s, 429, "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("limit 1"));
    assert_eq!(post(&h, &ann, "/api/v1/schedules", cron("f1", "web")).0, 200, "changing the one that exists is not a new schedule");
    assert_eq!(get(&h, &ann, "/api/v1/schedules").1["limit"], 1);
}

#[test]
fn the_preview_shows_the_next_times_in_the_tenants_zone() {
    let (h, _, ann, _, _) = acme_cloud();
    let from = 1_800_000_000_000i64;
    let (s, v) = get(&h, &ann, &format!("/api/v1/schedules/preview?expr=0 9 * * *&tz=+05:30&count=3&from_ms={from}"));
    assert_eq!(s, 200, "{v}");
    let next: Vec<i64> = v["next"].as_array().unwrap().iter().map(|x| x.as_i64().unwrap()).collect();
    assert_eq!(next.len(), 3);
    assert!(next.iter().all(|t| (t / 1000).rem_euclid(86_400) == 3 * 3600 + 1800), "09:00 at +05:30 is 03:30 UTC: {next:?}");
    assert_eq!(next[1] - next[0], 86_400_000);
    // the tenant's own setting is what `local` means
    std::fs::create_dir_all(h.dir.join("cloud/config")).unwrap();
    std::fs::write(h.dir.join("cloud/config/tenant-config.json"), json!({"items": [{"kind": "schedule.timezone", "enabled": true, "values": {"timezone": "-0800"}}]}).to_string()).unwrap();
    let (_, v) = get(&h, &ann, &format!("/api/v1/schedules/preview?expr=0 9 * * *&count=1&from_ms={from}"));
    assert_eq!((v["next"][0].as_i64().unwrap() / 1000).rem_euclid(86_400), 17 * 3600, "09:00 at -08:00 is 17:00 UTC");
    assert_eq!(get(&h, &ann, "/api/v1/schedules/preview?expr=nonsense").0, 400);
    assert_eq!(get(&h, &ann, "/api/v1/schedules/preview").0, 400, "no expression");
    assert_eq!(get(&h, &ann, &format!("/api/v1/schedules/preview?expr=0 9 * * *&count=500&from_ms={from}")).1["next"].as_array().unwrap().len(), 10, "at most ten");
}

#[test]
fn a_schedule_the_service_paused_does_not_resume_until_the_owner_has_the_access_again() {
    let (h, root, ann, _, _) = acme_cloud();
    assert_eq!(post(&h, &ann, "/api/v1/schedules", cron("f1", "web")).0, 200);
    assert_eq!(h.call("POST", "/api/v1/namespaces/web/members", Some(&root), Some(json!({"name": "ann", "role": "remove"}))).0, 200);
    let (s, v) = post(&h, &ann, "/api/v1/schedules/f1/resume", json!({}));
    assert_eq!(s, 403, "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("stays paused"));
    let (_, l) = get(&h, &ann, "/api/v1/schedules");
    assert!(l["schedules"][0]["owner_check"].as_str().unwrap().contains("no longer in the team"), "the list says why");
}

#[test]
fn the_status_route_has_counters_and_the_routes_exist_only_in_the_cloud_profile() {
    let (h, _, ann, _, gus) = acme_cloud();
    let (s, v) = get(&h, &ann, "/api/v1/schedules/status");
    assert_eq!(s, 200);
    assert!(v.get("approvals_waiting").is_some());
    assert_eq!(get(&h, &gus, "/api/v1/schedules/status").0, 403);
    let agent = harness("agent-api", |_| {});
    let tok = agent.ann();
    assert_eq!(get(&agent, &tok, "/api/v1/schedules").0, 404);
    assert_eq!(get(&agent, &tok, "/api/v1/schedules/status").0, 404);
}

#[test]
fn the_schedule_routes_are_in_the_api_document_and_need_the_flows_permission() {
    let doc = openapi::document(Profile::Cloud);
    let paths = doc["paths"].as_object().unwrap();
    for p in ["/api/v1/schedules", "/api/v1/schedules/status", "/api/v1/schedules/preview", "/api/v1/schedules/{id}/pause", "/api/v1/schedules/{id}/resume", "/api/v1/schedules/{id}/{name}"] {
        assert!(paths.contains_key(p), "{p}");
    }
    assert!(ROUTES.iter().filter(|r| r.id.starts_with("sched_")).all(|r| r.profiles == [Profile::Cloud] && r.needs == routes::Need::Flows && r.access == routes::Access::User));
}
