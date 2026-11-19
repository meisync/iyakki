//! Browser Web Push for the person's notifications (F-INBOX-09): the subscriptions a browser gave, the VAPID key of this installation
//! and the sending of one encrypted message to a push service (RFC 8030, RFC 8291, RFC 8292; the cryptography is `iyakki-webcrypto`).
//!
//! Everything secret lives in the person's vault as one entry, [`STATE`]: the VAPID private key and each subscription's `auth` secret.
//! Nothing secret is returned by a tool, logged or put in an audit line; the audit line names the host of the push service and a count.
//! A message leaves only through the notification hub's network ([`crate::notifyhub::transport::Net`]), so the egress policy, the
//! public-address check and the certificate check of that path apply.
//!
//! Tools: `webpush_status` (free), `webpush_subscribe`, `webpush_unsubscribe`, `webpush_send` (each asks on a card every time).

use std::sync::Arc;

use iyakki_webcrypto::{b64, webpush as wp};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::gitkeys::{AuditFn, SecretStore};
use crate::notifyhub::transport::Net;
use crate::notifyhub::webhook::HttpReq;

/// The vault entry that holds the VAPID key and the subscriptions.
pub const STATE: &str = "webpush.state";
pub const MAX_SUBSCRIPTIONS: usize = 20;
pub const STATUS: &str = "webpush_status";
pub const SUBSCRIBE: &str = "webpush_subscribe";
pub const UNSUBSCRIBE: &str = "webpush_unsubscribe";
pub const SEND: &str = "webpush_send";
pub const PREFS: &str = "webpush_prefs";

pub fn is_webpush_tool(n: &str) -> bool {
    matches!(n, STATUS | SUBSCRIBE | UNSUBSCRIBE | SEND | PREFS)
}

pub const PROMPT_RULE: &str = "\nWEB PUSH: webpush_status lists the browsers that may get this person's notifications (a label and the push service's host, never a key) and the public key a page needs. webpush_subscribe saves a subscription the browser made (endpoint, p256dh, auth) and webpush_unsubscribe removes one; webpush_send sends one short notification to the saved browsers through their push service. All three ask the person on a card every time. webpush_prefs shows or changes the person's own push settings (on or off, the lowest severity that is pushed, quiet hours such as 22:00-06:30, the minutes east of UTC): a change asks first, and a critical notification is the only kind that goes through quiet hours. Never put a password, key or other secret into a notification: it is encrypted on the way but shown on a lock screen.";

#[derive(Clone, Serialize, Deserialize, Default)]
struct Rec {
    id: String,
    label: String,
    endpoint: String,
    p256dh: String,
    auth: String,
    added: u64,
}

/// The person's own push settings (opt-out, the lowest severity, quiet hours). Default: on, normal and above, no quiet hours.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prefs {
    /// `false`: nothing is pushed to any saved browser (the person's opt-out); the notification still reaches the inbox.
    pub on: bool,
    /// `low`, `normal`, `high` or `critical`.
    pub min_severity: String,
    /// Quiet hours as `22:00-06:30`, empty: none.
    pub quiet: String,
    /// Minutes east of UTC, for the quiet hours.
    pub offset_min: i32,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs { on: true, min_severity: "normal".into(), quiet: String::new(), offset_min: 0 }
    }
}

/// What [`WebPush::notify`] did with one notification.
#[derive(Debug, PartialEq)]
pub enum Pushed {
    Sent(SendReport),
    /// Quiet hours: send at or after this time (Unix seconds); the caller keeps it until then.
    Held(u64),
    /// Not pushed, and why in words (opted out, below the lowest severity, nothing saved).
    Skipped(String),
}

#[derive(Serialize, Deserialize, Default)]
struct State {
    /// The VAPID private key as PKCS#8, base64.
    vapid: String,
    subs: Vec<Rec>,
    #[serde(default)]
    prefs: Prefs,
}

pub struct WebPush {
    store: Arc<dyn SecretStore>,
    net: Arc<dyn Net>,
    audit: AuditFn,
    now: fn() -> u64,
}

/// What one send did.
#[derive(Debug, PartialEq)]
pub struct SendReport {
    pub delivered: usize,
    pub gone: Vec<String>,
    pub failed: Vec<String>,
}

