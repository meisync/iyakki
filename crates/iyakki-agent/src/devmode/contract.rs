//! The core contract (docs/development/CORE-CONTRACT.md) carried over developer mode's TLS channel: the first remote transport of the
//! contract, **off by default** and opt-in per device and per computer. Nothing else changes in developer mode; this is a handful of
//! routes under `/v1/dev/core/` that the listener hands to [`handle`] after it has authenticated the signed request.
//!
//! * Switched on by an administrator on the device ([`DevMode::set_core_enabled`]); while off every route answers 403.
//! * A paired computer starts with NO contract scopes ([`DevMode::core_scopes`]); an administrator grants them one by one on the
//!   device ([`DevMode::set_core_scope`]). The context of every call is built here from the paired key and the transport
//!   `dev_mode`; a request cannot name its own scopes or pretend to be local.
//! * `POST hello` negotiates the version; `POST call` is `(domain, call, args) -> {ok, result} | {ok:false, error}` with the contract's
//!   own errors; `watch`/`events` take `resume` and may wait (`wait_ms`, at most 10 s) for news; `POST stream/read` reads a large
//!   payload in chunks by offset, `stream/close` frees it.
//! * What only a person at the machine may do stays there: `vault.unlock` and `consent.answer` answer `needs_consent` (and an unlock
//!   attempt raises a question on the machine's screen, so the person sees "waiting for you"); there is no remote path to them.
//! * Every call goes to the developer-mode activity log and, through the adapters, to the system audit log: who, over what, the call,
//!   the outcome; keys and actions, never values, passwords or secrets. A computer is rate limited by the machine's `hello` limit.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use iyakki_core_api::contract::catalog::{self, hello_for};
use iyakki_core_api::contract::*;
use iyakki_core_api::Capabilities;
use iyakki_sec::policy::Policy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::api::Out;
use super::{DevKey, DevMode};
use crate::contract_local::Core;

/// The routes of this module (the listener calls [`handle`] for exactly these).
pub const ROUTES: &[(&str, &str)] = &[("POST", "/v1/dev/core/hello"), ("POST", "/v1/dev/core/call"), ("POST", "/v1/dev/core/stream/read"), ("POST", "/v1/dev/core/stream/close")];

pub fn is_route(method: &str, path: &str) -> bool {
    ROUTES.iter().any(|(m, p)| *m == method && *p == path)
}

/// The contract scopes an administrator can grant a computer (never `local`: no scope unlocks a local-only call).
pub const GRANTABLE: [&str; 9] = [
    Scope::SETTINGS_READ,
    Scope::SETTINGS_WRITE,
    Scope::VAULT_STATUS,
    Scope::VAULT_LOCK,
    Scope::VAULT_USE,
    Scope::FLOWS_READ,
    Scope::FLOWS_RUN,
    Scope::FLOWS_APPROVE,
    Scope::CONSENT_READ,
];

/// The longest a `watch`/`events` call waits for news.
pub const MAX_WAIT_MS: u64 = 10_000;

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
struct CoreState {
    enabled: bool,
    /// computer name -> contract scopes it holds
    grants: BTreeMap<String, Vec<String>>,
}

impl DevMode {
    fn core_state(&self) -> CoreState {
        self.read("core.json")
    }

    /// Is the contract transport switched on (off by default)?
    pub fn core_enabled(&self) -> bool {
        self.core_state().enabled
    }

    /// Switch the contract transport on or off (administrators, on the device).
    pub fn set_core_enabled(&self, policy: &Policy, user: &str, on: bool) -> Result<(), String> {
        Self::need_admin(policy)?;
        let _one_at_a_time = super::keys_lock();
        let mut st = self.core_state();
        st.enabled = on;
        self.write("core.json", &st)?;
        self.log(user, user, if on { "devmode.core.on" } else { "devmode.core.off" }, "the typed core contract over developer mode", true);
        Ok(())
    }

    /// The contract scopes a paired computer holds (none until an administrator grants them).
    pub fn core_scopes(&self, computer: &str) -> Vec<String> {
        self.core_state().grants.get(computer).cloned().unwrap_or_default()
    }

