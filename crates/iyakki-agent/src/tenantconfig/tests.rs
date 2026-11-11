use super::api::{self, ConfigAudit, Caller, Env};
use super::approvals;
use super::{discovery, runtime, vault};
use super::runtime::DeployStep;
use super::vault::SecretState;
use super::enforce::{self, AgentUse, Deploy, DeployAsk};
use super::resolve::{resolve, state_of, Layer, LayerKind, State};
use super::schema::{self, SecretRef};
use super::vault::{MemVault, SecretStore};
use super::{Item, MAX_VERSIONS};
use crate::cloudroles::{NsRole, Roles, TenantRole};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Rec(Mutex<Vec<String>>);
impl ConfigAudit for Rec {
    fn record(&self, who: &str, event: &str, detail: &str) {
        self.0.lock().unwrap().push(format!("{who} {event} {detail}"));
    }
}
impl Rec {
    fn all(&self) -> String {
        self.0.lock().unwrap().join("\n")
    }
}

fn dir(tag: &str) -> PathBuf {
    let d = crate::testdir::unique("tenantconfig", tag);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// olga owner, adam admin, mia member (admin of team web), max member (editor of web), vic member (admin of team data), eve outsider.
fn roles() -> Roles {
    let mut r = Roles::new("olga").unwrap();
    r.add_tenant_member("olga", "adam", TenantRole::Admin).unwrap();
    for m in ["mia", "max", "vic"] {
        r.add_tenant_member("olga", m, TenantRole::Member).unwrap();
    }
    r.create_namespace("adam", "web").unwrap();
    r.add_namespace_member("adam", "web", "mia", NsRole::Admin).unwrap();
    r.add_namespace_member("mia", "web", "max", NsRole::Editor).unwrap();
    r.create_namespace("adam", "data").unwrap();
    r.add_namespace_member("adam", "data", "vic", NsRole::Admin).unwrap();
    // the creator is a tenant admin, not a member of the teams
    r.namespaces.get_mut("web").unwrap().remove("adam");
    r.namespaces.get_mut("data").unwrap().remove("adam");
    r
}

struct T {
    dir: PathBuf,
    rec: Arc<Rec>,
    vault: Arc<MemVault>,
    roles: Roles,
}

fn t(tag: &str) -> T {
    T { dir: dir(tag), rec: Arc::new(Rec::default()), vault: MemVault::new(), roles: roles() }
}

impl T {
    fn env(&self) -> Env<'_> {
        Env { dir: &self.dir, vault: self.vault.clone(), audit: self.rec.as_ref() }
    }
    fn who<'a>(&'a self, u: &'a str) -> Caller<'a> {
        Caller { user: u, roles: Some(&self.roles) }
    }
    /// A PUT that acknowledges every warning (the tests of acknowledgement call `api::put` directly).
    fn put(&self, u: &str, kind: &str, p: &[&str], mut body: Value) -> Result<Value, api::ApiErr> {
        let p: Vec<String> = p.iter().map(|s| s.to_string()).collect();
        if body.get("ack_warnings").is_none() {
            body["ack_warnings"] = super::safety::rules_table();
        }
        api::put(&self.env(), &self.who(u), kind, &p, body)
    }
    fn get(&self, u: &str, kind: &str, p: &[&str]) -> Result<Value, api::ApiErr> {
        let p: Vec<String> = p.iter().map(|s| s.to_string()).collect();
        api::get(&self.env(), &self.who(u), kind, &p)
    }
    fn write_file(&self, doc: Value) {
        let d = self.dir.join("cloud").join("config");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("tenant-config.json"), doc.to_string()).unwrap();
    }
}

fn item(kind: &str, name: &str, ns: Option<&str>, enabled: bool, values: Value) -> Item {
    Item { kind: kind.into(), name: name.into(), namespace: ns.map(String::from), enabled, managed: false, values: values.as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect() }
}

fn status<V>(r: &Result<V, api::ApiErr>) -> u16 {
    match r {
        Ok(_) => 200,
        Err(e) => e.status,
    }
}

const SHARED: fn() -> Value = || json!({"enabled": true, "values": {"mode": "shared", "terms_ack": true, "allowed_models": ["claude-*", "sonnet"], "daily_requests_per_person": 2, "daily_requests_tenant": 3, "monthly_requests_team": 100}, "secrets": {"token": "sk-ant-SUPERSECRET-123"}});

// ---------------------------------------------------------------------------------------------------------------- registry and validation

#[test]
fn tcfg_registry_has_the_first_release_kinds_and_refuses_planned_ones() {
    for k in ["agent.claude", "agent.copilot", "agent.other", "models.provider", "git.remote", "k3s.cluster", "k3s.environments", "egress.policy", "media.policy"] {
        assert!(schema::kind(k).is_some(), "{k}");
    }
    assert!(schema::kind_from_url("k3s-cluster").is_some() && schema::kind_from_url("k3s.cluster").is_some());
    let p = schema::validate(&item("mcp.server", "x", None, true, json!({})), &[]);
    assert!(p[0].message.contains("planned"));
    assert!(schema::describe()["kinds"].as_array().unwrap().iter().any(|k| k["fields"].as_array().unwrap().iter().any(|f| f["secret"] == true)));
}

#[test]
fn tcfg_validation_checks_types_limits_unknown_fields_and_never_echoes_a_secret() {
    let bad = item("agent.claude", "default", None, true, json!({"mode": "shared", "token": "sk-ant-INLINE-SECRET", "bogus": 1, "daily_requests_per_person": 0, "terms_ack": true}));
    let p = schema::validate(&bad, &[]);
    let text = format!("{p:?}");
    assert!(p.iter().any(|x| x.field == "token") && p.iter().any(|x| x.field == "bogus") && p.iter().any(|x| x.field == "daily_requests_per_person"));
    assert!(!text.contains("INLINE-SECRET"), "{text}");
    // a reference of another namespace is refused
    let wrong = item("agent.claude", "default", None, true, json!({"mode": "per_person", "token": "vault:agent.copilot/x"}));
    assert!(schema::validate(&wrong, &[]).iter().any(|x| x.field == "token"));
    // hosts, urls, devices
    assert!(!schema::validate(&item("egress.policy", "default", None, true, json!({"allowed_domains": ["Bad Host", "*.ok.com"]})), &[]).is_empty());
    assert!(schema::validate(&item("egress.policy", "default", None, true, json!({"allowed_domains": ["*.ok.com", "a.org"]})), &[]).is_empty());
    assert!(!schema::validate(&item("git.remote", "gh", None, true, json!({"url_prefixes": ["https://user:pw@github.com/x"]})), &[]).is_empty());
    assert!(!schema::validate(&item("media.policy", "default", None, true, json!({"media": "allow", "deny_devices": ["laser"]})), &[]).is_empty());
    assert!(!schema::validate(&item("egress.policy", "other", None, true, json!({})), &[]).is_empty(), "single kinds are named default");
}

#[test]
fn tcfg_a_shared_agent_needs_token_terms_and_limits() {
    let ok = item("agent.copilot", "default", None, true, json!({"mode": "shared", "token": "vault:agent.copilot/default-token", "terms_ack": true, "daily_requests_per_person": 5, "monthly_requests_team": 50}));
    assert!(schema::validate(&ok, &[]).is_empty());
    for missing in ["token", "terms_ack", "daily_requests_per_person"] {
        let mut x = ok.clone();
        x.values.remove(missing);
        assert!(schema::validate(&x, &[]).iter().any(|p| p.field == missing), "{missing}");
    }
    let mut no_ack = ok.clone();
    no_ack.values.insert("terms_ack".into(), json!(false));
    assert!(schema::validate(&no_ack, &[]).iter().any(|p| p.field == "terms_ack"));
    // per person is the default and needs nothing
    assert!(schema::validate(&item("agent.claude", "default", None, true, json!({})), &[]).is_empty());
}

// ---------------------------------------------------------------------------------------------------------------- precedence

#[test]
fn tcfg_precedence_lower_layers_only_narrow_and_off_wins() {
    let spec = schema::kind("egress.policy").unwrap();
    let tenant = item("egress.policy", "default", None, true, json!({"allowed_domains": ["*.example.com", "api.other.org"]}));
    let team = item("egress.policy", "default", Some("web"), true, json!({"allowed_domains": ["a.example.com", "evil.com", "api.other.org"]}));
    let e = resolve(spec, &[Layer { kind: LayerKind::Team, item: &team }, Layer { kind: LayerKind::Tenant, item: &tenant }]);
    assert_eq!(e.values["allowed_domains"], json!(["a.example.com", "api.other.org"]), "a team cannot add evil.com");
    assert_eq!(e.values["mode"], "allow_list");
    // the apex is not covered by *.example.com
    let apex = item("egress.policy", "default", Some("web"), true, json!({"allowed_domains": ["example.com"]}));
    assert_eq!(resolve(spec, &[Layer { kind: LayerKind::Team, item: &apex }, Layer { kind: LayerKind::Tenant, item: &tenant }]).values["allowed_domains"], json!([]));
    // the operator's value beats the tenant's for a First field, and a stricter enum wins
    let op = item("egress.policy", "default", None, true, json!({"mode": "deny_all"}));
    let e = resolve(spec, &[Layer { kind: LayerKind::Tenant, item: &tenant }, Layer { kind: LayerKind::Operator, item: &op }]);
    assert_eq!(e.values["mode"], "deny_all");
    // deny wins: a team that is off switches the item off whatever the tenant says
    let off = item("egress.policy", "default", Some("web"), false, json!({}));
    let e = resolve(spec, &[Layer { kind: LayerKind::Tenant, item: &tenant }, Layer { kind: LayerKind::Team, item: &off }]);
    assert!(!e.enabled && e.off_by == Some(LayerKind::Team));
    // nothing configured: does not exist, off
    let e = resolve(spec, &[]);
    assert!(!e.exists && !e.enabled);
}

#[test]
fn tcfg_a_person_layer_counts_only_where_the_kind_allows_it() {
    let tenant = item("models.provider", "p", None, true, json!({"base_url": "https://m.example.com", "allowed_models": ["*"]}));
    let person = item("models.provider", "p", None, true, json!({"allowed_models": ["tiny"]}));
    let spec = schema::kind("models.provider").unwrap();
    let e = resolve(spec, &[Layer { kind: LayerKind::Tenant, item: &tenant }, Layer { kind: LayerKind::Person, item: &person }]);
    assert_eq!(e.values["allowed_models"], json!(["*"]), "person override is forbidden for providers");
    let a1 = item("agent.claude", "default", None, true, json!({"allowed_models": ["*"]}));
    let a2 = item("agent.claude", "default", None, true, json!({"allowed_models": ["sonnet"]}));
    let e = resolve(schema::kind("agent.claude").unwrap(), &[Layer { kind: LayerKind::Tenant, item: &a1 }, Layer { kind: LayerKind::Person, item: &a2 }]);
    assert_eq!(e.values["allowed_models"], json!(["sonnet"]));
}

// ---------------------------------------------------------------------------------------------------------------- secrets and state

#[test]
fn tcfg_a_secret_never_reaches_the_config_versions_audit_or_output() {
    let x = t("secret");
    let r = x.put("adam", "agent.claude", &[], SHARED()).unwrap();
    assert_eq!(r["state"], "ready");
    let mut everything = String::new();
    let walk = |d: &std::path::Path, out: &mut String| {
        fn rec(p: &std::path::Path, out: &mut String) {
            for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
                if e.path().is_dir() {
                    rec(&e.path(), out);
                } else {
                    out.push_str(&std::fs::read_to_string(e.path()).unwrap_or_default());
                }
            }
        }
        rec(d, out);
    };
    walk(&x.dir, &mut everything);
    let g = x.get("adam", "agent.claude", &[]).unwrap();
    let l = api::list(&x.env(), &x.who("adam")).unwrap();
    let ex = api::export(&x.env(), &x.who("adam")).unwrap();
    let vs = api::versions(&x.env(), &x.who("adam")).unwrap();
    for out in [everything, g.to_string(), l.to_string(), ex.to_string(), vs.to_string(), x.rec.all()] {
        assert!(!out.contains("SUPERSECRET"), "a secret leaked: {out}");
    }
    assert_eq!(g["values"]["token"], "[set]");
    assert!(ex.to_string().contains("vault:agent.claude/default-token") || ex.to_string().contains("vault:agent.claude/token"));
    // the audit line has hashes and field names
    let a = x.rec.all();
    assert!(a.contains("tenant.config.put") && a.contains("before=none") && a.contains("after=") && a.contains("secrets_set=token"), "{a}");
    // a read-modify-write with the marker keeps the reference
    let again = x.put("adam", "agent.claude", &[], json!({"enabled": true, "values": g["values"]})).unwrap();
    assert_eq!(again["state"], "ready");
}

