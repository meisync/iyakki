//! The vault service's program and its client (`iyakki vault serve`; docs/development/CLOUD-MULTITENANT-PLAN.md, 3.2 and 3a).
//!
//! The service stores **ciphertext only**. Values are sealed by the member's side with the scope's key (`cloudkeys.rs`); the service holds
//! each member's public keys, the scope key wrapped for each member, and the sealed values. It never has a key that opens a vault, so
//! the people who run the service or read its volume cannot read the secrets. Because of that, the member's side does the cryptography of
//! a rotation (open the values, seal them again) and the service checks what it can: who may do it, that the key generation moves by one,
//! that the removed person has no wrap left and everybody else still has theirs, that no value is lost.
//!
//! **Scopes:** `tenant`, `namespace:<ns>` and `common`. Who may do what is decided by `cloudroles::Roles::can` on every request, read from
//! the tenant's own `roles.json`; anything not allowed is denied, and a wrap is needed as well as the role (a person who is no longer in
//! the roles cannot read even if their wrap was not yet removed, and the other way round):
//!
//! * `tenant`: every tenant member may read; an admin or owner writes and manages the members.
//! * `namespace:<ns>`: a member of the namespace reads; an editor or admin writes; the namespace's admin manages the members. A tenant admin
//!   who is not in the namespace has no access (they join it first, which the roles record).
//! * `common`: one for the whole service. Tenant members only read it; an **operator** (a name in `operators.json`) writes it and wraps
//!   its key for the tenant members that may read it. Operators have no access to any tenant's scopes.
//!
//! **Who is calling** is proved by a signed request, not by a token: `Authorization: IYAKKI-SIG id=...,ts=...,sig=...`, an Ed25519
//! signature (by the member's key, `cloudkeys.rs`) over the identity, method, path, time and the hash of the body, valid for two minutes.
//! The ids are `TENANT/PERSON` and `operator:NAME`. A signature needs no password store and no session in a service that has no accounts of its
//! own; the public keys are put on the service's volume by whoever runs it (`iyakki vault enroll`, file access as the authority) or, for the
//! members of one tenant, by that tenant's owner or admin through `POST /v1/enroll` (`iyakki vault enroll-member`): only for a person who is
//! in the tenant's roles, only with a role not higher than the caller's, and never over a different key already enrolled (replacing a key is
//! the operator's job, since whoever holds the signing key speaks as that person). A removal of a role is followed by a rotation of the
//! matching scopes by the acting administrator (`cloudrotate.rs`); what could not be rotated is kept as "pending" and retried.
//! The listener is the web service's own TLS (`webservice::tls`) and HTTP reader (`httpmini`).
//!
//! On disk (under the data directory): `operators.json`, `common/common.json`, and per tenant `tenants/T/roles.json`, `tenants/T/members.json`,
//! `tenants/T/vault/*.json`: one file per scope, written whole and atomically, mode 0600. Secret *names* are stored in the clear (the list needs
//! them); values are not. Neither is written to the log.

use crate::cloudkeys::{self, MemberKeys, PublicKeys, ScopeKey, Wrapped};
use crate::cloudroles::{self, Action, Roles};
use crate::cloudstorage::plain_name;
use crate::cloudvaultguard::{self, Guard, Limits, Refused};
use crate::httpmini::{parse_limited, Parse, Request, Response};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const MAX_BODY: usize = 256 * 1024;
const MAX_SECRETS: usize = 5000;
const MAX_SEALED_HEX: usize = 128 * 1024;
const CLOCK_SKEW: u64 = cloudvaultguard::CLOCK_SKEW;
const MAX_CONNECTIONS: usize = 64;

// ---- who and what -----------------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Principal {
    Operator(String),
    Member { tenant: String, user: String },
}

impl Principal {
    pub fn parse(id: &str) -> Option<Principal> {
        if let Some(n) = id.strip_prefix("operator:") {
            return plain_name(n).then(|| Principal::Operator(n.into()));
        }
        let (t, u) = id.split_once('/')?;
        (plain_name(t) && plain_name(u)).then(|| Principal::Member { tenant: t.into(), user: u.into() })
    }

