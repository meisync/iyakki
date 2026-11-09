//! The OpenAPI 3.1 document, generated from the route table (`routes::ROUTES`) for the running profile. The schemas are written
//! here by hand (small, no generator crate); a test checks that every route is in the document, that the document has no route
//! the table lacks, and that every schema a route names exists.

use super::routes::{param_names, Access, ROUTES};
use super::{Profile, API_VERSION};
use serde_json::{json, Map, Value};

fn obj(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required, "additionalProperties": false})
}

/// Every schema the routes may name.
pub fn schemas() -> Map<String, Value> {
    let s = json!({"type": "string"});
    let i = json!({"type": "integer"});
    let b = json!({"type": "boolean"});
    let strs = json!({"type": "array", "items": {"type": "string"}});
    let any_arr = json!({"type": "array", "items": {"type": "object"}});
    let mut m = Map::new();
    let mut put = |k: &str, v: Value| {
        m.insert(k.to_string(), v);
    };
    put("Error", obj(json!({"error": obj(json!({"code": {"type": "string", "enum": ["not_found", "denied", "unsupported", "invalid_input", "busy", "io", "unauthorized", "rate_limited", "too_large", "unavailable"]}, "message": s}), &["code", "message"])}), &["error"]));
    put("Ok", obj(json!({"ok": b}), &["ok"]));
    put("Health", obj(json!({"status": s, "profile": s, "vault": s}), &["status"]));
    put("Version", obj(json!({"product": s, "version": s, "api": s, "profile": s}), &["product", "version", "api", "profile"]));
    put("LoginRequest", obj(json!({"name": s, "password": s}), &["name", "password"]));
    put("LoginResponse", obj(json!({"token": s, "expires_in": i, "user": s, "role": s}), &["token", "expires_in", "user", "role"]));
    put("OpenApi", json!({"type": "object"}));
    put("Whoami", obj(json!({"user": s, "role": s, "admin": b, "profile": s, "tools": strs}), &["user", "role", "admin", "profile", "tools"]));
    put("Models", obj(json!({"models": any_arr}), &["models"]));
    put("Tools", obj(json!({"tools": strs}), &["tools"]));
    put("RunRequest", obj(json!({"prompt": s, "provider": s, "model": s, "tools": strs}), &["prompt"]));
    put("RunStarted", obj(json!({"id": s}), &["id"]));
    put("RunList", obj(json!({"runs": any_arr}), &["runs"]));
    put("Run", obj(json!({"id": s, "status": {"type": "string", "enum": ["running", "completed", "failed", "stopped"]}, "events": any_arr, "next": i}), &["id", "status", "events", "next"]));
    put("Events", obj(json!({"id": s, "status": s, "events": any_arr, "next": i, "done": b}), &["id", "status", "events", "next", "done"]));
    put("Verdict", obj(json!({"verdict": {"type": "string", "enum": ["approve", "deny"]}}), &["verdict"]));
    put("ApprovalList", obj(json!({"approvals": any_arr}), &["approvals"]));
    put("ApprovalDecided", obj(json!({"run": s, "call": s, "tool": s, "verdict": s}), &["run", "call", "tool", "verdict"]));
    put("AdminStatus", obj(json!({"tokens": i, "runs_going": i, "approvals_pending": i}), &["tokens", "runs_going", "approvals_pending"]));
    put("FlowList", obj(json!({"flows": any_arr}), &["flows"]));
    put("Flow", json!({"type": "object"}));
    put("DataCall", obj(json!({"tool": {"type": "string", "enum": ["data_open", "data_describe", "data_query"]}, "args": {"type": "object"}, "namespace": s}), &["tool", "args"]));
    put("TenantList", obj(json!({"tenants": any_arr, "current": s, "can_switch": b, "scope": s}), &["tenants", "current", "can_switch", "scope"]));
    put("TenantSwitch", obj(json!({"tenant": s}), &["tenant"]));
    put("TenantSwitched", obj(json!({"switched": b, "tenant": {"type": "object"}, "previous": s, "runs_stopped": i, "vault_locked": b, "url": s}), &["switched", "tenant", "previous"]));
    put("TenantView", obj(json!({"user": s, "tenant_role": s, "members": any_arr, "namespaces": any_arr}), &["user", "tenant_role", "members", "namespaces"]));
    put("TenantMemberChange", obj(json!({"name": s, "role": {"type": "string", "enum": ["member", "admin", "owner", "remove"]}}), &["name", "role"]));
    put("NamespaceChange", obj(json!({"name": s, "action": {"type": "string", "enum": ["create", "delete"]}}), &["name", "action"]));
    put("NsMembers", obj(json!({"name": s, "role": s, "members": any_arr}), &["name", "role", "members"]));
    put("NsMemberChange", obj(json!({"name": s, "role": {"type": "string", "enum": ["viewer", "editor", "admin", "remove"]}}), &["name", "role"]));

    put("TileList", obj(json!({"tiles": any_arr}), &["tiles"]));
    put("TileEvents", obj(json!({"width": i, "height": i, "scale_pct": i, "dark": b, "reset": b, "events": any_arr}), &[]));
    put("TileFrame", obj(json!({"id": s, "redraw": b, "frame": {"type": "object"}, "stats": {"type": "object"}}), &["id", "redraw", "frame"]));

    put("WebAccess", obj(json!({"on": b, "port": i, "listening": b, "boot": b, "error": s, "status": s, "urls": strs, "fingerprint": {"type": ["string", "null"]}, "exposes": s, "has_admin_password": b}), &["on", "port", "listening", "status", "urls", "exposes"]));
    put("WebAccessChange", obj(json!({"on": b, "port": i}), &[]));

    let scope = json!({"type": "string", "enum": ["person", "shared", "personal"]});
    put("FileList", obj(json!({"path": s, "entries": any_arr}), &["path", "entries"]));
    put("FileContent", obj(json!({"path": s, "content": s}), &["path", "content"]));
    put("FileWrite", obj(json!({"scope": scope, "namespace": s, "path": s, "content": s}), &["path", "content"]));
    put("FileDelete", obj(json!({"scope": scope, "namespace": s, "path": s, "recursive": b}), &["path"]));
    put("FileUploaded", obj(json!({"ok": b, "bytes": i, "sha256": s}), &["ok", "bytes", "sha256"]));
    put("FileBytes", json!({"type": "string", "format": "binary", "description": "the file's bytes (application/octet-stream)"}));
    put("GitClone", obj(json!({"scope": scope, "namespace": s, "url": s, "folder": s, "branch": s, "depth": i}), &["url"]));
    put("GitCloned", obj(json!({"folder": s, "removed_links": i}), &["folder", "removed_links"]));
    put("GitCloneList", obj(json!({"clones": any_arr}), &["clones"]));
    put("FileMove", obj(json!({"scope": scope, "namespace": s, "from": s, "to": s, "overwrite": b}), &["from", "to"]));
    put("FileMkdir", obj(json!({"scope": scope, "namespace": s, "path": s}), &["path"]));
    put("GitStatus", obj(json!({"folder": s, "branch": s, "detached": b, "upstream": s, "ahead": i, "behind": i, "changed": i, "untracked": i, "conflicted": i, "dirty": b, "origin": {"type": ["string", "null"]}, "changes": any_arr, "more": i}), &["folder", "branch", "changes"]));
    put("GitBranches", obj(json!({"current": {"type": ["string", "null"]}, "branches": any_arr}), &["branches"]));
    put("GitLog", obj(json!({"page": i, "per": i, "more": b, "commits": any_arr}), &["page", "per", "more", "commits"]));
    put("GitDiff", obj(json!({"files": any_arr, "too_large": b}), &["files"]));
    put("ScheduleStatus", json!({"type": "object"}));
    put("SchedulePreviewed", obj(json!({"expr": s, "zone": s, "next": any_arr}), &["expr", "zone", "next"]));
    put("ScheduleList", obj(json!({"schedules": any_arr, "limit": i}), &["schedules", "limit"]));
    put("ScheduleSave", obj(json!({"flow": s, "team": s, "environment": s, "run_as": {"type": "string", "enum": ["owner", "team-service"]}, "trigger": {"type": "object"}}), &["flow", "team", "trigger"]));
    put("ScheduleSaved", json!({"type": "object"}));
    put("FlowRun", obj(json!({"draft": b, "inputs": {"type": "object"}}), &[]));
    put("FlowRunStarted", obj(json!({"run": s, "flow": s}), &["run", "flow"]));
    put("FlowRunList", obj(json!({"runs": any_arr, "page": i, "per": i, "more": b}), &["runs", "page", "per", "more"]));
    put("FlowRunView", obj(json!({"id": s, "flow": s, "status": s, "nodes": any_arr}), &["id", "flow", "status"]));
    put("TenantConfig", json!({"type": "object"}));
    put("TenantConfigPut", obj(json!({"enabled": b, "values": {"type": "object"}, "secrets": {"type": "object", "description": "setting name to secret value; write-only"}, "namespace": s, "name": s}), &[]));
    put("TeamApprovers", obj(json!({"people": strs, "groups": strs}), &[]));
    put("TeamGroup", obj(json!({"members": strs}), &["members"]));
    put("TenantConfigRollback", obj(json!({"version": i}), &["version"]));
    put("TenantConfigImport", obj(json!({"doc": {"type": "object"}, "dry_run": b, "replace": b}), &["doc"]));
    put("NotifList", obj(json!({"notifications": any_arr, "next_before": {"type": ["integer", "null"]}}), &["notifications"]));
    put("NotifCount", obj(json!({"unread": i, "attention": i, "approvals": i, "overdue": i, "total": i, "red": b}), &["unread", "approvals", "total", "red"]));
    put("NotifSnooze", obj(json!({"minutes": i, "until": i}), &[]));
    put("NotifChannels", obj(json!({"channels": any_arr}), &["channels"]));
    put("NotifOptout", obj(json!({"channel": s, "out": b}), &["channel", "out"]));
    put("DataResult", obj(json!({"result": s}), &["result"]));
    m
}