#[test]
fn tcfg_config_without_a_secret_is_not_ready_and_a_secret_set_later_makes_it_ready_without_editing_the_config() {
    let x = t("later");
    let body = json!({"enabled": true, "values": {"mode": "shared", "terms_ack": true, "token": "vault:agent.claude/shared-token", "daily_requests_per_person": 5, "monthly_requests_team": 50}});
    let r = x.put("adam", "agent.claude", &[], body).unwrap();
    assert_eq!(r["state"], "secret_missing");
    let v1 = api::versions(&x.env(), &x.who("adam")).unwrap()["current"].clone();
    // the secret arrives in the vault by another route (the vault's own page)
    x.vault.put(&SecretRef::parse("vault:agent.claude/shared-token").unwrap(), "tok").unwrap();
    assert_eq!(x.get("adam", "agent.claude", &[]).unwrap()["state"], "ready");
    assert_eq!(api::versions(&x.env(), &x.who("adam")).unwrap()["current"], v1, "the config was not edited");
    // a locked vault: the item is held back and says why
    x.vault.set_locked(true);
    let g = x.get("adam", "agent.claude", &[]).unwrap();
    assert_eq!(g["state"], "vault_locked");
    assert_eq!(g["values"]["token"], "[vault locked]");
    // writing a secret into a locked vault is refused and stores no config
    assert_eq!(status(&x.put("adam", "agent.copilot", &[], SHARED())), 409);
    assert!(x.get("adam", "agent.copilot", &[]).is_err());
    // an item that is switched off asks nothing of the vault
    x.vault.set_locked(false);
    let off = x.put("adam", "agent.claude", &[], json!({"enabled": false, "values": {}})).unwrap();
    assert_eq!(off["state"], "disabled");
}

#[test]
fn tcfg_a_dry_run_writes_nothing() {
    let x = t("dry");
    let v = api::validate(&x.env(), &x.who("adam"), "agent.claude", SHARED()).unwrap();
    assert_eq!(v["valid"], true);
    assert_eq!(v["secrets_to_store"], json!(["token"]));
    assert!(!v.to_string().contains("SUPERSECRET"));
    assert!(x.get("adam", "agent.claude", &[]).is_err());
    assert!(!x.dir.join("cloud/config/tenant-config.local.json").exists());
    let bad = api::validate(&x.env(), &x.who("adam"), "egress.policy", json!({"enabled": true, "values": {"allowed_domains": ["No Good"]}})).unwrap();
    assert_eq!(bad["valid"], false);
    assert_eq!(x.vault.state(&SecretRef::parse("vault:agent.claude/token").unwrap()), super::vault::SecretState::Missing);
}

// ---------------------------------------------------------------------------------------------------------------- versions, roles, files

#[test]
fn tcfg_versions_keep_ten_and_a_rollback_restores_one_as_a_new_version() {
    let x = t("versions");
    for i in 0..13 {
        let doms = vec![format!("h{i}.example.com")];
        x.put("adam", "egress.policy", &[], json!({"enabled": true, "values": {"allowed_domains": doms}})).unwrap();
    }
    let v = api::versions(&x.env(), &x.who("adam")).unwrap();
    assert_eq!(v["current"], 13);
    let list = v["versions"].as_array().unwrap();
    assert_eq!(list.len(), MAX_VERSIONS);
    assert_eq!(list[0]["version"], 13);
    assert_eq!(list[9]["version"], 4);
    assert!(api::rollback(&x.env(), &x.who("adam"), json!({"version": 2})).is_err(), "version 2 is no longer kept");
    api::rollback(&x.env(), &x.who("adam"), json!({"version": 8})).unwrap();
    assert_eq!(x.get("adam", "egress.policy", &[]).unwrap()["values"]["allowed_domains"], json!(["h7.example.com"]));
    assert_eq!(api::versions(&x.env(), &x.who("adam")).unwrap()["current"], 14);
    assert!(x.rec.all().contains("tenant.config.rollback"));
    // a member may not
    assert_eq!(status(&api::rollback(&x.env(), &x.who("max"), json!({"version": 8}))), 403);
    assert_eq!(status(&api::versions(&x.env(), &x.who("max"))), 403);
}

#[test]
fn tcfg_roles_decide_who_reads_values_and_who_writes() {
    let x = t("roles");
    let eg = json!({"enabled": true, "values": {"allowed_domains": ["*.example.com"]}});
    assert_eq!(status(&x.put("max", "egress.policy", &[], eg.clone())), 403, "a member does not write");
    assert_eq!(status(&x.put("eve", "egress.policy", &[], eg.clone())), 403, "a stranger neither");
    assert_eq!(status(&api::list(&x.env(), &x.who("eve"))), 403);
    x.put("adam", "egress.policy", &[], eg).unwrap();
    // a member sees the state of an egress policy and its values (kind says members may); an agent's values only admins see
    assert_eq!(x.get("max", "egress.policy", &[]).unwrap()["values"]["allowed_domains"], json!(["*.example.com"]));
    x.put("adam", "agent.claude", &[], json!({"enabled": true, "values": {"allowed_models": ["sonnet"]}})).unwrap();
    let g = x.get("max", "agent.claude", &[]).unwrap();
    assert_eq!(g["state"], "ready");
    assert!(g.get("values").is_none(), "{g}");
    assert_eq!(g["can"]["write"], false);
    // a team's own admin writes the team's entry and narrows only
    assert_eq!(status(&x.put("mia", "egress.policy", &["web"], json!({"enabled": true, "values": {"allowed_domains": ["a.example.com", "evil.com"]}}))), 200);
    assert_eq!(x.get("mia", "egress.policy", &["web"]).unwrap()["effective"]["allowed_domains"], json!(["a.example.com"]));
    assert_eq!(status(&x.put("mia", "egress.policy", &["data"], json!({"enabled": true, "values": {}}))), 404, "not her team");
    assert_eq!(status(&x.put("max", "egress.policy", &["web"], json!({"enabled": true, "values": {}}))), 403, "an editor does not administer");
    assert_eq!(status(&x.put("mia", "agent.claude", &[], json!({"enabled": true, "values": {}}))), 403, "the tenant level is the tenant admin's");
    // delete
    assert_eq!(status(&api::delete(&x.env(), &x.who("mia"), "egress.policy", &["web".to_string()])), 200);
    assert!(x.get("mia", "egress.policy", &["web"]).is_err());
}

#[test]
fn tcfg_an_item_the_operators_file_manages_is_read_only_and_shows_its_source() {
    let x = t("managed");
    x.write_file(json!({"schema": 1, "items": [
        {"kind": "egress.policy", "enabled": true, "managed": true, "values": {"allowed_domains": ["*.corp.example"]}},
        {"kind": "media.policy", "enabled": true, "values": {"media": "deny"}}
    ]}));
    let g = x.get("adam", "egress.policy", &[]).unwrap();
    assert_eq!((g["source"].as_str(), g["managed"].clone(), g["can"]["write"].clone()), (Some("file"), json!(true), json!(false)));
    assert_eq!(status(&x.put("adam", "egress.policy", &[], json!({"enabled": true, "values": {}}))), 409);
    assert_eq!(status(&api::delete(&x.env(), &x.who("adam"), "egress.policy", &[])), 409);
    // an unmanaged file item is a default the API may override
    assert_eq!(x.get("adam", "media.policy", &[]).unwrap()["source"], "file");
    x.put("adam", "media.policy", &[], json!({"enabled": true, "values": {"media": "allow"}})).unwrap();
    assert_eq!(x.get("adam", "media.policy", &[]).unwrap()["source"], "api");
    // the operator's managed value is the top layer for a team's narrowing
    assert_eq!(status(&x.put("mia", "egress.policy", &["web"], json!({"enabled": true, "values": {"allowed_domains": ["x.corp.example", "y.other.org"]}}))), 200);
    assert_eq!(x.get("mia", "egress.policy", &["web"]).unwrap()["effective"]["allowed_domains"], json!(["x.corp.example"]));
    // a broken file leaves no operator items and the list says so
    std::fs::write(x.dir.join("cloud/config/tenant-config.json"), "{ nope").unwrap();
    assert_eq!(api::list(&x.env(), &x.who("adam")).unwrap()["file_error"], true);
}

#[test]
fn tcfg_export_holds_no_secret_and_an_import_round_trips_all_or_nothing() {
    let x = t("export");
    x.put("adam", "agent.claude", &[], SHARED()).unwrap();
    x.put("adam", "egress.policy", &[], json!({"enabled": true, "values": {"allowed_domains": ["a.example.com"]}})).unwrap();
    let ex = api::export(&x.env(), &x.who("adam")).unwrap();
    assert!(!ex.to_string().contains("SUPERSECRET"));
    let y = t("import");
    let dry = api::import(&y.env(), &y.who("adam"), json!({"doc": ex, "dry_run": true})).unwrap();
    assert_eq!(dry["would_import"], 2);
    assert!(y.get("adam", "egress.policy", &[]).is_err(), "a dry run writes nothing");
    api::import(&y.env(), &y.who("adam"), json!({"doc": ex})).unwrap();
    assert_eq!(y.get("adam", "agent.claude", &[]).unwrap()["state"], "secret_missing", "the vault is not exported");
    // all or nothing
    let bad = json!({"doc": {"items": [{"kind": "egress.policy", "enabled": true, "values": {"allowed_domains": ["ok.com"]}}, {"kind": "git.remote", "name": "g", "enabled": true, "values": {"url_prefixes": ["http://insecure"]}}]}, "replace": true});
    assert_eq!(status(&api::import(&y.env(), &y.who("adam"), bad)), 400);
    assert_eq!(y.get("adam", "egress.policy", &[]).unwrap()["values"]["allowed_domains"], json!(["a.example.com"]));
}

// ---------------------------------------------------------------------------------------------------------------- enforcement

#[test]
fn tcfg_the_agent_gate_is_off_by_default_and_counts_a_shared_agent() {
    let x = t("gate");
    let gate = |u: &str, m: &str| enforce::agent_gate(&x.env(), "agent.claude", "default", None, u, Some(m), 1_000_000);
    assert!(matches!(gate("max", "sonnet"), AgentUse::Refused(w) if w.contains("not switched on")));
    x.put("adam", "agent.claude", &[], SHARED()).unwrap();
    assert_eq!(gate("max", "sonnet"), AgentUse::Allowed { shared: true });
    assert!(matches!(gate("max", "gpt-x"), AgentUse::Refused(w) if w.contains("model")));
    assert_eq!(gate("max", "claude-opus"), AgentUse::Allowed { shared: true });
    assert!(matches!(gate("max", "sonnet"), AgentUse::Refused(w) if w.contains("limit")), "2 per person per day");
    assert_eq!(gate("mia", "sonnet"), AgentUse::Allowed { shared: true });
    assert!(matches!(gate("vic", "sonnet"), AgentUse::Refused(w) if w.contains("limit")), "3 for the tenant per day");
    // the next day the counters start again
    assert_eq!(enforce::agent_gate(&x.env(), "agent.claude", "default", None, "max", Some("sonnet"), 1_000_000 + 86_400), AgentUse::Allowed { shared: true });
    let a = x.rec.all();
    assert!(a.contains("tenant.agent.use") && a.contains("tenant.agent.refused") && !a.contains("SUPERSECRET"));
    // switching it off refuses at once
    x.put("adam", "agent.claude", &[], json!({"enabled": false, "values": {}})).unwrap();
    assert!(matches!(gate("mia", "sonnet"), AgentUse::Refused(_)));
    // a missing secret or a locked vault refuses too
    x.put("adam", "agent.claude", &[], json!({"enabled": true, "values": {"mode": "shared", "terms_ack": true, "token": "vault:agent.claude/other", "daily_requests_per_person": 5, "monthly_requests_team": 50, "allowed_models": ["*"]}})).unwrap();
    assert!(matches!(gate("mia", "sonnet"), AgentUse::Refused(w) if w.contains("secret_missing")));
    // per person mode: the person's own login, nothing to count
    x.put("adam", "agent.claude", &[], json!({"enabled": true, "values": {"allowed_models": ["*"]}})).unwrap();
    assert_eq!(gate("mia", "sonnet"), AgentUse::Allowed { shared: false });
}

#[test]
fn tcfg_a_shared_token_is_handed_over_for_a_named_action_only() {
    let x = t("use");
    x.put("adam", "agent.claude", &[], SHARED()).unwrap();
    let mut seen = String::new();
    let out = enforce::with_agent_token(&x.env(), "agent.claude", "default", None, "max", "start-claude-code", &mut |tok| {
        seen = tok.to_string();
        Ok("started".into())
    });
    assert_eq!(out, Ok("started".into()));
    assert_eq!(seen, "sk-ant-SUPERSECRET-123");
    assert_eq!(x.vault.uses.lock().unwrap()[0].0, "start-claude-code");
    assert!(x.rec.all().contains("tenant.secret.use") && !x.rec.all().contains("SUPERSECRET"));
}