    /// Grant or take away one contract scope of a paired computer (administrators, on the device). Scopes only grow by this hand.
    pub fn set_core_scope(&self, policy: &Policy, user: &str, computer: &str, scope: &str, on: bool) -> Result<Vec<String>, String> {
        Self::need_admin(policy)?;
        if !GRANTABLE.contains(&scope) {
            return Err(format!("`{scope}` is not a scope of the core contract that can be granted"));
        }
        let _one_at_a_time = super::keys_lock();
        if !self.keys().iter().any(|k| k.name == computer) {
            return Err(format!("no paired computer called {computer}"));
        }
        let mut st = self.core_state();
        let list = st.grants.entry(computer.to_string()).or_default();
        list.retain(|s| s != scope);
        if on {
            list.push(scope.to_string());
        }
        list.sort();
        let now = list.clone();
        if now.is_empty() {
            st.grants.remove(computer);
        }
        self.write("core.json", &st)?;
        self.log(user, user, if on { "devmode.core.grant" } else { "devmode.core.revoke" }, &format!("{computer} {scope} -> now: {}", if now.is_empty() { "nothing".to_string() } else { now.join(", ") }), true);
        Ok(now)
    }
}

/// Per-computer call limiter (the limit the machine announces in `hello`).
pub struct Gate {
    limit: RateLimit,
    by_computer: Mutex<HashMap<String, RateLimiter>>,
    started: Instant,
}

impl Gate {
    pub fn new(limit: RateLimit) -> Gate {
        Gate { limit, by_computer: Mutex::new(HashMap::new()), started: Instant::now() }
    }
    /// The limit this build announces.
    pub fn standard() -> Gate {
        Gate::new(hello_for("", "", Capabilities::none()).rate_limit)
    }
    fn check(&self, computer: &str) -> CallResult<()> {
        let now = self.started.elapsed().as_millis() as u64;
        self.by_computer.lock().unwrap_or_else(|e| e.into_inner()).entry(computer.to_string()).or_insert_with(|| RateLimiter::new(self.limit)).check(now)
    }
}

/// Everything a contract request is served with.
pub struct CoreCx<'a> {
    pub dm: &'a DevMode,
    pub key: &'a DevKey,
    pub policy: &'a Policy,
    pub core: &'a Core,
    pub gate: &'a Gate,
    pub hostname: &'a str,
    /// Write to the system audit log: (action, detail).
    pub audit: &'a dyn Fn(&str, &str),
}

fn ok(v: Value) -> Out {
    Out::json(200, &json!({ "ok": true, "result": v }))
}

fn failed(e: &CallError) -> Out {
    Out::json(200, &json!({ "ok": false, "error": e }))
}

fn bad(msg: &str) -> CallError {
    CallError::new(ErrorCode::InvalidInput, "core.bad_request", msg)
}

fn arg<T: serde::de::DeserializeOwned>(a: &Value, name: &str) -> CallResult<T> {
    serde_json::from_value(a.get(name).cloned().unwrap_or(Value::Null)).map_err(|_| bad(&format!("`{name}` is missing or has the wrong shape")))
}

fn opt<T: serde::de::DeserializeOwned>(a: &Value, name: &str) -> CallResult<Option<T>> {
    match a.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => serde_json::from_value(v.clone()).map(Some).map_err(|_| bad(&format!("`{name}` has the wrong shape"))),
    }
}

fn idem(a: &Value) -> CallResult<Option<IdempotencyKey>> {
    match opt::<String>(a, "idempotency_key")? {
        None => Ok(None),
        Some(k) => IdempotencyKey::new(&k).map(Some),
    }
}

fn to<T: Serialize>(v: T) -> CallResult<Value> {
    serde_json::to_value(v).map_err(|e| CallError::new(ErrorCode::Internal, "core.internal", &e.to_string()))
}

/// The context of a call: the paired computer, over `dev_mode`, with the contract scopes an administrator granted it.
fn context(cx: &CoreCx) -> CallContext {
    let scopes = cx.dm.core_scopes(&cx.key.name);
    let refs: Vec<&str> = scopes.iter().map(String::as_str).collect();
    CallContext::remote(&cx.key.name, Transport::DevMode, &refs)
}