fn schema_ref(name: &str) -> Value {
    json!({"$ref": format!("#/components/schemas/{name}")})
}

/// The document for `profile`.
pub fn document(profile: Profile) -> Value {
    let mut paths = Map::new();
    for r in ROUTES.iter().filter(|r| r.in_profile(profile)) {
        let mut op = Map::new();
        op.insert("operationId".into(), json!(r.id));
        op.insert("summary".into(), json!(r.summary));
        let params: Vec<Value> = param_names(r.path).into_iter().map(|n| json!({"name": n, "in": "path", "required": true, "schema": {"type": "string"}})).collect();
        if !params.is_empty() {
            op.insert("parameters".into(), json!(params));
        }
        if !r.request.is_empty() {
            op.insert("requestBody".into(), json!({"required": true, "content": {"application/json": {"schema": schema_ref(r.request)}}}));
        }
        let mut resp = Map::new();
        resp.insert("200".into(), json!({"description": "OK", "content": {"application/json": {"schema": schema_ref(r.response)}}}));
        let err = |d: &str| json!({"description": d, "content": {"application/json": {"schema": schema_ref("Error")}}});
        if r.access != Access::Public {
            resp.insert("401".into(), err("No or invalid token"));
            resp.insert("403".into(), err("The role does not allow this"));
            resp.insert("429".into(), err("Rate limit"));
            op.insert("security".into(), json!([{"bearer": []}]));
        } else {
            op.insert("security".into(), json!([]));
        }
        resp.insert("400".into(), err("Invalid request"));
        if r.audit {
            op.insert("x-audited".into(), json!(true));
        }
        op.insert("x-access".into(), json!(match r.access {
            Access::Public => "public",
            Access::User => "user",
            Access::Admin => "admin",
        }));
        op.insert("responses".into(), Value::Object(resp));
        paths.entry(r.path.to_string()).or_insert_with(|| json!({})).as_object_mut().map(|p| p.insert(r.method.to_ascii_lowercase(), Value::Object(op)));
    }
    json!({
        "openapi": "3.1.0",
        "info": {"title": "IYAKKI web service", "version": API_VERSION, "description": format!("Profile {}. HTTPS only; sign in at /api/v1/login and send the token as `Authorization: Bearer ...`. No cookies, no CORS unless the administrator lists origins.", profile.name()), "license": {"name": "Apache-2.0", "identifier": "Apache-2.0"}},
        "servers": [{"url": "/"}],
        "paths": Value::Object(paths),
        "components": {"securitySchemes": {"bearer": {"type": "http", "scheme": "bearer"}}, "schemas": Value::Object(schemas())},
    })
}

#[cfg(test)]
mod drift_tests {
    use super::*;

    fn checked_in(profile: Profile, file: &str) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/api").join(file);
        let now = serde_json::to_string_pretty(&document(profile)).unwrap() + "\n";
        // `IYAKKI_WRITE_OPENAPI=1 cargo test -p iyakki-agent --lib -- the_checked_in_openapi` rewrites the files after a route change
        if std::env::var_os("IYAKKI_WRITE_OPENAPI").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &now).unwrap();
        }
        let have = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{} is missing: run the test with IYAKKI_WRITE_OPENAPI=1", path.display()));
        assert!(have == now, "{file} is out of date with the route table: run the test with IYAKKI_WRITE_OPENAPI=1 and check the change in");
    }

    #[test]
    fn the_checked_in_openapi_documents_are_the_generated_ones() {
        checked_in(Profile::AgentApi, "openapi-agent-api.json");
        checked_in(Profile::Cloud, "openapi-cloud.json");
    }
}