#[test]
fn tcfg_media_policy_comes_from_the_tenant_config() {
    use crate::browsermedia::Policy;
    let x = t("media");
    let role = iyakki_sec::policy::Role::User;
    // no item: the machine's setting stays
    assert_eq!(enforce::media_policy_for(&x.dir, None, "max", role, Policy::allow_all()), Policy::allow_all());
    x.put("adam", "media.policy", &[], json!({"enabled": true, "values": {"media": "allow", "deny_devices": ["camera"]}})).unwrap();
    let p = enforce::media_policy_for(&x.dir, None, "max", role, Policy::deny_all());
    assert!(p.mic && !p.camera && p.audio && p.clip_read);
    // a guest keeps the role's smaller grant
    let g = enforce::media_policy_for(&x.dir, None, "g", iyakki_sec::policy::Role::Guest, Policy::deny_all());
    assert!(!g.mic && g.audio);
    x.put("adam", "media.policy", &[], json!({"enabled": true, "values": {"media": "deny"}})).unwrap();
    assert_eq!(enforce::media_policy_for(&x.dir, None, "max", role, Policy::allow_all()), Policy::deny_all());
    x.put("adam", "media.policy", &[], json!({"enabled": false, "values": {"media": "allow"}})).unwrap();
    assert_eq!(enforce::media_policy_for(&x.dir, None, "max", role, Policy::allow_all()), Policy::deny_all(), "a policy that is off denies");
}

#[test]
fn tcfg_egress_git_and_provider_hooks_read_the_config() {
    let x = t("hooks");
    assert_eq!(enforce::egress_allows(&x.dir, None, "a.example.com"), None);
    x.put("adam", "egress.policy", &[], json!({"enabled": true, "values": {"allowed_domains": ["*.example.com", "api.ok.org"]}})).unwrap();
    assert_eq!(enforce::egress_allows(&x.dir, None, "A.example.com"), Some(true));
    assert_eq!(enforce::egress_allows(&x.dir, None, "example.com"), Some(false));
    assert_eq!(enforce::egress_allows(&x.dir, None, "evil.org"), Some(false));
    x.put("adam", "git.remote", &["gh"], json!({"enabled": true, "values": {"url_prefixes": ["https://github.com/acme/"], "access": "push"}, "secrets": {"credential": "ghp_SECRET"}})).unwrap();
    assert_eq!(enforce::git_remote_allows(&x.env(), None, "gh", "https://github.com/acme/app.git"), (true, true));
    assert_eq!(enforce::git_remote_allows(&x.env(), None, "gh", "https://github.com/other/app.git"), (false, false));
    x.put("adam", "git.remote", &["ro"], json!({"enabled": true, "values": {"url_prefixes": ["https://github.com/"]}})).unwrap();
    assert_eq!(enforce::git_remote_allows(&x.env(), None, "ro", "https://github.com/x/y"), (true, false));
    x.put("adam", "models.provider", &["m"], json!({"enabled": true, "values": {"base_url": "http://ollama.models.svc:11434", "allowed_models": ["tiny"]}})).unwrap();
    assert_eq!(enforce::provider_models(&x.env(), None, "m"), Some(vec!["tiny".to_string()]));
    assert!(!schema::validate(&item("models.provider", "m", None, true, json!({"base_url": "http://evil.example.com"})), &[]).is_empty());
}

// ---------------------------------------------------------------------------------------------------------------- k3s

fn binding(x: &T, team: &str, env_name: &str, extra: Value) -> Result<Value, api::ApiErr> {
    let mut vals = json!({"api_server": format!("https://{team}-{env_name}.k3s.example.com:6443"), "allowed_registries": ["registry.example.com"], "quota_pods": 20, "deploy_role": "editor"});
    for (k, v) in extra.as_object().unwrap() {
        vals[k] = v.clone();
    }
    let user = if team == "web" { "mia" } else { "vic" };
    x.put(user, "k3s.cluster", &[team, env_name], json!({"enabled": true, "values": vals, "secrets": {"credential": format!("kubeconfig-{team}-{env_name}")}}))
}

fn ask<'a>(x: &'a T, team: &'a str, env_name: &'a str, who: &'a str, rank: i32) -> DeployAsk<'a> {
    DeployAsk { team, environment: env_name, requester: who, requester_rank: rank, approval: None, promoted_from: None, registry: None, roles: &x.roles, guest: false }
}

#[test]
fn tcfg_each_k3s_binding_has_its_own_credential_and_dev_cannot_serve_prod() {
    let x = t("k3s-cred");
    assert_eq!(binding(&x, "web", "dev", json!({})).unwrap()["state"], "ready");
    let r = x.get("mia", "k3s.cluster", &["web", "dev"]).unwrap();
    assert_eq!((r["values"]["credential"].as_str(), r["reachability"].as_str()), (Some("[set]"), Some("unknown")));
    // pointing prod at the dev credential is refused, and so is reusing it for a second binding
    let reuse = x.put("mia", "k3s.cluster", &["web", "prod"], json!({"enabled": true, "values": {"api_server": "https://p.example.com", "credential": "vault:k3s.cluster/web-dev-credential"}}));
    assert_eq!(status(&reuse), 400);
    let msg = reuse.unwrap_err().message;
    assert!(msg.contains("credential"), "{msg}");
    // even if a config file sneaks it in, the enforcement point refuses
    x.write_file(json!({"items": [{"kind": "k3s.cluster", "namespace": "web", "name": "prod", "enabled": true, "values": {"api_server": "https://p.example.com", "credential": "vault:k3s.cluster/web-dev-credential", "deploy_role": "editor"}}]}));
    x.vault.put(&SecretRef::parse("vault:k3s.cluster/web-dev-credential").unwrap(), "kc").unwrap();
    let mut a = ask(&x, "web", "prod", "mia", 2);
    a.promoted_from = Some("preprod");
    assert!(matches!(enforce::k3s_decide(&x.env(), &a), Deploy::Deny(w) if w.contains("credential does not belong")));
}

#[test]
fn tcfg_prod_needs_a_second_person_and_promotion_follows_the_chain() {
    let x = t("k3s-eyes");
    for e in ["dev", "preprod", "prod"] {
        binding(&x, "web", e, json!({"deploy_role": if e == "dev" { "editor" } else { "admin" }})).unwrap();
    }
    let env = x.env();
    // dev: an editor deploys alone
    assert_eq!(enforce::k3s_decide(&env, &ask(&x, "web", "dev", "max", 1)), Deploy::Allow);
    assert!(matches!(enforce::k3s_decide(&env, &ask(&x, "web", "dev", "eve", -1)), Deploy::Deny(_)));
    // preprod comes from dev and is for team leads
    let mut pre = ask(&x, "web", "preprod", "mia", 2);
    assert!(matches!(enforce::k3s_decide(&env, &pre), Deploy::Deny(w) if w.contains("promotion order")));
    pre.promoted_from = Some("dev");
    assert_eq!(enforce::k3s_decide(&env, &pre), Deploy::Allow);
    pre.requester = "max";
    pre.requester_rank = 1;
    assert!(matches!(enforce::k3s_decide(&env, &pre), Deploy::Deny(w) if w.contains("admin")));
    // prod comes from preprod only, and needs approval by somebody else (four eyes is the default for the last environment)
    let mut prod = ask(&x, "web", "prod", "mia", 2);
    prod.promoted_from = Some("dev");
    assert!(matches!(enforce::k3s_decide(&env, &prod), Deploy::Deny(w) if w.contains("promotion order")));
    prod.promoted_from = Some("preprod");
    assert_eq!(enforce::k3s_decide(&env, &prod), Deploy::NeedsApproval { approver_role: "tenant_admin".into() }, "mia is the only admin of web: escalated");
    prod.approval = Some("mia");
    assert!(matches!(enforce::k3s_decide(&env, &prod), Deploy::Deny(w) if w.contains("person who asked")));
    prod.approval = Some("max");
    assert!(matches!(enforce::k3s_decide(&env, &prod), Deploy::Deny(w) if w.contains("approver") && w.contains("escalated")));
    prod.approval = Some("adam");
    assert_eq!(enforce::k3s_decide(&env, &prod), Deploy::Allow);
    // every decision was audited with who, what and which environment
    let a = x.rec.all();
    assert!(a.contains("tenant.k3s.deploy") && a.contains("environment=prod") && a.contains("decision=needs_approval") && a.contains("approved_by=adam"));
    assert!(!a.contains("kubeconfig-web"), "no secret value: {a}");
    // an environment outside the chain is refused until the chain names it
    binding(&x, "web", "qa", json!({})).unwrap();
    assert!(matches!(enforce::k3s_decide(&env, &ask(&x, "web", "qa", "mia", 2)), Deploy::Deny(w) if w.contains("chain")));
    x.put("adam", "k3s.environments", &[], json!({"enabled": true, "values": {"order": ["dev", "qa", "preprod", "prod"]}})).unwrap();
    assert!(matches!(enforce::k3s_decide(&env, &ask(&x, "web", "qa", "mia", 2)), Deploy::Deny(w) if w.contains("from dev")));
    prod.approval = None;
    // four eyes can be switched off for a binding that is not the last, never lowered by a team under a tenant that requires it
    x.put("adam", "k3s.cluster", &["prod"], json!({"enabled": true, "values": {"four_eyes": true, "approver_role": "tenant_admin"}})).unwrap();
    assert_eq!(enforce::k3s_decide(&env, &prod), Deploy::NeedsApproval { approver_role: "tenant_admin".into() });
    binding(&x, "web", "prod", json!({"four_eyes": false, "deploy_role": "admin"})).unwrap();
    assert_eq!(enforce::k3s_decide(&env, &prod), Deploy::NeedsApproval { approver_role: "tenant_admin".into() }, "the tenant's rule beats the team's");
}

#[test]
fn tcfg_a_frozen_or_forbidden_environment_denies_and_the_tenant_beats_the_team() {
    let x = t("k3s-freeze");
    binding(&x, "web", "dev", json!({})).unwrap();
    assert_eq!(enforce::k3s_decide(&x.env(), &ask(&x, "web", "dev", "max", 1)), Deploy::Allow);
    binding(&x, "web", "dev", json!({"frozen": true})).unwrap();
    assert!(matches!(enforce::k3s_decide(&x.env(), &ask(&x, "web", "dev", "max", 1)), Deploy::Deny(w) if w.contains("freeze")));
    // a tenant-level freeze of an environment cannot be lifted by the team
    binding(&x, "web", "dev", json!({"frozen": false})).unwrap();
    assert_eq!(enforce::k3s_decide(&x.env(), &ask(&x, "web", "dev", "max", 1)), Deploy::Allow);
    x.put("adam", "k3s.cluster", &["dev"], json!({"enabled": true, "values": {"frozen": true}})).unwrap();
    assert!(matches!(enforce::k3s_decide(&x.env(), &ask(&x, "web", "dev", "max", 1)), Deploy::Deny(w) if w.contains("freeze")));
    // a tenant that switches an environment off forbids it for every team, whatever the team has
    x.put("adam", "k3s.cluster", &["dev"], json!({"enabled": false, "values": {}})).unwrap();
    assert!(matches!(enforce::k3s_decide(&x.env(), &ask(&x, "web", "dev", "max", 1)), Deploy::Deny(w) if w.contains("tenant level")));
    // no binding at all
    assert!(matches!(enforce::k3s_decide(&x.env(), &ask(&x, "data", "dev", "vic", 2)), Deploy::Deny(w) if w.contains("tenant level")), "the forbid reaches teams with no binding too");
    assert!(matches!(enforce::k3s_decide(&x.env(), &ask(&x, "data", "preprod", "vic", 2)), Deploy::Deny(w) if w.contains("no cluster binding")));
    // registries narrow
    x.put("adam", "k3s.cluster", &["dev"], json!({"enabled": true, "values": {"allowed_registries": ["*.example.com"]}})).unwrap();
    let mut a = ask(&x, "web", "dev", "max", 1);
    a.registry = Some("registry.example.com");
    assert_eq!(enforce::k3s_decide(&x.env(), &a), Deploy::Allow);
    a.registry = Some("evil.io");
    assert!(matches!(enforce::k3s_decide(&x.env(), &a), Deploy::Deny(w) if w.contains("registry")));
}