impl SendReport {
    pub fn words(&self) -> String {
        let mut s = format!("Sent to {} browser(s).", self.delivered);
        if !self.gone.is_empty() {
            s.push_str(&format!(" The push service says these no longer exist and they were removed: {}.", self.gone.join(", ")));
        }
        if !self.failed.is_empty() {
            s.push_str(&format!(" Not delivered: {}.", self.failed.join("; ")));
        }
        s
    }
}

fn host_of(endpoint: &str) -> String {
    wp::origin(endpoint).map(|o| o.trim_start_matches("https://").to_string()).unwrap_or_default()
}

impl WebPush {
    pub fn new(store: Arc<dyn SecretStore>, net: Arc<dyn Net>, audit: AuditFn) -> WebPush {
        WebPush { store, net, audit, now: || crate::clock::now_unix().max(0) as u64 }
    }

    fn load(&self) -> Result<State, String> {
        match self.store.get(STATE) {
            Ok(v) => serde_json::from_str(&v).map_err(|_| "the saved push state is damaged".to_string()),
            Err(e) if e.contains("locked") => Err("your vault is locked: sign in again, then try again".into()),
            Err(_) => Ok(State::default()),
        }
    }
    fn save(&self, s: &State) -> Result<(), String> {
        self.store.set(STATE, &serde_json::to_string(s).map_err(|e| e.to_string())?)
    }
    fn log(&self, action: &str, detail: &str) {
        (self.audit)(&format!("webpush.{action}"), detail);
    }
    fn vapid(s: &mut State) -> Result<wp::Vapid, String> {
        if s.vapid.is_empty() {
            s.vapid = b64::std(wp::Vapid::generate()?.to_pkcs8());
        }
        wp::Vapid::from_pkcs8(&b64::decode(&s.vapid)?)
    }

    /// The public key a page passes as `applicationServerKey`. Makes the key on first use.
    pub fn public_key(&self) -> Result<String, String> {
        let mut s = self.load()?;
        let fresh = s.vapid.is_empty();
        let k = Self::vapid(&mut s)?.public_key_b64();
        if fresh {
            self.save(&s)?;
            self.log("key", "a VAPID key was made");
        }
        Ok(k)
    }

    pub fn status(&self) -> Result<String, String> {
        let s = self.load()?;
        let key = if s.vapid.is_empty() { "No VAPID key yet (it is made when the first browser is saved).".to_string() } else { format!("Public key for pages: {}", Self::vapid(&mut State { vapid: s.vapid.clone(), subs: vec![], prefs: Prefs::default() })?.public_key_b64()) };
        if s.subs.is_empty() {
            return Ok(format!("No browsers are saved for notifications. {key}"));
        }
        let rows: Vec<String> = s.subs.iter().map(|r| format!("{} ({}) via {}", r.id, r.label, host_of(&r.endpoint))).collect();
        let p = &s.prefs;
        Ok(format!("{} browser(s) saved:\n{}\n{key}\nPush is {}; lowest severity {}; quiet hours {}.", s.subs.len(), rows.join("\n"), if p.on { "on" } else { "off" }, p.min_severity, if p.quiet.is_empty() { "none".to_string() } else { format!("{} (UTC{:+}h)", p.quiet, p.offset_min as f32 / 60.0) }))
    }

    pub fn subscribe(&self, label: &str, endpoint: &str, p256dh: &str, auth: &str) -> Result<String, String> {
        let label: String = label.chars().filter(|c| !c.is_control()).take(40).collect();
        if label.trim().is_empty() {
            return Err("give the browser a short name (for example \"phone\")".into());
        }
        wp::Subscription::new(endpoint, p256dh, auth)?;
        let mut s = self.load()?;
        if s.subs.iter().any(|r| r.endpoint == endpoint) {
            return Err("this browser is already saved".into());
        }
        if s.subs.len() >= MAX_SUBSCRIPTIONS {
            return Err(format!("at most {MAX_SUBSCRIPTIONS} browsers can be saved; remove one first"));
        }
        Self::vapid(&mut s)?;
        let id = (1..).map(|n| format!("b{n}")).find(|id| !s.subs.iter().any(|r| &r.id == id)).unwrap_or_default();
        s.subs.push(Rec { id: id.clone(), label: label.trim().into(), endpoint: endpoint.into(), p256dh: p256dh.into(), auth: auth.into(), added: (self.now)() });
        self.save(&s)?;
        self.log("subscribe", &format!("id={id} host={}", host_of(endpoint)));
        Ok(id)
    }

