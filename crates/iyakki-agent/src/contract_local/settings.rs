//! `settings.*` over the settings store (`settings.json`): a typed view of a few keys, with a schema, scopes, local-only keys, an
//! idempotent `set` and a `watch` that resumes from a sequence number. The store stays the one place that validates and saves.
//!
//! Keys are added here one by one as screens migrate; a key never exposes a secret (the store holds none) and `vault.keyfile`,
//! provider addresses and tokens are not keys at all.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use iyakki_core_api::contract::*;

use super::{guarded, now_ms, who, Audit};
use crate::settings::{Settings as AppSettings, SettingsError, SettingsStore};

struct Key {
    spec: SettingSpec,
    get: fn(&AppSettings) -> SettingValue,
    set: fn(&mut AppSettings, &SettingValue),
}

fn spec(key: &str, kind: SettingKind, default: SettingValue, write_risk: Risk) -> SettingSpec {
    SettingSpec { key: key.to_string(), kind, default, read_scope: Scope::SETTINGS_READ.into(), write_scope: Scope::SETTINGS_WRITE.into(), write_risk, label_key: format!("settings.{key}") }
}

fn text(v: &SettingValue) -> String {
    if let SettingValue::Text(t) = v { t.clone() } else { String::new() }
}
fn flag(v: &SettingValue) -> bool {
    matches!(v, SettingValue::Bool(true))
}
fn int(v: &SettingValue) -> i64 {
    if let SettingValue::Int(n) = v { *n } else { 0 }
}

fn keys() -> Vec<Key> {
    let d = AppSettings::default();
    vec![
        Key { spec: spec("time.zone", SettingKind::Text { max_len: 64 }, SettingValue::Text(String::new()), Risk::Change), get: |s| SettingValue::Text(s.time.zone.clone()), set: |s, v| s.time.zone = text(v) },
        Key { spec: spec("time.zone_auto", SettingKind::Bool, SettingValue::Bool(d.time.zone_auto), Risk::Change), get: |s| SettingValue::Bool(s.time.zone_auto), set: |s, v| s.time.zone_auto = flag(v) },
        Key { spec: spec("time.no_ntp", SettingKind::Bool, SettingValue::Bool(d.time.no_ntp), Risk::Change), get: |s| SettingValue::Bool(s.time.no_ntp), set: |s, v| s.time.no_ntp = flag(v) },
        Key { spec: spec("time.ntp_server", SettingKind::Text { max_len: 253 }, SettingValue::Text(String::new()), Risk::Change), get: |s| SettingValue::Text(s.time.ntp_server.clone()), set: |s, v| s.time.ntp_server = text(v) },
        Key {
            spec: spec("tool_log_days", SettingKind::Int { min: 0, max: crate::toollog::MAX_DAYS as i64 }, SettingValue::Int(0), Risk::Change),
            get: |s| SettingValue::Int(s.tool_log_days as i64),
            set: |s, v| s.tool_log_days = int(v) as u32,
        },
        Key {
            spec: spec("media_download_max_mb", SettingKind::Int { min: 0, max: 1_048_576 }, SettingValue::Int(0), Risk::Change),
            get: |s| SettingValue::Int(s.media_download_max_mb as i64),
            set: |s, v| s.media_download_max_mb = int(v) as u32,
        },
        Key { spec: spec("show_all_ai", SettingKind::Bool, SettingValue::Bool(false), Risk::Change), get: |s| SettingValue::Bool(s.show_all_ai), set: |s, v| s.show_all_ai = flag(v) },
        // what the vault does when the last administrator signs out: only the person at the machine may change it
        Key {
            spec: spec("vault.lock_on_admin_logout", SettingKind::Bool, SettingValue::Bool(false), Risk::LocalOnly),
            get: |s| SettingValue::Bool(s.vault.lock_on_admin_logout),
            set: |s, v| s.vault.lock_on_admin_logout = flag(v),
        },
    ]
}

struct Inner {
    last: BTreeMap<String, SettingValue>,
    log: EventLog<SettingEvent>,
    idem: HashMap<IdempotencyKey, (String, SettingValue, u64)>,
}

pub struct CoreSettings {
    store: Arc<Mutex<SettingsStore>>,
    keys: Vec<Key>,
    inner: Mutex<Inner>,
    audit: Arc<Audit>,
}

impl CoreSettings {
    pub fn new(store: Arc<Mutex<SettingsStore>>) -> CoreSettings {
        let keys = keys();
        let last = {
            let g = store.lock().unwrap_or_else(|e| e.into_inner());
            keys.iter().map(|k| (k.spec.key.clone(), (k.get)(g.get()))).collect()
        };
        CoreSettings { store, keys, inner: Mutex::new(Inner { last, log: EventLog::new(256), idem: HashMap::new() }), audit: Arc::new(Audit::default()) }
    }