#[test]
fn tcfg_a_team_sees_only_its_own_bindings() {
    let x = t("k3s-see");
    binding(&x, "web", "dev", json!({})).unwrap();
    binding(&x, "data", "dev", json!({})).unwrap();
    let l = |u: &str| api::list(&x.env(), &x.who(u)).unwrap()["items"].as_array().unwrap().iter().filter_map(|i| i["namespace"].as_str().map(String::from)).collect::<Vec<_>>();
    assert_eq!(l("mia"), ["web"]);
    assert_eq!(l("vic"), ["data"]);
    assert_eq!(l("max"), ["web"]);
    assert_eq!(l("adam").len(), 2);
    assert_eq!(status(&x.get("vic", "k3s.cluster", &["web", "dev"])), 404);
    assert_eq!(status(&x.get("vic", "k3s.cluster", &[])), 200);
    assert_eq!(x.get("vic", "k3s.cluster", &[]).unwrap()["items"].as_array().unwrap().len(), 1);
    // an editor may see the binding exists (read_role viewer) but not another team's, and not the credential reference's state of a hidden one
    assert_eq!(x.get("max", "k3s.cluster", &["web", "dev"]).unwrap()["state"], "ready");
}

// ---------------------------------------------------------------------------------------------------------------- random input

#[test]
fn tcfg_random_input_never_panics() {
    let mut s: u64 = 0x9E3779B97F4A7C15;
    let mut next = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let pool = ["", "x", "vault:agent.claude/t", "https://a.b", "*.a.com", "-", "\u{0}", "\u{202e}abc", "mode", "shared", "[set]", "../../etc", "0", "9999999999999999999"];
    let kinds: Vec<&str> = schema::KINDS.iter().map(|k| k.id).chain(["nope", "mcp.server"]).collect();
    let x = t("fuzz");
    fn val(n: &mut dyn FnMut() -> u64, pool: &[&str], depth: u32) -> Value {
        match n() % 7 {
            0 => json!(n() as i64),
            1 => json!(n() % 2 == 0),
            2 => Value::Null,
            3 if depth < 3 => Value::Array((0..n() % 4).map(|_| val(n, pool, depth + 1)).collect()),
            4 if depth < 3 => Value::Object((0..n() % 4).map(|_| (pool[(n() % pool.len() as u64) as usize].to_string(), val(n, pool, depth + 1))).collect()),
            _ => json!(pool[(n() % pool.len() as u64) as usize]),
        }
    }
    for _ in 0..400 {
        let kind = kinds[(next() % kinds.len() as u64) as usize];
        let values = val(&mut next, &pool, 0);
        let item = Item { kind: kind.into(), name: pool[(next() % pool.len() as u64) as usize].into(), namespace: if next() % 2 == 0 { Some(pool[(next() % pool.len() as u64) as usize].into()) } else { None }, enabled: next() % 2 == 0, managed: false, values: values.as_object().cloned().unwrap_or_default().into_iter().collect() };
        let _ = schema::validate(&item, &[]);
        let body = json!({"enabled": next() % 2 == 0, "values": values, "secrets": {pool[(next() % pool.len() as u64) as usize]: pool[(next() % pool.len() as u64) as usize]}});
        let _ = api::validate(&x.env(), &x.who("adam"), kind, body.clone());
        let _ = api::put(&x.env(), &x.who("adam"), kind, &[], body);
        let eff = resolve(schema::kind("agent.claude").unwrap(), &[Layer { kind: LayerKind::Tenant, item: &item }]);
        let _ = state_of(schema::kind("agent.claude").unwrap(), &eff, x.vault.as_ref());
        assert!(!x.rec.all().contains("\u{0}"), "control characters never reach an audit line");
    }
    let _ = State::Ready;
}


fn route(r: &Roles, team: &str, who: &str, tenant_only: bool, role_only: bool) -> super::approvals::Route {
    super::approvals::route(r, team, who, tenant_only, &["prod", "k3s.cluster"], role_only)
}

#[test]
fn tcfg_the_router_asks_the_approver_role_then_the_team_admins_then_the_tenant_admins() {
    let mut r = roles();
    r.add_tenant_member("olga", "zed", TenantRole::Member).unwrap();
    // only same-team admins, the requester excluded; another team's admin (vic, admin of data) is not eligible
    let a = route(&r, "web", "max", false, false);
    assert_eq!((a.approvers.clone(), a.via, a.escalated), (vec!["mia".to_string()], "team_admin", false));
    // the sole admin asks: escalated to the tenant admins, flagged
    let a = route(&r, "web", "mia", false, false);
    assert!(a.escalated && a.via == "escalation" && a.approvers == ["adam", "olga"] && !a.approvers.contains(&"vic".to_string()));
    // a second admin makes the team self-sufficient again
    r.add_namespace_member("mia", "web", "zed", NsRole::Admin).unwrap();
    assert_eq!(route(&r, "web", "mia", false, false).approvers, ["zed"]);
    // a holder of the approver role comes first; admins join them unless the binding says role only
    r.change_approvers("mia", "web", "prod", &["max".into()], &[], true).unwrap();
    let a = route(&r, "web", "vic", false, false);
    assert_eq!((a.via, a.approvers.clone()), ("role", vec!["max".to_string(), "mia".to_string(), "zed".to_string()]));
    assert_eq!(route(&r, "web", "vic", false, true).approvers, ["max"], "role only: the admins are not approvers");
    // a holder who asks cannot approve their own request, even as the only holder: the next step of the chain
    let a = route(&r, "web", "max", false, true);
    assert_eq!((a.via, a.approvers), ("team_admin", vec!["mia".to_string(), "zed".to_string()]));
    // the approver role of another team does not count here
    r.change_approvers("vic", "data", "prod", &["zed".into()], &[], true).unwrap();
    assert!(!route(&r, "web", "vic", false, true).approvers.contains(&"zed".to_string()) || route(&r, "web", "vic", false, true).approvers == ["max"]);
    assert_eq!(route(&r, "web", "vic", false, true).approvers, ["max"]);
    // the tenant's rule narrows to tenant admins
    let a = route(&r, "web", "vic", true, true);
    assert_eq!((a.via, a.approvers), ("tenant", vec!["adam".to_string(), "olga".to_string()]));
    // granting is audited with who, what and to whom; an admin of another team cannot grant
    let ch = r.change_approvers("mia", "web", "qa", &["max".into()], &[], true).unwrap();
    assert!(ch.0.contains("mia") && ch.0.contains("web/qa") && ch.0.contains("granted") && ch.0.contains("max"));
    assert!(r.change_approvers("vic", "web", "prod", &["vic".into()], &[], true).is_err());
    assert!(r.change_approvers("max", "web", "prod", &["max".into()], &[], true).is_err(), "an editor does not grant");
    let ch = r.change_approvers("adam", "web", "qa", &["max".into()], &[], false).unwrap();
    assert!(ch.0.contains("revoked"));
    assert!(!r.approvers["web"].contains_key("qa"));
}

#[test]
fn tcfg_groups_hold_people_expand_at_decision_time_and_stay_inside_their_team() {
    let mut r = roles();
    // person, group and mixed holders; duplicates collapse; the requester is excluded though in the group
    r.set_group("mia", "web", "leads", &["max".into(), "vic".into()]).unwrap();
    r.change_approvers("mia", "web", "prod", &["max".into()], &["leads".into()], true).unwrap();
    assert_eq!(r.approver_holders("web", &["prod"]).into_iter().collect::<Vec<_>>(), ["max", "vic"]);
    assert_eq!(route(&r, "web", "vic", false, true).approvers, ["max"]);
    // a tenant-wide group (tenant admin) is usable by a team; a team admin cannot define one
    assert!(r.set_group("mia", "*", "ops", &["max".into()]).is_err());
    r.set_group("adam", "*", "ops", &["max".into()]).unwrap();
    r.change_approvers("mia", "web", "qa", &[], &["ops".into()], true).unwrap();
    assert_eq!(r.approver_holders("web", &["qa"]).into_iter().collect::<Vec<_>>(), ["max"]);
    // another team's group cannot be named; groups do not nest; people must be in the tenant
    r.set_group("vic", "data", "dgroup", &["vic".into()]).unwrap();
    assert!(r.change_approvers("mia", "web", "prod", &[], &["dgroup".into()], true).is_err());
    assert!(r.set_group("mia", "web", "nested", &["leads".into()]).is_err());
    assert!(r.set_group("mia", "web", "x", &["nobody".into()]).is_err());
    assert!(r.set_group("vic", "web", "x", &["max".into()]).is_err(), "not vic's team");
    // a person leaving the group loses the right at once; the audit line says who was removed
    let ch = r.set_group("mia", "web", "leads", &["max".into()]).unwrap();
    assert!(ch.0.contains("removed [vic]") && ch.0.contains("mia"), "{}", ch.0);
    assert_eq!(route(&r, "web", "zz", false, true).approvers, ["max"]);
    // leaving the tenant too
    r.remove_tenant_member("olga", "max").unwrap();
    assert!(r.approver_holders("web", &["prod"]).is_empty());
    // limits
    let mut big = Roles::new("olga").unwrap();
    let names: Vec<String> = (0..201).map(|i| format!("u{i}")).collect();
    for n in &names {
        big.add_tenant_member("olga", n, TenantRole::Member).unwrap();
    }
    assert!(big.set_group("olga", "*", "all", &names).is_err());
    assert!(big.set_group("olga", "*", "all", &names[..200]).is_ok());
    for i in 0..49 {
        big.set_group("olga", "*", &format!("g{i}"), &[]).unwrap();
    }
    assert!(big.set_group("olga", "*", "one-too-many", &[]).is_err());
}

#[test]
fn tcfg_requests_reach_only_eligible_approvers_expire_and_are_audited() {
    let mut x = t("approvals");
    x.roles.set_group("mia", "web", "leads", &["max".into(), "vic".into()]).unwrap();
    x.roles.change_approvers("mia", "web", "prod", &[], &["leads".into()], true).unwrap();
    let now = 1_000_000u64;
    let p = approvals::request(&x.env(), &x.roles, "k3s.cluster", "web", "prod", "vic", false, true, now).unwrap();
    assert_eq!(approvals::request(&x.env(), &x.roles, "k3s.cluster", "web", "prod", "vic", false, true, now + 5).unwrap().id, p.id, "no duplicate");
    assert_eq!(seen(&x, "max", now), 1, "a holder sees it");
    assert_eq!(seen(&x, "vic", now), 0, "the requester does not, though in the group");
    assert_eq!(seen(&x, "mia", now), 0, "role only: the team admin is not an approver");
    assert_eq!(seen(&x, "adam", now), 0, "a tenant admin is not an approver by default");
    // the other team's admin and an outsider get nothing
    assert_eq!(status(&approvals::decide(&x.env(), &x.who("vic"), &p.id, true, now)), 403, "self");
    assert_eq!(status(&approvals::decide(&x.env(), &x.who("mia"), &p.id, true, now)), 404);
    // a holder removed mid-flight can no longer approve
    x.roles.set_group("mia", "web", "leads", &["vic".into()]).unwrap();
    assert_eq!(status(&approvals::decide(&x.env(), &x.who("max"), &p.id, true, now)), 404);
    x.roles.set_group("mia", "web", "leads", &["vic".into(), "max".into()]).unwrap();
    // expiry
    assert_eq!(seen(&x, "max", now + approvals::DEFAULT_TTL_SECS + 1), 0);
    assert_eq!(status(&approvals::decide(&x.env(), &x.who("max"), &p.id, true, now + approvals::DEFAULT_TTL_SECS + 1)), 409);
    // approval in time, used once
    assert_eq!(status(&approvals::decide(&x.env(), &x.who("max"), &p.id, true, now + 10)), 200);
    assert_eq!(status(&approvals::decide(&x.env(), &x.who("max"), &p.id, true, now + 11)), 409, "already decided");
    assert_eq!(approvals::take_approval(&x.env(), "k3s.cluster", "web", "prod", "vic", now + 12).as_deref(), Some("max"));
    assert_eq!(approvals::take_approval(&x.env(), "k3s.cluster", "web", "prod", "vic", now + 13), None);
    let a = x.rec.all();
    assert!(a.contains("tenant.approval.request") && a.contains("requester=vic") && a.contains("team=web") && a.contains("item=prod") && a.contains("via=role"), "{a}");
    assert!(a.contains("tenant.approval.approve") && a.contains("approver=max") && a.contains("escalated=false"), "{a}");
    assert!(a.contains("tenant.approval.refused") && a.contains("reason=self"));
    // a sole-admin escalation is flagged in the audit
    let q = approvals::request(&x.env(), &x.roles, "k3s.cluster", "data", "prod", "vic", false, false, now).unwrap();
    assert!(x.rec.all().contains(&format!("id={} kind=k3s.cluster team=data item=prod requester=vic approvers=adam,olga escalated=true", q.id)));
    assert_eq!(seen(&x, "adam", now), 1);
}

