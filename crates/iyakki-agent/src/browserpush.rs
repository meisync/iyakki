//! Web Push for the pages of IYAKKI's own browser (F-INBOX-09): what a page's `PushManager` and service worker registration reach.
//!
//! The person's browser registers a worker for an origin, the person is asked on a card (once per origin; the answer is kept and can be taken
//! back with `webpush_site_forget`), and a subscription is made: a fresh P-256 key and a 16-byte auth secret that stay in the person's vault
//! (entry [`STATE`]), and an endpoint that the push service gives. Messages the service holds are fetched ([`PagePush::poll`]), opened with
//! the subscription's own key (RFC 8291) and shown as a notification in the INBOX that names the site.
//!
//! The push service is a trait ([`PushService`]): this core ships no service of its own, so by default nothing can subscribe and the page is told
//! so in a sentence ([`NoService`]); which service to use is the owner's choice (D-BROWSERPUSH-SERVICE in ALPHA-7-DECISIONS.md). Nothing secret
//! is returned to a page, logged or audited: audit lines carry the origin and counts only.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use iyakki_webcrypto::{b64, p256, webpush as wp};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::gitkeys::{AuditFn, SecretStore};

pub const STATE: &str = "browserpush.state";
pub const MAX_REGISTRATIONS: usize = 50;
/// Most messages opened in one round.
pub const MAX_PER_POLL: usize = 50;

/// The service that gives subscriptions an endpoint and holds their messages.
pub trait PushService: Send + Sync {
    /// A new endpoint (an https address) for `origin`'s subscription made for the application server key `key`.
    fn create(&self, origin: &str, key: &str) -> Result<String, String>;
    /// The encrypted bodies waiting for `endpoint`, taken from the service.
    fn fetch(&self, endpoint: &str) -> Result<Vec<Vec<u8>>, String>;
    /// The subscription is gone: the service may forget the endpoint.
    fn revoke(&self, endpoint: &str);
}

/// No push service is set up on this computer.
pub struct NoService;
impl PushService for NoService {
    fn create(&self, _: &str, _: &str) -> Result<String, String> {
        Err("NotSupportedError: this computer has no push service set up yet (the administrator chooses one)".into())
    }
    fn fetch(&self, _: &str) -> Result<Vec<Vec<u8>>, String> {
        Ok(Vec::new())
    }
    fn revoke(&self, _: &str) {}
}

/// The person is asked whether `origin` may send notifications. True only for an explicit yes on this machine.
pub trait AskOrigin: Send + Sync {
    fn allow(&self, origin: &str) -> bool;
}

