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
    put("Health", obj(json!({"status": s, "profile": s}), &["status"]));
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
    put("DataCall", obj(json!({"tool": {"type": "string", "enum": ["data_open", "data_describe", "data_query"]}, "args": {"type": "object"}}), &["tool", "args"]));
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