#[test]
fn tcfg_role_only_binds_the_deploy_decision_and_the_tenant_may_narrow_it() {
    let mut x = t("k3s-roleonly");
    x.roles.set_group("mia", "web", "leads", &["vic".into()]).unwrap();
    x.roles.change_approvers("mia", "web", "prod", &[], &["leads".into()], true).unwrap();
    for e in ["dev", "preprod", "prod"] {
        binding(&x, "web", e, json!({"deploy_role": "editor"})).unwrap();
    }
    let mut prod = ask(&x, "web", "prod", "max", 1);
    prod.promoted_from = Some("preprod");
    assert_eq!(enforce::k3s_decide(&x.env(), &prod), Deploy::NeedsApproval { approver_role: "admin".into() });
    // prod with a holder defaults to role only: the team admin cannot approve, the holder can
    prod.approval = Some("mia");
    assert!(matches!(enforce::k3s_decide(&x.env(), &prod), Deploy::Deny(w) if w.contains("approver")));
    prod.approval = Some("vic");
    assert_eq!(enforce::k3s_decide(&x.env(), &prod), Deploy::Allow);
    // a team may not widen: with the tenant requiring role_only, the team's role_or_admin changes nothing
    binding(&x, "web", "prod", json!({"deploy_role": "editor", "approver_mode": "role_or_admin"})).unwrap();
    prod.approval = Some("mia");
    assert_eq!(enforce::k3s_decide(&x.env(), &prod), Deploy::Allow, "the team chose role or admin");
    x.put("adam", "k3s.cluster", &["prod"], json!({"enabled": true, "values": {"approver_mode": "role_only"}})).unwrap();
    assert!(matches!(enforce::k3s_decide(&x.env(), &prod), Deploy::Deny(w) if w.contains("approver")), "the tenant's role only beats the team's choice");
}

fn seen(x: &T, u: &str, at: u64) -> usize {
    approvals::list(&x.env(), &x.who(u), at).map(|v| v["approvals"].as_array().unwrap().len()).unwrap_or(99)
}

// ---------------------------------------------------------------------------------------------------------------- team integrations, catalog, safety, local hosts

/// The warnings of an item as a PUT would see them, by validating it (dry run).
fn dry(x: &T, user: &str, kind: &str, body: Value) -> Value {
    api::validate(&x.env(), &x.who(user), kind, body).unwrap()
}

fn has(v: &Value, code: &str) -> bool {
    v["warnings"].as_array().unwrap().iter().any(|w| w["code"] == code)
}

fn sev(v: &Value, code: &str) -> String {
    v["warnings"].as_array().unwrap().iter().find(|w| w["code"] == code).map(|w| w["severity"].as_str().unwrap().to_string()).unwrap_or_default()
}

fn claude_key(team_extra: Value) -> Value {
    let mut b = json!({"enabled": true, "namespace": "web", "name": "default", "values": {"mode": "shared", "terms_ack": true, "allowed_models": ["claude-sonnet-4"], "daily_requests_per_person": 5, "monthly_requests_team": 200}, "secrets": {"token": "sk-ant-TEAMSECRET-999"}});
    for (k, v) in team_extra.as_object().unwrap() {
        b["values"][k] = v.clone();
    }
    b
}

#[test]
fn tcfg_a_team_admin_adds_a_claude_key_that_lives_only_in_the_vault_and_members_use_it_within_limits() {
    let x = t("team-claude");
    let url = ["web", "default"];
    // a member cannot add; the team admin can, and must acknowledge the warnings
    assert_eq!(status(&x.put("max", "agent.claude", &url, claude_key(json!({})))), 403);
    let no_ack = api::put(&x.env(), &x.who("mia"), "agent.claude", &["web".into(), "default".into()], claude_key(json!({})));
    assert_eq!(no_ack.as_ref().unwrap_err().code, "ack_required", "{no_ack:?}");
    let r = x.put("mia", "agent.claude", &url, claude_key(json!({}))).unwrap();
    assert_eq!(r["state"], "ready");
    assert!(x.vault.state(&SecretRef::parse("vault:team/web/agent-claude-token").unwrap()) == super::vault::SecretState::Set);
    // nowhere but the vault: files, versions, export, API output, audit
    let mut all = String::new();
    fn walk(p: &std::path::Path, out: &mut String) {
        for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
            if e.path().is_dir() { walk(&e.path(), out) } else { out.push_str(&std::fs::read_to_string(e.path()).unwrap_or_default()) }
        }
    }
    walk(&x.dir, &mut all);
    let outs = [all, x.get("mia", "agent.claude", &url).unwrap().to_string(), api::export(&x.env(), &x.who("adam")).unwrap().to_string(), api::list(&x.env(), &x.who("adam")).unwrap().to_string(), r.to_string(), x.rec.all()];
    for o in outs {
        assert!(!o.contains("TEAMSECRET"), "leaked: {o}");
    }
    // another team does not see it
    assert_eq!(status(&x.get("vic", "agent.claude", &url)), 404);
    // a member uses it within the per-person limit (5) and the team cap
    let gate = |u: &str| enforce::agent_gate(&x.env(), "agent.claude", "default", Some("web"), u, Some("claude-sonnet-4"), 5_000_000);
    for _ in 0..5 {
        assert_eq!(gate("max"), AgentUse::Allowed { shared: true });
    }
    assert!(matches!(gate("max"), AgentUse::Refused(w) if w.contains("limit")));
    assert_eq!(gate("mia"), AgentUse::Allowed { shared: true });
    assert!(x.rec.all().contains("team=web") && x.rec.all().contains("tenant.agent.use"));
    // rotation: a new key replaces the old one, no config edit needed and the audit says only that a secret was set
    let mut b = claude_key(json!({}));
    b["secrets"]["token"] = json!("sk-ant-ROTATED-888");
    x.put("mia", "agent.claude", &url, b).unwrap();
    let mut seen = String::new();
    enforce::with_agent_token(&x.env(), "agent.claude", "default", Some("web"), "max", "check", &mut |k| { seen = k.into(); Ok(String::new()) }).unwrap();
    assert_eq!(seen, "sk-ant-ROTATED-888");
    assert!(!x.rec.all().contains("ROTATED"));
    // a locked vault holds it back
    x.vault.set_locked(true);
    assert!(matches!(gate("max"), AgentUse::Refused(w) if w.contains("vault_locked")));
}

#[test]
fn tcfg_the_tenant_can_forbid_narrow_or_require_approval_for_team_integrations() {
    let x = t("team-policy");
    let body = |models: Value| json!({"enabled": true, "values": {"provider": "openai", "terms_ack": true, "allowed_models": models, "daily_requests_per_person": 5, "monthly_requests_team": 100}, "secrets": {"api_key": "sk-OPENAI-1234567890123456"}});
    let put = |u: &str, b: Value| x.put(u, "integration.apikey", &["web", "oa"], b);
    // forbid
    x.put("adam", "integration.policy", &[], json!({"enabled": true, "values": {"team_integrations": "deny"}})).unwrap();
    assert_eq!(status(&put("mia", body(json!(["gpt-4o"])))), 403);
    // allow only listed providers and models
    x.put("adam", "integration.policy", &[], json!({"enabled": true, "values": {"allowed_providers": ["anthropic"]}})).unwrap();
    assert_eq!(status(&put("mia", body(json!(["gpt-4o"])))), 403);
    x.put("adam", "integration.policy", &[], json!({"enabled": true, "values": {"allowed_providers": ["openai"], "allowed_models": ["gpt-4o*"]}})).unwrap();
    assert_eq!(status(&put("mia", body(json!(["o3"])))), 403, "a model outside the list");
    // a new provider waits for a tenant admin: the approval goes to a tenant admin, not the team admin
    x.put("adam", "integration.policy", &[], json!({"enabled": true, "values": {"allowed_providers": ["openai"], "allowed_models": ["gpt-4o*"], "new_provider_needs_approval": true}})).unwrap();
    let r = put("mia", body(json!(["gpt-4o"]))).unwrap();
    assert_eq!(r["ok"], false);
    let id = r["pending_approval"].as_str().unwrap().to_string();
    assert_eq!(seen(&x, "zed-nobody", 0), 99);
    assert_eq!(seen(&x, "adam", super::now_secs()), 1);
    assert_eq!(seen(&x, "mia", super::now_secs()), 0, "the team admin is not the approver here");
    assert_eq!(status(&approvals::decide(&x.env(), &x.who("adam"), &id, true, super::now_secs())), 200);
    assert_eq!(put("mia", body(json!(["gpt-4o"]))).unwrap()["ok"], true);
    // an operator-managed entry cannot be edited
    x.write_file(json!({"items": [{"kind": "integration.apikey", "namespace": "web", "name": "fixed", "enabled": false, "managed": true, "values": {"provider": "openai"}}]}));
    assert_eq!(status(&x.put("mia", "integration.apikey", &["web", "fixed"], body(json!(["gpt-4o"])))), 409);
}

#[test]
fn tcfg_the_catalog_knows_its_providers_and_checks_urls_keys_and_special_fields() {
    use super::catalog;
    for id in ["anthropic", "openai", "gemini", "openrouter", "azure_openai", "mistral", "openai_compatible"] {
        assert!(catalog::provider(id).is_some(), "{id}");
    }
    assert_eq!(catalog::provider("openrouter").unwrap().domains, ["openrouter.ai"]);
    for id in ["ollama", "lemonade", "lmstudio", "vllm", "llamacpp"] {
        assert!(catalog::local_server(id).is_some(), "{id}");
    }
    assert_eq!(catalog::local_server("lemonade").unwrap().port, 0, "no port is claimed where it is not known");
    // key shape is a warning, never a refusal
    assert!(catalog::key_warning("openai", "not-a-usual-key-xxxxxxxxxxxxxxxxx").is_some());
    assert!(catalog::key_warning("openai", "sk-aaaaaaaaaaaaaaaaaaaaaaaa").is_none());
    let x = t("catalog");
    let mk = |prov: &str, extra: Value| {
        let mut b = json!({"enabled": true, "values": {"provider": prov, "terms_ack": true, "allowed_models": ["m1"], "daily_requests_per_person": 5, "monthly_requests_team": 100}, "secrets": {"api_key": "weird-key-shape"}});
        for (k, v) in extra.as_object().unwrap() {
            b["values"][k] = v.clone();
        }
        b
    };
    let r = x.put("mia", "integration.apikey", &["web", "or1"], mk("openrouter", json!({"site_name": "Research", "referer": "https://research.example.com"}))).unwrap();
    assert!(r["key_warnings"].as_array().unwrap().len() == 1, "{r}");
    // two accounts of one provider in a team
    assert_eq!(status(&x.put("mia", "integration.apikey", &["web", "or2"], mk("openrouter", json!({})))), 200);
    assert_eq!(x.get("mia", "integration.apikey", &[]).unwrap()["items"].as_array().unwrap().len(), 2);
    // provider fields: azure needs a deployment and an endpoint, openrouter fields do not belong elsewhere
    assert_eq!(status(&x.put("mia", "integration.apikey", &["web", "az"], mk("azure_openai", json!({"base_url": "https://acme.openai.azure.com"})))), 400);
    assert_eq!(status(&x.put("mia", "integration.apikey", &["web", "az"], mk("azure_openai", json!({"base_url": "https://acme.openai.azure.com", "deployment": "gpt4"})))), 200);
    assert_eq!(status(&x.put("mia", "integration.apikey", &["web", "oa"], mk("openai", json!({"site_name": "x"})))), 400);
    assert_eq!(status(&x.put("mia", "integration.apikey", &["web", "oa"], mk("openai", json!({"base_url": "https://evil.example.com"})))), 400, "fixed base URL");
    // https is required; a private endpoint needs the tenant's say-so
    assert_eq!(status(&x.put("mia", "integration.apikey", &["web", "vl"], mk("openai_compatible", json!({"base_url": "http://gpu.internal:8000"})))), 400);
    x.put("adam", "integration.policy", &[], json!({"enabled": true, "values": {"allow_private_endpoints": true}})).unwrap();
    assert_eq!(status(&x.put("mia", "integration.apikey", &["web", "vl"], mk("openai_compatible", json!({"base_url": "http://gpu.internal:8000"})))), 200);
    assert_eq!(status(&x.put("mia", "integration.apikey", &["web", "pub"], mk("openai_compatible", json!({"base_url": "http://api.example.com"})))), 400, "plain http to a public host never");
}

