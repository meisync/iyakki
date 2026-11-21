//! `PushManager` and a service worker registration for secure pages (F-INBOX-09): the page-facing half in IYAKKI's own script engine. The
//! script side (`push.js`) checks the shapes of what the page passes (an https page, a script of its own origin and a scope inside the
//! script's folder, `userVisibleOnly`, a 65-byte P-256 `applicationServerKey`), keeps the registrations of the page and gives it the classes;
//! this side hands the real work to the embedder's [`PushHost`] (the person's permission, the registration at a push service, the stored
//! subscription), tells it which origin asked (the page cannot name another one), and checks everything the host answers before the page
//! sees it (an https endpoint, a 65-byte `p256dh`, a 16-byte `auth`). A page that was given no host sees the engine's plain stub.
//!
//! The worker's script is NOT run here: a registration records that the origin has one, so the push service's messages can be delivered to
//! it later; showing a pushed message is the system's (the notification hub).

use std::any::Any;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::engine::WebModule;
use crate::modules::passkeys::secure_origin;

/// One push subscription as the host keeps it. `key` is the applicationServerKey it was made for, `p256dh` the browser's receiving public
/// key and `auth` the 16-byte secret, all in base64url without padding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushSub {
    pub endpoint: String,
    pub expiration_ms: Option<i64>,
    pub key: String,
    pub p256dh: String,
    pub auth: String,
}

/// What registers, asks the person and talks to the push service. Every call carries the origin of the asking page. An `Err` is shown to
/// the page as a refusal (`NotAllowedError` unless it begins with another error name and a colon).
pub trait PushHost: Send {
    /// `granted`, `denied` or `prompt` for this origin (what `permissionState` answers).
    fn permission(&mut self, origin: &str) -> String;
    /// Remember a service worker registration of the origin.
    fn register(&mut self, origin: &str, scope: &str, script: &str) -> Result<(), String>;
    fn unregister(&mut self, origin: &str, scope: &str) -> Result<bool, String>;
    /// Make (or return the existing) subscription of the registration for `key`. The host asks the person when the permission is `prompt`.
    fn subscribe(&mut self, origin: &str, scope: &str, key: &str) -> Result<PushSub, String>;
    fn get(&mut self, origin: &str, scope: &str) -> Option<PushSub>;
    fn unsubscribe(&mut self, origin: &str, scope: &str) -> Result<bool, String>;
}

pub struct PushModule {
    origin: String,
    host: Arc<Mutex<dyn PushHost>>,
}

impl PushModule {
    /// `origin` is the page's own (`https://example.org`).
    pub fn new(origin: &str, host: Arc<Mutex<dyn PushHost>>) -> PushModule {
        PushModule { origin: origin.to_string(), host }
    }
}