    pub fn id(&self) -> String {
        match self {
            Principal::Operator(n) => format!("operator:{n}"),
            Principal::Member { tenant, user } => format!("{tenant}/{user}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    Tenant,
    Namespace(String),
    Common,
}

impl Scope {
    pub fn parse(s: &str) -> Option<Scope> {
        match s {
            "tenant" => Some(Scope::Tenant),
            "common" => Some(Scope::Common),
            _ => s.strip_prefix("namespace:").filter(|n| plain_name(n)).map(|n| Scope::Namespace(n.into())),
        }
    }

    pub fn name(&self) -> String {
        match self {
            Scope::Tenant => "tenant".into(),
            Scope::Namespace(n) => format!("namespace:{n}"),
            Scope::Common => "common".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Read,
    Write,
    Manage,
    /// What a rotation needs: a writer of the scope (a namespace editor, a tenant admin). See [`Store::rotate`].
    Rotate,
}

#[derive(Debug, PartialEq, Eq)]
pub enum VaultError {
    Unauthenticated,
    Denied,
    NotFound,
    Conflict(String),
    Invalid(String),
    /// Too many requests (or too many failures) from this key or address: try again after this many seconds.
    RateLimited(u64),
    Io,
}

impl VaultError {
    pub fn status(&self) -> u16 {
        match self {
            VaultError::Unauthenticated => 401,
            VaultError::Denied => 403,
            VaultError::NotFound => 404,
            VaultError::Conflict(_) => 409,
            VaultError::Invalid(_) => 400,
            VaultError::RateLimited(_) => 429,
            VaultError::Io => 500,
        }
    }

    pub fn message(&self) -> String {
        match self {
            VaultError::Unauthenticated => "sign the request with your key".into(),
            VaultError::Denied => "no access".into(),
            VaultError::NotFound => "not found".into(),
            VaultError::Conflict(m) | VaultError::Invalid(m) => m.clone(),
            VaultError::RateLimited(_) => "too many requests: slow down".into(),
            VaultError::Io => "internal error".into(),
        }
    }
}

type R<T> = Result<T, VaultError>;

fn invalid<T>(m: &str) -> R<T> {
    Err(VaultError::Invalid(m.into()))
}

fn io<E>(_: E) -> VaultError {
    VaultError::Io
}

// ---- the files ---------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScopeData {
    generation: u64,
    wraps: BTreeMap<String, Wrapped>,
    secrets: BTreeMap<String, String>,
}

fn read_json<T: serde::de::DeserializeOwned>(p: &Path) -> R<Option<T>> {
    match std::fs::read_to_string(p) {
        Ok(t) => serde_json::from_str(&t).map(Some).map_err(|e| {
            log::warn!("vault service: {}: {e}", p.display());
            VaultError::Io
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => {
            log::warn!("vault service: {}: {e}", p.display());
            Err(VaultError::Io)
        }
    }
}

/// Whole and atomic (a temporary file, then a rename), readable by the service's user only.
fn write_json<T: Serialize>(p: &Path, v: &T) -> R<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = p.parent().ok_or(VaultError::Io)?;
    std::fs::create_dir_all(dir).map_err(io)?;
    let tmp = p.with_extension("json.tmp");
    let text = serde_json::to_vec_pretty(v).map_err(io)?;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(io)?;
    f.write_all(&text).and_then(|_| f.sync_all()).map_err(io)?;
    std::fs::rename(&tmp, p).map_err(io)
}

fn secret_name_ok(n: &str) -> bool {
    !n.is_empty() && n.len() <= 128 && !n.starts_with('.') && n.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

fn sealed_ok(s: &str) -> bool {
    s.len() <= MAX_SEALED_HEX && cloudkeys::unhex(s).is_some_and(|b| b.len() >= 24 + 16)
}

// ---- the store ---------------------------------------------------------------------------------------------------------------------

pub struct Store {
    data: PathBuf,
    /// One writer at a time: the files are the state.
    lock: Mutex<()>,
    /// Replays and rates (`cloudvaultguard.rs`).
    guard: Mutex<Guard>,
}

impl Store {
    pub fn open(data: &Path) -> Result<Store, String> {
        Store::open_at(data, unix_now())
    }

    /// [`Store::open`] with the clock given (the replay log is read against it).
    pub fn open_at(data: &Path, now: u64) -> Result<Store, String> {
        std::fs::create_dir_all(data).map_err(|e| format!("{}: {e}", data.display()))?;
        Ok(Store { data: data.to_path_buf(), lock: Mutex::new(()), guard: Mutex::new(Guard::open(data, now, Limits::default())) })
    }

    /// Other limits than the defaults (tests; a deployment that needs them).
    pub fn set_limits(&self, l: Limits) {
        self.guard.lock().unwrap_or_else(|e| e.into_inner()).set_limits(l);
    }

    fn operators_path(&self) -> PathBuf {
        self.data.join("operators.json")
    }

    fn tenant_dir(&self, tenant: &str) -> R<PathBuf> {
        if !plain_name(tenant) {
            return Err(VaultError::Denied);
        }
        Ok(self.data.join("tenants").join(tenant))
    }

    fn scope_path(&self, p: &Principal, s: &Scope) -> R<PathBuf> {
        Ok(match (s, p) {
            (Scope::Common, _) => self.data.join("common").join("common.json"),
            (Scope::Tenant, Principal::Member { tenant, .. }) => self.tenant_dir(tenant)?.join("vault").join("tenant.json"),
            (Scope::Namespace(n), Principal::Member { tenant, .. }) => self.tenant_dir(tenant)?.join("vault").join(format!("ns-{n}.json")),
            _ => return Err(VaultError::Denied),
        })
    }

    fn roles_of(&self, tenant: &str) -> R<Roles> {
        // no roles file: nobody is a member of anything
        match cloudroles::load(&cloudroles::path(&self.tenant_dir(tenant)?)) {
            Ok(Some(r)) => Ok(r),
            Ok(None) => Err(VaultError::Denied),
            Err(e) => {
                log::warn!("vault service: roles of a tenant: {e}");
                Err(VaultError::Io)
            }
        }
    }

    // ---- public keys -----------------------------------------------------------------------------------------------------------

    fn keys_of(&self, id: &Principal) -> R<Option<PublicKeys>> {
        match id {
            Principal::Operator(n) => Ok(read_json::<BTreeMap<String, PublicKeys>>(&self.operators_path())?.and_then(|m| m.get(n).cloned())),
            Principal::Member { tenant, user } => Ok(read_json::<BTreeMap<String, PublicKeys>>(&self.tenant_dir(tenant)?.join("members.json"))?.and_then(|m| m.get(user).cloned())),
        }
    }

    /// Put someone's public keys on the volume: the operator's way (`iyakki vault enroll`), with file access as the authority.
    pub fn enroll(&self, who: &Principal, keys: &PublicKeys) -> Result<(), String> {
        keys.check()?;
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let (path, name) = match who {
            Principal::Operator(n) => (self.operators_path(), n.clone()),
            Principal::Member { tenant, user } => (self.tenant_dir(tenant).map_err(|_| "not a tenant name")?.join("members.json"), user.clone()),
        };
        let mut m = read_json::<BTreeMap<String, PublicKeys>>(&path).map_err(|_| "cannot read the key list")?.unwrap_or_default();
        m.insert(name, keys.clone());
        write_json(&path, &m).map_err(|_| "cannot write the key list".to_string())
    }

    /// Checks the signed request and says who sent it. `now` is seconds since 1970. Without a source address only the key-side checks
    /// apply (freshness, replay, the key's rate); [`Store::authenticate_from`] adds the source's.
    pub fn authenticate(&self, method: &str, path: &str, body: &[u8], header: Option<&str>, now: u64) -> R<Principal> {
        self.authenticate_from(method, path, body, header, now, None)
    }

    /// In order: the source's bucket; the header; the clock (120 s either way); the enrolled key and the signature; the nonce (required) not
    /// seen before; the key's bucket. Failures count against the source only (never against the name claimed), and a locked source
    /// still gets through when its request verifies (see `cloudvaultguard.rs`).
    pub fn authenticate_from(&self, method: &str, path: &str, body: &[u8], header: Option<&str>, now: u64, source: Option<IpAddr>) -> R<Principal> {
        let g = &mut *self.guard.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ip) = source {
            g.source_allowed(ip, now).map_err(VaultError::RateLimited)?;
        }
        let verdict = (|| -> R<(Principal, cloudkeys::SignedRequest)> {
            let sr = header.and_then(cloudkeys::parse_authorization).ok_or(VaultError::Unauthenticated)?;
            let who = Principal::parse(&sr.id).ok_or(VaultError::Unauthenticated)?;
            if sr.ts.abs_diff(now) > CLOCK_SKEW || sr.nonce.is_none() {
                return Err(VaultError::Unauthenticated);
            }
            let keys = self.keys_of(&who)?.ok_or(VaultError::Unauthenticated)?;
            if sr.verify(&keys, method, path, body) {
                Ok((who, sr))
            } else {
                Err(VaultError::Unauthenticated)
            }
        })();
        let fail = |g: &mut Guard, e: VaultError| {
            if let Some(ip) = source {
                let left = g.locked_for(ip, now);
                if left > 0 {
                    return VaultError::RateLimited(left);
                }
                g.failed(ip, now);
            }
            e
        };
        let (who, sr) = match verdict {
            Ok(v) => v,
            Err(VaultError::Io) => return Err(VaultError::Io),
            Err(e) => return Err(fail(g, e)),
        };
        match g.accept(&sr.id, sr.nonce.as_deref().unwrap_or(""), sr.ts, now) {
            Ok(()) => {
                if let Some(ip) = source {
                    g.succeeded(ip, now);
                }
                Ok(who)
            }
            Err(Refused::Slow(s)) => Err(VaultError::RateLimited(s)),
            Err(Refused::Rejected(_)) => Err(fail(g, VaultError::Unauthenticated)),
        }
    }

    /// The public keys of `target` (`PERSON`, or `TENANT/PERSON` / `operator:NAME` for an operator): so a wrap can be made for them.
    pub fn lookup(&self, p: &Principal, target: &str) -> R<PublicKeys> {
        let t = match (p, target.contains(['/', ':'])) {
            (Principal::Member { tenant, .. }, false) => {
                // only people who are in the tenant
                if self.roles_of(tenant)?.tenant_role(target).is_none() {
                    return Err(VaultError::Denied);
                }
                Principal::Member { tenant: tenant.clone(), user: target.into() }
            }
            (Principal::Operator(_), true) => Principal::parse(target).ok_or(VaultError::Denied)?,
            _ => return Err(VaultError::Denied),
        };
        if let Principal::Member { tenant, user } = &t {
            if self.roles_of(tenant)?.tenant_role(user).is_none() {
                return Err(VaultError::Denied);
            }
        }
        self.keys_of(&t)?.ok_or(VaultError::NotFound)
    }

    /// A tenant owner or admin puts the public keys of a person of THEIR tenant on the service. Deny by default: the caller must be a
    /// member of the tenant with the right to manage its members, `name` must already be in that tenant's roles with a role not above the
    /// caller's, and a key that is already there is never replaced (the same keys again are accepted: nothing changes).
    pub fn enroll_member(&self, p: &Principal, name: &str, keys: &PublicKeys) -> R<()> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let Principal::Member { tenant, user } = p else { return Err(VaultError::Denied) };
        let roles = self.roles_of(tenant)?;
        if !roles.can(user, &Action::ManageTenantMembers).ok() || !plain_name(name) {
            return Err(VaultError::Denied);
        }
        let (Some(target), Some(mine)) = (roles.tenant_role(name), roles.tenant_role(user)) else { return Err(VaultError::Denied) };
        if target > mine {
            return Err(VaultError::Denied);
        }
        keys.check().or_else(|e| invalid(&e))?;
        let path = self.tenant_dir(tenant)?.join("members.json");
        let mut m = read_json::<BTreeMap<String, PublicKeys>>(&path)?.unwrap_or_default();
        match m.get(name) {
            Some(k) if k == keys => return Ok(()),
            Some(_) => return Err(VaultError::Conflict("that person already has a key on the service; the operator replaces it".into())),
            None => {}
        }
        m.insert(name.into(), keys.clone());
        write_json(&path, &m)
    }

    // ---- who may do what -------------------------------------------------------------------------------------------------------

    /// The one place a decision is made. Everything not listed here is denied.
    fn allow(&self, p: &Principal, s: &Scope, op: Op) -> R<()> {
        let ok = match p {
            Principal::Operator(_) => *s == Scope::Common,
            Principal::Member { tenant, user } => {
                let roles = self.roles_of(tenant)?;
                let action = match (s, op) {
                    (Scope::Common, Op::Read) | (Scope::Tenant, Op::Read) => Some(Action::EnterTenant),
                    (Scope::Common, _) => None,
                    (Scope::Tenant, Op::Write | Op::Rotate | Op::Manage) => Some(Action::ManageTenantMembers),
                    (Scope::Namespace(n), Op::Rotate) => Some(Action::WriteNamespace(n.clone())),
                    (Scope::Namespace(n), Op::Read) => Some(Action::ReadNamespace(n.clone())),
                    (Scope::Namespace(n), Op::Write) => Some(Action::WriteNamespace(n.clone())),
                    (Scope::Namespace(n), Op::Manage) => Some(Action::ManageNamespaceMembers(n.clone())),
                };
                action.is_some_and(|a| roles.can(user, &a).ok())
            }
        };
        if ok {
            Ok(())
        } else {
            Err(VaultError::Denied)
        }
    }

    /// The role check, the scope, and the caller's wrap in it: both are needed.
    fn open_scope(&self, p: &Principal, s: &Scope, op: Op) -> R<(PathBuf, ScopeData)> {
        self.allow(p, s, op)?;
        let path = self.scope_path(p, s)?;
        let d = read_json::<ScopeData>(&path)?.ok_or(VaultError::NotFound)?;
        if !d.wraps.contains_key(&self.member_id(p, s)) {
            return Err(VaultError::Denied);
        }
        Ok((path, d))
    }

    /// The name a member goes by inside a scope's file: the plain name inside a tenant, the full id in `common`.
    fn member_id(&self, p: &Principal, s: &Scope) -> String {
        match (p, s) {
            (Principal::Member { user, .. }, Scope::Tenant | Scope::Namespace(_)) => user.clone(),
            _ => p.id(),
        }
    }

    // ---- operations ------------------------------------------------------------------------------------------------------------

    /// Make a scope; `wrap` is its first key wrapped for the caller.
    pub fn create_scope(&self, p: &Principal, s: &Scope, wrap: &Wrapped) -> R<()> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.allow(p, s, Op::Manage)?;
        wrap.check().or_else(|e| invalid(&e))?;
        let path = self.scope_path(p, s)?;
        if path.exists() {
            return Err(VaultError::Conflict("the scope exists".into()));
        }
        let mut d = ScopeData { generation: 1, ..ScopeData::default() };
        d.wraps.insert(self.member_id(p, s), wrap.clone());
        write_json(&path, &d)
    }

    /// The caller's own wrap and the key generation it is for.
    pub fn my_wrap(&self, p: &Principal, s: &Scope) -> R<(u64, Wrapped)> {
        let (_, d) = self.open_scope(p, s, Op::Read)?;
        let w = d.wraps.get(&self.member_id(p, s)).cloned().ok_or(VaultError::Denied)?;
        Ok((d.generation, w))
    }

    pub fn list(&self, p: &Principal, s: &Scope) -> R<Vec<String>> {
        let (_, d) = self.open_scope(p, s, Op::Read)?;
        Ok(d.secrets.keys().cloned().collect())
    }

    pub fn get(&self, p: &Principal, s: &Scope, name: &str) -> R<(u64, String)> {
        let (_, d) = self.open_scope(p, s, Op::Read)?;
        d.secrets.get(name).map(|v| (d.generation, v.clone())).ok_or(VaultError::NotFound)
    }

    /// Store a value sealed with the scope's key of `generation` (the current one: a writer holding an older key is told to fetch the new wrap).
    pub fn put(&self, p: &Principal, s: &Scope, name: &str, generation: u64, sealed: &str) -> R<()> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let (path, mut d) = self.open_scope(p, s, Op::Write)?;
        if !secret_name_ok(name) || !sealed_ok(sealed) {
            return invalid("the name or the sealed value is not valid");
        }
        if generation != d.generation {
            return Err(VaultError::Conflict("the scope's key has changed: fetch your wrap again".into()));
        }
        if !d.secrets.contains_key(name) && d.secrets.len() >= MAX_SECRETS {
            return Err(VaultError::Conflict("the scope holds as many secrets as it may".into()));
        }
        d.secrets.insert(name.into(), sealed.into());
        write_json(&path, &d)
    }

    pub fn delete(&self, p: &Principal, s: &Scope, name: &str) -> R<()> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let (path, mut d) = self.open_scope(p, s, Op::Write)?;
        d.secrets.remove(name).ok_or(VaultError::NotFound)?;
        write_json(&path, &d)
    }

    /// Who holds a wrap in the scope (for the person who manages it, to wrap again after a removal).
    pub fn members(&self, p: &Principal, s: &Scope) -> R<Vec<String>> {
        let (_, d) = self.open_scope(p, s, Op::Manage)?;
        Ok(d.wraps.keys().cloned().collect())
    }

    /// Who holds a wrap in the scope, for someone who may rotate it (a writer of the scope; see [`Store::rotate`]). [`Store::members`] stays
    /// for the people who manage the scope.
    pub fn rotation_members(&self, p: &Principal, s: &Scope) -> R<Vec<String>> {
        let (_, d) = self.open_scope(p, s, Op::Rotate)?;
        Ok(d.wraps.keys().cloned().collect())
    }

    /// Add a member: the caller (who holds the key) sends the key wrapped for them. The person must already be in the roles for this
    /// scope; the service cannot check the wrap opens, only that it is the right shape.
    pub fn add_member(&self, p: &Principal, s: &Scope, who: &str, wrap: &Wrapped) -> R<()> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let (path, mut d) = self.open_scope(p, s, Op::Manage)?;
        wrap.check().or_else(|e| invalid(&e))?;
        let tenant = match p {
            Principal::Member { tenant, .. } => Some(tenant.clone()),
            Principal::Operator(_) => None,
        };
        let entitled = match (s, &tenant) {
            (Scope::Tenant, Some(t)) => plain_name(who) && self.roles_of(t)?.tenant_role(who).is_some(),
            (Scope::Namespace(n), Some(t)) => plain_name(who) && self.roles_of(t)?.ns_role(n, who).is_some(),
            (Scope::Common, None) => match Principal::parse(who) {
                Some(Principal::Member { tenant, user }) => self.roles_of(&tenant).is_ok_and(|r| r.tenant_role(&user).is_some()),
                _ => false,
            },
            _ => false,
        };
        if !entitled {
            return Err(VaultError::Denied);
        }
        d.wraps.insert(who.into(), wrap.clone());
        write_json(&path, &d)
    }

