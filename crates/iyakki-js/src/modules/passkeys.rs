//! `navigator.credentials.create()` and `.get()` for passkeys (F-BROX-03): the page-facing half. The script side (`passkeys.js`)
//! checks the shapes of what the page passes, turns buffers into base64url and back, and gives the page `PublicKeyCredential` and
//! the response classes; this side only hands the request to the embedder's [`PasskeyHost`], which makes or uses the passkey after
//! the person's own approval, and tells the host which origin asked (the page cannot name another one). Only a secure page gets the
//! object at all, and a page that was given no host sees `navigator.credentials` as missing, as in a browser without passkeys.

use std::any::Any;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::engine::WebModule;

/// What makes the passkey. Both calls take the origin of the asking page and the request as the script side shaped it
/// (`rpId`, `challenge` and ids in base64url, a `rpId` of "" means the page's own host) and answer with the fields of the
/// credential in base64url (`credentialId`, `clientDataJSON` and `attestationObject`, or `authenticatorData`, `signature` and
/// `userHandle`). An `Err` is shown to the page as a refusal (`NotAllowedError` unless it begins with another error name and a colon).
pub trait PasskeyHost: Send {
    fn create(&mut self, origin: &str, request: &Value) -> Result<Value, String>;
    fn get(&mut self, origin: &str, request: &Value) -> Result<Value, String>;
}

pub struct PasskeysModule {
    origin: String,
    host_name: String,
    host: Arc<Mutex<dyn PasskeyHost>>,
}

impl PasskeysModule {
    /// `origin` is the page's own (`https://example.org`), `host_name` its host.
    pub fn new(origin: &str, host_name: &str, host: Arc<Mutex<dyn PasskeyHost>>) -> PasskeysModule {
        PasskeysModule { origin: origin.to_string(), host_name: host_name.to_string(), host }
    }
}

/// A page's origin counts as secure for passkeys when it is https, or a local address.
pub fn secure_origin(origin: &str) -> bool {
    origin.starts_with("https://") || origin.starts_with("http://localhost") || origin.starts_with("http://127.0.0.1")
}