#[test]
fn tcfg_a_provider_the_egress_policy_does_not_allow_is_disabled_by_policy_with_the_domain_named() {
    let x = t("egress-dom");
    x.put("adam", "egress.policy", &[], json!({"enabled": true, "values": {"allowed_domains": ["api.openai.com"]}})).unwrap();
    let b = json!({"enabled": true, "values": {"provider": "openrouter", "terms_ack": true, "allowed_models": ["m"], "daily_requests_per_person": 5, "monthly_requests_team": 100}, "secrets": {"api_key": "sk-or-aaaaaaaaaaaaaaaaaaaaaaaa"}});
    let r = api::put(&x.env(), &x.who("mia"), "integration.apikey", &["web".into(), "or".into()], { let mut b = b.clone(); b["ack_warnings"] = super::safety::rules_table(); b });
    let msg = r.unwrap_err().message;
    assert!(msg.contains("egress.domain.blocked") && msg.contains("openrouter.ai"), "{msg}");
    // allow it and it is ready; remove it again and the stored entry reads disabled with the same text
    x.put("adam", "egress.policy", &[], json!({"enabled": true, "values": {"allowed_domains": ["api.openai.com", "openrouter.ai"]}})).unwrap();
    assert_eq!(x.put("mia", "integration.apikey", &["web", "or"], b).unwrap()["state"], "ready");
    x.put("adam", "egress.policy", &[], json!({"enabled": true, "values": {"allowed_domains": ["api.openai.com"]}})).unwrap();
    let g = x.get("mia", "integration.apikey", &["web", "or"]).unwrap();
    assert_eq!(g["state"], "disabled");
    assert!(g["reasons"].to_string().contains("openrouter.ai"));
}

#[test]
fn tcfg_every_block_rule_refuses() {
    let x = t("blocks");
    let base = |prov: &str| json!({"enabled": true, "namespace": "web", "name": "k", "values": {"provider": prov, "terms_ack": true, "allowed_models": ["m"], "daily_requests_per_person": 5, "monthly_requests_team": 100}, "secrets": {"api_key": "sk-xxxxxxxxxxxxxxxxxxxxxxxx"}});
    let blocked = |b: Value, code: &str| {
        let v = dry(&x, "mia", "integration.apikey", b);
        assert_eq!(v["valid"], false, "{code}: {v}");
        assert_eq!(sev(&v, code), "block", "{code}: {v}");
        assert!(!v.to_string().contains("sk-xxxxxxxx"), "a text carries a secret");
    };
    // spend control on a paid provider
    let mut b = base("openai");
    b["values"].as_object_mut().unwrap().remove("monthly_requests_team");
    blocked(b, "spend.cap.missing");
    let mut b = base("anthropic");
    b["values"].as_object_mut().unwrap().remove("daily_requests_per_person");
    blocked(b, "spend.cap.missing");
    // self-hosted is exempt
    let mut b = base("openai_compatible");
    b["values"]["base_url"] = json!("https://llm.example.com");
    b["values"].as_object_mut().unwrap().remove("monthly_requests_team");
    assert!(!has(&dry(&x, "mia", "integration.apikey", b), "spend.cap.missing"));
    // terms
    let mut b = base("openai");
    b["values"].as_object_mut().unwrap().remove("terms_ack");
    assert_eq!(dry(&x, "mia", "integration.apikey", b)["valid"], false);
    // a key inline in a URL or in a non-secret field
    let mut b = base("openai_compatible");
    b["values"]["base_url"] = json!("https://llm.example.com/v1?api_key=abc");
    blocked(b, "secret.inline");
    let mut b = base("openai");
    b["values"]["site_name"] = json!("x");
    let mut b2 = base("openrouter");
    b2["values"]["site_name"] = json!("sk-or-v1-abcdefabcdefabcdefabcdef");
    blocked(b2, "secret.inline");
    let _ = b;
    // another team's secret reference
    let mut b = base("openai");
    b["values"]["api_key"] = json!("vault:team/data/oa-key");
    b.as_object_mut().unwrap().remove("secrets");
    let v = dry(&x, "mia", "integration.apikey", b);
    assert_eq!(v["valid"], false);
    // plain http to a public host and a private one without the policy
    let mut b = base("openai_compatible");
    b["values"]["base_url"] = json!("http://10.0.0.5:8000");
    blocked(b, "endpoint.private");
    // wildcard egress
    for d in ["*", "*.com"] {
        let v = dry(&x, "adam", "egress.policy", json!({"enabled": true, "values": {"allowed_domains": [d]}}));
        assert_eq!(sev(&v, "egress.wildcard"), "block", "{d}");
    }
    assert!(!has(&dry(&x, "adam", "egress.policy", json!({"enabled": true, "values": {"allowed_domains": ["*.example.com"]}})), "egress.wildcard"));
    // a shared agent without limits
    let v = dry(&x, "mia", "agent.claude", json!({"enabled": true, "namespace": "web", "values": {"mode": "shared", "terms_ack": true, "token": "vault:team/web/t", "daily_requests_per_person": 3, "allowed_models": ["m"]}}));
    assert_eq!(sev(&v, "agent.shared.limits"), "block");
    // prod without four eyes when the tenant requires it
    x.put("adam", "integration.policy", &[], json!({"enabled": true, "values": {"require_four_eyes_prod": true}})).unwrap();
    let v = dry(&x, "mia", "k3s.cluster", json!({"enabled": true, "namespace": "web", "name": "prod", "values": {"api_server": "https://p.example.com", "credential": "vault:k3s.cluster/web-prod-credential", "four_eyes": false}}));
    assert_eq!(sev(&v, "k3s.prod.four_eyes"), "block");
    // audit cannot be switched off while a secret is in use
    x.put("adam", "integration.policy", &[], json!({"enabled": true, "values": {"audit_enabled": false}})).unwrap();
    assert_eq!(sev(&dry(&x, "mia", "integration.apikey", base("openai")), "audit.disabled"), "block");
}

#[test]
fn tcfg_every_warn_rule_shows_and_must_be_acknowledged_and_the_ack_is_recorded() {
    let x = t("warns");
    let good = json!({"enabled": true, "namespace": "web", "name": "oa", "values": {"provider": "openai", "terms_ack": true, "allowed_models": ["*"], "daily_requests_per_person": 5000, "monthly_requests_team": 900000}, "secrets": {"api_key": "sk-aaaaaaaaaaaaaaaaaaaaaaaa"}});
    let v = dry(&x, "mia", "integration.apikey", good.clone());
    for c in ["data.leaves", "models.unrestricted", "cap.high", "test.never"] {
        assert_eq!(sev(&v, c), "warn", "{c}: {v}");
    }
    assert!(v["warnings"].to_string().contains("OpenAI") && !v.to_string().contains("sk-aaaa"));
    assert!(sev(&v, "info.key.scope") == "info");
    // ack is required, server side, for each warn
    let mut b = good.clone();
    b["ack_warnings"] = json!(["data.leaves"]);
    let err = api::put(&x.env(), &x.who("mia"), "integration.apikey", &["web".into(), "oa".into()], b).unwrap_err();
    assert_eq!(err.code, "ack_required");
    assert!(err.message.contains("models.unrestricted") && !err.message.contains("data.leaves"));
    // a disabled item needs none
    let mut b = good.clone();
    b["enabled"] = json!(false);
    assert_eq!(status(&api::put(&x.env(), &x.who("mia"), "integration.apikey", &["web".into(), "oa".into()], b)), 200);
    // all acknowledged: saved, and the audit line has the codes and the ack, no texts
    let mut b = good.clone();
    b["ack_warnings"] = super::safety::rules_table();
    api::put(&x.env(), &x.who("mia"), "integration.apikey", &["web".into(), "oa".into()], b).unwrap();
    let a = x.rec.all();
    assert!(a.contains("warnings=") && a.contains("acked=") && a.contains("data.leaves") && !a.contains("processed by"), "{a}");
    // a stale key warns at read time, without a PUT
    let g = x.get("mia", "integration.apikey", &["web", "oa"]).unwrap();
    assert!(!g["warnings"].to_string().contains("key.age"));
    super::store::Store::new(&x.dir).update_meta(&super::store::meta_key("integration.apikey", Some("web"), "oa"), |m| m.last_rotated = Some(super::now_secs() - 100 * 86_400));
    assert_eq!(sev(&json!({"warnings": x.get("mia", "integration.apikey", &["web", "oa"]).unwrap()["warnings"]}), "key.age"), "warn");
    super::store::Store::new(&x.dir).update_meta(&super::store::meta_key("integration.apikey", Some("web"), "oa"), |m| m.last_rotated = Some(super::now_secs() - 200 * 86_400));
    assert_eq!(sev(&json!({"warnings": x.get("mia", "integration.apikey", &["web", "oa"]).unwrap()["warnings"]}), "key.age.strong"), "warn");
    // thresholds are configurable by the tenant
    x.put("adam", "integration.policy", &[], json!({"enabled": true, "values": {"key_warn_days": 300, "key_strong_days": 400, "warn_monthly_cap": 5000000, "warn_daily_per_person": 9000}})).unwrap();
    let w = x.get("mia", "integration.apikey", &["web", "oa"]).unwrap()["warnings"].to_string();
    assert!(!w.contains("key.age") && !w.contains("cap.high"), "{w}");
    // the same key in two entries
    let mut b2 = good.clone();
    b2["name"] = json!("oa2");
    b2["values"]["api_key"] = json!("vault:team/web/integration-apikey-oa-api-key");
    b2.as_object_mut().unwrap().remove("secrets");
    b2["values"]["api_key"] = x.get("adam", "integration.apikey", &["web", "oa"]).unwrap()["effective"]["api_key"].clone();
    let _ = b2;
    // other kinds
    let v = dry(&x, "adam", "git.remote", json!({"enabled": true, "name": "gh", "values": {"url_prefixes": ["https://github.com/acme/"], "access": "push", "credential": "vault:git.remote/gh-credential"}}));
    assert_eq!(sev(&v, "git.push.unprotected"), "warn");
    let many: Vec<String> = (0..60).map(|i| format!("h{i}.example.com")).collect();
    assert_eq!(sev(&dry(&x, "adam", "egress.policy", json!({"enabled": true, "values": {"allowed_domains": many}})), "egress.many"), "warn");
    let v = dry(&x, "mia", "k3s.cluster", json!({"enabled": true, "namespace": "web", "name": "prod", "values": {"api_server": "https://p.example.com", "credential": "vault:k3s.cluster/web-prod-credential"}}));
    for c in ["k3s.registry.missing", "k3s.quota.missing", "approver.sole"] {
        assert_eq!(sev(&v, c), "warn", "{c}: {v}");
    }
}

#[test]
fn tcfg_an_item_from_the_operators_file_that_breaks_a_block_rule_loads_as_invalid() {
    let x = t("file-invalid");
    x.write_file(json!({"items": [{"kind": "integration.apikey", "namespace": "web", "name": "bad", "enabled": true, "managed": true, "values": {"provider": "openai", "terms_ack": true, "api_key": "vault:team/web/k", "allowed_models": ["m"]}}]}));
    x.vault.put(&SecretRef::parse("vault:team/web/k").unwrap(), "key").unwrap();
    let g = x.get("mia", "integration.apikey", &["web", "bad"]).unwrap();
    assert_eq!(g["state"], "invalid", "{g}");
    assert!(g["reasons"].to_string().contains("spend.cap.missing"));
    assert_eq!(g["severity"], "block");
}

fn local_body(extra: Value) -> Value {
    let mut b = json!({"enabled": true, "namespace": "web", "name": "gpu", "values": {"server": "ollama", "host": "10.0.5.7", "port": 11434, "api_style": "ollama", "allowed_models": ["llama3"], "max_concurrent_per_person": 1, "max_concurrent_team": 2, "queue_timeout_secs": 30, "terms_ack": true}});
    for (k, v) in extra.as_object().unwrap() {
        b["values"][k] = v.clone();
    }
    b
}