    /// Rotate the scope's key: `generation` is the next one, `wraps` the new key wrapped for everyone who keeps access (all current
    /// holders except `remove`), `values` every secret sealed again. The old wraps and values are replaced, so the removed person's key
    /// opens nothing that was written from now on, and the file no longer holds their wrap.
    ///
    /// **Who may rotate.** A person who manages the scope may rotate it and remove anyone. A *writer* of the scope (a namespace editor, which
    /// is what a tenant's pod is given; a tenant admin for the tenant scope) may rotate it too, under two conditions the service
    /// checks itself: they hold a wrap, and they remove nobody who the tenant's roles still entitle to the scope. So a pod can finish
    /// the rotation that follows a revoked role (the roles file is the authority; a rotation is only catching up with it) or change the
    /// key of its own scope without removing anyone, but cannot use rotation to cut a member off.
    pub fn rotate(&self, p: &Principal, s: &Scope, remove: Option<&str>, generation: u64, wraps: &BTreeMap<String, Wrapped>, values: &BTreeMap<String, String>) -> R<()> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let (path, d) = self.open_scope(p, s, Op::Rotate)?;
        if self.allow(p, s, Op::Manage).is_err() {
            if let (Some(r), Principal::Member { tenant, .. }) = (remove, p) {
                let roles = self.roles_of(tenant)?;
                let entitled = match s {
                    Scope::Tenant => roles.tenant_role(r).is_some(),
                    Scope::Namespace(n) => roles.ns_role(n, r).is_some(),
                    Scope::Common => true,
                };
                if entitled {
                    return Err(VaultError::Denied);
                }
            }
        }
        let me = self.member_id(p, s);
        if remove == Some(me.as_str()) {
            return invalid("you cannot remove yourself");
        }
        if let Some(r) = remove {
            if !d.wraps.contains_key(r) {
                return Err(VaultError::NotFound);
            }
        }
        if generation != d.generation + 1 {
            return Err(VaultError::Conflict("the new generation must follow the current one".into()));
        }
        let keep: BTreeSet<&String> = d.wraps.keys().filter(|k| Some(k.as_str()) != remove).collect();
        if wraps.keys().collect::<BTreeSet<_>>() != keep {
            return invalid("the new wraps must be for exactly the members who keep access");
        }
        if values.keys().collect::<BTreeSet<_>>() != d.secrets.keys().collect::<BTreeSet<_>>() {
            return invalid("every secret must be sealed again, and no other");
        }
        if wraps.values().any(|w| w.check().is_err()) || values.values().any(|v| !sealed_ok(v)) {
            return invalid("a wrap or a value is not the right shape");
        }
        write_json(&path, &ScopeData { generation, wraps: wraps.clone(), secrets: values.clone() })
    }

    // ---- the HTTP routes -------------------------------------------------------------------------------------------------------

    /// One request, already read: sign-in, route, answer. Nothing but the kind of route and the status is logged.
    pub fn handle(&self, req: &Request, now: u64) -> Response {
        self.handle_from(req, now, None)
    }

    /// [`Store::handle`] for a request that came from `source` (its rate limits and lockout apply).
    pub fn handle_from(&self, req: &Request, now: u64, source: Option<IpAddr>) -> Response {
        let who = match self.authenticate_from(&req.method, &req.path, &req.body, req.header("authorization"), now, source) {
            Ok(w) => w,
            Err(e) => return fail(&e),
        };
        let body: Value = if req.body.is_empty() { Value::Null } else { serde_json::from_slice(&req.body).unwrap_or(Value::Null) };
        let segs: Vec<&str> = req.path.trim_start_matches('/').split('/').collect();
        let r = self.route(&who, &req.method, &segs, &body);
        let status = r.as_ref().map(|_| 200).unwrap_or_else(|e| e.status());
        log::info!("vault service: {} {} {} {}", req.method, segs.get(1).copied().unwrap_or("-"), status, who.id());
        match r {
            Ok(v) => Response::json(200, &v),
            Err(e) => fail(&e),
        }
    }

    fn route(&self, p: &Principal, method: &str, segs: &[&str], body: &Value) -> R<Value> {
        let scope = |s: &str| Scope::parse(s).ok_or(VaultError::NotFound);
        let wrap_of = |b: &Value| -> R<Wrapped> { serde_json::from_value(b["wrap"].clone()).or_else(|_| invalid("send {\"wrap\":{\"epk\":..,\"ct\":..}}")) };
        match (method, segs) {
            ("GET", ["v1", "keys", rest @ ..]) if !rest.is_empty() => Ok(serde_json::to_value(self.lookup(p, &rest.join("/"))?).unwrap_or_default()),
            ("POST", ["v1", "enroll"]) => {
                let name = body["name"].as_str().ok_or(VaultError::Invalid("send {\"name\":..,\"keys\":{\"wrap\":..,\"sign\":..}}".into()))?;
                let keys: PublicKeys = serde_json::from_value(body["keys"].clone()).or_else(|_| invalid("send the keys as iyakki vault keygen prints them"))?;
                self.enroll_member(p, name, &keys).map(|_| json!({}))
            }
            ("POST", ["v1", "scope", s]) => self.create_scope(p, &scope(s)?, &wrap_of(body)?).map(|_| json!({"generation": 1})),
            ("GET", ["v1", "scope", s, "wrap"]) => self.my_wrap(p, &scope(s)?).map(|(g, w)| json!({"generation": g, "wrap": w})),
            ("GET", ["v1", "scope", s, "members"]) => self.members(p, &scope(s)?).map(|m| json!({"members": m})),
            ("GET", ["v1", "scope", s, "rotation-members"]) => self.rotation_members(p, &scope(s)?).map(|m| json!({"members": m})),
            ("GET", ["v1", "scope", s, "secrets"]) => self.list(p, &scope(s)?).map(|n| json!({"names": n})),
            ("GET", ["v1", "scope", s, "secrets", n]) => self.get(p, &scope(s)?, n).map(|(g, v)| json!({"generation": g, "value": v})),
            ("PUT", ["v1", "scope", s, "secrets", n]) => {
                let g = body["generation"].as_u64().ok_or(VaultError::Invalid("send the generation".into()))?;
                self.put(p, &scope(s)?, n, g, body["value"].as_str().unwrap_or("")).map(|_| json!({}))
            }
            ("DELETE", ["v1", "scope", s, "secrets", n]) => self.delete(p, &scope(s)?, n).map(|_| json!({})),
            ("PUT", ["v1", "scope", s, "members", rest @ ..]) if !rest.is_empty() => self.add_member(p, &scope(s)?, &rest.join("/"), &wrap_of(body)?).map(|_| json!({})),
            ("POST", ["v1", "scope", s, "rotate"]) => {
                let wraps: BTreeMap<String, Wrapped> = serde_json::from_value(body["wraps"].clone()).or_else(|_| invalid("send the wraps"))?;
                let values: BTreeMap<String, String> = serde_json::from_value(body["values"].clone()).or_else(|_| invalid("send the values"))?;
                let g = body["generation"].as_u64().ok_or(VaultError::Invalid("send the generation".into()))?;
                self.rotate(p, &scope(s)?, body["remove"].as_str(), g, &wraps, &values).map(|_| json!({"generation": g}))
            }
            _ => Err(VaultError::NotFound),
        }
    }
}

fn fail(e: &VaultError) -> Response {
    let r = Response::json(e.status(), &json!({"error": e.message()}));
    match e {
        VaultError::RateLimited(s) => r.header("Retry-After", &s.max(&1).to_string()),
        _ => r,
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ---- the listener ------------------------------------------------------------------------------------------------------------------

/// A running service. Dropping it stops it; [`Running::stop`] waits for the accept thread.
pub struct Running {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    pub fingerprint: String,
}

impl Running {
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = h.join();
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Listen on `addr` (HTTPS only) and answer for the store in `data`. The certificate is the person's own (`tls`), or one made and kept under
/// `data/tls` (its fingerprint is logged, for the clients to pin).
pub fn start(addr: SocketAddr, data: &Path, tls: Option<(&Path, &Path)>) -> Result<Running, String> {
    use crate::webservice::tls as wtls;
    let identity = match tls {
        Some((c, k)) => wtls::from_files(c, k)?,
        None => {
            let host = std::fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()).unwrap_or_default();
            wtls::self_signed(&data.join("tls"), &[host].into_iter().filter(|h| !h.is_empty()).chain(["vault.iyakki-vault.svc".to_string()]).collect::<Vec<_>>())?
        }
    };
    let store = Arc::new(Store::open(data)?);
    let listener = TcpListener::bind(addr).map_err(|e| format!("cannot listen on {addr}: {e}"))?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let local = listener.local_addr().map_err(|e| e.to_string())?;
    let stop = Arc::new(AtomicBool::new(false));
    let open = Arc::new(AtomicUsize::new(0));
    let (cfg, st, stp) = (identity.config.clone(), stop.clone(), stop.clone());
    let h = std::thread::Builder::new()
        .name("iyakki-vault-accept".into())
        .spawn(move || {
            while !st.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((sock, _)) => {
                        if open.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                            continue;
                        }
                        open.fetch_add(1, Ordering::SeqCst);
                        let (store, cfg, open) = (store.clone(), cfg.clone(), open.clone());
                        let spawned = std::thread::Builder::new().name("iyakki-vault-conn".into()).spawn({
                            let open = open.clone();
                            move || {
                                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| connection(sock, &cfg, &store)));
                                if r.is_err() {
                                    log::error!("vault service: a connection handler crashed");
                                }
                                open.fetch_sub(1, Ordering::SeqCst);
                            }
                        });
                        if spawned.is_err() {
                            open.fetch_sub(1, Ordering::SeqCst);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(15)),
                    Err(_) => std::thread::sleep(Duration::from_millis(100)),
                }
            }
        })
        .map_err(|e| e.to_string())?;
    log::info!("vault service: on https://{local} (certificate SHA-256 {})", identity.fingerprint);
    Ok(Running { addr: local, stop: stp, thread: Mutex::new(Some(h)), fingerprint: identity.fingerprint })
}