/// Base64url (no padding) into bytes; `None` for anything else.
fn b64url(s: &str) -> Option<Vec<u8>> {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let (mut acc, mut bits, mut out) = (0u32, 0u32, Vec::new());
    for c in s.bytes() {
        let v = A.iter().position(|x| *x == c)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// What the page may be shown of a subscription: the host's answer, checked.
fn checked(sub: &PushSub) -> Result<Value, String> {
    let e = &sub.endpoint;
    if !e.starts_with("https://") || e.len() > 2048 || e.chars().any(|c| c.is_control() || c.is_whitespace()) || e["https://".len()..].contains('@') {
        return Err("NotAllowedError: the push service gave an address that is not usable".into());
    }
    if b64url(&sub.p256dh).is_none_or(|k| k.len() != 65 || k[0] != 4) || b64url(&sub.auth).is_none_or(|k| k.len() != 16) || b64url(&sub.key).is_none_or(|k| k.len() != 65) {
        return Err("NotAllowedError: the push subscription's keys are not usable".into());
    }
    Ok(json!({"endpoint": sub.endpoint, "expirationTime": sub.expiration_ms, "key": sub.key, "p256dh": sub.p256dh, "auth": sub.auth}))
}

fn scope_ok(origin: &str, scope: &str) -> bool {
    scope.strip_prefix(origin).is_some_and(|rest| rest.starts_with('/')) && scope.len() <= 2048 && !scope.chars().any(char::is_control)
}

impl WebModule for PushModule {
    fn name(&self) -> &'static str {
        "push"
    }

    fn prelude(&self) -> &'static str {
        include_str!("push.js")
    }

    fn call(&mut self, op: &str, args: &Value) -> Result<Value, String> {
        let fail = |m: &str| Ok(json!({ "error": m }));
        if !secure_origin(&self.origin) {
            return fail("SecurityError: push is only for secure (https) pages");
        }
        if args.to_string().len() > 16 * 1024 {
            return fail("TypeError: the request is too large");
        }
        let scope = args["scope"].as_str().unwrap_or("");
        if op != "permission" && !scope_ok(&self.origin, scope) {
            return fail("SecurityError: the scope is not inside this page's origin");
        }
        let mut h = self.host.lock().map_err(|_| "push is unavailable".to_string())?;
        let origin = self.origin.clone();
        let r: Result<Value, String> = match op {
            "permission" => Ok(json!({ "state": h.permission(&origin) })),
            "register" => {
                let script = args["script"].as_str().unwrap_or("");
                if !scope_ok(&origin, script) {
                    return fail("SecurityError: the worker script is not inside this page's origin");
                }
                h.register(&origin, scope, script).map(|()| json!({ "ok": true }))
            }
            "unregister" => h.unregister(&origin, scope).map(|d| json!({ "done": d })),
            "subscribe" => {
                let key = args["key"].as_str().unwrap_or("");
                if b64url(key).is_none_or(|k| k.len() != 65 || k[0] != 4) {
                    return fail("InvalidAccessError: the applicationServerKey is not a P-256 public key");
                }
                h.subscribe(&origin, scope, key).and_then(|s| checked(&s)).map(|s| json!({ "sub": s }))
            }
            "get" => match h.get(&origin, scope) {
                Some(s) => checked(&s).map(|s| json!({ "sub": s })),
                None => Ok(json!({ "sub": null })),
            },
            "unsubscribe" => h.unsubscribe(&origin, scope).map(|d| json!({ "done": d })),
            other => return Err(format!("push has no operation {other}")),
        };
        match r {
            Ok(v) => Ok(v),
            Err(e) => fail(&e),
        }
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{ConsoleModule, EncodingModule, EnvModule, TimersModule, UrlModule};
    use crate::{JsEngine, Limits};
    use iyakki_web::Url;

    // a 65-byte key starting with 4, a 16-byte secret (base64url, no padding)
    fn b64(bytes: &[u8]) -> String {
        const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut o = String::new();
        for c in bytes.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            o.push(A[(n >> 18) as usize & 63] as char);
            o.push(A[(n >> 12) as usize & 63] as char);
            if c.len() > 1 {
                o.push(A[(n >> 6) as usize & 63] as char);
            }
            if c.len() > 2 {
                o.push(A[n as usize & 63] as char);
            }
        }
        o
    }
    fn app_key() -> Vec<u8> {
        let mut k = vec![4u8];
        k.extend((1..=64).map(|i| i as u8));
        k
    }

    #[derive(Default)]
    struct Fake {
        calls: Vec<String>,
        permission: String,
        subs: Vec<(String, PushSub)>,
        refuse: Option<String>,
        bad_endpoint: bool,
    }
    struct Shared(Arc<Mutex<Fake>>);
    impl PushHost for Shared {
        fn permission(&mut self, o: &str) -> String {
            let f = self.0.lock().unwrap();
            let _ = o;
            if f.permission.is_empty() { "prompt".into() } else { f.permission.clone() }
        }
        fn register(&mut self, o: &str, scope: &str, script: &str) -> Result<(), String> {
            self.0.lock().unwrap().calls.push(format!("register {o} {scope} {script}"));
            Ok(())
        }
        fn unregister(&mut self, o: &str, scope: &str) -> Result<bool, String> {
            self.0.lock().unwrap().calls.push(format!("unregister {o} {scope}"));
            Ok(true)
        }
        fn subscribe(&mut self, o: &str, scope: &str, key: &str) -> Result<PushSub, String> {
            let mut f = self.0.lock().unwrap();
            f.calls.push(format!("subscribe {o} {scope} {key}"));
            if let Some(e) = f.refuse.clone() {
                return Err(e);
            }
            let s = PushSub { endpoint: if f.bad_endpoint { "http://push.example/abc".into() } else { "https://push.example/wpush/abc".into() }, expiration_ms: None, key: key.into(), p256dh: b64(&app_key()), auth: b64(&[7u8; 16]) };
            f.subs.push((scope.into(), s.clone()));
            Ok(s)
        }
        fn get(&mut self, _: &str, scope: &str) -> Option<PushSub> {
            self.0.lock().unwrap().subs.iter().find(|(s, _)| s == scope).map(|(_, p)| p.clone())
        }
        fn unsubscribe(&mut self, _: &str, scope: &str) -> Result<bool, String> {
            let mut f = self.0.lock().unwrap();
            let n = f.subs.len();
            f.subs.retain(|(s, _)| s != scope);
            Ok(f.subs.len() != n)
        }
    }

    fn engine(url: &str, fake: Arc<Mutex<Fake>>) -> JsEngine {
        let u = Url::parse(url).unwrap();
        let origin = format!("{}://{}", if u.https { "https" } else { "http" }, u.host);
        let host: Arc<Mutex<dyn PushHost>> = Arc::new(Mutex::new(Shared(fake)));
        JsEngine::new(Limits::default(), vec![Box::new(ConsoleModule::new()), Box::new(TimersModule), Box::new(EncodingModule), Box::new(UrlModule), Box::new(EnvModule::new(u)), Box::new(PushModule::new(&origin, host))]).unwrap()
    }

    fn run(e: &mut JsEngine, js: &str) -> String {
        e.eval("t", &format!("globalThis.out = 'pending'; (async () => {{ {js} }})().then((v) => {{ out = String(v); }}, (x) => {{ out = 'rejected ' + (x && x.name) + ': ' + (x && x.message); }});")).unwrap();
        e.run_jobs().unwrap();
        e.eval("t", "out").unwrap()
    }

    fn key_js() -> String {
        format!("new Uint8Array([{}])", app_key().iter().map(|b| b.to_string()).collect::<Vec<_>>().join(","))
    }

    #[test]
    fn a_page_registers_its_worker_subscribes_and_reads_the_subscription_the_way_a_browser_gives_it() {
        let fake = Arc::new(Mutex::new(Fake { permission: "granted".into(), ..Default::default() }));
        let mut e = engine("https://shop.example.org/app/index.html", fake.clone());
        let r = run(&mut e, &format!("const reg = await navigator.serviceWorker.register('/app/sw.js'); const s = await reg.pushManager.subscribe({{ userVisibleOnly: true, applicationServerKey: {} }}); const j = s.toJSON(); return [reg.scope, reg.active.state, s.endpoint, new Uint8Array(s.getKey('p256dh')).length, new Uint8Array(s.getKey('auth')).join('.').slice(0, 7), String(s.getKey('other')), j.keys.p256dh.length, String(j.expirationTime), s.options.userVisibleOnly, new Uint8Array(s.options.applicationServerKey).length, s instanceof PushSubscription].join('|');", key_js()));
        assert_eq!(r, "https://shop.example.org/app/|activated|https://push.example/wpush/abc|65|7.7.7.7|null|87|null|true|65|true");
        let f = fake.lock().unwrap();
        assert_eq!(f.calls[0], "register https://shop.example.org https://shop.example.org/app/ https://shop.example.org/app/sw.js");
        assert!(f.calls[1].starts_with("subscribe https://shop.example.org https://shop.example.org/app/ "), "the host sees the page's own origin and the registration's scope");
    }

    #[test]
    fn ready_get_subscription_permission_state_and_unsubscribe_follow_the_registration() {
        let fake = Arc::new(Mutex::new(Fake { permission: "granted".into(), ..Default::default() }));
        let mut e = engine("https://a.example.org/", fake.clone());
        let r = run(
            &mut e,
            &format!("const p = navigator.serviceWorker.ready; await navigator.serviceWorker.register('/sw.js'); const reg = await p; const before = await reg.pushManager.getSubscription(); const st = await reg.pushManager.permissionState({{ userVisibleOnly: true }}); await reg.pushManager.subscribe({{ userVisibleOnly: true, applicationServerKey: {} }}); const mid = await reg.pushManager.getSubscription(); const gone = await mid.unsubscribe(); const after = await reg.pushManager.getSubscription(); return [String(before), st, mid.endpoint, gone, String(after), PushManager.supportedContentEncodings.join(), (await navigator.serviceWorker.getRegistrations()).length].join('|');", key_js()),
        );
        assert_eq!(r, "null|granted|https://push.example/wpush/abc|true|null|aes128gcm|1");
    }

    #[test]
    fn what_a_browser_would_refuse_is_refused_before_the_host_is_asked() {
        let fake = Arc::new(Mutex::new(Fake::default()));
        let mut e = engine("https://a.example.org/dir/page.html", fake.clone());
        run(&mut e, "await navigator.serviceWorker.register('/dir/sw.js', { scope: '/dir/' });");
        let n = fake.lock().unwrap().calls.len();
        assert_eq!(n, 1);
        let sub = |o: &str| format!("const reg = await navigator.serviceWorker.getRegistration('/dir/'); return await reg.pushManager.subscribe({o});");
        assert!(run(&mut e, &sub(&format!("{{ applicationServerKey: {} }}", key_js()))).starts_with("rejected NotSupportedError"), "userVisibleOnly is required");
        assert!(run(&mut e, &sub("{ userVisibleOnly: true }")).starts_with("rejected InvalidAccessError"), "a key is required");
        assert!(run(&mut e, &sub("{ userVisibleOnly: true, applicationServerKey: new Uint8Array(65) }")).starts_with("rejected InvalidAccessError"), "a key that does not start with 4");
        assert!(run(&mut e, &sub("{ userVisibleOnly: true, applicationServerKey: new Uint8Array(33) }")).starts_with("rejected InvalidAccessError"));
        assert!(run(&mut e, &sub("{ userVisibleOnly: true, applicationServerKey: 42 }")).starts_with("rejected TypeError"));
        assert!(run(&mut e, "return await navigator.serviceWorker.register('https://other.example/sw.js');").starts_with("rejected SecurityError"), "a script from another origin");
        assert!(run(&mut e, "return await navigator.serviceWorker.register('/dir/sw.js', { scope: '/' });").starts_with("rejected SecurityError"), "a scope wider than the script's folder");
        assert!(run(&mut e, "return await navigator.serviceWorker.register('/dir/sw.js', { scope: 'https://other.example/dir/' });").starts_with("rejected SecurityError"));
        assert_eq!(fake.lock().unwrap().calls.len(), n, "the host was never asked");
    }

    #[test]
    fn a_refusal_of_the_person_or_the_service_is_a_dom_exception_and_a_bad_answer_never_reaches_the_page() {
        let fake = Arc::new(Mutex::new(Fake { refuse: Some("the person said no".into()), ..Default::default() }));
        let mut e = engine("https://a.example.org/", fake.clone());
        let sub = format!("const reg = await navigator.serviceWorker.register('/sw.js'); return await reg.pushManager.subscribe({{ userVisibleOnly: true, applicationServerKey: {} }});", key_js());
        assert_eq!(run(&mut e, &sub), "rejected NotAllowedError: the person said no");
        fake.lock().unwrap().refuse = Some("NotSupportedError: this browser has no push service".into());
        assert_eq!(run(&mut e, &sub), "rejected NotSupportedError: this browser has no push service");
        fake.lock().unwrap().refuse = None;
        fake.lock().unwrap().bad_endpoint = true;
        assert!(run(&mut e, &sub).contains("not usable"), "an endpoint that is not https is not given to the page");
        assert!(fake.lock().unwrap().subs.iter().all(|(_, s)| s.endpoint.starts_with("http://")), "the bad one was the host's own");
    }

    #[test]
    fn checked_answers_need_an_https_endpoint_and_keys_of_the_right_size() {
        let good = PushSub { endpoint: "https://p.example/x".into(), expiration_ms: Some(5), key: b64(&app_key()), p256dh: b64(&app_key()), auth: b64(&[1; 16]) };
        assert!(checked(&good).is_ok());
        for bad in [
            PushSub { endpoint: "http://p.example/x".into(), ..good.clone() },
            PushSub { endpoint: "https://u:p@p.example/x".into(), ..good.clone() },
            PushSub { endpoint: format!("https://p.example/{}", "a".repeat(2100)), ..good.clone() },
            PushSub { endpoint: "https://p.example/a b".into(), ..good.clone() },
            PushSub { p256dh: b64(&[4; 64]), ..good.clone() },
            PushSub { p256dh: b64(&[2; 65]), ..good.clone() },
            PushSub { auth: b64(&[1; 15]), ..good.clone() },
            PushSub { key: "!!".into(), ..good.clone() },
        ] {
            assert!(checked(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn an_unprotected_page_keeps_the_plain_stub_and_the_host_side_refuses_a_direct_call() {
        let fake = Arc::new(Mutex::new(Fake::default()));
        let mut e = engine("http://plain.example.org/", fake.clone());
        assert_eq!(e.eval("t", "typeof PushManager").unwrap(), "undefined");
        let r = e.eval("t", "__host('push', 'permission', '{}')").unwrap();
        assert!(r.contains("SecurityError"), "{r}");
        assert!(fake.lock().unwrap().calls.is_empty());
    }

    #[test]
    fn scopes_outside_the_origin_are_refused_by_the_host_side_even_for_a_script_that_calls_it_directly() {
        let fake = Arc::new(Mutex::new(Fake::default()));
        let mut e = engine("https://a.example.org/", fake.clone());
        for s in ["https://evil.example/", "https://a.example.org.evil.example/", "", "https://a.example.orgx/"] {
            let r = e.eval("t", &format!("__host('push', 'get', JSON.stringify({{scope: '{s}'}}))")).unwrap();
            assert!(r.contains("SecurityError"), "{s}: {r}");
        }
        assert!(fake.lock().unwrap().calls.is_empty());
    }
}