/// Serve a route of this module. `None`: not one of mine.
pub fn handle(cx: &CoreCx, method: &str, path: &str, body: &[u8]) -> Option<Out> {
    if !is_route(method, path) {
        return None;
    }
    let action = format!("dev.{}", path.trim_start_matches("/v1/dev/").replace('/', "."));
    if !cx.dm.core_enabled() {
        cx.dm.log(&cx.key.name, &cx.key.account, &action, "refused: the core contract is switched off", false);
        return Some(Out::err(403, "the core contract is switched off on this device: an administrator switches it on in Settings, Developer mode"));
    }
    let Ok(req) = serde_json::from_slice::<Value>(body) else { return Some(Out::err(400, "the body must be JSON")) };
    // one account's vault for every call of this computer: the account it is paired to
    let dm_dir = cx.dm.data_dir().to_path_buf();
    cx.core.vault.set_account_resolver(std::sync::Arc::new(move |c: &CallContext| {
        DevMode::open(&dm_dir).keys().into_iter().find(|k| k.name == c.identity.user).map(|k| k.account).unwrap_or_default()
    }));
    let ctx = context(cx);
    if let Err(e) = cx.gate.check(&cx.key.name) {
        cx.dm.log(&cx.key.name, &cx.key.account, &action, "refused: rate limited", false);
        return Some(failed(&e));
    }
    let r: CallResult<Value> = match path {
        "/v1/dev/core/hello" => hello(cx, &req),
        "/v1/dev/core/call" => call(cx, &ctx, &req),
        "/v1/dev/core/stream/read" => stream_read(cx, &ctx, &req),
        _ => stream_close(cx, &ctx, &req),
    };
    Some(match r {
        Ok(v) => ok(v),
        Err(e) => failed(&e),
    })
}

fn hello(cx: &CoreCx, req: &Value) -> CallResult<Value> {
    let caps = Capabilities { display: cx.core.svc.screen.present(), ..Capabilities::none() };
    let h = hello_for(cx.hostname, env!("CARGO_PKG_VERSION"), caps);
    let client: ClientHello = serde_json::from_value(req.clone()).map_err(|_| bad("send {\"contract\":{\"major\":0,\"minor\":1},\"wants\":[\"settings\"]}"))?;
    let n = negotiate(&h, &client);
    cx.dm.log(&cx.key.name, &cx.key.account, "dev.core.hello", &format!("client {}.{}", client.contract.major, client.contract.minor), n.version.is_some());
    to(json!({
        "hello": h,
        "negotiated": { "version": n.version, "usable": n.usable, "missing": n.missing },
        // what THIS computer may do with it: the scopes an administrator granted on the device
        "scopes": cx.dm.core_scopes(&cx.key.name),
    }))
}

fn call(cx: &CoreCx, ctx: &CallContext, req: &Value) -> CallResult<Value> {
    let domain: String = arg(req, "domain")?;
    let name: String = arg(req, "call")?;
    let args = req.get("args").cloned().unwrap_or(json!({}));
    // a client that says which contract it speaks is refused across a major, or when older than the machine still serves
    if let Some(v) = opt::<ContractVersion>(req, "contract")? {
        let h = hello_for(cx.hostname, "", Capabilities::none());
        if negotiate(&h, &ClientHello { contract: v, wants: vec![] }).version.is_none() {
            return Err(CallError::new(ErrorCode::VersionMismatch, "core.version_mismatch", &format!("This machine speaks contract {}.{} (oldest served minor {}).", h.contract.major, h.contract.minor, h.oldest_minor)).hint("Update the client or the machine."));
        }
    }
    let spec = catalog::call(&domain, &name).ok_or_else(|| CallError::new(ErrorCode::Unsupported, "core.unsupported", &format!("This machine has no call {domain}.{name}.")).hint("Ask hello what it supports."))?;
    let wait = opt::<u64>(&args, "wait_ms")?.unwrap_or(0).min(MAX_WAIT_MS);
    let deadline = Instant::now() + Duration::from_millis(wait);
    let result = loop {
        let r = dispatch(cx, ctx, &domain, &name, &args);
        // a watcher waits for news: an empty batch is asked again until something arrives or the time is up
        let empty = matches!(&r, Ok(v) if v.get("events").and_then(|e| e.as_array()).is_some_and(|e| e.is_empty()));
        if wait > 0 && empty && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(40));
            continue;
        }
        break r;
    };
    let outcome = match &result {
        Ok(_) => "ok".to_string(),
        Err(e) => format!("refused: {}", e.code.as_str()),
    };
    cx.dm.log(&cx.key.name, &cx.key.account, "dev.core", &format!("{domain}.{name} [{}] {outcome}", spec.risk.id()), result.is_ok());
    if spec.risk != Risk::Read {
        (cx.audit)("devmode.core", &format!("{domain}.{name} [{}] {outcome}", spec.risk.id()));
    }
    result
}

/// Where a request that only a person at the machine may make ends: `needs_consent`, never an answer.
fn needs_person(what: &str) -> CallError {
    CallError::new(ErrorCode::NeedsConsent, "core.needs_local_person", &format!("{what} can only be done by a person at the machine.")).hint("Ask the person at the machine; the question is waiting on its screen.")
}