fn connection(sock: TcpStream, cfg: &Arc<rustls::ServerConfig>, store: &Store) {
    let _ = sock.set_nodelay(true);
    let peer = sock.peer_addr().ok().map(|a| a.ip());
    let _ = sock.set_nonblocking(false);
    let _ = sock.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = sock.set_write_timeout(Some(Duration::from_secs(10)));
    let Ok(c) = rustls::ServerConnection::new(cfg.clone()) else { return };
    let mut tls = rustls::StreamOwned::new(c, sock);
    let started = std::time::Instant::now();
    let (mut buf, mut chunk) = (Vec::new(), [0u8; 4096]);
    let resp = loop {
        match parse_limited(&buf, MAX_BODY) {
            Parse::Done(r) => break store.handle_from(&r, unix_now(), peer),
            Parse::Bad(code, msg) => break Response::json(code, &json!({"error": msg})),
            Parse::Incomplete => {}
        }
        if started.elapsed() > Duration::from_secs(10) {
            return;
        }
        match tls.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let _ = tls.write_all(&resp.header("Strict-Transport-Security", "max-age=31536000").to_bytes());
    let _ = tls.flush();
    tls.conn.send_close_notify();
    let _ = tls.flush();
}

// ---- the member's side -------------------------------------------------------------------------------------------------------------

/// A member talking to a vault service: signs every request, wraps and unwraps keys, seals and opens values on this side.
pub struct Client {
    pub host: String,
    pub port: u16,
    /// The certificate's SHA-256 fingerprint to pin.
    pub fingerprint: Option<String>,
    pub who: Principal,
    pub keys: MemberKeys,
}

impl Client {
    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let bytes = body.map(|b| b.to_string().into_bytes()).unwrap_or_default();
        let mut tries = 0;
        let (r, v) = loop {
            // a new signature (and nonce) every time: the service refuses a request it has seen
            let auth = self.keys.sign_request(&self.who.id(), method, path, unix_now(), &bytes);
            let mut headers = vec![("Authorization".to_string(), auth)];
            if body.is_some() {
                headers.push(("Content-Type".into(), "application/json".into()));
            }
            let r = crate::devmode::client::https(&self.host, self.port, self.fingerprint.as_deref(), method, path, &headers, &bytes, Duration::from_secs(15))?;
            let v = r.json();
            tries += 1;
            if r.status == 429 && tries < 4 {
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
            break (r, v);
        };
        if r.status == 200 {
            Ok(v)
        } else {
            Err(format!("{}: {}", r.status, v["error"].as_str().unwrap_or("refused")))
        }
    }

    fn member_name(&self, s: &Scope) -> String {
        match (&self.who, s) {
            (Principal::Member { user, .. }, Scope::Tenant | Scope::Namespace(_)) => user.clone(),
            (w, _) => w.id(),
        }
    }

    pub fn public_keys(&self, target: &str) -> Result<PublicKeys, String> {
        serde_json::from_value(self.call("GET", &format!("/v1/keys/{target}"), None)?).map_err(|e| e.to_string())
    }

    /// Make the scope with a new key, wrapped for this member.
    pub fn create_scope(&self, s: &Scope) -> Result<(), String> {
        let key = ScopeKey::generate()?;
        let w = key.wrap(&s.name(), &self.member_name(s), 1, &self.keys.public())?;
        self.call("POST", &format!("/v1/scope/{}", s.name()), Some(&json!({"wrap": w}))).map(|_| ())
    }

    /// The scope's current key as this member holds it, and its generation.
    pub fn scope_key(&self, s: &Scope) -> Result<(u64, ScopeKey), String> {
        let v = self.call("GET", &format!("/v1/scope/{}/wrap", s.name()), None)?;
        let g = v["generation"].as_u64().ok_or("no generation")?;
        let w: Wrapped = serde_json::from_value(v["wrap"].clone()).map_err(|e| e.to_string())?;
        Ok((g, self.keys.unwrap_key(&s.name(), &self.member_name(s), g, &w)?))
    }

    pub fn put(&self, s: &Scope, name: &str, value: &[u8]) -> Result<(), String> {
        let (g, key) = self.scope_key(s)?;
        let sealed = key.seal(&s.name(), name, g, value)?;
        self.call("PUT", &format!("/v1/scope/{}/secrets/{name}", s.name()), Some(&json!({"generation": g, "value": sealed}))).map(|_| ())
    }

    pub fn get(&self, s: &Scope, name: &str) -> Result<Vec<u8>, String> {
        let (g, key) = self.scope_key(s)?;
        let v = self.call("GET", &format!("/v1/scope/{}/secrets/{name}", s.name()), None)?;
        if v["generation"].as_u64() != Some(g) {
            return Err("the key changed while reading: try again".into());
        }
        key.open(&s.name(), name, g, v["value"].as_str().unwrap_or(""))
    }

    pub fn list(&self, s: &Scope) -> Result<Vec<String>, String> {
        let v = self.call("GET", &format!("/v1/scope/{}/secrets", s.name()), None)?;
        Ok(v["names"].as_array().map(|a| a.iter().filter_map(|n| n.as_str().map(String::from)).collect()).unwrap_or_default())
    }

    pub fn delete(&self, s: &Scope, name: &str) -> Result<(), String> {
        self.call("DELETE", &format!("/v1/scope/{}/secrets/{name}", s.name()), None).map(|_| ())
    }

    /// Wrap the scope's key for `who` (looked up by `lookup` name: see [`Store::lookup`]) and send it.
    pub fn add_member(&self, s: &Scope, who: &str) -> Result<(), String> {
        let (g, key) = self.scope_key(s)?;
        let w = key.wrap(&s.name(), who, g, &self.public_keys(who)?)?;
        self.call("PUT", &format!("/v1/scope/{}/members/{who}", s.name()), Some(&json!({"wrap": w}))).map(|_| ())
    }

    /// Enrol a person of this member's tenant (the member must be an owner or admin of it).
    pub fn enroll_member(&self, name: &str, keys: &PublicKeys) -> Result<(), String> {
        self.call("POST", "/v1/enroll", Some(&json!({"name": name, "keys": keys}))).map(|_| ())
    }

    /// Change the scope's key without removing anyone (a routine rotation, or a pod rotating its own namespace's key). Needs the right
    /// to rotate the scope (see [`Store::rotate`]).
    pub fn rotate_key(&self, s: &Scope) -> Result<(), String> {
        self.rotate_to(s, None)
    }

    /// Rotate `who` out of `s` if there is anything to rotate: no scope, or `who` holding no wrap in it, is "nothing to do" (a role
    /// that was never given a vault key). Any other failure is an error, to be retried.
    pub fn rotate_out(&self, s: &Scope, who: &str) -> Result<bool, String> {
        match self.call("GET", &format!("/v1/scope/{}/wrap", s.name()), None) {
            Err(e) if e.starts_with("404") => return Ok(false),
            Err(e) => return Err(e),
            Ok(_) => {}
        }
        let members = self.call("GET", &format!("/v1/scope/{}/rotation-members", s.name()), None)?;
        if !members["members"].as_array().into_iter().flatten().any(|m| m.as_str() == Some(who)) {
            return Ok(false);
        }
        self.remove_member(s, who).map(|_| true)
    }

    /// Remove a member by rotating: open every value, make a new key, wrap it for everyone who stays, seal every value again.
    pub fn remove_member(&self, s: &Scope, who: &str) -> Result<(), String> {
        self.rotate_to(s, Some(who))
    }

    fn rotate_to(&self, s: &Scope, remove: Option<&str>) -> Result<(), String> {
        let (g, old) = self.scope_key(s)?;
        let members = self.call("GET", &format!("/v1/scope/{}/rotation-members", s.name()), None)?;
        let new = ScopeKey::generate()?;
        let ng = g + 1;
        let mut wraps = BTreeMap::new();
        for m in members["members"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|m| remove != Some(*m)) {
            let keys = if m == self.member_name(s) { self.keys.public() } else { self.public_keys(m)? };
            wraps.insert(m.to_string(), new.wrap(&s.name(), m, ng, &keys)?);
        }
        let mut values = BTreeMap::new();
        let mut plains: BTreeMap<String, zeroize::Zeroizing<Vec<u8>>> = BTreeMap::new();
        for n in self.list(s)? {
            let v = self.call("GET", &format!("/v1/scope/{}/secrets/{n}", s.name()), None)?;
            let plain = zeroize::Zeroizing::new(old.open(&s.name(), &n, g, v["value"].as_str().unwrap_or(""))?);
            let sealed = new.seal(&s.name(), &n, ng, &plain)?;
            // the service cannot read what it stores, so the check is made here: before sending, the new ciphertext must open to the same value
            if new.open(&s.name(), &n, ng, &sealed).map(zeroize::Zeroizing::new).ok().as_ref() != Some(&plain) {
                return Err(format!("rotation of {} stopped: {n} did not seal again correctly; nothing was changed", s.name()));
            }
            values.insert(n.clone(), sealed);
            plains.insert(n, plain);
        }
        self.call("POST", &format!("/v1/scope/{}/rotate", s.name()), Some(&json!({"remove": remove, "generation": ng, "wraps": wraps, "values": values})))?;
        // and after: what the service now holds must open with the new key to the same values (a damaged write is reported at once, not found later)
        let (g2, check) = self.scope_key(s)?;
        if g2 != ng {
            return Err(format!("rotation of {} could not be verified: the service reports generation {g2}, not {ng}", s.name()));
        }
        for (n, plain) in &plains {
            let v = self.call("GET", &format!("/v1/scope/{}/secrets/{n}", s.name()), None)?;
            let ok = check.open(&s.name(), n, ng, v["value"].as_str().unwrap_or("")).map(zeroize::Zeroizing::new).ok().as_ref() == Some(plain);
            if !ok {
                return Err(format!("rotation of {} could not be verified: {n} does not open to the same value with the new key", s.name()));
            }
        }
        Ok(())
    }
}

// ---- the command -------------------------------------------------------------------------------------------------------------------

fn opt(a: &[String], name: &str) -> Option<String> {
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).cloned()
}

/// `iyakki vault serve | keygen | enroll | enroll-member`.
pub fn run(a: &[String], out: &mut dyn FnMut(&str)) -> Result<(), String> {
    match a.first().map(String::as_str) {
        Some("serve") => {
            let listen = opt(a, "--listen").unwrap_or_else(|| format!("127.0.0.1:{}", crate::cloudvault::PORT));
            let addr: SocketAddr = listen.parse().map_err(|_| format!("--listen {listen}: not an address like 0.0.0.0:{}", crate::cloudvault::PORT))?;
            let data = opt(a, "--data").ok_or("--data DIR is needed (the volume the vault keeps its files on)")?;
            let tls = match (opt(a, "--tls-cert"), opt(a, "--tls-key")) {
                (Some(c), Some(k)) => Some((PathBuf::from(c), PathBuf::from(k))),
                (None, None) => None,
                _ => return Err("--tls-cert and --tls-key go together".into()),
            };
            let run = start(addr, Path::new(&data), tls.as_ref().map(|(c, k)| (c.as_path(), k.as_path())))?;
            out(&format!("vault service on https://{} (certificate SHA-256 {})", run.local_addr(), run.fingerprint));
            // a service: it runs until it is stopped
            loop {
                std::thread::park();
            }
        }
        Some("keygen") => {
            let file = opt(a, "--out").ok_or("--out FILE is needed (the member's secret is kept there, mode 0600)")?;
            let k = MemberKeys::generate()?;
            {
                use std::os::unix::fs::OpenOptionsExt;
                let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&file).map_err(|e| format!("{file}: {e}"))?;
                f.write_all(k.seed_hex().as_bytes()).map_err(|e| format!("{file}: {e}"))?;
            }
            out(&serde_json::to_string(&k.public()).map_err(|e| e.to_string())?);
            Ok(())
        }
        Some("enroll") => {
            let data = opt(a, "--data").ok_or("--data DIR is needed")?;
            let id = opt(a, "--id").ok_or("--id TENANT/PERSON or operator:NAME is needed")?;
            let who = Principal::parse(&id).ok_or("the id is TENANT/PERSON or operator:NAME")?;
            let keys: PublicKeys = serde_json::from_str(&opt(a, "--keys").ok_or("--keys '{\"wrap\":..,\"sign\":..}' is needed (iyakki vault keygen prints it)")?).map_err(|e| format!("--keys: {e}"))?;
            Store::open(Path::new(&data))?.enroll(&who, &keys)?;
            out(&format!("{id} is enrolled"));
            Ok(())
        }
        Some("enroll-member") => {
            // the tenant's owner or admin, from their own machine: signs with their secret
            let usage = "usage: iyakki vault enroll-member --vault HOST:PORT [--fingerprint SHA256] --id TENANT/ADMIN --key FILE --name PERSON --keys JSON";
            let (hp, id, kf, name) = (opt(a, "--vault"), opt(a, "--id"), opt(a, "--key"), opt(a, "--name"));
            let (Some(hp), Some(id), Some(kf), Some(name)) = (hp, id, kf, name) else { return Err(usage.into()) };
            let (host, port) = hp.rsplit_once(':').and_then(|(h, p)| p.parse::<u16>().ok().map(|p| (h.to_string(), p))).ok_or("--vault is HOST:PORT")?;
            let who = Principal::parse(&id).filter(|w| matches!(w, Principal::Member { .. })).ok_or("--id is TENANT/PERSON")?;
            let mine = MemberKeys::from_hex(&std::fs::read_to_string(&kf).map_err(|e| format!("{kf}: {e}"))?)?;
            let keys: PublicKeys = serde_json::from_str(&opt(a, "--keys").ok_or(usage)?).map_err(|e| format!("--keys: {e}"))?;
            Client { host, port, fingerprint: opt(a, "--fingerprint"), who, keys: mine }.enroll_member(&name, &keys)?;
            out(&format!("{name} is enrolled"));
            Ok(())
        }
        _ => Err("usage: iyakki vault serve --listen ADDR --data DIR [--tls-cert F --tls-key F] | keygen --out FILE | enroll --data DIR --id ID --keys JSON | enroll-member --vault HOST:PORT --id TENANT/ADMIN --key FILE --name PERSON --keys JSON".into()),
    }
}