impl WebModule for PasskeysModule {
    fn name(&self) -> &'static str {
        "passkeys"
    }

    fn prelude(&self) -> &'static str {
        include_str!("passkeys.js")
    }

    fn call(&mut self, op: &str, args: &Value) -> Result<Value, String> {
        let fail = |m: String| Ok(json!({ "error": m }));
        if !secure_origin(&self.origin) {
            return fail("SecurityError: passkeys are only for secure (https) pages".into());
        }
        // an empty relying party id is the page's own host; the host checks that the id is the page's host or a parent of it
        let mut req = args.clone();
        if req.get("rpId").and_then(Value::as_str).is_some_and(str::is_empty) {
            req["rpId"] = json!(self.host_name);
        }
        if req.to_string().len() > 64 * 1024 {
            return fail("TypeError: the request is too large".into());
        }
        let mut h = self.host.lock().map_err(|_| "passkeys are unavailable".to_string())?;
        let r = match op {
            "create" => h.create(&self.origin, &req),
            "get" => h.get(&self.origin, &req),
            other => return Err(format!("passkeys has no operation {other}")),
        };
        match r {
            Ok(v) => Ok(v),
            Err(e) => fail(e),
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

    #[derive(Default)]
    struct Fake {
        seen: Vec<(String, String, Value)>,
        answer: Option<Result<Value, String>>,
    }

    struct Shared(Arc<Mutex<Fake>>);

    impl PasskeyHost for Shared {
        fn create(&mut self, origin: &str, request: &Value) -> Result<Value, String> {
            let mut f = self.0.lock().unwrap();
            f.seen.push(("create".into(), origin.into(), request.clone()));
            f.answer.clone().unwrap_or_else(|| Ok(json!({ "credentialId": "AQID", "clientDataJSON": "e30", "attestationObject": "BAUG" })))
        }
        fn get(&mut self, origin: &str, request: &Value) -> Result<Value, String> {
            let mut f = self.0.lock().unwrap();
            f.seen.push(("get".into(), origin.into(), request.clone()));
            f.answer.clone().unwrap_or_else(|| Ok(json!({ "credentialId": "AQID", "clientDataJSON": "e30", "authenticatorData": "BwgJ", "signature": "CgsM", "userHandle": "DQ4P" })))
        }
    }

    fn engine(url: &str, fake: Arc<Mutex<Fake>>) -> JsEngine {
        let u = Url::parse(url).unwrap();
        let origin = format!("{}://{}", if u.https { "https" } else { "http" }, u.host);
        let host: Arc<Mutex<dyn PasskeyHost>> = Arc::new(Mutex::new(Shared(fake)));
        JsEngine::new(
            Limits::default(),
            vec![Box::new(ConsoleModule::new()), Box::new(TimersModule), Box::new(EncodingModule), Box::new(UrlModule), Box::new(EnvModule::new(u.clone())), Box::new(PasskeysModule::new(&origin, &u.host, host))],
        )
        .unwrap()
    }

    /// Runs a script that settles a promise into `out`, and gives `out`.
    fn run(e: &mut JsEngine, js: &str) -> String {
        e.eval("t", &format!("globalThis.out = 'pending'; (async () => {{ {js} }})().then((v) => {{ out = String(v); }}, (x) => {{ out = 'rejected ' + (x && x.name) + ': ' + (x && x.message); }});")).unwrap();
        e.run_jobs().unwrap();
        e.eval("t", "out").unwrap()
    }

    const CREATE: &str = "navigator.credentials.create({ publicKey: { challenge: new Uint8Array([1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16]), rp: { name: 'Ex' }, user: { id: new Uint8Array([9,9]), name: 'ada', displayName: 'Ada' }, pubKeyCredParams: [{ type: 'public-key', alg: -7 }] } })";

    #[test]
    fn a_page_makes_a_passkey_and_gets_buffers_back_and_the_host_sees_the_pages_own_origin() {
        let fake = Arc::new(Mutex::new(Fake::default()));
        let mut e = engine("https://login.example.org/start", fake.clone());
        let r = run(&mut e, &format!("const c = await {CREATE}; return [c.type, c.id, new Uint8Array(c.rawId).join('.'), new Uint8Array(c.response.attestationObject).join('.'), c instanceof PublicKeyCredential, c.response.getTransports()].join('|');"));
        assert_eq!(r, "public-key|AQID|1.2.3|4.5.6|true|internal");
        let f = fake.lock().unwrap();
        let (op, origin, req) = &f.seen[0];
        assert_eq!((op.as_str(), origin.as_str()), ("create", "https://login.example.org"));
        assert_eq!(req["rpId"], "login.example.org", "an empty relying party id is the page's own host");
        assert_eq!(req["challenge"], "AQIDBAUGBwgJCgsMDQ4PEA");
        assert_eq!(req["userHandle"], "CQk");
        assert_eq!(req["userName"], "ada");
    }

    #[test]
    fn signing_in_gives_the_assertion_and_passes_the_allowed_ids() {
        let fake = Arc::new(Mutex::new(Fake::default()));
        let mut e = engine("https://login.example.org/", fake.clone());
        let r = run(&mut e, "const c = await navigator.credentials.get({ publicKey: { challenge: new Uint8Array(16), rpId: 'example.org', allowCredentials: [{ type: 'public-key', id: new Uint8Array([1,2,3]) }, { type: 'other', id: new Uint8Array([7]) }] } }); return [new Uint8Array(c.response.authenticatorData).join('.'), new Uint8Array(c.response.signature).join('.'), new Uint8Array(c.response.userHandle).join('.')].join('|');");
        assert_eq!(r, "7.8.9|10.11.12|13.14.15");
        let f = fake.lock().unwrap();
        assert_eq!(f.seen[0].2["rpId"], "example.org");
        assert_eq!(f.seen[0].2["allow"], json!(["AQID"]));
    }

    #[test]
    fn a_refusal_is_a_dom_exception_the_page_can_read_and_other_errors_keep_their_names() {
        let fake = Arc::new(Mutex::new(Fake { answer: Some(Err("the person said no".into())), ..Default::default() }));
        let mut e = engine("https://a.example.org/", fake.clone());
        assert_eq!(run(&mut e, &format!("return await {CREATE};")), "rejected NotAllowedError: the person said no");
        fake.lock().unwrap().answer = Some(Err("SecurityError: this site may not use that id".into()));
        assert_eq!(run(&mut e, &format!("return await {CREATE};")), "rejected SecurityError: this site may not use that id");
    }

    #[test]
    fn what_cannot_be_done_is_said_before_the_host_is_asked() {
        let fake = Arc::new(Mutex::new(Fake::default()));
        let mut e = engine("https://a.example.org/", fake.clone());
        let no_es256 = "navigator.credentials.create({ publicKey: { challenge: new Uint8Array(16), rp: { name: 'x' }, user: { id: new Uint8Array(1), name: 'a' }, pubKeyCredParams: [{ type: 'public-key', alg: -257 }] } })";
        assert!(run(&mut e, &format!("return await {no_es256};")).starts_with("rejected NotSupportedError"));
        assert!(run(&mut e, "return await navigator.credentials.get({ mediation: 'conditional', publicKey: { challenge: new Uint8Array(16) } });").starts_with("rejected NotSupportedError"));
        assert!(run(&mut e, "return await navigator.credentials.get({ password: true });").starts_with("rejected NotSupportedError"));
        assert!(run(&mut e, "return await navigator.credentials.get({ publicKey: { challenge: 'text' } });").starts_with("rejected TypeError"));
        assert_eq!(run(&mut e, "return await PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable();"), "false");
        assert!(fake.lock().unwrap().seen.is_empty(), "the host was never asked");
    }

    #[test]
    fn an_unprotected_page_has_no_credentials_at_all() {
        let fake = Arc::new(Mutex::new(Fake::default()));
        let mut e = engine("http://plain.example.org/", fake.clone());
        assert_eq!(e.eval("t", "typeof navigator.credentials + ',' + typeof PublicKeyCredential").unwrap(), "undefined,undefined");
        // the host side refuses as well when a script reaches it directly
        let r = e.eval("t", "__host('passkeys', 'create', '{}')").unwrap();
        assert!(r.contains("SecurityError"), "{r}");
        assert!(fake.lock().unwrap().seen.is_empty());
    }
}
