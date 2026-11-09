//! Fake bridge services for the emulator. Nothing here reaches the real machine: files live in a scratch folder, the vault is a table of
//! made-up secrets, the network answers only canned replies, and audio and camera say they are emulated.
//!
//! Every call is logged with its arguments (secrets and tokens redacted) and its reply; the calls that would ask the person on a
//! device are recorded as prompts with the emulated answer; a call the manifest does not allow is recorded as a violation.

use crate::project::{cap_sensitive, cap_words, Config};
use iyakki_tile_host::ServiceBackend;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

/// 2026-01-01T00:00:00Z in milliseconds: the fake clock's start, so a recorded run always sees the same time.
pub const CLOCK_START_MS: u64 = 1_767_225_600_000;
const MAX_LOG: usize = 2000;
const MAX_FILE: usize = 1 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallRecord {
    pub seq: u32,
    pub domain: String,
    pub method: String,
    /// The request with secrets replaced by `[redacted]`.
    pub args: String,
    /// The reply (or the error text), also redacted.
    pub reply: String,
    pub ok: bool,
    /// The manifest does not allow this call; it was refused before any service ran.
    pub refused: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptRecord {
    pub domain: String,
    pub method: String,
    pub words: String,
    /// What the emulated person answered (by `deny` in the settings; yes by default).
    pub granted: bool,
}

#[derive(Default)]
struct State {
    seq: u32,
    now_ms: u64,
    log: Vec<CallRecord>,
    prompts: Vec<PromptRecord>,
    violations: Vec<String>,
    storage: BTreeMap<String, Value>,
    notes: Vec<(String, String)>,
}

pub struct FakeServices {
    st: Mutex<State>,
    scratch: PathBuf,
    config: Config,
}

/// Keys whose values never reach the call log.
fn secret_key(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    ["secret", "password", "passwd", "token", "authorization", "api_key", "apikey", "cookie", "private"].iter().any(|w| k.contains(w))
}

/// Replace secret values in a JSON value (and every `value` of a vault reply) and shorten long strings.
pub fn redact(v: &mut Value, vault: bool) {
    match v {
        Value::Object(o) => {
            for (k, x) in o.iter_mut() {
                if secret_key(k) || (vault && k == "value") {
                    *x = Value::String("[redacted]".into());
                } else {
                    redact(x, vault);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| redact(x, vault)),
        Value::String(s) if s.chars().count() > 200 => *s = format!("{}... ({} characters)", s.chars().take(200).collect::<String>(), s.chars().count()),
        _ => {}
    }
}

fn shown(json: &str, vault: bool) -> String {
    match serde_json::from_str::<Value>(json) {
        Ok(mut v) => {
            redact(&mut v, vault);
            v.to_string()
        }
        Err(_) => "(not JSON)".into(),
    }
}

/// `2026-01-01T00:00:00Z` for a Unix time in milliseconds.
pub fn iso(ms: u64) -> String {
    let secs = ms / 1000;
    let (days, rem) = (secs / 86400, secs % 86400);
    // civil-from-days
    let z = days as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

impl FakeServices {
    pub fn new(scratch: PathBuf, config: Config) -> FakeServices {
        FakeServices { st: Mutex::new(State { now_ms: CLOCK_START_MS, ..State::default() }), scratch, config }
    }

    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Move the fake clock (the emulator does it on every `Tick`).
    pub fn advance(&self, ms: u64) {
        self.st().now_ms += ms;
    }
    pub fn calls(&self) -> Vec<CallRecord> {
        self.st().log.clone()
    }
    pub fn prompts(&self) -> Vec<PromptRecord> {
        self.st().prompts.clone()
    }
    /// Calls the manifest did not allow, as `domain.method`.
    pub fn violations(&self) -> Vec<String> {
        self.st().violations.clone()
    }
    /// Notes left with `notify.send`: title and body.
    pub fn notes(&self) -> Vec<(String, String)> {
        self.st().notes.clone()
    }
    pub fn scratch(&self) -> &Path {
        &self.scratch
    }

    fn denied_by_person(&self, domain: &str, method: &str) -> bool {
        self.config.deny.iter().any(|d| d == domain || *d == format!("{domain}.*") || *d == format!("{domain}.{method}"))
    }

    /// A path from the tile, inside the scratch folder only: no absolute paths, no `..`, no way out through a link.
    fn resolve(&self, rel: &str) -> Result<PathBuf, String> {
        let p = Path::new(rel);
        if rel.is_empty() || rel.len() > 200 || rel.contains('\\') || !p.components().all(|c| matches!(c, Component::Normal(_))) {
            return Err("the path must be a plain relative path inside the scratch folder".into());
        }
        std::fs::create_dir_all(&self.scratch).map_err(|e| format!("scratch folder: {e}"))?;
        let root = std::fs::canonicalize(&self.scratch).map_err(|e| format!("scratch folder: {e}"))?;
        let full = root.join(p);
        let check = if full.exists() { std::fs::canonicalize(&full) } else { full.parent().map(std::fs::canonicalize).unwrap_or_else(|| Ok(root.clone())) };
        match check {
            Ok(c) if c.starts_with(&root) => Ok(full),
            _ => Err("the path leaves the scratch folder".into()),
        }
    }

    fn dispatch(&self, domain: &str, method: &str, a: &Value) -> Result<Value, String> {
        let s = |k: &str| a.get(k).and_then(Value::as_str).ok_or_else(|| format!("{domain}.{method} needs a string \"{k}\""));
        match (domain, method) {
            ("clock", "now") => {
                let ms = self.st().now_ms;
                Ok(json!({"ms": ms, "iso": iso(ms), "emulated": true}))
            }
            ("settings", "get") => {
                let k = s("key")?;
                self.config.settings.iter().find(|(n, _)| n == k).map(|(_, v)| json!({"value": v})).ok_or_else(|| format!("no setting {k:?} in iyakki-emulator.json"))
            }
            ("storage", "get") => {
                let k = s("key")?;
                Ok(json!({"value": self.st().storage.get(k).cloned().unwrap_or(Value::Null)}))
            }
            ("storage", "put") => {
                let k = s("key")?.to_string();
                let v = a.get("value").cloned().unwrap_or(Value::Null);
                let mut st = self.st();
                let total: usize = st.storage.iter().filter(|(n, _)| **n != k).map(|(n, v)| n.len() + v.to_string().len()).sum::<usize>() + k.len() + v.to_string().len();
                if total > 256 * 1024 {
                    return Err("storage is limited to 256 KB per app".into());
                }
                st.storage.insert(k, v);
                Ok(json!({"ok": true}))
            }
            ("storage", "delete") => {
                self.st().storage.remove(s("key")?);
                Ok(json!({"ok": true}))
            }
            ("files", "read") => {
                let p = self.resolve(s("path")?)?;
                let b = std::fs::read(&p).map_err(|e| format!("read failed: {e}"))?;
                if b.len() > MAX_FILE {
                    return Err("the file is larger than 1 MiB".into());
                }
                Ok(json!({"text": String::from_utf8_lossy(&b)}))
            }
            ("files", "write") => {
                let text = s("text")?;
                if text.len() > MAX_FILE {
                    return Err("the text is larger than 1 MiB".into());
                }
                let p = self.resolve(s("path")?)?;
                std::fs::write(&p, text).map_err(|e| format!("write failed: {e}"))?;
                Ok(json!({"ok": true, "bytes": text.len()}))
            }
            ("files", "list") => {
                let dir = if a.get("path").is_some() { self.resolve(s("path")?)? } else { std::fs::create_dir_all(&self.scratch).ok(); self.scratch.clone() };
                let mut names: Vec<String> = std::fs::read_dir(&dir).map_err(|e| format!("list failed: {e}"))?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
                names.sort();
                Ok(json!({"names": names}))
            }
            ("vault", "get") => {
                let n = s("name")?;
                let v = self.config.secrets.iter().find(|(k, _)| k == n).map_or_else(|| format!("emulated-secret-for-{n}"), |(_, v)| v.clone());
                Ok(json!({"value": v, "emulated": true}))
            }
            ("net", "fetch") => {
                let url = s("url")?;
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    return Err("only http and https addresses can be fetched".into());
                }
                self.config
                    .net
                    .iter()
                    .find(|(u, _, _)| u == url)
                    .map(|(_, status, body)| json!({"status": status, "body": body, "emulated": true}))
                    .ok_or_else(|| format!("no canned answer for {url}: add it under \"net\" in iyakki-emulator.json (the emulator never uses the real network)"))
            }
            ("notify", "send") => {
                let (t, b) = (s("title")?.to_string(), a.get("body").and_then(Value::as_str).unwrap_or("").to_string());
                self.st().notes.push((t, b));
                Ok(json!({"shown": true, "emulated": true}))
            }
            ("audio", "play") | ("audio", "record") => Ok(json!({"emulated": true, "note": "audio is emulated: nothing was played or recorded"})),
            ("camera", "capture") | ("camera", "list") => Ok(json!({"emulated": true, "note": "the camera is emulated: no picture is taken", "width": 0, "height": 0})),
            ("chat", "ask") => Ok(json!({"emulated": true, "text": "(emulated assistant reply)"})),
            (d, _) if crate::project::known_domain(d) => Err(format!("{domain}.{method} is not emulated")),
            _ => Err(format!("the service {domain} is not emulated")),
        }
    }
}

impl ServiceBackend for FakeServices {
    fn call(&self, domain: &str, method: &str, json: &str) -> Result<String, String> {
        let vault = domain == "vault";
        let parsed: Result<Value, String> = if json.trim().is_empty() { Ok(json!({})) } else { serde_json::from_str(json).map_err(|e| format!("the request is not valid JSON: {e}")) };
        let args = shown(json, vault);
        // the person is asked first for the sensitive calls (once per call kind; the answer comes from the settings)
        let mut granted = true;
        if cap_sensitive(&format!("{domain}.{method}")) {
            granted = !self.denied_by_person(domain, method);
            let mut st = self.st();
            if !st.prompts.iter().any(|p| p.domain == domain && p.method == method) {
                st.prompts.push(PromptRecord { domain: domain.into(), method: method.into(), words: cap_words(&format!("{domain}.{method}")), granted });
            } else {
                granted = st.prompts.iter().find(|p| p.domain == domain && p.method == method).is_some_and(|p| p.granted);
            }
        }
        let result = if !granted { Err("the person said no (emulated prompt)".to_string()) } else { parsed.and_then(|a| self.dispatch(domain, method, &a)) };
        let (ok, reply, out) = match &result {
            Ok(v) => {
                let mut shown = v.clone();
                redact(&mut shown, vault);
                (true, shown.to_string(), Ok(v.to_string()))
            }
            Err(e) => (false, e.clone(), Err(e.clone())),
        };
        let mut st = self.st();
        st.seq += 1;
        let seq = st.seq;
        if st.log.len() < MAX_LOG {
            st.log.push(CallRecord { seq, domain: domain.into(), method: method.into(), args, reply, ok, refused: false });
        }
        out
    }

    fn denied(&self, domain: &str, method: &str, json: &str) {
        let mut st = self.st();
        st.seq += 1;
        let seq = st.seq;
        st.violations.push(format!("{domain}.{method}"));
        if st.log.len() < MAX_LOG {
            st.log.push(CallRecord { seq, domain: domain.into(), method: method.into(), args: shown(json, domain == "vault"), reply: "REFUSED: the manifest does not declare this capability".into(), ok: false, refused: true });
        }
    }
}