#[test]
fn tcfg_a_local_model_host_needs_listed_addresses_limits_and_warns_about_exposure() {
    let x = t("local");
    // the tenant lists no internal hosts: every address is refused
    let v = dry(&x, "mia", "models.local", local_body(json!({})));
    assert_eq!(sev(&v, "local.ssrf"), "block", "{v}");
    x.put("adam", "integration.policy", &[], json!({"enabled": true, "values": {"internal_hosts": ["10.0.5.0/24", "*.gpu.example.com"]}})).unwrap();
    let v = dry(&x, "mia", "models.local", local_body(json!({})));
    assert!(!has(&v, "local.ssrf") && v["valid"] == true, "{v}");
    for c in ["local.noauth", "local.http", "local.model_files", "test.never"] {
        assert_eq!(sev(&v, c), "warn", "{c}");
    }
    assert_eq!(sev(&v, "info.local.sleep"), "info");
    // SSRF refusals: metadata, the cluster API server, loopback, outside the list, denied ranges
    for (h, why) in [("169.254.169.254", "metadata"), ("127.0.0.1", "loopback"), ("localhost", "loopback"), ("10.9.9.9", "not on")] {
        let v = dry(&x, "mia", "models.local", local_body(json!({"host": h})));
        assert_eq!(sev(&v, "local.ssrf"), "block", "{h}");
        assert!(v["warnings"].to_string().contains(why) || h == "localhost" || h == "127.0.0.1", "{h}: {v}");
    }
    binding(&x, "web", "dev", json!({"api_server": "https://10.0.5.1:6443"})).unwrap();
    assert_eq!(sev(&dry(&x, "mia", "models.local", local_body(json!({"host": "10.0.5.1"}))), "local.ssrf"), "block");
    x.put("adam", "integration.policy", &[], json!({"enabled": true, "values": {"internal_hosts": ["10.0.5.0/24"], "deny_hosts": ["10.0.5.128/25"], "allow_same_host": true}})).unwrap();
    assert_eq!(sev(&dry(&x, "mia", "models.local", local_body(json!({"host": "10.0.5.200"}))), "local.ssrf"), "block");
    assert!(!has(&dry(&x, "mia", "models.local", local_body(json!({"host": "127.0.0.1"}))), "local.ssrf"), "same host allowed for development");
    // limits are mandatory
    let mut b = local_body(json!({}));
    b["values"].as_object_mut().unwrap().remove("max_concurrent_team");
    assert_eq!(sev(&dry(&x, "mia", "models.local", b), "local.limits.missing"), "block");
    // saved with acknowledgements; another team does not see it; a token makes the no-auth warning go
    let r = x.put("mia", "models.local", &["web", "gpu"], local_body(json!({}))).unwrap();
    assert_eq!(r["state"], "ready");
    assert_eq!(status(&x.get("vic", "models.local", &["web", "gpu"])), 404);
    let mut b = local_body(json!({}));
    b["secrets"] = json!({"token": "local-token"});
    assert!(!has(&dry(&x, "mia", "models.local", b), "local.noauth"));
}

#[test]
fn tcfg_the_fair_share_limiter_and_health_are_pure() {
    use super::fairshare::*;
    let l = Limits { per_person: 1, per_team: 2, queue_timeout_secs: 30 };
    let mut run = Running::default();
    assert_eq!(admit(&l, &run, "a", None, 100), Admit::Run);
    run.by_person.insert("a".into(), 1);
    assert_eq!(admit(&l, &run, "a", None, 100), Admit::Wait { until: 130 }, "a person at their limit waits");
    assert_eq!(admit(&l, &run, "b", None, 100), Admit::Run);
    run.by_person.insert("b".into(), 1);
    assert_eq!(admit(&l, &run, "c", Some(100), 110), Admit::Wait { until: 130 }, "the team is full");
    assert!(matches!(admit(&l, &run, "c", Some(100), 130), Admit::Reject(w) if w.contains("timed out")));
    // who goes next: the person with fewest running, then the longest waiting; skipped when timed out or at their own limit
    run.by_person.clear();
    run.by_person.insert("a".into(), 1);
    let q = vec![("a".to_string(), 1), ("b".to_string(), 50), ("c".to_string(), 40)];
    assert_eq!(next_to_run(&l, &run, &q, 60), Some(2), "c waited longer than b; a is at the limit");
    assert_eq!(next_to_run(&l, &run, &q, 75), Some(1), "c timed out");
    run.by_person.insert("b".into(), 1);
    assert_eq!(next_to_run(&l, &run, &q, 60), None, "the team is full");
    // health transitions from the last test with the fake tester
    assert_eq!(health_of(None, 0, 900), "unknown");
    assert_eq!(health_of(Some((100, true, 200)), 200, 900), "healthy");
    assert_eq!(health_of(Some((100, false, 401)), 200, 900), "auth_failed");
    assert_eq!(health_of(Some((100, false, 0)), 200, 900), "unreachable");
    assert_eq!(health_of(Some((100, false, 500)), 200, 900), "failing");
    assert_eq!(health_of(Some((100, true, 200)), 5000, 900), "unknown", "stale");
}

struct FakeTester(Mutex<Vec<(String, String)>>, Mutex<Vec<super::catalog::TestResult>>);
impl super::catalog::ConnectionTester for FakeTester {
    fn test(&self, r: &super::catalog::TestRequest) -> Result<super::catalog::TestResult, String> {
        self.0.lock().unwrap().push((r.url.to_string(), r.key.to_string()));
        Ok(self.1.lock().unwrap().remove(0))
    }
}

#[test]
fn tcfg_a_connection_test_is_rate_limited_and_stores_the_status_only() {
    use super::catalog::TestResult;
    let x = t("conn-test");
    x.put("adam", "integration.policy", &[], json!({"enabled": true, "values": {"internal_hosts": ["10.0.5.0/24"]}})).unwrap();
    x.put("mia", "models.local", &["web", "gpu"], { let mut b = local_body(json!({})); b["secrets"] = json!({"token": "local-token-xyz"}); b }).unwrap();
    let tester = Arc::new(FakeTester(Mutex::new(vec![]), Mutex::new(vec![TestResult { ok: false, status: 0 }, TestResult { ok: true, status: 200 }])));
    super::catalog::bind_tester(&x.dir, tester.clone());
    let run = |u: &str, at: u64| api::test(&x.env(), &x.who(u), "models.local", &["web".to_string(), "gpu".to_string()], at);
    assert_eq!(status(&run("max", 1000)), 403, "admins trigger it");
    let r = run("mia", 1000).unwrap();
    assert_eq!((r["ok"].clone(), r["status"].clone()), (json!(false), json!(0)));
    assert_eq!(status(&run("mia", 1030)), 429, "once a minute per entry");
    assert_eq!(super::fairshare::health_of(super::store::Store::new(&x.dir).meta().values().next().unwrap().test, 1040, 900), "unreachable");
    let r = run("mia", 1061).unwrap();
    assert_eq!(r["ok"], true);
    let calls = tester.0.lock().unwrap().clone();
    assert_eq!(calls[0].0, "http://10.0.5.7:11434/api/tags");
    assert_eq!(calls[0].1, "local-token-xyz", "the key reached only the tester");
    let g = x.get("mia", "models.local", &["web", "gpu"]).unwrap();
    assert_eq!(g["last_test"]["status"], 200);
    assert_eq!(g["health"], "unknown", "the test is old by the real clock");
    assert_eq!(super::fairshare::health_of(super::store::Store::new(&x.dir).meta().values().next().unwrap().test, 1100, 900), "healthy");
    let stored = std::fs::read_to_string(x.dir.join("cloud/config/meta.json")).unwrap();
    assert!(!stored.contains("local-token") && !x.rec.all().contains("local-token"));
    assert!(x.rec.all().contains("tenant.integration.test"));
    super::catalog::bind_tester(&x.dir, Arc::new(super::catalog::NoTester));
    assert_eq!(status(&run("mia", 5000)), 503, "the real caller is not bound");
}

// ---------------------------------------------------------------------------------------------------------------- hooks wired (runtime.rs, discovery.rs, vault binding)

#[test]
fn tcfg_hook_the_egress_policy_narrows_and_never_widens() {
    let x = t("hook-egress");
    let f = enforce::egress_narrowing(x.dir.clone(), None);
    assert_eq!(f("api.example.com"), None, "no policy: the caller keeps its own rule");
    x.put("adam", "egress.policy", &[], json!({"enabled": true, "values": {"mode": "allow_list", "allowed_domains": ["*.example.com"]}})).unwrap();
    assert_eq!(f("api.example.com"), Some(true));
    assert_eq!(f("evil.org"), Some(false));
    x.put("adam", "egress.policy", &[], json!({"enabled": false, "values": {"mode": "allow_list", "allowed_domains": ["*.example.com"]}})).unwrap();
    assert_eq!(f("api.example.com"), Some(false), "a policy switched off denies everything");
    std::fs::write(x.dir.join("cloud/config/tenant-config.local.json"), "{ not json").unwrap();
    assert_eq!(f("api.example.com"), Some(false), "an unreadable configuration denies");
}

#[test]
fn tcfg_hook_the_git_clone_guard_follows_the_tenants_remotes() {
    let x = t("hook-git");
    assert!(runtime::git_clone_guard(&x.env(), Some("web"), "max", "https://anywhere.example/x.git").is_ok(), "a tenant with no git.remote entry changes nothing");
    x.put("adam", "git.remote", &["gh"], json!({"enabled": true, "values": {"url_prefixes": ["https://github.com/acme/"]}})).unwrap();
    assert!(runtime::git_clone_guard(&x.env(), Some("web"), "max", "https://github.com/acme/app.git").is_ok());
    let e = runtime::git_clone_guard(&x.env(), Some("web"), "max", "https://github.com/other/app.git").unwrap_err();
    assert!(!e.contains("other/app"), "the refusal does not repeat the address: {e}");
    let a = x.rec.all();
    assert!(a.contains("tenant.git.clone") && a.contains("tenant.git.refused"));
    x.put("adam", "git.remote", &["gh"], json!({"enabled": false, "values": {"url_prefixes": ["https://github.com/acme/"]}})).unwrap();
    assert!(runtime::git_clone_guard(&x.env(), Some("web"), "max", "https://github.com/acme/app.git").is_err(), "an entry that is switched off allows nothing, and the tenant still has a policy");
}

fn apikey_body(extra: Value) -> Value {
    let mut b = json!({"enabled": true, "values": {"provider": "openai", "terms_ack": true, "allowed_models": ["gpt-4o*"], "daily_requests_per_person": 2, "monthly_requests_team": 50}, "secrets": {"api_key": "sk-test-0123456789abcdefghij"}});
    for (k, v) in extra.as_object().unwrap() {
        b["values"][k] = v.clone();
    }
    b
}

#[test]
fn tcfg_hook_a_team_provider_is_gated_by_model_role_and_counters() {
    let x = t("hook-provider");
    x.put("mia", "integration.apikey", &["web", "oai"], apikey_body(json!({}))).unwrap();
    let go = |m: &str, now: u64| runtime::provider_gate(&x.env(), "integration.apikey", Some("web"), "oai", "max", 1, m, now);
    assert_eq!(go("gpt-4o-mini", 1_000_000), AgentUse::Allowed { shared: true });
    assert!(matches!(go("o1-preview", 1_000_000), AgentUse::Refused(w) if w.contains("model")), "a model off the list is refused");
    assert_eq!(go("gpt-4o", 1_000_001), AgentUse::Allowed { shared: true });
    assert!(matches!(go("gpt-4o", 1_000_002), AgentUse::Refused(w) if w.contains("limit")), "the person's daily limit is 2");
    assert_eq!(go("gpt-4o", 1_000_000 + 86_400 + 5), AgentUse::Allowed { shared: true }, "the next day counts again");
    let other = runtime::provider_gate(&x.env(), "integration.apikey", Some("web"), "oai", "vic", 1, "gpt-4o", 1_000_000);
    assert_eq!(other, AgentUse::Allowed { shared: true }, "counters are per person");
    assert!(matches!(runtime::provider_gate(&x.env(), "integration.apikey", Some("web"), "nope", "max", 1, "gpt-4o", 1), AgentUse::Refused(w) if w.contains("not configured")));
    // a locked vault stops the use
    x.vault.set_locked(true);
    assert!(matches!(go("gpt-4o", 2_000_000), AgentUse::Refused(w) if w.contains("vault")));
    x.vault.set_locked(false);
    let a = x.rec.all();
    assert!(a.contains("tenant.provider.use") && a.contains("tenant.provider.refused") && !a.contains("sk-test-0123"), "{a}");
}

#[test]
fn tcfg_hook_a_models_provider_entry_uses_provider_models_for_its_allow_list() {
    let x = t("hook-modelsprov");
    x.put("adam", "models.provider", &["corp"], json!({"enabled": true, "values": {"api": "openai", "base_url": "https://llm.corp.example", "allowed_models": ["m1"], "terms_ack": true, "monthly_requests_team": 10}, "secrets": {"api_key": "key-for-corp-llm-123456"}})).unwrap();
    assert_eq!(enforce::provider_models(&x.env(), None, "corp"), Some(vec!["m1".to_string()]));
    assert_eq!(runtime::provider_gate(&x.env(), "models.provider", None, "corp", "max", 0, "m1", 5), AgentUse::Allowed { shared: true });
    assert!(matches!(runtime::provider_gate(&x.env(), "models.provider", None, "corp", "max", 0, "m2", 5), AgentUse::Refused(_)));
}