    pub fn unsubscribe(&self, id: &str) -> Result<(), String> {
        let mut s = self.load()?;
        let before = s.subs.len();
        s.subs.retain(|r| r.id != id);
        if s.subs.len() == before {
            return Err(format!("no saved browser called {id}"));
        }
        self.save(&s)?;
        self.log("unsubscribe", &format!("id={id}"));
        Ok(())
    }

    pub fn prefs(&self) -> Result<Prefs, String> {
        Ok(self.load()?.prefs)
    }

    /// Change the person's push settings. Refuses a severity or quiet-hours text it cannot read.
    pub fn set_prefs(&self, p: Prefs) -> Result<(), String> {
        if iyakki_shared::notifyhub_sink::Severity::parse(&p.min_severity).is_none() {
            return Err("the lowest severity is low, normal, high or critical".into());
        }
        if !p.quiet.is_empty() {
            iyakki_camdetect::alerts::Quiet::parse(&p.quiet)?;
        }
        if !(-14 * 60..=14 * 60).contains(&p.offset_min) {
            return Err("the time zone offset is within 14 hours of UTC".into());
        }
        let mut s = self.load()?;
        s.prefs = p;
        self.save(&s)?;
        self.log("prefs", "changed");
        Ok(())
    }

    /// The path a hub channel uses for one notification: the person's own settings decide whether, and when, it is pushed. The
    /// title and body go to the browsers only when the caller passes them (the hub passes a short summary unless the channel lets
    /// details out). A critical notification goes through quiet hours; everything else waits for them to end.
    pub fn notify(&self, severity: iyakki_shared::notifyhub_sink::Severity, title: &str, body: &str, now: u64) -> Result<Pushed, String> {
        use iyakki_shared::notifyhub_rules::{Quiet, Route, Rules};
        let s = self.load()?;
        if !s.prefs.on {
            return Ok(Pushed::Skipped("push is switched off for this person".into()));
        }
        if s.subs.is_empty() {
            return Ok(Pushed::Skipped("no browsers are saved for notifications".into()));
        }
        let min = iyakki_shared::notifyhub_sink::Severity::parse(&s.prefs.min_severity).unwrap_or(iyakki_shared::notifyhub_sink::Severity::Normal);
        let quiet = if s.prefs.quiet.is_empty() { None } else { iyakki_camdetect::alerts::Quiet::parse(&s.prefs.quiet).ok().map(|q| Quiet { start_min: q.from_min as u32, end_min: q.to_min as u32 }) };
        let rules = Rules { quiet, offset_min: s.prefs.offset_min, ..Rules::default() };
        match rules.route(severity, min, now) {
            Route::Skip | Route::Digest => Ok(Pushed::Skipped(format!("{} is below the lowest severity that is pushed ({})", severity.name(), min.name()))),
            Route::Send(at) if at > now => Ok(Pushed::Held(at)),
            Route::Send(_) => self.send(None, title, body, 3600).map(Pushed::Sent),
        }
    }

    /// Sends `{"title","body"}` to one saved browser (`only`) or to all. A 404 or 410 removes the subscription (RFC 8030).
    pub fn send(&self, only: Option<&str>, title: &str, body: &str, ttl: u32) -> Result<SendReport, String> {
        let title: String = title.chars().filter(|c| !c.is_control()).take(120).collect();
        let body: String = body.chars().filter(|c| !c.is_control() || *c == '\n').take(1000).collect();
        if title.trim().is_empty() {
            return Err("a notification needs a title".into());
        }
        let mut s = self.load()?;
        if s.subs.is_empty() {
            return Err("no browsers are saved for notifications".into());
        }
        if let Some(id) = only {
            if !s.subs.iter().any(|r| r.id == id) {
                return Err(format!("no saved browser called {id}"));
            }
        }
        let vapid = Self::vapid(&mut s)?;
        let payload = json!({"title": title, "body": body}).to_string();
        let now = (self.now)();
        let mut rep = SendReport { delivered: 0, gone: vec![], failed: vec![] };
        for r in s.subs.clone().iter().filter(|r| only.is_none_or(|o| o == r.id)) {
            let sub = match wp::Subscription::new(&r.endpoint, &r.p256dh, &r.auth) {
                Ok(x) => x,
                Err(e) => {
                    rep.failed.push(format!("{}: {e}", r.id));
                    continue;
                }
            };
            let req = wp::build_request(&sub, &vapid, payload.as_bytes(), ttl.clamp(0, 86_400 * 7), wp::Urgency::Normal, now, "mailto:admin@iyakki.invalid")?;
            let http = HttpReq { url: req.url, headers: req.headers.into_iter().filter(|(k, _)| k != "Content-Length").collect(), body: req.body, timeout_secs: 20 };
            match self.net.post(None, &http) {
                Ok(200..=202) => rep.delivered += 1,
                Ok(404 | 410) => {
                    s.subs.retain(|x| x.id != r.id);
                    rep.gone.push(r.id.clone());
                }
                Ok(c) => rep.failed.push(format!("{}: the push service answered {c}", r.id)),
                Err(e) => rep.failed.push(format!("{}: {e}", r.id)),
            }
        }
        if !rep.gone.is_empty() {
            self.save(&s)?;
        }
        self.log("send", &format!("delivered={} gone={} failed={}", rep.delivered, rep.gone.len(), rep.failed.len()));
        Ok(rep)
    }
}