    pub fn audit(&self) -> Vec<String> {
        self.audit.all()
    }
    pub(crate) fn audit_handle(&self) -> &Audit {
        &self.audit
    }

    fn key(&self, name: &str) -> CallResult<&Key> {
        self.keys.iter().find(|k| k.spec.key == name).ok_or_else(|| CallError::new(ErrorCode::NotFound, "settings.unknown_key", &format!("There is no setting called {name}.")))
    }

    /// Settings changed by something else (a screen, a restore) become events too, so a watcher misses nothing.
    fn catch_up(&self, inner: &mut Inner) {
        let g = self.store.lock().unwrap_or_else(|e| e.into_inner());
        for k in &self.keys {
            let now = (k.get)(g.get());
            if inner.last.get(&k.spec.key) != Some(&now) {
                inner.last.insert(k.spec.key.clone(), now.clone());
                inner.log.push("changed", now_ms(), SettingEvent { key: k.spec.key.clone(), value: now, by: "machine".into() });
            }
        }
    }
}

impl Settings for CoreSettings {
    fn schema(&self, ctx: &CallContext) -> CallResult<Vec<SettingSpec>> {
        guarded(&self.audit, ctx, "settings.schema", Scope::SETTINGS_READ, Risk::Read)?;
        Ok(self.keys.iter().map(|k| k.spec.clone()).collect())
    }

    fn get(&self, ctx: &CallContext, key: &str) -> CallResult<SettingValue> {
        let k = self.key(key)?;
        guarded(&self.audit, ctx, "settings.get", &k.spec.read_scope, Risk::Read)?;
        let g = self.store.lock().unwrap_or_else(|e| e.into_inner());
        Ok((k.get)(g.get()))
    }

    fn set(&self, ctx: &CallContext, key: &str, value: SettingValue, idem: Option<&IdempotencyKey>) -> CallResult<SettingChange> {
        let k = self.key(key)?;
        let risk = k.spec.write_risk;
        let scope = if risk == Risk::LocalOnly { Scope::LOCAL } else { k.spec.write_scope.as_str() };
        guarded(&self.audit, ctx, "settings.set", scope, risk)?;
        let refuse = |e: CallError| {
            self.audit.line(ctx, "settings.set", risk, &format!("refused: {}", e.code.as_str()));
            e
        };
        // the system settings are an administrator's: a local person needs the role, a remote computer needs the scope
        if ctx.is_local() && !ctx.identity.admin {
            return Err(refuse(CallError::new(ErrorCode::Denied, "settings.admin_only", "Only an administrator can change settings.")));
        }
        if !k.spec.kind.accepts(&value) {
            return Err(refuse(CallError::new(ErrorCode::InvalidInput, "settings.bad_value", &format!("That value does not fit {key}.")).hint("Look at the schema for the allowed values.")));
        }
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ik) = idem {
            if let Some((ek, ev, seq)) = inner.idem.get(ik) {
                if ek == key && *ev == value {
                    return Ok(SettingChange { key: key.into(), value, seq: *seq, repeated: true });
                }
                return Err(CallError::new(ErrorCode::Conflict, "core.idempotency_conflict", "That key was already used for a different request."));
            }
        }
        // anything changed behind our back becomes its own event first, so sequence numbers follow the order things happened in
        self.catch_up(&mut inner);
        let saved = self.store.lock().unwrap_or_else(|e| e.into_inner()).update(|s| (k.set)(s, &value));
        if let Err(e) = saved {
            let err = match e {
                SettingsError::Invalid(m) => CallError::new(ErrorCode::InvalidInput, "settings.rejected", &m).hint("Look at the schema for the allowed values."),
                SettingsError::Storage(m) => CallError::new(ErrorCode::Io, "settings.not_saved", &m),
            };
            return Err(refuse(err));
        }
        inner.last.insert(key.to_string(), value.clone());
        let seq = inner.log.push("changed", now_ms(), SettingEvent { key: key.into(), value: value.clone(), by: who(ctx) });
        if let Some(ik) = idem {
            inner.idem.insert(ik.clone(), (key.into(), value.clone(), seq));
        }
        // the audit line names the key, never the value
        self.audit.line(ctx, &format!("settings.set {key}"), risk, "ok");
        Ok(SettingChange { key: key.into(), value, seq, repeated: false })
    }

    fn watch(&self, ctx: &CallContext, key: Option<&str>, resume: Resume, limit: usize) -> CallResult<EventBatch<SettingEvent>> {
        guarded(&self.audit, ctx, "settings.watch", Scope::SETTINGS_READ, Risk::Read)?;
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        self.catch_up(&mut inner);
        let mut b = inner.log.read(resume, Some("changed"), limit);
        if let Some(k) = key {
            b.events.retain(|e| e.body.key == k);
        }
        Ok(b)
    }
}