/// A notification from a site is shown to the person (the INBOX).
pub trait ShowPush: Send + Sync {
    fn show(&self, origin: &str, title: &str, body: &str) -> Result<(), String>;
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SubRec {
    endpoint: String,
    key: String,
    p256dh: String,
    auth: String,
    /// The subscription's private key, base64url (vault only).
    private: String,
    created: u64,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Reg {
    origin: String,
    scope: String,
    script: String,
    sub: Option<SubRec>,
}

#[derive(Clone, Serialize, Deserialize, Default)]
struct State {
    regs: Vec<Reg>,
    /// origin to `granted` or `denied`; no entry is `prompt`.
    perms: BTreeMap<String, String>,
}

pub struct PagePush {
    store: Arc<dyn SecretStore>,
    service: Arc<dyn PushService>,
    ask: Arc<dyn AskOrigin>,
    show: Arc<dyn ShowPush>,
    audit: AuditFn,
    now: fn() -> u64,
}

fn host_of(endpoint: &str) -> String {
    wp::origin(endpoint).map(|o| o.trim_start_matches("https://").to_string()).unwrap_or_default()
}

fn clean(s: &str, max: usize, keep_newline: bool) -> String {
    s.chars().filter(|c| !c.is_control() || (keep_newline && *c == '\n')).take(max).collect()
}

impl PagePush {
    pub fn new(store: Arc<dyn SecretStore>, service: Arc<dyn PushService>, ask: Arc<dyn AskOrigin>, show: Arc<dyn ShowPush>, audit: AuditFn) -> PagePush {
        PagePush { store, service, ask, show, audit, now: || crate::clock::now_unix().max(0) as u64 }
    }

    fn load(&self) -> Result<State, String> {
        match self.store.get(STATE) {
            Ok(v) => serde_json::from_str(&v).map_err(|_| "the saved page push state is damaged".to_string()),
            Err(e) if e.contains("locked") => Err("your vault is locked: sign in again, then try again".into()),
            Err(_) => Ok(State::default()),
        }
    }
    fn save(&self, s: &State) -> Result<(), String> {
        self.store.set(STATE, &serde_json::to_string(s).map_err(|e| e.to_string())?)
    }
    fn log(&self, action: &str, detail: &str) {
        (self.audit)(&format!("browserpush.{action}"), detail);
    }

    fn to_sub(r: &SubRec) -> PagePushSubOut {
        PagePushSubOut { endpoint: r.endpoint.clone(), expiration_ms: None, key: r.key.clone(), p256dh: r.p256dh.clone(), auth: r.auth.clone() }
    }

    pub fn permission_of(&self, origin: &str) -> String {
        self.load().ok().and_then(|s| s.perms.get(origin).cloned()).unwrap_or_else(|| "prompt".into())
    }

    pub fn register_script(&self, origin: &str, scope: &str, script: &str) -> Result<(), String> {
        if !origin.starts_with("https://") {
            return Err("SecurityError: only secure pages register a service worker".into());
        }
        let mut s = self.load()?;
        match s.regs.iter_mut().find(|r| r.origin == origin && r.scope == scope) {
            Some(r) => r.script = script.to_string(),
            None => {
                if s.regs.len() >= MAX_REGISTRATIONS {
                    return Err("QuotaExceededError: too many registrations are kept; remove some with webpush_site_forget".into());
                }
                s.regs.push(Reg { origin: origin.into(), scope: scope.into(), script: script.into(), sub: None });
            }
        }
        self.save(&s)?;
        self.log("register", &format!("origin={origin}"));
        Ok(())
    }

    pub fn remove_registration(&self, origin: &str, scope: &str) -> Result<bool, String> {
        let mut s = self.load()?;
        let before = s.regs.len();
        let gone: Vec<Reg> = s.regs.iter().filter(|r| r.origin == origin && r.scope == scope).cloned().collect();
        s.regs.retain(|r| !(r.origin == origin && r.scope == scope));
        for r in &gone {
            if let Some(sub) = &r.sub {
                self.service.revoke(&sub.endpoint);
            }
        }
        self.save(&s)?;
        self.log("unregister", &format!("origin={origin}"));
        Ok(s.regs.len() != before)
    }

    fn random_scalar() -> Result<[u8; 32], String> {
        for _ in 0..8 {
            let mut d = [0u8; 32];
            iyakki_webcrypto::random(&mut d)?;
            if p256::scalar_in_range(&d) {
                return Ok(d);
            }
        }
        Err("no usable random key".into())
    }

    pub fn make_subscription(&self, origin: &str, scope: &str, key: &str) -> Result<PagePushSubOut, String> {
        let mut s = self.load()?;
        let at = s.regs.iter().position(|r| r.origin == origin && r.scope == scope).ok_or("InvalidStateError: the page has no service worker registration for this scope")?;
        if let Some(sub) = &s.regs[at].sub {
            return if sub.key == key { Ok(Self::to_sub(sub)) } else { Err("InvalidStateError: a subscription with another key already exists; unsubscribe first".into()) };
        }
        match s.perms.get(origin).map(String::as_str) {
            Some("granted") => {}
            Some("denied") => return Err("NotAllowedError: notifications from this site are blocked".into()),
            _ => {
                let yes = self.ask.allow(origin);
                s.perms.insert(origin.to_string(), if yes { "granted" } else { "denied" }.into());
                self.save(&s)?;
                self.log("permission", &format!("origin={origin} answer={}", if yes { "granted" } else { "denied" }));
                if !yes {
                    return Err("NotAllowedError: notifications from this site were not allowed".into());
                }
            }
        }
        let endpoint = self.service.create(origin, key)?;
        wp::origin(&endpoint).map_err(|e| format!("AbortError: {e}"))?;
        if !endpoint.starts_with("https://") || endpoint.len() > 2048 {
            return Err("AbortError: the push service gave an address that is not usable".into());
        }
        let private = Self::random_scalar()?;
        let public = p256::public_from_private(&private)?;
        let mut auth = [0u8; 16];
        iyakki_webcrypto::random(&mut auth)?;
        let rec = SubRec { endpoint: endpoint.clone(), key: key.to_string(), p256dh: b64::url(&public), auth: b64::url(&auth), private: b64::url(&private), created: (self.now)() };
        let out = Self::to_sub(&rec);
        s.regs[at].sub = Some(rec);
        self.save(&s)?;
        self.log("subscribe", &format!("origin={origin} service={}", host_of(&endpoint)));
        Ok(out)
    }

    pub fn current(&self, origin: &str, scope: &str) -> Option<PagePushSubOut> {
        let s = self.load().ok()?;
        s.regs.iter().find(|r| r.origin == origin && r.scope == scope).and_then(|r| r.sub.as_ref()).map(Self::to_sub)
    }

    pub fn drop_subscription(&self, origin: &str, scope: &str) -> Result<bool, String> {
        let mut s = self.load()?;
        let Some(r) = s.regs.iter_mut().find(|r| r.origin == origin && r.scope == scope) else { return Ok(false) };
        let Some(sub) = r.sub.take() else { return Ok(false) };
        self.service.revoke(&sub.endpoint);
        self.save(&s)?;
        self.log("unsubscribe", &format!("origin={origin}"));
        Ok(true)
    }

    /// The sites with a registration, their permission and whether they hold a subscription (for the chat tool; no secret).
    pub fn sites(&self) -> Result<String, String> {
        let s = self.load()?;
        let mut origins: Vec<&str> = s.regs.iter().map(|r| r.origin.as_str()).chain(s.perms.keys().map(String::as_str)).collect();
        origins.sort();
        origins.dedup();
        if origins.is_empty() {
            return Ok("No site has asked to send notifications.".into());
        }
        let mut out = String::new();
        for o in origins {
            let perm = s.perms.get(o).map(String::as_str).unwrap_or("prompt");
            let subs: Vec<String> = s.regs.iter().filter(|r| r.origin == o).filter_map(|r| r.sub.as_ref()).map(|x| host_of(&x.endpoint)).collect();
            out.push_str(&format!("{o}: {perm}, {}\n", if subs.is_empty() { "no subscription".to_string() } else { format!("subscribed through {}", subs.join(", ")) }));
        }
        Ok(out.trim_end().to_string())
    }

    /// Forget a site: its registrations, subscriptions and the saved permission (it will ask again).
    pub fn forget_site(&self, origin: &str) -> Result<usize, String> {
        let mut s = self.load()?;
        let mut n = 0;
        for r in s.regs.iter().filter(|r| r.origin == origin) {
            n += 1;
            if let Some(sub) = &r.sub {
                self.service.revoke(&sub.endpoint);
            }
        }
        s.regs.retain(|r| r.origin != origin);
        let had_perm = s.perms.remove(origin).is_some();
        if n == 0 && !had_perm {
            return Err(format!("no saved notification settings for {origin}"));
        }
        self.save(&s)?;
        self.log("forget", &format!("origin={origin} registrations={n}"));
        Ok(n)
    }

    /// One round: fetch what each subscription's endpoint holds, open it with the subscription's key and show it. A message that does not
    /// open is dropped (counted), a site that is no longer allowed is skipped. Returns how many messages were shown.
    pub fn poll(&self) -> Result<usize, String> {
        let s = self.load()?;
        let (mut shown, mut dropped) = (0usize, 0usize);
        for r in &s.regs {
            let Some(sub) = &r.sub else { continue };
            if s.perms.get(&r.origin).map(String::as_str) != Some("granted") || shown >= MAX_PER_POLL {
                continue;
            }
            let bodies = match self.service.fetch(&sub.endpoint) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let private: [u8; 32] = match b64::decode(&sub.private).ok().and_then(|v| v.try_into().ok()) {
                Some(p) => p,
                None => continue,
            };
            let auth = b64::decode(&sub.auth).unwrap_or_default();
            for body in bodies.into_iter().take(MAX_PER_POLL) {
                match wp::decrypt(&body, &private, &auth) {
                    Ok(plain) => {
                        let (title, text) = parse_message(&plain);
                        if self.show.show(&r.origin, &title, &text).is_ok() {
                            shown += 1;
                        }
                    }
                    Err(_) => dropped += 1,
                }
            }
        }
        if shown + dropped > 0 {
            self.log("receive", &format!("shown={shown} dropped={dropped}"));
        }
        Ok(shown)
    }
}

/// A pushed message: JSON with `title` and `body` (as the IYAKKI service worker sends), or plain text.
fn parse_message(plain: &[u8]) -> (String, String) {
    let text = String::from_utf8_lossy(plain);
    match serde_json::from_str::<Value>(&text) {
        Ok(v) if v.get("title").is_some() || v.get("body").is_some() => (clean(v["title"].as_str().unwrap_or(""), 120, false), clean(v["body"].as_str().unwrap_or(""), 1000, true)),
        _ => (String::new(), clean(&text, 1000, true)),
    }
}

/// The same shape as the facet's `PagePushSub` without naming the API crate here.
pub type PagePushSubOut = iyakki_agent_api::PagePushSub;

impl iyakki_agent_api::PagePushHost for PagePush {
    fn permission(&mut self, origin: &str) -> String {
        self.permission_of(origin)
    }
    fn register(&mut self, origin: &str, scope: &str, script: &str) -> Result<(), String> {
        self.register_script(origin, scope, script)
    }
    fn unregister(&mut self, origin: &str, scope: &str) -> Result<bool, String> {
        self.remove_registration(origin, scope)
    }
    fn subscribe(&mut self, origin: &str, scope: &str, key: &str) -> Result<PagePushSubOut, String> {
        self.make_subscription(origin, scope, key)
    }
    fn get(&mut self, origin: &str, scope: &str) -> Option<PagePushSubOut> {
        self.current(origin, scope)
    }
    fn unsubscribe(&mut self, origin: &str, scope: &str) -> Result<bool, String> {
        self.drop_subscription(origin, scope)
    }
}

/// The shell's card: one question in the approval store for the signed-in person, naming the site; only "Allow this time" allows, and the
/// answer is then kept for the site (so the card is shown once per site, not on every message).
pub struct ShellOriginCard {
    services: Arc<crate::services::Services>,
    user: String,
}

pub fn card_text(origin: &str) -> (String, String) {
    (format!("Allow {origin} to send you notifications?"), format!("The page {origin} asks to send notifications to this computer through a push service. Its messages are shown in your INBOX with the site's name. Allow, or Deny; the answer is kept for this site and you can take it back with the assistant (webpush_site_forget). If nobody answers, it is denied."))
}

impl AskOrigin for ShellOriginCard {
    fn allow(&self, origin: &str) -> bool {
        use crate::browserconsent::Answer;
        let (title, text) = card_text(origin);
        let store = &self.services.browser.consent;
        let Ok(id) = store.create_device(&self.user, origin, &title, &text, crate::clock::now_unix()) else { return false };
        matches!(store.wait(&id, crate::passkeys::CARD_WAIT), Some(Answer::Allow))
    }
}

/// Shows a message as an INBOX note of the person.
pub struct InboxShow {
    services: Arc<crate::services::Services>,
    user: String,
}

impl ShowPush for InboxShow {
    fn show(&self, origin: &str, title: &str, body: &str) -> Result<(), String> {
        let host = origin.trim_start_matches("https://");
        let t = if title.trim().is_empty() { format!("Notification from {host}") } else { format!("{host}: {title}") };
        self.services.notify(&self.user, crate::inbox::Kind::Info, &t, body, None)
    }
}

/// The page-facing push of one signed-in person on this machine: their vault, the shell's card, the INBOX, the push service of this core.
pub fn page_host(services: &Arc<crate::services::Services>, user: &str, audit: AuditFn) -> Arc<Mutex<dyn iyakki_agent_api::PagePushHost>> {
    Arc::new(Mutex::new(person(services, user, audit)))
}

pub fn person(services: &Arc<crate::services::Services>, user: &str, audit: AuditFn) -> PagePush {
    let store: Arc<dyn SecretStore> = Arc::new(crate::gitkeys::ServicesStore::new(services, user));
    PagePush::new(store, Arc::new(NoService), Arc::new(ShellOriginCard { services: services.clone(), user: user.into() }), Arc::new(InboxShow { services: services.clone(), user: user.into() }), audit)
}

/// The heartbeat: fetch waiting pushes of the person. True when something was shown.
pub fn poll_tick(services: &Arc<crate::services::Services>, user: &str, audit: AuditFn) -> bool {
    person(services, user, audit).poll().map(|n| n > 0).unwrap_or(false)
}

#[cfg(test)]
#[path = "browserpush_tests.rs"]
mod tests;