/// The assistant's tools over [`WebPush`].
pub struct WebPushTools {
    inner: Arc<dyn Tools>,
    svc: Arc<WebPush>,
}

impl WebPushTools {
    pub fn new(inner: Arc<dyn Tools>, svc: Arc<WebPush>) -> WebPushTools {
        WebPushTools { inner, svc }
    }
    fn t(args: &Value, k: &str) -> String {
        args.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string()
    }
    fn card(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            SUBSCRIBE => Some(format!("Save the browser \"{}\" for notifications (push service: {}). Notifications to it are encrypted and sent through that service.", Self::t(args, "label"), host_of(&Self::t(args, "endpoint")))),
            UNSUBSCRIBE => Some(format!("Stop sending notifications to the saved browser {}", Self::t(args, "id"))),
            PREFS => Some(format!("Change the push settings for your browsers:{}{}{}{}", args.get("on").and_then(Value::as_bool).map_or(String::new(), |b| format!("\npush {}", if b { "on" } else { "off" })), match Self::t(args, "min_severity") { v if v.is_empty() => String::new(), v => format!("\nlowest severity {v}") }, match Self::t(args, "quiet") { v if v.is_empty() => String::new(), v => format!("\nquiet hours {v}") }, args.get("offset_min").and_then(Value::as_i64).map_or(String::new(), |m| format!("\ntime zone {m} minutes east of UTC")))),
            SEND => Some(format!("Send a notification to {} through its push service:\n{}\n{}", match Self::t(args, "id") { i if i.is_empty() => "every saved browser".to_string(), i => format!("the saved browser {i}") }, Self::t(args, "title"), Self::t(args, "body"))),
            _ => None,
        }
    }
}