// ---- tests -------------------------------------------------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloudroles::{NsRole, TenantRole};

    struct Dir(PathBuf);
    impl Dir {
        fn new() -> Dir {
            let d = std::env::temp_dir().join(format!("iyakki-vaultd-{}", cloudkeys::hex(&cloudkeys::random::<8>().unwrap())));
            std::fs::create_dir_all(&d).unwrap();
            Dir(d)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn keys(n: u8) -> MemberKeys {
        MemberKeys::from_seed([n; 32])
    }

    fn member(t: &str, u: &str) -> Principal {
        Principal::Member { tenant: t.into(), user: u.into() }
    }

    /// Tenant acme: alice owner (and admin of namespace dev), bob member (editor in dev), carol member (in no namespace), dave admin (not in dev).
    fn acme(d: &Dir) -> Store {
        let s = Store::open(&d.0).unwrap();
        let mut r = Roles::new("alice").unwrap();
        r.add_tenant_member("alice", "bob", TenantRole::Member).unwrap();
        r.add_tenant_member("alice", "carol", TenantRole::Member).unwrap();
        r.add_tenant_member("alice", "dave", TenantRole::Admin).unwrap();
        r.create_namespace("alice", "dev").unwrap();
        r.add_namespace_member("alice", "dev", "bob", NsRole::Editor).unwrap();
        cloudroles::save(&cloudroles::path(&s.tenant_dir("acme").unwrap()), &r).unwrap();
        for (n, u) in [(1u8, "alice"), (2, "bob"), (3, "carol"), (4, "dave")] {
            s.enroll(&member("acme", u), &keys(n).public()).unwrap();
        }
        s.enroll(&Principal::Operator("ops".into()), &keys(9).public()).unwrap();
        s
    }

    fn wrap_for(scope: &ScopeKey, s: &Scope, who: &str, g: u64, k: &MemberKeys) -> Wrapped {
        scope.wrap(&s.name(), who, g, &k.public()).unwrap()
    }

    #[test]
    fn a_namespace_vault_works_for_its_members_and_nobody_else_even_a_tenant_admin() {
        let d = Dir::new();
        let s = acme(&d);
        let ns = Scope::Namespace("dev".into());
        let (alice, bob, carol, dave) = (member("acme", "alice"), member("acme", "bob"), member("acme", "carol"), member("acme", "dave"));
        let key = ScopeKey::generate().unwrap();
        s.create_scope(&alice, &ns, &wrap_for(&key, &ns, "alice", 1, &keys(1))).unwrap();
        let sealed = key.seal("namespace:dev", "token", 1, b"hunter2").unwrap();
        s.put(&alice, &ns, "token", 1, &sealed).unwrap();
        // bob is in the namespace by the roles but has no wrap yet
        assert_eq!(s.get(&bob, &ns, "token"), Err(VaultError::Denied));
        s.add_member(&alice, &ns, "bob", &wrap_for(&key, &ns, "bob", 1, &keys(2))).unwrap();
        let (g, w) = s.my_wrap(&bob, &ns).unwrap();
        let bk = keys(2).unwrap_key("namespace:dev", "bob", g, &w).unwrap();
        let (g2, v) = s.get(&bob, &ns, "token").unwrap();
        assert_eq!(bk.open("namespace:dev", "token", g2, &v).unwrap(), b"hunter2");
        // carol is in the tenant, not in the namespace; dave administers the tenant, not the namespace
        for who in [&carol, &dave] {
            assert_eq!(s.get(who, &ns, "token"), Err(VaultError::Denied));
            assert_eq!(s.list(who, &ns), Err(VaultError::Denied));
            assert_eq!(s.my_wrap(who, &ns), Err(VaultError::Denied));
            assert_eq!(s.put(who, &ns, "x", 1, &sealed), Err(VaultError::Denied));
        }
        // a wrap made for them is still refused while the roles do not have them in the namespace
        assert_eq!(s.add_member(&alice, &ns, "dave", &wrap_for(&key, &ns, "dave", 1, &keys(4))), Err(VaultError::Denied));
        assert_eq!(s.add_member(&alice, &ns, "carol", &wrap_for(&key, &ns, "carol", 1, &keys(3))), Err(VaultError::Denied));
        // bob is an editor, not the namespace's administrator: he cannot add people or rotate
        assert_eq!(s.add_member(&bob, &ns, "alice", &wrap_for(&key, &ns, "alice", 1, &keys(1))), Err(VaultError::Denied));
        assert_eq!(s.members(&bob, &ns), Err(VaultError::Denied));
        // a person of another tenant, or of none
        assert_eq!(s.get(&member("other", "eve"), &ns, "token"), Err(VaultError::Denied));
        assert_eq!(s.get(&member("acme", "mallory"), &ns, "token"), Err(VaultError::Denied));
        assert_eq!(s.get(&member("../x", "eve"), &ns, "token"), Err(VaultError::Denied));
        // an operator has nothing in a tenant's scopes
        assert_eq!(s.get(&Principal::Operator("ops".into()), &ns, "token"), Err(VaultError::Denied));
        assert_eq!(s.get(&Principal::Operator("ops".into()), &Scope::Tenant, "token"), Err(VaultError::Denied));
    }

    #[test]
    fn a_viewer_reads_but_does_not_write() {
        let d = Dir::new();
        let s = acme(&d);
        let ns = Scope::Namespace("dev".into());
        let mut r = cloudroles::load(&cloudroles::path(&s.tenant_dir("acme").unwrap())).unwrap().unwrap();
        r.add_namespace_member("alice", "dev", "bob", NsRole::Viewer).unwrap();
        cloudroles::save(&cloudroles::path(&s.tenant_dir("acme").unwrap()), &r).unwrap();
        let key = ScopeKey::generate().unwrap();
        let (alice, bob) = (member("acme", "alice"), member("acme", "bob"));
        s.create_scope(&alice, &ns, &wrap_for(&key, &ns, "alice", 1, &keys(1))).unwrap();
        s.add_member(&alice, &ns, "bob", &wrap_for(&key, &ns, "bob", 1, &keys(2))).unwrap();
        let sealed = key.seal("namespace:dev", "a", 1, b"v").unwrap();
        assert_eq!(s.put(&bob, &ns, "a", 1, &sealed), Err(VaultError::Denied));
        s.put(&alice, &ns, "a", 1, &sealed).unwrap();
        assert!(s.get(&bob, &ns, "a").is_ok());
        assert_eq!(s.delete(&bob, &ns, "a"), Err(VaultError::Denied));
    }

    #[test]
    fn removing_a_member_rotates_the_key_and_cuts_them_off() {
        let d = Dir::new();
        let s = acme(&d);
        let ns = Scope::Namespace("dev".into());
        let (alice, bob) = (member("acme", "alice"), member("acme", "bob"));
        let k1 = ScopeKey::generate().unwrap();
        s.create_scope(&alice, &ns, &wrap_for(&k1, &ns, "alice", 1, &keys(1))).unwrap();
        s.put(&alice, &ns, "old", 1, &k1.seal("namespace:dev", "old", 1, b"before").unwrap()).unwrap();
        s.add_member(&alice, &ns, "bob", &wrap_for(&k1, &ns, "bob", 1, &keys(2))).unwrap();
        // what bob holds before he is removed
        let (_, bw) = s.my_wrap(&bob, &ns).unwrap();
        let bob_old_key = keys(2).unwrap_key("namespace:dev", "bob", 1, &bw).unwrap();
        // the roles drop him, and alice rotates
        let k2 = ScopeKey::generate().unwrap();
        let mut wraps = BTreeMap::new();
        wraps.insert("alice".to_string(), wrap_for(&k2, &ns, "alice", 2, &keys(1)));
        let mut values = BTreeMap::new();
        values.insert("old".to_string(), k2.seal("namespace:dev", "old", 2, b"before").unwrap());
        // refused: a wrap left for the removed person, a value missing, the wrong generation, removing oneself
        let mut with_bob = wraps.clone();
        with_bob.insert("bob".into(), wrap_for(&k2, &ns, "bob", 2, &keys(2)));
        assert!(matches!(s.rotate(&alice, &ns, Some("bob"), 2, &with_bob, &values), Err(VaultError::Invalid(_))));
        assert!(matches!(s.rotate(&alice, &ns, Some("bob"), 2, &wraps, &BTreeMap::new()), Err(VaultError::Invalid(_))));
        assert!(matches!(s.rotate(&alice, &ns, Some("bob"), 3, &wraps, &values), Err(VaultError::Conflict(_))));
        assert!(matches!(s.rotate(&alice, &ns, Some("alice"), 2, &wraps, &values), Err(VaultError::Invalid(_))));
        assert_eq!(s.rotate(&bob, &ns, Some("alice"), 2, &wraps, &values), Err(VaultError::Denied));
        s.rotate(&alice, &ns, Some("bob"), 2, &wraps, &values).unwrap();
        // a value written after the removal
        s.put(&alice, &ns, "new", 2, &k2.seal("namespace:dev", "new", 2, b"after").unwrap()).unwrap();
        // bob: the service refuses him even if the roles still had him (the wrap is gone), and with the roles changed too
        assert_eq!(s.get(&bob, &ns, "new"), Err(VaultError::Denied));
        assert_eq!(s.my_wrap(&bob, &ns), Err(VaultError::Denied));
        // what he could have copied: the new ciphertext on disk does not open with the key he held
        let on_disk: ScopeData = read_json(&s.scope_path(&alice, &ns).unwrap()).unwrap().unwrap();
        assert_eq!(on_disk.generation, 2);
        assert!(!on_disk.wraps.contains_key("bob") && on_disk.wraps.len() == 1, "the old wrap is destroyed");
        for (n, v) in &on_disk.secrets {
            assert!(bob_old_key.open("namespace:dev", n, 2, v).is_err(), "{n} must not open with the old key");
            assert!(bob_old_key.open("namespace:dev", n, 1, v).is_err());
        }
        // and the old wrap does not open for generation 2 either
        assert!(keys(2).unwrap_key("namespace:dev", "bob", 2, &bw).is_err());
        // alice still reads everything, with the new key
        let (g, w) = s.my_wrap(&alice, &ns).unwrap();
        let ak = keys(1).unwrap_key("namespace:dev", "alice", g, &w).unwrap();
        let (_, v) = s.get(&alice, &ns, "old").unwrap();
        assert_eq!(ak.open("namespace:dev", "old", g, &v).unwrap(), b"before");
        // a writer with the old key is told to fetch the new wrap
        assert!(matches!(s.put(&alice, &ns, "late", 1, &k1.seal("namespace:dev", "late", 1, b"x").unwrap()), Err(VaultError::Conflict(_))));
    }

    #[test]
    fn the_common_vault_is_written_by_an_operator_and_only_read_by_tenants() {
        let d = Dir::new();
        let s = acme(&d);
        let (ops, bob, alice) = (Principal::Operator("ops".into()), member("acme", "bob"), member("acme", "alice"));
        let c = Scope::Common;
        let key = ScopeKey::generate().unwrap();
        // tenants cannot make it, write it, manage it
        assert_eq!(s.create_scope(&alice, &c, &wrap_for(&key, &c, "acme/alice", 1, &keys(1))), Err(VaultError::Denied));
        s.create_scope(&ops, &c, &wrap_for(&key, &c, "operator:ops", 1, &keys(9))).unwrap();
        s.put(&ops, &c, "license", 1, &key.seal("common", "license", 1, b"abc").unwrap()).unwrap();
        assert_eq!(s.get(&bob, &c, "license"), Err(VaultError::Denied), "no wrap yet");
        // the operator wraps it for a tenant member who is in a tenant that has roles
        s.add_member(&ops, &c, "acme/bob", &wrap_for(&key, &c, "acme/bob", 1, &keys(2))).unwrap();
        assert_eq!(s.add_member(&ops, &c, "nobody/eve", &wrap_for(&key, &c, "nobody/eve", 1, &keys(5))), Err(VaultError::Denied));
        assert_eq!(s.add_member(&ops, &c, "acme/mallory", &wrap_for(&key, &c, "acme/mallory", 1, &keys(5))), Err(VaultError::Denied));
        let (g, w) = s.my_wrap(&bob, &c).unwrap();
        let bk = keys(2).unwrap_key("common", "acme/bob", g, &w).unwrap();
        let (_, v) = s.get(&bob, &c, "license").unwrap();
        assert_eq!(bk.open("common", "license", g, &v).unwrap(), b"abc");
        let sealed = bk.seal("common", "mine", g, b"x").unwrap();
        assert_eq!(s.put(&bob, &c, "mine", g, &sealed), Err(VaultError::Denied));
        assert_eq!(s.delete(&bob, &c, "license"), Err(VaultError::Denied));
        assert_eq!(s.add_member(&bob, &c, "acme/alice", &wrap_for(&bk, &c, "acme/alice", g, &keys(1))), Err(VaultError::Denied));
        assert_eq!(s.rotate(&bob, &c, None, 2, &BTreeMap::new(), &BTreeMap::new()), Err(VaultError::Denied));
        assert_eq!(s.members(&bob, &c), Err(VaultError::Denied));
        // a person removed from the tenant loses it at once, before any rotation
        let mut r = cloudroles::load(&cloudroles::path(&s.tenant_dir("acme").unwrap())).unwrap().unwrap();
        r.remove_tenant_member("alice", "bob").unwrap();
        cloudroles::save(&cloudroles::path(&s.tenant_dir("acme").unwrap()), &r).unwrap();
        assert_eq!(s.get(&bob, &c, "license"), Err(VaultError::Denied));
    }

    #[test]
    fn the_tenant_vault_is_written_by_admins_and_read_by_members_who_were_given_the_key() {
        let d = Dir::new();
        let s = acme(&d);
        let (alice, bob, carol) = (member("acme", "alice"), member("acme", "bob"), member("acme", "carol"));
        let key = ScopeKey::generate().unwrap();
        s.create_scope(&bob, &Scope::Tenant, &wrap_for(&key, &Scope::Tenant, "bob", 1, &keys(2))).unwrap_err();
        s.create_scope(&alice, &Scope::Tenant, &wrap_for(&key, &Scope::Tenant, "alice", 1, &keys(1))).unwrap();
        assert!(matches!(s.create_scope(&alice, &Scope::Tenant, &wrap_for(&key, &Scope::Tenant, "alice", 1, &keys(1))), Err(VaultError::Conflict(_))));
        s.add_member(&alice, &Scope::Tenant, "bob", &wrap_for(&key, &Scope::Tenant, "bob", 1, &keys(2))).unwrap();
        s.put(&alice, &Scope::Tenant, "a", 1, &key.seal("tenant", "a", 1, b"1").unwrap()).unwrap();
        assert!(s.get(&bob, &Scope::Tenant, "a").is_ok());
        assert_eq!(s.put(&bob, &Scope::Tenant, "b", 1, &key.seal("tenant", "b", 1, b"1").unwrap()), Err(VaultError::Denied));
        assert_eq!(s.get(&carol, &Scope::Tenant, "a"), Err(VaultError::Denied), "no wrap");
        assert_eq!(s.get(&alice, &Scope::Tenant, "missing"), Err(VaultError::NotFound));
        // names that are not names
        for bad in ["", "../x", ".hidden", "a b", &"x".repeat(129)] {
            assert!(matches!(s.put(&alice, &Scope::Tenant, bad, 1, &key.seal("tenant", "a", 1, b"1").unwrap()), Err(VaultError::Invalid(_))), "{bad:?}");
        }
        assert!(matches!(s.put(&alice, &Scope::Tenant, "a", 1, "not sealed"), Err(VaultError::Invalid(_))));
    }

    #[test]
    fn files_are_private_atomic_and_hold_no_plaintext() {
        use std::os::unix::fs::PermissionsExt;
        let d = Dir::new();
        let s = acme(&d);
        let ns = Scope::Namespace("dev".into());
        let alice = member("acme", "alice");
        let key = ScopeKey::generate().unwrap();
        s.create_scope(&alice, &ns, &wrap_for(&key, &ns, "alice", 1, &keys(1))).unwrap();
        s.put(&alice, &ns, "pw", 1, &key.seal("namespace:dev", "pw", 1, b"correct horse battery").unwrap()).unwrap();
        let p = s.scope_path(&alice, &ns).unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(!text.contains("correct horse") && !text.contains(&cloudkeys::hex(b"correct horse")));
        assert!(!p.with_extension("json.tmp").exists());
        // the service holds no secret key: nothing in the data directory contains a member's secret
        fn walk(d: &Path, out: &mut String) {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                if e.path().is_dir() {
                    walk(&e.path(), out);
                } else {
                    out.push_str(&std::fs::read_to_string(e.path()).unwrap_or_default());
                }
            }
        }
        let mut all = String::new();
        walk(&d.0, &mut all);
        for n in 1..=4u8 {
            assert!(!all.contains(&keys(n).seed_hex()));
        }
    }

    #[test]
    fn a_request_must_be_signed_by_an_enrolled_key_and_be_recent() {
        let d = Dir::new();
        let s = acme(&d);
        let sign = |k: &MemberKeys, id: &str, ts: u64| k.sign_request(id, "GET", "/v1/scope/tenant/secrets", ts, b"");
        let now = 1_000_000;
        assert_eq!(s.authenticate("GET", "/v1/scope/tenant/secrets", b"", Some(&sign(&keys(1), "acme/alice", now)), now), Ok(member("acme", "alice")));
        assert_eq!(s.authenticate("GET", "/v1/scope/tenant/secrets", b"", Some(&sign(&keys(1), "operator:ops", now)), now), Err(VaultError::Unauthenticated), "alice's key is not the operator's");
        assert_eq!(s.authenticate("GET", "/v1/scope/tenant/secrets", b"", Some(&sign(&keys(2), "acme/alice", now)), now), Err(VaultError::Unauthenticated), "bob's key claiming to be alice");
        assert_eq!(s.authenticate("GET", "/v1/scope/tenant/secrets", b"", Some(&sign(&keys(1), "acme/alice", now - 500)), now), Err(VaultError::Unauthenticated), "too old");
        assert_eq!(s.authenticate("GET", "/v1/scope/tenant/secrets", b"", Some(&sign(&keys(7), "acme/zed", now)), now), Err(VaultError::Unauthenticated), "not enrolled");
        assert_eq!(s.authenticate("GET", "/v1/scope/tenant/secrets", b"", None, now), Err(VaultError::Unauthenticated));
        assert_eq!(s.authenticate("DELETE", "/v1/scope/tenant/secrets", b"", Some(&sign(&keys(1), "acme/alice", now)), now), Err(VaultError::Unauthenticated), "another method");
    }

    #[test]
    fn the_public_keys_of_a_person_are_given_only_to_people_in_the_same_tenant() {
        let d = Dir::new();
        let s = acme(&d);
        let (bob, ops) = (member("acme", "bob"), Principal::Operator("ops".into()));
        assert_eq!(s.lookup(&bob, "carol"), Ok(keys(3).public()));
        assert_eq!(s.lookup(&bob, "mallory"), Err(VaultError::Denied));
        assert_eq!(s.lookup(&bob, "acme/carol"), Err(VaultError::Denied));
        assert_eq!(s.lookup(&ops, "acme/carol"), Ok(keys(3).public()));
        assert_eq!(s.lookup(&ops, "carol"), Err(VaultError::Denied));
        assert_eq!(s.lookup(&member("other", "eve"), "carol"), Err(VaultError::Denied));
    }

    /// The whole thing over a real socket: TLS, signed requests, the member's side doing the cryptography.
    #[test]
    fn a_loopback_service_serves_two_members_and_a_removal_cuts_one_off() {
        let d = Dir::new();
        let s = acme(&d);
        let run = start("127.0.0.1:0".parse().unwrap(), &d.0, None).unwrap();
        drop(s);
        let port = run.local_addr().port();
        let client = |n: u8, u: &str| Client { host: "127.0.0.1".into(), port, fingerprint: Some(run.fingerprint.clone()), who: member("acme", u), keys: keys(n) };
        let (alice, bob, carol) = (client(1, "alice"), client(2, "bob"), client(3, "carol"));
        let ns = Scope::Namespace("dev".into());
        alice.create_scope(&ns).unwrap();
        alice.put(&ns, "api_key", b"abc123").unwrap();
        assert!(bob.get(&ns, "api_key").is_err(), "not given the key yet");
        alice.add_member(&ns, "bob").unwrap();
        assert_eq!(bob.get(&ns, "api_key").unwrap(), b"abc123");
        bob.put(&ns, "from_bob", b"hello").unwrap();
        assert_eq!(alice.get(&ns, "from_bob").unwrap(), b"hello");
        assert_eq!(alice.list(&ns).unwrap(), vec!["api_key".to_string(), "from_bob".to_string()]);
        assert!(carol.get(&ns, "api_key").unwrap_err().starts_with("403"));
        // a wrong key cannot sign in; a pinned fingerprint that is wrong is refused
        let forged = Client { keys: keys(77), ..client(1, "alice") };
        assert!(forged.list(&ns).unwrap_err().starts_with("401"));
        let wrong_pin = Client { fingerprint: Some("00:11".into()), ..client(1, "alice") };
        assert!(wrong_pin.list(&ns).is_err());
        // remove bob: the roles first (the tenant's own step), then the rotation
        let tdir = d.0.join("tenants").join("acme");
        let mut r = cloudroles::load(&cloudroles::path(&tdir)).unwrap().unwrap();
        r.remove_namespace_member("alice", "dev", "bob").unwrap();
        cloudroles::save(&cloudroles::path(&tdir), &r).unwrap();
        alice.remove_member(&ns, "bob").unwrap();
        alice.put(&ns, "after", b"secret after removal").unwrap();
        assert_eq!(alice.get(&ns, "api_key").unwrap(), b"abc123", "old values survive the rotation");
        assert_eq!(alice.get(&ns, "after").unwrap(), b"secret after removal");
        assert!(bob.get(&ns, "after").unwrap_err().starts_with("403"));
        assert!(bob.get(&ns, "api_key").unwrap_err().starts_with("403"));
        alice.delete(&ns, "from_bob").unwrap();
        assert_eq!(alice.list(&ns).unwrap().len(), 2);
        // an unknown route and a bad scope name are plain 404s, after sign-in
        assert!(alice.call("GET", "/v1/nothing", None).unwrap_err().starts_with("404"));
        assert!(alice.call("GET", "/v1/scope/namespace:../secrets", None).unwrap_err().starts_with("404"));
        run.stop();
    }

    /// acme plus: erin (member, no key on the service yet), owen (a second owner, no key), frank (member of ANOTHER tenant's roles only).
    fn acme_with_unenrolled(d: &Dir) -> Store {
        let s = acme(d);
        let tdir = s.tenant_dir("acme").unwrap();
        let mut r = cloudroles::load(&cloudroles::path(&tdir)).unwrap().unwrap();
        r.add_tenant_member("alice", "erin", TenantRole::Member).unwrap();
        r.add_tenant_member("alice", "owen", TenantRole::Owner).unwrap();
        cloudroles::save(&cloudroles::path(&tdir), &r).unwrap();
        s
    }

    #[test]
    fn a_tenant_admin_enrols_members_of_their_own_tenant_and_nobody_else() {
        let d = Dir::new();
        let s = acme_with_unenrolled(&d);
        let (alice, bob, dave) = (member("acme", "alice"), member("acme", "bob"), member("acme", "dave"));
        let new = keys(50).public();
        // an admin (dave) enrols a member; the owner too; the same keys again change nothing
        s.enroll_member(&dave, "erin", &new).unwrap();
        assert_eq!(s.keys_of(&member("acme", "erin")).unwrap(), Some(new.clone()));
        s.enroll_member(&alice, "erin", &new).unwrap();
        // denied: a plain member, an operator, an unknown caller's tenant, a person who is not in the tenant, a bad name
        assert_eq!(s.enroll_member(&bob, "erin", &keys(51).public()), Err(VaultError::Denied));
        assert_eq!(s.enroll_member(&Principal::Operator("ops".into()), "erin", &new), Err(VaultError::Denied));
        assert_eq!(s.enroll_member(&member("other", "alice"), "erin", &new), Err(VaultError::Denied));
        assert_eq!(s.enroll_member(&dave, "mallory", &keys(52).public()), Err(VaultError::Denied));
        assert_eq!(s.enroll_member(&dave, "../acme", &keys(52).public()), Err(VaultError::Denied));
        assert!(s.keys_of(&member("acme", "mallory")).unwrap().is_none());
        // an admin cannot enrol someone above them (owen is an owner)
        assert_eq!(s.enroll_member(&dave, "owen", &keys(53).public()), Err(VaultError::Denied));
        assert!(s.keys_of(&member("acme", "owen")).unwrap().is_none());
        s.enroll_member(&alice, "owen", &keys(53).public()).unwrap();
        // never over a different key: not for the admin, not for the owner, not for a person's own entry
        assert!(matches!(s.enroll_member(&dave, "erin", &keys(54).public()), Err(VaultError::Conflict(_))));
        assert!(matches!(s.enroll_member(&alice, "bob", &keys(54).public()), Err(VaultError::Conflict(_))));
        assert_eq!(s.keys_of(&member("acme", "bob")).unwrap(), Some(keys(2).public()));
        // keys that are not keys
        let bad = PublicKeys { wrap: "zz".into(), sign: "zz".into() };
        assert!(matches!(s.enroll_member(&alice, "carol", &bad), Err(VaultError::Invalid(_))));
        // a person removed from the roles cannot be enrolled any more, and a removed admin enrols nobody
        let tdir = s.tenant_dir("acme").unwrap();
        let mut r = cloudroles::load(&cloudroles::path(&tdir)).unwrap().unwrap();
        r.remove_tenant_member("alice", "dave").unwrap();
        r.remove_tenant_member("alice", "erin").unwrap();
        cloudroles::save(&cloudroles::path(&tdir), &r).unwrap();
        assert_eq!(s.enroll_member(&dave, "carol", &keys(55).public()), Err(VaultError::Denied));
        assert_eq!(s.enroll_member(&alice, "erin", &keys(56).public()), Err(VaultError::Denied));
    }

    #[test]
    fn the_enrol_route_is_signed_and_the_command_uses_it() {
        let d = Dir::new();
        let s = acme_with_unenrolled(&d);
        drop(s);
        let run = start("127.0.0.1:0".parse().unwrap(), &d.0, None).unwrap();
        let port = run.local_addr().port();
        let client = |n: u8, u: &str| Client { host: "127.0.0.1".into(), port, fingerprint: Some(run.fingerprint.clone()), who: member("acme", u), keys: keys(n) };
        // a member without the role is refused by the route; an admin is not
        let e = client(2, "bob").enroll_member("erin", &keys(60).public()).unwrap_err();
        assert!(e.starts_with("403"), "{e}");
        // not signed by an enrolled key: 401 before anything is looked at
        let e = Client { keys: keys(99), ..client(4, "dave") }.enroll_member("erin", &keys(60).public()).unwrap_err();
        assert!(e.starts_with("401"), "{e}");
        client(4, "dave").enroll_member("erin", &keys(60).public()).unwrap();
        // the new member can now sign in and see its own tenant scope (here: reach a 404, not a 401)
        let e = client(60, "erin").scope_key(&Scope::Tenant).err().unwrap();
        assert!(e.starts_with("404"), "{e}");
        // the same through the command, with the admin's secret in a file
        let f = d.0.join("dave.key");
        std::fs::write(&f, keys(4).seed_hex()).unwrap();
        let said = std::cell::RefCell::new(Vec::<String>::new());
        let mut out = |s: &str| said.borrow_mut().push(s.to_string());
        let a = ["enroll-member", "--vault", &format!("127.0.0.1:{port}"), "--fingerprint", &run.fingerprint, "--id", "acme/dave", "--key", &f.display().to_string(), "--name", "owen"].map(String::from);
        let mut with_keys = a.to_vec();
        with_keys.extend(["--keys".to_string(), serde_json::to_string(&keys(61).public()).unwrap()]);
        let e = super::run(&with_keys, &mut out).unwrap_err();
        assert!(e.starts_with("403"), "an admin cannot enrol an owner: {e}");
        let mut ok = with_keys.clone();
        ok[10] = "erin".into();
        let e = super::run(&ok, &mut out).unwrap_err();
        assert!(e.starts_with("409"), "erin already has a key: {e}");
        assert!(super::run(&a, &mut out).unwrap_err().contains("usage"));
        run.stop();
    }

    #[test]
    fn the_enrol_route_is_covered_by_the_nonce_and_the_rate_limits() {
        let d = Dir::new();
        let s = acme_with_unenrolled(&d);
        let body = serde_json::to_vec(&json!({"name": "erin", "keys": keys(60).public()})).unwrap();
        let req = |ts: u64| Request {
            method: "POST".into(),
            path: "/v1/enroll".into(),
            query: Default::default(),
            headers: [("authorization".to_string(), keys(4).sign_request("acme/dave", "POST", "/v1/enroll", ts, &body))].into(),
            body: body.clone(),
        };
        let first = req(NOW);
        assert_eq!(s.handle(&first, NOW).status, 200);
        // the captured request, sent again, is a replay: refused before the route looks at it (the same keys would be a no-op anyway)
        assert_eq!(s.handle(&first, NOW + 1).status, 401, "replayed enrol request");
        // a request signed too long ago is refused too
        assert_eq!(s.handle(&req(NOW - 500), NOW).status, 401, "old enrol request");
        // a flood from one address is slowed before the signature is even checked
        s.set_limits(Limits { source_burst: 3.0, source_per_sec: 0.0, ..Limits::default() });
        let ip: IpAddr = "192.0.2.9".parse().unwrap();
        let codes: Vec<u16> = (0..6).map(|i| s.handle_from(&req(NOW + 10 + i), NOW + 10, Some(ip)).status).collect();
        assert!(codes.contains(&429), "{codes:?}");
    }

    #[test]
    fn the_listener_reads_certificates_from_a_live_loopback_vault_through_the_adapter() {
        use crate::cloudlisten::vault::{Reader, SecretReader, VaultCerts};
        use crate::cloudlisten::CertSource;
        use crate::cloudroute::CertRef;
        let d = Dir::new();
        drop(acme_at(&d, unix_now()));
        let run = start("127.0.0.1:0".parse().unwrap(), &d.0, None).unwrap();
        let port = run.local_addr().port();
        let client = |n: u8, u: &str| Client { host: "127.0.0.1".into(), port, fingerprint: Some(run.fingerprint.clone()), who: member("acme", u), keys: keys(n) };
        let alice = client(1, "alice");
        alice.create_scope(&Scope::Tenant).unwrap();
        let (chain, key) = crate::cloudroute::self_signed("app.example.test").unwrap();
        alice.put(&Scope::Tenant, "appcert-chain", chain.as_bytes()).unwrap();
        alice.put(&Scope::Tenant, "appcert-key", key.as_bytes()).unwrap();
        alice.put(&Scope::Tenant, "clients-ca", chain.as_bytes()).unwrap();
        let certs = VaultCerts::new(Box::new(Reader { client: client(1, "alice"), scope: Scope::Tenant }));
        // the secrets come back sealed on the way in and opened on this side: the listener gets the PEM it was given
        assert_eq!(certs.load("app.example.test", &CertRef::Vault("appcert".into())).unwrap(), (chain.clone(), key.clone()));
        assert_eq!(certs.load_client_ca("clients-ca").unwrap(), chain);
        // a rotation in the vault reaches the next load
        let (chain2, key2) = crate::cloudroute::self_signed("app.example.test").unwrap();
        alice.put(&Scope::Tenant, "appcert-chain", chain2.as_bytes()).unwrap();
        alice.put(&Scope::Tenant, "appcert-key", key2.as_bytes()).unwrap();
        assert_eq!(certs.load("app.example.test", &CertRef::Vault("appcert".into())).unwrap(), (chain2, key2));
        // a missing secret, a name that is not a name, and a member without the scope key are errors that hold no secret text
        let e = certs.load("app.example.test", &CertRef::Vault("nothere".into())).unwrap_err();
        assert!(e.contains("nothere-chain") && e.contains("404"), "{e}");
        assert!(certs.load("app.example.test", &CertRef::Vault("../x".into())).is_err());
        let carol = Reader { client: client(3, "carol"), scope: Scope::Tenant };
        let e = carol.read("appcert-key").unwrap_err();
        assert!(!e.contains("BEGIN"), "{e}");
        // a vault that has gone away is an error, not a hang or a panic
        run.stop();
        assert!(certs.load("app.example.test", &CertRef::Vault("appcert".into())).is_err());
    }

    #[test]
    fn the_command_refuses_what_it_cannot_serve() {
        let said = std::cell::RefCell::new(Vec::<String>::new());
        let mut out = |s: &str| said.borrow_mut().push(s.to_string());
        assert!(run(&["serve".into()], &mut out).unwrap_err().contains("--data"));
        assert!(run(&["serve".into(), "--data".into(), "x".into(), "--tls-cert".into(), "c".into()], &mut out).unwrap_err().contains("together"));
        assert!(run(&["serve".into(), "--data".into(), "x".into(), "--listen".into(), "nope".into()], &mut out).is_err());
        assert!(run(&[], &mut out).unwrap_err().contains("usage"));
        let d = Dir::new();
        let f = d.0.join("me.key");
        run(&["keygen".into(), "--out".into(), f.display().to_string()], &mut out).unwrap();
        let keys = MemberKeys::from_hex(&std::fs::read_to_string(&f).unwrap()).unwrap();
        let first = said.borrow()[0].clone();
        assert!(first.contains(&keys.public().wrap), "{first}");
        assert!(run(&["keygen".into(), "--out".into(), f.display().to_string()], &mut out).is_err(), "never overwrites a secret");
        let a = ["enroll", "--data", &d.0.display().to_string(), "--id", "acme/zed", "--keys", &first].map(String::from);
        run(&a, &mut out).unwrap();
        assert!(Store::open(&d.0).unwrap().keys_of(&member("acme", "zed")).unwrap().is_some());
    }

    // ---- replay, clock and rate limits -------------------------------------------------------------------------------------------------

    const NOW: u64 = 1_000_000;
    const PATH: &str = "/v1/scope/tenant/secrets";

    fn acme_at(d: &Dir, now: u64) -> Store {
        let s = Store::open_at(&d.0, now).unwrap();
        let mut r = Roles::new("alice").unwrap();
        r.add_tenant_member("alice", "bob", TenantRole::Member).unwrap();
        cloudroles::save(&cloudroles::path(&s.tenant_dir("acme").unwrap()), &r).unwrap();
        s.enroll(&member("acme", "alice"), &keys(1).public()).unwrap();
        s.enroll(&member("acme", "bob"), &keys(2).public()).unwrap();
        s
    }

    fn signed(k: &MemberKeys, id: &str, ts: u64) -> String {
        k.sign_request(id, "GET", PATH, ts, b"")
    }

    fn auth(s: &Store, h: &str, now: u64) -> R<Principal> {
        s.authenticate("GET", PATH, b"", Some(h), now)
    }

    fn ip(n: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, n])
    }

    #[test]
    fn a_copy_of_a_signed_request_is_refused_and_a_new_signature_is_not() {
        let d = Dir::new();
        let s = acme_at(&d, NOW);
        let h = signed(&keys(1), "acme/alice", NOW);
        assert_eq!(auth(&s, &h, NOW), Ok(member("acme", "alice")));
        assert_eq!(auth(&s, &h, NOW), Err(VaultError::Unauthenticated), "the same bytes again");
        assert_eq!(auth(&s, &h, NOW + 30), Err(VaultError::Unauthenticated), "still inside its window");
        assert_eq!(auth(&s, &signed(&keys(1), "acme/alice", NOW), NOW), Ok(member("acme", "alice")), "the same call, signed again");
        // the same nonce under another id is another request; under the same id it is a replay even for another path
        let n = "ab".repeat(16);
        let a = keys(1).sign_request_with_nonce("acme/alice", "GET", PATH, NOW, b"", &n);
        let b = keys(2).sign_request_with_nonce("acme/bob", "GET", PATH, NOW, b"", &n);
        assert!(auth(&s, &a, NOW).is_ok() && auth(&s, &b, NOW).is_ok());
        assert_eq!(auth(&s, &a, NOW), Err(VaultError::Unauthenticated));
        // a request with no nonce (the old form) is not accepted
        let old = h.replace(&format!("nonce={},", cloudkeys::parse_authorization(&h).unwrap().nonce.unwrap()), "");
        assert_eq!(auth(&s, &old, NOW), Err(VaultError::Unauthenticated));
        // through the HTTP side too: the second copy is a 401, the first an ordinary answer
        let req = |h: &str| Request { method: "GET".into(), path: PATH.into(), query: Default::default(), headers: [("authorization".to_string(), h.to_string())].into(), body: vec![] };
        let h2 = signed(&keys(1), "acme/alice", NOW);
        assert_ne!(s.handle(&req(&h2), NOW).status, 401);
        assert_eq!(s.handle(&req(&h2), NOW).status, 401);
    }

    #[test]
    fn the_clock_window_is_120_seconds_either_way() {
        let d = Dir::new();
        let s = acme_at(&d, NOW);
        for (ts, ok) in [(NOW - 120, true), (NOW - 121, false), (NOW - 3600, false), (NOW + 120, true), (NOW + 121, false), (NOW + 86_400, false), (0, false), (u64::MAX, false)] {
            let r = auth(&s, &signed(&keys(1), "acme/alice", ts), NOW);
            assert_eq!(r.is_ok(), ok, "ts {ts}: {r:?}");
        }
    }

    #[test]
    fn a_burst_from_one_key_is_slowed_and_the_next_second_it_works_again() {
        let d = Dir::new();
        let s = acme_at(&d, NOW);
        s.set_limits(Limits { key_burst: 5.0, key_per_sec: 1.0, ..Limits::default() });
        for i in 0..5 {
            assert!(auth(&s, &signed(&keys(1), "acme/alice", NOW), NOW).is_ok(), "{i}");
        }
        assert!(matches!(auth(&s, &signed(&keys(1), "acme/alice", NOW), NOW), Err(VaultError::RateLimited(_))));
        assert!(auth(&s, &signed(&keys(2), "acme/bob", NOW), NOW).is_ok(), "another key has its own allowance");
        assert!(auth(&s, &signed(&keys(1), "acme/alice", NOW + 2), NOW + 2).is_ok(), "tokens come back with time");
        // the answer says how long to wait
        let req = Request { method: "GET".into(), path: PATH.into(), query: Default::default(), headers: [("authorization".to_string(), signed(&keys(1), "acme/alice", NOW + 2))].into(), body: vec![] };
        for _ in 0..3 {
            s.handle(&req, NOW + 2);
        }
        let mut limited = req.clone();
        limited.headers.insert("authorization".into(), signed(&keys(1), "acme/alice", NOW + 2));
        let r = s.handle(&limited, NOW + 2);
        assert_eq!(r.status, 429);
        assert!(r.headers.iter().any(|(k, v)| k == "Retry-After" && v.parse::<u64>().is_ok()));
    }

    #[test]
    fn a_source_that_floods_is_slowed_before_anything_is_checked() {
        let d = Dir::new();
        let s = acme_at(&d, NOW);
        s.set_limits(Limits { source_burst: 4.0, source_per_sec: 0.0, fail_max: 1000, ..Limits::default() });
        let call = |from: IpAddr| s.authenticate_from("GET", PATH, b"", Some("junk"), NOW, Some(from));
        for _ in 0..4 {
            assert_eq!(call(ip(1)), Err(VaultError::Unauthenticated));
        }
        assert!(matches!(call(ip(1)), Err(VaultError::RateLimited(_))));
        assert_eq!(call(ip(2)), Err(VaultError::Unauthenticated), "another address is not affected");
    }

    #[test]
    fn failures_lock_the_source_but_never_the_owner() {
        let d = Dir::new();
        let s = acme_at(&d, NOW);
        s.set_limits(Limits { fail_max: 3, fail_window: 60, lock_secs: 60, ..Limits::default() });
        let from = |h: &str, ip: IpAddr, now: u64| s.authenticate_from("GET", PATH, b"", Some(h), now, Some(ip));
        // someone at .9 guesses with a key that is not alice's, in alice's name
        let bad = |now| signed(&keys(66), "acme/alice", now);
        for _ in 0..3 {
            assert_eq!(from(&bad(NOW), ip(9), NOW), Err(VaultError::Unauthenticated));
        }
        assert!(matches!(from(&bad(NOW), ip(9), NOW), Err(VaultError::RateLimited(_))), "locked now");
        // the lock is on the address: alice is not locked out, from anywhere
        assert!(from(&signed(&keys(1), "acme/alice", NOW), ip(8), NOW).is_ok(), "alice from another address");
        assert!(from(&signed(&keys(1), "acme/alice", NOW), ip(9), NOW).is_ok(), "alice from the very address that is locked");
        // that success cleared the lock for the address: the next failure starts a count again
        assert_eq!(from(&bad(NOW), ip(9), NOW), Err(VaultError::Unauthenticated));
        // a lot of bad requests in alice's name from elsewhere do not touch her own key's allowance
        for _ in 0..50 {
            let _ = from(&bad(NOW), ip(7), NOW);
        }
        for _ in 0..5 {
            assert!(from(&signed(&keys(1), "acme/alice", NOW), ip(6), NOW).is_ok());
        }
        // a lock ends by itself
        for _ in 0..3 {
            let _ = from(&bad(NOW), ip(5), NOW);
        }
        assert!(matches!(from("junk", ip(5), NOW + 30), Err(VaultError::RateLimited(_))));
        assert_eq!(from("junk", ip(5), NOW + 61), Err(VaultError::Unauthenticated));
    }

    #[test]
    fn a_restart_does_not_make_a_seen_request_new() {
        let d = Dir::new();
        let h = signed(&keys(1), "acme/alice", NOW);
        {
            let s = acme_at(&d, NOW);
            assert!(auth(&s, &h, NOW).is_ok());
        }
        let s = Store::open_at(&d.0, NOW + 10).unwrap();
        assert_eq!(auth(&s, &h, NOW + 10), Err(VaultError::Unauthenticated), "replayed after the restart, inside its window");
        assert!(auth(&s, &signed(&keys(1), "acme/alice", NOW + 10), NOW + 10).is_ok());
        let log = d.0.join("replay.log");
        assert!(std::fs::read_to_string(&log).unwrap().lines().count() >= 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&log).unwrap().permissions().mode() & 0o777, 0o600);
        }
        drop(s);
        // long after: the log is cleaned at start, and the old request is simply too old
        let s = Store::open_at(&d.0, NOW + 1000).unwrap();
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "");
        assert_eq!(auth(&s, &h, NOW + 1000), Err(VaultError::Unauthenticated));
    }

    #[test]
    fn a_damaged_replay_log_falls_back_to_refusing_everything_signed_before_the_start() {
        let d = Dir::new();
        std::fs::create_dir_all(&d.0).unwrap();
        std::fs::write(d.0.join("replay.log"), "this is not a log\n").unwrap();
        let s = acme_at(&d, NOW);
        assert_eq!(auth(&s, &signed(&keys(1), "acme/alice", NOW - 5), NOW), Err(VaultError::Unauthenticated), "signed before the start");
        assert!(auth(&s, &signed(&keys(1), "acme/alice", NOW), NOW).is_ok());
    }

    #[test]
    fn the_loopback_service_limits_its_callers_and_the_client_waits_and_retries() {
        let d = Dir::new();
        drop(acme_at(&d, unix_now()));
        let run = start("127.0.0.1:0".parse().unwrap(), &d.0, None).unwrap();
        let c = |n: u8, u: &str| Client { host: "127.0.0.1".into(), port: run.local_addr().port(), fingerprint: Some(run.fingerprint.clone()), who: member("acme", u), keys: keys(n) };
        let alice = c(1, "alice");
        alice.create_scope(&Scope::Tenant).unwrap();
        for i in 0..20 {
            alice.put(&Scope::Tenant, &format!("s{i}"), b"v").unwrap();
        }
        assert_eq!(alice.list(&Scope::Tenant).unwrap().len(), 20);
        // one replayed request over the real socket is a 401
        let h = alice.keys.sign_request("acme/alice", "GET", "/v1/scope/tenant/secrets", unix_now(), b"");
        let get = || crate::devmode::client::https("127.0.0.1", run.local_addr().port(), Some(&run.fingerprint), "GET", "/v1/scope/tenant/secrets", &[("Authorization".to_string(), h.clone())], b"", Duration::from_secs(10)).unwrap().status;
        assert_eq!((get(), get()), (200, 401));
        run.stop();
    }
}