fn dispatch(cx: &CoreCx, ctx: &CallContext, domain: &str, name: &str, a: &Value) -> CallResult<Value> {
    let core = cx.core;
    match (domain, name) {
        ("settings", "schema") => to(core.settings.schema(ctx)?),
        ("settings", "get") => to(core.settings.get(ctx, &arg::<String>(a, "key")?)?),
        ("settings", "set") => to(core.settings.set(ctx, &arg::<String>(a, "key")?, arg(a, "value")?, idem(a)?.as_ref())?),
        ("settings", "watch") => to(core.settings.watch(ctx, opt::<String>(a, "key")?.as_deref(), opt(a, "resume")?.unwrap_or(Resume::Latest), opt(a, "limit")?.unwrap_or(100))?),
        ("vault", "status") => to(core.vault.status(ctx)?),
        ("vault", "lock") => to(core.vault.lock(ctx)?),
        ("vault", "use_secret") => to(core.vault.use_secret(ctx, &NamedAction(arg(a, "action")?), idem(a)?.as_ref())?),
        ("vault", "unlock") => {
            // the machine's screen asks its person; the remote side learns only that someone must
            core.consents.ask_once(ConsentKind::VaultUnlock, &ctx.identity.user, "consent.vault_unlock", &format!("{} asks to unlock the vault. Unlock it here if you agree.", ctx.identity.user));
            Err(needs_person("Unlocking the vault"))
        }
        ("consent", "pending") => to(core.consents.pending(ctx)?),
        ("consent", "answer") => Err(needs_person("Answering this question")),
        ("flows", _) => {
            let flows = core.flows_for(&cx.key.account, cx.policy)?;
            match name {
                "list" => to(flows.list(ctx)?),
                "status" => to(flows.status(ctx, &RunId(arg(a, "run")?))?),
                "events" => to(flows.events(ctx, opt::<String>(a, "run")?.map(RunId).as_ref(), opt(a, "resume")?.unwrap_or(Resume::Latest), opt(a, "limit")?.unwrap_or(100))?),
                "start" => {
                    let k = idem(a)?.ok_or_else(|| bad("`idempotency_key` is needed to start a run"))?;
                    to(flows.start(ctx, &arg::<String>(a, "flow")?, &k)?)
                }
                "stop" => to(flows.stop(ctx, &RunId(arg(a, "run")?))?),
                "approve" => to(flows.approve(ctx, &RunId(arg(a, "run")?), &arg::<String>(a, "approval_id")?, arg(a, "allow")?)?),
                _ => Err(CallError::new(ErrorCode::Unsupported, "core.unsupported", "No such call.")),
            }
        }
        _ => Err(CallError::new(ErrorCode::Unsupported, "core.unsupported", "No such call.")),
    }
}

fn stream_read(cx: &CoreCx, ctx: &CallContext, req: &Value) -> CallResult<Value> {
    let id = StreamId(arg(req, "id")?);
    let c = cx.core.streams.read(ctx, &id, opt(req, "offset")?.unwrap_or(0), opt(req, "max_len")?.unwrap_or(16 * 1024))?;
    to(c)
}

fn stream_close(cx: &CoreCx, ctx: &CallContext, req: &Value) -> CallResult<Value> {
    cx.core.streams.close(ctx, &StreamId(arg(req, "id")?))?;
    Ok(json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scope_the_catalogue_names_can_be_granted_except_the_local_marker() {
        for d in catalog::DOMAINS {
            for c in d.calls {
                assert!(c.scope == Scope::LOCAL || GRANTABLE.contains(&c.scope), "{}.{} needs {} which an administrator cannot grant", d.name, c.name, c.scope);
            }
        }
        assert!(!GRANTABLE.contains(&Scope::LOCAL));
    }

    #[test]
    fn the_gate_limits_each_computer_on_its_own() {
        let g = Gate::new(RateLimit { calls: 2, per_ms: 60_000 });
        assert!(g.check("a").is_ok() && g.check("a").is_ok());
        assert_eq!(g.check("a").unwrap_err().code, ErrorCode::RateLimited);
        assert!(g.check("b").is_ok(), "another computer is not slowed down");
    }

    #[test]
    fn the_routes_are_not_in_the_signed_route_table_of_developer_mode() {
        // they are served by this module, behind their own switch; the older table and its tests stay as they were
        for (m, p) in ROUTES {
            assert!(super::super::api::scope_of(m, p).is_none(), "{p}");
        }
    }
}