impl Tools for WebPushTools {
    fn inner_tools(&self) -> Option<&dyn Tools> {
        Some(&*self.inner)
    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_webpush_tool(&d.spec.name));
        let spec = |name: &str, description: &str, schema: Value| ToolSpec { name: name.into(), description: description.into(), schema };
        v.push(ToolDef { spec: spec(STATUS, "List the browsers saved for notifications (id, label, push service host) and the public key a page needs. No secrets.", json!({"type": "object", "properties": {}})), risk: Risk::Safe, untrusted: false });
        v.push(ToolDef {
            spec: spec(SUBSCRIBE, "Save a browser's push subscription (from PushSubscription.toJSON()). The person approves a card every time.", json!({"type": "object", "properties": {
                "label": {"type": "string", "description": "A short name, such as phone."},
                "endpoint": {"type": "string", "description": "The https endpoint of the subscription."},
                "p256dh": {"type": "string", "description": "keys.p256dh, base64url."},
                "auth": {"type": "string", "description": "keys.auth, base64url."}}, "required": ["label", "endpoint", "p256dh", "auth"]})),
            risk: Risk::Ask,
            untrusted: false,
        });
        v.push(ToolDef { spec: spec(UNSUBSCRIBE, "Remove a saved browser. The person approves a card every time.", json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]})), risk: Risk::Ask, untrusted: false });
        v.push(ToolDef {
            spec: spec(PREFS, "Change this person's push settings: `on` (false is the opt-out), `min_severity` (low, normal, high, critical), `quiet` (hours such as 22:00-06:30, or \"none\"), `offset_min` (minutes east of UTC, for the quiet hours). Only what is given changes. The person approves a card first. webpush_status shows the current settings.", json!({"type": "object", "properties": {"on": {"type": "boolean"}, "min_severity": {"type": "string"}, "quiet": {"type": "string"}, "offset_min": {"type": "integer"}}})),
            risk: Risk::Ask,
            untrusted: false,
        });
        v.push(ToolDef {
            spec: spec(SEND, "Send one short notification (title, body) to a saved browser or all of them. The person approves a card showing the text, every time. Never include secrets.", json!({"type": "object", "properties": {
                "title": {"type": "string"}, "body": {"type": "string"}, "id": {"type": "string", "description": "A saved browser id; leave out for all."}}, "required": ["title"]})),
            risk: Risk::Ask,
            untrusted: false,
        });
        v
    }
    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            STATUS => self.svc.status(),
            SUBSCRIBE => {
                let id = self.svc.subscribe(&Self::t(args, "label"), &Self::t(args, "endpoint"), &Self::t(args, "p256dh"), &Self::t(args, "auth"))?;
                Ok(format!("Saved as {id}. Public key for the page: {}", self.svc.public_key()?))
            }
            UNSUBSCRIBE => self.svc.unsubscribe(&Self::t(args, "id")).map(|_| "Removed.".into()),
            PREFS => {
                let mut p = self.svc.prefs()?;
                if let Some(b) = args.get("on").and_then(Value::as_bool) {
                    p.on = b;
                }
                if !Self::t(args, "min_severity").is_empty() {
                    p.min_severity = Self::t(args, "min_severity").to_lowercase();
                }
                match Self::t(args, "quiet") {
                    q if q.is_empty() => {}
                    q if q.eq_ignore_ascii_case("none") => p.quiet.clear(),
                    q => p.quiet = q,
                }
                if let Some(m) = args.get("offset_min").and_then(Value::as_i64) {
                    p.offset_min = i32::try_from(m).map_err(|_| "the time zone offset is within 14 hours of UTC".to_string())?;
                }
                self.svc.set_prefs(p)?;
                self.svc.status()
            }
            SEND => {
                let id = Self::t(args, "id");
                Ok(self.svc.send(if id.is_empty() { None } else { Some(&id) }, &Self::t(args, "title"), &Self::t(args, "body"), 3600)?.words())
            }
            _ => self.inner.call(name, args),
        }
    }
    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            STATUS => "List the browsers saved for notifications".into(),
            SUBSCRIBE | UNSUBSCRIBE | SEND | PREFS => self.card(name, args).and_then(|c| c.lines().next().map(str::to_string)).unwrap_or_default(),
            _ => self.inner.describe(name, args),
        }
    }
    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            SUBSCRIBE | UNSUBSCRIBE | SEND | PREFS => self.card(name, args),
            STATUS => None,
            _ => self.inner.preview(name, args),
        }
    }
    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            SEND if Self::t(args, "title").is_empty() => Some("a notification needs a title".into()),
            SUBSCRIBE => wp::Subscription::new(&Self::t(args, "endpoint"), &Self::t(args, "p256dh"), &Self::t(args, "auth")).err(),
            STATUS | UNSUBSCRIBE | SEND | PREFS => None,
            _ => self.inner.refusal(name, args),
        }
    }
    fn pre_approved(&self, name: &str) -> bool {
        if is_webpush_tool(name) { name == STATUS } else { self.inner.pre_approved(name) }
    }
    fn approved_always(&self, name: &str) -> bool {
        !is_webpush_tool(name) && self.inner.approved_always(name)
    }
    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            SUBSCRIBE | UNSUBSCRIBE | SEND | PREFS => Some("changing or using the saved browsers asks every time".into()),
            _ => self.inner.grant_blocker(name, args),
        }
    }
    fn grant_scope(&self, name: &str, args: &Value) -> String {
        self.inner.grant_scope(name, args)
    }
    fn unresolved_work(&self) -> Vec<String> {
        self.inner.unresolved_work()
    }
    fn begin_step(&self) {
        self.inner.begin_step()
    }
    fn should_end_run(&self) -> Option<String> {
        self.inner.should_end_run()
    }
    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.inner.take_diagnosis()
    }
    fn note_external(&self, name: &str, args: &Value, ok: bool) {
        self.inner.note_external(name, args, ok)
    }
}

#[cfg(test)]
#[path = "webpush_tests.rs"]
mod tests;