#[test]
fn tcfg_hook_the_local_model_call_waits_in_the_fair_share_queue_and_times_out() {
    let x = t("hook-local");
    x.put("adam", "integration.policy", &[], json!({"enabled": true, "values": {"internal_hosts": ["10.0.5.0/24"]}})).unwrap();
    let mut b = local_body(json!({"max_concurrent_per_person": 1, "max_concurrent_team": 1, "queue_timeout_secs": 2}));
    b["secrets"] = json!({"token": "local-token-xyz"});
    x.put("mia", "models.local", &["web", "gpu"], b).unwrap();
    let tick = std::sync::atomic::AtomicU64::new(100);
    let clock = || tick.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let mut seen = Vec::new();
    // the first call gets its slot, the token and the address; while it runs, a second person's call queues and times out
    let r = runtime::call_local(&x.env(), "web", "gpu", "max", 1, "llama3", 500, &clock, &mut |base, style, tok| {
        seen.push((base.to_string(), style.to_string(), tok.map(String::from)));
        let inner = runtime::call_local(&x.env(), "web", "gpu", "vic", 1, "llama3", 501, &clock, &mut |_, _, _| Ok("must not run".into()));
        assert_eq!(inner, Err("the queue timed out".to_string()), "the team limit is 1 and the queue timeout 2 s");
        Ok("answer".into())
    });
    assert_eq!(r, Ok("answer".to_string()));
    assert_eq!(seen, vec![("http://10.0.5.7:11434".to_string(), "ollama".to_string(), Some("local-token-xyz".to_string()))]);
    // the slot was given back
    assert_eq!(runtime::call_local(&x.env(), "web", "gpu", "vic", 1, "llama3", 502, &clock, &mut |_, _, _| Ok("ok".into())), Ok("ok".to_string()));
    // gate refusals come before any slot
    assert!(runtime::call_local(&x.env(), "web", "gpu", "vic", 1, "other-model", 503, &clock, &mut |_, _, _| Ok("x".into())).is_err());
    assert!(x.vault.uses.lock().unwrap().iter().all(|(a, _)| a == "local-model-call"));
    assert!(!x.rec.all().contains("local-token-xyz"));
}

#[test]
fn tcfg_hook_the_deploy_gate_files_and_consumes_an_approval_once() {
    let x = t("hook-deploy");
    for e in ["dev", "preprod", "prod"] {
        binding(&x, "web", e, json!({"deploy_role": if e == "dev" { "editor" } else { "admin" }})).unwrap();
    }
    let now = super::now_secs();
    let mut prod = ask(&x, "web", "prod", "mia", 2);
    prod.promoted_from = Some("preprod");
    let first = runtime::deploy_gate(&x.env(), &prod, now);
    let DeployStep::Pending { id, approver_role } = first else { panic!("{first:?}") };
    assert_eq!(approver_role, "tenant_admin");
    assert!(matches!(runtime::deploy_gate(&x.env(), &prod, now), DeployStep::Pending { id: id2, .. } if id2 == id), "asking again does not file a second request");
    // the requester cannot approve, a tenant admin can, by their own call
    assert!(approvals::decide(&x.env(), &x.who("mia"), &id, true, now).is_err());
    approvals::decide(&x.env(), &x.who("adam"), &id, true, now).unwrap();
    assert_eq!(runtime::deploy_gate(&x.env(), &prod, now), DeployStep::Go { approved_by: Some("adam".into()) });
    assert!(matches!(runtime::deploy_gate(&x.env(), &prod, now), DeployStep::Pending { .. }), "an approval is used once");
    // a plain denial passes through
    assert!(matches!(runtime::deploy_gate(&x.env(), &ask(&x, "web", "dev", "eve", -1), now), DeployStep::Denied(_)));
    assert_eq!(runtime::deploy_gate(&x.env(), &ask(&x, "web", "dev", "max", 1), now), DeployStep::Go { approved_by: None });
}

#[test]
fn tcfg_discovery_parses_ollama_and_openai_model_lists_and_rejects_the_rest() {
    let ollama = r#"{"models":[{"name":"llama3:8b","model":"llama3:8b","size":4661224676},{"name":"qwen2.5-coder:7b"},{"name":"llama3:8b"},{"name":"bad name"},{"model":"only-model-field"}]}"#;
    assert_eq!(discovery::parse_ollama_tags(ollama).unwrap(), vec!["llama3:8b", "only-model-field", "qwen2.5-coder:7b"]);
    let openai = r#"{"object":"list","data":[{"id":"gpt-4o","object":"model"},{"id":"gpt-4o-mini"},{"id":"x\u0007y"},{"id":""},{"nope":1}]}"#;
    assert_eq!(discovery::parse_openai_models(openai).unwrap(), vec!["gpt-4o", "gpt-4o-mini"]);
    assert_eq!(discovery::parse_models("ollama", ollama).unwrap().len(), 3);
    assert_eq!(discovery::parse_models("openai_compatible", openai).unwrap().len(), 2);
    assert_eq!(discovery::parse_models("gemini", r#"{"models":[{"name":"models/gemini-2.5-pro"}]}"#).unwrap(), vec!["gemini-2.5-pro"]);
    for bad in ["", "not json", "[]", r#"{"models":"x"}"#, r#"{"data":{}}"#] {
        assert!(discovery::parse_models("ollama", bad).is_err() || discovery::parse_models("ollama", bad).unwrap().is_empty(), "{bad}");
        assert!(discovery::parse_openai_models(bad).is_err(), "{bad}");
    }
    let many = format!("{{\"data\":[{}]}}", (0..900).map(|i| format!("{{\"id\":\"m{i}\"}}")).collect::<Vec<_>>().join(","));
    assert_eq!(discovery::parse_openai_models(&many).unwrap().len(), discovery::MAX_MODELS);
    let long = format!("{{\"data\":[{{\"id\":\"{}\"}}]}}", "a".repeat(200));
    assert!(discovery::parse_openai_models(&long).unwrap().is_empty());
}

struct DiscTester;
impl super::catalog::ConnectionTester for DiscTester {
    fn test(&self, _: &super::catalog::TestRequest) -> Result<super::catalog::TestResult, String> {
        Ok(super::catalog::TestResult { ok: true, status: 200 })
    }
    fn list_models(&self, r: &super::catalog::TestRequest, style: &str) -> Result<Vec<String>, String> {
        assert_eq!(r.url, "http://10.0.5.7:11434/api/tags");
        discovery::parse_models(style, r#"{"models":[{"name":"llama3:8b"},{"name":"phi3"}]}"#)
    }
}

#[test]
fn tcfg_discovery_goes_through_the_tester_for_admins_only() {
    let x = t("hook-discover");
    x.put("adam", "integration.policy", &[], json!({"enabled": true, "values": {"internal_hosts": ["10.0.5.0/24"]}})).unwrap();
    x.put("mia", "models.local", &["web", "gpu"], local_body(json!({}))).unwrap();
    super::catalog::bind_tester(&x.dir, Arc::new(DiscTester));
    let p = vec!["web".to_string(), "gpu".to_string()];
    assert_eq!(status(&api::discover(&x.env(), &x.who("max"), "models.local", &p, 1)), 403);
    let r = api::discover(&x.env(), &x.who("mia"), "models.local", &p, 1).unwrap();
    assert_eq!(r["models"], json!(["llama3:8b", "phi3"]));
    assert!(x.rec.all().contains("tenant.integration.discover"));
    super::catalog::bind_tester(&x.dir, Arc::new(super::catalog::NoTester));
    assert_eq!(status(&api::discover(&x.env(), &x.who("mia"), "models.local", &p, 1)), 503);
}

// the real vault binding, over a fake service

#[derive(Default)]
struct FakeOps {
    scopes: Mutex<std::collections::BTreeMap<String, std::collections::BTreeMap<String, Vec<u8>>>>,
    down: std::sync::atomic::AtomicBool,
}
impl vault::VaultOps for Arc<FakeOps> {
    fn list(&self, s: &crate::cloudvaultd::Scope) -> Result<Vec<String>, String> {
        if self.down.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("401: refused".into());
        }
        self.scopes.lock().unwrap().get(&s.name()).map(|m| m.keys().cloned().collect()).ok_or_else(|| "404: no such scope".into())
    }
    fn put(&self, s: &crate::cloudvaultd::Scope, n: &str, v: &[u8]) -> Result<(), String> {
        if self.down.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("401: refused".into());
        }
        self.scopes.lock().unwrap().get_mut(&s.name()).ok_or_else(|| "404: no such scope".to_string())?.insert(n.into(), v.to_vec());
        Ok(())
    }
    fn get(&self, s: &crate::cloudvaultd::Scope, n: &str) -> Result<Vec<u8>, String> {
        self.scopes.lock().unwrap().get(&s.name()).and_then(|m| m.get(n).cloned()).ok_or_else(|| "404: not found".into())
    }
    fn delete(&self, s: &crate::cloudvaultd::Scope, n: &str) -> Result<(), String> {
        self.scopes.lock().unwrap().get_mut(&s.name()).and_then(|m| m.remove(n)).map(|_| ()).ok_or_else(|| "404: not found".into())
    }
    fn create_scope(&self, s: &crate::cloudvaultd::Scope) -> Result<(), String> {
        self.scopes.lock().unwrap().entry(s.name()).or_default();
        Ok(())
    }
}

#[test]
fn tcfg_the_real_vault_binding_maps_references_and_reads_locked_without_access() {
    use crate::cloudvaultd::Scope;
    let team = SecretRef::parse("vault:team/web/openai-key").unwrap();
    assert_eq!(vault::scope_of(&team).unwrap(), (Scope::Namespace("web".into()), "openai-key".to_string()));
    let tenant = SecretRef::parse("vault:agent.claude/token").unwrap();
    assert_eq!(vault::scope_of(&tenant).unwrap(), (Scope::Tenant, "agent.claude.token".to_string()));
    let ops = Arc::new(FakeOps::default());
    let v = vault::LinkVault { ops: ops.clone() };
    assert_eq!(v.state(&tenant), SecretState::Missing, "no such scope yet: missing, not locked");
    v.put(&tenant, "sk-ant-real").unwrap();
    v.put(&team, "team-key").unwrap();
    assert_eq!(v.state(&tenant), SecretState::Set);
    assert_eq!(v.state(&team), SecretState::Set);
    let mut got = String::new();
    v.use_secret(&tenant, "agent-run", &mut |s| { got = s.to_string(); Ok(String::new()) }).unwrap();
    assert_eq!(got, "sk-ant-real");
    assert!(ops.scopes.lock().unwrap().contains_key("namespace:web"));
    v.remove(&tenant).unwrap();
    assert_eq!(v.state(&tenant), SecretState::Missing);
    ops.down.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(v.state(&team), SecretState::Locked, "a service that refuses reads as locked");
    assert_eq!(v.put(&team, "x").unwrap_err(), "the tenant vault is locked");
    // no link, or no key for the person: locked
    let d = dir("vault-for-user");
    assert_eq!(vault::for_user_with(&d, "mia", &crate::cloudvaultlink::Link::NotConfigured).state(&tenant), SecretState::Locked);
    let link = crate::cloudvaultlink::Link::from_values(Some("https://vault.example.org"), None, Some("acme"));
    assert_eq!(vault::for_user_with(&d, "mia", &link).state(&tenant), SecretState::Locked, "no key on this volume for mia");
    // a bound store wins
    let mem = MemVault::new();
    vault::bind(&d, mem.clone());
    mem.put(&tenant, "v").unwrap();
    assert_eq!(vault::for_user_with(&d, "mia", &crate::cloudvaultlink::Link::NotConfigured).state(&tenant), SecretState::Set);
    vault::unbind(&d);
}

#[test]
fn tcfg_the_file_is_read_fresh_on_every_request_and_stays_consistent_under_load() {
    let x = t("load");
    x.write_file(json!({"schema": 1, "items": [{"kind": "media.policy", "enabled": true, "managed": true, "values": {"media": "allow"}}]}));
    x.put("adam", "egress.policy", &[], json!({"enabled": true, "values": {"allowed_domains": ["a.example.com"]}})).unwrap();
    let dir = x.dir.clone();
    // many readers while a writer keeps committing: every read is a whole document (atomic writes), never an error
    let readers: Vec<_> = (0..8)
        .map(|_| {
            let d = dir.clone();
            std::thread::spawn(move || {
                let s = super::store::Store::new(&d);
                for _ in 0..200 {
                    let c = s.load().expect("a read during a write is whole");
                    assert!(c.file_error.is_none());
                    assert!(!c.local.is_empty());
                }
            })
        })
        .collect();
    for i in 0..20 {
        x.put("adam", "egress.policy", &[], json!({"enabled": true, "values": {"allowed_domains": [format!("h{i}.example.com")]}})).unwrap();
    }
    for r in readers {
        r.join().unwrap();
    }
    // no cache: an edit of the operator's file is seen by the very next request
    assert_eq!(x.get("adam", "media.policy", &[]).unwrap()["managed"], true);
    x.write_file(json!({"schema": 1, "items": []}));
    assert_eq!(x.get("adam", "media.policy", &[]).map(|v| v["managed"].clone()).unwrap_or(json!(false)), json!(false));
}
